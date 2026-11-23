//! Instalador embutido: `line-quota --install` / `--uninstall`. Instalação por usuário
//! (sem administrador): copia o .exe, configura os hooks do Claude, a inicialização com o
//! Windows e a entrada em "Aplicativos instalados".
use crate::install;
use lq_ui::win;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const EXE_NAME: &str = "line-quota.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED: u32 = 0x0000_0008;


/// Variável de ambiente que aponta para uma pasta absoluta (vazia/relativa = não confiável: nada é apagado com ela).
fn env_dir(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var(name).ok()?);
    (p.is_absolute() && p.parent().is_some()).then_some(p)
}

/// Programas do sistema por caminho absoluto (não pela busca no PATH/pasta atual, que inclui a pasta do app).
fn system_exe(name: &str) -> PathBuf {
    env_dir("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32").join(name)
}
/// Pasta de instalação (por usuário).
pub fn install_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").unwrap_or_else(|| PathBuf::from(r"C:\LineQuota-sem-LOCALAPPDATA")).join("Programs").join("LineQuota")
}

/// Encerra outras instâncias do app (menos esta), para poder trocar/remover o arquivo.
fn stop_others() {
    let me = std::process::id().to_string();
    let _ = Command::new(system_exe("taskkill.exe"))
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

/// Instala (ou atualiza) o app por usuário. Claude Code, Codex e OpenCode são opcionais:
/// problemas na integração com o Claude viram aviso, nunca abortam a instalação.
pub fn install_app(quiet: bool, autostart: bool, launch: bool) {
    let dir = install_dir();
    let target = dir.join(EXE_NAME);
    let result = (|| -> Result<Vec<String>, String> {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("não consegui criar {}: {e}", dir.display()))?;
        stop_others();
        let same = std::fs::canonicalize(&me).ok() == std::fs::canonicalize(&target).ok();
        if !same { copy_with_retry(&me, &target).map_err(|e| format!("não consegui copiar o programa: {e}"))?; }
        let mut log = vec![format!("programa em {}", target.display())];
        match install::install_for(&target) {
            Ok(l) => log.extend(l),
            Err(e) => log.push(format!("Aviso: não consegui configurar o Claude Code ({e}). O widget funciona sem isso.")),
        }
        match install::install_cursor_for(&target) {
            Ok(l) => log.extend(l),
            Err(e) => log.push(format!("Aviso: não consegui configurar o Cursor ({e}). O widget funciona sem isso.")),
        }
        match install::install_antigravity_for(&target) {
            Ok(l) => log.extend(l),
            Err(e) => log.push(format!("Aviso: não consegui configurar o Antigravity ({e}). O widget funciona sem isso.")),
        }
        win::unregister_toast_identity(); // o app recria o atalho do Menu Iniciar apontando para o novo caminho
        win::set_autostart_for(&target, autostart);
        win::register_uninstall(&dir, &target, env!("CARGO_PKG_VERSION"));
        Ok(log)
    })();
    match result {
        Ok(log) => {
            // Sem herdar stdin/stdout/stderr: quem chamou (o instalador) espera o EOF desses pipes.
            if launch { let _ = Command::new(&target).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(DETACHED | CREATE_NO_WINDOW).spawn(); }
            let notes: String = log.iter().filter(|l| l.starts_with("Aviso") || l.starts_with("Claude Code não")).map(|l| format!("\n\n{l}")).collect();
            report(quiet, "Line Quota",
                &format!("Instalado!\n\nO widget já está na barra de tarefas.\n\nPasta: {}\nInício com o Windows: {}{notes}\n\nPara remover: Configurações do Windows > Aplicativos instalados > Line Quota.",
                    dir.display(), if autostart { "ligado" } else { "desligado" }));
        }
        Err(e) => report(quiet, "Line Quota — erro na instalação", &e),
    }
}

/// Assistente de instalação (duplo clique no `LineQuota-Setup.exe`): confirma, pergunta do início
/// com o Windows e instala. Cancelar não altera nada.
pub fn install_wizard() {
    let dir = install_dir();
    let update = dir.join(EXE_NAME).is_file();
    let text = format!(
        "O Line Quota mostra na barra de tarefas os limites do Claude Code, Codex e OpenCode (e a atividade do Cursor e do Antigravity).\n\n\
         Nenhum deles é obrigatório e não é preciso ser administrador.\n\nPasta: {}",
        dir.display());
    let heading = if update { "Atualizar o Line Quota" } else { "Instalar o Line Quota" };
    if let Some(autostart) = win::confirm_with_check("Line Quota — Instalação", heading, &text,
        if update { "Atualizar" } else { "Instalar" }, "Iniciar com o Windows", true) {
        install_app(false, autostart, true);
    }
}

/// Remove tudo o que o instalador criou. `purge` apaga também as configurações.
pub fn uninstall_app(quiet: bool, purge: bool) {
    // Sem pastas de usuário válidas não há o que remover com segurança (variável vazia/relativa apagaria o lugar errado).
    let (Some(local_base), Some(roaming)) = (env_dir("LOCALAPPDATA"), env_dir("APPDATA")) else {
        report(quiet, "Line Quota — erro na remoção", "Não encontrei as pastas do usuário (LOCALAPPDATA/APPDATA); nada foi removido.");
        return;
    };
    let dir = install_dir();
    stop_others();
    let _ = install::uninstall();
    let _ = install::uninstall_cursor();
    let _ = install::uninstall_antigravity();
    lq_providers::cursor_auth::clear(); // segredo: nunca fica para trás, nem sem o purge
    win::unregister_toast_identity();
    win::set_autostart_for(&dir.join(EXE_NAME), false);
    win::unregister_uninstall();
    let local = local_base.join("LineQuota");
    let _ = std::fs::remove_dir_all(local.join("activity"));
    let cfg = roaming.join("LineQuota");
    if purge {
        let _ = std::fs::remove_dir_all(&local);
        let _ = std::fs::remove_dir_all(&cfg);
    }
    // O próprio .exe está em uso: um cmd separado apaga a pasta logo depois que este processo sair.
    // O cmd expande % ^ & e aspas: caminhos com esses caracteres não passam pelo cmd (a pasta fica; sem risco).
    let d = dir.to_string_lossy().to_string();
    if !d.contains(['%', '^', '&', '"', '!', '|', '<', '>']) {
        let _ = Command::new(system_exe("cmd.exe"))
            .raw_arg(format!("/C ping -n 3 127.0.0.1 >nul & rmdir /S /Q \"{d}\""))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(DETACHED | CREATE_NO_WINDOW).spawn();
    }
    report(quiet, "Line Quota",
        &format!("Line Quota removido.{}", if purge { String::new() } else { format!("\n\nSuas configurações foram mantidas em {}.", cfg.display()) }));
}

/// Desinstalador interativo (Aplicativos instalados do Windows): confirma e oferece apagar as configurações.
pub fn uninstall_wizard(purge_default: bool) {
    let cfg = PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("LineQuota");
    let text = format!(
        "Remove o programa, o início com o Windows e a integração com o Claude Code.\n\nSuas configurações ficam em {}.",
        cfg.display());
    if let Some(purge) = win::confirm_with_check("Line Quota — Desinstalação", "Remover o Line Quota?", &text,
        "Remover", "Apagar também minhas configurações e dados (purge)", purge_default) {
        uninstall_app(false, purge);
    }
}

/// O .exe foi baixado como instalador (`LineQuota-Setup-*.exe`)? Então abre o assistente em vez do widget.
pub fn is_setup_exe() -> bool {
    let me = std::env::current_exe().unwrap_or_default();
    let same = std::fs::canonicalize(&me).ok() == std::fs::canonicalize(install_dir().join(EXE_NAME)).ok();
    !same && me.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().contains("setup"))
}

/// Interpreta `--install [--quiet] [--no-autostart]` / `--uninstall [--quiet] [--purge]`.
/// Sem `--quiet` o assistente pergunta o necessário; com `--quiet` age direto (scripts).
pub fn run(args: &[String]) -> bool {
    let has = |f: &str| args.iter().any(|a| a == f);
    match args.first().map(String::as_str) {
        Some("--install") if has("--quiet") => { install_app(true, !has("--no-autostart"), !has("--no-launch")); true }
        Some("--install") => { if has("--no-autostart") { install_app(false, false, true) } else { install_wizard() } true }
        Some("--uninstall") if has("--quiet") => { uninstall_app(true, has("--purge")); true }
        Some("--uninstall") => { uninstall_wizard(has("--purge")); true }
        None if is_setup_exe() => { install_wizard(); true }
        _ => false,
    }
}
