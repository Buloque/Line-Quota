//! Posicionamento do tile em relação à barra de tarefas e aos monitores (puro, sem Win32).
use lq_core::config::Anchor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect { pub l: i32, pub t: i32, pub r: i32, pub b: i32 }

impl Rect {
    pub fn new(l: i32, t: i32, r: i32, b: i32) -> Self { Self { l, t, r, b } }
    pub fn w(&self) -> i32 { self.r - self.l }
    pub fn h(&self) -> i32 { self.b - self.t }
    fn contains_loosely(&self, o: &Rect) -> bool {
        o.l >= self.l - 2 && o.r <= self.r + 2 && o.t >= self.t - 2 && o.b <= self.b + 2
    }
}

/// Posiciona o tile na barra. Funciona com barra embaixo, no topo (inclusive compacta),
/// à esquerda ou à direita. `fallback_gap` é a reserva do lado do relógio/bandeja quando a
/// geometria da bandeja não é confiável (ou não existe, como em monitores secundários).
pub fn place_tile(taskbar: Rect, tray: Option<Rect>, w: i32, h: i32, anchor: Anchor, gap: i32, fallback_gap: i32) -> Rect {
    let tray = tray.filter(|t| t.w() > 0 && t.h() > 0 && taskbar.contains_loosely(t));
    if taskbar.w() >= taskbar.h() {
        let (w, h) = (w.min(taskbar.w()), h.min(taskbar.h()));
        let y = taskbar.t + (taskbar.h() - h) / 2;
        let end = tray.map_or(taskbar.r - fallback_gap, |t| t.l - gap); // limite à direita livre
        let x = match anchor {
            Anchor::TaskbarStart => taskbar.l + gap,
            Anchor::TaskbarCenter => taskbar.l + (taskbar.w() - w) / 2,
            _ => end - w,
        };
        let x = x.min(end - w).max(taskbar.l);
        Rect::new(x, y, x + w, y + h)
    } else {
        let (w, h) = (w.min(taskbar.w()), h.min(taskbar.h()));
        let x = taskbar.l + (taskbar.w() - w) / 2;
        let end = tray.map_or(taskbar.b - fallback_gap, |t| t.t - gap); // limite inferior livre
        let y = match anchor {
            Anchor::TaskbarStart => taskbar.t + gap,
            Anchor::TaskbarCenter => taskbar.t + (taskbar.h() - h) / 2,
            _ => end - h,
        };
        let y = y.min(end - h).max(taskbar.t);
        Rect::new(x, y, x + w, y + h)
    }
}

/// Posição livre, mas presa ao eixo da barra: `offset` é a distância desde o início da barra
/// (x em barras horizontais, y em verticais). Limitada para não sair da barra nem cobrir a bandeja.
pub fn place_on_bar(taskbar: Rect, tray: Option<Rect>, w: i32, h: i32, offset: i32, gap: i32, fallback_gap: i32) -> Rect {
    let tray = tray.filter(|t| t.w() > 0 && t.h() > 0 && taskbar.contains_loosely(t));
    let (w, h) = (w.min(taskbar.w()), h.min(taskbar.h()));
    if taskbar.w() >= taskbar.h() {
        let max_l = (tray.map_or(taskbar.r - fallback_gap, |t| t.l - gap) - w).max(taskbar.l);
        let l = (taskbar.l + offset).clamp(taskbar.l, max_l);
        let t = taskbar.t + (taskbar.h() - h) / 2;
        Rect::new(l, t, l + w, t + h)
    } else {
        let max_t = (tray.map_or(taskbar.b - fallback_gap, |t| t.t - gap) - h).max(taskbar.t);
        let t = (taskbar.t + offset).clamp(taskbar.t, max_t);
        let l = taskbar.l + (taskbar.w() - w) / 2;
        Rect::new(l, t, l + w, t + h)
    }
}

/// Inverso de `place_on_bar`: distância do widget desde o início da barra, no eixo dela.
pub fn offset_of(taskbar: Rect, r: Rect) -> i32 {
    if taskbar.w() >= taskbar.h() { r.l - taskbar.l } else { r.t - taskbar.t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Anchor::*;

    #[test]
    fn bottom_taskbar_left_of_tray() {
        let tb = Rect::new(0, 1032, 1920, 1080);
        let tray = Rect::new(1600, 1032, 1920, 1080);
        assert_eq!(place_tile(tb, Some(tray), 200, 40, BesideTray, 4, 160), Rect::new(1396, 1036, 1596, 1076));
    }

    #[test]
    fn thin_top_bar_clamps_height() {
        let tb = Rect::new(0, 0, 1920, 30);
        let tray = Rect::new(1700, 0, 1920, 30);
        let r = place_tile(tb, Some(tray), 180, 48, BesideTray, 4, 160);
        assert_eq!((r.t, r.b, r.r), (0, 30, 1696));
    }

    #[test]
    fn invalid_tray_uses_fallback() {
        let tb = Rect::new(0, 0, 1920, 40);
        assert_eq!(place_tile(tb, Some(Rect::new(0, 0, 0, 0)), 100, 30, BesideTray, 4, 160).r, 1760);
        assert_eq!(place_tile(tb, None, 100, 30, BesideTray, 4, 160).r, 1760); // monitor secundário
    }

    #[test]
    fn start_and_center_anchors() {
        let tb = Rect::new(0, 0, 1920, 40);
        let tray = Rect::new(1700, 0, 1920, 40);
        assert_eq!(place_tile(tb, Some(tray), 100, 30, TaskbarStart, 4, 160).l, 4);
        assert_eq!(place_tile(tb, Some(tray), 100, 30, TaskbarCenter, 4, 160).l, 910);
    }

    #[test]
    fn center_never_overlaps_tray() {
        let tb = Rect::new(0, 0, 400, 40);
        let tray = Rect::new(300, 0, 400, 40);
        assert!(place_tile(tb, Some(tray), 200, 30, TaskbarCenter, 4, 160).r <= 296);
    }

    #[test]
    fn vertical_bar_stacks_above_tray() {
        let tb = Rect::new(0, 0, 60, 1080);
        let tray = Rect::new(0, 900, 60, 1080);
        let r = place_tile(tb, Some(tray), 120, 30, BesideTray, 4, 160);
        assert_eq!((r.w(), r.b), (60, 896));
    }

    #[test]
    fn on_bar_moves_only_along_the_bar() {
        let tb = Rect::new(0, 0, 1920, 32);
        let tray = Rect::new(1700, 0, 1920, 32);
        let r = place_on_bar(tb, Some(tray), 200, 22, 400, 4, 160);
        assert_eq!(r, Rect::new(400, 5, 600, 27)); // y fixo, centralizado na barra
        assert_eq!(offset_of(tb, r), 400);
    }

    #[test]
    fn on_bar_is_clamped_and_never_covers_tray() {
        let tb = Rect::new(0, 0, 1920, 32);
        let tray = Rect::new(1700, 0, 1920, 32);
        assert_eq!(place_on_bar(tb, Some(tray), 200, 22, 9999, 4, 160).r, 1696);
        assert_eq!(place_on_bar(tb, Some(tray), 200, 22, -50, 4, 160).l, 0);
    }

    #[test]
    fn on_bar_vertical_taskbar_moves_in_y() {
        let tb = Rect::new(0, 0, 60, 1080);
        let r = place_on_bar(tb, None, 120, 30, 300, 4, 160);
        assert_eq!((r.t, r.w()), (300, 60));
        assert_eq!(offset_of(tb, r), 300);
    }
}
