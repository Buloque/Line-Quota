//! Detecta quais programas estão abertos (nomes de executáveis em execução).
use std::collections::HashSet;

/// Nomes (minúsculos) dos executáveis em execução. Barato (~1 ms): um snapshot da lista de processos.
#[cfg(windows)]
pub fn running_exes() -> HashSet<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut set = HashSet::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return set };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            set.insert(String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase());
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    set
}

#[cfg(not(windows))]
pub fn running_exes() -> HashSet<String> { HashSet::new() }

/// true se algum dos `wanted` (comparação sem diferenciar maiúsculas) está em execução.
pub fn any_running(running: &HashSet<String>, wanted: &[String]) -> bool {
    wanted.iter().any(|w| running.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_is_case_insensitive() {
        let r: HashSet<String> = ["claude.exe".to_string()].into();
        assert!(any_running(&r, &["Claude.EXE".to_string()]));
        assert!(!any_running(&r, &["codex.exe".to_string()]));
        assert!(!any_running(&r, &[]));
    }

    #[test]
    #[cfg(windows)]
    fn sees_own_process() {
        let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(running_exes().contains(&me));
    }
}
