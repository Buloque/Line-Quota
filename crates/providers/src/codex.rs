use lq_core::model::{AgentDetail, AgentState, Meter, MeterKind, ProviderId, Usage};
use lq_core::provider::Provider;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_BASE: &str = "https://chatgpt.com/backend-api";

pub struct Codex;

fn codex_home() -> PathBuf {
    if let Ok(h) = std::env::var("CODEX_HOME") {
        return PathBuf::from(h);
    }
    PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default()).join(".codex")
}

/// Lê `chatgpt_base_url` do config.toml (linha simples `chave = "valor"`).
fn base_url(config_toml: &str) -> String {
    for line in config_toml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("chatgpt_base_url") {
            if let Some(v) = rest.trim_start().strip_prefix('=') {
                let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim_end_matches('/');
                if !v.is_empty() {
                    return v.to_string();
                }
            }
        }
    }
    DEFAULT_BASE.to_string()
}

/// Retorna (access_token, account_id) do auth.json. Os tokens nunca são logados.
fn parse_auth(json: &str) -> Result<(String, Option<String>), String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("auth.json inválido: {e}"))?;
    let t = &v["tokens"];
    let access = t["access_token"].as_str().ok_or("sem access_token (rode `codex login`)")?;
    Ok((access.to_string(), t["account_id"].as_str().map(str::to_string)))
}

fn window(w: &Value, fallback: MeterKind) -> Option<Meter> {
    let used = w["used_percent"].as_f64()? as f32;
    let kind = match w["limit_window_seconds"].as_i64() {
        Some(s) if s <= 6 * 3600 => MeterKind::FiveHour,
        Some(s) if s >= 5 * 86400 => MeterKind::Weekly,
        _ => fallback,
    };
    let resets_at = w["reset_at"].as_i64().or_else(|| {
        let after = w["reset_after_seconds"].as_i64()?;
        Some(now_secs() + after)
    });
    Some(Meter { kind, used_pct: used.clamp(0.0, 100.0), resets_at, estimated: false })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn parse_usage(json: &str) -> Result<Usage, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("resposta inválida: {e}"))?;
    let rl = &v["rate_limit"];
    let mut meters = Vec::new();
    if let Some(m) = window(&rl["primary_window"], MeterKind::FiveHour) { meters.push(m); }
    if let Some(m) = window(&rl["secondary_window"], MeterKind::Weekly) { meters.push(m); }
    if meters.is_empty() {
        return Err("resposta sem janelas de limite".into());
    }
    Ok(Usage { provider: ProviderId::Codex, meters })
}

impl Provider for Codex {
    fn id(&self) -> ProviderId { ProviderId::Codex }

    fn activity(&self) -> AgentState { if self.working() { AgentState::Working } else { AgentState::Idle } }

    fn detail(&self) -> Option<AgentDetail> {
        let (p, age) = session_age()?;
        if age > 6 * 3600 { return None; }
        let (head, tail) = read_ends(&p)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        Some(detail_from_lines(&head, &tail, now - age as i64))
    }

    fn local_only(&self) -> bool { false }

    fn chats(&self) -> Vec<lq_core::model::ChatInfo> { chats() }

    fn fetch(&self) -> Result<Usage, String> {
        let home = codex_home();
        let auth = std::fs::read_to_string(home.join("auth.json")).map_err(|_| "Codex não logado (auth.json ausente)".to_string())?;
        let (token, account) = parse_auth(&auth)?;
        let base = base_url(&std::fs::read_to_string(home.join("config.toml")).unwrap_or_default());
        let mut req = ureq::get(&format!("{base}/wham/usage"))
            .timeout(Duration::from_secs(10))
            .set("Authorization", &format!("Bearer {token}"))
            .set("User-Agent", "line-quota");
        if let Some(a) = account { req = req.set("ChatGPT-Account-Id", &a); }
        let body = req.call().map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("HTTP {c}"),
            ureq::Error::Transport(_) => "sem conexão".to_string(),
        })?.into_string().map_err(|e| e.to_string())?;
        parse_usage(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_windows() {
        let j = r#"{"plan_type":"plus","rate_limit":{
            "primary_window":{"used_percent":42.5,"limit_window_seconds":18000,"reset_at":1000},
            "secondary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_after_seconds":3600}}}"#;
        let u = parse_usage(j).unwrap();
        assert_eq!(u.meter(MeterKind::FiveHour).unwrap().used_pct, 42.5);
        assert_eq!(u.meter(MeterKind::FiveHour).unwrap().resets_at, Some(1000));
        assert!(u.meter(MeterKind::Weekly).unwrap().resets_at.unwrap() > 1000);
    }

    #[test]
    fn missing_windows_is_error() {
        assert!(parse_usage(r#"{"rate_limit":{}}"#).is_err());
    }

    #[test]
    fn base_url_override() {
        assert_eq!(base_url("chatgpt_base_url = \"http://x/y/\"\n"), "http://x/y");
        assert_eq!(base_url("model=\"a\""), DEFAULT_BASE);
    }

    #[test]
    fn auth_without_token_errors() {
        assert!(parse_auth(r#"{"tokens":{}}"#).is_err());
        assert_eq!(parse_auth(r#"{"tokens":{"access_token":"t","account_id":"a"}}"#).unwrap().1.as_deref(), Some("a"));
    }
}

// ---- Atividade / detalhes locais (sessões em ~/.codex/sessions/AAAA/MM/DD/*.jsonl) ----

/// Arquivo de sessão mais recente (desce só pelo ano/mês/dia mais novos).
fn latest_session(root: &std::path::Path) -> Option<(PathBuf, std::time::SystemTime)> {
    fn newest_dir(p: &std::path::Path) -> Option<PathBuf> {
        std::fs::read_dir(p).ok()?.flatten().filter(|e| e.path().is_dir()).map(|e| e.path()).max()
    }
    let day = newest_dir(&newest_dir(&newest_dir(root)?)?)?;
    std::fs::read_dir(day).ok()?.flatten()
        .filter(|e| e.path().extension().map_or(false, |x| x == "jsonl"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .max_by_key(|(_, m)| *m)
}

fn clip(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { format!("{}…", one.chars().take(n).collect::<String>()) }
}

/// Extrai o detalhe de um trecho de JSONL (primeira linha = meta; resto = eventos recentes).
pub fn detail_from_lines(head: &str, tail: &str, updated: i64) -> lq_core::model::AgentDetail {
    let mut d = lq_core::model::AgentDetail { updated: Some(updated), ..Default::default() };
    if let Ok(v) = serde_json::from_str::<Value>(head) {
        if let Some(cwd) = v["payload"]["cwd"].as_str() {
            d.project = lq_core::model::project_from_dir(cwd);
        }
    }
    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        match (v["type"].as_str(), p["type"].as_str()) {
            (Some("event_msg"), Some("user_message")) => {
                if let Some(m) = p["message"].as_str() { d.prompt = Some(clip(m, 140)); }
                d.tool = None; d.target = None;
            }
            (Some("response_item"), Some("function_call")) | (Some("response_item"), Some("custom_tool_call")) => {
                d.tool = p["name"].as_str().map(str::to_string);
                let args = p["arguments"].as_str().or_else(|| p["input"].as_str()).unwrap_or("");
                d.target = serde_json::from_str::<Value>(args).ok()
                    .and_then(|a| a["command"].as_str().map(str::to_string)
                        .or_else(|| a["command"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))))
                    .or_else(|| (!args.is_empty()).then(|| args.to_string()))
                    .map(|t| clip(&t, 60));
            }
            _ => {}
        }
    }
    d
}

fn read_ends(p: &std::path::Path) -> Option<(String, String)> {
    use std::io::{BufRead, Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(p).ok()?;
    let mut head = String::new();
    std::io::BufReader::new(&f).read_line(&mut head).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(96 * 1024))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let tail = String::from_utf8_lossy(&buf).into_owned();
    // descarta a linha cortada no início do trecho
    let tail = if len > 96 * 1024 { tail.split_once('\n').map(|x| x.1.to_string()).unwrap_or_default() } else { tail };
    Some((head, tail))
}

fn session_age() -> Option<(PathBuf, u64)> {
    let (p, m) = latest_session(&codex_home().join("sessions"))?;
    Some((p, std::time::SystemTime::now().duration_since(m).ok()?.as_secs()))
}

impl Codex {
    fn working(&self) -> bool { session_age().map_or(false, |(_, a)| a <= 8) }
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    #[test]
    fn parses_prompt_and_tool() {
        let head = r#"{"type":"session_meta","payload":{"cwd":"C:/x/Meu App"}}"#;
        let tail = concat!(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"corrige   o bug"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"cargo\",\"test\"]}"}}"#, "\n");
        let d = detail_from_lines(head, tail, 5);
        assert_eq!(d.project.as_deref(), Some("Meu App"));
        assert_eq!(d.prompt.as_deref(), Some("corrige o bug"));
        assert_eq!((d.tool.as_deref(), d.target.as_deref()), (Some("shell"), Some("cargo test")));
    }
}

// ---- Lista de chats (sessões das últimas 24 h) ----

const CHAT_WINDOW_SECS: u64 = 24 * 3600;
const MAX_CHATS: usize = 20;

/// Sessões recentes: olha os 3 dias mais novos e ordena pela última escrita.
fn recent_sessions(root: &std::path::Path) -> Vec<(PathBuf, u64)> {
    fn sorted_dirs(p: &std::path::Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).ok().into_iter().flatten().flatten()
            .map(|e| e.path()).filter(|p| p.is_dir()).collect();
        v.sort_by(|a, b| b.cmp(a));
        v
    }
    let mut days = Vec::new();
    'o: for y in sorted_dirs(root) {
        for m in sorted_dirs(&y) {
            for d in sorted_dirs(&m) {
                days.push(d);
                if days.len() >= 3 { break 'o; }
            }
        }
    }
    let now = std::time::SystemTime::now();
    let mut out: Vec<(PathBuf, u64)> = days.iter()
        .flat_map(|d| std::fs::read_dir(d).ok().into_iter().flatten().flatten())
        .filter(|e| e.path().extension().map_or(false, |x| x == "jsonl"))
        .filter_map(|e| {
            let age = now.duration_since(e.metadata().ok()?.modified().ok()?).ok()?.as_secs();
            (age <= CHAT_WINDOW_SECS).then(|| (e.path(), age))
        }).collect();
    out.sort_by_key(|(_, age)| *age);
    out.truncate(MAX_CHATS);
    out
}

/// Primeiro pedido do usuário no trecho (título do chat).
pub fn first_prompt(text: &str) -> Option<String> {
    text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "event_msg" && v["payload"]["type"] == "user_message")
        .and_then(|v| v["payload"]["message"].as_str().map(|m| clip(m, 48)))
}

fn read_head_text(p: &std::path::Path) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(p) { let _ = f.take(256 * 1024).read_to_end(&mut buf); }
    String::from_utf8_lossy(&buf).into_owned()
}

fn load_chats() -> Vec<lq_core::model::ChatInfo> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    recent_sessions(&codex_home().join("sessions")).into_iter().filter_map(|(p, age)| {
        let (head, tail) = read_ends(&p)?;
        let d = detail_from_lines(&head, &tail, now - age as i64);
        let title = first_prompt(&read_head_text(&p)).or_else(|| d.prompt.clone()).unwrap_or_else(|| "Chat".into());
        Some(lq_core::model::ChatInfo {
            id: p.file_stem()?.to_string_lossy().to_string(),
            project: d.project.clone(), title,
            state: if age <= 8 { AgentState::Working } else { AgentState::Idle },
            detail: d,
        })
    }).collect()
}

static CHAT_CACHE: std::sync::OnceLock<lq_core::model::TtlCache<Vec<lq_core::model::ChatInfo>>> = std::sync::OnceLock::new();

pub fn chats() -> Vec<lq_core::model::ChatInfo> {
    CHAT_CACHE.get_or_init(|| lq_core::model::TtlCache::new(4)).get_or(load_chats)
}

#[cfg(test)]
mod chat_tests {
    use super::*;

    #[test]
    fn first_prompt_is_the_title() {
        let t = concat!(r#"{"type":"session_meta","payload":{}}"#, "\n",
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"  ajusta   o login "}}"#, "\n",
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"depois"}}"#);
        assert_eq!(first_prompt(t).as_deref(), Some("ajusta o login"));
    }
}
