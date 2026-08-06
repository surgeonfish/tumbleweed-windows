//! Taskbar attention — flash the app's taskbar button when something needs
//! attention (e.g. an incoming-upload dialog) while the window isn't focused.

use windows::Win32::winuser::{
    FlashWindowEx, FindWindowW, GetForegroundWindow, FLASHWINFO, FLASHW_ALL,
    FLASHW_TIMERNOFG,
};

/// Flash the taskbar button if the app isn't in the foreground. No-op when the
/// window can't be found or is already focused.
pub(crate) fn flash_if_background() {
    // WinUI 3 top-level windows use this fixed window class.
    let hwnd = unsafe {
        FindWindowW(
            windows::core::w!("WinUIDesktopWin32WindowClass"),
            windows::core::PCWSTR::null(),
        )
    };
    if hwnd.0.is_null() {
        return;
    }
    // Already focused — nothing to flash.
    if unsafe { GetForegroundWindow() } == hwnd {
        return;
    }

    let mut info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        // Flash the caption + taskbar button until the window comes to the
        // foreground (uCount = 0 means "keep flashing").
        dwFlags: (FLASHW_ALL | FLASHW_TIMERNOFG) as u32,
        uCount: 0,
        dwTimeout: 0,
    };
    unsafe {
        let _ = FlashWindowEx(&mut info);
    }
}
