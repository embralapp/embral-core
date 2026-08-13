//! Notice-window styling: `WS_EX_NOACTIVATE`, so even a button click on a
//! notice never activates the app. A notice matters most mid-call, and
//! pulling focus off the meeting app would be the worst moment to do it.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
};

/// Apply the never-activate style to the notice window. Takes the Tauri
/// window and extracts the native handle here, so the caller needs no
/// `cfg` (`platform/mod.rs`). Must run on the main thread; the caller
/// uses the window's main-thread hook.
pub fn style_notice(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd.0);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
    }
}
