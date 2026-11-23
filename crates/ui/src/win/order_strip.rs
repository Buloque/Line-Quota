//! Faixa "Ordem na barra": os ícones dos provedores, na ordem atual, que o usuário arrasta na horizontal
//! para onde quiser. Janela própria desenhada à mão (sem flicker); ao soltar, a ordem é aplicada e salva.
use super::settings::{bg_color, fg_color, DARK};
use super::*;
use std::sync::Mutex;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TRACKMOUSEEVENT, TME_LEAVE};

const TILE_W: i32 = 56;
const TILE_H: i32 = 68;
const ICON: i32 = 26;

/// Arraste em andamento: (posição de origem, x do mouse dentro da faixa, deslocamento do clique dentro do ícone).
static DRAG: Mutex<Option<(usize, i32, i32)>> = Mutex::new(None);
static HOVER: AtomicIsize = AtomicIsize::new(-1);

pub(super) unsafe fn create(parent: HWND, x: i32, y: i32) -> HWND {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let class = w!("LineQuotaOrderStrip");
    let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
    RegisterClassW(&wc);
    CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WS_CHILD | WS_VISIBLE, scale(x), scale(y), scale(TILE_W * 5), scale(TILE_H),
        parent, HMENU(ID_ORDER_STRIP as *mut c_void), hinst, None).unwrap_or_default()
}

/// Redesenha (a configuração pode ter mudado: ordem, provedores ligados, tema).
pub(super) unsafe fn refresh(parent: HWND) {
    if let Ok(s) = GetDlgItem(parent, ID_ORDER_STRIP as i32) { let _ = InvalidateRect(s, None, false); }
}

/// Ordem a mostrar: a da configuração; durante o arraste, com o item já na posição de destino.
fn shown_order(drag: Option<(usize, i32, i32)>) -> (Vec<ProviderId>, Option<usize>) {
    let Some(ctx) = CTX.get() else { return (ProviderId::ALL.to_vec(), None) };
    let mut order = ctx.shared.config.lock().unwrap().ordered_providers();
    let Some((from, mx, _)) = drag else { return (order, None) };
    let to = ((mx / scale(TILE_W)).max(0) as usize).min(order.len() - 1);
    let moved = order.remove(from.min(order.len() - 1));
    order.insert(to, moved);
    (order, Some(to))
}

unsafe fn tile_at(x: i32) -> usize { ((x / scale(TILE_W)).max(0) as usize).min(4) }

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right, rc.bottom);
    let mem = CreateCompatibleDC(dc);
    let bmp = CreateCompatibleBitmap(dc, w, h);
    let old_bmp = SelectObject(mem, bmp);
    let dark = DARK.load(Ordering::Relaxed);
    fill(mem, &rc, bg_color(dark));
    let font = CreateFontW(-scale(9), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
    let old_font = SelectObject(mem, font);
    SetBkMode(mem, TRANSPARENT);
    let drag = *DRAG.lock().unwrap();
    let (order, target) = shown_order(drag);
    let hover = HOVER.load(Ordering::Relaxed);
    let cfg = CTX.get().map(|c| c.shared.config.lock().unwrap().clone());
    let card = if dark { COLORREF(0x0036_3636) } else { COLORREF(0x00E6_E6E6) };
    let accent = COLORREF(0x00F0_A050); // azul de destaque (BGR)
    let tw = scale(TILE_W);
    for (i, id) in order.iter().enumerate() {
        let dragging = target == Some(i);
        let x = i as i32 * tw;
        let tile = RECT { left: x + scale(3), top: scale(3), right: x + tw - scale(3), bottom: h - scale(3) };
        if dragging || hover == i as isize {
            let b = CreateSolidBrush(if dragging { accent } else { card });
            let p = CreatePen(PS_SOLID, 1, if dragging { accent } else { card });
            let (ob, op) = (SelectObject(mem, b), SelectObject(mem, p));
            let _ = RoundRect(mem, tile.left, tile.top, tile.right, tile.bottom, scale(8), scale(8));
            SelectObject(mem, ob); SelectObject(mem, op);
            let _ = DeleteObject(b); let _ = DeleteObject(p);
        }
        let on = cfg.as_ref().is_none_or(|c| c.provider(*id).enabled);
        // Ícone no tema certo; desligados ficam apagados (o texto, em cinza).
        let isz = scale(ICON);
        draw_icon(mem, *id, x + (tw - isz) / 2, scale(8) + isz / 2, isz, !dark);
        if !on {
            // véu: mistura o fundo por cima do ícone (aspecto "desligado")
            let veil = RECT { left: x + (tw - isz) / 2 - 1, top: scale(8) - 1, right: x + (tw + isz) / 2 + 1, bottom: scale(8) + isz + 1 };
            draw_veil(mem, &veil, bg_color(dark));
        }
        let name = wide(crate::view::full_name(*id));
        let mut r = RECT { left: x, top: scale(8) + isz + scale(4), right: x + tw, bottom: h };
        SetTextColor(mem, if dragging { COLORREF(0x0010_1010) } else if on { fg_color(dark) } else { COLORREF(0x0080_8080) });
        DrawTextW(mem, &mut wide(crate::view::full_name(*id)), &mut r, DT_CENTER | DT_TOP | DT_SINGLELINE | DT_NOPREFIX);
        let _ = name;
    }
    let _ = BitBlt(dc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_font); let _ = DeleteObject(font);
    SelectObject(mem, old_bmp); let _ = DeleteObject(bmp); let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

/// Escurece/apaga um retângulo misturando 60% do fundo por cima (ícone de provedor desligado).
unsafe fn draw_veil(hdc: HDC, r: &RECT, bg: COLORREF) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 { return; }
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old = SelectObject(mem, bmp);
    fill(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, bg);
    let bf = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 150, AlphaFormat: 0 };
    let _ = windows::Win32::Graphics::Gdi::GdiAlphaBlend(hdc, r.left, r.top, w, h, mem, 0, 0, w, h, bf);
    SelectObject(mem, old); let _ = DeleteObject(bmp); let _ = DeleteDC(mem);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    match msg {
        WM_PAINT => { paint(hwnd); LRESULT(0) }
        WM_ERASEBKGND => LRESULT(1),
        WM_SETCURSOR => {
            let _ = SetCursor(LoadCursorW(None, if DRAG.lock().unwrap().is_some() { IDC_SIZEWE } else { IDC_HAND }).unwrap_or_default());
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            let from = tile_at(x);
            *DRAG.lock().unwrap() = Some((from, x, x - from as i32 * scale(TILE_W)));
            SetCapture(hwnd);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut d = DRAG.lock().unwrap();
            if let Some((from, _, off)) = *d { *d = Some((from, x, off)); } else if HOVER.swap(tile_at(x) as isize, Ordering::Relaxed) == tile_at(x) as isize { return LRESULT(0); }
            drop(d);
            // para o WM_MOUSELEAVE
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = TrackMouseEvent(&mut t);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        0x02A3 /* WM_MOUSELEAVE */ => { HOVER.store(-1, Ordering::Relaxed); let _ = InvalidateRect(hwnd, None, false); LRESULT(0) }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            let taken = DRAG.lock().unwrap().take();
            if msg == WM_LBUTTONUP { let _ = ReleaseCapture(); }
            if let (Some((from, mx, _)), true) = (taken, msg == WM_LBUTTONUP) {
                let to = tile_at(mx);
                if to != from { ORDER_FROM.store(from as isize, Ordering::Relaxed); ORDER_TO.store(to as isize, Ordering::Relaxed); apply_order(hwnd); }
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Manda ao widget o pedido de mover (ele aplica na configuração e salva).
unsafe fn apply_order(strip: HWND) {
    let Some(ctx) = CTX.get() else { return };
    let tile = HWND(ctx.shared.hwnd.load(Ordering::Relaxed) as *mut _);
    SendMessageW(tile, WM_COMMAND, WPARAM(ID_ORDER_MOVE), LPARAM(0));
    let _ = InvalidateRect(strip, None, false);
}
