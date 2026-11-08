//! Select the host desktop, not the presence of an XWayland DISPLAY socket.
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopBackend { X11, Wayland, Portal, Kms }

/// `UU_DESKTOP_BACKEND=none`: headless host — no display is opened; terminal, file transfer and
/// port mapping stay available (servers without a monitor or logged-in session).
pub fn headless() -> bool { std::env::var("UU_DESKTOP_BACKEND").is_ok_and(|v| v == "none") }

/// A Wayland compositor socket exists for this user or the environment says so, even when this
/// process (e.g. a systemd unit with only DISPLAY set) would select X11.
pub fn wayland_session_running() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland")
        || std::env::var("WAYLAND_DISPLAY").is_ok_and(|v| !v.is_empty())
        || std::env::var_os("XDG_RUNTIME_DIR").and_then(|dir| std::fs::read_dir(dir).ok())
            .is_some_and(|entries| entries.flatten().any(|e| e.file_name().to_str()
                .is_some_and(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))))
}

/// The display logind reports for this user's active graphical session (`wayland-0`, `:0`).
fn active_display() -> Option<String> {
    let uid = unsafe { libc::getuid() }.to_string();
    let out = Command::new("loginctl").args(["show-user", &uid, "-p", "Display", "--value"]).output().ok()?;
    let display = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    out.status.success().then_some(display).filter(|d| !d.is_empty())
}

fn wayland_socket(display: &str) -> bool {
    std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)
        .is_some_and(|dir| dir.join(display).exists())
}

/// A force-stopped GNOME session (RDP login) leaves the user manager environment pointing at
/// the dead compositor: `WAYLAND_DISPLAY` names a socket that no longer exists. Resolve the
/// live display from logind and export it before backend detection, bounded by 15 s (the RDP
/// session may still be starting). Nothing changes for a live socket or an X11 session.
pub fn adopt_current_session() {
    if headless() || std::env::var("UU_DESKTOP_BACKEND").as_deref() == Ok("x11") { return; }
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("x11") { return; }
    let wayland = std::env::var("WAYLAND_DISPLAY").ok().filter(|d| !d.is_empty());
    match &wayland {
        Some(display) if wayland_socket(display) => return, // the session that started us is alive
        Some(_) => {}
        None if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") => {}
        None => return, // no graphical environment at all: keep the existing fallbacks
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut reported = false;
    loop {
        match active_display() {
            Some(display) if display.starts_with("wayland-") && wayland_socket(&display) => {
                std::env::set_var("WAYLAND_DISPLAY", &display);
                std::env::set_var("XDG_SESSION_TYPE", "wayland");
                log::info!("Session environment refreshed: WAYLAND_DISPLAY={display}");
                return;
            }
            Some(display) if display.starts_with(':') => {
                std::env::set_var("DISPLAY", &display);
                std::env::set_var("XDG_SESSION_TYPE", "x11");
                log::info!("Session environment refreshed: DISPLAY={display}");
                return;
            }
            _ if Instant::now() >= deadline => {
                log::warn!("Stale WAYLAND_DISPLAY {:?} and no live session display after 15 s; keeping the inherited environment", wayland);
                return;
            }
            _ => {
                if !reported { log::info!("Waiting for the new graphical session to publish a display"); reported = true; }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

pub fn detect() -> Result<DesktopBackend, String> {
    let backend=select(std::env::var("UU_DESKTOP_BACKEND").ok().as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref())?;
    let explicit=std::env::var("UU_DESKTOP_BACKEND").unwrap_or_else(|_|"auto".into());
    let desktop=std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    Ok(if explicit=="auto" && backend==DesktopBackend::Wayland && desktop.split(':').any(|s|s.eq_ignore_ascii_case("gnome")||s.eq_ignore_ascii_case("kde")) { DesktopBackend::Portal } else { backend })
}

fn select(explicit: Option<&str>, session: Option<&str>, socket: Option<&str>) -> Result<DesktopBackend, String> {
    match explicit {
        Some("x11") => Ok(DesktopBackend::X11),
        Some("wayland") => Ok(DesktopBackend::Wayland),
        Some("kms") => Ok(DesktopBackend::Kms),
        Some("portal") => Ok(DesktopBackend::Portal),
        Some("none") => Ok(DesktopBackend::X11), // never opened, see headless()
        None | Some("auto") => {
            if session == Some("wayland") || socket.is_some_and(|s| !s.is_empty()) {
                Ok(DesktopBackend::Wayland)
            } else { Ok(DesktopBackend::X11) }
        },
        Some(_) => Err("UU_DESKTOP_BACKEND must be auto, x11, wayland, portal, kms or none".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xwayland_does_not_override_wayland_session() {
        assert_eq!(select(None,Some("wayland"),Some("wayland-0")),Ok(DesktopBackend::Wayland));
        assert_eq!(select(None,None,Some("wayland-1")),Ok(DesktopBackend::Wayland));
        assert_eq!(select(None,Some("x11"),None),Ok(DesktopBackend::X11));
        assert_eq!(select(Some("x11"),Some("wayland"),Some("wayland-0")),Ok(DesktopBackend::X11));
        assert_eq!(select(Some("kms"),Some("wayland"),Some("wayland-0")),Ok(DesktopBackend::Kms));
        assert!(select(Some("typo"),None,None).is_err());
    }
}
