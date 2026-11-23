//! Instalador do Line Quota: janela própria (Win32), instala por usuário (sem administrador) e atualiza
//! uma instalação existente. O `line-quota.exe` vem embutido; a instalação em si (cópia, hooks do Claude,
//! início com o Windows, entrada em "Aplicativos instalados") é feita por `line-quota.exe --install --quiet`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Versão do `line-quota.exe` embutido (a mesma deste instalador).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// "0.10.2" -> (0, 10, 2); partes ausentes ou inválidas valem 0.
fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

#[derive(Debug, PartialEq, Eq)]
enum Plan { Fresh, Update { from: String }, Reinstall, Downgrade { from: String } }

fn plan(installed: Option<&str>, new: &str) -> Plan {
    match installed {
        None => Plan::Fresh,
        Some(old) if parse_version(old) < parse_version(new) => Plan::Update { from: old.to_string() },
        Some(old) if parse_version(old) > parse_version(new) => Plan::Downgrade { from: old.to_string() },
        Some(_) => Plan::Reinstall,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("v1.2"), (1, 2, 0));
        assert_eq!(parse_version("lixo"), (0, 0, 0));
    }

    #[test]
    fn plan_from_installed_version() {
        assert_eq!(plan(None, "0.2.0"), Plan::Fresh);
        assert_eq!(plan(Some("0.1.0"), "0.2.0"), Plan::Update { from: "0.1.0".into() });
        assert_eq!(plan(Some("0.2.0"), "0.2.0"), Plan::Reinstall);
        assert_eq!(plan(Some("0.3.0"), "0.2.0"), Plan::Downgrade { from: "0.3.0".into() });
    }
}

#[cfg(windows)]
mod gui;

#[cfg(windows)]
fn main() { gui::run(); }

#[cfg(not(windows))]
fn main() { eprintln!("O instalador do Line Quota é só para Windows."); }
