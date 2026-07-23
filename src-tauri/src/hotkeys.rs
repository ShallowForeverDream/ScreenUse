use crate::collectors::{CollectorAdapter, DesktopCollector};
use crate::db::AppDb;
use std::sync::Arc;

pub(crate) const MANUAL_AWAY_SHORTCUT_LABEL: &str = "Ctrl+Alt+Enter";

pub(crate) fn begin_manual_away(db: &Arc<AppDb>, collector: &Arc<DesktopCollector>) {
    collector.begin_manual_away();
    if let Err(error) = collector.start(db.clone()) {
        collector.cancel_manual_away();
        collector.report_error(error);
    }
}

#[cfg(windows)]
pub(crate) fn register_manual_away_hotkey(
    db: Arc<AppDb>,
    collector: Arc<DesktopCollector>,
) -> anyhow::Result<()> {
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_RETURN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    const HOTKEY_ID: i32 = 0x5355;
    let (registered_tx, registered_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::Builder::new()
        .name("screenuse-manual-away-hotkey".into())
        .spawn(move || {
            let window = HWND::default();
            let modifiers = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
            let registration =
                unsafe { RegisterHotKey(window, HOTKEY_ID, modifiers, u32::from(VK_RETURN.0)) };
            if let Err(error) = registration {
                let _ = registered_tx.send(Err(error.to_string()));
                return;
            }
            let _ = registered_tx.send(Ok(()));

            let mut message = MSG::default();
            loop {
                let result = unsafe { GetMessageW(&mut message, window, 0, 0) };
                if result.0 <= 0 {
                    break;
                }
                if message.message == WM_HOTKEY && message.wParam.0 == HOTKEY_ID as usize {
                    begin_manual_away(&db, &collector);
                }
            }
            let _ = unsafe { UnregisterHotKey(window, HOTKEY_ID) };
        })?;

    match registered_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!(
            "cannot register {MANUAL_AWAY_SHORTCUT_LABEL}: {error}"
        )),
        Err(error) => Err(anyhow::anyhow!(
            "timed out registering {MANUAL_AWAY_SHORTCUT_LABEL}: {error}"
        )),
    }
}

#[cfg(not(windows))]
pub(crate) fn register_manual_away_hotkey(
    _db: Arc<AppDb>,
    _collector: Arc<DesktopCollector>,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_manual_away_shortcut_is_documented_consistently() {
        assert_eq!(MANUAL_AWAY_SHORTCUT_LABEL, "Ctrl+Alt+Enter");
    }
}
