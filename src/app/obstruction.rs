//! Details about a blocker are for the person at the terminal. Only the role or count goes back
//! to the caller, even when progress is disabled or redirected to a log.
use crate::text::{PrivKind, Private, Text};

fn diagnostic(text: Text) -> String {
    if text.is_marked() {
        super::live_progress(format!("  {}", text.local()));
    }
    text.public().into_owned()
}

pub fn ancestor(name: &Private<String>) -> String {
    diagnostic(name.redacted("the app or daemon strobe runs under"))
}

pub fn timer(ms: f64, job: &Private<String>) -> String {
    ancestor(job);
    format!("timers that wake about {} late: it runs in a launchd job whose ProcessType lets macOS coalesce the timers of everything it starts", crate::fmt::time(ms))
}

/// The requested browser name is configuration; the PIDs came from a session strobe does not own.
pub fn running_browser(name: &str, pids: &[u32]) -> String {
    super::live_progress(format!("  running browser PIDs: {pids:?}"));
    format!("{name} is running: a WebDriver session would run in that same app, beside your tabs and their web content processes")
}

pub fn unverified_browser(name: &str, pid: u32, why: &str) -> String {
    super::live_progress(format!("  unverified browser: pid {pid}"));
    format!("{name} opened a session, but strobe could not prove it is the browser it asked for ({why}): strobe touched nothing of it")
}

pub fn foreign_launch(pid: u32, why: &str) -> String {
    super::live_progress(format!("  Launch Services answered with pid {pid}"));
    format!("Launch Services answered with a process outside this launch ({why}): strobe touched nothing of it")
}

pub fn unattributed(pid: u32) -> String {
    super::live_progress(format!("  responsible process: {pid}"));
    "the app started responsible to the app or daemon strobe runs under, not to itself, so its XPC services (web content, GPU and network helpers) are not counted".into()
}

pub fn window(title: &Private<String>, pid: u32) {
    super::live_progress(crate::msg!("  covering window or element: {} (pid {})", title.quoted_or("another window"), pid).local());
}

pub fn tab_count(tabs: &[Private<String>], serial: &str) -> String {
    if let Some(first) = tabs.first() {
        super::live_progress(crate::msg!("  open tab on {}: {}", serial, first.quoted_or("a tab")).local());
    }
    format!("{} tab{} a person opened", tabs.len(), if tabs.len() == 1 { "" } else { "s" })
}

pub fn socket_holder(holder: &Private<String>, serial: &str) -> String {
    diagnostic(crate::msg!("{} holds Chrome's DevTools socket on {}", holder.redacted("another app"), Text::marked(serial, PrivKind::DeviceId, "the phone")))
}

pub fn stay_awake(owner: &str, serial: &str) -> String {
    super::live_progress(format!("  phone {serial} kept awake by strobe {owner}"));
    "another strobe keeps the phone awake".into()
}

pub fn helper_pipe(exe: &Private<String>, pid: u32, owner: &Private<String>) -> &'static str {
    super::live_progress(crate::msg!("  helper pipe served by {} (pid {}, owner {})", exe.redacted("another process"), pid, owner.redacted("another account")).local());
    "refused the helper pipe: served by another process or account, not the installed helper"
}
