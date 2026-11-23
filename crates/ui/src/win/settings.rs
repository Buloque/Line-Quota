//! Janela de configurações (controles nativos do Windows). Cada controle reaproveita o ID de
//! comando do menu: o clique vira um WM_COMMAND para a janela do widget, que aplica e salva.
use super::*;

static SET_HWND: AtomicIsize = AtomicIsize::new(0);
static SET_FONT: AtomicIsize = AtomicIsize::new(0);
const ID_STATUS: usize = 900;
const ID_ORDER_LIST: usize = 910;
const ID_CLOSE: usize = 999;
const TIMER_SYNC: usize = 3;

/// Comboboxes: o ID do controle é a base do comando; o item escolhido soma ao ID.
const COMBOS: [usize; 9] = [ID_ACTSTYLE, ID_BARSTYLE, ID_DISPLAY, ID_LABELS, ID_FONT, ID_ANCHOR, ID_MONITOR, ID_THRESH, ID_CHATWIN];

unsafe fn mk(parent: HWND, class: PCWSTR, text: &str, style: u32, r: (i32, i32, i32, i32), id: usize) -> HWND {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let h = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR(wide0(text).as_ptr()),
        WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE, scale(r.0), scale(r.1), scale(r.2), scale(r.3),
        parent, HMENU(id as *mut c_void), hinst, None).unwrap_or_default();
    let font = SET_FONT.load(Ordering::Relaxed);
    if font != 0 { SendMessageW(h, WM_SETFONT, WPARAM(font as usize), LPARAM(1)); }
    h
}

const BS_CHECK: u32 = 0x3 | 0x10000; // AUTOCHECKBOX | WS_TABSTOP
const BS_PUSH: u32 = 0x10000;
const BS_GROUP: u32 = 0x7;
const CB_LIST: u32 = 0x3 | 0x200000 | 0x10000; // DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP

unsafe fn group(p: HWND, text: &str, x: i32, y: i32, w: i32, h: i32) { mk(p, w!("BUTTON"), text, BS_GROUP, (x, y, w, h), 0); }
unsafe fn check(p: HWND, text: &str, x: i32, y: i32, w: i32, id: usize) { mk(p, w!("BUTTON"), text, BS_CHECK, (x, y, w, 22), id); }
unsafe fn push(p: HWND, text: &str, x: i32, y: i32, w: i32, id: usize) { mk(p, w!("BUTTON"), text, BS_PUSH, (x, y, w, 26), id); }
unsafe fn label(p: HWND, text: &str, x: i32, y: i32, w: i32) { mk(p, w!("STATIC"), text, 0, (x, y + 4, w, 18), 0); }
unsafe fn combo(p: HWND, x: i32, y: i32, w: i32, id: usize, items: &[&str]) -> HWND {
    let h = mk(p, w!("COMBOBOX"), "", CB_LIST, (x, y, w, 200), id);
    for it in items { SendMessageW(h, CB_ADDSTRING, WPARAM(0), LPARAM(wide0(it).as_ptr() as isize)); }
    h
}

pub(super) unsafe fn open(tile: HWND) {
    let cur = SET_HWND.load(Ordering::Relaxed);
    if cur != 0 {
        let h = HWND(cur as *mut _);
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = SetForegroundWindow(h);
        return;
    }
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let class = w!("LineQuotaSettings");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hIcon: LoadIconW(hinst, PCWSTR(1 as *const u16)).unwrap_or_default(),
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut c_void),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let _ = tile;
    // Fonte da interface (Segoe UI 9 pt).
    let f = CreateFontW(-scale(12), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
    SET_FONT.store(f.0 as isize, Ordering::Relaxed);

    let (cw, ch) = (scale(648), scale(648));
    let mut rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let _ = AdjustWindowRectEx(&mut rc, style, false, WINDOW_EX_STYLE(0));
    let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
    let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Line Quota — Configurações"), style,
        ((sw - ww) / 2).max(0), ((sh - wh) / 2).max(0), ww, wh, None, None, hinst, None) else { return };
    SET_HWND.store(h.0 as isize, Ordering::Relaxed);

    // Coluna esquerda: aparência e alertas.
    let (lx, lw) = (12, 300);
    group(h, "Aparência", lx, 8, lw, 296);
    label(h, "Exibição", lx + 12, 26, 90);
    combo(h, lx + 110, 26, 176, ID_DISPLAY, &["Número", "Gráfico", "Gráfico + número"]);
    label(h, "Gráfico", lx + 12, 54, 90);
    combo(h, lx + 110, 54, 176, ID_BARSTYLE, &["Barra horizontal", "Barra vertical", "Círculo", "Blocos"]);
    label(h, "Identificação", lx + 12, 82, 90);
    combo(h, lx + 110, 82, 176, ID_LABELS, &["Ícone", "Texto (Cl / Cx)", "Nenhuma"]);
    label(h, "Fonte", lx + 12, 110, 90);
    let fc = combo(h, lx + 110, 110, 176, ID_FONT, &[]);
    for (i, n) in FONTS.iter().enumerate().filter(|(_, n)| font_installed(n)) {
        let idx = SendMessageW(fc, CB_ADDSTRING, WPARAM(0), LPARAM(wide0(n).as_ptr() as isize)).0;
        SendMessageW(fc, CB_SETITEMDATA, WPARAM(idx as usize), LPARAM(i as isize));
    }
    label(h, "Atividade", lx + 12, 138, 90);
    combo(h, lx + 110, 138, 176, ID_ACTSTYLE, &["Ponto grande", "Ponto pequeno"]);
    check(h, "Fundo transparente", lx + 12, 170, 270, ID_TRANSPARENT);
    check(h, "Mostrar rótulos (5h / 7d / 30d)", lx + 12, 192, 270, ID_TAGS);
    check(h, "Mostrar o que resta (em vez do usado)", lx + 12, 214, 270, ID_SHOW_REMAINING);
    check(h, "Limite de 5 horas", lx + 12, 236, 270, ID_SHOW_5H);
    check(h, "Limite semanal", lx + 12, 258, 270, ID_SHOW_WEEK);
    check(h, "Tarefa ativa / aguardando resposta", lx + 12, 280, 270, ID_SHOW_ACTIVITY);

    group(h, "Alertas", lx, 312, lw, 216);
    check(h, "Alertas ativados", lx + 12, 332, 270, ID_ALERT_ENABLED);
    check(h, "Mostrar notificações do Windows", lx + 12, 354, 270, ID_ALERT_TOAST);
    check(h, "Tocar som", lx + 12, 376, 270, ID_ALERT_SOUND);
    check(h, "Avisar quando o limite renovar", lx + 12, 398, 270, ID_ALERT_RESET);
    check(h, "Avisar quando a tarefa terminar", lx + 12, 420, 270, ID_ALERT_DONE);
    check(h, "Avisar quando precisar de resposta", lx + 12, 442, 270, ID_ALERT_INPUT);
    label(h, "Avisar acima de", lx + 12, 470, 100);
    let labels: Vec<String> = THRESHOLD_PRESETS.iter()
        .map(|p| p.iter().map(|t| format!("{t:.0}%")).collect::<Vec<_>>().join(" / ")).collect();
    combo(h, lx + 110, 470, 176, ID_THRESH, &labels.iter().map(String::as_str).collect::<Vec<_>>());
    push(h, "Testar notificação", lx + 12, 496, 150, ID_ALERT_TEST);

    // Coluna direita: provedores, conta, posição, sistema.
    let (rx, rw) = (324, 312);
    // Um provedor por linha; o que é só dele (cookie do Cursor, cotas do Antigravity) fica à direita, na mesma linha.
    group(h, "Provedores", rx, 8, rw, 198);
    check(h, "Claude Code", rx + 12, 26, 285, ID_PROVIDER);
    check(h, "Codex", rx + 12, 48, 285, ID_PROVIDER + 1);
    check(h, "OpenCode", rx + 12, 70, 285, ID_PROVIDER + 2);
    check(h, "Cursor", rx + 12, 96, 100, ID_PROVIDER + 3);
    push(h, "Colar cookie", rx + 120, 94, 108, ID_CURSOR_PASTE);
    push(h, "Limpar", rx + 234, 94, 66, ID_CURSOR_CLEAR);
    check(h, "Antigravity", rx + 12, 124, 100, ID_PROVIDER + 4);
    push(h, "Escolher cotas  ▾", rx + 120, 122, 180, ID_AG_MENU);
    check(h, "Modo automático (só o que está aberto)", rx + 12, 158, 285, ID_AUTO);
    check(h, "Ocultar o widget se nada estiver aberto", rx + 28, 180, 270, ID_AUTO_HIDE);

    group(h, "Conta Claude", rx, 214, rw, 104);
    mk(h, w!("STATIC"), "", 0, (rx + 12, 234, rw - 24, 18), ID_STATUS);
    push(h, "Entrar (abre o navegador)", rx + 12, 256, 180, ID_CLAUDE_LOGIN);
    push(h, "Colar código", rx + 200, 256, 100, ID_CLAUDE_PASTE);
    push(h, "Sair da conta Claude", rx + 12, 286, 180, ID_CLAUDE_LOGOUT);

    group(h, "Posição", rx, 326, rw, 116);
    label(h, "Na barra", rx + 12, 344, 90);
    combo(h, rx + 110, 344, 190, ID_ANCHOR, &["Ao lado da bandeja", "Início da barra", "Centro da barra", "Livre (arrastar)"]);
    label(h, "Monitor", rx + 12, 372, 90);
    let mc = combo(h, rx + 110, 372, 190, ID_MONITOR, &["Automático"]);
    for (i, m) in monitors().iter().enumerate() {
        let n = format!("Monitor {}{}", i + 1, if m.primary { " (principal)" } else { "" });
        SendMessageW(mc, CB_ADDSTRING, WPARAM(0), LPARAM(wide0(&n).as_ptr() as isize));
    }
    check(h, "Travar a posição na barra", rx + 12, 406, 285, ID_LOCK);

    group(h, "Sistema", rx, 450, rw, 128);
    check(h, "Iniciar com o Windows", rx + 12, 468, 285, ID_AUTOSTART);
    push(h, "Atualizar agora", rx + 12, 494, 140, ID_REFRESH);
    push(h, "Abrir config.toml", rx + 160, 494, 140, ID_OPEN_CONFIG);
    push(h, "Recarregar do arquivo", rx + 12, 524, 140, ID_RELOAD);
    label(h, "Chats no painel: últimos", rx + 12, 550, 150);
    let wl: Vec<String> = CHAT_WINDOWS.iter().map(|m| crate::view::fmt_window(*m)).collect();
    combo(h, rx + 170, 550, 120, ID_CHATWIN, &wl.iter().map(String::as_str).collect::<Vec<_>>());

    push(h, "Fechar", rx + rw - 100, 588, 100, ID_CLOSE);

    // Ordem dos provedores na barra: escolhe um na lista e sobe/desce.
    group(h, "Ordem na barra", lx, 536, lw, 100);
    mk(h, w!("LISTBOX"), "", 0x1 | 0x100 | 0x10000 | 0x800000, (lx + 12, 556, 170, 78), ID_ORDER_LIST); // NOTIFY | NOINTEGRALHEIGHT | TABSTOP | BORDER
    if let Ok(lb) = GetDlgItem(h, ID_ORDER_LIST as i32) { SendMessageW(lb, LB_SETITEMHEIGHT, WPARAM(0), LPARAM(scale(15) as isize)); }
    push(h, "▲  Subir", lx + 192, 556, 96, ID_ORDER_UP);
    push(h, "▼  Descer", lx + 192, 596, 96, ID_ORDER_DOWN);

    apply_theme(h);
    sync(h);
    SetTimer(h, TIMER_SYNC, 2000, None);
    let _ = ShowWindow(h, SW_SHOW);
    let _ = SetForegroundWindow(h);
}

pub(super) unsafe fn sync_if_open() {
    let h = SET_HWND.load(Ordering::Relaxed);
    if h != 0 { sync(HWND(h as *mut _)); }
}

unsafe fn set_check(h: HWND, id: usize, on: bool) {
    if let Ok(c) = GetDlgItem(h, id as i32) { SendMessageW(c, BM_SETCHECK, WPARAM(on as usize), LPARAM(0)); }
}

unsafe fn set_sel(h: HWND, id: usize, idx: usize) {
    if let Ok(c) = GetDlgItem(h, id as i32) { SendMessageW(c, CB_SETCURSEL, WPARAM(idx), LPARAM(0)); }
}

static ORDER_SHOWN: std::sync::Mutex<Vec<ProviderId>> = std::sync::Mutex::new(Vec::new());
/// Seleção a restaurar depois de subir/descer (o item acompanha o provedor movido).
static ORDER_PENDING: AtomicIsize = AtomicIsize::new(-1);

/// Mantém a lista "Ordem na barra" igual à configuração (só reconstrói quando mudou, preservando a seleção).
unsafe fn refresh_order_list(h: HWND, cfg: &Config) {
    let Ok(lb) = GetDlgItem(h, ID_ORDER_LIST as i32) else { return };
    let order = cfg.ordered_providers();
    let mut shown = ORDER_SHOWN.lock().unwrap();
    if *shown == order { return; }
    let pending = ORDER_PENDING.swap(-1, Ordering::Relaxed);
    let cur = if pending >= 0 { pending } else { SendMessageW(lb, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    SendMessageW(lb, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
    for id in &order {
        let name = format!("{}. {}", order.iter().position(|o| o == id).unwrap_or(0) + 1, crate::view::full_name(*id));
        SendMessageW(lb, LB_ADDSTRING, WPARAM(0), LPARAM(wide0(&name).as_ptr() as isize));
    }
    if cur >= 0 { SendMessageW(lb, LB_SETCURSEL, WPARAM(cur as usize), LPARAM(0)); }
    *shown = order;
}

/// Reflete a configuração atual nos controles.
unsafe fn sync(h: HWND) {
    let Some(ctx) = CTX.get() else { return };
    let cfg = ctx.shared.config.lock().unwrap().clone();
    let (u, a) = (&cfg.ui, &cfg.alerts);
    for (id, on) in [
        (ID_TRANSPARENT, u.transparent), (ID_SHOW_REMAINING, u.show_remaining),
        (ID_SHOW_5H, cfg.claude.show_five_hour), (ID_SHOW_WEEK, cfg.claude.show_weekly), (ID_TAGS, u.show_tags), (ID_LOCK, u.locked), (ID_SHOW_ACTIVITY, cfg.claude.show_activity),
        (ID_ALERT_ENABLED, a.enabled), (ID_ALERT_TOAST, a.toast), (ID_ALERT_SOUND, a.sound),
        (ID_ALERT_RESET, a.notify_reset), (ID_ALERT_DONE, a.notify_task_done), (ID_ALERT_INPUT, a.notify_needs_input),
        (ID_PROVIDER, cfg.claude.enabled), (ID_PROVIDER + 1, cfg.codex.enabled), (ID_PROVIDER + 2, cfg.opencode.enabled), (ID_PROVIDER + 3, cfg.cursor.enabled), (ID_PROVIDER + 4, cfg.antigravity.enabled),
        (ID_AUTO, u.auto_mode), (ID_AUTO_HIDE, u.auto_hide_when_none),
        (ID_AUTOSTART, autostart_enabled()),
    ] { set_check(h, id, on); }
    set_sel(h, ID_DISPLAY, match u.display { DisplayMode::Number => 0, DisplayMode::Bar => 1, DisplayMode::Both => 2 });
    set_sel(h, ID_BARSTYLE, match u.bar_style { BarStyle::Horizontal => 0, BarStyle::Vertical => 1, BarStyle::Ring => 2, BarStyle::Blocks => 3 });
    set_sel(h, ID_ACTSTYLE, if u.activity_style == ActivityStyle::SmallDot { 1 } else { 0 });
    set_sel(h, ID_LABELS, match u.labels { LabelStyle::Icon => 0, LabelStyle::Text => 1, LabelStyle::None => 2 });
    set_sel(h, ID_ANCHOR, match u.anchor { Anchor::BesideTray => 0, Anchor::TaskbarStart => 1, Anchor::TaskbarCenter => 2, Anchor::Custom => 3 });
    set_sel(h, ID_MONITOR, u.monitor as usize);
    set_sel(h, ID_THRESH, THRESHOLD_PRESETS.iter().position(|p| a.thresholds == *p).unwrap_or(usize::MAX));
    set_sel(h, ID_CHATWIN, CHAT_WINDOWS.iter().position(|m| *m == u.chat_window_mins).unwrap_or(usize::MAX));
    if let Ok(fc) = GetDlgItem(h, ID_FONT as i32) {
        let n = SendMessageW(fc, CB_GETCOUNT, WPARAM(0), LPARAM(0)).0;
        for i in 0..n {
            let d = SendMessageW(fc, CB_GETITEMDATA, WPARAM(i as usize), LPARAM(0)).0 as usize;
            if FONTS.get(d).map_or(false, |f| *f == u.font) { SendMessageW(fc, CB_SETCURSEL, WPARAM(i as usize), LPARAM(0)); }
        }
    }
    let status = if ctx.shared.account.logged_in() { "Claude: conectado" } else { "Claude: não conectado" };
    if let Ok(s) = GetDlgItem(h, ID_STATUS as i32) { let _ = SetWindowTextW(s, PCWSTR(wide0(status).as_ptr())); }
    refresh_order_list(h, &cfg);
    let cursor_txt = if ctx.shared.account.cursor_connected() { "Trocar cookie" } else { "Colar cookie" };
    if let Ok(s) = GetDlgItem(h, ID_CURSOR_PASTE as i32) { let _ = SetWindowTextW(s, PCWSTR(wide0(cursor_txt).as_ptr())); }
}

/// Menu de cotas do Antigravity (marcas): escolhe quais mostrar e se vão juntas num gráfico só.
unsafe fn antigravity_menu(parent: HWND, button: HWND, tile: HWND) {
    let Some(ctx) = CTX.get() else { return };
    let v = ctx.shared.config.lock().unwrap().antigravity_view.clone();
    let Ok(menu) = CreatePopupMenu() else { return };
    let item = |id: usize, text: &str, on: bool| {
        let flags = MF_STRING | if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, flags, id, PCWSTR(wide0(text).as_ptr()));
    };
    item(ID_AG_GEMINI, "Gemini (semanal)", v.gemini);
    item(ID_AG_CLAUDE_WEEKLY, "Claude e GPT (semanal)", v.claude_weekly);
    item(ID_AG_CLAUDE_5H, "Claude e GPT (5 horas)", v.claude_5h);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    item(ID_AG_COMBINED, "Juntar num gráfico só", v.combined);
    let mut r = RECT::default();
    let _ = GetWindowRect(button, &mut r);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, r.left, r.bottom, 0, parent, None).0 as usize;
    let _ = DestroyMenu(menu);
    if cmd != 0 { SendMessageW(tile, WM_COMMAND, WPARAM(cmd), LPARAM(0)); sync(parent); }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let (id, code) = (wp.0 & 0xFFFF, (wp.0 >> 16) & 0xFFFF);
            let tile = CTX.get().map(|c| HWND(c.shared.hwnd.load(Ordering::Relaxed) as *mut _)).unwrap_or_default();
            if id == ID_CLOSE { let _ = DestroyWindow(hwnd); return LRESULT(0); }
            if id == ID_AG_MENU && code == 0 { antigravity_menu(hwnd, HWND(lp.0 as *mut _), tile); return LRESULT(0); }
            if id == ID_ORDER_UP || id == ID_ORDER_DOWN {
                if code != 0 { return LRESULT(0); }
                let sel = GetDlgItem(hwnd, ID_ORDER_LIST as i32).map_or(-1, |lb| SendMessageW(lb, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0);
                if sel < 0 { return LRESULT(0); } // nada selecionado
                ORDER_SEL.store(sel, Ordering::Relaxed);
                ORDER_PENDING.store(sel + if id == ID_ORDER_UP { -1 } else { 1 }, Ordering::Relaxed);
                SendMessageW(tile, WM_COMMAND, WPARAM(id), LPARAM(0));
                sync(hwnd);
                ORDER_PENDING.store(-1, Ordering::Relaxed); // no limite (já primeiro/último) nada mudou: não deixa sobra
                return LRESULT(0);
            }
            if id == ID_ORDER_LIST { return LRESULT(0); } // clique na lista: só seleciona
            let cmd = if COMBOS.contains(&id) {
                if code != 1 { return LRESULT(0); } // só CBN_SELCHANGE
                let sel = SendMessageW(HWND(lp.0 as *mut _), CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                if sel < 0 { return LRESULT(0); }
                let sel = sel as usize;
                match id {
                    ID_FONT => SendMessageW(HWND(lp.0 as *mut _), CB_GETITEMDATA, WPARAM(sel), LPARAM(0)).0 as usize + ID_FONT,
                    _ => id + sel,
                }
            } else {
                if code != 0 { return LRESULT(0); } // só BN_CLICKED
                id
            };
            SendMessageW(tile, WM_COMMAND, WPARAM(cmd), LPARAM(0));
            sync(hwnd);
            LRESULT(0)
        }
        WM_TIMER | WM_SETTINGCHANGE => {
            if apps_dark() != DARK.load(Ordering::Relaxed) { apply_theme(hwnd); }
            sync(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            fill(HDC(wp.0 as *mut c_void), &rc, bg_color(DARK.load(Ordering::Relaxed)));
            LRESULT(1)
        }
        WM_CTLCOLORLISTBOX => {
            let dark = DARK.load(Ordering::Relaxed);
            let hdc = HDC(wp.0 as *mut c_void);
            SetTextColor(hdc, fg_color(dark));
            SetBkColor(hdc, bg_color(dark));
            LRESULT(BG_BRUSH.load(Ordering::Relaxed))
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORDLG => {
            let dark = DARK.load(Ordering::Relaxed);
            let hdc = HDC(wp.0 as *mut c_void);
            SetTextColor(hdc, fg_color(dark));
            SetBkColor(hdc, bg_color(dark));
            LRESULT(BG_BRUSH.load(Ordering::Relaxed))
        }
        WM_CLOSE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_SYNC);
            SET_HWND.store(0, Ordering::Relaxed);
            let f = SET_FONT.swap(0, Ordering::Relaxed);
            if f != 0 { let _ = DeleteObject(HGDIOBJ(f as *mut c_void)); }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------- Tema (claro/escuro do Windows) ----------

static DARK: AtomicBool = AtomicBool::new(false);
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);

/// true se os apps do Windows usam o tema escuro.
fn apps_dark() -> bool {
    let mut v: u32 = 1;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut c_void), Some(&mut size))
    };
    r.0 == 0 && v == 0
}

fn bg_color(dark: bool) -> COLORREF { COLORREF(if dark { 0x00202020 } else { 0x00F3F3F3 }) }
fn fg_color(dark: bool) -> COLORREF { COLORREF(if dark { 0x00F0F0F0 } else { 0x00101010 }) }

unsafe extern "system" fn theme_child(h: HWND, lp: LPARAM) -> BOOL {
    use windows::Win32::UI::Controls::SetWindowTheme;
    let mut cls = [0u16; 16];
    let n = GetClassNameW(h, &mut cls) as usize;
    let name = String::from_utf16_lossy(&cls[..n]).to_lowercase();
    let dark = lp.0 != 0;
    // Título de grupo: com tema visual o texto ignora a cor; sem tema, respeita WM_CTLCOLORSTATIC.
    let is_group = name == "button" && (GetWindowLongW(h, GWL_STYLE) as u32 & 0xF) == 7;
    let theme = match (dark, name.as_str()) {
        (true, _) if is_group => w!(""),
        (true, "combobox") => w!("DarkMode_CFD"),
        (true, _) => w!("DarkMode_Explorer"),
        (false, _) => w!("Explorer"),
    };
    let _ = SetWindowTheme(h, theme, if is_group && dark { w!("") } else { PCWSTR::null() });
    TRUE
}

/// Aplica claro/escuro: barra de título, controles e fundo.
unsafe fn apply_theme(h: HWND) {
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
    let dark = apps_dark();
    DARK.store(dark, Ordering::Relaxed);
    let b = set_bg_brush(bg_color(dark));
    let _ = b;
    let flag = BOOL::from(dark);
    let _ = DwmSetWindowAttribute(h, DWMWINDOWATTRIBUTE(20), &flag as *const BOOL as *const c_void, 4);
    let _ = EnumChildWindows(h, Some(theme_child), LPARAM(dark as isize));
    let _ = RedrawWindow(h, None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME);
}

unsafe fn set_bg_brush(c: COLORREF) -> HBRUSH {
    let nb = CreateSolidBrush(c);
    let old = BG_BRUSH.swap(nb.0 as isize, Ordering::Relaxed);
    if old != 0 { let _ = DeleteObject(HGDIOBJ(old as *mut c_void)); }
    nb
}
