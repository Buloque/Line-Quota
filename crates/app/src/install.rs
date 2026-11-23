//! Instala/remove os hooks e o statusLine do Claude Code em ~/.claude/settings.json.
use serde_json::{json, Value};
use std::path::PathBuf;

const MARKER: &str = "--hook claude";
const STATUS_MARKER: &str = "--statusline";
const EVENTS: [&str; 5] = ["UserPromptSubmit", "PostToolUse", "Notification", "Stop", "SessionEnd"];

fn settings_path() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".claude").join("settings.json")
}

/// Caminho com `/` para funcionar também no shell (bash) que o Claude Code usa no Windows.
fn exe_cmd() -> String {
    let p = std::env::current_exe().unwrap_or_default().to_string_lossy().replace('\\', "/");
    format!("\"{p}\"")
}

fn has_cmd(entry: &Value, marker: &str) -> bool {
    entry["hooks"].as_array().is_some_and(|h| h.iter().any(|x| x["command"].as_str().is_some_and(|c| c.contains(marker))))
}

/// Reaponta (ou adiciona) o comando `marker` de uma lista de hooks. Retorna true se mudou algo.
fn upsert(list: &mut Vec<Value>, marker: &str, command: &str) -> bool {
    let mut found = false;
    let mut changed = false;
    for e in list.iter_mut() {
        if let Some(hs) = e["hooks"].as_array_mut() {
            for h in hs.iter_mut() {
                if h["command"].as_str().is_some_and(|c| c.contains(marker)) {
                    found = true;
                    if h["command"] != command { h["command"] = json!(command); changed = true; }
                }
            }
        }
    }
    if !found { list.push(json!({"hooks": [{"type": "command", "command": command}]})); changed = true; }
    changed
}

/// Aplica a instalação em um objeto de settings. Retorna mensagens do que foi feito.
/// Hooks já existentes do Line Quota são reapontados para `exe` (útil ao reinstalar em outro caminho).
pub fn apply(settings: &mut Value, exe: &str) -> Vec<String> {
    let mut log = Vec::new();
    if !settings.is_object() { *settings = json!({}); }
    // Estruturas inesperadas no arquivo do usuário (hooks que não é objeto, evento que não é lista) são
    // preservadas: avisa e pula, em vez de derrubar o processo ou sobrescrever.
    match settings.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({})).as_object_mut() {
        None => log.push("AVISO: \"hooks\" no settings.json não é um objeto; hooks não instalados".into()),
        Some(hooks) => for ev in EVENTS {
            match hooks.entry(ev).or_insert_with(|| json!([])).as_array_mut() {
                None => log.push(format!("AVISO: hook {ev} no settings.json não é uma lista; ignorado")),
                Some(list) => if upsert(list, MARKER, &format!("{exe} {MARKER}")) { log.push(format!("hook {ev} configurado")); },
            }
        },
    }
    let sl = &settings["statusLine"];
    if sl.is_null() {
        settings["statusLine"] = json!({"type": "command", "command": format!("{exe} {STATUS_MARKER}")});
        log.push("statusLine configurado".into());
    } else if sl["command"].as_str().is_some_and(|c| c.contains(STATUS_MARKER)) {
        let cmd = format!("{exe} {STATUS_MARKER}");
        if sl["command"] != cmd.as_str() { settings["statusLine"]["command"] = json!(cmd); log.push("statusLine reapontado".into()); }
    } else {
        log.push("AVISO: você já tem um statusLine; o limite do Claude só aparece no Line Quota se ele chamar `line-quota --statusline`".into());
    }
    log
}

pub fn remove(settings: &mut Value) {
    if let Some(h) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_, arr) in h.iter_mut() {
            if let Some(l) = arr.as_array_mut() { l.retain(|e| !has_cmd(e, MARKER)); }
        }
        h.retain(|_, v| v.as_array().is_none_or(|l| !l.is_empty()));
    }
    if settings["statusLine"]["command"].as_str().is_some_and(|c| c.contains(STATUS_MARKER)) {
        settings.as_object_mut().unwrap().remove("statusLine");
    }
}

/// O Claude Code é opcional: sem a pasta `~/.claude` ele não está instalado (e não há o que configurar).
fn claude_present(p: &std::path::Path) -> bool { p.parent().is_some_and(|d| d.is_dir()) }

fn edit(f: impl FnOnce(&mut Value) -> Vec<String>) -> Result<Vec<String>, String> {
    edit_at(&settings_path(), f)
}

fn edit_at(p: &std::path::Path, f: impl FnOnce(&mut Value) -> Vec<String>) -> Result<Vec<String>, String> {
    if !claude_present(p) {
        return Ok(vec!["Claude Code não encontrado: integração com o Claude ignorada (o widget funciona sem ele)".into()]);
    }
    // Só "arquivo inexistente" vira `{}`; qualquer outro erro de leitura aborta sem tocar no arquivo do usuário.
    let txt = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(format!("não consegui ler o settings.json ({e}); nada foi alterado")),
    };
    let mut v: Value = serde_json::from_str(txt.trim_start_matches('\u{feff}')).map_err(|e| format!("settings.json inválido: {e}"))?;
    let before = v.clone();
    let log = f(&mut v);
    if v == before { return Ok(log); } // nada mudou: não reescreve (evita corrida com o Claude Code)
    // Backup do original uma única vez (reinstalar/atualizar não o sobrescreve com o arquivo já modificado).
    let bak = p.with_extension("json.lq-bak");
    if p.is_file() && !bak.exists() { let _ = std::fs::write(&bak, &txt); }
    // Gravação atômica: arquivo temporário na mesma pasta + rename.
    let tmp = p.with_extension("json.lq-tmp");
    let out = serde_json::to_string_pretty(&v).unwrap();
    std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| { let _ = std::fs::remove_file(&tmp); e.to_string() })?;
    Ok(log)
}

pub fn install() -> Result<Vec<String>, String> { edit(|v| apply(v, &exe_cmd())) }

/// Instala os hooks apontando para um executável específico (o instalado).
pub fn install_for(exe: &std::path::Path) -> Result<Vec<String>, String> {
    let cmd = format!("\"{}\"", exe.to_string_lossy().replace('\\', "/"));
    edit(|v| apply(v, &cmd))
}

pub fn uninstall() -> Result<Vec<String>, String> {
    let p = settings_path();
    if !p.is_file() { return Ok(Vec::new()); } // nada a desfazer: não cria settings.json à toa
    edit_at(&p, |v| { remove(v); vec!["hooks do Line Quota removidos".into()] })
}

// ---------------------------------------------------------------------------------------------
// Cursor: hooks em ~/.cursor/hooks.json (`{"version": 1, "hooks": {evento: [{"command": ...}]}}`).
// Só os eventos que não bloqueiam nada: o hook sempre responde que pode seguir.
// ---------------------------------------------------------------------------------------------
const CURSOR_MARKER: &str = "--hook cursor";
const CURSOR_EVENTS: [&str; 4] = ["beforeSubmitPrompt", "postToolUse", "stop", "sessionEnd"];

fn cursor_hooks_path() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".cursor").join("hooks.json")
}

/// Comando de um hook do Cursor. No Windows o Cursor roda `Get-Content entrada.json | & { $input | <comando> }` pelo
/// PowerShell (por isso o comando NÃO começa com `$input |`: já vem pipeado). O PowerShell não espera nem captura
/// a saída de programas "de janela" (como o app): o `| Out-Null` força a espera e a resposta é dada pelo próprio
/// PowerShell (`{"continue":true}` em beforeSubmitPrompt, `{}` nos demais), sem depender da saída do programa.
/// Sem aspas duplas no comando (caminho entre aspas simples e `[char]34` na resposta), para sobreviver a
/// qualquer forma de repasse pela linha de comando.
fn cursor_command(exe: &str, event: &str) -> String {
    let reply = if event == "beforeSubmitPrompt" { "'{' + [char]34 + 'continue' + [char]34 + ':true}'" } else { "'{}'" };
    format!("& '{}' {CURSOR_MARKER} | Out-Null; {reply}", exe.replace(['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'], "''"))
}


/// Aplica os hooks do Cursor num objeto `hooks.json`. Estruturas inesperadas são preservadas (avisa e pula).
pub fn apply_cursor(cfg: &mut Value, exe: &str) -> Vec<String> {
    let mut log = Vec::new();
    if !cfg.is_object() { *cfg = json!({}); }
    let obj = cfg.as_object_mut().unwrap();
    obj.entry("version").or_insert_with(|| json!(1));
    match obj.entry("hooks").or_insert_with(|| json!({})).as_object_mut() {
        None => log.push("AVISO: \"hooks\" no hooks.json do Cursor não é um objeto; hooks não instalados".into()),
        Some(hooks) => for ev in CURSOR_EVENTS {
            match hooks.entry(ev).or_insert_with(|| json!([])).as_array_mut() {
                None => log.push(format!("AVISO: hook {ev} do Cursor não é uma lista; ignorado")),
                Some(list) => {
                    let command = cursor_command(exe, ev);
                    let mut found = false;
                    let mut changed = false;
                    for e in list.iter_mut() {
                        if e["command"].as_str().is_some_and(|c| c.contains(CURSOR_MARKER)) {
                            found = true;
                            if e["command"] != command.as_str() { e["command"] = json!(command); changed = true; }
                        }
                    }
                    if !found { list.push(json!({"command": command})); changed = true; }
                    if changed { log.push(format!("hook {ev} do Cursor configurado")); }
                }
            }
        },
    }
    log
}

pub fn remove_cursor(cfg: &mut Value) {
    if let Some(h) = cfg.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_, arr) in h.iter_mut() {
            if let Some(l) = arr.as_array_mut() { l.retain(|e| !e["command"].as_str().is_some_and(|c| c.contains(CURSOR_MARKER))); }
        }
        h.retain(|_, v| v.as_array().is_none_or(|l| !l.is_empty()));
    }
}

fn edit_cursor_at(p: &std::path::Path, f: impl FnOnce(&mut Value) -> Vec<String>) -> Result<Vec<String>, String> {
    // Sem a pasta ~/.cursor o Cursor não está instalado: não cria nada.
    if !p.parent().is_some_and(|d| d.is_dir()) {
        return Ok(vec!["Cursor não encontrado: integração com o Cursor ignorada".into()]);
    }
    let txt = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(format!("não consegui ler o hooks.json do Cursor ({e}); nada foi alterado")),
    };
    let t = txt.trim_start_matches('\u{feff}');
    let mut v: Value = serde_json::from_str(if t.trim().is_empty() { "{}" } else { t }) // arquivo vazio = {}
        .map_err(|e| format!("hooks.json do Cursor inválido: {e}"))?;
    let before = v.clone();
    let log = f(&mut v);
    if v == before { return Ok(log); }
    let bak = p.with_extension("json.lq-bak");
    if p.is_file() && !bak.exists() { let _ = std::fs::write(&bak, &txt); }
    let tmp = p.with_extension("json.lq-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap()).and_then(|_| std::fs::rename(&tmp, p))
        .map_err(|e| { let _ = std::fs::remove_file(&tmp); e.to_string() })?;
    Ok(log)
}

/// Instala os hooks do Cursor apontando para o executável informado (o instalado).
pub fn install_cursor_for(exe: &std::path::Path) -> Result<Vec<String>, String> {
    let exe = exe.to_string_lossy().replace('\\', "/");
    edit_cursor_at(&cursor_hooks_path(), |v| apply_cursor(v, &exe))
}

pub fn uninstall_cursor() -> Result<Vec<String>, String> {
    let p = cursor_hooks_path();
    if !p.is_file() { return Ok(Vec::new()); }
    edit_cursor_at(&p, |v| { remove_cursor(v); vec!["hooks do Line Quota removidos do Cursor".into()] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_preserves_existing() {
        let mut s = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}, "x": 1});
        let first = apply(&mut s, "\"a.exe\"");
        assert!(!first.is_empty());
        assert!(apply(&mut s, "\"a.exe\"").is_empty());
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(s["x"], 1);
        assert!(s["statusLine"]["command"].as_str().unwrap().ends_with("--statusline"));
    }

    #[test]
    fn existing_statusline_is_kept_with_warning() {
        let mut s = json!({"statusLine": {"type": "command", "command": "mine"}});
        assert!(apply(&mut s, "\"a.exe\"").iter().any(|l| l.contains("AVISO")));
        assert_eq!(s["statusLine"]["command"], "mine");
    }

    #[test]
    fn remove_restores_clean_state() {
        let mut s = json!({});
        apply(&mut s, "\"a.exe\"");
        remove(&mut s);
        assert!(s["hooks"].as_object().unwrap().is_empty());
        assert!(s["statusLine"].is_null());
    }
}

#[cfg(test)]
mod optional_claude_tests {
    use super::*;

    #[test]
    fn without_claude_installed_nothing_fails_and_nothing_is_created() {
        let d = std::env::temp_dir().join(format!("lq-noclaude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join(".claude").join("settings.json"); // a pasta .claude não existe
        let log = edit_at(&p, |v| apply(v, "\"a.exe\"")).unwrap();
        assert!(log[0].contains("Claude Code não encontrado"));
        assert!(!d.exists());
    }

    #[test]
    fn with_claude_folder_the_settings_file_is_created() {
        let d = std::env::temp_dir().join(format!("lq-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        let p = d.join(".claude").join("settings.json");
        edit_at(&p, |v| apply(v, "\"a.exe\"")).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("--hook claude"));
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod repoint_tests {
    use super::*;

    #[test]
    fn reinstall_repoints_existing_hooks() {
        let mut s = json!({});
        apply(&mut s, "\"old.exe\"");
        let log = apply(&mut s, "\"new.exe\"");
        assert!(!log.is_empty());
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "\"new.exe\" --hook claude");
        assert_eq!(s["statusLine"]["command"], "\"new.exe\" --statusline");
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;

    #[test]
    fn unexpected_shapes_do_not_panic_or_get_overwritten() {
        let mut s = json!({"hooks": [1, 2], "x": 1});
        let log = apply(&mut s, "\"a.exe\"");
        assert!(log.iter().any(|l| l.contains("AVISO")));
        assert_eq!(s["hooks"], json!([1, 2])); // preservado
        let mut s = json!({"hooks": {"Stop": "texto"}});
        assert!(apply(&mut s, "\"a.exe\"").iter().any(|l| l.contains("Stop")));
        assert_eq!(s["hooks"]["Stop"], "texto");
        assert!(s["hooks"]["PostToolUse"].is_array()); // os demais eventos seguem
    }

    #[test]
    fn unreadable_settings_abort_without_touching_the_file_and_backup_keeps_the_original() {
        let d = std::env::temp_dir().join(format!("lq-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        let p = d.join(".claude").join("settings.json");
        std::fs::write(&p, [0xFF, 0xFE, b'{', 0, b'}', 0]).unwrap(); // UTF-16: não é UTF-8
        assert!(edit_at(&p, |v| apply(v, "\"a.exe\"")).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), vec![0xFF, 0xFE, b'{', 0, b'}', 0]);
        // arquivo válido: o backup guarda o original e uma segunda execução não o sobrescreve
        std::fs::write(&p, r#"{"model":"opus"}"#).unwrap();
        edit_at(&p, |v| apply(v, "\"a.exe\"")).unwrap();
        edit_at(&p, |v| apply(v, "\"b.exe\"")).unwrap();
        assert_eq!(std::fs::read_to_string(p.with_extension("json.lq-bak")).unwrap(), r#"{"model":"opus"}"#);
        // sem mudança, não regrava
        let m1 = std::fs::metadata(&p).unwrap().modified().unwrap();
        edit_at(&p, |v| apply(v, "\"b.exe\"")).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), m1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cursor_hooks_merge_without_touching_the_users_own_hooks() {
        let mut c = json!({"version": 1, "hooks": {"stop": [{"command": "./meu.sh"}], "afterFileEdit": [{"command": "fmt"}]}});
        let log = apply_cursor(&mut c, "a.exe");
        assert_eq!(c["hooks"]["stop"].as_array().unwrap().len(), 2); // o do usuário continua lá
        assert_eq!(c["hooks"]["stop"][0]["command"], "./meu.sh");
        assert_eq!(c["hooks"]["afterFileEdit"][0]["command"], "fmt");
        for ev in CURSOR_EVENTS { assert!(c["hooks"][ev].as_array().unwrap().iter().any(|e| e["command"].as_str().unwrap().contains("--hook cursor")), "{ev}"); }
        assert!(!log.is_empty());
        // O PowerShell espera o programa (Out-Null) e dá a resposta: continue=true só em beforeSubmitPrompt.
        let cmd = |ev: &str| c["hooks"][ev].as_array().unwrap().last().unwrap()["command"].as_str().unwrap().to_string();
        assert_eq!(cmd("beforeSubmitPrompt"), "& 'a.exe' --hook cursor | Out-Null; '{' + [char]34 + 'continue' + [char]34 + ':true}'");
        assert_eq!(cmd("stop"), "& 'a.exe' --hook cursor | Out-Null; '{}'");
        assert!(!cmd("stop").contains('"') && !cmd("beforeSubmitPrompt").contains('"')); // sem aspas duplas
        assert!(cursor_command("C:/Users/O'Brien/x.exe", "stop").contains("'C:/Users/O''Brien/x.exe'")); // aspa simples escapada
        // reinstalar em outro caminho reaponta (não duplica); sem mudança, não há o que logar
        apply_cursor(&mut c, "b.exe");
        assert_eq!(c["hooks"]["stop"].as_array().unwrap().len(), 2);
        assert!(c["hooks"]["stop"][1]["command"].as_str().unwrap().contains("b.exe"));
        assert!(apply_cursor(&mut c, "b.exe").is_empty());
        // arquivo novo recebe a versão do esquema
        let mut n = json!({});
        apply_cursor(&mut n, "x.exe");
        assert_eq!(n["version"], 1);
        // remover tira só os nossos
        remove_cursor(&mut c);
        assert_eq!(c["hooks"]["stop"], json!([{"command": "./meu.sh"}]));
        assert!(c["hooks"]["beforeSubmitPrompt"].is_null());
        // estrutura inesperada é preservada
        let mut w = json!({"hooks": [1, 2]});
        assert!(apply_cursor(&mut w, "x.exe").iter().any(|l| l.contains("AVISO")));
        assert_eq!(w["hooks"], json!([1, 2]));
    }

    #[test]
    fn cursor_install_needs_the_cursor_folder_and_keeps_a_backup() {
        let d = std::env::temp_dir().join(format!("lq-cursor-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join(".cursor").join("hooks.json");
        // sem a pasta .cursor: ignora e não cria nada
        assert!(edit_cursor_at(&p, |v| apply_cursor(v, "x.exe")).unwrap()[0].contains("não encontrado"));
        assert!(!d.exists());
        std::fs::create_dir_all(d.join(".cursor")).unwrap();
        std::fs::write(&p, r#"{"version":1,"hooks":{"stop":[{"command":"./meu.sh"}]}}"#).unwrap();
        edit_cursor_at(&p, |v| apply_cursor(v, "x.exe")).unwrap();
        assert!(std::fs::read_to_string(p.with_extension("json.lq-bak")).unwrap().contains("meu.sh"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
