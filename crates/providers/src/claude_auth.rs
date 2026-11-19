//! Login do Claude pelo próprio widget (OAuth + PKCE), sem precisar do `claude` no terminal.
//! Endpoints e client_id são os do Claude Code (não documentados oficialmente).
//! O token é guardado com DPAPI e nunca aparece em logs nem em mensagens de erro.
use crate::pkce::{base64url, challenge, hex, percent_encode, random_bytes};
use crate::secure;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
/// Mínimo para ler o uso. `FULL` só se o servidor recusar o mínimo (config `account.claude_full_scope`).
const SCOPE_MIN: &str = "user:profile";
const SCOPE_FULL: &str = "user:inference user:profile user:file_upload user:mcp_servers user:sessions:claude_code";
const REFRESH_MARGIN_MS: i64 = 120_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Tokens { pub access: String, pub refresh: String, pub expires_at_ms: i64 }

struct Pending { verifier: String, state: String }
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Muda a cada login/logout; quem guarda cache de uso o descarta quando muda.
pub fn generation() -> u32 { GENERATION.load(Ordering::Relaxed) }

fn store_path() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("LineQuota").join("claude-auth.bin")
}

// ---------- Partes puras ----------

pub fn authorize_url(challenge: &str, state: &str, full_scope: bool) -> String {
    let scope = if full_scope { SCOPE_FULL } else { SCOPE_MIN };
    format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={CLIENT_ID}&redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}",
        percent_encode(REDIRECT_URI), percent_encode(scope)
    )
}

/// A página mostra `CÓDIGO#STATE`. Retorna (código, state se presente).
pub fn parse_pasted(s: &str) -> Result<(String, Option<String>), String> {
    let s = s.trim();
    let ok = |t: &str| !t.is_empty() && t.len() < 512 && t.chars().all(|c| c.is_ascii_alphanumeric() || "-_.~%".contains(c));
    let (code, state) = match s.split_once('#') { Some((c, st)) => (c, Some(st)), None => (s, None) };
    if !ok(code) || state.is_some_and(|x| !ok(x)) {
        return Err("O texto copiado não parece um código do Claude (formato CÓDIGO#STATE)".into());
    }
    Ok((code.to_string(), state.map(str::to_string)))
}

pub fn parse_token_response(json: &str, now_ms: i64, old_refresh: Option<&str>) -> Result<Tokens, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "resposta de login inválida".to_string())?;
    let access = v["access_token"].as_str().ok_or("resposta de login sem token")?;
    let refresh = v["refresh_token"].as_str().or(old_refresh).ok_or("resposta de login sem refresh token")?;
    let secs = v["expires_in"].as_i64().unwrap_or(3600);
    Ok(Tokens { access: access.into(), refresh: refresh.into(), expires_at_ms: now_ms + secs * 1000 })
}

pub fn needs_refresh(t: &Tokens, now_ms: i64) -> bool { t.expires_at_ms - now_ms < REFRESH_MARGIN_MS }

// ---------- Armazenamento ----------

fn save(t: &Tokens) -> Result<(), String> {
    let plain = json!({"a": t.access, "r": t.refresh, "e": t.expires_at_ms}).to_string();
    let enc = secure::protect(plain.as_bytes())?;
    let p = store_path();
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    std::fs::write(p, enc).map_err(|_| "não foi possível salvar o login".to_string())
}

fn load() -> Option<Result<Tokens, String>> {
    let enc = std::fs::read(store_path()).ok()?;
    Some((|| {
        let plain = secure::unprotect(&enc)?;
        let v: Value = serde_json::from_slice(&plain).map_err(|_| "login salvo corrompido".to_string())?;
        Ok(Tokens { access: v["a"].as_str().unwrap_or("").into(), refresh: v["r"].as_str().unwrap_or("").into(), expires_at_ms: v["e"].as_i64().unwrap_or(0) })
    })())
}

pub fn logged_in() -> bool { store_path().exists() }

pub fn logout() {
    *PENDING.lock().unwrap() = None;
    let _ = std::fs::remove_file(store_path());
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

// ---------- Rede ----------

/// Mensagem segura de erro: só o código de erro do servidor (ex.: invalid_grant), nunca o corpo inteiro.
fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(c, resp) => {
            let code = resp.into_string().ok().and_then(|b| serde_json::from_str::<Value>(&b).ok())
                .and_then(|v| v["error"].as_str().map(str::to_string).or_else(|| v["error"]["type"].as_str().map(str::to_string)));
            match code { Some(k) if k.len() < 40 => format!("HTTP {c} ({k})"), _ => format!("HTTP {c}") }
        }
        ureq::Error::Transport(_) => "sem conexão".to_string(),
    }
}

fn token_call(body: Result<ureq::Response, ureq::Error>) -> Result<String, String> {
    body.map_err(http_err)?.into_string().map_err(|_| "resposta ilegível".to_string())
}

fn exchange(code: &str, state: &str, verifier: &str, now_ms: i64) -> Result<Tokens, String> {
    let body = ureq::post(TOKEN_URL).timeout(Duration::from_secs(15)).set("anthropic-version", "2023-06-01")
        .send_json(json!({
        "grant_type": "authorization_code", "code": code, "state": state,
        "redirect_uri": REDIRECT_URI, "client_id": CLIENT_ID, "code_verifier": verifier,
    }));
    let txt = token_call(body).map_err(|e| format!("Falha no login: {e}"))?;
    parse_token_response(&txt, now_ms, None)
}

fn refresh(old: &Tokens, now_ms: i64) -> Result<Tokens, String> {
    let body = ureq::post(TOKEN_URL).timeout(Duration::from_secs(15))
        .send_form(&[("grant_type", "refresh_token"), ("refresh_token", &old.refresh), ("client_id", CLIENT_ID)]);
    let txt = token_call(body).map_err(|e| format!("Claude: sessão expirada ({e}) — entre novamente"))?;
    parse_token_response(&txt, now_ms, Some(&old.refresh))
}

// ---------- API usada pelo app ----------

/// Passo 1: gera PKCE/state e devolve a URL para abrir no navegador.
pub fn begin(full_scope: bool) -> Result<String, String> {
    let verifier = base64url(&random_bytes::<32>()?);
    let state = hex(&random_bytes::<16>()?);
    let url = authorize_url(&challenge(&verifier), &state, full_scope);
    *PENDING.lock().unwrap() = Some(Pending { verifier, state });
    Ok(url)
}

/// Passo 2: recebe o texto colado, valida o state, troca por token e salva.
pub fn complete(pasted: &str, now_ms: i64) -> Result<(), String> {
    let pending = PENDING.lock().unwrap().take().ok_or("Clique primeiro em \"Entrar no Claude\"")?;
    let (code, state) = parse_pasted(pasted)?;
    if state.as_deref().is_some_and(|s| s != pending.state) {
        return Err("O código não corresponde a este login (state diferente). Tente entrar de novo.".into());
    }
    let t = exchange(&code, &pending.state, &pending.verifier, now_ms)?;
    save(&t)?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Token de acesso válido do login do widget (renova se perto de vencer).
/// `None` = não há login do widget; `Some(Err)` = há, mas falhou.
pub fn access_token(now_ms: i64) -> Option<Result<String, String>> {
    let t = match load()? { Ok(t) => t, Err(e) => return Some(Err(e)) };
    if !needs_refresh(&t, now_ms) { return Some(Ok(t.access)); }
    Some(refresh(&t, now_ms).and_then(|n| { save(&n)?; Ok(n.access) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Os testes que mexem na pendência global (PENDING) não podem rodar em paralelo.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn url_has_pkce_and_minimal_scope() {
        let u = authorize_url("CH", "ST", false);
        assert!(u.starts_with("https://claude.ai/oauth/authorize?response_type=code&client_id="));
        assert!(u.contains("scope=user%3Aprofile&"));
        assert!(u.contains("code_challenge=CH&code_challenge_method=S256&state=ST"));
        assert!(u.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"));
        assert!(authorize_url("CH", "ST", true).contains("user%3Ainference"));
    }

    #[test]
    fn pasted_code_parsing() {
        assert_eq!(parse_pasted("  abc-DEF_1#st4te \n").unwrap(), ("abc-DEF_1".into(), Some("st4te".into())));
        assert_eq!(parse_pasted("abc").unwrap(), ("abc".into(), None));
        assert!(parse_pasted("").is_err());
        assert!(parse_pasted("rm -rf /; echo").is_err());
        assert!(parse_pasted(&"a".repeat(600)).is_err());
    }

    #[test]
    fn token_response_and_refresh_logic() {
        let t = parse_token_response(r#"{"access_token":"A","refresh_token":"R","expires_in":3600}"#, 1000, None).unwrap();
        assert_eq!(t, Tokens { access: "A".into(), refresh: "R".into(), expires_at_ms: 3_601_000 });
        // refresh sem novo refresh_token mantém o antigo
        let n = parse_token_response(r#"{"access_token":"B","expires_in":60}"#, 0, Some("R")).unwrap();
        assert_eq!((n.access.as_str(), n.refresh.as_str()), ("B", "R"));
        assert!(parse_token_response(r#"{"access_token":"B"}"#, 0, None).is_err());
        assert!(!needs_refresh(&t, 1000));
        assert!(needs_refresh(&t, 3_600_000 - 100_000));
    }

    #[test]
    fn complete_rejects_wrong_state_without_network() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *PENDING.lock().unwrap() = Some(Pending { verifier: "v".into(), state: "good".into() });
        let e = complete("code#evil", 0).unwrap_err();
        assert!(e.contains("state"));
        assert!(PENDING.lock().unwrap().is_none()); // pendência é consumida
        assert!(complete("code#good", 0).is_err()); // sem pendência
    }

    #[test]
    fn begin_creates_distinct_pending_each_time() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = begin(false).unwrap();
        let b = begin(false).unwrap();
        assert_ne!(a, b);
    }
}
