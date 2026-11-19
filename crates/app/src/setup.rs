//! Instalador embutido: `line-quota --install` / `--uninstall`. Instalação por usuário
//! (sem administrador): copia o .exe, configura os hooks do Claude, a inicialização com o
//! Windows e a entrada em "Aplicativos instalados".
use crate::install;
use lq_ui::win;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXE_NAME: &str = "line-quota.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED: u32 = 0x0000_0008;

/// Pasta de instalação (por usuário).
pub fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("LineQuota")
}

/// Encerra outras instâncias do app (menos esta), para poder trocar/remover o arquivo.
fn stop_others() {
    let me = std::process::id().to_string();
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", EXE_NAME, "/FI", &format!("PID ne {me}")])
        .creation_flags(CREATE_NO_WINDOW).output();
}

fn copy_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = Ok(());
    for _ in 0..10 {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) => { last = Err(e); std::thread::sleep(std::time::Duration::from_millis(300)); }
        }
    }
    last
}

/// Mensagem final: caixa de diálogo, a não ser em modo silencioso.
fn report(quiet: bool, title: &str, text: &str) {
    if quiet { eprintln!("{text}"); } else { win::info_box(title, text); }
}

pub fn install_app(quiet: bool, autostart: bool) {
    let dir = install_dir();
    let target = dir.join(EXE_NAME);
    let result = (|| -> Result<Vec<String>, String> {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("não consegui criar {}: {e}", dir.display()))?;
        stop_others();
        let same = std::fs::canonicalize(&me).ok() == std::fs::canonicalize(&target).ok();
        if !same { copy_with_retry(&me, &target).map_err(|e| format!("não consegui copiar o programa: {e}"))?; }
        let mut log = vec![format!("programa em {}", target.display())];
        log.extend(install::install_for(&target).map_err(|e| format!("hooks do Claude: {e}"))?);
        win::unregister_toast_identity(); // o app recria o atalho do Menu Iniciar apontando para o novo caminho
        win::set_autostart_for(&target, autostart);
        win::register_uninstall(&dir, &target, env!("CARGO_PKG_VERSION"));
        Ok(log)
    })();
    match result {
        Ok(_) => {
            let _ = Command::new(&target).current_dir(&dir).creation_flags(DETACHED | CREATE_NO_WINDOW).spawn();
            report(quiet, "Line Quota",
                &format!("Instalado!\n\nO widget já está na barra de tarefas.\n\nPasta: {}\nInício com o Windows: {}\n\nPara remover: Configurações do Windows > Aplicativos instalados > Line Quota.",
                    dir.display(), if autostart { "ligado" } else { "desligado" }));
        }
        Err(e) => report(quiet, "Line Quota — erro na instalação", &e),
    }
}

/// Remove tudo o que o instalador criou. `purge` apaga também as configurações.
pub fn uninstall_app(quiet: bool, purge: bool) {
    let dir = install_dir();
    stop_others();
    let _ = install::uninstall();
    win::unregister_toast_identity();
    win::set_autostart_for(&dir.join(EXE_NAME), false);
    win::unregister_uninstall();
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("LineQuota");
    let _ = std::fs::remove_dir_all(local.join("activity"));
    let cfg = PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("LineQuota");
    if purge {
        let _ = std::fs::remove_dir_all(&local);
        let _ = std::fs::remove_dir_all(&cfg);
    }
    // O próprio .exe está em uso: um cmd separado apaga a pasta logo depois que este processo sair.
    let d = dir.to_string_lossy().to_string();
    let _ = Command::new("cmd")
        .raw_arg(format!("/C ping -n 3 127.0.0.1 >nul & rmdir /S /Q \"{d}\""))
        .creation_flags(DETACHED | CREATE_NO_WINDOW).spawn();
    report(quiet, "Line Quota",
        &format!("Line Quota removido.{}", if purge { String::new() } else { format!("\n\nSuas configurações foram mantidas em {}.", cfg.display()) }));
}

/// Interpreta `--install [--quiet] [--no-autostart]` / `--uninstall [--quiet] [--purge]`.
pub fn run(args: &[String]) -> bool {
    let has = |f: &str| args.iter().any(|a| a == f);
    match args.first().map(String::as_str) {
        Some("--install") => { install_app(has("--quiet"), !has("--no-autostart")); true }
        Some("--uninstall") => { uninstall_app(has("--quiet"), has("--purge")); true }
        _ => false,
    }
}
