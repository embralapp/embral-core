//! Repairs the main window's geometry after a monitor change.
//!
//! `tauri-plugin-window-state` validates a restored position against the
//! connected monitors but applies the saved size unconditionally: in
//! physical pixels, with no scale factor recorded. A size saved on a
//! 100%-scale external monitor and restored onto a 150–200% laptop panel
//! ends up far below the app minimum, and because closing only hides the
//! window, every later show reuses the broken geometry. The math is pure
//! and unit-tested here; the real monitor unplug can't run in CI.

/// Logical floor from tauri.conf.json's `minWidth`/`minHeight`.
const MIN_LOGICAL: (f64, f64) = (840.0, 560.0);
/// The rescue target: the default window size, shrunk to fit the monitor.
const DEFAULT_LOGICAL: (f64, f64) = (1100.0, 720.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectPx {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorPx {
    pub rect: RectPx,
    pub scale: f64,
}

fn intersection_area(a: RectPx, b: RectPx) -> u64 {
    let left = i64::from(a.x).max(i64::from(b.x));
    let top = i64::from(a.y).max(i64::from(b.y));
    let right = (i64::from(a.x) + i64::from(a.w)).min(i64::from(b.x) + i64::from(b.w));
    let bottom = (i64::from(a.y) + i64::from(a.h)).min(i64::from(b.y) + i64::from(b.h));
    ((right - left).max(0) as u64) * ((bottom - top).max(0) as u64)
}

/// The corrected rect when the window is undersized, oversized, or mostly
/// off-screen for every connected monitor; `None` when the geometry is
/// sane, so callers never move a healthy window.
pub fn rescue(window: RectPx, monitors: &[MonitorPx]) -> Option<RectPx> {
    let first = monitors.first()?;
    // The monitor the window mostly lives on; the glue puts the current
    // monitor first, so a fully off-screen window falls back to it.
    let target = monitors
        .iter()
        .max_by_key(|m| intersection_area(window, m.rect))
        .filter(|m| intersection_area(window, m.rect) > 0)
        .unwrap_or(first);

    let scale = target.scale;
    let undersized = (f64::from(window.w) / scale) + 0.5 < MIN_LOGICAL.0
        || (f64::from(window.h) / scale) + 0.5 < MIN_LOGICAL.1;
    let oversized = window.w > target.rect.w || window.h > target.rect.h;
    // Half the window visible keeps it reachable; less counts as lost.
    let off_screen =
        intersection_area(window, target.rect) * 2 < u64::from(window.w) * u64::from(window.h);
    if !undersized && !oversized && !off_screen {
        return None;
    }

    let w = (DEFAULT_LOGICAL.0.min(f64::from(target.rect.w) / scale) * scale).round() as u32;
    let h = (DEFAULT_LOGICAL.1.min(f64::from(target.rect.h) / scale) * scale).round() as u32;
    Some(RectPx {
        x: target.rect.x + ((i64::from(target.rect.w) - i64::from(w)) / 2) as i32,
        y: target.rect.y + ((i64::from(target.rect.h) - i64::from(h)) / 2) as i32,
        w,
        h,
    })
}

/// Repair the window's geometry if a monitor change left it off-screen or
/// undersized. Called on every path that surfaces the main window.
pub fn ensure_on_screen(window: &tauri::WebviewWindow) {
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        return;
    };
    let Ok(available) = window.available_monitors() else {
        return;
    };
    let as_px = |m: &tauri::Monitor| MonitorPx {
        rect: RectPx {
            x: m.position().x,
            y: m.position().y,
            w: m.size().width,
            h: m.size().height,
        },
        scale: m.scale_factor(),
    };
    let mut monitors: Vec<MonitorPx> = available.iter().map(as_px).collect();
    // Put the monitor the OS considers current first: it becomes the
    // fallback anchor when the window intersects nothing.
    if let Ok(Some(current)) = window.current_monitor() {
        let cur = as_px(&current);
        if let Some(i) = monitors.iter().position(|m| m.rect == cur.rect) {
            monitors.swap(0, i);
        }
    }
    let rect = RectPx {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
    };
    if let Some(fix) = rescue(rect, &monitors) {
        let _ = window.set_size(tauri::PhysicalSize::new(fix.w, fix.h));
        let _ = window.set_position(tauri::PhysicalPosition::new(fix.x, fix.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP_2X: MonitorPx = MonitorPx {
        rect: RectPx { x: 0, y: 0, w: 2880, h: 1920 },
        scale: 2.0,
    };
    const DESKTOP_1X: MonitorPx = MonitorPx {
        rect: RectPx { x: 0, y: 0, w: 1920, h: 1080 },
        scale: 1.0,
    };

    #[test]
    fn a_cross_dpi_restore_grows_back_to_the_default_size() {
        // The bug: 1100×720 physical saved at 100% scale is 550×360 logical
        // on a 200% laptop panel, far under the 840×560 minimum.
        let fixed = rescue(RectPx { x: 0, y: 0, w: 1100, h: 720 }, &[LAPTOP_2X]).unwrap();
        assert_eq!((fixed.w, fixed.h), (2200, 1440)); // 1100×720 logical
        assert_eq!((fixed.x, fixed.y), (340, 240)); // centered
    }

    #[test]
    fn an_off_screen_window_recenters_on_the_current_monitor() {
        // Position saved on a monitor that no longer exists.
        let window = RectPx { x: 3000, y: 200, w: 1100, h: 720 };
        let fixed = rescue(window, &[DESKTOP_1X]).unwrap();
        assert_eq!((fixed.x, fixed.y), (410, 180));
        assert_eq!((fixed.w, fixed.h), (1100, 720));
    }

    #[test]
    fn a_mostly_hidden_window_counts_as_off_screen() {
        // Only a 100px sliver visible on the left edge.
        let window = RectPx { x: -1000, y: 100, w: 1100, h: 720 };
        assert!(rescue(window, &[DESKTOP_1X]).is_some());
    }

    #[test]
    fn sane_geometry_is_left_alone() {
        let window = RectPx { x: 100, y: 100, w: 1100, h: 720 };
        assert_eq!(rescue(window, &[DESKTOP_1X]), None);
    }

    #[test]
    fn a_window_larger_than_the_monitor_shrinks_to_fit() {
        let window = RectPx { x: 0, y: 0, w: 2560, h: 1440 };
        let fixed = rescue(window, &[DESKTOP_1X]).unwrap();
        assert!(fixed.w <= 1920 && fixed.h <= 1080);
    }

    #[test]
    fn straddling_two_monitors_picks_the_dominant_one() {
        let right_1x = MonitorPx {
            rect: RectPx { x: 2880, y: 0, w: 1920, h: 1080 },
            scale: 1.0,
        };
        // 1100×720 physical sits fully on the 1× monitor: sane there, even
        // though the same rect would be undersized on the 2× laptop.
        let window = RectPx { x: 3000, y: 100, w: 1100, h: 720 };
        assert_eq!(rescue(window, &[LAPTOP_2X, right_1x]), None);
    }

    #[test]
    fn no_monitors_means_no_judgement() {
        let window = RectPx { x: 0, y: 0, w: 10, h: 10 };
        assert_eq!(rescue(window, &[]), None);
    }
}
