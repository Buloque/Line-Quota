//! Cursor. Atividade (trabalhando / ociosa) vem dos hooks oficiais do Cursor (`~/.cursor/hooks.json`,
//! instalados como `line-quota --hook cursor`). O limite (uso incluído do plano, ciclo mensal) só aparece se o
//! usuário colar o cookie de sessão do painel web (`cursor_auth`): o Cursor não tem API oficial de uso para
//! contas individuais, então usa-se o mesmo endereço que o painel do Cursor consulta (não documentado).
use lq_core::model::{AgentState, Meter, MeterKind, ProviderId, Usage};
use lq_core::provider::Provider;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Sem sinal de `stop` por tanto tempo, a tarefa é dada como encerrada (o hook pode ter falhado).
const WORKING_STALE_SECS: i64 = 30 * 60;
const USAGE_URL: &str = "https://cursor.com/api/usage-summary";

pub struct Cursor { pub dir: PathBuf }

impl Cursor {
    pub fn new(dir: PathBuf) -> Self { Self { dir } }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Interpreta a resposta do painel: percentual do plano incluído (ciclo mensal) e o fim do ciclo.
/// Sem limite a medir (plano ilimitado) devolve lista vazia, e a interface mostra só o ícone.
pub fn parse_usage_summary(json: &str) -> Result<Usage, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "Cursor: resposta de uso inválida".to_string())?;
    let empty = || Usage { provider: ProviderId::Cursor, meters: Vec::new() };
    if v["isUnlimited"].as_bool() == Some(true) { return Ok(empty()); }
    let plan = &v["individualUsage"]["plan"];
    let num = |x: &Value| x.as_f64().filter(|n| n.is_finite());
    let pct = num(&plan["totalPercentUsed"])
        .or_else(|| match (num(&plan["used"]), num(&plan["limit"])) { (Some(u), Some(l)) if l > 0.0 => Some(u / l * 100.0), _ => None })
        .ok_or_else(|| "Cursor: resposta de uso em formato inesperado".to_string())?;
    let resets_at = v["billingCycleEnd"].as_str().and_then(crate::claude_oauth::iso_to_epoch);
    Ok(Usage { provider: ProviderId::Cursor, meters: vec![Meter { kind: MeterKind::Monthly, used_pct: pct.clamp(0.0, 100.0) as f32, resets_at, estimated: false }] })
}

fn request(cookie: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(10)).build(); // o cookie só vai a este endereço
    agent.get(USAGE_URL)
        .set("Cookie", &format!("WorkosCursorSessionToken={cookie}"))
        .set("Accept", "application/json").set("User-Agent", "line-quota")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401 | 403, _) => "Cursor: cookie expirado — cole o cookie de novo".to_string(),
            ureq::Error::Status(c, _) if (300..400).contains(&c) => "Cursor: cookie expirado — cole o cookie de novo".to_string(),
            ureq::Error::Status(c, _) => format!("Cursor: HTTP {c}"),
            ureq::Error::Transport(_) => "Cursor: sem conexão".to_string(),
        })?
        .into_string().map_err(|_| "Cursor: resposta ilegível".to_string())
}

impl Provider for Cursor {
    fn id(&self) -> ProviderId { ProviderId::Cursor }

    /// Sem cookie colado não há o que medir (lista vazia: só o ícone). Com cookie, consulta o painel.
    fn fetch(&self) -> Result<Usage, String> {
        match crate::cursor_auth::load() {
            None => Ok(Usage { provider: ProviderId::Cursor, meters: Vec::new() }),
            Some(cookie) => parse_usage_summary(&request(&cookie)?),
        }
    }

    // A consulta usa a rede: roda na thread de rede (intervalo configurado), não no tick local de 2 s.
    // A atividade, o detalhe e os chats continuam locais.
    fn local_only(&self) -> bool { false }

    fn activity(&self) -> AgentState { lq_activity::aggregate(&self.dir, "cursor", now_secs(), WORKING_STALE_SECS) }

    fn detail(&self) -> Option<lq_core::model::AgentDetail> { lq_activity::detail(&self.dir, "cursor", now_secs()) }

    fn chats(&self) -> Vec<lq_core::model::ChatInfo> { lq_activity::chats(&self.dir, "cursor", now_secs(), WORKING_STALE_SECS) }
}

/// Diagnóstico (`line-quota --cursor-usage`): consulta o painel com o cookie salvo e imprime o resultado
/// interpretado e a ESTRUTURA da resposta (números e textos curtos; nada de e-mail, token ou ids).
pub fn diagnose() -> String {
    fn scrub(v: &Value, key: &str, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => for (k, x) in m { scrub(x, k, &format!("{path}.{k}"), out); },
            Value::Array(a) => for (i, x) in a.iter().enumerate().take(3) { scrub(x, key, &format!("{path}[{i}]"), out); },
            Value::String(s) => {
                let k = key.to_lowercase();
                let hide = k.contains("mail") || k.contains("token") || k.contains("name") || k.contains("id") || s.len() > 40;
                out.push(if hide { format!("{path} = <texto oculto>") } else { format!("{path} = {s:?}") });
            }
            o => out.push(format!("{path} = {o}")),
        }
    }
    let Some(cookie) = crate::cursor_auth::load() else { return "Nenhum cookie do Cursor salvo (Configurações → Provedores → Colar cookie).".into() };
    match request(&cookie) {
        Err(e) => e,
        Ok(body) => {
            let mut lines = Vec::new();
            if let Ok(v) = serde_json::from_str::<Value>(&body) { scrub(&v, "", "", &mut lines); }
            let parsed = match parse_usage_summary(&body) {
                Ok(u) => format!("{:?}", u.meters),
                Err(e) => e,
            };
            format!("interpretado: {parsed}\nestrutura da resposta:\n{}", lines.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_cookie_means_no_meters_and_idle() {
        let c = Cursor::new(std::env::temp_dir().join("lq-cursor-test-empty"));
        assert_eq!(c.activity(), AgentState::Idle);
        assert!(!c.local_only()); // a consulta usa a rede
    }

    #[test]
    fn percent_from_the_dashboard_summary() {
        let j = r#"{"billingCycleStart":"2026-11-23T00:00:00.000Z","billingCycleEnd":"2026-12-23T00:00:00.000Z","membershipType":"free","isUnlimited":false,
            "individualUsage":{"plan":{"enabled":true,"used":240,"limit":2000,"totalPercentUsed":12.0}}}"#;
        let u = parse_usage_summary(j).unwrap();
        let m = u.meter(MeterKind::Monthly).unwrap();
        assert_eq!(m.used_pct, 12.0);
        assert!(m.resets_at.is_some_and(|r| r > 1_700_000_000));
    }

    #[test]
    fn percent_falls_back_to_used_over_limit_and_handles_odd_answers() {
        let j = r#"{"individualUsage":{"plan":{"used":500,"limit":2000}}}"#;
        assert_eq!(parse_usage_summary(j).unwrap().meters[0].used_pct, 25.0);
        // acima de 100% é limitado; limite zero/ausente é formato inesperado; ilimitado = sem medidores
        assert_eq!(parse_usage_summary(r#"{"individualUsage":{"plan":{"totalPercentUsed":140.0}}}"#).unwrap().meters[0].used_pct, 100.0);
        assert!(parse_usage_summary(r#"{"individualUsage":{"plan":{"used":5,"limit":0}}}"#).is_err());
        assert!(parse_usage_summary("{}").is_err());
        assert!(parse_usage_summary("nao e json").is_err());
        assert!(parse_usage_summary(r#"{"isUnlimited":true}"#).unwrap().meters.is_empty());
    }
}
