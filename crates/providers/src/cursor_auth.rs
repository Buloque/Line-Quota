//! Cookie de sessão do Cursor colado pelo usuário (`WorkosCursorSessionToken`), para ler o uso do plano.
//! O Cursor não tem API oficial de uso para contas individuais: usa-se o mesmo endereço do painel web,
//! com o cookie que o próprio usuário copia do navegador. O valor é guardado com DPAPI (só este usuário do
//! Windows abre), nunca aparece em logs nem em mensagens, e só é enviado a `cursor.com`.
use crate::secure;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

const COOKIE_NAME: &str = "WorkosCursorSessionToken";
const MIN_LEN: usize = 20;
/// A área de transferência é lida com limite de 2 KB: um valor maior que isso veio truncado.
const MAX_LEN: usize = 2000;

static GENERATION: AtomicU32 = AtomicU32::new(0);
static STORE: Mutex<()> = Mutex::new(());

/// Muda a cada gravação/remoção; quem guarda cache de uso o descarta quando muda.
pub fn generation() -> u32 { GENERATION.load(Ordering::Relaxed) }

fn store_path() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("LineQuota").join("cursor-auth.bin")
}

/// Extrai o valor do cookie do que foi colado: aceita só o valor, `Nome=valor`, ou uma linha inteira de
/// cookies (`a=b; WorkosCursorSessionToken=valor; c=d`), com ou sem o prefixo `Cookie:`.
pub fn parse_cookie(pasted: &str) -> Result<String, String> {
    let s = pasted.trim().trim_start_matches("Cookie:").trim_start_matches("cookie:").trim();
    let value = match s.split(';').map(str::trim).find_map(|p| p.strip_prefix(&format!("{COOKIE_NAME}="))) {
        Some(v) => v,
        None if !s.contains('=') && !s.contains(';') => s, // só o valor
        None => return Err(format!("Não encontrei o cookie {COOKIE_NAME} no texto copiado.")),
    }.trim().trim_matches('"');
    let ok = |c: char| c.is_ascii_alphanumeric() || "-_.~%:+/=".contains(c);
    if value.len() < MIN_LEN || value.len() > MAX_LEN || !value.chars().all(ok) {
        return Err("O texto copiado não parece o cookie do Cursor (valor do WorkosCursorSessionToken).".into());
    }
    Ok(value.to_string())
}

/// O texto copiado tem cara de cookie de sessão do Cursor (`<id>%3A%3A<token>`)? Usado para detectar a cópia
/// na área de transferência sem salvar qualquer texto que o usuário copie.
pub fn looks_like_session(text: &str) -> bool {
    parse_cookie(text).is_ok_and(|v| v.contains("%3A%3A") || v.contains("::"))
}

/// Guarda o cookie (criptografado com DPAPI). Aceita o texto como foi copiado: extrai só o valor.
pub fn save(pasted: &str) -> Result<(), String> {
    let value = parse_cookie(pasted)?;
    let _g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let p = store_path();
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    let enc = secure::protect(value.as_bytes())?;
    let tmp = p.with_extension("bin.tmp");
    std::fs::write(&tmp, enc).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| { let _ = std::fs::remove_file(&tmp); e.to_string() })?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn load() -> Option<String> {
    let _g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let enc = std::fs::read(store_path()).ok()?;
    String::from_utf8(secure::unprotect(&enc).ok()?).ok()
}

pub fn clear() {
    let _g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_file(store_path());
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn connected() -> bool { store_path().is_file() }

#[cfg(test)]
mod tests {
    /// O cookie vai num cabeçalho HTTP: nada que quebre linha, feche aspas ou separe cookies pode passar.
    #[test]
    fn header_injection_and_garbage_are_rejected() {
        use super::*;
        let good = "user_01ABCDEFGHIJKLMNOP%3A%3AeyJhbGciOi.payload.sig";
        assert!(parse_cookie(good).is_ok());
        let long = "A".repeat(5000);
        for bad in ["abc\r\nX-Evil: 1", "user_01ABCDEFGHIJKLMNOP%3A%3Aey\nJh", "a b c d e f g h i j k l m n o p", "x;y;z", "Cookie: other=1", "\"\"", "",
                    "WorkosCursorSessionToken=short", long.as_str(), "user\u{0}01ABCDEFGHIJKLMNOP%3A%3Aeyabc", "<script>alert(1)</script>aaaaaaaaaaaa"] {
            assert!(!looks_like_session(bad), "aceitou: {bad:?}");
        }
        assert_eq!(parse_cookie(&format!("a=b; WorkosCursorSessionToken={good}; c=d")).unwrap(), good);
    }

    use super::*;

    const V: &str = "user_01ABCDEF0123456789%3A%3AeyJhbGciOiJIUzI1NiJ9.payload.sig";

    #[test]
    fn accepts_value_name_value_and_full_cookie_lines() {
        assert_eq!(parse_cookie(V).unwrap(), V);
        assert_eq!(parse_cookie(&format!("  WorkosCursorSessionToken={V}\n")).unwrap(), V);
        assert_eq!(parse_cookie(&format!("Cookie: a=b; WorkosCursorSessionToken={V}; c=d")).unwrap(), V);
        assert_eq!(parse_cookie(&format!("WorkosCursorSessionToken=\"{V}\"")).unwrap(), V);
    }

    #[test]
    fn rejects_junk_short_long_and_other_cookies() {
        assert!(parse_cookie("").is_err());
        assert!(parse_cookie("curto").is_err());
        assert!(parse_cookie("a=b; c=d").is_err()); // outros cookies, sem o do Cursor
        assert!(parse_cookie(&format!("WorkosCursorSessionToken={V} com espaco")).is_err());
        assert!(parse_cookie(&"a".repeat(MAX_LEN + 1)).is_err());
        assert!(parse_cookie("<script>alert(1)</script>-----------").is_err());
    }
}
