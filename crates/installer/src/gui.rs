//! Janela do instalador (controles nativos do Windows, tema claro).
use super::{plan, Plan, VERSION};
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows::Win32::UI::Controls::{InitCommonControlsEx, SetWindowTheme, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

/// O `line-quota.exe` do app, embutido na compilação (vazio = instalador gerado sem o pacote).
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\LineQuota";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

const ID_INSTALL: usize = 1;
const ID_CANCEL: usize = 2;
const ID_UNINSTALL: usize = 3;
const ID_AUTOSTART: usize = 10;
const ID_LAUNCH: usize = 11;
const ID_PURGE: usize = 12;
const ID_PROGRESS: usize = 20;
const ID_STATUS: usize = 21;
const ID_NOTE: usize = 22;
const WM_STEP: u32 = WM_APP + 1; // wParam = % ; lParam = 0 em andamento, 1 = ok, 2 = erro
const PBM_SETPOS: u32 = 0x0402;
const PBM_SETRANGE32: u32 = 0x0406;
const BS_AUTOCHECKBOX: u32 = 3;
const BS_DEFPUSHBUTTON: u32 = 1;
const WS_TABSTOP_: u32 = 0x10000;

static MAIN: AtomicIsize = AtomicIsize::new(0);
static FONT: AtomicIsize = AtomicIsize::new(0);
static FONT_BIG: AtomicIsize = AtomicIsize::new(0);
static FONT_TITLE: AtomicIsize = AtomicIsize::new(0);
/// Pincel do fundo (branco no tema claro, cinza-escuro no escuro).
static WHITE: AtomicIsize = AtomicIsize::new(0);
static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// true depois de clicar em "Desinstalar": o botão principal passa a remover o programa.
static UNINSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MESSAGE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn wide0(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn scale(v: i32) -> i32 { v * unsafe { GetDpiForSystem() } as i32 / 96 }

/// true se os apps do Windows usam o tema escuro.
fn apps_dark() -> bool {
    let mut v: u32 = 1;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut c_void), Some(&mut size))
    };
    r.0 == 0 && v == 0
}

unsafe extern "system" fn theme_child(h: HWND, lp: LPARAM) -> BOOL {
    let dark = lp.0 != 0;
    let _ = SetWindowTheme(h, if dark { w!("DarkMode_Explorer") } else { w!("Explorer") }, PCWSTR::null());
    TRUE
}

/// Barra de título e controles no tema do Windows (claro/escuro).
unsafe fn apply_theme(h: HWND) {
    let dark = DARK.load(Ordering::Relaxed);
    let flag = BOOL::from(dark);
    let _ = DwmSetWindowAttribute(h, DWMWINDOWATTRIBUTE(20), &flag as *const BOOL as *const c_void, 4);
    let _ = EnumChildWindows(h, Some(theme_child), LPARAM(dark as isize));
}

fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("LineQuota")
}

fn reg_string(subkey: &str, name: &str) -> Option<String> {
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(wide0(subkey).as_ptr()), PCWSTR(wide0(name).as_ptr()), RRF_RT_REG_SZ,
            None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut size))
    };
    if r.0 != 0 { return None; }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Versão instalada (só vale se o programa ainda estiver na pasta).
fn installed_version() -> Option<String> {
    let v = reg_string(UNINSTALL_KEY, "DisplayVersion")?;
    install_dir().join("line-quota.exe").is_file().then_some(v)
}

unsafe fn mk(parent: HWND, class: PCWSTR, text: &str, style: u32, r: (i32, i32, i32, i32), id: usize, font: &AtomicIsize) -> HWND {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let h = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR(wide0(text).as_ptr()), WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
        scale(r.0), scale(r.1), scale(r.2), scale(r.3), parent, HMENU(id as *mut c_void), hinst, None).unwrap_or_default();
    let f = font.load(Ordering::Relaxed);
    if f != 0 { SendMessageW(h, WM_SETFONT, WPARAM(f as usize), LPARAM(1)); }
    h
}

unsafe fn font(px: i32, weight: i32) -> isize {
    CreateFontW(-scale(px), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI")).0 as isize
}

fn set_text(h: HWND, id: usize, text: &str) {
    unsafe { if let Ok(c) = GetDlgItem(h, id as i32) { let _ = SetWindowTextW(c, PCWSTR(wide0(text).as_ptr())); } }
}

fn checked(h: HWND, id: usize) -> bool {
    unsafe { GetDlgItem(h, id as i32).map_or(false, |c| SendMessageW(c, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1) }
}

fn show(h: HWND, id: usize, on: bool) {
    unsafe { if let Ok(c) = GetDlgItem(h, id as i32) { let _ = ShowWindow(c, if on { SW_SHOW } else { SW_HIDE }); } }
}

fn enable(h: HWND, id: usize, on: bool) {
    unsafe { if let Ok(c) = GetDlgItem(h, id as i32) { let _ = EnableWindow(c, on); } }
}

fn headline(p: &Plan) -> (String, &'static str) {
    match p {
        Plan::Fresh => (format!("Instalar a versão {VERSION}"), "Instalar"),
        Plan::Update { from } => (format!("Atualizar da versão {from} para {VERSION}"), "Atualizar"),
        Plan::Reinstall => (format!("A versão {VERSION} já está instalada. Reinstalar?"), "Reinstalar"),
        Plan::Downgrade { from } => (format!("Há uma versão mais nova ({from}). Voltar para a {VERSION}?"), "Instalar"),
    }
}

pub fn run() {
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_PROGRESS_CLASS });
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let dark = apps_dark();
        DARK.store(dark, Ordering::Relaxed);
        WHITE.store(CreateSolidBrush(COLORREF(if dark { 0x0020_2020 } else { 0x00FF_FFFF })).0 as isize, Ordering::Relaxed);
        FONT.store(font(13, 400), Ordering::Relaxed);
        FONT_BIG.store(font(15, 600), Ordering::Relaxed);
        FONT_TITLE.store(font(26, 700), Ordering::Relaxed);
        let class = w!("LineQuotaSetup");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(hinst, PCWSTR(1 as *const u16)).unwrap_or_default(),
            hbrBackground: HBRUSH(WHITE.load(Ordering::Relaxed) as *mut c_void),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let (cw, ch) = (scale(500), scale(360));
        let mut rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let _ = AdjustWindowRectEx(&mut rc, style, false, WINDOW_EX_STYLE(0));
        let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Instalador do Line Quota"), style,
            ((sw - ww) / 2).max(0), ((sh - wh) / 2).max(0), ww, wh, None, None, hinst, None) else { return };
        MAIN.store(h.0 as isize, Ordering::Relaxed);
        build(h);
        apply_theme(h);
        let _ = ShowWindow(h, SW_SHOW);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(h, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

unsafe fn build(h: HWND) {
    let installed = installed_version();
    let p = plan(installed.as_deref(), VERSION);
    let (line, button) = headline(&p);
    let autostart_default = installed.is_none() || reg_string(RUN_KEY, "LineQuota").is_some();

    mk(h, w!("STATIC"), "Line Quota", 0, (28, 22, 440, 40), 0, &FONT_TITLE);
    mk(h, w!("STATIC"), "Claude Code, Codex, OpenCode, Cursor e Antigravity na barra de tarefas", 0, (30, 66, 440, 20), 0, &FONT);
    mk(h, w!("STATIC"), &line, 0, (30, 110, 440, 24), ID_STATUS, &FONT_BIG);
    mk(h, w!("STATIC"), &format!("Pasta: {}", install_dir().display()), 0, (30, 140, 440, 20), 0, &FONT);
    mk(h, w!("STATIC"), "Não precisa de administrador. Nenhum deles é obrigatório.", 0, (30, 162, 440, 20), ID_NOTE, &FONT);
    let a = mk(h, w!("BUTTON"), "Iniciar com o Windows", BS_AUTOCHECKBOX | WS_TABSTOP_, (30, 196, 300, 24), ID_AUTOSTART, &FONT);
    let l = mk(h, w!("BUTTON"), "Abrir o Line Quota ao concluir", BS_AUTOCHECKBOX | WS_TABSTOP_, (30, 222, 300, 24), ID_LAUNCH, &FONT);
    SendMessageW(a, BM_SETCHECK, WPARAM(autostart_default as usize), LPARAM(0));
    SendMessageW(l, BM_SETCHECK, WPARAM(1), LPARAM(0));
    // Modo "desinstalar" (só aparece depois de clicar em Desinstalar): opção de apagar também as configurações.
    mk(h, w!("BUTTON"), "Apagar também minhas configurações e dados", BS_AUTOCHECKBOX | WS_TABSTOP_, (30, 196, 440, 24), ID_PURGE, &FONT);
    show(h, ID_PURGE, false);
    let bar = mk(h, w!("msctls_progress32"), "", 0, (30, 262, 440, 16), ID_PROGRESS, &FONT);
    SendMessageW(bar, PBM_SETRANGE32, WPARAM(0), LPARAM(100));
    show(h, ID_PROGRESS, false);
    mk(h, w!("BUTTON"), button, BS_DEFPUSHBUTTON | WS_TABSTOP_, (270, 306, 100, 30), ID_INSTALL, &FONT);
    mk(h, w!("BUTTON"), "Cancelar", WS_TABSTOP_, (380, 306, 90, 30), ID_CANCEL, &FONT);
    // Já instalado: além de atualizar/reinstalar, dá para desinstalar.
    if installed.is_some() { mk(h, w!("BUTTON"), "Desinstalar", WS_TABSTOP_, (30, 306, 110, 30), ID_UNINSTALL, &FONT); }
    if PAYLOAD.is_empty() {
        set_text(h, ID_STATUS, "Instalador gerado sem o programa. Rode scripts\\package.ps1.");
        enable(h, ID_INSTALL, false);
    }
}

/// Trabalho de instalação (outra thread): grava o app numa pasta temporária e roda a instalação silenciosa dele.
fn install_job(hwnd: isize, autostart: bool, launch: bool) {
    let post = |pct: usize, state: isize| unsafe { let _ = PostMessageW(HWND(hwnd as *mut c_void), WM_STEP, WPARAM(pct), LPARAM(state)); };
    let result = (|| -> Result<(), String> {
        let tmp = std::env::temp_dir().join(format!("LineQuotaSetup-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).map_err(|e| format!("não consegui criar a pasta temporária: {e}"))?;
        let exe = tmp.join("line-quota.exe");
        post(15, 0);
        std::fs::write(&exe, PAYLOAD).map_err(|e| format!("não consegui extrair o programa: {e}"))?;
        post(45, 0);
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(["--install", "--quiet"]).creation_flags(CREATE_NO_WINDOW);
        if !autostart { cmd.arg("--no-autostart"); }
        cmd.arg("--no-launch").stdin(std::process::Stdio::null()); // quem abre o app é este instalador (sem herdar pipes)
        let out = cmd.output().map_err(|e| format!("não consegui executar a instalação: {e}"));
        let _ = std::fs::remove_dir_all(&tmp);
        let out = out?;
        post(90, 0);
        let installed = install_dir().join("line-quota.exe");
        if !out.status.success() || !installed.is_file() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() { "a instalação não terminou corretamente".into() } else { err });
        }
        if launch {
            let _ = std::process::Command::new(&installed).current_dir(install_dir())
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .creation_flags(CREATE_NO_WINDOW | 0x0000_0008).spawn();
        }
        Ok(())
    })();
    match result {
        Ok(()) => post(100, 1),
        Err(e) => { *MESSAGE.lock().unwrap() = e; post(100, 2) }
    }
}

/// Remoção (outra thread): roda `line-quota.exe --uninstall --quiet` da pasta instalada.
fn uninstall_job(hwnd: isize, purge: bool) {
    let post = |pct: usize, state: isize| unsafe { let _ = PostMessageW(HWND(hwnd as *mut c_void), WM_STEP, WPARAM(pct), LPARAM(state)); };
    let result = (|| -> Result<(), String> {
        let exe = install_dir().join("line-quota.exe");
        if !exe.is_file() { return Err("o programa não está mais na pasta de instalação".into()); }
        post(30, 0);
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(["--uninstall", "--quiet"]).creation_flags(CREATE_NO_WINDOW).stdin(std::process::Stdio::null());
        if purge { cmd.arg("--purge"); }
        let out = cmd.output().map_err(|e| format!("não consegui executar a remoção: {e}"))?;
        post(90, 0);
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() { "a remoção não terminou corretamente".into() } else { err });
        }
        Ok(())
    })();
    match result {
        Ok(()) => post(100, 1),
        Err(e) => { *MESSAGE.lock().unwrap() = e; post(100, 2) }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let (id, code) = (wp.0 & 0xFFFF, (wp.0 >> 16) & 0xFFFF);
            // Só cliques reais de botão (lp = controle): ignora IDOK/IDCANCEL sintéticos do gerenciador de diálogo.
            if code != 0 || lp.0 == 0 { return LRESULT(0); }
            match id {
                ID_CANCEL => { let _ = DestroyWindow(hwnd); }
                ID_UNINSTALL => {
                    // Troca a tela para a remoção: o botão principal passa a "Remover".
                    UNINSTALLING.store(true, Ordering::Relaxed);
                    set_text(hwnd, ID_STATUS, "Remover o Line Quota?");
                    set_text(hwnd, ID_NOTE, "Suas configurações ficam guardadas, a menos que marque abaixo.");
                    set_text(hwnd, ID_INSTALL, "Remover");
                    show(hwnd, ID_UNINSTALL, false);
                    for i in [ID_AUTOSTART, ID_LAUNCH] { show(hwnd, i, false); }
                    show(hwnd, ID_PURGE, true);
                }
                ID_INSTALL if UNINSTALLING.load(Ordering::Relaxed) && !DONE.load(Ordering::Relaxed) => {
                    let purge = checked(hwnd, ID_PURGE);
                    for i in [ID_INSTALL, ID_CANCEL, ID_PURGE] { enable(hwnd, i, false); }
                    show(hwnd, ID_PROGRESS, true);
                    set_text(hwnd, ID_STATUS, "Removendo…");
                    let h = hwnd.0 as isize;
                    std::thread::spawn(move || uninstall_job(h, purge));
                }
                ID_INSTALL => {
                    // Depois de concluir, o mesmo botão vira "Concluir".
                    if DONE.load(Ordering::Relaxed) { let _ = DestroyWindow(hwnd); return LRESULT(0); }
                    let (autostart, launch) = (checked(hwnd, ID_AUTOSTART), checked(hwnd, ID_LAUNCH));
                    for i in [ID_INSTALL, ID_CANCEL, ID_AUTOSTART, ID_LAUNCH] { enable(hwnd, i, false); }
                    show(hwnd, ID_PROGRESS, true);
                    set_text(hwnd, ID_STATUS, "Instalando…");
                    let h = hwnd.0 as isize;
                    std::thread::spawn(move || install_job(h, autostart, launch));
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_STEP => {
            if let Ok(bar) = GetDlgItem(hwnd, ID_PROGRESS as i32) { SendMessageW(bar, PBM_SETPOS, wp, LPARAM(0)); }
            match lp.0 {
                1 => {
                    let removed = UNINSTALLING.load(Ordering::Relaxed);
                    set_text(hwnd, ID_STATUS, if removed { "Pronto! O Line Quota foi removido." } else { "Pronto! O Line Quota foi instalado." });
                    finish(hwnd, "Concluir");
                }
                2 => {
                    let m = MESSAGE.lock().unwrap().clone();
                    set_text(hwnd, ID_STATUS, &format!("Não foi possível {}: {m}", if UNINSTALLING.load(Ordering::Relaxed) { "remover" } else { "instalar" }));
                    finish(hwnd, "Fechar");
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = HDC(wp.0 as *mut c_void);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(if DARK.load(Ordering::Relaxed) { 0x00F0_F0F0 } else { 0x0010_1010 }));
            LRESULT(WHITE.load(Ordering::Relaxed))
        }
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Troca os botões para o estado final (o botão principal passa a fechar a janela).
unsafe fn finish(hwnd: HWND, label: &str) {
    if let Ok(b) = GetDlgItem(hwnd, ID_INSTALL as i32) {
        let _ = SetWindowTextW(b, PCWSTR(wide0(label).as_ptr()));
        let _ = EnableWindow(b, true);
    }
    DONE.store(true, Ordering::Relaxed);
    show(hwnd, ID_CANCEL, false);
    for i in [ID_AUTOSTART, ID_LAUNCH, ID_PURGE, ID_UNINSTALL] { show(hwnd, i, false); }
}
