//! Antigravity (Google). Atividade (trabalhando / ociosa) vem dos hooks oficiais (`~/.gemini/config/hooks.json`,
//! instalados como `line-quota --hook antigravity`). As cotas (Gemini e Claude/modelos abertos) vêm do servidor
//! local do próprio Antigravity: ele publica porta e token em `~/.gemini/antigravity/daemon/hub_*.json`, e o app
//! consulta só `127.0.0.1` (nunca a rede). O token nunca é gravado, registrado nem mostrado.
use lq_core::model::{AgentState, Meter, MeterKind, ProviderId, Usage};
use lq_core::provider::Provider;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Sem sinal de `Stop` por tanto tempo, a tarefa é dada como encerrada (o hook pode ter falhado).
const WORKING_STALE_SECS: i64 = 30 * 60;
const SUMMARY_PATH: &str = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";

pub struct Antigravity { pub dir: PathBuf }

impl Antigravity {
    pub fn new(dir: PathBuf) -> Self { Self { dir } }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn empty() -> Usage { Usage { provider: ProviderId::Antigravity, meters: Vec::new() } }

/// Interpreta a resposta de `GetUserStatus`: uma cota para os modelos Gemini e outra para os demais (Claude e
/// modelos abertos, que dividem a mesma cota). Em cada grupo vale o modelo mais gasto. Modelos sem cota são ignorados.
pub fn parse_user_status(json: &str) -> Result<Usage, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "Antigravity: resposta de uso inválida".to_string())?;
    let models = v["userStatus"]["cascadeModelConfigData"]["clientModelConfigs"].as_array()
        .ok_or_else(|| "Antigravity: resposta de uso em formato inesperado".to_string())?;
    let mut groups: [Option<(f32, Option<i64>)>; 2] = [None, None]; // [Gemini, Claude e abertos]
    for m in models {
        let Some(frac) = m["quotaInfo"]["remainingFraction"].as_f64().filter(|f| f.is_finite()) else { continue };
        let used = ((1.0 - frac) * 100.0).clamp(0.0, 100.0) as f32;
        let resets = m["quotaInfo"]["resetTime"].as_str().and_then(crate::claude_oauth::iso_to_epoch);
        let i = if m["label"].as_str().is_some_and(|l| l.starts_with("Gemini")) { 0 } else { 1 };
        match &mut groups[i] {
            Some((u, r)) if *u >= used => { if r.is_none() { *r = resets; } }
            slot => *slot = Some((used, resets)),
        }
    }
    let meters = [(MeterKind::Gemini, groups[0]), (MeterKind::Claude, groups[1])].into_iter()
        .filter_map(|(kind, g)| g.map(|(used_pct, resets_at)| Meter { kind, used_pct, resets_at, estimated: false })).collect();
    Ok(Usage { provider: ProviderId::Antigravity, meters })
}

/// Interpreta `RetrieveUserQuotaSummary` (o que a tela de uso do Antigravity mostra): grupos de modelos com janelas
/// (`weekly`, `5h`). Gemini -> cota semanal; Claude e modelos abertos -> semanal e de 5 horas.
pub fn parse_quota_summary(json: &str) -> Result<Usage, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "Antigravity: resposta de uso inválida".to_string())?;
    let groups = v["response"]["groups"].as_array().ok_or_else(|| "Antigravity: resposta de uso em formato inesperado".to_string())?;
    let mut meters: Vec<Meter> = Vec::new();
    for g in groups {
        let gemini = g["displayName"].as_str().is_some_and(|n| n.starts_with("Gemini"));
        for b in g["buckets"].as_array().into_iter().flatten() {
            let Some(frac) = b["remainingFraction"].as_f64().filter(|f| f.is_finite()) else { continue };
            let kind = match (gemini, b["window"].as_str().unwrap_or("")) {
                (true, _) => MeterKind::Gemini,
                (false, "5h") => MeterKind::Claude5h,
                (false, _) => MeterKind::Claude,
            };
            let used_pct = ((1.0 - frac) * 100.0).clamp(0.0, 100.0) as f32;
            let resets_at = b["resetTime"].as_str().and_then(crate::claude_oauth::iso_to_epoch);
            match meters.iter_mut().find(|m| m.kind == kind) {
                Some(m) => if used_pct > m.used_pct { m.used_pct = used_pct; m.resets_at = resets_at; },
                None => meters.push(Meter { kind, used_pct, resets_at, estimated: false }),
            }
        }
    }
    Ok(Usage { provider: ProviderId::Antigravity, meters })
}

/// Porta HTTP e token do servidor local, do arquivo que o próprio Antigravity grava (o mais recente).
fn daemon_info() -> Option<(u16, String)> {
    let dir = PathBuf::from(std::env::var("USERPROFILE").ok()?).join(".gemini").join("antigravity").join("daemon");
    let newest = std::fs::read_dir(dir).ok()?.flatten()
        .filter(|e| { let n = e.file_name().to_string_lossy().to_string(); n.starts_with("hub_") && n.ends_with(".json") })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(newest.path()).ok()?).ok()?;
    let port = u16::try_from(v["httpPort"].as_u64()?).ok().filter(|p| *p >= 1024)?;
    let token = v["csrfToken"].as_str().filter(|t| !t.is_empty() && t.len() < 256)?.to_string();
    Some((port, token))
}

impl Provider for Antigravity {
    fn id(&self) -> ProviderId { ProviderId::Antigravity }

    /// Antigravity fechado (sem arquivo do servidor ou sem resposta) = sem medidores: fica só o ícone.
    fn fetch(&self) -> Result<Usage, String> {
        let Some((port, token)) = daemon_info() else { return Ok(empty()) };
        let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(4)).build();
        let post = |path: &str, body: &str| agent
            // Só o endereço local, escrito aqui: a porta vem do arquivo, o servidor nunca.
            .post(&format!("http://127.0.0.1:{port}{path}"))
            .set("Content-Type", "application/json").set("Connect-Protocol-Version", "1").set("X-Codeium-Csrf-Token", &token)
            .send_string(body);
        let read = |r: ureq::Response| r.into_string().map_err(|_| "Antigravity: resposta ilegível".to_string());
        // Preferido: o resumo de cotas (5 h e semanal, como na tela de uso). Versões sem ele caem no status do usuário.
        match post(SUMMARY_PATH, "{}") {
            Ok(r) => return parse_quota_summary(&read(r)?),
            Err(ureq::Error::Transport(_)) => return Ok(empty()), // fechado (arquivo de uma execução antiga)
            Err(ureq::Error::Status(..)) => {}
        }
        match post(STATUS_PATH, r#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","locale":"en"}}"#) {
            Ok(r) => parse_user_status(&read(r)?),
            Err(ureq::Error::Transport(_)) => Ok(empty()),
            Err(ureq::Error::Status(c, _)) => Err(format!("Antigravity: HTTP {c}")),
        }
    }

    fn local_only(&self) -> bool { false }
    fn activity(&self) -> AgentState { lq_activity::aggregate(&self.dir, "antigravity", now_secs(), WORKING_STALE_SECS) }
    fn detail(&self) -> Option<lq_core::model::AgentDetail> { lq_activity::detail(&self.dir, "antigravity", now_secs()) }
    fn chats(&self) -> Vec<lq_core::model::ChatInfo> { lq_activity::chats(&self.dir, "antigravity", now_secs(), WORKING_STALE_SECS) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_hooks_means_idle() {
        let a = Antigravity::new(std::env::temp_dir().join("lq-antigravity-test-empty"));
        assert_eq!(a.activity(), AgentState::Idle);
        assert!(!a.local_only()); // consulta o servidor local do app
    }

    #[test]
    fn two_quota_groups_from_user_status() {
        let j = r#"{"userStatus":{"email":"x","cascadeModelConfigData":{"clientModelConfigs":[
            {"label":"Gemini 3.8 Flash (High)","quotaInfo":{"remainingFraction":0.9648192,"resetTime":"2026-11-30T18:16:40Z"}},
            {"label":"Gemini 3.8 Flash (Low)","quotaInfo":{"remainingFraction":0.9,"resetTime":"2026-11-30T18:16:40Z"}},
            {"label":"Claude Opus 4.6 (Thinking)","quotaInfo":{"remainingFraction":1,"resetTime":"2026-11-30T18:26:53Z"}},
            {"label":"GPT-OSS 120B (Medium)","quotaInfo":{"remainingFraction":1,"resetTime":"2026-11-30T18:26:53Z"}},
            {"label":"Gemini 4 Carbon","quotaInfo":{}}]}}}"#;
        let u = parse_user_status(j).unwrap();
        assert_eq!(u.meters.len(), 2);
        let g = u.meter(MeterKind::Gemini).unwrap();
        assert!((g.used_pct - 10.0).abs() < 0.01); // o modelo Gemini mais gasto manda
        assert!(g.resets_at.is_some_and(|r| r > 1_700_000_000));
        assert_eq!(u.meter(MeterKind::Claude).unwrap().used_pct, 0.0);
    }

    #[test]
    fn quota_summary_has_gemini_weekly_and_claude_two_windows() {
        let j = r#"{"response":{"groups":[
            {"displayName":"Gemini Models","buckets":[{"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0.9584,"resetTime":"2026-11-30T18:16:40Z"}]},
            {"displayName":"Claude and GPT models","buckets":[
              {"bucketId":"3p-weekly","window":"weekly","remainingFraction":0.8707069,"resetTime":"2026-11-30T18:37:38Z"},
              {"bucketId":"3p-5h","window":"5h","remainingFraction":0.5,"resetTime":"2026-11-23T23:37:38Z"}]}]}}"#;
        let u = parse_quota_summary(j).unwrap();
        assert_eq!(u.meters.len(), 3);
        assert!((u.meter(MeterKind::Gemini).unwrap().used_pct - 4.16).abs() < 0.01);
        assert!((u.meter(MeterKind::Claude).unwrap().used_pct - 12.93).abs() < 0.01);
        assert_eq!(u.meter(MeterKind::Claude5h).unwrap().used_pct, 50.0);
        assert!(parse_quota_summary("{}").is_err() && parse_quota_summary("x").is_err());
    }

    #[test]
    fn odd_answers_are_errors_not_panics() {
        assert!(parse_user_status("nao e json").is_err());
        assert!(parse_user_status("{}").is_err());
        assert!(parse_user_status(r#"{"userStatus":{"cascadeModelConfigData":{"clientModelConfigs":[]}}}"#).unwrap().meters.is_empty());
    }
}
