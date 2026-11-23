//! Janela do passo a passo do cookie do Cursor: mostra o nome do cookie (com botão para copiar), uma contagem
//! regressiva de 3 minutos e detecta sozinha o valor copiado do navegador. O valor nunca é mostrado.
use super::*;
use std::sync::Mutex;
use std::sync::atomic::AtomicI32;

const ID_COPY_NAME: usize = 101;
const ID_CANCEL: usize = 102;
const ID_COUNT: usize = 103;
const TIMER: usize = 9;
const TOTAL_SECS: i32 = 180;
const COOKIE_NAME: &str = "WorkosCursorSessionToken";

static LEFT: AtomicI32 = AtomicI32::new(0);
static FOUND: Mutex<Option<String>> = Mutex::new(None);

unsafe fn child(parent: HWND, class: PCWSTR, text: &str, style: u32, r: (i32, i32, i32, i32), id: usize, font: HFONT) -> HWND {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let h = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR(wide0(text).as_ptr()), WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
        scale(r.0), scale(r.1), scale(r.2), scale(r.3), parent, HMENU(id as *mut c_void), hinst, None).unwrap_or_default();
    SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    h
}

fn count_text(secs: i32) -> String { format!("Aguardando você copiar o cookie… {}:{:02}", secs / 60, secs % 60) }

/// Mostra a janela (bloqueia esta thread até fechar) e devolve o texto copiado que parece o cookie, se houve.
pub(super) fn run() -> Option<String> {
    unsafe {
        *FOUND.lock().unwrap() = None;
        LEFT.store(TOTAL_SECS, Ordering::Relaxed);
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("LineQuotaCookieGuide");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(hinst, PCWSTR(1 as *const u16)).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut c_void),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let font = CreateFontW(-scale(12), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
        let (cw, ch) = (scale(440), scale(268));
        let mut rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let _ = AdjustWindowRectEx(&mut rc, style, false, WS_EX_TOPMOST);
        let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let sw = GetSystemMetrics(SM_CXSCREEN);
        // No canto direito: o navegador, ao lado, continua visível; fica por cima dele.
        let Ok(h) = CreateWindowExW(WS_EX_TOPMOST, class, w!("Cursor — copiar o cookie do limite"), style,
            (sw - ww - scale(24)).max(0), scale(80), ww, wh, None, None, hinst, None) else { let _ = DeleteObject(HGDIOBJ(font.0)); return None };
        let steps = "O painel do Cursor abriu no navegador (entre na conta, se pedir). Depois:\r\n\r\n\
            1) Aperte F12 (ou Ctrl+Shift+I) para abrir o DevTools.\r\n\
            2) Aba Application (Aplicativo) > Cookies > https://cursor.com.\r\n\
            3) Na linha com o nome abaixo, dê dois cliques na coluna Value e copie (Ctrl+C).";
        child(h, w!("STATIC"), steps, 0, (16, 12, 408, 100), 0, font);
        child(h, w!("EDIT"), COOKIE_NAME, 0x800 | 0x80 | 0x10000, (16, 122, 262, 26), 0, font); // ES_READONLY | ES_AUTOHSCROLL
        child(h, w!("BUTTON"), "Copiar o nome", 0x10000, (288, 121, 136, 28), ID_COPY_NAME, font);
        child(h, w!("STATIC"), &count_text(TOTAL_SECS), 0, (16, 166, 408, 20), ID_COUNT, font);
        child(h, w!("STATIC"), "O valor é detectado sozinho, guardado criptografado e a área de transferência é limpa em seguida.", 0, (16, 190, 408, 34), 0, font);
        child(h, w!("BUTTON"), "Cancelar", 0x10000, (324, 230, 100, 28), ID_CANCEL, font);
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
        SetTimer(h, TIMER, 1000, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if !IsDialogMessageW(h, &msg).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
        }
        let _ = DeleteObject(HGDIOBJ(font.0));
        FOUND.lock().unwrap().take()
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            match wp.0 & 0xFFFF {
                ID_COPY_NAME => write_clipboard(COOKIE_NAME),
                ID_CANCEL => { let _ = DestroyWindow(hwnd); }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            let clip = read_clipboard();
            if CTX.get().is_some_and(|c| c.shared.account.cursor_looks_like_cookie(&clip)) {
                *FOUND.lock().unwrap() = Some(clip);
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            let left = LEFT.fetch_sub(1, Ordering::Relaxed) - 1;
            if left <= 0 { let _ = DestroyWindow(hwnd); return LRESULT(0); }
            if let Ok(c) = GetDlgItem(hwnd, ID_COUNT as i32) { let _ = SetWindowTextW(c, PCWSTR(wide0(&count_text(left)).as_ptr())); }
            LRESULT(0)
        }
        WM_CLOSE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_DESTROY => { let _ = KillTimer(hwnd, TIMER); PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
