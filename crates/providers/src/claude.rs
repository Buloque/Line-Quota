//! Claude Code: usa o JSON que o próprio Claude Code envia ao `statusLine`
//! (`rate_limits.five_hour/seven_day`), com fallback para o endpoint OAuth (`claude_oauth`).
use lq_core::model::{AgentState, Meter, MeterKind, ProviderId, Usage};
use lq_core::provider::Provider;
use serde_json::{json, Value};
use std::path::PathBuf;

const SNAPSHOT: &str = "claude.json";
const WORKING_STALE_SECS: i64 = 30 * 60;
const SNAPSHOT_FRESH_SECS: i64 = 15 * 60;
/// Snapshot gravado a partir do OAuth: vale pouco (o OAuth é consultado de novo depois disso).
const OAUTH_SNAPSHOT_FRESH_SECS: i64 = 3 * 60;

pub struct Claude { pub dir: PathBuf, oauth: crate::claude_oauth::OAuth }

impl Claude {
    pub fn new(dir: PathBuf) -> Self { Self { dir, oauth: Default::default() } }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn norm_window(w: &Value) -> Value {
    match (w["used_percentage"].as_f64(), w["resets_at"].as_i64()) {
        (Some(u), r) => json!({"used": u, "resets_at": r}),
        _ => Value::Null,
    }
}

/// Converte o JSON do statusLine em snapshot normalizado.
pub fn snapshot_from_statusline(stdin: &str, now: i64) -> Value {
    let v: Value = serde_json::from_str(stdin.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let rl = &v["rate_limits"];
    json!({"ts": now, "five_hour": norm_window(&rl["five_hour"]), "seven_day": norm_window(&rl["seven_day"])})
}

/// Grava o snapshot (atômico) e devolve a linha curta exibida no statusLine do Claude Code.
pub fn write_snapshot(dir: &std::path::Path, stdin: &str, now: i64) -> std::io::Result<String> {
    let snap = snapshot_from_statusline(stdin, now);
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("claude.json.tmp");
    std::fs::write(&tmp, snap.to_string())?;
    std::fs::rename(&tmp, dir.join(SNAPSHOT))?;
    let pct = |k: &str| snap[k]["used"].as_f64().map(|u| format!("{u:.0}%")).unwrap_or_else(|| "-".into());
    Ok(format!("5h {} · 7d {}", pct("five_hour"), pct("seven_day")))
}

fn meter(w: &Value, kind: MeterKind, now: i64) -> Option<Meter> {
    let used = w["used"].as_f64()? as f32;
    let resets_at = w["resets_at"].as_i64();
    // Janela já reiniciada desde o último snapshot: uso volta a zero.
    if resets_at.is_some_and(|r| r <= now) {
        return Some(Meter { kind, used_pct: 0.0, resets_at: None, estimated: false });
    }
    Some(Meter { kind, used_pct: used.clamp(0.0, 100.0), resets_at, estimated: false })
}

pub fn usage_from_snapshot(json: &str, now: i64) -> Result<Usage, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("snapshot inválido: {e}"))?;
    let meters: Vec<Meter> = [meter(&v["five_hour"], MeterKind::FiveHour, now), meter(&v["seven_day"], MeterKind::Weekly, now)]
        .into_iter().flatten().collect();
    if meters.is_empty() {
        return Err("Claude sem dados de limite (requer plano Pro/Max)".into());
    }
    Ok(Usage { provider: ProviderId::Claude, meters })
}

impl Provider for Claude {
    fn id(&self) -> ProviderId { ProviderId::Claude }

    /// Snapshot do statusLine se recente (< 15 min); senão OAuth (cache de 3 min);
    /// se o OAuth falhar, usa o snapshot antigo (com reset aplicado) em vez de nada.
    fn fetch(&self) -> Result<Usage, String> {
        let now = now_secs();
        let snap = std::fs::read_to_string(self.dir.join(SNAPSHOT)).ok();
        let fresh = snap.as_deref().and_then(|t| {
            let ts = serde_json::from_str::<Value>(t).ok()?["ts"].as_i64()?;
            let limit = if serde_json::from_str::<Value>(t).ok()?["src"] == "oauth" { OAUTH_SNAPSHOT_FRESH_SECS } else { SNAPSHOT_FRESH_SECS };
            (now - ts < limit).then(|| usage_from_snapshot(t, now).ok()).flatten()
        });
        if let Some(u) = fresh { return Ok(u); }
        match self.oauth.get(now * 1000) {
            Ok(u) => { let _ = store_oauth_snapshot(&self.dir, &u, now); Ok(u) } // sobrevive a reinícios e a falhas (ex.: HTTP 429)
            Err(e) => snap.as_deref().and_then(|t| usage_from_snapshot(t, now).ok()).ok_or(e),
        }
    }

    fn local_only(&self) -> bool { true }

    fn activity(&self) -> AgentState { lq_activity::aggregate(&self.dir, "claude", now_secs(), WORKING_STALE_SECS) }

    fn detail(&self) -> Option<lq_core::model::AgentDetail> { lq_activity::detail(&self.dir, "claude", now_secs()) }

    fn chats(&self) -> Vec<lq_core::model::ChatInfo> { lq_activity::chats(&self.dir, "claude", now_secs(), WORKING_STALE_SECS) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = r#"{"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":2000},"seven_day":{"used_percentage":41,"resets_at":9000}}}"#;

    #[test]
    fn statusline_to_usage() {
        let snap = snapshot_from_statusline(INPUT, 1000).to_string();
        let u = usage_from_snapshot(&snap, 1500).unwrap();
        assert_eq!(u.meter(MeterKind::FiveHour).unwrap().used_pct, 23.5);
        assert_eq!(u.meter(MeterKind::Weekly).unwrap().resets_at, Some(9000));
    }

    #[test]
    fn expired_window_resets_to_zero() {
        let snap = snapshot_from_statusline(INPUT, 1000).to_string();
        let u = usage_from_snapshot(&snap, 2500).unwrap();
        assert_eq!(u.meter(MeterKind::FiveHour).unwrap().used_pct, 0.0);
        assert_eq!(u.meter(MeterKind::Weekly).unwrap().used_pct, 41.0);
    }

    #[test]
    fn missing_rate_limits_is_error() {
        let snap = snapshot_from_statusline(r#"{"model":{}}"#, 1).to_string();
        assert!(usage_from_snapshot(&snap, 2).is_err());
    }

    #[test]
    fn write_returns_status_line() {
        let d = std::env::temp_dir().join(format!("lq-claude-{}", std::process::id()));
        assert_eq!(write_snapshot(&d, INPUT, 1000).unwrap(), "5h 24% · 7d 41%");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Grava o último uso obtido pelo OAuth no mesmo formato do snapshot do statusLine (marcado `src: oauth`).
fn store_oauth_snapshot(dir: &std::path::Path, u: &Usage, now: i64) -> std::io::Result<()> {
    let win = |k: MeterKind| u.meter(k).map_or(Value::Null, |m| json!({"used": m.used_pct, "resets_at": m.resets_at}));
    let snap = json!({"ts": now, "src": "oauth", "five_hour": win(MeterKind::FiveHour), "seven_day": win(MeterKind::Weekly)});
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("claude.json.tmp");
    std::fs::write(&tmp, snap.to_string())?;
    std::fs::rename(&tmp, dir.join(SNAPSHOT))
}

#[cfg(test)]
mod oauth_snapshot_tests {
    use super::*;

    #[test]
    fn last_good_usage_survives_a_failed_refresh() {
        let d = std::env::temp_dir().join(format!("lq-claude-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let m = |kind, used_pct| Meter { kind, used_pct, resets_at: Some(9000), estimated: false };
        let u = Usage { provider: ProviderId::Claude, meters: vec![m(MeterKind::FiveHour, 65.0), m(MeterKind::Weekly, 49.0)] };
        store_oauth_snapshot(&d, &u, 1000).unwrap();
        let txt = std::fs::read_to_string(d.join(SNAPSHOT)).unwrap();
        assert_eq!(usage_from_snapshot(&txt, 1100).unwrap(), u); // antigo, mas melhor que nada
        assert_eq!(serde_json::from_str::<Value>(&txt).unwrap()["src"], "oauth");
        let _ = std::fs::remove_dir_all(&d);
    }
}
