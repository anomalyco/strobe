//! An app's frames on the headless Wayland compositor strobe runs (`target::wayland`), for the
//! driver's waits only (`until: "frame"`, idle, settles): each time the compositor changes the app's
//! main window (screencopy's damage, read without the pixels), on the host clock. They reach no
//! display, so no metric is made of them: `stop` refuses them `Headless`.

use super::{Live, Presents};
use crate::host::wayland::Compositor;
use crate::model::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Capture {
    pub live: Arc<Live>,
    pids: Arc<Mutex<Vec<u32>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    pub fn start(c: &Arc<Compositor>) -> Result<Capture> {
        let mut watch = c.watch()?;
        let (live, pids, stop) = (Arc::new(Live::default()), Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (l, p, s) = (live.clone(), pids.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("strobe-present".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    let pids = p.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some((pid, at)) = watch.next(&pids, Duration::from_millis(100)) {
                        l.push(pid, at, at);
                    }
                }
            })
            .map_err(|e| Error::msg(format!("start the present thread: {e}")))?;
        Ok(Capture { live, pids, stop, thread: Some(thread) })
    }

    /// Follows the main window of `pids`.
    pub fn track(&mut self, pids: &[u32]) {
        *self.pids.lock().unwrap_or_else(|e| e.into_inner()) = pids.to_vec();
    }

    /// Ends the watch: what it saw reached no display (`Headless`).
    pub fn stop(mut self) -> Result<Presents> {
        self.end();
        Err(super::headless_compositor())
    }

    fn end(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = crate::probe::join_within(t, Duration::from_secs(2));
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.end();
    }
}
