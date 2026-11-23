pub mod claude;
pub mod claude_auth;
pub mod claude_oauth;
pub mod codex;
pub mod opencode;
pub mod antigravity;
pub mod cursor;
pub mod cursor_auth;
pub mod pkce;
pub mod secure;

/// Fuzz leve: todos os leitores de resposta/credenciais recebem amostras válidas truncadas, com bytes trocados e
/// lixo aleatório. Podem recusar (Err), mas nunca entrar em pânico.
#[cfg(test)]
mod fuzz_tests {
    fn rng(seed: &mut u64) -> u64 { *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17; *seed }

    #[test]
    fn parsers_never_panic_on_mutated_input() {
        let samples = [
            r#"{"five_hour":{"utilization":37.0,"resets_at":"2026-11-23T12:00:00Z"},"seven_day":{"utilization":10,"resets_at":null}}"#,
            r#"{"plan_type":"free","rate_limit":{"primary_window":{"used_percent":12.5,"limit_window_seconds":18000,"reset_after_seconds":100}}}"#,
            r#"{"individualUsage":{"plan":{"used":5,"limit":10,"totalPercentUsed":12.0}},"billingCycleEnd":"2026-12-23T15:16:51.605Z"}"#,
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[{"window":"weekly","remainingFraction":0.9,"resetTime":"2026-11-30T18:16:40Z"}]}]}}"#,
            r#"{"userStatus":{"cascadeModelConfigData":{"clientModelConfigs":[{"label":"Gemini","quotaInfo":{"remainingFraction":0.5,"resetTime":"2026-11-30T18:16:40Z"}}]}}}"#,
            r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1795000000000}}"#,
            r#"{"access_token":"a","refresh_token":"b","expires_in":3600}"#,
            "abcdEFGH1234#stateSTATE5678",
            "WorkosCursorSessionToken=user_01ABCDEFGHIJKLMNOP%3A%3AeyJhbGci.payload.sig",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let run = |s: &str| {
            let _ = crate::antigravity::parse_user_status(s);
            let _ = crate::antigravity::parse_quota_summary(s);
            let _ = crate::cursor::parse_usage_summary(s);
            let _ = crate::cursor_auth::parse_cookie(s);
            let _ = crate::claude_oauth::parse_usage(s);
            let _ = crate::claude_oauth::parse_credentials(s);
            let _ = crate::claude_oauth::iso_to_epoch(s);
            let _ = crate::claude_auth::parse_pasted(s);
            let _ = crate::claude_auth::parse_token_response(s, 0, Some("x"));
            let _ = crate::codex::parse_usage(s);
        };
        for s in samples {
            run(s);
            let b = s.as_bytes();
            for _ in 0..600 {
                let mut m = b.to_vec();
                for _ in 0..(1 + rng(&mut seed) % 4) {
                    let i = (rng(&mut seed) as usize) % m.len().max(1);
                    match rng(&mut seed) % 3 {
                        0 => { if !m.is_empty() { m[i] = rng(&mut seed) as u8; } }
                        1 => m.truncate(i),
                        _ => { let c = b"{}[]\":,0-9eE.nul"[(rng(&mut seed) % 16) as usize]; if i <= m.len() { m.insert(i, c); } }
                    }
                }
                run(&String::from_utf8_lossy(&m));
            }
        }
        for _ in 0..3000 {
            let n = (rng(&mut seed) % 200) as usize;
            let junk: Vec<u8> = (0..n).map(|_| rng(&mut seed) as u8).collect();
            run(&String::from_utf8_lossy(&junk));
        }
    }
}
