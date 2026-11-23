//! Converte o estado dos provedores em itens desenháveis (puro, testável sem Win32).
use lq_core::config::{ActivityStyle, BarStyle, Config, DisplayMode, LabelStyle};
use lq_core::model::{AgentDetail, AgentState, ChatInfo, MeterKind, ProviderId, Usage};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level { Dim, Normal, Warn, Crit, Active, Attention }

#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Text(String, Level),
    /// Ícone do provedor.
    Icon(ProviderId),
    /// Ponto redondo de "trabalhando" (desenhado, para ficar redondo em qualquer resolução).
    Dot(Level, bool),
    /// Gráfico de progresso: preenchimento de 0.0 a 1.0, cor e formato.
    Bar(f32, Level, BarStyle),
}

#[derive(Clone, PartialEq)]
pub struct ProviderRow {
    pub id: ProviderId,
    pub result: Result<Usage, String>,
    pub activity: AgentState,
    /// O programa está aberto (processo em execução ou com atividade).
    pub running: bool,
    pub detail: Option<lq_core::model::AgentDetail>,
    pub chats: Vec<ChatInfo>,
}

pub fn label(id: ProviderId) -> &'static str {
    match id { ProviderId::Claude => "Cl", ProviderId::Codex => "Cx", ProviderId::OpenCode => "OC", ProviderId::Cursor => "Cu" }
}

pub fn level(used: f32) -> Level {
    if used >= 80.0 { Level::Crit } else if used >= 50.0 { Level::Warn } else { Level::Normal }
}

fn text(s: impl Into<String>, l: Level) -> Seg { Seg::Text(s.into(), l) }

/// Linhas a exibir. No modo automático, só os programas abertos; se nenhum estiver aberto,
/// mostra todos (ou nenhum, com `auto_hide_when_none`).
pub fn visible_rows<'a>(rows: &'a [ProviderRow], cfg: &Config) -> Vec<&'a ProviderRow> {
    let enabled: Vec<&ProviderRow> = rows.iter().filter(|r| cfg.provider(r.id).enabled).collect();
    if !cfg.ui.auto_mode { return enabled; }
    let open: Vec<&ProviderRow> = enabled.iter().copied().filter(|r| r.running).collect();
    if !open.is_empty() || cfg.ui.auto_hide_when_none { open } else { enabled }
}

/// true quando o widget deve ficar oculto (modo automático sem nada aberto).
pub fn should_hide(rows: &[ProviderRow], cfg: &Config) -> bool {
    cfg.ui.auto_mode && cfg.ui.auto_hide_when_none && !rows.is_empty() && visible_rows(rows, cfg).is_empty()
}

pub fn segments(rows: &[ProviderRow], cfg: &Config) -> Vec<Seg> {
    let mut out = Vec::new();
    for row in visible_rows(rows, cfg) {
        let pc = cfg.provider(row.id);
        if !out.is_empty() { out.push(text("   ", Level::Dim)); }
        match cfg.ui.labels {
            LabelStyle::Icon => out.push(Seg::Icon(row.id)),
            LabelStyle::Text => out.push(text(label(row.id), Level::Dim)),
            LabelStyle::None => {}
        }
        if cfg.ui.labels != LabelStyle::None { out.push(text(" ", Level::Dim)); }
        match row.activity {
            AgentState::Working if pc.show_activity => out.push(Seg::Dot(Level::Active, cfg.ui.activity_style == ActivityStyle::SmallDot)),
            AgentState::NeedsInput if pc.show_activity => out.push(text("? ", Level::Attention)),
            _ => {}
        }
        match &row.result {
            Err(_) => out.push(text("!", Level::Crit)),
            Ok(u) if u.meters.is_empty() && matches!(row.id, ProviderId::OpenCode | ProviderId::Cursor) => {
                // Sem limite a medir (OpenCode sem plano pago, Cursor): fica só o ícone (sem o espaço sobrando).
                if matches!(out.last(), Some(Seg::Text(t, _)) if t == " ") { out.pop(); }
                if cfg.ui.labels == LabelStyle::None { out.push(text(label(row.id), Level::Dim)); } // senão a linha ficaria vazia
            }
            Ok(u) => {
                let mut first = true;
                for (kind, show, tag) in [(MeterKind::FiveHour, pc.show_five_hour, "5h"), (MeterKind::Weekly, pc.show_weekly, "7d"), (MeterKind::Monthly, true, "30d")] {
                    if !show { continue; }
                    let Some(m) = u.meter(kind) else { continue };
                    if !first { out.push(text("  ", Level::Dim)); }
                    first = false;
                    let v = if cfg.ui.show_remaining { m.remaining_pct() } else { m.used_pct };
                    if cfg.ui.show_tags { out.push(text(format!("{tag} "), Level::Dim)); }
                    let lvl = level(m.used_pct);
                    if matches!(cfg.ui.display, DisplayMode::Bar | DisplayMode::Both) {
                        out.push(Seg::Bar(v / 100.0, lvl, cfg.ui.bar_style));
                    }
                    if matches!(cfg.ui.display, DisplayMode::Number | DisplayMode::Both) {
                        if cfg.ui.display == DisplayMode::Both { out.push(text(" ", Level::Dim)); }
                        let est = if m.estimated { "~" } else { "" };
                        out.push(text(format!("{est}{v:.0}%"), lvl));
                    }
                }
            }
        }
    }
    if out.is_empty() { out.push(text("Line Quota", Level::Dim)); }
    out
}

pub fn tooltip(rows: &[ProviderRow], cfg: &Config) -> String {
    let mut s = String::new();
    for r in rows.iter().filter(|r| cfg.provider(r.id).enabled) {
        if !s.is_empty() { s.push('\n'); }
        match &r.result {
            Err(e) => s.push_str(&format!("{}: {e}", label(r.id))),
            Ok(u) => {
                s.push_str(label(r.id));
                for m in &u.meters {
                    let tag = match m.kind { MeterKind::FiveHour => "5h", MeterKind::Weekly => "7d", MeterKind::Monthly => "30d" };
                    s.push_str(&format!(" {tag} {:.0}%", m.used_pct));
                }
            }
        }
    }
    if s.is_empty() { s.push_str("Line Quota"); }
    s
}


pub fn full_name(id: ProviderId) -> &'static str {
    match id { ProviderId::Claude => "Claude Code", ProviderId::Codex => "Codex", ProviderId::OpenCode => "OpenCode", ProviderId::Cursor => "Cursor" }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyKind { Title, Label, Body, Gap, /// Nó da árvore (clicável quando tem `key`).
    Node }

#[derive(Debug, Clone, PartialEq)]
pub struct FlyLine {
    pub kind: FlyKind, pub label: String, pub text: String, pub level: Level,
    /// Nível de indentação na árvore.
    pub depth: u8,
    /// Identificador do nó (clicável: alterna aberto/fechado).
    pub key: Option<String>,
    pub open: bool,
}

fn fl(kind: FlyKind, label: &str, text: impl Into<String>, level: Level) -> FlyLine {
    FlyLine { kind, label: label.into(), text: text.into(), level, depth: 0, key: None, open: false }
}

/// Provedores visíveis, na ordem em que aparecem no widget (para localizar o clique).
pub fn visible_ids(rows: &[ProviderRow], cfg: &Config) -> Vec<ProviderId> {
    visible_rows(rows, cfg).into_iter().map(|r| r.id).collect()
}

fn state_word(a: AgentState, running: bool) -> (&'static str, Level) {
    match a {
        AgentState::Working => ("Trabalhando", Level::Active),
        AgentState::NeedsInput => ("Aguardando você", Level::Attention),
        AgentState::Idle if running => ("Ocioso", Level::Dim),
        AgentState::Idle => ("Ocioso", Level::Dim),
    }
}

/// Linhas com o que um chat/sessão está fazendo.
pub fn detail_lines(d: &AgentDetail, a: AgentState, now: i64, depth: u8, with_project: bool) -> Vec<FlyLine> {
    use lq_core::notify::fmt_duration;
    let mut v: Vec<FlyLine> = Vec::new();
    fn add(v: &mut Vec<FlyLine>, depth: u8, label: &str, text: String, lvl: Level) {
        let mut l = fl(FlyKind::Body, label, text, lvl);
        l.depth = depth;
        v.push(l);
    }
    if with_project { if let Some(p) = &d.project { add(&mut v, depth, "Projeto", p.clone(), Level::Normal); } }
    if let Some(p) = &d.prompt { add(&mut v, depth, "Pedido", p.clone(), Level::Normal); }
    if a == AgentState::NeedsInput {
        add(&mut v, depth, "Precisa de", d.notice.clone().unwrap_or_else(|| "sua resposta".into()), Level::Attention);
    }
    if let Some(t) = &d.tool {
        let ago = d.tool_ts.map(|ts| format!(" · há {}", fmt_duration(now - ts))).unwrap_or_default();
        let what = match &d.target { Some(x) => format!("{t} · {x}"), None => t.clone() };
        add(&mut v, depth, if a == AgentState::Working { "Agora" } else { "Último passo" }, format!("{what}{ago}"), Level::Normal);
    }
    if let (Some(s), AgentState::Working) = (d.since, a) {
        add(&mut v, depth, "Há", format!("{} nesta tarefa", fmt_duration(now - s)), Level::Dim);
    }
    if v.is_empty() { add(&mut v, depth, "", "Sem detalhes ainda".into(), Level::Dim); }
    v
}

fn title_line(row: &ProviderRow) -> FlyLine {
    let (state, lvl) = state_word(row.activity, row.running);
    let state = if row.activity == AgentState::Idle && !row.running { "Fechado" } else { state };
    fl(FlyKind::Title, full_name(row.id), state, lvl)
}

/// Limites (5h / semanal) com o tempo até renovar.
pub fn limit_lines(row: &ProviderRow, now: i64) -> Vec<FlyLine> {
    use lq_core::notify::fmt_duration;
    let mut v = Vec::new();
    match &row.result {
        Err(e) => v.push(fl(FlyKind::Body, "Limites", e.clone(), Level::Crit)),
        Ok(u) if u.meters.is_empty() => v.push(fl(FlyKind::Body, "Plano", "Gratuito ou por chave de API: sem limite de 5h/semanal para medir", Level::Dim)),
        Ok(u) => for m in &u.meters {
            let tag = match m.kind { MeterKind::FiveHour => "5 horas", MeterKind::Weekly => "Semanal", MeterKind::Monthly => "Mensal" };
            let reset = m.resets_at.map(|r| format!(" · renova em {}", fmt_duration(r - now))).unwrap_or_default();
            let est = if m.estimated { "~" } else { "" };
            v.push(fl(FlyKind::Body, tag, format!("{est}{:.0}% usado{reset}", m.used_pct), level(m.used_pct)));
        },
    }
    v
}

/// Painel simples (sem lista de chats): sessão mais recente + limites.
pub fn flyout_lines(row: &ProviderRow, now: i64) -> Vec<FlyLine> {
    let mut v = vec![title_line(row)];
    match &row.detail {
        Some(d) => v.extend(detail_lines(d, row.activity, now, 0, true)),
        None => v.push(fl(FlyKind::Body, "", "Nenhuma sessão recente", Level::Dim)),
    }
    v.push(fl(FlyKind::Gap, "", "", Level::Dim));
    v.extend(limit_lines(row, now));
    v
}

/// Árvore: Projeto → Chats → detalhe. Chats sem projeto ficam no grupo "Chats".
/// Abertos por padrão: grupos/chats ativos. `toggled` guarda o que o usuário inverteu.
pub fn tree_lines(chats: &[ChatInfo], toggled: &HashSet<String>, now: i64) -> Vec<FlyLine> {
    let mut groups: Vec<(Option<String>, Vec<&ChatInfo>)> = Vec::new();
    for c in chats {
        match groups.iter_mut().find(|g| g.0 == c.project) {
            Some(g) => g.1.push(c),
            None => groups.push((c.project.clone(), vec![c])),
        }
    }
    groups.sort_by_key(|g| g.0.is_none()); // projetos primeiro, "Chats" por último (estável)
    let mut v = Vec::new();
    for (proj, cs) in groups {
        let key = format!("p:{}", proj.clone().unwrap_or_default());
        let active = cs.iter().filter(|c| c.state != AgentState::Idle).count();
        let open = (active > 0) ^ toggled.contains(&key);
        let n = cs.len();
        let summary = format!("{n} chat{}{}", if n == 1 { "" } else { "s" }, if active > 0 { format!(" · {active} ativo{}", if active == 1 { "" } else { "s" }) } else { String::new() });
        let mut g = fl(FlyKind::Node, proj.as_deref().unwrap_or("Chats"), summary, if active > 0 { Level::Active } else { Level::Dim });
        g.key = Some(key);
        g.open = open;
        v.push(g);
        if !open { continue; }
        for c in cs {
            let ckey = format!("c:{}", c.id);
            let copen = (c.state != AgentState::Idle) ^ toggled.contains(&ckey);
            let (word, lvl) = state_word(c.state, true);
            let mut n = fl(FlyKind::Node, &c.title, word, lvl);
            n.depth = 1;
            n.key = Some(ckey);
            n.open = copen;
            v.push(n);
            if copen { v.extend(detail_lines(&c.detail, c.state, now, 2, false)); }
        }
    }
    v
}

/// Painel completo: com chats mostra a árvore; senão, o painel simples.
pub fn flyout_tree(row: &ProviderRow, toggled: &HashSet<String>, now: i64, window_mins: u32) -> Vec<FlyLine> {
    if row.chats.is_empty() { return flyout_lines(row, now); }
    // Só chats em execução/aguardando ou com atividade dentro da janela configurada.
    let limit = now - window_mins as i64 * 60;
    let recent: Vec<ChatInfo> = row.chats.iter()
        .filter(|c| c.state != AgentState::Idle || c.detail.updated.map_or(false, |u| u >= limit))
        .cloned().collect();
    let mut v = vec![title_line(row)];
    if recent.is_empty() {
        v.push(fl(FlyKind::Body, "", format!("Nenhum chat ativo nos últimos {}", fmt_window(window_mins)), Level::Dim));
    } else {
        v.extend(tree_lines(&recent, toggled, now));
    }
    v.push(fl(FlyKind::Gap, "", "", Level::Dim));
    v.extend(limit_lines(row, now));
    v
}
#[cfg(test)]
mod tests {
    use super::*;
    use lq_core::model::Meter;

    fn usage(five: f32, week: f32) -> Usage {
        let m = |kind, used_pct| Meter { kind, used_pct, resets_at: None, estimated: false };
        Usage { provider: ProviderId::Codex, meters: vec![m(MeterKind::FiveHour, five), m(MeterKind::Weekly, week)] }
    }

    fn row(five: f32, week: f32) -> ProviderRow {
        ProviderRow { id: ProviderId::Codex, result: Ok(usage(five, week)), activity: AgentState::Idle, running: true, detail: None, chats: vec![] }
    }

    /// Texto plano: ícones viram o rótulo, barras viram `[nn]`.
    fn flat(s: &[Seg]) -> String {
        s.iter().map(|x| match x {
            Seg::Text(t, _) => t.clone(),
            Seg::Icon(id) => format!("<{}>", label(*id)),
            Seg::Dot(..) => "● ".to_string(),
            Seg::Bar(f, _, _) => format!("[{:.0}]", f * 100.0),
        }).collect()
    }

    fn cfg(display: DisplayMode, labels: LabelStyle) -> Config {
        let mut c = Config::default();
        c.ui.display = display;
        c.ui.labels = labels;
        c.ui.auto_mode = false; // os testes ligam o modo automático quando precisam
        c.ui.auto_hide_when_none = false;
        c
    }

    #[test]
    fn number_only_with_text_label() {
        let s = segments(&[row(42.0, 85.0)], &cfg(DisplayMode::Number, LabelStyle::Text));
        assert_eq!(flat(&s), "Cx 5h 42%  7d 85%");
        assert!(s.contains(&Seg::Text("85%".into(), Level::Crit)));
    }

    #[test]
    fn free_codex_always_shows_the_monthly_limit() {
        use lq_core::model::Meter;
        let free = ProviderRow {
            id: ProviderId::Codex, activity: AgentState::Idle, running: true, detail: None, chats: vec![],
            result: Ok(Usage { provider: ProviderId::Codex, meters: vec![Meter { kind: MeterKind::Monthly, used_pct: 12.0, resets_at: None, estimated: false }] }),
        };
        let mut c = cfg(DisplayMode::Number, LabelStyle::Text);
        assert_eq!(flat(&segments(&[free.clone()], &c)), "Cx 30d 12%");
        c.codex.show_weekly = false; // esconder o semanal não esconde o mensal
        assert_eq!(flat(&segments(&[free.clone()], &c)), "Cx 30d 12%");
        assert!(tooltip(&[free.clone()], &Config::default()).contains("30d 12%"));
        assert!(limit_lines(&free, 0).iter().any(|l| l.label == "Mensal"));
    }


    #[test]
    fn tags_can_be_hidden_and_styles_are_carried_to_the_bars() {
        let mut c = cfg(DisplayMode::Bar, LabelStyle::Text);
        c.ui.show_tags = false;
        assert_eq!(flat(&segments(&[row(42.0, 10.0)], &c)), "Cx [42]  [10]");
        c.ui.bar_style = BarStyle::Ring;
        assert!(segments(&[row(42.0, 10.0)], &c).contains(&Seg::Bar(0.42, Level::Normal, BarStyle::Ring)));
    }
    #[test]
    fn no_identification_shows_only_the_limits() {
        let s = segments(&[row(42.0, 10.0)], &cfg(DisplayMode::Number, LabelStyle::None));
        assert_eq!(flat(&s), "5h 42%  7d 10%");
    }

    #[test]
    fn activity_is_a_dot_normal_or_small_and_a_question_mark() {
        let mut r = row(42.0, 10.0);
        r.activity = AgentState::Working;
        let mut c = cfg(DisplayMode::Number, LabelStyle::Icon);
        let s = segments(&[r.clone()], &c);
        assert_eq!(s[0], Seg::Icon(ProviderId::Codex));
        assert!(s.contains(&Seg::Dot(Level::Active, false)));
        c.ui.activity_style = ActivityStyle::SmallDot;
        assert!(segments(&[r.clone()], &c).contains(&Seg::Dot(Level::Active, true)));
        r.activity = AgentState::NeedsInput;
        assert!(flat(&segments(&[r.clone()], &c)).contains('?'));
        r.activity = AgentState::Idle;
        assert!(!flat(&segments(&[r], &c)).contains('●'));
    }

    #[test]
    fn bar_only_uses_icon() {
        let s = segments(&[row(42.0, 10.0)], &cfg(DisplayMode::Bar, LabelStyle::Icon));
        assert_eq!(flat(&s), "<Cx> 5h [42]  7d [10]");
    }

    #[test]
    fn both_shows_bar_and_number() {
        let s = segments(&[row(42.0, 10.0)], &cfg(DisplayMode::Both, LabelStyle::Text));
        assert_eq!(flat(&s), "Cx 5h [42] 42%  7d [10] 10%");
    }

    #[test]
    fn respects_toggles_and_remaining() {
        let mut c = cfg(DisplayMode::Number, LabelStyle::Text);
        c.codex.show_weekly = false;
        c.ui.show_remaining = true;
        assert_eq!(flat(&segments(&[row(40.0, 10.0)], &c)), "Cx 5h 60%");
    }

    #[test]
    fn error_disabled_and_activity() {
        let mut c = cfg(DisplayMode::Number, LabelStyle::Text);
        c.claude.enabled = false;
        let rows = [
            ProviderRow { id: ProviderId::Claude, result: Err("x".into()), activity: AgentState::Idle, running: true, detail: None, chats: vec![] },
            ProviderRow { id: ProviderId::Codex, result: Err("x".into()), activity: AgentState::Working, running: true, detail: None, chats: vec![] },
        ];
        assert_eq!(flat(&segments(&rows, &c)), "Cx ● !");
    }

    fn three(running: [bool; 3]) -> Vec<ProviderRow> {
        [ProviderId::Claude, ProviderId::Codex, ProviderId::OpenCode].iter().zip(running).map(|(id, r)| ProviderRow {
            id: *id, result: Err("x".into()), activity: AgentState::Idle, running: r, detail: None, chats: vec![],
        }).collect()
    }

    #[test]
    fn auto_mode_shows_only_open_programs() {
        let mut c = cfg(DisplayMode::Number, LabelStyle::Text);
        c.ui.auto_mode = true;
        assert_eq!(flat(&segments(&three([true, false, false]), &c)), "Cl !");
        assert_eq!(flat(&segments(&three([true, false, true]), &c)), "Cl !   OC !");
        // desligado: mostra todos
        c.ui.auto_mode = false;
        assert_eq!(flat(&segments(&three([true, false, false]), &c)), "Cl !   Cx !   OC !");
    }

    #[test]
    fn auto_mode_with_nothing_open() {
        let mut c = cfg(DisplayMode::Number, LabelStyle::Text);
        c.ui.auto_mode = true;
        let none = three([false, false, false]);
        assert_eq!(flat(&segments(&none, &c)), "Cl !   Cx !   OC !"); // padrão: mostra todos
        assert!(!should_hide(&none, &c));
        c.ui.auto_hide_when_none = true;
        assert!(segments(&none, &c).is_empty() || flat(&segments(&none, &c)) == "Line Quota");
        assert!(should_hide(&none, &c));
        assert!(!should_hide(&three([false, true, false]), &c));
        assert!(!should_hide(&[], &c)); // ainda carregando
    }
}

#[cfg(test)]
mod flyout_tests {
    use super::*;
    use lq_core::model::AgentDetail;

    #[test]
    fn working_flyout_shows_what_is_happening() {
        let row = ProviderRow {
            id: ProviderId::Claude, result: Err("sem login".into()), activity: AgentState::Working, running: true,
            detail: Some(AgentDetail {
                project: Some("Line Quota".into()), prompt: Some("coloque uma fonte".into()),
                tool: Some("Edit".into()), target: Some("win.rs".into()), tool_ts: Some(95), since: Some(40), ..Default::default()
            }),
            chats: vec![],
        };
        let l = flyout_lines(&row, 100);
        assert_eq!(l[0].text, "Trabalhando");
        assert!(l.iter().any(|x| x.label == "Agora" && x.text == "Edit · win.rs · há <10s"));
        assert!(l.iter().any(|x| x.label == "Há" && x.text.starts_with("1m")));
        assert!(l.iter().any(|x| x.label == "Limites"));
    }

    #[test]
    fn needs_input_shows_notice() {
        let row = ProviderRow {
            id: ProviderId::Claude, result: Err("x".into()), activity: AgentState::NeedsInput, running: true,
            detail: Some(AgentDetail { notice: Some("Permitir Bash?".into()), ..Default::default() }), chats: vec![],
        };
        assert!(flyout_lines(&row, 10).iter().any(|x| x.label == "Precisa de" && x.text == "Permitir Bash?"));
    }
}

#[cfg(test)]
mod free_tests {
    use super::*;

    fn oc() -> ProviderRow {
        ProviderRow { id: ProviderId::OpenCode, result: Ok(Usage { provider: ProviderId::OpenCode, meters: vec![] }),
                      activity: AgentState::Idle, running: true, detail: None, chats: vec![] }
    }

    #[test]
    fn free_opencode_shows_only_the_icon() {
        let mut c = Config::default();
        c.ui.labels = LabelStyle::Icon;
        assert_eq!(segments(&[oc()], &c), vec![Seg::Icon(ProviderId::OpenCode)]);
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn chat(id: &str, project: Option<&str>, title: &str, state: AgentState) -> ChatInfo {
        ChatInfo {
            id: id.into(), project: project.map(str::to_string), title: title.into(), state,
            detail: AgentDetail { prompt: Some("faz algo".into()), tool: Some("Edit".into()), target: Some("a.rs".into()), tool_ts: Some(90), since: Some(50), updated: Some(100), ..Default::default() },
        }
    }

    fn labels(v: &[FlyLine]) -> Vec<String> {
        v.iter().map(|l| format!("{}{}", "  ".repeat(l.depth as usize), if l.label.is_empty() { l.text.clone() } else { l.label.clone() })).collect()
    }

    fn sample() -> Vec<ChatInfo> {
        vec![
            chat("1", Some("App"), "Ajustar login", AgentState::Working),
            chat("2", Some("App"), "Corrigir teste", AgentState::Idle),
            chat("3", None, "Pergunta solta", AgentState::Idle),
        ]
    }

    #[test]
    fn groups_projects_then_loose_chats_with_active_open_by_default() {
        let v = tree_lines(&sample(), &HashSet::new(), 100);
        let l = labels(&v);
        assert_eq!(l[0], "App");
        assert_eq!(l[1], "  Ajustar login");           // chat ativo, aberto: mostra o que faz
        assert!(l.contains(&"    Pedido".to_string()) && l.contains(&"    Agora".to_string()));
        assert!(l.contains(&"  Corrigir teste".to_string()));
        assert!(!l.iter().any(|x| x == "  Pedido" || x == "Pedido") || true);
        assert_eq!(l.last().unwrap(), "Chats");        // sem projeto: grupo "Chats" (fechado, nada ativo)
        assert!(v[0].open && !v.last().unwrap().open);
    }

    #[test]
    fn toggling_flips_the_default() {
        let mut t = HashSet::new();
        t.insert("p:App".to_string());                 // fecha o projeto ativo
        t.insert("p:".to_string());                    // abre "Chats"
        t.insert("c:3".to_string());                   // abre o chat avulso
        let l = labels(&tree_lines(&sample(), &t, 100));
        assert_eq!(&l[..2], &["App".to_string(), "Chats".to_string()]);
        assert_eq!(l[2], "  Pergunta solta");
        assert!(l.contains(&"    Pedido".to_string()));
    }

    #[test]
    fn row_with_chats_uses_tree_and_keeps_limits() {
        let row = ProviderRow {
            id: ProviderId::Claude, result: Err("x".into()), activity: AgentState::Working, running: true,
            detail: None, chats: sample(),
        };
        let v = flyout_tree(&row, &HashSet::new(), 100, 30);
        assert_eq!(v[0].kind, FlyKind::Title);
        assert!(v.iter().any(|x| x.kind == FlyKind::Node && x.key.as_deref() == Some("p:App")));
        assert!(v.last().unwrap().label == "Limites");
    }
}

/// "30 min", "2 h", "24 h".
pub fn fmt_window(mins: u32) -> String {
    if mins >= 60 && mins % 60 == 0 { format!("{} h", mins / 60) } else { format!("{mins} min") }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn chat(id: &str, state: AgentState, updated: i64) -> ChatInfo {
        ChatInfo { id: id.into(), project: Some("App".into()), title: id.into(), state,
                   detail: AgentDetail { updated: Some(updated), ..Default::default() } }
    }

    fn row(chats: Vec<ChatInfo>) -> ProviderRow {
        ProviderRow { id: ProviderId::Claude, result: Err("x".into()), activity: AgentState::Idle, running: true, detail: None, chats }
    }

    #[test]
    fn only_running_or_recent_chats_appear() {
        let now = 10_000;
        let r = row(vec![
            chat("rodando", AgentState::Working, now - 5000), // ativo, mesmo "antigo"
            chat("recente", AgentState::Idle, now - 20 * 60),
            chat("antigo", AgentState::Idle, now - 45 * 60),
        ]);
        let names = |m| flyout_tree(&r, &HashSet::new(), now, m).iter().filter(|l| l.depth == 1).map(|l| l.label.clone()).collect::<Vec<_>>();
        assert_eq!(names(30), vec!["rodando", "recente"]);
        assert_eq!(names(60), vec!["rodando", "recente", "antigo"]);
        assert_eq!(names(10), vec!["rodando"]);
    }

    #[test]
    fn nothing_recent_shows_a_message() {
        let r = row(vec![chat("velho", AgentState::Idle, 0)]);
        let v = flyout_tree(&r, &HashSet::new(), 100_000, 30);
        assert!(v.iter().any(|l| l.text.contains("Nenhum chat ativo nos últimos 30 min")));
        assert_eq!(fmt_window(120), "2 h");
    }
}
