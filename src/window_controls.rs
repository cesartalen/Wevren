use gpui::Window;

pub fn toggle_maximize(window: &Window) -> anyhow::Result<()> {
    #[cfg(windows)]
    if window.is_maximized() {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{SW_RESTORE, ShowWindowAsync},
        };

        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .as_raw()
        else {
            anyhow::bail!("The window has no Windows handle");
        };
        // SAFETY: This handle belongs to the live GPUI window and is used on its UI thread.
        unsafe {
            if !ShowWindowAsync(HWND(handle.hwnd.get() as *mut std::ffi::c_void), SW_RESTORE)
                .as_bool()
            {
                anyhow::bail!("Could not restore the window");
            }
        }
        return Ok(());
    }
    window.zoom_window();
    Ok(())
}
