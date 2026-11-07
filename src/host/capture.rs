//! A target's window as it shows on the screen, a region at a time, from outside (SPEC §9.6 image anchors
//! and pixel waits). One `Grabber` per window, opened when a call first needs pixels:
//!
//! - Windows: Windows.Graphics.Capture of the window, the device and capture item `probe::video::wgc`
//!   opens: a free-threaded pool of one buffer whose frame event copies each frame into a texture strobe
//!   keeps, so the pool is never held; a read copies only the region out of it
//!   (`CopySubresourceRegion`) and maps that. Each frame carries the compositor's time on the QPC clock,
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
    /// as it is composed and scanned out up to a refresh later (Windows: the window's display's refresh
    /// period); a read of what is already on the screen, 0.
    pub late_ns: u64,
}

pub struct Grabber {
    win: isize,
    #[cfg(windows)]
    os: Option<wgc::Wgc>,
}

impl Grabber {
    /// Pixels of `win` from now on: on Windows its capture starts here and waits up to a second for the
    /// window's first frame.
    pub fn open(win: isize) -> Result<Grabber> {
        #[cfg(windows)]
        {
            if super::window_state(win).is_some_and(|w| w.iconic) {
                return Err(hidden("the window is minimized, so the compositor draws none of it"));
            }
            // Without Windows.Graphics.Capture (or a frame of the window from it): PrintWindow.
            Ok(Grabber { win, os: wgc::Wgc::open(win).ok() })
        }
        #[cfg(not(windows))]
        Ok(Grabber { win })
    }

    pub fn window(&self) -> isize {
        self.win
    }

    /// The client area's size in device-independent pixels.
    pub fn size(&self) -> Result<(f64, f64)> {
        let p = super::window_placement(self.win).ok_or_else(|| Error::msg("the app's window closed"))?;
        Ok((p.client.0 / p.per_dip, p.client.1 / p.per_dip))
    }

    /// The pixels of `r` (device-independent pixels in the client area, clipped to it) as they show now.
    pub fn grab(&mut self, r: [f64; 4]) -> Result<Shot> {
        let p = super::window_placement(self.win).ok_or_else(|| Error::msg("the app's window closed"))?;
        if super::window_state(self.win).is_some_and(|w| w.iconic || !w.visible) {
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
                Some(w) => w.grab(self.win, px)?,
                None => {
                    let full = super::win::window_image(self.win).ok_or_else(|| Error::msg("the window did not draw itself (PrintWindow), and Windows.Graphics.Capture is not available"))?;
                    (crate::image::crop(&full, px.map(|v| v.max(0) as usize)), super::now_ns(), 0)
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
        // refresh later.
        let t0 = if seq > 0 { at } else { t0 };
        let late_ns = if seq > 0 {
            let at = super::window_state(self.win).map_or((0, 0), |w| (w.rect.0 + w.rect.2 / 2, w.rect.1 + w.rect.3 / 2));
            super::refresh_at(at.0, at.1).map_or(16_666_667, |hz| 1_000_000_000 / hz.max(1) as u64)
        } else {
            0
        };
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

    /// strobe's CPU (ms) on the capture's own threads since the last call: its frame events copying each
    /// frame out of the pool (Windows), part of what its reads cost (`overhead.pixel_pct`).
    pub fn take_cpu_ms(&self) -> f64 {
        #[cfg(windows)]
        return self.os.as_ref().map_or(0.0, |w| w.take_cpu_ms());
        #[cfg(not(windows))]
        0.0
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
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
    use windows_core::{IInspectable, Interface};

    /// The newest frame, copied out of the pool, with its time, its content's size and the count so far.
    #[derive(Default)]
    struct Latest {
        tex: Option<ID3D11Texture2D>,
        size: (u32, u32),
        content: (u32, u32),
        t_ns: u64,
        count: u64,
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
        staging: Option<(ID3D11Texture2D, (u32, u32))>,
    }

    fn win(e: windows_core::Error, what: &str) -> Error {
        Error::msg(format!("{what}: {}", e.message()))
    }

    impl Wgc {
        pub fn open(hwnd: isize) -> Result<Wgc> {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let (device, ctx, d3d) = device(false)?;
            let item = item(hwnd)?;
            let size = item.Size().map_err(|e| win(e, "the window's size"))?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, size).map_err(|e| win(e, "the capture's frame pool"))?;
            let shared = Arc::new(Shared { device, ctx, d3d, latest: Mutex::new(Latest::default()), cv: Condvar::new(), cpu_ns: Default::default() });
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
            let w = Wgc { shared, session, pool, _item: item, staging: None };
            if !w.wait(0, Instant::now() + Duration::from_secs(1)) {
                return Err(Error::msg("Windows.Graphics.Capture delivered no frame of the window in a second"));
            }
            Ok(w)
        }

        /// The frames copied so far.
        pub fn seq(&self) -> u64 {
            self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).count
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

        /// `[x, y, w, h]` (physical pixels in the client area) of the newest frame: its pixels, time and number.
        pub fn grab(&mut self, hwnd: isize, [x, y, w, h]: [i32; 4]) -> Result<(crate::png::Image, u64, u64)> {
            let (ox, oy) = crate::host::win::client_in_visual(hwnd).ok_or_else(|| Error::msg("the app's window closed"))?;
            let (tex, content, t_ns, count) = {
                let l = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
                (l.tex.clone().ok_or_else(|| Error::msg("Windows.Graphics.Capture has no frame of the window"))?, l.content, l.t_ns, l.count)
            };
            let (left, top) = ((ox + x).max(0) as u32, (oy + y).max(0) as u32);
            let (right, bottom) = (((ox + x + w).max(0) as u32).min(content.0), ((oy + y + h).max(0) as u32).min(content.1));
            if right <= left || bottom <= top {
                return Err(Error::msg("the region is outside the window's captured frame"));
            }
            let (rw, rh) = (right - left, bottom - top);
            if self.staging.as_ref().is_none_or(|s| s.1 != (rw, rh)) {
                let t = texture(&self.shared.device, rw, rh, DXGI_FORMAT_B8G8R8A8_UNORM, 0, true).map_err(|e| win(e, "a staging texture"))?;
                self.staging = Some((t, (rw, rh)));
            }
            let staging = &self.staging.as_ref().ok_or_else(|| Error::msg("internal: the staging texture"))?.0;
            let ctx = &self.shared.ctx;
            let mut rgba = Vec::with_capacity((rw * rh * 4) as usize);
            unsafe {
                ctx.CopySubresourceRegion(staging, 0, 0, 0, 0, &tex, 0, Some(&D3D11_BOX { left, top, front: 0, right, bottom, back: 1 }));
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                let mut tries = 0;
                // Waits for the GPU's copy by sleeping, not in the driver (which may spin a core).
                loop {
                    match ctx.Map(staging, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut m)) {
                        Ok(()) => break,
                        Err(e) if e.code() == windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING && tries < 2000 => {
                            tries += 1;
                            std::thread::sleep(Duration::from_micros(250));
                        }
                        Err(e) => return Err(win(e, "reading the frame")),
                    }
                }
                let src = m.pData as *const u8;
                for row in 0..rh as usize {
                    let line = std::slice::from_raw_parts(src.add(row * m.RowPitch as usize), rw as usize * 4);
                    for p in line.as_chunks::<4>().0 {
                        rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                    }
                }
                ctx.Unmap(staging, 0);
            }
            Ok((crate::png::Image { width: rw as usize, height: rh as usize, rgba }, t_ns, count))
        }
    }

    impl Shared {
        /// Copies every frame the pool holds into `latest` and lets it go, so the one buffer is free for
        /// the next; a frame of another size makes the pool follow it.
        fn take(&self, pool: &Direct3D11CaptureFramePool) {
            while let Ok(f) = pool.TryGetNextFrame() {
                let t = f.SystemRelativeTime().map(|t| t.Duration as u64 * 100).unwrap_or_else(|_| crate::host::now_ns());
                let content = f.ContentSize().ok().map(|s| (s.Width.max(1) as u32, s.Height.max(1) as u32));
                let src: Option<ID3D11Texture2D> = f.Surface().ok().and_then(|s| s.cast::<IDirect3DDxgiInterfaceAccess>().ok()).and_then(|a| unsafe { a.GetInterface() }.ok());
                if let Some(src) = src {
                    let mut d = D3D11_TEXTURE2D_DESC::default();
                    unsafe { src.GetDesc(&mut d) };
                    let mut l = self.latest.lock().unwrap_or_else(|e| e.into_inner());
                    if l.tex.is_none() || l.size != (d.Width, d.Height) {
                        l.tex = texture(&self.device, d.Width, d.Height, DXGI_FORMAT_B8G8R8A8_UNORM, 0, false).ok();
                        l.size = (d.Width, d.Height);
                    }
                    if let Some(dst) = &l.tex {
                        unsafe { self.ctx.CopyResource(dst, &src) };
                        let c = content.unwrap_or(l.size);
                        l.content = (c.0.min(l.size.0), c.1.min(l.size.1));
                        l.t_ns = t;
                        l.count += 1;
                    }
                    drop(l);
                    let _ = f.Close();
                    if let Some(c) = content.filter(|c| *c != (d.Width, d.Height)) {
                        let _ = pool.Recreate(&self.d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, windows::Graphics::SizeInt32 { Width: c.0 as i32, Height: c.1 as i32 });
                    }
                    continue;
                }
                let _ = f.Close();
            }
            self.cv.notify_all();
        }
    }

    impl Drop for Wgc {
        fn drop(&mut self) {
            let _ = self.session.Close();
            let _ = self.pool.Close();
        }
    }
}
