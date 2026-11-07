//! A window's video on Linux (SPEC §8.8, kind `screen`): the X server's copy of the window
//! (`GetImage`, what the server holds of it, which a compositor shows), or for a window of the Wayland
//! compositor strobe runs (`target::wayland`) that compositor's copy of it (screencopy), 30 times a
//! second on the host clock, piped raw to ffmpeg, which scales it and writes a constant 30 frames a
//! second: frame `k` is the window at `t0 + k / 30 s`. A tick that came late repeats the last image for
//! the ticks it missed, so media time stays the host clock.
//!
//! A native Wayland window of the user's own compositor has no X11 window, and Wayland gives another
//! client its pixels only through the desktop portal's picker, a person's choice each time (rule 11):
//! refused.

use super::ffmpeg::{self, Ffmpeg};
use super::{Kind, Video};
use crate::model::{Error, Result};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};

const TICK_NS: u64 = 1_000_000_000 / ffmpeg::FPS as u64;

pub struct Capture {
    stop: Arc<AtomicBool>,
    end: Arc<AtomicU64>,
    thread: Option<std::thread::JoinHandle<Result<Video>>>,
}

/// Where the window's pixels come from, into a `size` image (cropped or padded black when the window's
/// size changed); false when they could not be read.
type Grab = Box<dyn FnMut(&mut Vec<u8>) -> bool + Send>;

impl Capture {
    pub fn start(window: &crate::host::TopWindow, base: &Path) -> Result<Capture> {
        if crate::host::wayland::owns(window.hwnd) {
            let hwnd = window.hwnd;
            let size = (window.client.0.max(2) as u32 & !1, window.client.1.max(2) as u32 & !1);
            let grab: Grab = Box::new(move |into: &mut Vec<u8>| {
                let Some(g) = crate::host::wayland::window_region(hwnd, [0, 0, size.0 as i32, size.1 as i32], false, Duration::from_secs(1)) else { return false };
                fit(&g.bgra, (g.width as u32, g.height as u32), size, into);
                true
            });
            return Capture::record(grab, size, base, "Wayland screencopy of the headless compositor strobe runs (its frames tick on a timer, for no display)");
        }
        if std::env::var_os("DISPLAY").is_none() {
            return Err(if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                super::refused(
                    "a Wayland session gives another client a window's pixels only through the desktop portal's picker, a person's choice each time, and this window has no X11 one",
                    "run the app on a compositor strobe starts (a native target's display: \"wayland\") or under X11 (or XWayland) for a video, or record a Chromium page (its renderer's own frames)",
                )
            } else {
                super::refused("no X11 display (DISPLAY is unset)", "record on a desktop session, or a Chromium page headless (its renderer's own frames)")
            });
        }
        let (conn, _) = x11rb::connect(None).map_err(|e| Error::msg(format!("X11: {e}")))?;
        let win = window.hwnd as Window;
        let g = conn.get_geometry(win).map_err(|e| Error::msg(format!("X11: {e}")))?.reply().map_err(|e| Error::msg(format!("X11: the window: {e}")))?;
        let size = (g.width.max(2) as u32 & !1, g.height.max(2) as u32 & !1);
        let grab: Grab = Box::new(move |into: &mut Vec<u8>| grab_x11(&conn, win, size, into));
        Capture::record(grab, size, base, "X11 GetImage")
    }

    fn record(grab: Grab, size: (u32, u32), base: &Path, source: &'static str) -> Result<Capture> {
        let exe = ffmpeg::find_raw()?;
        let encoder = ffmpeg::encoder(&exe)?;
        let out = super::fit(size.0, size.1);
        let input: Vec<String> =
            ["-f", "rawvideo", "-pix_fmt", "bgr0", "-video_size", &format!("{}x{}", size.0, size.1), "-framerate", &ffmpeg::FPS.to_string(), "-i", "pipe:0"].iter().map(|s| s.to_string()).collect();
        let filter = format!("scale={}:{}:flags=area,format=yuv420p", out.0, out.1);
        let ff = Ffmpeg::spawn(&exe, encoder, &input, &filter, base)?;
        let (stop, end) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let (s, e) = (stop.clone(), end.clone());
        let thread = std::thread::Builder::new().name("strobe-video".into()).spawn(move || run(grab, out, ff, &s, &e, source)).map_err(|e| Error::msg(format!("start the video thread: {e}")))?;
        Ok(Capture { stop, end, thread: Some(thread) })
    }

    pub fn stop(mut self) -> Result<Video> {
        self.end.store(crate::host::now_ns(), Ordering::Relaxed);
        self.stop.store(true, Ordering::Relaxed);
        let t = self.thread.take().ok_or_else(|| Error::msg("the video stopped twice"))?;
        crate::probe::join_within(t, Duration::from_secs(90)).ok_or_else(|| Error::msg("the video's encoder did not finish within 90 s"))?
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// `src` (`have` pixels, 4 bytes each) into a `size` image: cropped, or padded black.
fn fit(src: &[u8], have: (u32, u32), size: (u32, u32), into: &mut Vec<u8>) {
    let (w, h) = (have.0.min(size.0) as usize, have.1.min(size.1) as usize);
    into.clear();
    into.resize((size.0 * size.1 * 4) as usize, 0);
    for y in 0..h {
        let (from, to) = (y * have.0 as usize * 4, y * size.0 as usize * 4);
        into[to..to + w * 4].copy_from_slice(&src[from..from + w * 4]);
    }
}

/// The X11 window's pixels now, as `size`.
fn grab_x11(conn: &impl Connection, win: Window, size: (u32, u32), into: &mut Vec<u8>) -> bool {
    let Some(g) = conn.get_geometry(win).ok().and_then(|c| c.reply().ok()) else { return false };
    let (w, h) = ((g.width as u32).min(size.0), (g.height as u32).min(size.1));
    let Some(r) = conn.get_image(ImageFormat::Z_PIXMAP, win, 0, 0, w as u16, h as u16, !0).ok().and_then(|c| c.reply().ok()) else { return false };
    if r.data.len() < (w * h * 4) as usize {
        return false;
    }
    fit(&r.data, (w, h), size, into);
    true
}

fn run(mut grab: Grab, out: (u32, u32), mut ff: Ffmpeg, stop: &AtomicBool, end: &AtomicU64, source: &str) -> Result<Video> {
    let cpu = super::ThreadCpu::start();
    let mut image = Vec::new();
    if !grab(&mut image) {
        return Err(super::refused("strobe got no image of the window", "make sure the window is mapped and on screen while the video records"));
    }
    let t0 = crate::host::now_ns();
    let (mut ticks, mut grabbed) = (0u64, 1u64);
    loop {
        // Every tick up to now gets the newest image: a late tick repeats it, so frame k stays t0 + k ticks.
        let now = crate::host::now_ns();
        while t0 + ticks * TICK_NS <= now {
            ff.write(&image)?;
            ticks += 1;
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_nanos((t0 + ticks * TICK_NS).saturating_sub(crate::host::now_ns())));
        if grab(&mut image) {
            grabbed += 1;
        }
    }
    let encoder = ff.encoder.name;
    let path = ff.out.clone();
    let ext = ff.encoder.ext;
    let encoder_cpu_ms = ff.finish()?;
    Ok(Video {
        kind: Kind::Screen,
        path,
        ext,
        width: out.0,
        height: out.1,
        t0_ns: t0,
        t1_ns: end.load(Ordering::Relaxed).max(t0 + ticks * TICK_NS),
        frames: ticks,
        source_frames: grabbed,
        fps: Some(ffmpeg::FPS),
        source: source.into(),
        encoder: format!("ffmpeg {encoder}"),
        cpu_ms: cpu.ms(),
        setup_cpu_ms: None,
        encoder_cpu_ms,
        precision_ms: TICK_NS as f64 / 1e6,
    })
}
