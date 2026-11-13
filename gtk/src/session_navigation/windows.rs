//! HWND activation is tied to the captured host process incarnation. Windows
//! Terminal's session GUID is not a pane-selection API and is never used as one.
use super::*;
mod api;

pub(super) fn activate(location: &SessionLocation, host: &dyn Host) -> Result<Outcome, String> {
    verify(host, location)?;
    let pid = location
        .host_pid
        .ok_or("No application process identity was recorded.")?;
    let candidates = api::visible_windows(pid);
    let window = choose_window(location, &candidates)?;
    if !api::is_window(window) || api::owner(window) != pid as u32 {
        return Err("The original window closed or changed owner.".into());
    }
    verify(host, location)?;
    api::restore(window);
    if !api::set_foreground(window) {
        return Err(
            "Windows refused to foreground the original application. Select it from the taskbar."
                .into(),
        );
    }
    Ok(if api::foreground_window() == window {
        Outcome::Verified
    } else {
        Outcome::Dispatched
    })
}

pub(super) fn open_link(link: &str) -> Result<Outcome, String> {
    if api::open_link(link) {
        Ok(Outcome::Dispatched)
    } else {
        Err(
            "Windows could not dispatch this session link. No replacement session was started."
                .into(),
        )
    }
}
