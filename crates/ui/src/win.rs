//! Widget Win32 (GDI) na barra de tarefas + ícone de bandeja + menu de opções.
//! Renderização: janela em camadas (per-pixel alpha). O GDI desenha o conteúdo duas vezes
//! (fundo preto e branco) e o alpha real de cada pixel sai da diferença: texto e ícones ficam
//! suaves e o resto é transparente, sem franjas de cor.
use crate::layout::{offset_of, place_on_bar, place_tile, Rect};
use crate::view::{flyout_tree, segments, should_hide, tooltip, visible_ids, FlyKind, Level, ProviderRow, Seg};
use lq_core::config::{Anchor, Config, DisplayMode, LabelStyle};
use lq_core::model::ProviderId;
use lq_core::notify::{Notification, Severity};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegDeleteKeyW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_UPDATE: u32 = WM_APP + 1;
const WM_TRAY: u32 = WM_APP + 2;
const WM_TOAST: u32 = WM_APP + 3;
const TIMER_POS: usize = 1;
mod settings;

// Alertas: toggles e presets de limiar.
const ID_ALERT_ENABLED: usize = 100;
const ID_ALERT_TOAST: usize = 101;
const ID_ALERT_SOUND: usize = 102;
const ID_ALERT_RESET: usize = 103;
const ID_ALERT_DONE: usize = 104;
const ID_ALERT_INPUT: usize = 105;
const ID_ALERT_TEST: usize = 106;
const ID_THRESH: usize = 110; // +0 80/90/95, +1 90/95, +2 95
const THRESHOLD_PRESETS: [&[f32]; 3] = [&[80.0, 90.0, 95.0], &[90.0, 95.0], &[95.0]];

// IDs de comando do menu.
const ID_DISPLAY: usize = 10; // +0 número, +1 barra, +2 ambos
const ID_LABELS: usize = 20; // +0 ícone, +1 texto
const ID_SHOW_5H: usize = 30;
const ID_SHOW_WEEK: usize = 31;
const ID_SHOW_ACTIVITY: usize = 32;
const ID_SHOW_REMAINING: usize = 33;
const ID_TRANSPARENT: usize = 34;
const ID_SHOW_MONTH: usize = 35;
const ID_PROVIDER: usize = 40; // +0 claude, +1 codex, +2 opencode
const ID_ANCHOR: usize = 50; // +0 bandeja, +1 início, +2 centro
const ID_AUTO: usize = 55;
const ID_AUTO_HIDE: usize = 56;
const ID_MONITOR: usize = 60; // +0 automático, +1.. monitor N
const ID_CLAUDE_LOGIN: usize = 70;
const ID_CLAUDE_PASTE: usize = 71;
const ID_CLAUDE_LOGOUT: usize = 72;
const ID_FONT: usize = 200; // +i da lista FONTS
const FONTS: [&str; 7] = ["Segoe UI Variable Text", "Segoe UI", "Bahnschrift", "Cascadia Mono", "Consolas", "Calibri", "Trebuchet MS"];
const ID_CHATWIN: usize = 210; // +i de CHAT_WINDOWS
const CHAT_WINDOWS: [u32; 6] = [10, 30, 60, 120, 360, 1440];
const ID_OC_FREE: usize = 57;
const ID_AUTOSTART: usize = 58;
const ID_SETTINGS: usize = 94;
const ID_LOCK: usize = 95;
const ID_OPEN_CONFIG: usize = 90;
const ID_REFRESH: usize = 91;
const ID_RELOAD: usize = 92;
const ID_QUIT: usize = 93;

/// Estado compartilhado entre o poller (outra thread) e a UI.
pub struct Shared {
    pub rows: Mutex<Vec<ProviderRow>>,
    pub config: Mutex<Config>,
    pub config_path: PathBuf,
    pub account: Arc<dyn crate::Account>,
    toasts: Mutex<VecDeque<Notification>>,
    hwnd: AtomicIsize,
}

impl Shared {
    pub fn new(config: Config, config_path: PathBuf, account: Arc<dyn crate::Account>) -> Arc<Self> {
        Arc::new(Self {
            rows: Mutex::new(Vec::new()), config: Mutex::new(config), config_path, account,
            toasts: Mutex::new(VecDeque::new()), hwnd: AtomicIsize::new(0),
        })
    }

    /// Enfileira uma notificação (de qualquer thread); a UI a exibe.
    pub fn toast(&self, n: Notification) {
        self.toasts.lock().unwrap().push_back(n);
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe { let _ = PostMessageW(HWND(h as *mut _), WM_TOAST, WPARAM(0), LPARAM(0)); }
        }
    }

    /// Chamado pelo poller após atualizar `rows`; acorda a UI.
    pub fn notify(&self) {
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe { let _ = PostMessageW(HWND(h as *mut _), WM_UPDATE, WPARAM(0), LPARAM(0)); }
        }
    }

    fn save(&self) {
        let txt = self.config.lock().unwrap().to_toml();
        if let Some(dir) = self.config_path.parent() { let _ = std::fs::create_dir_all(dir); }
        let _ = std::fs::write(&self.config_path, txt);
    }
}

struct Ctx { shared: Arc<Shared>, refresh: Mutex<Sender<()>> }
static CTX: OnceLock<Ctx> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
static DPI: AtomicU32 = AtomicU32::new(96);
static FONT_PX: AtomicI32 = AtomicI32::new(13);
static DRAGGING: AtomicBool = AtomicBool::new(false);
static DRAG: Mutex<Option<Drag>> = Mutex::new(None);
/// Assinatura do último quadro desenhado (evita redesenhar sem mudança).
static LAST_FRAME: Mutex<String> = Mutex::new(String::new());
/// Faixas horizontais (x local, em px) de cada provedor no tile, para localizar o clique.
static HITS: Mutex<Vec<(i32, i32, ProviderId)>> = Mutex::new(Vec::new());
/// (fonte pedida, fonte efetiva).
static FONT_NAME: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
static FLY_HWND: AtomicIsize = AtomicIsize::new(0);
static FLY_ID: Mutex<Option<ProviderId>> = Mutex::new(None);
const TIMER_FLY: usize = 2;
const TIMER_FLY_CLICK: usize = 4; // verifica cliques fora do painel (rápido)

struct Drag { cursor: (i32, i32), origin: (i32, i32), moved: bool, /// false = barra travada: o clique nunca vira arrasto.
    can_move: bool }

/// Retorna false se já existe outra instância.
pub fn single_instance() -> bool {
    unsafe {
        let _ = CreateMutexW(None, true, w!("Local\\LineQuotaSingleInstance"));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn run(shared: Arc<Shared>, refresh: Sender<()>) -> windows::core::Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        init_toast_identity();
        let hinst = GetModuleHandleW(None)?;
        let class = w!("LineQuotaTile");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        let _ = CTX.set(Ctx { shared: shared.clone(), refresh: Mutex::new(refresh) });

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class, w!("Line Quota"), WS_POPUP,
            0, 0, 10, 10, None, None, hinst, None,
        )?;
        shared.hwnd.store(hwnd.0 as isize, Ordering::Relaxed);
        add_tray(hwnd);
        relayout(hwnd);
        SetTimer(hwnd, TIMER_POS, 1000, None);
        SetTimer(hwnd, TIMER_RAISE, 250, None);
        // Clicar na barra de tarefas a traz para frente do widget: reergue na hora (e de novo logo depois).
        use windows::Win32::UI::Accessibility::SetWinEventHook;
        let _ = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const TIMER_RAISE: usize = 5; // mantém o widget acima da barra de tarefas
const TIMER_RAISE_SOON: usize = 6; // reerguida única logo após a barra de tarefas ganhar o foco

/// Garante que o widget continua visível por cima da barra (o Windows a reergue ao ser clicada).
unsafe fn raise(hwnd: HWND) {
    if DRAGGING.load(Ordering::Relaxed) || !IsWindowVisible(hwnd).as_bool() { return; }
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
}

unsafe extern "system" fn on_foreground(_: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let Some(ctx) = CTX.get() else { return };
    let h = HWND(ctx.shared.hwnd.load(Ordering::Relaxed) as *mut _);
    if h.0.is_null() { return; }
    raise(h);
    SetTimer(h, TIMER_RAISE_SOON, 60, None);
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().collect() }
fn wide0(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn scale(v: i32) -> i32 { v * DPI.load(Ordering::Relaxed) as i32 / 96 }

// ---------- Monitores ----------

#[derive(Clone, Copy)]
struct Mon { h: HMONITOR, rect: Rect, primary: bool }

unsafe extern "system" fn enum_cb(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    let v = &mut *(lp.0 as *mut Vec<Mon>);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(h, &mut mi).as_bool() {
        let r = mi.rcMonitor;
        v.push(Mon { h, rect: Rect::new(r.left, r.top, r.right, r.bottom), primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0 });
    }
    TRUE
}

/// Monitores ordenados da esquerda para a direita (numeração estável do menu).
unsafe fn monitors() -> Vec<Mon> {
    let mut v: Vec<Mon> = Vec::new();
    let _ = EnumDisplayMonitors(None, None, Some(enum_cb), LPARAM(&mut v as *mut _ as isize));
    v.sort_by_key(|m| (m.rect.l, m.rect.t));
    v
}

unsafe fn rect_of(h: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    GetWindowRect(h, &mut r).ok()?;
    Some(Rect::new(r.left, r.top, r.right, r.bottom))
}

struct Target { bar: Rect, tray: Option<Rect>, dpi: u32, fallback_gap: i32 }

/// Descobre a barra de tarefas (e a bandeja) do monitor escolhido.
unsafe fn resolve_target(cfg: &Config, own: HWND) -> Target {
    let mons = monitors();
    let main = FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default();
    let main_mon = MonitorFromWindow(if main.0.is_null() { own } else { main }, MONITOR_DEFAULTTOPRIMARY);
    let chosen = if cfg.ui.monitor == 0 { main_mon } else { mons.get(cfg.ui.monitor as usize - 1).map_or(main_mon, |m| m.h) };
    let mon_rect = mons.iter().find(|m| m.h == chosen).map_or(Rect::new(0, 0, 1920, 1080), |m| m.rect);
    let (mut dx, mut dy) = (96u32, 96u32);
    let _ = GetDpiForMonitor(chosen, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    let dpi = dx.max(96);

    let (bar, tray, fb) = if chosen == main_mon && !main.0.is_null() {
        let tray = FindWindowExW(main, None, w!("TrayNotifyWnd"), PCWSTR::null()).ok().and_then(|h| rect_of(h));
        (rect_of(main), tray, 160)
    } else {
        // Barra secundária (barra de tarefas em vários monitores).
        let mut found = None;
        let mut prev = HWND::default();
        while let Ok(h) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), PCWSTR::null()) {
            if MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY) == chosen { found = rect_of(h); break; }
            prev = h;
        }
        (found, None, 120)
    };
    // Sem barra naquele monitor: usa uma faixa na base do monitor.
    let bar = bar.unwrap_or_else(|| Rect::new(mon_rect.l, mon_rect.b - 40 * dpi as i32 / 96, mon_rect.r, mon_rect.b));
    Target { bar, tray, dpi, fallback_gap: fb * dpi as i32 / 96 }
}

// ---------- Tema e desenho ----------

/// true se a barra de tarefas do Windows está no tema claro.
fn taskbar_is_light() -> bool {
    let mut v: u32 = 0;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut c_void), Some(&mut size))
    };
    r.0 == 0 && v == 1
}

fn color(l: Level, light: bool) -> COLORREF {
    COLORREF(match (l, light) {
        (Level::Dim, false) => 0x00A0A0A0,
        (Level::Dim, true) => 0x00606060,
        (Level::Normal, false) => 0x00F0F0F0,
        (Level::Normal, true) => 0x00202020,
        (Level::Warn, false) => 0x0000B4FF,       // âmbar (BGR)
        (Level::Warn, true) => 0x000078E0,
        (Level::Crit, false) => 0x005050FF,       // vermelho
        (Level::Crit, true) => 0x002828D0,
        (Level::Active, false) => 0x0078D250,     // verde
        (Level::Active, true) => 0x00329632,
        (Level::Attention, false) => 0x00FFA040,  // azul claro
        (Level::Attention, true) => 0x00C06010,
    })
}

fn bar_color(l: Level, light: bool) -> COLORREF {
    if l == Level::Normal { COLORREF(if light { 0x00329632 } else { 0x0078C850 }) } else { color(l, light) }
}

unsafe extern "system" fn font_cb(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, lp: LPARAM) -> i32 {
    *(lp.0 as *mut bool) = true;
    0
}

unsafe fn font_installed(name: &str) -> bool {
    let mut lf = LOGFONTW { lfCharSet: DEFAULT_CHARSET, ..Default::default() };
    for (i, c) in name.encode_utf16().take(31).enumerate() { lf.lfFaceName[i] = c; }
    let hdc = GetDC(None);
    let mut found = false;
    let _ = EnumFontFamiliesExW(hdc, &lf, Some(font_cb), LPARAM(&mut found as *mut bool as isize), 0);
    ReleaseDC(None, hdc);
    found
}

/// Define a fonte do widget; se a pedida não existir, cai para Segoe UI.
unsafe fn apply_font(req: &str) {
    let mut f = FONT_NAME.lock().unwrap();
    if f.0 == req && !f.1.is_empty() { return; }
    let eff = if font_installed(req) { req.to_string() } else { "Segoe UI".to_string() };
    *f = (req.to_string(), eff);
}

unsafe fn create_font_w(px: i32, weight: i32) -> HFONT {
    let name = { let f = FONT_NAME.lock().unwrap(); if f.1.is_empty() { "Segoe UI".to_string() } else { f.1.clone() } };
    let n = wide0(&name);
    // Suavização em tons de cinza (não ClearType): é o que permite extrair o alpha corretamente.
    CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32, ANTIALIASED_QUALITY.0 as u32, 0, PCWSTR(n.as_ptr()))
}

unsafe fn create_font(px: i32) -> HFONT { create_font_w(px, 600) }

fn font_px(dpi: u32, bar_h: i32) -> i32 {
    let want = 13 * dpi as i32 / 96;
    want.min((bar_h * 6 / 10).max(8))
}

fn current_state() -> (Vec<Seg>, bool, Config, Vec<ProviderId>) {
    let Some(ctx) = CTX.get() else { return (Vec::new(), false, Config::default(), Vec::new()) };
    let rows = ctx.shared.rows.lock().unwrap();
    let cfg = ctx.shared.config.lock().unwrap();
    (segments(&rows, &cfg), should_hide(&rows, &cfg), cfg.clone(), visible_ids(&rows, &cfg))
}

fn icon_px() -> i32 { FONT_PX.load(Ordering::Relaxed) * 115 / 100 }
fn bar_w() -> i32 { scale(34) }
fn bar_h() -> i32 { (FONT_PX.load(Ordering::Relaxed) * 45 / 100).max(4) }

unsafe fn fill(hdc: HDC, r: &RECT, c: COLORREF) {
    let b = CreateSolidBrush(c);
    FillRect(hdc, r, b);
    let _ = DeleteObject(b);
}

unsafe fn line(hdc: HDC, a: (i32, i32), b: (i32, i32)) {
    let _ = MoveToEx(hdc, a.0, a.1, None);
    let _ = LineTo(hdc, b.0, b.1);
}

/// Ícones simples desenhados com primitivas GDI (sem arquivos de imagem).
unsafe fn draw_icon(hdc: HDC, id: ProviderId, x: i32, cy: i32, s: i32, light: bool) {
    let cx = x + s / 2;
    let r = s / 2 - 1;
    let pen = |c: COLORREF, wpx: i32| CreatePen(PS_SOLID, wpx.max(1), c);
    match id {
        ProviderId::Claude => {
            // Estrela de 8 pontas (marca do Claude), laranja.
            let p = pen(COLORREF(0x005777D9), s / 7);
            let old = SelectObject(hdc, p);
            let d = r * 707 / 1000;
            line(hdc, (cx - r, cy), (cx + r, cy));
            line(hdc, (cx, cy - r), (cx, cy + r));
            line(hdc, (cx - d, cy - d), (cx + d, cy + d));
            line(hdc, (cx - d, cy + d), (cx + d, cy - d));
            SelectObject(hdc, old);
            let _ = DeleteObject(p);
        }
        ProviderId::Codex => {
            // Quadrado arredondado violeta com o prompt ">_".
            let b = CreateSolidBrush(COLORREF(0x00FF5C7A));
            let np = pen(COLORREF(0x00FF5C7A), 1);
            let (ob, op) = (SelectObject(hdc, b), SelectObject(hdc, np));
            let _ = RoundRect(hdc, x, cy - r, x + s, cy + r + 1, s / 3, s / 3);
            SelectObject(hdc, ob);
            let wp = pen(COLORREF(0x00FFFFFF), s / 9);
            SelectObject(hdc, wp);
            line(hdc, (x + s * 28 / 100, cy - s * 20 / 100), (x + s * 48 / 100, cy));
            line(hdc, (x + s * 48 / 100, cy), (x + s * 28 / 100, cy + s * 20 / 100));
            line(hdc, (x + s * 54 / 100, cy + s * 22 / 100), (x + s * 76 / 100, cy + s * 22 / 100));
            SelectObject(hdc, op);
            let _ = DeleteObject(b);
            let _ = DeleteObject(np);
            let _ = DeleteObject(wp);
        }
        ProviderId::OpenCode => {
            // Quadrado com miolo contrastante (marca do OpenCode).
            let (outer, inner_c) = if light { (0x00303030, 0x00E0E0E0) } else { (0x00E0E0E0, 0x00202020) };
            let b = CreateSolidBrush(COLORREF(outer));
            let np = pen(COLORREF(outer), 1);
            let (ob, op) = (SelectObject(hdc, b), SelectObject(hdc, np));
            let _ = Rectangle(hdc, x + s / 6, cy - r, x + s - s / 6, cy + r + 1);
            SelectObject(hdc, ob);
            let inner = RECT { left: x + s * 3 / 10, top: cy - r + s / 5, right: x + s - s * 3 / 10, bottom: cy + r + 1 - s / 5 };
            fill(hdc, &inner, COLORREF(inner_c));
            SelectObject(hdc, op);
            let _ = DeleteObject(b);
            let _ = DeleteObject(np);
        }
    }
}

/// Mede (ou desenha, se `draw`) os itens. Retorna (largura total, altura do texto).
unsafe fn run_items(hdc: HDC, segs: &[Seg], origin_x: i32, height: i32, draw: bool, light: bool) -> (i32, i32) {
    let mut ext = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, &wide("Ag"), &mut ext);
    let text_h = ext.cy;
    let mut x = origin_x;
    for s in segs {
        match s {
            Seg::Text(t, l) => {
                let tw = wide(t);
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &tw, &mut sz);
                if draw {
                    SetTextColor(hdc, color(*l, light));
                    let _ = TextOutW(hdc, x, (height - text_h) / 2, &tw);
                }
                x += sz.cx;
            }
            Seg::Icon(id) => {
                if draw { draw_icon(hdc, *id, x, height / 2, icon_px(), light); }
                x += icon_px();
            }
            Seg::Bar(f, l) => {
                if draw {
                    let (bw, bh) = (bar_w(), bar_h());
                    let top = (height - bh) / 2;
                    let track = if light { 0x00C8C8C8 } else { 0x00484848 };
                    fill(hdc, &RECT { left: x, top, right: x + bw, bottom: top + bh }, COLORREF(track));
                    let fw = if *f > 0.0 { ((bw as f32 * f.clamp(0.0, 1.0)).round() as i32).max(2) } else { 0 };
                    if fw > 0 { fill(hdc, &RECT { left: x, top, right: x + fw, bottom: top + bh }, bar_color(*l, light)); }
                }
                x += bar_w();
            }
        }
    }
    (x - origin_x, text_h)
}

/// Desenha o quadro e o envia à janela em camadas.
unsafe fn render(hwnd: HWND, segs: &[Seg], rect: Rect, transparent: bool, light: bool) {
    let (w, h) = (rect.w().max(1), rect.h().max(1));
    let screen = GetDC(None);
    let mem = CreateCompatibleDC(screen);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h,
            biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let Ok(bmp) = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        return;
    };
    let old_bmp = SelectObject(mem, bmp);
    let font = create_font(FONT_PX.load(Ordering::Relaxed));
    let old_font = SelectObject(mem, font);
    SetBkMode(mem, TRANSPARENT);
    let full = RECT { left: 0, top: 0, right: w, bottom: h };
    let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    let pad = scale(8);

    if transparent {
        // Passo 1: sobre preto. Passo 2: sobre branco. alpha = 255 - (branco - preto).
        fill(mem, &full, COLORREF(0x00000000));
        run_items(mem, segs, pad, h, true, light);
        let black = px.to_vec();
        fill(mem, &full, COLORREF(0x00FFFFFF));
        run_items(mem, segs, pad, h, true, light);
        for (i, p) in px.chunks_exact_mut(4).enumerate() {
            let b = &black[i * 4..i * 4 + 3];
            let d: i32 = (0..3).map(|c| (p[c] as i32 - b[c] as i32).max(0)).sum::<i32>() / 3;
            let a = (255 - d).clamp(0, 255);
            for c in 0..3 { p[c] = (b[c] as i32).min(a) as u8; } // cor pré-multiplicada
            p[3] = a.max(1) as u8; // alpha mínimo 1: a área inteira recebe cliques/arraste
        }
    } else {
        fill(mem, &full, COLORREF(if light { 0x00F3F3F3 } else { 0x00202020 }));
        run_items(mem, segs, pad, h, true, light);
        for p in px.chunks_exact_mut(4) { p[3] = 255; }
    }

    let pos = POINT { x: rect.l, y: rect.t };
    let size = SIZE { cx: w, cy: h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let _ = UpdateLayeredWindow(hwnd, HDC::default(), Some(&pos), Some(&size), mem, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);

    SelectObject(mem, old_font);
    let _ = DeleteObject(font);
    SelectObject(mem, old_bmp);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
}

/// Mede o conteúdo, escolhe barra/monitor/posição, aplica e redesenha se algo mudou.
unsafe fn relayout(hwnd: HWND) {
    if CTX.get().is_none() || DRAGGING.load(Ordering::Relaxed) { return; }
    let (segs, hide, cfg, ids) = current_state();
    if hide {
        hide_flyout();
        let _ = ShowWindow(hwnd, SW_HIDE);
        LAST_FRAME.lock().unwrap().clear();
        return;
    }
    let t = resolve_target(&cfg, hwnd);
    apply_font(&cfg.ui.font);
    DPI.store(t.dpi, Ordering::Relaxed);
    let thickness = t.bar.h().min(t.bar.w());
    FONT_PX.store(font_px(t.dpi, thickness), Ordering::Relaxed);

    let font = create_font(FONT_PX.load(Ordering::Relaxed));
    let hdc = GetDC(hwnd);
    let old = SelectObject(hdc, font);
    let (content_w, text_h) = run_items(hdc, &segs, 0, 0, false, false);
    update_hits(hdc, &segs, &ids, content_w);
    SelectObject(hdc, old);
    let _ = DeleteObject(font);
    ReleaseDC(hwnd, hdc);

    let (w, h) = (content_w + 2 * scale(8), text_h.max(icon_px()) + scale(6));
    let r = if cfg.ui.anchor == Anchor::Custom {
        place_on_bar(t.bar, t.tray, w, h, cfg.ui.bar_offset, scale(4), t.fallback_gap)
    } else {
        place_tile(t.bar, t.tray, w, h, cfg.ui.anchor, scale(4), t.fallback_gap)
    };
    let light = taskbar_is_light();
    let sig = format!("{segs:?}|{}.{}.{}.{}|{}|{light}|{}|{}", r.l, r.t, r.r, r.b, cfg.ui.transparent, t.dpi, FONT_NAME.lock().unwrap().1);
    let mut last = LAST_FRAME.lock().unwrap();
    if *last == sig && IsWindowVisible(hwnd).as_bool() {
        // Nada mudou; só garante que continua por cima (a barra pode ter se reerguido).
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        return;
    }
    *last = sig;
    drop(last);
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, r.l, r.t, r.w(), r.h(), SWP_NOACTIVATE | SWP_SHOWWINDOW);
    render(hwnd, &segs, r, cfg.ui.transparent, light);
}

// ---------- Bandeja e menu ----------

unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        // Ícone embutido no exe (recurso 1); se faltar, o padrão do Windows.
        hIcon: GetModuleHandleW(None).ok()
            .and_then(|h| LoadIconW(h, PCWSTR(1 as *const u16)).ok())
            .or_else(|| LoadIconW(None, IDI_APPLICATION).ok())
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(ctx) = CTX.get() {
        let tip = {
            let rows = ctx.shared.rows.lock().unwrap();
            let cfg = ctx.shared.config.lock().unwrap();
            tooltip(&rows, &cfg)
        };
        for (i, c) in tip.encode_utf16().take(127).enumerate() { nid.szTip[i] = c; }
    }
    nid
}

const AUMID: &str = "LineQuota.App";
static LOGO_URL: OnceLock<String> = OnceLock::new();
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

unsafe fn reg_sz(subkey: &str, name: &str, value: &str) {
    let v = wide0(value);
    let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(wide0(subkey).as_ptr()), PCWSTR(wide0(name).as_ptr()),
        REG_SZ.0, Some(v.as_ptr() as *const c_void), (v.len() * 2) as u32);
}

fn shortcut_path() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
        .join("Microsoft\\Windows\\Start Menu\\Programs\\Line Quota.lnk")
}

/// O Windows só mostra toasts de apps sem instalador se existir um atalho no Menu Iniciar
/// carregando o AppUserModelID. Cria o atalho uma vez (remova com `line-quota --unregister`).
unsafe fn ensure_shortcut() {
    use windows::core::{Interface, GUID, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY};
    let lnk = shortcut_path();
    if lnk.exists() { return; }
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = wide0(&exe.to_string_lossy());
    let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { return };
    let _ = link.SetPath(PCWSTR(exe.as_ptr()));
    let _ = link.SetIconLocation(PCWSTR(exe.as_ptr()), 0);
    let _ = link.SetDescription(w!("Line Quota"));
    if let Ok(store) = link.cast::<IPropertyStore>() {
        // PKEY_AppUserModel_ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
        let key = PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };
        let pv = PROPVARIANT::from(AUMID);
        let _ = store.SetValue(&key, &pv);
        let _ = store.Commit();
    }
    if let Ok(file) = link.cast::<IPersistFile>() {
        if let Some(dir) = lnk.parent() { let _ = std::fs::create_dir_all(dir); }
        let _ = file.Save(PCWSTR(wide0(&lnk.to_string_lossy()).as_ptr()), true);
    }
}

/// Remove o atalho e o registro criados para as notificações.
pub fn unregister_toast_identity() {
    let _ = std::fs::remove_file(shortcut_path());
    unsafe { let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide0(&format!("Software\\Classes\\AppUserModelId\\{AUMID}")).as_ptr())); }
}

/// Identidade do app para os toasts: nome "Line Quota" e logo próprio (registro do usuário, sem instalador).
unsafe fn init_toast_identity() {
    let _ = RoInitialize(RO_INIT_MULTITHREADED);
    let dir = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("LineQuota");
    let _ = std::fs::create_dir_all(&dir);
    let icon = dir.join("icon.png");
    if std::fs::read(&icon).map_or(true, |b| b != ICON_PNG) { let _ = std::fs::write(&icon, ICON_PNG); }
    let path = icon.to_string_lossy().to_string();
    let key = format!("Software\\Classes\\AppUserModelId\\{AUMID}");
    reg_sz(&key, "DisplayName", "Line Quota");
    reg_sz(&key, "IconUri", &path);
    reg_sz(&key, "IconBackgroundColor", "FF1B2336");
    let _ = SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide0(AUMID).as_ptr()));
    ensure_shortcut();
    let _ = LOGO_URL.set(format!("file:///{}", path.replace('\\', "/").replace(' ', "%20")));
}

/// Exibe um toast nativo (WinRT). Erro = usar o balão da bandeja.
unsafe fn winrt_toast(n: &Notification, sound: bool) -> windows::core::Result<()> {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    let xml = crate::toast::toast_xml(n, LOGO_URL.get().map_or("", |s| s.as_str()), sound);
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

fn copy_wide<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (i, c) in s.encode_utf16().take(N - 1).enumerate() { dst[i] = c; }
}

/// Exibe as notificações pendentes. Várias de uma vez viram uma só (o Windows sobrescreve balões).
unsafe fn show_toasts(hwnd: HWND) {
    let Some(ctx) = CTX.get() else { return };
    let batch: Vec<Notification> = ctx.shared.toasts.lock().unwrap().drain(..).collect();
    if batch.is_empty() { return; }
    let (toast, sound) = {
        let c = ctx.shared.config.lock().unwrap();
        (c.alerts.toast, c.alerts.sound)
    };
    if !toast {
        if sound { let _ = MessageBeep(MB_ICONASTERISK); }
        return;
    }
    // Toast nativo com logo e barra de progresso; o balão da bandeja é só plano B.
    let batch: Vec<Notification> = batch.into_iter().filter(|n| winrt_toast(n, sound).is_err()).collect();
    if batch.is_empty() { return; }
    let (title, body, sev) = if batch.len() == 1 {
        (batch[0].title.clone(), batch[0].body.clone(), batch[0].severity)
    } else {
        let worst = if batch.iter().any(|n| n.severity == Severity::Crit) { Severity::Crit }
            else if batch.iter().any(|n| n.severity == Severity::Warn) { Severity::Warn } else { Severity::Info };
        (format!("Line Quota: {} avisos", batch.len()),
         batch.iter().map(|n| n.title.as_str()).collect::<Vec<_>>().join("\n"), worst)
    };
    let mut nid = tray_data(hwnd);
    nid.uFlags |= NIF_INFO;
    copy_wide(&mut nid.szInfoTitle, &title);
    copy_wide(&mut nid.szInfo, &body);
    let kind = match sev { Severity::Info => NIIF_INFO, Severity::Warn => NIIF_WARNING, Severity::Crit => NIIF_ERROR };
    nid.dwInfoFlags = if sound { kind } else { kind | NIIF_NOSOUND };
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
}

unsafe fn add_tray(hwnd: HWND) { let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(hwnd)); }
unsafe fn update_tray(hwnd: HWND) { let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(hwnd)); }

unsafe fn add(menu: HMENU, id: usize, text: &str, checked: bool) {
    let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, flags, id, PCWSTR(wide0(text).as_ptr()));
}


/// Menu do botão direito: só o essencial. O resto fica na janela de configurações.
unsafe fn show_menu(hwnd: HWND) {
    let Some(ctx) = CTX.get() else { return };
    let locked = ctx.shared.config.lock().unwrap().ui.locked;
    let Ok(menu) = CreatePopupMenu() else { return };
    add(menu, ID_LOCK, if locked { "Destravar barra" } else { "Travar barra" }, false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    add(menu, ID_REFRESH, "Atualizar", false);
    add(menu, ID_SETTINGS, "Abrir configurações", false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    add(menu, ID_QUIT, "Sair", false);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
}

fn open_url(url: &str) {
    unsafe { ShellExecuteW(None, w!("open"), PCWSTR(wide0(url).as_ptr()), None, None, SW_SHOWNORMAL); }
}

fn message(title: &str, text: &str) {
    unsafe { MessageBoxW(None, PCWSTR(wide0(text).as_ptr()), PCWSTR(wide0(title).as_ptr()), MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST); }
}

/// Pergunta Sim/Não; true se o usuário clicou em Sim.
fn ask_yes_no(title: &str, text: &str) -> bool {
    unsafe {
        MessageBoxW(None, PCWSTR(wide0(text).as_ptr()), PCWSTR(wide0(title).as_ptr()), MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST) == IDYES
    }
}

/// Texto da área de transferência (limitado a 2 KB; códigos de login são curtos).
fn read_clipboard() -> String {
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(None).is_err() { return String::new(); }
        let mut out = String::new();
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while n < 2048 && *p.add(n) != 0 { n += 1; }
                out = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                let _ = GlobalUnlock(HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

/// Lê o código da área de transferência, conclui o login e avisa o resultado (roda em thread própria).
fn paste_code(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        match shared.account.complete(&read_clipboard()) {
            Ok(()) => message("Claude", "Conectado! Os limites aparecem em instantes."),
            Err(e) => message("Claude", &e),
        }
        request_refresh();
        shared.notify();
    });
}

fn request_refresh() {
    if let Some(ctx) = CTX.get() { let _ = ctx.refresh.lock().unwrap().send(()); }
}

/// Aplica uma escolha do menu à configuração e persiste. Retorna true se algo mudou.
unsafe fn apply_command(hwnd: HWND, id: usize) -> bool {
    let Some(ctx) = CTX.get() else { return false };
    let sh = &ctx.shared;
    match id {
        ID_OPEN_CONFIG => {
            sh.save();
            let p = wide0(&format!("\"{}\"", sh.config_path.to_string_lossy()));
            ShellExecuteW(None, w!("open"), w!("notepad.exe"), PCWSTR(p.as_ptr()), None, SW_SHOWNORMAL);
            return false;
        }
        ID_SETTINGS => { settings::open(hwnd); return false; }
        ID_REFRESH => { request_refresh(); return false; }
        ID_CLAUDE_LOGIN => {
            let full = sh.config.lock().unwrap().account.claude_full_scope;
            let shared = sh.clone();
            std::thread::spawn(move || match shared.account.login_url(full) {
                Ok(url) => {
                    open_url(&url);
                    // Botão de colar já na própria janela: Sim = colar o código copiado.
                    if ask_yes_no("Entrar no Claude",
                        "1) Autorize o acesso no navegador que abriu.\n2) Copie o código mostrado na página (CÓDIGO#STATE).\n3) Volte aqui e clique em SIM para colar o código.\n\nNÃO: cole depois em Conta Claude > Colar código copiado.") {
                        paste_code(shared);
                    }
                }
                Err(e) => message("Entrar no Claude", &e),
            });
            return false;
        }
        ID_ALERT_TEST => {
            let mut n = Notification::new("🔔 Line Quota: notificação de teste", "Se você está vendo isto, os alertas funcionam.", Severity::Info);
            n.progress = Some(lq_core::notify::Progress { fraction: 0.62, title: "Exemplo: limite de 5 horas".into(), status: "62% usado".into() });
            sh.toast(n);
            return false;
        }
        ID_CLAUDE_PASTE => { paste_code(sh.clone()); return false; }
        ID_CLAUDE_LOGOUT => { sh.account.logout(); request_refresh(); return true; }
        ID_QUIT => { let _ = DestroyWindow(hwnd); return false; }
        ID_RELOAD => {
            if let Ok(txt) = std::fs::read_to_string(&sh.config_path) {
                if let Ok(c) = Config::from_toml(&txt) {
                    *sh.config.lock().unwrap() = c;
                    return true;
                }
            }
            return false;
        }
        _ => {}
    }
    {
        let mut c = sh.config.lock().unwrap();
        match id {
            ID_DISPLAY => c.ui.display = DisplayMode::Number,
            x if x == ID_DISPLAY + 1 => c.ui.display = DisplayMode::Bar,
            x if x == ID_DISPLAY + 2 => c.ui.display = DisplayMode::Both,
            ID_TRANSPARENT => c.ui.transparent = !c.ui.transparent,
            ID_LABELS => c.ui.labels = LabelStyle::Icon,
            x if x == ID_LABELS + 1 => c.ui.labels = LabelStyle::Text,
            ID_SHOW_5H => { let v = !c.claude.show_five_hour; for p in c.providers_mut() { p.show_five_hour = v; } }
            ID_SHOW_WEEK => { let v = !c.claude.show_weekly; for p in c.providers_mut() { p.show_weekly = v; } }
            ID_SHOW_MONTH => { let v = !c.claude.show_monthly; for p in c.providers_mut() { p.show_monthly = v; } }
            ID_LOCK => c.ui.locked = !c.ui.locked,
            ID_SHOW_ACTIVITY => { let v = !c.claude.show_activity; for p in c.providers_mut() { p.show_activity = v; } }
            ID_SHOW_REMAINING => c.ui.show_remaining = !c.ui.show_remaining,
            ID_PROVIDER => c.claude.enabled = !c.claude.enabled,
            x if x == ID_PROVIDER + 1 => c.codex.enabled = !c.codex.enabled,
            x if x == ID_PROVIDER + 2 => c.opencode.enabled = !c.opencode.enabled,
            ID_ALERT_ENABLED => c.alerts.enabled = !c.alerts.enabled,
            ID_ALERT_TOAST => c.alerts.toast = !c.alerts.toast,
            ID_ALERT_SOUND => c.alerts.sound = !c.alerts.sound,
            ID_ALERT_RESET => c.alerts.notify_reset = !c.alerts.notify_reset,
            ID_ALERT_DONE => c.alerts.notify_task_done = !c.alerts.notify_task_done,
            ID_ALERT_INPUT => c.alerts.notify_needs_input = !c.alerts.notify_needs_input,
            x if (ID_THRESH..ID_THRESH + THRESHOLD_PRESETS.len()).contains(&x) => c.alerts.thresholds = THRESHOLD_PRESETS[x - ID_THRESH].to_vec(),
            x if (ID_FONT..ID_FONT + FONTS.len()).contains(&x) => c.ui.font = FONTS[x - ID_FONT].to_string(),
            ID_OC_FREE => c.ui.opencode_free_icon_only = !c.ui.opencode_free_icon_only,
            x if (ID_CHATWIN..ID_CHATWIN + CHAT_WINDOWS.len()).contains(&x) => c.ui.chat_window_mins = CHAT_WINDOWS[x - ID_CHATWIN],
            ID_AUTOSTART => { let on = !autostart_enabled(); set_autostart(on); c.ui.autostart = on; }
            ID_AUTO => c.ui.auto_mode = !c.ui.auto_mode,
            ID_AUTO_HIDE => c.ui.auto_hide_when_none = !c.ui.auto_hide_when_none,
            ID_ANCHOR => c.ui.anchor = Anchor::BesideTray,
            x if x == ID_ANCHOR + 1 => c.ui.anchor = Anchor::TaskbarStart,
            x if x == ID_ANCHOR + 2 => c.ui.anchor = Anchor::TaskbarCenter,
            x if (ID_MONITOR..ID_MONITOR + 16).contains(&x) => c.ui.monitor = (x - ID_MONITOR) as u32,
            _ => return false,
        }
    }
    sh.save();
    true
}

/// Durante o arraste: o widget só anda ao longo da barra (nunca sai dela nem cobre a bandeja).
unsafe fn drag_to(hwnd: HWND, d: &Drag, dx: i32, dy: i32) {
    let Some(ctx) = CTX.get() else { return };
    let Some(cur) = rect_of(hwnd) else { return };
    let cfg = ctx.shared.config.lock().unwrap().clone();
    let t = resolve_target(&cfg, hwnd);
    let desired = if t.bar.w() >= t.bar.h() { d.origin.0 + dx - t.bar.l } else { d.origin.1 + dy - t.bar.t };
    let r = place_on_bar(t.bar, t.tray, cur.w(), cur.h(), desired, scale(4), t.fallback_gap);
    let _ = SetWindowPos(hwnd, None, r.l, r.t, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
}

/// Fim do arraste: grava a distância desde o início da barra.
unsafe fn finish_drag(hwnd: HWND) {
    let Some(ctx) = CTX.get() else { return };
    let Some(r) = rect_of(hwnd) else { return };
    let cfg = ctx.shared.config.lock().unwrap().clone();
    let t = resolve_target(&cfg, hwnd);
    {
        let mut c = ctx.shared.config.lock().unwrap();
        c.ui.anchor = Anchor::Custom;
        c.ui.bar_offset = offset_of(t.bar, r);
    }
    ctx.shared.save();
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            // Janela em camadas: o conteúdo vem de UpdateLayeredWindow; só valida a área.
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_UPDATE => { relayout(hwnd); update_tray(hwnd); LRESULT(0) }
        WM_TOAST => { show_toasts(hwnd); LRESULT(0) }
        WM_TIMER if wp.0 == TIMER_RAISE => { raise(hwnd); LRESULT(0) }
        WM_TIMER if wp.0 == TIMER_RAISE_SOON => { let _ = KillTimer(hwnd, TIMER_RAISE_SOON); raise(hwnd); LRESULT(0) }
        WM_TIMER => { relayout(hwnd); LRESULT(0) } // acompanha mudanças da barra (posição, DPI, auto-hide, tema)
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => { relayout(hwnd); LRESULT(0) }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let o = rect_of(hwnd).map_or((0, 0), |r| (r.l, r.t));
            let can_move = CTX.get().map_or(false, |c| !c.shared.config.lock().unwrap().ui.locked);
            *DRAG.lock().unwrap() = Some(Drag { cursor: (pt.x, pt.y), origin: o, moved: false, can_move });
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // Perdeu a captura no meio de um clique/arrasto (ex.: clique na barra de tarefas): encerra sem travar o relayout.
            let d = DRAG.lock().unwrap().take();
            if let Some(d) = d {
                DRAGGING.store(false, Ordering::Relaxed);
                if d.moved { finish_drag(hwnd); LAST_FRAME.lock().unwrap().clear(); relayout(hwnd); }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(d) = DRAG.lock().unwrap().as_mut().filter(|d| d.can_move) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let (dx, dy) = (pt.x - d.cursor.0, pt.y - d.cursor.1);
                if d.moved || dx.abs() > 4 || dy.abs() > 4 {
                    d.moved = true;
                    DRAGGING.store(true, Ordering::Relaxed);
                    drag_to(hwnd, d, dx, dy);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let d = DRAG.lock().unwrap().take(); // antes de soltar a captura (que dispara WM_CAPTURECHANGED)
            let _ = ReleaseCapture();
            DRAGGING.store(false, Ordering::Relaxed);
            match d {
                Some(d) if d.moved => { finish_drag(hwnd); LAST_FRAME.lock().unwrap().clear(); relayout(hwnd); }
                _ => { request_refresh(); toggle_flyout(hwnd); }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => { show_menu(hwnd); LRESULT(0) }
        WM_TRAY => {
            let ev = (lp.0 as u32) & 0xFFFF;
            if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU { show_menu(hwnd); }
            else if ev == WM_LBUTTONUP { request_refresh(); }
            LRESULT(0)
        }
        WM_COMMAND => {
            if apply_command(hwnd, wp.0 & 0xFFFF) {
                relayout(hwnd);
                update_tray(hwnd);
                settings::sync_if_open();
                if let Some(ctx) = CTX.get() { ctx.shared.notify(); }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if Some(&m) == TASKBAR_CREATED.get() => { add_tray(hwnd); LAST_FRAME.lock().unwrap().clear(); relayout(hwnd); LRESULT(0) } // explorer reiniciou
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------- Painel de detalhes (flyout) ----------

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Calcula as faixas de clique de cada provedor (separados pelo texto de 3 espaços).
unsafe fn update_hits(hdc: HDC, segs: &[Seg], ids: &[ProviderId], content_w: i32) {
    let is_sep = |s: &Seg| matches!(s, Seg::Text(t, _) if t == "   ");
    let groups: Vec<&[Seg]> = segs.split(|s| is_sep(s)).collect();
    let mut hits = Vec::new();
    if groups.len() == ids.len() {
        let pad = scale(8);
        let sep_w = run_items(hdc, &[Seg::Text("   ".into(), Level::Dim)], 0, 0, false, false).0;
        let mut x = pad;
        for (i, (g, id)) in groups.iter().zip(ids).enumerate() {
            let w = run_items(hdc, g, 0, 0, false, false).0;
            let l = if i == 0 { 0 } else { x - sep_w / 2 };
            let r = if i + 1 == groups.len() { content_w + 2 * pad } else { x + w + sep_w / 2 };
            hits.push((l, r, *id));
            x += w + sep_w;
        }
    }
    *HITS.lock().unwrap() = hits;
}

fn fly_width() -> i32 { scale(360) }

/// Chaves alternadas pelo usuário (invertem o aberto/fechado padrão da árvore).
static TOGGLED: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));
/// Faixas verticais (topo, base, chave) dos nós clicáveis, em coordenadas do cliente.
static FLY_ROWS: Mutex<Vec<(i32, i32, String)>> = Mutex::new(Vec::new());
static FLY_SCROLL: AtomicI32 = AtomicI32::new(0);
static FLY_CONTENT_H: AtomicI32 = AtomicI32::new(0);

unsafe fn triangle(hdc: HDC, x: i32, cy: i32, s: i32, open: bool, c: COLORREF) {
    let (b, p) = (CreateSolidBrush(c), CreatePen(PS_SOLID, 1, c));
    let (ob, op) = (SelectObject(hdc, b), SelectObject(hdc, p));
    let pts = if open {
        [POINT { x, y: cy - s / 3 }, POINT { x: x + s, y: cy - s / 3 }, POINT { x: x + s / 2, y: cy + s / 2 }]
    } else {
        [POINT { x: x + s / 6, y: cy - s / 2 }, POINT { x: x + s / 6, y: cy + s / 2 }, POINT { x: x + s * 5 / 6, y: cy }]
    };
    let _ = Polygon(hdc, &pts);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    let _ = DeleteObject(b);
    let _ = DeleteObject(p);
}

/// Mede (ou desenha) o conteúdo do painel; devolve a altura total (sem rolagem).
unsafe fn draw_flyout(hdc: HDC, lines: &[crate::view::FlyLine], w: i32, draw: bool, light: bool) -> i32 {
    let pad = scale(14);
    let indent = scale(16);
    let base = scale(13);
    let (f_body, f_title, f_node) = (create_font_w(base, 500), create_font_w(base + scale(2), 700), create_font_w(base, 600));
    let old = SelectObject(hdc, f_body);
    let label_w = lines.iter().filter(|l| l.kind == FlyKind::Body && !l.label.is_empty()).map(|l| {
        let mut sz = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &wide(&l.label), &mut sz);
        sz.cx
    }).max().unwrap_or(0) + scale(12);
    let scroll = if draw { FLY_SCROLL.load(Ordering::Relaxed) } else { 0 };
    if draw { FLY_ROWS.lock().unwrap().clear(); }
    let mut y = pad - scroll;
    for l in lines {
        let x0 = pad + l.depth as i32 * indent;
        match l.kind {
            FlyKind::Gap => y += scale(8),
            FlyKind::Title => {
                SelectObject(hdc, f_title);
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &wide(&l.label), &mut sz);
                if draw {
                    SetTextColor(hdc, color(Level::Normal, light));
                    let _ = TextOutW(hdc, pad, y, &wide(&l.label));
                    SelectObject(hdc, f_body);
                    SetTextColor(hdc, color(l.level, light));
                    let _ = TextOutW(hdc, pad + sz.cx + scale(10), y + scale(1), &wide(&l.text));
                }
                y += sz.cy + scale(8);
                SelectObject(hdc, f_body);
            }
            FlyKind::Node => {
                let h = scale(24);
                if draw {
                    let cy = y + h / 2;
                    if l.key.is_some() { triangle(hdc, x0, cy, scale(9), l.open, color(Level::Dim, light)); }
                    SelectObject(hdc, f_node);
                    // Estado alinhado à direita; título com reticências se não couber.
                    let mut st = wide(&l.text);
                    let mut sz = SIZE::default();
                    let _ = GetTextExtentPoint32W(hdc, &st, &mut sz);
                    SelectObject(hdc, f_body);
                    SetTextColor(hdc, color(l.level, light));
                    let mut rs = RECT { left: w - pad - sz.cx, top: y, right: w - pad, bottom: y + h };
                    DrawTextW(hdc, &mut st, &mut rs, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                    SelectObject(hdc, f_node);
                    SetTextColor(hdc, color(Level::Normal, light));
                    let mut t = wide(&l.label);
                    let mut rt = RECT { left: x0 + scale(16), top: y, right: (w - pad - sz.cx - scale(10)).max(x0 + scale(40)), bottom: y + h };
                    DrawTextW(hdc, &mut t, &mut rt, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
                    SelectObject(hdc, f_body);
                    if let Some(k) = &l.key { FLY_ROWS.lock().unwrap().push((y, y + h, k.clone())); }
                }
                y += h;
            }
            FlyKind::Label | FlyKind::Body => {
                let lx = x0;
                let tw = (w - pad - lx - label_w).max(scale(80));
                let mut rc = RECT { left: lx + label_w, top: y, right: lx + label_w + tw, bottom: y + 4000 };
                let mut t = wide(&l.text);
                if t.is_empty() { t.push(32); }
                DrawTextW(hdc, &mut t, &mut rc, DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX);
                let h = (rc.bottom - y).max(scale(16));
                if draw {
                    if !l.label.is_empty() {
                        SetTextColor(hdc, color(Level::Dim, light));
                        let _ = TextOutW(hdc, lx, y, &wide(&l.label));
                    }
                    SetTextColor(hdc, color(l.level, light));
                    let mut rc2 = RECT { left: lx + label_w, top: y, right: lx + label_w + tw, bottom: y + h };
                    DrawTextW(hdc, &mut t, &mut rc2, DT_WORDBREAK | DT_NOPREFIX);
                }
                y += h + scale(5);
            }
        }
    }
    SelectObject(hdc, old);
    let _ = DeleteObject(f_body);
    let _ = DeleteObject(f_title);
    let _ = DeleteObject(f_node);
    y + scroll + pad - scale(5)
}

fn fly_lines() -> Vec<crate::view::FlyLine> {
    let Some(ctx) = CTX.get() else { return Vec::new() };
    let Some(id) = *FLY_ID.lock().unwrap() else { return Vec::new() };
    let toggled = TOGGLED.lock().unwrap().clone();
    let rows = ctx.shared.rows.lock().unwrap();
    let mins = ctx.shared.config.lock().unwrap().ui.chat_window_mins;
    rows.iter().find(|r| r.id == id).map(|r| flyout_tree(r, &toggled, now_secs(), mins)).unwrap_or_default()
}

unsafe fn fly_hwnd(tile: HWND) -> HWND {
    let h = FLY_HWND.load(Ordering::Relaxed);
    if h != 0 { return HWND(h as *mut _); }
    let Ok(hinst) = GetModuleHandleW(None) else { return HWND::default() };
    let class = w!("LineQuotaFlyout");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(fly_proc), hInstance: hinst.into(), lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default()
    };
    RegisterClassW(&wc);
    let _ = tile;
    let Ok(h) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, class, w!("Line Quota"), WS_POPUP,
        0, 0, 10, 10, None, None, hinst, None) else { return HWND::default() };
    FLY_HWND.store(h.0 as isize, Ordering::Relaxed);
    h
}

/// Mede o conteúdo, posiciona junto ao tile (acima se a barra está embaixo, abaixo se no topo) e mostra.
unsafe fn place_flyout(fly: HWND, tile: HWND) {
    let lines = fly_lines();
    if lines.is_empty() { hide_flyout(); return; }
    let light = taskbar_is_light();
    let w = fly_width();
    let hdc = GetDC(fly);
    let total = draw_flyout(hdc, &lines, w, false, light);
    ReleaseDC(fly, hdc);
    let Some(tr) = rect_of(tile) else { return };
    let id = (*FLY_ID.lock().unwrap()).unwrap_or(ProviderId::Claude);
    let (l, r) = HITS.lock().unwrap().iter().find(|x| x.2 == id).map_or((0, tr.w()), |x| (x.0, x.1));
    let mon = MonitorFromWindow(tile, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let work = mi.rcWork;
    let h = total.min(((work.bottom - work.top) * 7 / 10).max(scale(120)));
    FLY_CONTENT_H.store(total, Ordering::Relaxed);
    let max_scroll = (total - h).max(0);
    if FLY_SCROLL.load(Ordering::Relaxed) > max_scroll { FLY_SCROLL.store(max_scroll, Ordering::Relaxed); }
    let gap = scale(8);
    let x = (tr.l + (l + r) / 2 - w / 2).clamp(work.left + gap, (work.right - w - gap).max(work.left));
    let below = tr.t + tr.h() / 2 < (work.top + work.bottom) / 2;
    let y = if below { (tr.b + gap).min(work.bottom - h) } else { (tr.t - gap - h).max(work.top) };
    let cur = rect_of(fly);
    if cur != Some(Rect::new(x, y, x + w, y + h)) || !IsWindowVisible(fly).as_bool() {
        let _ = SetWindowPos(fly, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, scale(14), scale(14));
        SetWindowRgn(fly, rgn, true);
    }
    let _ = InvalidateRect(fly, None, false);
    SetTimer(fly, TIMER_FLY, 1000, None);
    SetTimer(fly, TIMER_FLY_CLICK, 40, None);
}

unsafe fn hide_flyout() {
    FLY_SCROLL.store(0, Ordering::Relaxed);
    *FLY_ID.lock().unwrap() = None;
    let h = FLY_HWND.load(Ordering::Relaxed);
    if h != 0 {
        let fly = HWND(h as *mut _);
        let _ = KillTimer(fly, TIMER_FLY);
        let _ = KillTimer(fly, TIMER_FLY_CLICK);
        let _ = ShowWindow(fly, SW_HIDE);
    }
}

/// Clique no tile: abre o painel do provedor sob o cursor (ou fecha se já estava aberto).
unsafe fn toggle_flyout(tile: HWND) -> bool {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let Some(tr) = rect_of(tile) else { return false };
    let x = pt.x - tr.l;
    let Some(id) = HITS.lock().unwrap().iter().find(|h| x >= h.0 && x < h.1).map(|h| h.2) else { return false };
    if *FLY_ID.lock().unwrap() == Some(id) { hide_flyout(); return true; }
    *FLY_ID.lock().unwrap() = Some(id);
    let fly = fly_hwnd(tile);
    if fly.0.is_null() { return false; }
    place_flyout(fly, tile);
    true
}

unsafe extern "system" fn fly_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let light = taskbar_is_light();
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right.max(1), rc.bottom.max(1));
            let old = SelectObject(mem, bmp);
            fill(mem, &rc, COLORREF(if light { 0x00F6F6F6 } else { 0x002B2B2B }));
            // Borda fina.
            let edge = CreateSolidBrush(COLORREF(if light { 0x00D0D0D0 } else { 0x00444444 }));
            FrameRect(mem, &rc, edge);
            let _ = DeleteObject(edge);
            SetBkMode(mem, TRANSPARENT);
            draw_flyout(mem, &fly_lines(), rc.right, true, light);
            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let key = FLY_ROWS.lock().unwrap().iter().find(|r| y >= r.0 && y < r.1).map(|r| r.2.clone());
            if let Some(k) = key {
                {
                    let mut t = TOGGLED.lock().unwrap();
                    if !t.remove(&k) { t.insert(k); }
                }
                let tile = CTX.get().map(|c| HWND(c.shared.hwnd.load(Ordering::Relaxed) as *mut _)).unwrap_or_default();
                place_flyout(hwnd, tile);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let max = (FLY_CONTENT_H.load(Ordering::Relaxed) - rc.bottom).max(0);
            let cur = FLY_SCROLL.load(Ordering::Relaxed);
            FLY_SCROLL.store((cur - delta * scale(40) / 120).clamp(0, max), Ordering::Relaxed);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_TIMER => {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let inside = |r: Option<Rect>| r.map_or(false, |r| pt.x >= r.l && pt.x < r.r && pt.y >= r.t && pt.y < r.b);
            let tile = CTX.get().map(|c| HWND(c.shared.hwnd.load(Ordering::Relaxed) as *mut _)).unwrap_or_default();
            let down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| GetAsyncKeyState(k.0 as i32) < 0);
            let clicked_away = down && !inside(rect_of(hwnd)) && !inside(rect_of(tile));
            if clicked_away || GetAsyncKeyState(VK_ESCAPE.0 as i32) < 0 { hide_flyout(); }
            else if wp.0 == TIMER_FLY { place_flyout(hwnd, tile); } // o timer rápido só vigia cliques
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------- Iniciar com o Windows ----------

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn autostart_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(wide0(RUN_KEY).as_ptr()), w!("LineQuota"), RRF_RT_REG_SZ, None, None, Some(&mut size)).0 == 0
    }
}

fn set_autostart(on: bool) {
    unsafe {
        if on {
            if let Ok(exe) = std::env::current_exe() { reg_sz(RUN_KEY, "LineQuota", &format!("\"{}\"", exe.to_string_lossy())); }
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(wide0(RUN_KEY).as_ptr()), w!("LineQuota"));
        }
    }
}

// ---------- Suporte ao instalador (registro do usuário; sem administrador) ----------

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\LineQuota";

/// Liga/desliga o início com o Windows apontando para `exe`.
pub fn set_autostart_for(exe: &std::path::Path, on: bool) {
    unsafe {
        if on { reg_sz(RUN_KEY, "LineQuota", &format!("\"{}\"", exe.to_string_lossy())); }
        else { let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(wide0(RUN_KEY).as_ptr()), w!("LineQuota")); }
    }
}

/// Entrada em "Aplicativos instalados" (desinstalar pelo Windows).
pub fn register_uninstall(dir: &std::path::Path, exe: &std::path::Path, version: &str) {
    unsafe {
        let exe_s = exe.to_string_lossy().to_string();
        reg_sz(UNINSTALL_KEY, "DisplayName", "Line Quota");
        reg_sz(UNINSTALL_KEY, "DisplayVersion", version);
        reg_sz(UNINSTALL_KEY, "Publisher", "Line Quota");
        reg_sz(UNINSTALL_KEY, "DisplayIcon", &exe_s);
        reg_sz(UNINSTALL_KEY, "InstallLocation", &dir.to_string_lossy());
        reg_sz(UNINSTALL_KEY, "UninstallString", &format!("\"{exe_s}\" --uninstall"));
        reg_sz(UNINSTALL_KEY, "QuietUninstallString", &format!("\"{exe_s}\" --uninstall --quiet"));
        for n in ["NoModify", "NoRepair"] {
            let one = 1u32.to_le_bytes();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(wide0(UNINSTALL_KEY).as_ptr()), PCWSTR(wide0(n).as_ptr()),
                REG_DWORD.0, Some(one.as_ptr() as *const c_void), 4);
        }
    }
}

pub fn unregister_uninstall() {
    unsafe { let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide0(UNINSTALL_KEY).as_ptr())); }
}

/// Caixa de mensagem e pergunta Sim/Não (o app não tem console).
pub fn info_box(title: &str, text: &str) { message(title, text) }
pub fn ask(title: &str, text: &str) -> bool { ask_yes_no(title, text) }

/// Diálogo de confirmação do Windows (TaskDialog) com botões "<ok>" / "Cancelar" e uma caixa de marcação.
/// Retorna `Some(caixa marcada)` se o usuário confirmou e `None` se cancelou/fechou.
pub fn confirm_with_check(title: &str, heading: &str, text: &str, ok: &str, check: &str, check_default: bool) -> Option<bool> {
    use windows::Win32::UI::Controls::*;
    const ID_OK: i32 = 1001;
    const ID_CANCEL: i32 = 1002;
    let (t, h, c, okw, cancel, chk) = (wide0(title), wide0(heading), wide0(text), wide0(ok), wide0("Cancelar"), wide0(check));
    let buttons = [
        TASKDIALOG_BUTTON { nButtonID: ID_OK, pszButtonText: PCWSTR(okw.as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: ID_CANCEL, pszButtonText: PCWSTR(cancel.as_ptr()) },
    ];
    let mut cfg = TASKDIALOGCONFIG::default();
    cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
    cfg.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW
        | if check_default { TDF_VERIFICATION_FLAG_CHECKED } else { TASKDIALOG_FLAGS(0) };
    cfg.pszWindowTitle = PCWSTR(t.as_ptr());
    cfg.pszMainInstruction = PCWSTR(h.as_ptr());
    cfg.pszContent = PCWSTR(c.as_ptr());
    cfg.pszVerificationText = PCWSTR(chk.as_ptr());
    cfg.pButtons = buttons.as_ptr();
    cfg.cButtons = buttons.len() as u32;
    cfg.nDefaultButton = ID_OK;
    let (mut pressed, mut checked) = (0i32, BOOL(0));
    unsafe { TaskDialogIndirect(&cfg, Some(&mut pressed), None, Some(&mut checked)).ok()?; }
    (pressed == ID_OK).then_some(checked.as_bool())
}
