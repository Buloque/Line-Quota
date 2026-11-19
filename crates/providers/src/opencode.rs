//! OpenCode. Não há API oficial de limites; quem usa o OpenCode com login do ChatGPT (cost = 0)
//! consome o mesmo limite do Codex, então os medidores vêm do mesmo endpoint de uso.
//! A atividade vem do banco local do OpenCode: enquanto uma tarefa roda, o arquivo WAL é escrito.
use crate::codex::Codex;
use lq_core::model::{AgentState, ProviderId, Usage};
use lq_core::provider::Provider;
use std::path::PathBuf;
use std::time::SystemTime;

/// Escrita no WAL há menos que isto = tarefa em andamento.
const WORKING_WINDOW_SECS: u64 = 8;

pub struct OpenCode { pub data_dir: PathBuf }

impl OpenCode {
    pub fn new() -> Self {
        let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default()).join(".local").join("share")
        });
        Self { data_dir: base.join("opencode") }
    }
}

impl Default for OpenCode { fn default() -> Self { Self::new() } }

/// Segundos desde a última modificação do arquivo, se existir.
fn age_secs(p: &std::path::Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    SystemTime::now().duration_since(m).ok().map(|d| d.as_secs())
}

pub fn state_from_wal_age(age: Option<u64>) -> AgentState {
    match age { Some(a) if a <= WORKING_WINDOW_SECS => AgentState::Working, _ => AgentState::Idle }
}

/// Como o OpenCode está autenticado (decide se há limites a mostrar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Sem login em nenhum provedor: só modelos gratuitos, sem limite medido.
    Free,
    /// Login do ChatGPT: consome o mesmo limite do Codex.
    ChatGpt,
    /// Chave de API (pago por uso): não há janela de 5h/semanal.
    ApiKey,
}

/// Classifica o conteúdo do `auth.json` do OpenCode (ausente = gratuito).
pub fn plan_from_auth(auth: Option<&str>) -> Plan {
    let Some(v) = auth.and_then(|t| serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok()) else {
        return Plan::Free;
    };
    let Some(o) = v.as_object().filter(|o| !o.is_empty()) else { return Plan::Free };
    if o.get("openai").map_or(false, |e| e["type"] == "oauth") { Plan::ChatGpt } else { Plan::ApiKey }
}

impl OpenCode {
    pub fn plan(&self) -> Plan { plan_from_auth(std::fs::read_to_string(self.data_dir.join("auth.json")).ok().as_deref()) }
}

impl Provider for OpenCode {
    fn id(&self) -> ProviderId { ProviderId::OpenCode }

    fn fetch(&self) -> Result<Usage, String> {
        if self.plan() != Plan::ChatGpt {
            return Ok(Usage { provider: ProviderId::OpenCode, meters: Vec::new() });
        }
        let mut u = Codex.fetch()?;
        u.provider = ProviderId::OpenCode;
        Ok(u)
    }

    fn activity(&self) -> AgentState { state_from_wal_age(age_secs(&self.data_dir.join("opencode.db-wal"))) }

    fn chats(&self) -> Vec<ChatInfo> {
        CHAT_CACHE.get_or_init(|| TtlCache::new(4)).get_or(|| {
            let ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
            read_chats(&self.data_dir.join("opencode.db"), ms).unwrap_or_default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_detection() {
        assert_eq!(plan_from_auth(None), Plan::Free);
        assert_eq!(plan_from_auth(Some("{}")), Plan::Free);
        assert_eq!(plan_from_auth(Some(r#"{"openai":{"type":"oauth"}}"#)), Plan::ChatGpt);
        assert_eq!(plan_from_auth(Some(r#"{"anthropic":{"type":"api","key":"x"}}"#)), Plan::ApiKey);
    }

    #[test]
    fn wal_age_maps_to_state() {
        assert_eq!(state_from_wal_age(Some(2)), AgentState::Working);
        assert_eq!(state_from_wal_age(Some(8)), AgentState::Working);
        assert_eq!(state_from_wal_age(Some(30)), AgentState::Idle);
        assert_eq!(state_from_wal_age(None), AgentState::Idle);
    }
}

// ---- Chats (SQLite local do OpenCode, somente leitura) ----

use lq_core::model::{AgentDetail, ChatInfo, TtlCache};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

const CHAT_WINDOW_MS: i64 = 24 * 3600 * 1000;
const MAX_CHATS: i64 = 15;

fn clip(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { format!("{}…", one.chars().take(n).collect::<String>()) }
}

fn target_of(input: &Value) -> Option<String> {
    for k in ["filePath", "file_path", "path"] {
        if let Some(p) = input[k].as_str() { return Some(p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()); }
    }
    for k in ["command", "pattern", "query", "url", "description"] {
        if let Some(t) = input[k].as_str() { return Some(clip(t, 60)); }
    }
    None
}

/// Última ferramenta de uma mensagem do assistente: (nome, alvo, instante em ms).
pub fn last_tool(assistant_data: &str) -> Option<(String, Option<String>, Option<i64>)> {
    let v: Value = serde_json::from_str(assistant_data).ok()?;
    let t = v["content"].as_array()?.iter().rev().find(|c| c["type"] == "tool")?;
    Some((t["name"].as_str()?.to_string(), target_of(&t["state"]["input"]), t["time"]["created"].as_i64()))
}

/// Lê as sessões recentes. `now_ms` em milissegundos.
pub fn read_chats(db: &std::path::Path, now_ms: i64) -> Result<Vec<ChatInfo>, rusqlite::Error> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_millis(300))?;
    let mut st = c.prepare(
        "SELECT id, title, directory, time_updated, time_busy, time_idle FROM session_v2 \
         WHERE parent_id IS NULL AND time_archived IS NULL AND time_updated > ?1 \
         ORDER BY time_updated DESC LIMIT ?2")?;
    let rows = st.query_map(rusqlite::params![now_ms - CHAT_WINDOW_MS, MAX_CHATS], |r| Ok((
        r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?,
        r.get::<_, i64>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?,
    )))?.flatten().collect::<Vec<_>>();
    let last = |sid: &str, ty: &str| -> Option<String> {
        c.query_row("SELECT data FROM session_message WHERE session_id = ?1 AND type = ?2 ORDER BY seq DESC LIMIT 1",
            rusqlite::params![sid, ty], |r| r.get::<_, String>(0)).ok()
    };
    let mut out = Vec::new();
    for (id, title, dir, updated, busy, idle) in rows {
        let working = busy.map_or(false, |b| idle.map_or(true, |i| b > i)) && now_ms - updated < 10 * 60 * 1000;
        let user: Option<Value> = last(&id, "user").and_then(|d| serde_json::from_str(&d).ok());
        let prompt = user.as_ref().and_then(|u| u["text"].as_str()).map(|t| clip(t, 140));
        let tool = last(&id, "assistant").and_then(|d| last_tool(&d));
        let generic = title.as_deref().map_or(true, |t| t.trim().is_empty() || t.starts_with("New session"));
        let title = if generic { prompt.as_deref().map(|p| clip(p, 48)).unwrap_or_else(|| "Chat".into()) } else { clip(title.as_deref().unwrap_or(""), 48) };
        let project = lq_core::model::project_from_dir(&dir);
        out.push(ChatInfo {
            id, project: project.clone(), title,
            state: if working { AgentState::Working } else { AgentState::Idle },
            detail: AgentDetail {
                project, prompt,
                tool: tool.as_ref().map(|t| t.0.clone()), target: tool.as_ref().and_then(|t| t.1.clone()),
                tool_ts: tool.as_ref().and_then(|t| t.2).map(|ms| ms / 1000),
                since: user.as_ref().and_then(|u| u["time"]["created"].as_i64()).map(|ms| ms / 1000),
                notice: None, updated: Some(updated / 1000),
            },
        });
    }
    Ok(out)
}

static CHAT_CACHE: std::sync::OnceLock<TtlCache<Vec<ChatInfo>>> = std::sync::OnceLock::new();

#[cfg(test)]
mod chat_tests {
    use super::*;

    #[test]
    fn extracts_last_tool_and_target() {
        let d = r#"{"content":[{"type":"text","text":"x"},{"type":"tool","name":"edit","state":{"input":{"filePath":"C:/a/win.rs"}},"time":{"created":5000}}]}"#;
        assert_eq!(last_tool(d), Some(("edit".into(), Some("win.rs".into()), Some(5000))));
        assert_eq!(last_tool(r#"{"content":[{"type":"text"}]}"#), None);
    }

    #[test]
    fn reads_sessions_from_a_sqlite_file() {
        let p = std::env::temp_dir().join(format!("lq-oc-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch("CREATE TABLE session_v2 (id TEXT, parent_id TEXT, directory TEXT, title TEXT, time_updated INTEGER, time_busy INTEGER, time_idle INTEGER, time_archived INTEGER);
                CREATE TABLE session_message (session_id TEXT, type TEXT, seq INTEGER, data TEXT);
                INSERT INTO session_v2 VALUES ('s1', NULL, 'C:/x/Meu App', 'New session - x', 100000, 99000, 50000, NULL);
                INSERT INTO session_v2 VALUES ('s2', 's1', 'C:/x/Meu App', 'filho', 100000, NULL, NULL, NULL);
                INSERT INTO session_message VALUES ('s1','user',1,'{\"text\":\"corrige o bug\",\"time\":{\"created\":99000}}');
                INSERT INTO session_message VALUES ('s1','assistant',2,'{\"content\":[{\"type\":\"tool\",\"name\":\"bash\",\"state\":{\"input\":{\"command\":\"cargo test\"}},\"time\":{\"created\":99500}}]}');").unwrap();
        }
        let v = read_chats(&p, 101_000).unwrap();
        assert_eq!(v.len(), 1); // subagente (parent_id) fica de fora
        assert_eq!((v[0].title.as_str(), v[0].project.as_deref(), v[0].state), ("corrige o bug", Some("Meu App"), AgentState::Working));
        assert_eq!((v[0].detail.tool.as_deref(), v[0].detail.target.as_deref()), (Some("bash"), Some("cargo test")));
        let _ = std::fs::remove_file(&p);
    }
}
