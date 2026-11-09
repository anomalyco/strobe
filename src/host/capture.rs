//! A target's window as it shows on the screen, a region at a time, from outside (SPEC §9.6 image anchors
//! and pixel waits). One `Grabber` per window, opened when a call first needs pixels:
//!
//! - Windows: Windows.Graphics.Capture of the window, the device and capture item `probe::video::wgc`
//!   opens: a free-threaded pool of two buffers whose frame event keeps the newest frame and gives the
//!   one before back, so the compositor always has a buffer to draw the next into and no frame is copied
//!   unread; a read copies only the region out of the frame it holds (`CopySubresourceRegion`) and maps
//!   that. The window's place and display are read again only when they changed (`win::WindowKey`).
//!   Each frame carries the compositor's time on the QPC clock,
//!   and its event wakes waiters. The compositor still draws a window while the session is locked, and
//!   it captures Direct3D 12 and Vulkan flip-model windows a `PrintWindow` misses (black).
//!   Without Windows.Graphics.Capture: `PrintWindow`, cropped.
//! - macOS: `CGWindowListCreateImage` of the region's screen rect, this window only (Screen Recording).
//! - Linux X11: `GetImage` of the region of the window, once it is viewable and on top there. A window
//!   of the headless Wayland compositor strobe runs: the compositor's copy of the region, once it shows
//!   and no window covers it there.
//!
//! Regions are in the window's client area, device-independent pixels (macOS: points of the whole
//! frame, as `host::window_placement` has it); a capture says how many of its pixels make one.

use crate::model::{Error, Refusal, Result, ValidatorId};
use crate::png::Image;
use std::time::Instant;

/// One capture of a region.
pub struct Shot {
    pub img: Image,
    /// Capture pixels per device-independent pixel.
    pub scale: f64,
    /// When what it holds was on the screen (host clock): the compositor's frame time (Windows), else
    /// the read's start (`t0_ns`) and end (`t_ns`).
    pub t0_ns: u64,
    pub t_ns: u64,
    /// The source's frame count at the capture (Windows), for `Grabber::wait`.
    pub seq: u64,
    /// How long after `t_ns` what it holds may have reached the screen: a compositor's frame is captured
    /// as it is composed and scanned out up to a refresh later (Windows, and macOS's copy of the window:
    /// the window's display's refresh period); a read of what is already on the screen, 0.
    pub late_ns: u64,
}

pub struct Grabber {
    win: isize,
    /// strobe's CPU (ms) opening it, not yet taken (`take_cpu_ms`): a run whose read opened it pays for
    /// it, a Windows.Graphics.Capture open that failed or timed out (then PrintWindow) included.
    open_ms: std::cell::Cell<f64>,
    #[cfg(windows)]
    os: Option<wgc::Wgc>,
    /// What the last read knew of the window (Windows), kept while its `WindowKey` holds.
    #[cfg(windows)]
    view: std::cell::Cell<Option<View>>,
}

/// What a read needs of the window besides its pixels.
#[derive(Clone, Copy)]
struct View {
    placement: super::Placement,
    /// Minimized or hidden: nothing of it is on the screen.
    unseen: bool,
    /// The refresh period (ns) of the window's display: how much later than a frame's capture it may reach the
    /// screen (`Shot::late_ns`, where reads have a lateness).
    refresh_ns: u64,
    /// Where the client area starts in what Windows.Graphics.Capture captures of the window.
    #[cfg(windows)]
    origin: Option<(i32, i32)>,
    #[cfg(windows)]
    key: super::win::WindowKey,
}

fn period_ns(hz: Option<u32>) -> u64 {
    hz.map_or(16_666_667, |hz| 1_000_000_000 / hz.max(1) as u64)
}

impl Grabber {
    /// Pixels of `win` from now on: on Windows its capture starts here and waits up to a second for the
    /// window's first frame.
    pub fn open(win: isize) -> Result<Grabber> {
        let cpu = crate::probe::video::ThreadCpu::start();
        #[cfg(windows)]
        let g = {
            if super::window_state(win).is_some_and(|w| w.iconic) {
                return Err(hidden("the window is minimized, so the compositor draws none of it"));
            }
            // Without Windows.Graphics.Capture (or a frame of the window from it): PrintWindow.
            Grabber { win, open_ms: Default::default(), os: wgc::Wgc::open(win).ok(), view: Default::default() }
        };
        #[cfg(not(windows))]
        let g = Grabber { win, open_ms: Default::default() };
        // What the first read needs of the window, read now: the display's mode is slow to read.
        #[cfg(windows)]
        let _ = g.view();
        g.open_ms.set(cpu.ms().unwrap_or(0.0));
        Ok(g)
    }

    pub fn window(&self) -> isize {
        self.win
    }

    /// The window's placement, state and display now. Windows: read again only when its `WindowKey`
    /// changed (it moved, resized, minimized, hid, or went to another display, or its display's size or
    /// DPI changed); the display's refresh rate is that display's when the key last changed.
    fn view(&self) -> Result<View> {
        let closed = || Error::msg("the app's window closed");
        #[cfg(windows)]
        {
            let key = super::win::window_key(self.win).ok_or_else(closed)?;
            if let Some(v) = self.view.get().filter(|v| v.key == key) {
                return Ok(v);
            }
            let v = View {
                placement: super::window_placement(self.win).ok_or_else(closed)?,
                unseen: key.unseen(),
                refresh_ns: period_ns(key.refresh()),
                origin: super::win::client_in_visual(self.win),
                key,
            };
            self.view.set(Some(v));
            Ok(v)
        }
        #[cfg(not(windows))]
        {
            let placement = super::window_placement(self.win).ok_or_else(closed)?;
            let state = super::window_state(self.win);
            // Only macOS's reads have a lateness (`grab`).
            let refresh_ns = if cfg!(target_os = "macos") {
                let at = state.as_ref().map_or((0, 0), |w| (w.rect.0 + w.rect.2 / 2, w.rect.1 + w.rect.3 / 2));
                period_ns(super::refresh_at(at.0, at.1))
            } else {
                0
            };
            Ok(View { placement, unseen: state.is_some_and(|w| w.iconic || !w.visible), refresh_ns })
        }
    }

    /// Whether its window still shows: neither closed, minimized nor hidden (Windows: from the kept view).
    pub fn shown(&self) -> bool {
        #[cfg(windows)]
        return self.view().is_ok_and(|v| !v.unseen);
        #[cfg(not(windows))]
        super::window_state(self.win).is_some_and(|w| w.visible && !w.iconic)
    }

    /// The client area's size in device-independent pixels.
    pub fn size(&self) -> Result<(f64, f64)> {
        #[cfg(windows)]
        let p = self.view()?.placement;
        #[cfg(not(windows))]
        let p = super::window_placement(self.win).ok_or_else(|| Error::msg("the app's window closed"))?;
        Ok((p.client.0 / p.per_dip, p.client.1 / p.per_dip))
    }

    /// The pixels of `r` (device-independent pixels in the client area, clipped to it) as they show now.
    pub fn grab(&mut self, r: [f64; 4]) -> Result<Shot> {
        let v = self.view()?;
        let p = v.placement;
        if v.unseen {
            return Err(hidden("the window is minimized or hidden, so nothing of it is on the screen"));
        }
        let (cw, ch) = (p.client.0 / p.per_dip, p.client.1 / p.per_dip);
        let (x0, y0) = (r[0].clamp(0.0, cw), r[1].clamp(0.0, ch));
        let (x1, y1) = ((r[0] + r[2]).clamp(0.0, cw), (r[1] + r[3]).clamp(0.0, ch));
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            return Err(Refusal::new(ValidatorId::OutOfView, format!("the region [{:.0}, {:.0}, {:.0}, {:.0}] is off the {cw:.0}×{ch:.0} client area", r[0], r[1], r[2], r[3]))
                .fix("give a region inside the window's client area: [x, y, width, height] from its top left, or { …, space: \"norm\" }")
                .into());
        }
        // Screen units (physical pixels; points on macOS), outward to whole ones.
        let u = p.per_dip;
        let px = [(x0 * u).floor(), (y0 * u).floor(), (x1 * u).ceil() - (x0 * u).floor(), (y1 * u).ceil() - (y0 * u).floor()];
        let t0 = super::now_ns();
        #[cfg(windows)]
        let (img, at, seq) = {
            let px = px.map(|v| v as i32);
            match self.os.as_mut() {
                Some(w) => {
                    let origin = v.origin.ok_or_else(|| Error::msg("the app's window closed"))?;
                    match w.grab(origin, px)? {
                        Some(r) => r,
                        // The window grew and held still since: its capture's newest frame is cropped to
                        // the size before. A fresh capture starts with a whole frame: its open runs on this
                        // thread, inside the read that counts it; the old one's frame events not yet taken
                        // are kept.
                        None => {
                            let (pending, seen) = (w.take_cpu_ms(), w.seq());
                            self.open_ms.set(self.open_ms.get() + pending);
                            *w = wgc::Wgc::open(self.win)?;
                            // Its frames count on from the old one's, so a wait for a frame after one of
                            // those still means a newer frame.
                            w.count_from(seen);
                            w.grab(origin, px)?.ok_or_else(|| Error::msg("Windows.Graphics.Capture's frames of the window are smaller than the window"))?
                        }
                    }
                }
                None => {
                    let full = super::win::window_image(self.win).ok_or_else(|| Error::msg("the window did not draw itself (PrintWindow), and Windows.Graphics.Capture is not available"))?;
                    // The image is in the window's own pixels (fewer than physical ones for a DPI unaware window).
                    let k = full.width as f64 / p.client.0.max(1.0);
                    (crate::image::crop(&full, px.map(|v| (v.max(0) as f64 * k).round() as usize)), super::now_ns(), 0)
                }
            }
        };
        #[cfg(target_os = "macos")]
        let (img, at, seq) = (super::macos::window_region(self.win, px).map_err(no_source)?, super::now_ns(), 0);
        #[cfg(target_os = "linux")]
        let (img, at, seq) = {
            let px = px.map(|v| v as i32);
            // A window of the headless compositor strobe runs, else the X server's.
            let img = if super::wayland::owns(self.win) { super::wayland::shown_region(self.win, px) } else { super::x11::window_region(self.win, px) };
            (img.map_err(hidden)?, super::now_ns(), 0)
        };
        let scale = img.width as f64 / (x1 - x0);
        // A frame time is when it was composed: both ends are it, and it reaches the screen up to a
        // refresh later. macOS reads the window server's copy of the window, which holds a frame before
        // the display shows it (on air a reaction was in a capture ending 9.8 ms before ScreenCaptureKit's
        // frame of it): up to a refresh later too.
        let t0 = if seq > 0 { at } else { t0 };
        let late_ns = if seq > 0 || cfg!(target_os = "macos") { v.refresh_ns } else { 0 };
        Ok(Shot { img, scale, t0_ns: t0, t_ns: at, seq, late_ns })
    }

    /// Whether its frames come as events (Windows.Graphics.Capture): `wait` follows them.
    pub fn evented(&self) -> bool {
        #[cfg(windows)]
        return self.os.is_some();
        #[cfg(not(windows))]
        false
    }

    /// Waits until the window's capture has a frame after the one numbered `seq`, at most until `until`:
    /// true when one came. Only Windows' capture numbers its frames; elsewhere false at once, and the
    /// caller waits on the target's presents or a short poll.
    pub fn wait(&self, seq: u64, until: Instant) -> bool {
        #[cfg(windows)]
        if let Some(w) = &self.os {
            return w.wait(seq, until);
        }
        let _ = (seq, until);
        false
    }

    /// The capture's frame count now (Windows.Graphics.Capture); None where frames are not events.
    pub fn seq(&self) -> Option<u64> {
        #[cfg(windows)]
        return self.os.as_ref().map(|w| w.seq());
        #[cfg(not(windows))]
        None
    }

    /// strobe's CPU (ms) for the capture since the last call, besides its reads: opening it (once), and
    /// on its own threads its frame events copying each frame out of the pool (Windows), part of what
    /// its reads cost (`overhead.pixel_pct`). Take it before dropping the capture.
    pub fn take_cpu_ms(&self) -> f64 {
        let open = self.open_ms.take();
        #[cfg(windows)]
        return open + self.os.as_ref().map_or(0.0, |w| w.take_cpu_ms());
        #[cfg(not(windows))]
        open
    }
}

/// `WindowHidden`: what is on the screen there is not the target's.
fn hidden(why: impl Into<String>) -> Error {
    Refusal::new(ValidatorId::WindowHidden, format!("the target's pixels cannot be read: {}", why.into())).fix("keep the app's window shown and uncovered while strobe reads its pixels").into()
}

#[cfg(target_os = "macos")]
fn no_source(why: String) -> Error {
    Refusal::new(ValidatorId::NoPresentSource, format!("the target's pixels cannot be read: {why}"))
        .fix("grant Screen Recording (strobe status says to which app), and measure a window the app lets be captured")
        .into()
}

#[cfg(windows)]
mod wgc {
    use crate::model::{Error, Result};
    use crate::probe::video::wgc::{device, item, texture};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
    use windows_core::{IInspectable, Interface};

    /// The pool's buffers: the frame strobe holds and one the compositor draws the next into.
    const BUFFERS: i32 = 2;

    /// A frame of the pool strobe holds: the frame (closing it gives its buffer back to the pool), its
    /// time, its content's size and its number (the count when it came). Its surface is looked up only
    /// when a read wants it.
    struct Held {
        frame: Direct3D11CaptureFrame,
        content: (u32, u32),
        t_ns: u64,
        n: u64,
    }

    /// The newest frame, held, the count so far and the pool's buffer size. A read takes the frame out
    /// while it copies from it, so the frame event never gives its buffer back to the compositor
    /// mid-copy; a newer frame that came meanwhile replaces it when the read is done.
    #[derive(Default)]
    struct Latest {
        held: Option<Held>,
        count: u64,
        pool: (u32, u32),
    }

    /// The last read: the frame's number, what was asked (`grab`'s origin and region) and its pixels. The
    /// same ask of the same frame (a wait's baseline, of the frame the reset's wait last read) is these
    /// pixels again, with nothing copied.
    struct Last {
        n: u64,
        req: ((i32, i32), [i32; 4]),
        img: crate::png::Image,
    }

    struct Shared {
        device: ID3D11Device,
        ctx: ID3D11DeviceContext,
        d3d: IDirect3DDevice,
        latest: Mutex<Latest>,
        cv: Condvar,
        /// The frame events' CPU so far (ns), from each event thread's own cycles.
        cpu_ns: std::sync::atomic::AtomicU64,
    }

    // SAFETY: the device is free-threaded and its immediate context multithread-protected
    // (`wgc::device`); the WinRT device is agile. The frame event runs on a pool thread.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}

    pub struct Wgc {
        shared: Arc<Shared>,
        session: GraphicsCaptureSession,
        pool: Direct3D11CaptureFramePool,
        _item: GraphicsCaptureItem,
        /// Staging textures by region size, the most recently made last.
        staging: Vec<(ID3D11Texture2D, (u32, u32))>,
        last: Option<Last>,
    }

    /// Region sizes whose staging texture is kept.
    const STAGING: usize = 4;

    fn copy(img: &crate::png::Image) -> crate::png::Image {
        crate::png::Image { width: img.width, height: img.height, rgba: img.rgba.clone() }
    }

    fn win(e: windows_core::Error, what: &str) -> Error {
        Error::msg(format!("{what}: {}", e.message()))
    }

    /// The captures' Direct3D device, made once per process: making one is most of opening a capture
    /// (118 of 126 ms of CPU on adais), which a run that reads pixels can pay.
    struct Device(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice);

    // SAFETY: as `Shared`'s: the device is free-threaded, its immediate context multithread-protected.
    unsafe impl Send for Device {}

    static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

    fn shared_device(fresh: bool) -> Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
        let mut d = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
        // A device whose GPU was removed or reset since (a driver update, a TDR) is made again.
        let Device(a, b, c) = match d.take() {
            Some(held) if !fresh && unsafe { held.0.GetDeviceRemovedReason() }.is_ok() => d.insert(held),
            _ => {
                let (a, b, c) = device(false)?;
                d.insert(Device(a, b, c))
            }
        };
        Ok((a.clone(), b.clone(), c.clone()))
    }

    impl Wgc {
        /// Opens the window's capture (`Grabber::open` counts its CPU).
        pub fn open(hwnd: isize) -> Result<Wgc> {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let item = item(hwnd)?;
            let size = item.Size().map_err(|e| win(e, "the window's size"))?;
            let pool_on = |d3d: &IDirect3DDevice| Direct3D11CaptureFramePool::CreateFreeThreaded(d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, BUFFERS, size);
            // The shared device, else a new one (it failed: a GPU removed or reset since).
            let (mut device, mut ctx, mut d3d) = shared_device(false)?;
            let pool = match pool_on(&d3d) {
                Ok(p) => p,
                Err(_) => {
                    (device, ctx, d3d) = shared_device(true)?;
                    pool_on(&d3d).map_err(|e| win(e, "the capture's frame pool"))?
                }
            };
            let latest = Latest { pool: (size.Width.max(1) as u32, size.Height.max(1) as u32), ..Latest::default() };
            let shared = Arc::new(Shared { device, ctx, d3d, latest: Mutex::new(latest), cv: Condvar::new(), cpu_ns: Default::default() });
            let s = shared.clone();
            pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                if let Some(pool) = pool.as_ref() {
                    let cpu = crate::probe::video::ThreadCpu::start();
                    s.take(pool);
                    let ns = (cpu.ms().unwrap_or(0.0) * 1e6) as u64;
                    s.cpu_ns.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(())
            }))
            .map_err(|e| win(e, "the capture's frame events"))?;
            let session = pool.CreateCaptureSession(&item).map_err(|e| win(e, "the capture session"))?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture().map_err(|e| win(e, "the capture session"))?;
            let w = Wgc { shared, session, pool, _item: item, staging: Vec::new(), last: None };
            if !w.wait(0, Instant::now() + Duration::from_secs(1)) {
                return Err(Error::msg("Windows.Graphics.Capture delivered no frame of the window in a second"));
            }
            Ok(w)
        }

        /// The frames copied so far.
        pub fn seq(&self) -> u64 {
            self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).count
        }

        /// Numbers its frames on from `seen` (a capture it replaces had that many).
        pub fn count_from(&self, seen: u64) {
            let mut l = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
            l.count += seen;
            if let Some(h) = l.held.as_mut() {
                h.n += seen;
            }
        }

        /// The frame events' CPU (ms) since the last call.
        pub fn take_cpu_ms(&self) -> f64 {
            self.shared.cpu_ns.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
        }

        /// Waits for a frame after the `seq`th, at most until `until`.
        pub fn wait(&self, seq: u64, until: Instant) -> bool {
            let mut l = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
            while l.count <= seq {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                l = self.shared.cv.wait_timeout(l, left).unwrap_or_else(|e| e.into_inner()).0;
            }
            true
        }

        /// `[x, y, w, h]` (physical pixels in the client area, which starts at `origin` in the captured
        /// frame) of the newest frame: its pixels, time and number; None when that frame was cropped to
        /// a smaller window's size (a fresh capture's first frame is whole). The frame stays out of the
        /// frame event's reach until its region is read back.
        pub fn grab(&mut self, origin: (i32, i32), px: [i32; 4]) -> Result<Option<(crate::png::Image, u64, u64)>> {
            let held = {
                let mut l = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
                let h = l.held.as_ref().ok_or_else(|| Error::msg("Windows.Graphics.Capture has no frame of the window"))?;
                if let Some(last) = self.last.as_ref().filter(|x| x.n == h.n && x.req == (origin, px)) {
                    return Ok(Some((copy(&last.img), h.t_ns, h.n)));
                }
                l.held.take().ok_or_else(|| Error::msg("internal: the held frame"))?
            };
            let read = self.read(&held, origin, px);
            let mut l = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
            // A newer frame came while it was read: this one goes back to the pool.
            if l.held.is_none() {
                l.held = Some(held);
            } else {
                let _ = held.frame.Close();
            }
            drop(l);
            read
        }

        fn read(&mut self, held: &Held, (ox, oy): (i32, i32), [x, y, w, h]: [i32; 4]) -> Result<Option<(crate::png::Image, u64, u64)>> {
            let tex: ID3D11Texture2D =
                held.frame.Surface().and_then(|s| s.cast::<IDirect3DDxgiInterfaceAccess>()).and_then(|a| unsafe { a.GetInterface() }).map_err(|e| win(e, "the frame's surface"))?;
            let mut d = D3D11_TEXTURE2D_DESC::default();
            unsafe { tex.GetDesc(&mut d) };
            let content = (held.content.0.min(d.Width), held.content.1.min(d.Height));
            // The window grew past the pool's buffers: its newest frame was cropped to them, and a window
            // that holds still sends no other.
            let want = (((ox + x + w).max(0) as u32).min(held.content.0), ((oy + y + h).max(0) as u32).min(held.content.1));
            if want.0 > content.0 || want.1 > content.1 {
                return Ok(None);
            }
            let (left, top) = ((ox + x).max(0) as u32, (oy + y).max(0) as u32);
            let (right, bottom) = (((ox + x + w).max(0) as u32).min(content.0), ((oy + y + h).max(0) as u32).min(content.1));
            if right <= left || bottom <= top {
                return Err(Error::msg("the region is outside the window's captured frame"));
            }
            let (rw, rh) = (right - left, bottom - top);
            // A staging texture per region size, kept: a wait reads one region over and over, an aim and
            // its wait two.
            let staging = match self.staging.iter().position(|s| s.1 == (rw, rh)) {
                Some(i) => self.staging[i].0.clone(),
                None => {
                    let t = texture(&self.shared.device, rw, rh, DXGI_FORMAT_B8G8R8A8_UNORM, 0, true).map_err(|e| win(e, "a staging texture"))?;
                    if self.staging.len() >= STAGING {
                        self.staging.remove(0);
                    }
                    self.staging.push((t.clone(), (rw, rh)));
                    t
                }
            };
            let ctx = &self.shared.ctx;
            let (w4, n) = (rw as usize * 4, (rw * rh * 4) as usize);
            let mut rgba = vec![0u8; n];
            unsafe {
                ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &tex, 0, Some(&D3D11_BOX { left, top, front: 0, right, bottom, back: 1 }));
                ctx.Flush();
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                let mut tries = 0;
                // Waits for the GPU's copy by sleeping, not in the driver (which may spin a core); a small
                // region's copy is done within the first sleep.
                loop {
                    std::thread::sleep(Duration::from_micros(250));
                    match ctx.Map(&staging, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut m)) {
                        Ok(()) => break,
                        Err(e) if e.code() == windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING && tries < 2000 => tries += 1,
                        Err(e) => return Err(win(e, "reading the frame")),
                    }
                }
                // Whole rows out of the mapped memory (which may be uncached), then BGRA to RGBA in place.
                let src = m.pData as *const u8;
                for (row, dst) in rgba.chunks_exact_mut(w4).enumerate() {
                    dst.copy_from_slice(std::slice::from_raw_parts(src.add(row * m.RowPitch as usize), w4));
                }
                ctx.Unmap(&staging, 0);
            }
            for p in rgba.as_chunks_mut::<4>().0 {
                *p = [p[2], p[1], p[0], 255];
            }
            let img = crate::png::Image { width: rw as usize, height: rh as usize, rgba };
            self.last = Some(Last { n: held.n, req: ((ox, oy), [x, y, w, h]), img: copy(&img) });
            Ok(Some((img, held.t_ns, held.n)))
        }
    }

    impl Shared {
        /// Keeps the newest frame the pool holds in `latest` and gives the one it replaces back, so a
        /// buffer is free for the next; nothing is copied. A frame of another size makes the pool follow it.
        fn take(&self, pool: &Direct3D11CaptureFramePool) {
            while let Ok(f) = pool.TryGetNextFrame() {
                let t = f.SystemRelativeTime().map(|t| t.Duration as u64 * 100).unwrap_or_else(|_| crate::host::now_ns());
                let mut l = self.latest.lock().unwrap_or_else(|e| e.into_inner());
                let content = f.ContentSize().map(|s| (s.Width.max(1) as u32, s.Height.max(1) as u32)).unwrap_or(l.pool);
                l.count += 1;
                let n = l.count;
                let old = l.held.replace(Held { frame: f, content, t_ns: t, n });
                let resize = content != l.pool;
                if resize {
                    l.pool = content;
                }
                drop(l);
                self.cv.notify_all();
                // Outside the lock: giving a buffer back synchronizes with the GPU, which a read waiting
                // for the frame need not wait out.
                if let Some(old) = old {
                    let _ = old.frame.Close();
                }
                if resize {
                    let _ = pool.Recreate(&self.d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, BUFFERS, windows::Graphics::SizeInt32 { Width: content.0 as i32, Height: content.1 as i32 });
                }
            }
        }
    }

    impl Drop for Wgc {
        fn drop(&mut self) {
            let _ = self.session.Close();
            if let Some(h) = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).held.take() {
                let _ = h.frame.Close();
            }
            let _ = self.pool.Close();
        }
    }
}
