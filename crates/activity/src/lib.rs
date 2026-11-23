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
    handle_event(dir, "claude", &v, now)
}

/// Evento do Cursor -> o nome equivalente do Claude Code (assim a mesma lógica serve aos dois).
pub fn cursor_event(name: &str) -> Option<&'static str> {
    match name {
        "beforeSubmitPrompt" => Some("UserPromptSubmit"),
        "postToolUse" => Some("PostToolUse"),
        "stop" => Some("Stop"),
        "sessionEnd" => Some("SessionEnd"),
        _ => None,
    }
}

/// Processa o JSON que o Cursor envia ao hook via stdin e devolve o que escrever no stdout.
/// Campos usados (doc oficial de hooks): `conversation_id`, `workspace_roots`, `prompt` (beforeSubmitPrompt),
/// `tool_name`/`tool_input` (postToolUse). O Cursor não usa o texto de saída destes eventos além de
/// `{"continue": true}` em beforeSubmitPrompt, que damos sempre (nunca bloqueamos nada).
pub fn handle_cursor_hook(dir: &Path, stdin: &str, now: i64) -> std::io::Result<String> {
    let v: Value = serde_json::from_str(stdin.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let name = v["hook_event_name"].as_str().unwrap_or("");
    let reply = if name == "beforeSubmitPrompt" { r#"{"continue":true}"# } else { "{}" }.to_string();
    let Some(event) = cursor_event(name) else { return Ok(reply) };
    let session = v["conversation_id"].as_str().or_else(|| v["session_id"].as_str()).unwrap_or("default");
    // `tool_input` pode vir como objeto ou como texto JSON.
    let tool_input = match &v["tool_input"] {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        o => o.clone(),
    };
    let mut n = json!({"hook_event_name": event, "session_id": session, "tool_name": v["tool_name"], "tool_input": tool_input});
    if let Some(cwd) = v["workspace_roots"][0].as_str().or_else(|| v["cwd"].as_str()) { n["cwd"] = json!(cwd); }
    if let Some(p) = v["prompt"].as_str() { n["prompt"] = json!(p); }
    handle_event(dir, "cursor", &n, now)?;
    Ok(reply)
}

/// Argumentos de ferramenta do Antigravity (PascalCase, ex.: `CommandLine`, `AbsolutePath`) -> as chaves
/// que o painel entende (`command`, `file_path`, `query`...).
fn antigravity_args(args: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(m) = args.as_object() {
        for (k, v) in m {
            let lower = k.to_ascii_lowercase();
            let key = match lower.as_str() {
                "commandline" => "command",
                "absolutepath" | "targetfile" | "filepath" | "file" => "file_path",
                other => other,
            };
            out.entry(key.to_string()).or_insert_with(|| v.clone());
        }
    }
    Value::Object(out)
}

/// Processa o JSON que o Antigravity envia ao hook via stdin (`hooks.json`: PreInvocation, PostToolUse, Stop).
/// Campos usados: `hookEventName`, `conversationId`, `workspacePaths`/`cwd`, `lastUserInput`, `toolCall.{name,args}`.
/// Devolve `{}` sempre: nunca bloqueia nem injeta nada. O PreInvocation dispara a cada chamada ao modelo, então só
/// o primeiro de cada pedido conta como "novo pedido"; os demais apenas mantêm o estado de trabalhando.
pub fn handle_antigravity_hook(dir: &Path, stdin: &str, now: i64) -> std::io::Result<String> {
    let reply = "{}".to_string();
    let v: Value = serde_json::from_str(stdin.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let session = v["conversationId"].as_str().unwrap_or("default");
    let prompt = v["lastUserInput"].as_str();
    let event = match v["hookEventName"].as_str().unwrap_or("") {
        "PreInvocation" => {
            let known: Value = std::fs::read_to_string(file_for(dir, "antigravity", session)).ok()
                .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
            let same_turn = known["state"] == "working" && known["prompt"].as_str() == prompt.map(|p| clip(p, 140)).as_deref();
            if same_turn { "PostToolUse" } else { "UserPromptSubmit" }
        }
        "PostToolUse" => "PostToolUse",
        "Stop" => "Stop",
        _ => return Ok(reply),
    };
    let mut n = json!({"hook_event_name": event, "session_id": session});
    let dir_of = |k: &str| v[k].as_str().filter(|c| !c.is_empty()).map(str::to_string);
    if let Some(cwd) = v["workspacePaths"][0].as_str().filter(|c| !c.is_empty()).map(str::to_string).or_else(|| dir_of("cwd")) { n["cwd"] = json!(cwd); }
    if event == "UserPromptSubmit" { if let Some(p) = prompt { n["prompt"] = json!(p); } }
    if event == "PostToolUse" && v["toolCall"]["name"].is_string() {
        n["tool_name"] = v["toolCall"]["name"].clone();
        n["tool_input"] = antigravity_args(&v["toolCall"]["args"]);
    }
    handle_event(dir, "antigravity", &n, now)?;
    Ok(reply)
}

fn handle_event(dir: &Path, tool: &str, v: &Value, now: i64) -> std::io::Result<()> {
    let event = v["hook_event_name"].as_str().unwrap_or("");
    let session = v["session_id"].as_str().unwrap_or("default");
    let path = file_for(dir, tool, session);
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
            // Nome do chat (o que aparece no Claude Code): lido do transcript só em eventos raros.
            if tool == "claude" && matches!(event, "UserPromptSubmit" | "Stop") {
                if let Some(n) = v["transcript_path"].as_str().and_then(transcript_title) { o["name"] = json!(clip(&n, 48)); }
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

/// Nome do chat no transcript do Claude Code (`custom-title`, senão `agent-name`). Lê só o final do arquivo.
/// Só lê `.jsonl` dentro da pasta de dados do Claude (`~/.claude` ou `CLAUDE_CONFIG_DIR`): caminhos de rede (UNC)
/// ou dispositivos podem travar o hook, que roda dentro do Claude Code.
pub fn transcript_allowed(path: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let p = norm(path);
    if p.starts_with("//") || !p.ends_with(".jsonl") || p.contains("/../") { return false; }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let mut roots = vec![format!("{}/.claude/", norm(home.trim_end_matches(['\\', '/'])))];
    if let Ok(c) = std::env::var("CLAUDE_CONFIG_DIR") { roots.push(format!("{}/", norm(c.trim_end_matches(['\\', '/'])))); }
    roots.iter().any(|r| r.len() > 2 && p.starts_with(r.as_str()))
}

pub fn transcript_title(path: &str) -> Option<String> {
    if !transcript_allowed(path) { return None; }
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    title_from_transcript(&String::from_utf8_lossy(&buf))
}

fn title_from_transcript(text: &str) -> Option<String> {
    let (mut custom, mut agent) = (None, None);
    for line in text.lines() {
        if !line.contains("-title\"") && !line.contains("\"agent-name\"") { continue; }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") => custom = v["customTitle"].as_str().map(str::to_string),
            Some("agent-name") => agent = v["agentName"].as_str().map(str::to_string),
            _ => {}
        }
    }
    custom.or(agent).filter(|t| !t.trim().is_empty())
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
            // Nome do chat; sem nome, o 1º pedido.
            title: v["name"].as_str().or_else(|| v["title"].as_str()).map(str::to_string).or_else(|| d.prompt.clone()).unwrap_or_else(|| "Chat".into()),
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
    fn antigravity_hooks_drive_working_and_idle() {
        let d = tmp("antigravity");
        let pre = |now| handle_antigravity_hook(&d, r#"{"hookEventName":"PreInvocation","conversationId":"g1","workspacePaths":["C:\\a\\Meu App"],"lastUserInput":"pesquise pinguins"}"#, now).unwrap();
        assert_eq!(pre(100), "{}");
        assert_eq!(aggregate(&d, "antigravity", 105, 1800), AgentState::Working);
        handle_antigravity_hook(&d, r#"{"hookEventName":"PostToolUse","conversationId":"g1","toolCall":{"name":"run_command","args":{"CommandLine":"npm test"}}}"#, 110).unwrap();
        // outra chamada ao modelo no mesmo pedido: continua o mesmo pedido (since não reinicia)
        pre(112);
        let x = detail(&d, "antigravity", 115).unwrap();
        assert_eq!((x.project.as_deref(), x.prompt.as_deref(), x.tool.as_deref(), x.target.as_deref(), x.since), (Some("Meu App"), Some("pesquise pinguins"), Some("run_command"), Some("npm test"), Some(100)));
        assert_eq!(handle_antigravity_hook(&d, r#"{"hookEventName":"Stop","conversationId":"g1","terminationReason":"NO_TOOL_CALL"}"#, 120).unwrap(), "{}");
        assert_eq!(aggregate(&d, "antigravity", 125, 1800), AgentState::Idle);
        // mesmo texto depois de parar = novo pedido
        pre(130);
        assert_eq!(detail(&d, "antigravity", 131).unwrap().since, Some(130));
        // eventos desconhecidos e entrada inválida são inofensivos
        assert_eq!(handle_antigravity_hook(&d, r#"{"hookEventName":"SessionStart","conversationId":"g1"}"#, 140).unwrap(), "{}");
        assert_eq!(handle_antigravity_hook(&d, "lixo", 140).unwrap(), "{}");
    }

    #[test]
    fn cursor_hooks_drive_working_and_idle() {
        let d = tmp("cursor");
        let r = handle_cursor_hook(&d, r#"{"hook_event_name":"beforeSubmitPrompt","conversation_id":"c1","workspace_roots":["C:\\a\\Meu App"],"prompt":"conserta o login"}"#, 100).unwrap();
        assert_eq!(r, r#"{"continue":true}"#); // nunca bloqueia o prompt
        assert_eq!(aggregate(&d, "cursor", 105, 1800), AgentState::Working);
        handle_cursor_hook(&d, r#"{"hook_event_name":"postToolUse","conversation_id":"c1","tool_name":"Edit","tool_input":"{\"file_path\":\"C:\\\\a\\\\auth.rs\"}"}"#, 110).unwrap();
        let x = detail(&d, "cursor", 115).unwrap();
        assert_eq!((x.project.as_deref(), x.prompt.as_deref()), (Some("Meu App"), Some("conserta o login")));
        assert_eq!((x.tool.as_deref(), x.target.as_deref()), (Some("Edit"), Some("auth.rs"))); // tool_input como texto JSON
        assert_eq!(handle_cursor_hook(&d, r#"{"hook_event_name":"stop","conversation_id":"c1","status":"completed"}"#, 120).unwrap(), "{}");
        assert_eq!(aggregate(&d, "cursor", 125, 1800), AgentState::Idle);
        handle_cursor_hook(&d, r#"{"hook_event_name":"sessionEnd","conversation_id":"c1"}"#, 130).unwrap();
        assert!(chats(&d, "cursor", 131, 1800).is_empty());
        // evento que não usamos: não grava nada e responde com objeto vazio
        assert_eq!(handle_cursor_hook(&d, r#"{"hook_event_name":"preToolUse","conversation_id":"c1"}"#, 140).unwrap(), "{}");
        // o Cursor e o Claude não se misturam
        assert_eq!(aggregate(&d, "claude", 125, 1800), AgentState::Idle);
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
    fn chat_name_comes_from_the_transcript() {
        let t = concat!(r#"{"type":"user","message":"oi"}"#, "\n",
            r#"{"type":"agent-name","agentName":"Nome do agente","sessionId":"s"}"#, "\n",
            r#"{"type":"custom-title","customTitle":"Corrigir o instalador","sessionId":"s"}"#, "\n");
        assert_eq!(title_from_transcript(t).as_deref(), Some("Corrigir o instalador"));
        assert_eq!(title_from_transcript(r#"{"type":"agent-name","agentName":"Só agente"}"#).as_deref(), Some("Só agente"));
        assert_eq!(title_from_transcript(r#"{"type":"user"}"#), None);

        let d = std::env::temp_dir().join(format!("lq-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let tp = d.join("t.jsonl");
        std::fs::write(&tp, t).unwrap();
        assert!(!transcript_allowed(&tp.to_string_lossy())); // fora da pasta do Claude: ignorado
        assert!(!transcript_allowed("\\\\10.0.0.1\\share\\x.jsonl") && !transcript_allowed("CON"));
        std::env::set_var("CLAUDE_CONFIG_DIR", &d); // agora o temporário conta como pasta do Claude
        assert!(transcript_allowed(&tp.to_string_lossy()));
        let ev = format!(r#"{{"hook_event_name":"UserPromptSubmit","session_id":"a","cwd":"C:/x/App","prompt":"faz algo","transcript_path":{}}}"#, serde_json::to_string(&tp.to_string_lossy()).unwrap());
        handle_claude_hook(&d, &ev, 100).unwrap();
        let c = chats(&d, "claude", 110, 600);
        assert_eq!((c[0].title.as_str(), c[0].detail.prompt.as_deref()), ("Corrigir o instalador", Some("faz algo")));
        let _ = std::fs::remove_dir_all(&d);
    }

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
