//! Estado de atividade dos agentes (tarefa ativa / aguardando resposta).
//! Os hooks das ferramentas gravam um arquivo por sessão; o app só lê o agregado.
pub mod procs;

use lq_core::model::{AgentDetail, AgentState, ChatInfo};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const INPUT_STALE_SECS: i64 = 6 * 3600;
const GC_SECS: i64 = 24 * 3600;

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("LineQuota")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action { Set(AgentState), Remove, Ignore }

/// Mapeia um evento de hook do Claude Code para uma ação.
pub fn claude_action(event: &str, notification_type: Option<&str>) -> Action {
    match event {
        "UserPromptSubmit" | "PostToolUse" => Action::Set(AgentState::Working),
        "Stop" => Action::Set(AgentState::Idle),
        "SessionEnd" => Action::Remove,
        "Notification" => match notification_type {
            Some("idle_prompt") => Action::Set(AgentState::Idle),
            Some("permission_prompt") | Some("elicitation_dialog") | None => Action::Set(AgentState::NeedsInput),
            _ => Action::Ignore,
        },
        _ => Action::Ignore,
    }
}

fn state_str(s: AgentState) -> &'static str {
    match s { AgentState::Idle => "idle", AgentState::Working => "working", AgentState::NeedsInput => "needs_input" }
}

fn sanitize(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect()
}

fn file_for(dir: &Path, tool: &str, session: &str) -> PathBuf {
    dir.join("activity").join(format!("{tool}-{}.json", sanitize(session)))
}

/// Processa o JSON que o Claude Code envia ao hook via stdin.
pub fn handle_claude_hook(dir: &Path, stdin: &str, now: i64) -> std::io::Result<()> {
    let v: Value = serde_json::from_str(stdin.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let event = v["hook_event_name"].as_str().unwrap_or("");
    let session = v["session_id"].as_str().unwrap_or("default");
    let path = file_for(dir, "claude", session);
    match claude_action(event, v["notification_type"].as_str()) {
        Action::Ignore => Ok(()),
        Action::Remove => { let _ = std::fs::remove_file(path); Ok(()) }
        Action::Set(s) => {
            std::fs::create_dir_all(dir.join("activity"))?;
            // Mantém os detalhes anteriores e atualiza o que este evento traz.
            let mut o: Value = std::fs::read_to_string(&path).ok()
                .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
            o["state"] = json!(state_str(s));
            o["ts"] = json!(now);
            if let Some(cwd) = v["cwd"].as_str() {
                o["project"] = lq_core::model::project_from_dir(cwd).map_or(Value::Null, |n| json!(n));
            }
            match event {
                "UserPromptSubmit" => {
                    if let Some(p) = v["prompt"].as_str() {
                        o["prompt"] = json!(clip(p, 140));
                        if o["title"].is_null() { o["title"] = json!(clip(p, 48)); } // título = 1º pedido do chat
                    }
                    o["since"] = json!(now);
                    o["notice"] = Value::Null;
                    o["tool"] = Value::Null;
                    o["target"] = Value::Null;
                }
                "PostToolUse" => {
                    if let Some(t) = v["tool_name"].as_str() {
                        o["tool"] = json!(t);
                        o["target"] = tool_target(&v["tool_input"]).map_or(Value::Null, |t| json!(t));
                        o["tool_ts"] = json!(now);
                    }
                    o["notice"] = Value::Null;
                    if o["since"].is_null() { o["since"] = json!(now); }
                }
                "Notification" => {
                    o["notice"] = v["message"].as_str().map_or(Value::Null, |m| json!(clip(m, 140)));
                }
                "Stop" => { o["notice"] = Value::Null; }
                _ => {}
            }
            std::fs::write(path, o.to_string())
        }
    }
}

fn clip(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { format!("{}…", one.chars().take(n).collect::<String>()) }
}

/// Alvo legível de uma ferramenta (arquivo, comando, padrão…).
fn tool_target(input: &Value) -> Option<String> {
    for k in ["file_path", "path", "notebook_path"] {
        if let Some(p) = input[k].as_str() {
            return Some(p.rsplit(['\\', '/']).next().unwrap_or(p).to_string());
        }
    }
    for k in ["command", "pattern", "query", "url", "description", "prompt"] {
        if let Some(t) = input[k].as_str() { return Some(clip(t, 60)); }
    }
    None
}

fn detail_of(v: &Value, ts: i64) -> AgentDetail {
    let s = |k: &str| v[k].as_str().map(str::to_string);
    AgentDetail {
        project: s("project"), prompt: s("prompt"), tool: s("tool"), target: s("target"),
        tool_ts: v["tool_ts"].as_i64(), since: v["since"].as_i64(), notice: s("notice"), updated: Some(ts),
    }
}

/// Todos os chats (um por sessão) da ferramenta, do mais recente ao mais antigo.
/// `working_stale`: segundos sem sinal para considerar uma tarefa "trabalhando" encerrada.
pub fn chats(dir: &Path, tool: &str, now: i64, working_stale: i64) -> Vec<ChatInfo> {
    let Ok(rd) = std::fs::read_dir(dir.join("activity")) else { return Vec::new() };
    let prefix = format!("{tool}-");
    let mut out: Vec<ChatInfo> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) { continue; }
        let Ok(txt) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let ts = v["ts"].as_i64().unwrap_or(0);
        let age = now - ts;
        if age > GC_SECS { continue; }
        let state = match v["state"].as_str() {
            Some("needs_input") if age <= INPUT_STALE_SECS => AgentState::NeedsInput,
            Some("working") if age <= working_stale => AgentState::Working,
            _ => AgentState::Idle,
        };
        let d = detail_of(&v, ts);
        out.push(ChatInfo {
            id: name.trim_start_matches(&prefix).trim_end_matches(".json").to_string(),
            project: d.project.clone(),
            title: v["title"].as_str().map(str::to_string).or_else(|| d.prompt.clone()).unwrap_or_else(|| "Chat".into()),
            state, detail: d,
        });
    }
    out.sort_by(|a, b| b.detail.updated.cmp(&a.detail.updated));
    out
}

/// Detalhe da sessão mais recente (maior `ts`) dentro da janela de retenção.
pub fn detail(dir: &Path, tool: &str, now: i64) -> Option<AgentDetail> {
    let rd = std::fs::read_dir(dir.join("activity")).ok()?;
    let prefix = format!("{tool}-");
    let mut best: Option<(i64, Value)> = None;
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with(&prefix) { continue; }
        let Ok(txt) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let ts = v["ts"].as_i64().unwrap_or(0);
        if now - ts > GC_SECS { continue; }
        if best.as_ref().map_or(true, |(b, _)| ts > *b) { best = Some((ts, v)); }
    }
    let (ts, v) = best?;
    Some(detail_of(&v, ts))
}

/// Estado agregado de todas as sessões da ferramenta: NeedsInput > Working > Idle.
/// `working_stale` = segundos sem sinal para considerar uma tarefa "travada/encerrada".
pub fn aggregate(dir: &Path, tool: &str, now: i64, working_stale: i64) -> AgentState {
    let mut best = AgentState::Idle;
    let Ok(rd) = std::fs::read_dir(dir.join("activity")) else { return best };
    let prefix = format!("{tool}-");
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with(&prefix) { continue; }
        let Ok(txt) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let age = now - v["ts"].as_i64().unwrap_or(0);
        if age > GC_SECS { let _ = std::fs::remove_file(e.path()); continue; }
        match v["state"].as_str() {
            Some("needs_input") if age <= INPUT_STALE_SECS => return AgentState::NeedsInput,
            Some("working") if age <= working_stale => best = AgentState::Working,
            _ => {}
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lq-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn hook_records_details() {
        let d = tmp("detail");
        handle_claude_hook(&d, r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","cwd":"C:\\a\\Meu Proj","prompt":"faz   isso"}"#, 100).unwrap();
        handle_claude_hook(&d, r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Edit","tool_input":{"file_path":"C:\\a\\win.rs"}}"#, 110).unwrap();
        let x = detail(&d, "claude", 120).unwrap();
        assert_eq!((x.project.as_deref(), x.prompt.as_deref()), (Some("Meu Proj"), Some("faz isso")));
        assert_eq!((x.tool.as_deref(), x.target.as_deref(), x.since), (Some("Edit"), Some("win.rs"), Some(100)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn action_mapping() {
        assert_eq!(claude_action("Stop", None), Action::Set(AgentState::Idle));
        assert_eq!(claude_action("Notification", Some("permission_prompt")), Action::Set(AgentState::NeedsInput));
        assert_eq!(claude_action("Notification", Some("idle_prompt")), Action::Set(AgentState::Idle));
        assert_eq!(claude_action("Notification", Some("auth_success")), Action::Ignore);
        assert_eq!(claude_action("PreCompact", None), Action::Ignore);
    }

    #[test]
    fn aggregates_sessions() {
        let d = tmp("agg");
        let ev = |name: &str, sid: &str| format!(r#"{{"hook_event_name":"{name}","session_id":"{sid}"}}"#);
        handle_claude_hook(&d, &ev("UserPromptSubmit", "a"), 1000).unwrap();
        assert_eq!(aggregate(&d, "claude", 1010, 1800), AgentState::Working);
        handle_claude_hook(&d, &ev("Notification", "b"), 1020).unwrap();
        assert_eq!(aggregate(&d, "claude", 1030, 1800), AgentState::NeedsInput);
        handle_claude_hook(&d, &ev("SessionEnd", "b"), 1040).unwrap();
        handle_claude_hook(&d, &ev("Stop", "a"), 1050).unwrap();
        assert_eq!(aggregate(&d, "claude", 1060, 1800), AgentState::Idle);
        handle_claude_hook(&d, &ev("UserPromptSubmit", "a"), 2000).unwrap();
        assert_eq!(aggregate(&d, "claude", 2000 + 1801, 1800), AgentState::Idle); // travada
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_input_is_harmless() {
        let d = tmp("bad");
        handle_claude_hook(&d, "not json", 1).unwrap();
        assert_eq!(aggregate(&d, "claude", 2, 10), AgentState::Idle);
    }
}

#[cfg(test)]
mod chats_tests {
    use super::*;

    #[test]
    fn lists_every_session_with_title_and_state() {
        let d = std::env::temp_dir().join(format!("lq-chats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ev = |sid: &str, cwd: &str, p: &str| format!(r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{sid}","cwd":"{cwd}","prompt":"{p}"}}"#);
        handle_claude_hook(&d, &ev("a", "C:/x/App", "primeiro pedido"), 100).unwrap();
        handle_claude_hook(&d, &ev("a", "C:/x/App", "segundo pedido"), 110).unwrap();
        handle_claude_hook(&d, &ev("b", "C:/x/Outro", "outra coisa"), 120).unwrap();
        handle_claude_hook(&d, r#"{"hook_event_name":"Stop","session_id":"b"}"#, 130).unwrap();
        let c = chats(&d, "claude", 140, 600);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].id.as_str(), c[0].state, c[0].project.as_deref()), ("b", AgentState::Idle, Some("Outro")));
        assert_eq!((c[1].title.as_str(), c[1].state), ("primeiro pedido", AgentState::Working)); // título fixo no 1º pedido
        assert_eq!(c[1].detail.prompt.as_deref(), Some("segundo pedido"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
