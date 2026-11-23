//! Regras de notificação (puras, sem Win32): limiares de uso, reset, tarefa concluída e
//! agente aguardando resposta. O app só exibe o que `Notifier::update` devolve.
use crate::alert::AlertEngine;
use crate::config::AlertConfig;
use crate::model::{AgentState, MeterKind, ProviderId, Usage};
use std::collections::{HashMap, HashSet};

/// Tempo parado (sem sinal de trabalho) antes de considerar a tarefa concluída.
/// Evita avisos falsos quando a atividade pisca (ex.: escritas em rajadas).
const DONE_DEBOUNCE_SECS: i64 = 6;
/// Queda de uso que indica que a janela de limite foi renovada.
const RESET_DROP_PCT: f32 = 40.0;
const RESET_MIN_PREV_PCT: f32 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity { Info, Warn, Crit }

/// Barra de progresso opcional exibida no toast (ex.: uso do limite).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress { pub fraction: f32, pub title: String, pub status: String }

#[derive(Debug, Clone, PartialEq)]
pub struct Notification { pub title: String, pub body: String, pub severity: Severity, pub progress: Option<Progress> }

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>, severity: Severity) -> Self {
        Self { title: title.into(), body: body.into(), severity, progress: None }
    }
}

/// Estado de um provedor num instante.
pub struct Snap { pub id: ProviderId, pub usage: Option<Usage>, pub activity: AgentState }

#[derive(Default)]
struct Track { prev: AgentState, work_start: Option<i64>, idle_since: Option<i64> }

#[derive(Default)]
pub struct Notifier {
    engine: AlertEngine,
    seen: HashSet<(ProviderId, MeterKind)>,
    prev_used: HashMap<(ProviderId, MeterKind), f32>,
    track: HashMap<ProviderId, Track>,
}

pub fn provider_name(id: ProviderId) -> &'static str {
    match id { ProviderId::Claude => "Claude Code", ProviderId::Codex => "Codex", ProviderId::OpenCode => "OpenCode" }
}

fn meter_short(k: MeterKind) -> &'static str {
    match k { MeterKind::FiveHour => "5h", MeterKind::Weekly => "semana", MeterKind::Monthly => "mês" }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn meter_name(k: MeterKind) -> &'static str {
    match k { MeterKind::FiveHour => "limite de 5 horas", MeterKind::Weekly => "limite semanal", MeterKind::Monthly => "limite mensal" }
}

/// "1h 23m", "5m", "<1m".
pub fn fmt_duration(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 { format!("{}h {:02}m", s / 3600, (s % 3600) / 60) }
    else if s >= 60 { format!("{}m", s / 60) }
    else if s >= 10 { format!("{s}s") }
    else { "<10s".into() }
}

fn severity_for(threshold: f32) -> Severity {
    if threshold >= 95.0 { Severity::Crit } else if threshold >= 90.0 { Severity::Warn } else { Severity::Info }
}

impl Notifier {
    /// Compara o estado atual com o anterior e devolve o que deve ser avisado.
    /// A primeira observação de cada medidor só grava a linha de base (sem avisos ao iniciar).
    pub fn update(&mut self, snaps: &[Snap], cfg: &AlertConfig, now: i64) -> Vec<Notification> {
        let mut out = Vec::new();
        if !cfg.enabled { return out; }

        for s in snaps {
            if let Some(u) = &s.usage {
                self.usage_alerts(s.id, u, snaps, cfg, now, &mut out);
            }
            self.activity_alerts(s, cfg, now, &mut out);
        }
        out
    }

    fn usage_alerts(&mut self, id: ProviderId, u: &Usage, all: &[Snap], cfg: &AlertConfig, now: i64, out: &mut Vec<Notification>) {
        // OpenCode compartilha a conta do Codex: não avisar duas vezes o mesmo limite.
        let shared_with_codex = id == ProviderId::OpenCode && all.iter().any(|o| {
            o.id == ProviderId::Codex && o.usage.as_ref().is_some_and(|c| same_meters(c, u))
        });
        for m in &u.meters {
            let key = (id, m.kind);
            let first = self.seen.insert(key);
            let prev = self.prev_used.insert(key, m.used_pct);
            let crossed = self.engine.check(id, m.kind, m.used_pct, &cfg.thresholds);
            if first || shared_with_codex { continue; } // linha de base / duplicado

            if cfg.notify_reset && prev.is_some_and(|p| p >= RESET_MIN_PREV_PCT && m.used_pct <= p - RESET_DROP_PCT) {
                let mut n = Notification::new(
                    format!("🔄 {}: {} renovado", provider_name(id), meter_name(m.kind)),
                    format!("O uso voltou a {:.0}%. Pode trabalhar à vontade.", m.used_pct),
                    Severity::Info,
                );
                n.progress = Some(Progress { fraction: m.used_pct / 100.0, title: capitalize(meter_name(m.kind)), status: format!("{:.0}% usado", m.used_pct) });
                out.push(n);
            }
            if let Some(t) = crossed {
                let sev = severity_for(t);
                let icon = if sev == Severity::Crit { "🚨" } else { "⚠️" };
                let when = m.resets_at.map(|r| format!(" Renova em {}.", fmt_duration(r - now))).unwrap_or_default();
                let mut n = Notification::new(
                    format!("{icon} {}: {} em {:.0}%", provider_name(id), meter_short(m.kind), m.used_pct),
                    format!("Restam {:.0}% do {}.{when}", m.remaining_pct(), meter_name(m.kind)),
                    sev,
                );
                n.progress = Some(Progress { fraction: m.used_pct / 100.0, title: capitalize(meter_name(m.kind)), status: format!("{:.0}% usado", m.used_pct) });
                out.push(n);
            }
        }
    }

    fn activity_alerts(&mut self, s: &Snap, cfg: &AlertConfig, now: i64, out: &mut Vec<Notification>) {
        let t = self.track.entry(s.id).or_default();
        let prev = t.prev;
        t.prev = s.activity;
        match s.activity {
            AgentState::NeedsInput => {
                t.idle_since = None;
                t.work_start.get_or_insert(now);
                if prev != AgentState::NeedsInput && cfg.notify_needs_input {
                    out.push(Notification::new(
                        format!("💬 {} precisa de você", provider_name(s.id)),
                        "Aguardando sua resposta ou permissão.",
                        Severity::Warn,
                    ));
                }
            }
            AgentState::Working => {
                t.idle_since = None;
                t.work_start.get_or_insert(now);
            }
            AgentState::Idle => {
                let Some(start) = t.work_start else { return };
                let since = *t.idle_since.get_or_insert(now);
                if now - since >= DONE_DEBOUNCE_SECS {
                    let dur = since - start;
                    t.work_start = None;
                    t.idle_since = None;
                    if cfg.notify_task_done && dur >= cfg.task_done_min_secs {
                        out.push(Notification::new(
                            format!("✅ {} terminou", provider_name(s.id)),
                            format!("Tarefa concluída em {}.", fmt_duration(dur)),
                            Severity::Info,
                        ));
                    }
                }
            }
        }
    }
}

fn same_meters(a: &Usage, b: &Usage) -> bool {
    !a.meters.is_empty() && a.meters.len() == b.meters.len()
        && a.meters.iter().zip(&b.meters).all(|(x, y)| x.kind == y.kind && (x.used_pct - y.used_pct).abs() < 0.01 && x.resets_at == y.resets_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Meter;

    fn usage(id: ProviderId, five: f32, week: f32) -> Usage {
        let m = |kind, used_pct| Meter { kind, used_pct, resets_at: Some(10_000), estimated: false };
        Usage { provider: id, meters: vec![m(MeterKind::FiveHour, five), m(MeterKind::Weekly, week)] }
    }

    fn snap(id: ProviderId, five: f32, act: AgentState) -> Snap { Snap { id, usage: Some(usage(id, five, 1.0)), activity: act } }

    fn titles(v: &[Notification]) -> Vec<String> { v.iter().map(|n| n.title.clone()).collect() }

    #[test]
    fn first_observation_is_baseline_only() {
        let mut n = Notifier::default();
        // já em 96% ao iniciar: não avisa
        assert!(n.update(&[snap(ProviderId::Claude, 96.0, AgentState::Idle)], &AlertConfig::default(), 0).is_empty());
    }

    #[test]
    fn threshold_alert_fires_once_with_severity_and_countdown() {
        let mut n = Notifier::default();
        let c = AlertConfig::default();
        n.update(&[snap(ProviderId::Claude, 10.0, AgentState::Idle)], &c, 0);
        let r = n.update(&[snap(ProviderId::Claude, 83.0, AgentState::Idle)], &c, 100);
        assert_eq!(titles(&r), ["⚠️ Claude Code: 5h em 83%"]);
        assert_eq!(r[0].severity, Severity::Info);
        let p = r[0].progress.as_ref().unwrap();
        assert!((p.fraction - 0.83).abs() < 0.001 && p.status == "83% usado" && p.title == "Limite de 5 horas");
        assert!(r[0].body.contains("Renova em 2h 45m"));
        assert!(n.update(&[snap(ProviderId::Claude, 85.0, AgentState::Idle)], &c, 200).is_empty());
        let r = n.update(&[snap(ProviderId::Claude, 96.0, AgentState::Idle)], &c, 300);
        assert_eq!(r[0].severity, Severity::Crit);
        assert!(r[0].title.starts_with("🚨"));
    }

    #[test]
    fn reset_notification_and_rearm() {
        let mut n = Notifier::default();
        let c = AlertConfig::default();
        n.update(&[snap(ProviderId::Codex, 10.0, AgentState::Idle)], &c, 0);
        n.update(&[snap(ProviderId::Codex, 92.0, AgentState::Idle)], &c, 10);
        let r = n.update(&[snap(ProviderId::Codex, 3.0, AgentState::Idle)], &c, 20);
        assert_eq!(titles(&r), ["🔄 Codex: limite de 5 horas renovado"]);
        let r = n.update(&[snap(ProviderId::Codex, 81.0, AgentState::Idle)], &c, 30);
        assert_eq!(r.len(), 1); // limiar rearmado
        let mut off = AlertConfig::default();
        off.notify_reset = false;
        n.update(&[snap(ProviderId::Codex, 92.0, AgentState::Idle)], &off, 40);
        assert!(n.update(&[snap(ProviderId::Codex, 3.0, AgentState::Idle)], &off, 50).is_empty());
    }

    #[test]
    fn task_done_needs_min_duration_and_debounce() {
        let mut n = Notifier::default();
        let mut c = AlertConfig::default();
        c.task_done_min_secs = 30;
        let a = |s| snap(ProviderId::Claude, 0.0, s);
        n.update(&[a(AgentState::Working)], &c, 0);
        assert!(n.update(&[a(AgentState::Working)], &c, 40).is_empty());
        // pisca: Idle e volta a Working antes do debounce -> sem aviso
        assert!(n.update(&[a(AgentState::Idle)], &c, 41).is_empty());
        assert!(n.update(&[a(AgentState::Working)], &c, 44).is_empty());
        assert!(n.update(&[a(AgentState::Idle)], &c, 60).is_empty());
        let r = n.update(&[a(AgentState::Idle)], &c, 67);
        assert_eq!(titles(&r), ["✅ Claude Code terminou"]);
        assert!(r[0].body.contains("em 1m")); // 60s de trabalho
        assert!(n.update(&[a(AgentState::Idle)], &c, 80).is_empty()); // só uma vez
        // tarefa curta não avisa
        n.update(&[a(AgentState::Working)], &c, 100);
        n.update(&[a(AgentState::Idle)], &c, 105);
        assert!(n.update(&[a(AgentState::Idle)], &c, 112).is_empty());
    }

    #[test]
    fn needs_input_fires_on_edge_only() {
        let mut n = Notifier::default();
        let c = AlertConfig::default();
        let a = |s| snap(ProviderId::OpenCode, 0.0, s);
        n.update(&[a(AgentState::Working)], &c, 0);
        let r = n.update(&[a(AgentState::NeedsInput)], &c, 5);
        assert_eq!(titles(&r), ["💬 OpenCode precisa de você"]);
        assert!(n.update(&[a(AgentState::NeedsInput)], &c, 7).is_empty());
        let mut off = c.clone();
        off.notify_needs_input = false;
        n.update(&[a(AgentState::Working)], &off, 9);
        assert!(n.update(&[a(AgentState::NeedsInput)], &off, 11).is_empty());
    }

    #[test]
    fn opencode_sharing_codex_limits_does_not_duplicate() {
        let mut n = Notifier::default();
        let c = AlertConfig::default();
        let both = |five| vec![snap(ProviderId::Codex, five, AgentState::Idle), snap(ProviderId::OpenCode, five, AgentState::Idle)];
        n.update(&both(10.0), &c, 0);
        let r = n.update(&both(85.0), &c, 10);
        assert_eq!(titles(&r), ["⚠️ Codex: 5h em 85%"]);
    }

    #[test]
    fn disabled_is_silent() {
        let mut n = Notifier::default();
        let mut c = AlertConfig::default();
        c.enabled = false;
        n.update(&[snap(ProviderId::Claude, 10.0, AgentState::Idle)], &c, 0);
        assert!(n.update(&[snap(ProviderId::Claude, 99.0, AgentState::Idle)], &c, 10).is_empty());
    }

    #[test]
    fn duration_format() {
        assert_eq!(fmt_duration(3 * 3600 + 5 * 60), "3h 05m");
        assert_eq!(fmt_duration(300), "5m");
        assert_eq!(fmt_duration(30), "30s");
        assert_eq!(fmt_duration(3), "<10s");
        assert_eq!(fmt_duration(-5), "<10s");
    }
}
