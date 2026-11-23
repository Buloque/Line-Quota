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
fn detect_installed() -> [bool; 3] {
    let home = PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default());
    let opencode = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local").join("share")).join("opencode");
    [home.join(".claude").is_dir(), home.join(".codex").is_dir(), opencode.is_dir()]
}

/// Primeira execução: liga só os agentes encontrados (nenhum é obrigatório). Se não achar nenhum, mantém todos.
fn apply_detected(c: &mut Config, found: [bool; 3]) {
    if found.iter().any(|f| *f) {
        c.claude.enabled = found[0];
        c.codex.enabled = found[1];
        c.opencode.enabled = found[2];
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
}

/// Modos de linha de comando usados pelo Claude Code (hooks/statusLine) e pela instalação.
/// Retorna true se tratou um modo (e o app não deve abrir a UI).
fn cli(args: &[String]) -> bool {
    #[cfg(windows)]
    if setup::run(args) { return true; }
    let read_stdin = || { let mut s = String::new(); let _ = std::io::stdin().read_to_string(&mut s); s };
    match args.first().map(String::as_str) {
        Some("--hook") => { let _ = lq_activity::handle_claude_hook(&lq_activity::data_dir(), &read_stdin(), now_secs()); true }
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli(&args) { return; }
    if !win::single_instance() { return; }

    let cfg = load_config();
    let idle = Duration::from_secs(cfg.codex.refresh_idle_secs.max(30));
    let shared = Shared::new(cfg, config_path(), std::sync::Arc::new(ClaudeAccount));
    let (tx, rx) = mpsc::channel::<()>();
    type Slot = Option<Result<lq_core::model::Usage, String>>;
    let usage: Arc<Mutex<Vec<Slot>>> = Arc::new(Mutex::new(vec![None; 3]));

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
        apply_detected(&mut c, [false, true, false]);
        assert_eq!((c.claude.enabled, c.codex.enabled, c.opencode.enabled), (false, true, false));
        let mut c = Config::default();
        apply_detected(&mut c, [false, false, false]); // nada encontrado: não deixa o widget vazio
        assert!(c.claude.enabled && c.codex.enabled && c.opencode.enabled);
    }
}
