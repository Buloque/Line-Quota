#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
#[cfg(windows)]
mod setup;

use lq_core::config::Config;
use lq_core::notify::{Notifier, Snap};
use lq_core::provider::Provider;
use lq_ui::view::ProviderRow;
use lq_ui::win::{self, Shared};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

fn config_path() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("LineQuota").join("config.toml")
}

/// Quais agentes parecem instalados: pastas de dados do Claude Code, Codex e OpenCode.
fn detect_installed() -> [bool; 4] {
    let home = PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default());
    let opencode = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local").join("share")).join("opencode");
    let cursor = home.join(".cursor").is_dir() || PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("Cursor").is_dir();
    [home.join(".claude").is_dir(), home.join(".codex").is_dir(), opencode.is_dir(), cursor]
}

/// Primeira execução: liga só os agentes encontrados (nenhum é obrigatório). Se não achar nenhum, mantém todos.
fn apply_detected(c: &mut Config, found: [bool; 4]) {
    if found.iter().any(|f| *f) {
        c.claude.enabled = found[0];
        c.codex.enabled = found[1];
        c.opencode.enabled = found[2];
        c.cursor.enabled = found[3];
    }
}

/// Carrega a config; cria o arquivo com padrões se não existir.
fn load_config() -> Config {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => Config::from_toml(&s).unwrap_or_default(),
        Err(_) => {
            let mut c = Config::default();
            apply_detected(&mut c, detect_installed());
            if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
            let _ = std::fs::write(&p, c.to_toml());
            c
        }
    }
}

/// Provedores ativos. Novos provedores entram aqui.
fn providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(lq_providers::claude::Claude::new(lq_activity::data_dir())),
        Box::new(lq_providers::codex::Codex),
        Box::new(lq_providers::opencode::OpenCode::new()),
        Box::new(lq_providers::cursor::Cursor::new(lq_activity::data_dir())),
    ]
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Login do Claude feito pelo próprio widget (OAuth + PKCE), sem precisar do CLI.
struct ClaudeAccount;

impl lq_ui::Account for ClaudeAccount {
    fn logged_in(&self) -> bool { lq_providers::claude_auth::logged_in() }
    fn login_url(&self, full_scope: bool) -> Result<String, String> { lq_providers::claude_auth::begin(full_scope) }
    fn complete(&self, pasted: &str) -> Result<(), String> { lq_providers::claude_auth::complete(pasted, now_secs() * 1000) }
    fn logout(&self) { lq_providers::claude_auth::logout() }
    fn cursor_connected(&self) -> bool { lq_providers::cursor_auth::connected() }
    fn cursor_set_cookie(&self, pasted: &str) -> Result<(), String> { lq_providers::cursor_auth::save(pasted) }
    fn cursor_clear(&self) { lq_providers::cursor_auth::clear() }
    fn cursor_looks_like_cookie(&self, t: &str) -> bool { lq_providers::cursor_auth::looks_like_session(t) }
}

/// Modos de linha de comando usados pelo Claude Code (hooks/statusLine) e pela instalação.
/// Retorna true se tratou um modo (e o app não deve abrir a UI).
fn cli(args: &[String]) -> bool {
    #[cfg(windows)]
    if setup::run(args) { return true; }
    let read_stdin = || { let mut s = String::new(); let _ = std::io::stdin().read_to_string(&mut s); s };
    match args.first().map(String::as_str) {
        Some("--hook") => {
            let stdin = read_stdin();
            if args.get(1).map(String::as_str) == Some("cursor") {
                // O Cursor lê a resposta do hook no stdout.
                if let Ok(reply) = lq_activity::handle_cursor_hook(&lq_activity::data_dir(), &stdin, now_secs()) { print!("{reply}"); }
            } else {
                let _ = lq_activity::handle_claude_hook(&lq_activity::data_dir(), &stdin, now_secs());
            }
            true
        }
        Some("--cursor-usage") => { println!("{}", lq_providers::cursor::diagnose()); true }
        Some("--statusline") => {
            if let Ok(line) = lq_providers::claude::write_snapshot(&lq_activity::data_dir(), &read_stdin(), now_secs()) {
                println!("{line}");
            }
            true
        }
        #[cfg(windows)]
        Some("--unregister") => { win::unregister_toast_identity(); eprintln!("atalho e registro das notificações removidos"); true }
        Some(a @ ("--install-claude" | "--uninstall-claude")) => {
            let r = if a == "--install-claude" { install::install() } else { install::uninstall() };
            match r {
                Ok(log) => log.iter().for_each(|l| eprintln!("{l}")),
                Err(e) => eprintln!("erro: {e}"),
            }
            true
        }
        _ => false,
    }
}

/// Modo `--demo`: dados fictícios e config temporária (nada do usuário). Serve para gerar as imagens da documentação.
mod demo {
    use lq_core::model::{AgentDetail, AgentState, ChatInfo, Meter, MeterKind, ProviderId, Usage};
    use lq_ui::view::ProviderRow;

    fn meter(kind: MeterKind, used_pct: f32, resets_in: i64, now: i64) -> Meter {
        Meter { kind, used_pct, resets_at: Some(now + resets_in), estimated: false }
    }

    fn chat(id: &str, project: Option<&str>, title: &str, state: AgentState, prompt: &str, tool: Option<(&str, &str)>, now: i64) -> ChatInfo {
        ChatInfo {
            id: id.into(), project: project.map(str::to_string), title: title.into(), state,
            detail: AgentDetail {
                project: project.map(str::to_string), prompt: Some(prompt.into()),
                tool: tool.map(|t| t.0.into()), target: tool.map(|t| t.1.into()), tool_ts: Some(now - 4),
                since: Some(now - 95), updated: Some(now - if state == AgentState::Idle { 600 } else { 2 }), ..Default::default()
            },
        }
    }

    /// `free`: planos gratuitos (Codex só com o limite mensal; OpenCode sem limites, só o ícone).
    pub fn rows(now: i64, free: bool) -> Vec<ProviderRow> {
        let claude = Usage { provider: ProviderId::Claude, meters: vec![
            meter(MeterKind::FiveHour, 63.0, 5400, now), meter(MeterKind::Weekly, 41.0, 4 * 86400, now)] };
        // Codex com todas as janelas (5 h, semanal e mensal), para mostrar cada formato.
        let codex = Usage { provider: ProviderId::Codex, meters: if free { vec![meter(MeterKind::Monthly, 28.0, 20 * 86400, now)] } else { vec![
            meter(MeterKind::FiveHour, 34.0, 2 * 3600, now), meter(MeterKind::Weekly, 18.0, 3 * 86400, now),
            meter(MeterKind::Monthly, 28.0, 20 * 86400, now)] } };
        vec![
            ProviderRow {
                id: ProviderId::Claude, result: Ok(claude), activity: AgentState::Working, running: true, detail: None,
                chats: vec![
                    chat("a", Some("meu-app"), "Corrigir o login", AgentState::Working, "faça o login aceitar e-mail e senha", Some(("Edit", "auth.rs")), now),
                    chat("b", Some("meu-app"), "Refatorar o parser", AgentState::Idle, "separe o parser em módulos", Some(("Bash", "cargo test")), now),
                    chat("c", None, "Dúvida sobre lifetimes", AgentState::Idle, "quando usar 'static?", None, now),
                ],
            },
            ProviderRow { id: ProviderId::Codex, result: Ok(codex), activity: AgentState::Idle, running: true, detail: None, chats: vec![] },
            // OpenCode com login do ChatGPT: consome os mesmos limites de 5 h e semanal.
            ProviderRow { id: ProviderId::OpenCode, activity: AgentState::Idle, running: true, detail: None, chats: vec![],
                result: Ok(Usage { provider: ProviderId::OpenCode, meters: vec![
                    meter(MeterKind::FiveHour, 22.0, 3 * 3600, now), meter(MeterKind::Weekly, 9.0, 5 * 86400, now)].into_iter().filter(|_| !free).collect() }) },
            // Cursor: só atividade (sem limites); aparece quando ligado na config.
            ProviderRow { id: ProviderId::Cursor, result: Ok(Usage { provider: ProviderId::Cursor, meters: vec![] }),
                activity: AgentState::Working, running: true, detail: None, chats: vec![] },
        ]
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli(&args) { return; }
    let demo = args.iter().any(|a| a == "--demo");
    if demo {
        // Config própria na pasta temporária (nunca mexe na do usuário); `--demo` só roda com dados fictícios.
        let path = std::env::temp_dir().join("LineQuotaDemo").join("config.toml");
        let cfg = std::fs::read_to_string(&path).ok().and_then(|s| Config::from_toml(&s).ok()).unwrap_or_default();
        let shared = Shared::new(cfg, path, std::sync::Arc::new(ClaudeAccount));
        *shared.rows.lock().unwrap() = demo::rows(now_secs(), args.iter().any(|a| a == "--free"));
        let (tx, _rx) = mpsc::channel::<()>();
        let _ = win::run(shared, tx);
        return;
    }
    if !win::single_instance() { return; }

    let cfg = load_config();
    let idle = Duration::from_secs(cfg.codex.refresh_idle_secs.max(30));
    let shared = Shared::new(cfg, config_path(), std::sync::Arc::new(ClaudeAccount));
    let (tx, rx) = mpsc::channel::<()>();
    type Slot = Option<Result<lq_core::model::Usage, String>>;
    let usage: Arc<Mutex<Vec<Slot>>> = Arc::new(Mutex::new(vec![None; 4]));

    // Thread de rede: busca os limites no intervalo configurado ou sob pedido. Chamadas lentas
    // (até 10–15 s) ficam aqui e nunca seguram a atividade, os alertas nem a UI.
    {
        let (s, usage) = (shared.clone(), usage.clone());
        std::thread::spawn(move || {
            let provs = providers();
            loop {
                let cfg = s.config.lock().unwrap().clone();
                for (i, p) in provs.iter().enumerate().filter(|(_, p)| !p.local_only() && cfg.provider(p.id()).enabled) {
                    let r = p.fetch();
                    usage.lock().unwrap()[i] = Some(r);
                }
                let fetched = Instant::now();
                // Espera o intervalo ou um pedido de atualização; pedidos em sequência viram uma só busca (mín. 5 s).
                if rx.recv_timeout(idle).is_ok() {
                    while rx.try_recv().is_ok() {}
                    std::thread::sleep(Duration::from_secs(5).saturating_sub(fetched.elapsed()));
                    while rx.try_recv().is_ok() {}
                }
            }
        });
    }

    // Thread local (tick de 2 s): só leitura de arquivos/processos; provedores desligados não são consultados.
    let s = shared.clone();
    std::thread::spawn(move || {
        let provs = providers();
        let mut notifier = Notifier::default();
        loop {
            let cfg = s.config.lock().unwrap().clone();
            for (i, p) in provs.iter().enumerate().filter(|(_, p)| p.local_only() && cfg.provider(p.id()).enabled) {
                let r = p.fetch();
                usage.lock().unwrap()[i] = Some(r);
            }
            // Programas abertos (modo automático): processo em execução ou atividade registrada.
            let open = lq_activity::procs::running_exes();
            let snapshot: Vec<Slot> = usage.lock().unwrap().clone();
            let rows: Vec<ProviderRow> = provs.iter().zip(&snapshot).map(|(p, u)| {
                let result = u.clone().unwrap_or_else(|| Err("carregando".into()));
                if !cfg.provider(p.id()).enabled {
                    return ProviderRow { id: p.id(), result, activity: lq_core::model::AgentState::Idle, running: false, detail: None, chats: Vec::new() };
                }
                let activity = p.activity();
                let running = activity != lq_core::model::AgentState::Idle
                    || lq_activity::procs::any_running(&open, &cfg.provider(p.id()).processes);
                ProviderRow { id: p.id(), result, activity, running, detail: p.detail(), chats: p.chats() }
            }).collect();
            // Alertas: limiares, renovação, tarefa concluída e pedido de resposta.
            let snaps: Vec<Snap> = rows.iter().filter(|r| cfg.provider(r.id).enabled)
                .map(|r| Snap { id: r.id, usage: r.result.as_ref().ok().cloned(), activity: r.activity }).collect();
            for n in notifier.update(&snaps, &cfg.alerts, now_secs()) { s.toast(n); }
            let changed = *s.rows.lock().unwrap() != rows;
            if changed { *s.rows.lock().unwrap() = rows; s.notify(); }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    let _ = win::run(shared, tx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_enables_only_what_is_installed() {
        let mut c = Config::default();
        apply_detected(&mut c, [false, true, false, false]);
        assert_eq!((c.claude.enabled, c.codex.enabled, c.opencode.enabled, c.cursor.enabled), (false, true, false, false));
        let mut c = Config::default();
        apply_detected(&mut c, [false, false, false, false]); // nada encontrado: não deixa o widget vazio
        assert!(c.claude.enabled && c.codex.enabled && c.opencode.enabled && !c.cursor.enabled); // Cursor só liga se for encontrado
    }
}
