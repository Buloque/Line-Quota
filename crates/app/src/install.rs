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
    let hooks = settings.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    for ev in EVENTS {
        let arr = hooks.as_object_mut().unwrap().entry(ev).or_insert_with(|| json!([]));
        if upsert(arr.as_array_mut().unwrap(), MARKER, &format!("{exe} {MARKER}")) {
            log.push(format!("hook {ev} configurado"));
        }
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
    if let Some(h) = settings["hooks"].as_object_mut() {
        for (_, arr) in h.iter_mut() {
            if let Some(l) = arr.as_array_mut() { l.retain(|e| !has_cmd(e, MARKER)); }
        }
        h.retain(|_, v| v.as_array().is_none_or(|l| !l.is_empty()));
    }
    if settings["statusLine"]["command"].as_str().is_some_and(|c| c.contains(STATUS_MARKER)) {
        settings.as_object_mut().unwrap().remove("statusLine");
    }
}

fn edit(f: impl FnOnce(&mut Value) -> Vec<String>) -> Result<Vec<String>, String> {
    let p = settings_path();
    let txt = std::fs::read_to_string(&p).unwrap_or_else(|_| "{}".into());
    let mut v: Value = serde_json::from_str(txt.trim_start_matches('\u{feff}')).map_err(|e| format!("settings.json inválido: {e}"))?;
    let _ = std::fs::write(p.with_extension("json.lq-bak"), &txt); // backup antes de mexer
    let log = f(&mut v);
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).map_err(|e| e.to_string())?;
    Ok(log)
}

pub fn install() -> Result<Vec<String>, String> { edit(|v| apply(v, &exe_cmd())) }

/// Instala os hooks apontando para um executável específico (o instalado).
pub fn install_for(exe: &std::path::Path) -> Result<Vec<String>, String> {
    let cmd = format!("\"{}\"", exe.to_string_lossy().replace('\\', "/"));
    edit(|v| apply(v, &cmd))
}
pub fn uninstall() -> Result<Vec<String>, String> { edit(|v| { remove(v); vec!["hooks do Line Quota removidos".into()] }) }

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
