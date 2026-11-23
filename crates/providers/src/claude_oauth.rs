//! Limites do Claude via endpoint OAuth de uso (não oficial), usando o token que o
//! `claude login` grava em `~/.claude/.credentials.json`. O token nunca é logado.
use lq_core::model::{Meter, MeterKind, ProviderId, Usage};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TTL_OK: Duration = Duration::from_secs(180);
const TTL_ERR: Duration = Duration::from_secs(60);
const TTL_RATE_LIMITED: Duration = Duration::from_secs(5 * 60);

pub const NO_LOGIN: &str = "Claude: clique com o botão direito > Conta Claude > Entrar";

fn credentials_path() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default())
        .join(".claude").join(".credentials.json")
}

/// Extrai (token, expira_em_ms) do JSON de credenciais.
pub fn parse_credentials(json: &str) -> Result<(String, Option<i64>), String> {
    let v: Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|_| "credenciais ilegíveis".to_string())?;
    let o = &v["claudeAiOauth"];
    let tok = o["accessToken"].as_str().ok_or_else(|| NO_LOGIN.to_string())?;
    Ok((tok.to_string(), o["expiresAt"].as_i64()))
}

/// "2026-11-19T19:19:59.632Z" / "...+00:00" -> segundos Unix (UTC).
pub fn iso_to_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 { return None; }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    // dias desde 1970 (algoritmo de Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut t = days * 86400 + h * 3600 + mi * 60 + se;
    // fuso: sufixo +hh:mm / -hh:mm (Z = UTC)
    if let Some(i) = s[19..].find(['+', '-']) {
        let z = &s[19 + i..];
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let (oh, om) = (z.get(1..3)?.parse::<i64>().ok()?, z.get(4..6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0));
        t -= sign * (oh * 3600 + om * 60);
    }
    Some(t)
}

fn meter(w: &Value, kind: MeterKind) -> Option<Meter> {
    let used = w["utilization"].as_f64()? as f32;
    Some(Meter { kind, used_pct: used.clamp(0.0, 100.0), resets_at: w["resets_at"].as_str().and_then(iso_to_epoch), estimated: false })
}

pub fn parse_usage(json: &str) -> Result<Usage, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("resposta inválida: {e}"))?;
    let meters: Vec<Meter> = [meter(&v["five_hour"], MeterKind::FiveHour), meter(&v["seven_day"], MeterKind::Weekly)]
        .into_iter().flatten().collect();
    if meters.is_empty() { return Err("Claude sem dados de limite (requer plano Pro/Max)".into()); }
    Ok(Usage { provider: ProviderId::Claude, meters })
}

/// Token do login do widget (preferido) ou, se não houver, o do `claude login`.
fn token(now_ms: i64) -> Result<String, String> {
    if let Some(r) = crate::claude_auth::access_token(now_ms) { return r; }
    let txt = std::fs::read_to_string(credentials_path()).map_err(|_| NO_LOGIN.to_string())?;
    let (token, expires) = parse_credentials(&txt)?;
    if expires.is_some_and(|e| e <= now_ms) {
        return Err("Claude: token expirado — abra o Claude Code para renová-lo".into());
    }
    Ok(token)
}

fn fetch_now(now_ms: i64) -> Result<Usage, String> {
    let token = token(now_ms)?;
    let body = ureq::get(URL)
        .timeout(Duration::from_secs(10))
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "line-quota")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("Claude: HTTP {c}"),
            ureq::Error::Transport(_) => "Claude: sem conexão".to_string(),
        })?
        .into_string().map_err(|e| e.to_string())?;
    parse_usage(&body)
}

/// Cache com TTL: o poller roda a cada 2 s, a rede só a cada 3 min (1 min após erro).
#[derive(Default)]
pub struct OAuth { cache: Mutex<Option<(Instant, u32, Result<Usage, String>)>> }

impl OAuth {
    pub fn get(&self, now_ms: i64) -> Result<Usage, String> {
        let gen = crate::claude_auth::generation(); // muda em login/logout: invalida o cache
        let mut c = self.cache.lock().unwrap();
        if let Some((at, g, r)) = c.as_ref() {
            // 429 = limite de requisições: espera bem mais antes de tentar de novo.
            let ttl = match r { Ok(_) => TTL_OK, Err(e) if e.contains("429") => TTL_RATE_LIMITED, Err(_) => TTL_ERR };
            if *g == gen && at.elapsed() < ttl { return r.clone(); }
        }
        let r = fetch_now(now_ms);
        *c = Some((Instant::now(), gen, r.clone()));
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_parsing() {
        assert_eq!(iso_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_epoch("2026-11-19T19:19:59.632Z"), Some(1795115999));
        assert_eq!(iso_to_epoch("2026-11-19T16:19:59-03:00"), Some(1795115999));
        assert_eq!(iso_to_epoch("lixo"), None);
    }

    #[test]
    fn usage_parsing() {
        let j = r#"{"five_hour":{"utilization":37.0,"resets_at":"2026-11-19T19:19:59.632Z"},"seven_day":{"utilization":5,"resets_at":null},"seven_day_opus":null}"#;
        let u = parse_usage(j).unwrap();
        assert_eq!(u.meter(MeterKind::FiveHour).unwrap().used_pct, 37.0);
        assert_eq!(u.meter(MeterKind::FiveHour).unwrap().resets_at, Some(1795115999));
        assert_eq!(u.meter(MeterKind::Weekly).unwrap().resets_at, None);
        assert!(parse_usage(r#"{"five_hour":null,"seven_day":null}"#).is_err());
    }

    #[test]
    fn credentials_parsing() {
        let (t, e) = parse_credentials(r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":123}}"#).unwrap();
        assert_eq!((t.as_str(), e), ("abc", Some(123)));
        assert!(parse_credentials(r#"{}"#).is_err());
    }
}
