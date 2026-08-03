//! System folder picker used to choose where incoming uploads are saved.
//!
//! Must be started from the UI thread. It never blocks: the result is
//! delivered to `on_result` on the UI thread once the user picks (or cancels).

use std::path::PathBuf;
use windows::Storage::Pickers::{FolderPicker, PickerLocationId};
use windows::Storage::StorageFolder;
use windows::Win32::shobjidl_core::IInitializeWithWindow;
use windows::Win32::winuser::GetActiveWindow;
use windows::core::Interface;
use windows_future::AsyncOperationCompletedHandler;

/// Start the system folder picker. `on_result` runs when the user picks a
/// folder (or cancels), with `None` meaning cancelled.
pub(crate) fn pick_folder(on_result: impl FnOnce(Option<PathBuf>) + Send + 'static) {
    let Some(picker) = FolderPicker::new().ok() else {
        return on_result(None);
    };
    if picker
        .SetSuggestedStartLocation(PickerLocationId::Desktop)
        .is_err()
    {
        return on_result(None);
    }

    // Desktop (WinUI 3) apps must initialize the picker with the window HWND.
    let hwnd = unsafe { GetActiveWindow() };
    let init: IInitializeWithWindow = match picker.cast() {
        Ok(init) => init,
        Err(_) => return on_result(None),
    };
    if unsafe { init.Initialize(hwnd) }.is_err() {
        return on_result(None);
    }

    match picker.PickSingleFolderAsync() {
        Ok(op) => {
            let op2 = op.clone();
            let on_result = std::cell::RefCell::new(Some(on_result));
            let handler = AsyncOperationCompletedHandler::<StorageFolder>::new(
                move |_, _| {
                    let path = op2
                        .GetResults()
                        .ok()
                        .and_then(|folder| folder.Path().ok())
                        .map(|p| PathBuf::from(p.to_os_string()));
                    if let Some(cb) = on_result.borrow_mut().take() {
                        cb(path);
                    }
                    Ok(())
                },
            );
            let _ = op.SetCompleted(&handler);
        }
        Err(_) => on_result(None),
    }
}
