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
//! - macOS: a ScreenCaptureKit stream of the window alone on its display, started by the first read, its
//!   source rectangle the part of the window's frame read so far (it grows to hold a region outside it)
//!   at the display's pixels, BGRA, a frame only when something there changed. Each frame replaces the
//!   one strobe keeps (retained, never copied); a read copies only the region out of it. Each frame
//!   carries the time it was displayed (the host clock), and its arrival wakes waiters. A read checks
//!   the window's frame first (one window-list read of that window) and points the stream at a frame
//!   that moved. Without the stream (it did not start or cannot follow the window):
//!   `CGWindowListCreateImage` of the region's screen rect, this window only. Both need Screen Recording.
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
    /// When what it holds was on the screen (host clock): the compositor's frame time (Windows), the
    /// frame's display time (macOS's stream), else the read's start (`t0_ns`) and end (`t_ns`).
    pub t0_ns: u64,
    pub t_ns: u64,
    /// The source's frame count at the capture (Windows, macOS's stream), for `Grabber::wait`.
    pub seq: u64,
    /// How long after `t_ns` what it holds may have reached the screen: a compositor's frame is captured
    /// as it is composed and scanned out up to a refresh later (Windows, and macOS's copy of the window
    /// without the stream: the window's display's refresh period); a frame stamped with its display
    /// time, or a read of what is already on the screen, 0.
    pub late_ns: u64,
}

pub struct Grabber {
    win: isize,
    #[cfg(windows)]
    os: Option<wgc::Wgc>,
    /// The stream, once a read started it; `tried` once one did not start (it is not tried again).
    #[cfg(target_os = "macos")]
    os: Option<sck::Stream>,
    #[cfg(target_os = "macos")]
    tried: bool,
}

impl Grabber {
    /// Pixels of `win` from now on: on Windows its capture starts here and waits up to a second for the
    /// window's first frame (macOS: at the first read, which knows the region).
    pub fn open(win: isize) -> Result<Grabber> {
        #[cfg(windows)]
        {
            if super::window_state(win).is_some_and(|w| w.iconic) {
                return Err(hidden("the window is minimized, so the compositor draws none of it"));
            }
            // Without Windows.Graphics.Capture (or a frame of the window from it): PrintWindow.
            Ok(Grabber { win, os: wgc::Wgc::open(win).ok() })
        }
        #[cfg(target_os = "macos")]
        return Ok(Grabber { win, os: None, tried: false });
        #[cfg(target_os = "linux")]
        Ok(Grabber { win })
    }

    pub fn window(&self) -> isize {
        self.win
    }

    /// The client area's size in device-independent pixels, its screen units per one, whether the window
    /// shows, and its frame in screen units (macOS: its frame in points, from one read of that window).
    fn client(&self) -> Result<(f64, f64, f64, bool, [f64; 4])> {
        let closed = || Error::msg("the app's window closed");
        #[cfg(target_os = "macos")]
        {
            let (f, shown) = super::macos::window_frame(self.win).ok_or_else(closed)?;
            Ok((f[2], f[3], 1.0, shown, f))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let p = super::window_placement(self.win).ok_or_else(closed)?;
            let shown = !super::window_state(self.win).is_some_and(|w| w.iconic || !w.visible);
            Ok((p.client.0 / p.per_dip, p.client.1 / p.per_dip, p.per_dip, shown, p.frame))
        }
    }

    /// The client area's size in device-independent pixels.
    pub fn size(&self) -> Result<(f64, f64)> {
        let (w, h, ..) = self.client()?;
        Ok((w, h))
    }

    /// The pixels of `r` (device-independent pixels in the client area, clipped to it) as they show now.
    pub fn grab(&mut self, r: [f64; 4]) -> Result<Shot> {
        let (cw, ch, u, shown, frame) = self.client()?;
        if !shown {
            return Err(hidden("the window is minimized or hidden, so nothing of it is on the screen"));
        }
        let (x0, y0) = (r[0].clamp(0.0, cw), r[1].clamp(0.0, ch));
        let (x1, y1) = ((r[0] + r[2]).clamp(0.0, cw), (r[1] + r[3]).clamp(0.0, ch));
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            return Err(Refusal::new(ValidatorId::OutOfView, format!("the region [{:.0}, {:.0}, {:.0}, {:.0}] is off the {cw:.0}×{ch:.0} client area", r[0], r[1], r[2], r[3]))
                .fix("give a region inside the window's client area: [x, y, width, height] from its top left, or { …, space: \"norm\" }")
                .into());
        }
        // Screen units (physical pixels; points on macOS), outward to whole ones.
        let px = [(x0 * u).floor(), (y0 * u).floor(), (x1 * u).ceil() - (x0 * u).floor(), (y1 * u).ceil() - (y0 * u).floor()];
        let t0 = super::now_ns();
        // `composed`: what it holds reaches the screen up to a refresh after its time.
        #[cfg(windows)]
        let (img, at, seq, composed) = {
            let px = px.map(|v| v as i32);
            match self.os.as_mut() {
                Some(w) => {
                    let (img, at, seq) = w.grab(self.win, px)?;
                    (img, at, seq, true)
                }
                None => {
                    let full = super::win::window_image(self.win).ok_or_else(|| Error::msg("the window did not draw itself (PrintWindow), and Windows.Graphics.Capture is not available"))?;
                    (crate::image::crop(&full, px.map(|v| v.max(0) as usize)), super::now_ns(), 0, false)
                }
            }
        };
        // The stream's frame is stamped with when it was displayed. Without it, macOS reads the window
        // server's copy of the window, which holds a frame before the display shows it (on air a reaction
        // was in a capture ending 9.8 ms before ScreenCaptureKit's frame of it): up to a refresh later.
        // Without the stream (no Screen Recording, a window kept out of capture, no frame of it in a
        // second, a window it cannot follow): CGWindowListCreateImage, which says why when it cannot
        // read the window either.
        #[cfg(target_os = "macos")]
        let (img, at, seq, composed) = {
            if self.os.is_none() && !self.tried {
                self.tried = true;
                self.os = sck::Stream::open(self.win, frame, px).ok();
            }
            match self.os.as_mut().map(|s| s.grab(frame, px)) {
                Some(Ok((img, at, seq))) => (img, at, seq, false),
                other => {
                    if other.is_some() {
                        self.os = None;
                    }
                    (super::macos::window_region(self.win, px).map_err(no_source)?, super::now_ns(), 0, true)
                }
            }
        };
        #[cfg(target_os = "linux")]
        let (img, at, seq, composed) = {
            let px = px.map(|v| v as i32);
            // A window of the headless compositor strobe runs, else the X server's.
            let img = if super::wayland::owns(self.win) { super::wayland::shown_region(self.win, px) } else { super::x11::window_region(self.win, px) };
            (img.map_err(hidden)?, super::now_ns(), 0, false)
        };
        let scale = img.width as f64 / (x1 - x0);
        // A frame time is when it was composed: both ends are it, and it reaches the screen up to a
        // refresh later.
        let t0 = if seq > 0 { at } else { t0 };
        let late_ns = if composed {
            let (cx, cy) = ((frame[0] + frame[2] / 2.0) as i32, (frame[1] + frame[3] / 2.0) as i32);
            super::refresh_at(cx, cy).map_or(16_666_667, |hz| 1_000_000_000 / hz.max(1) as u64)
        } else {
            0
        };
        Ok(Shot { img, scale, t0_ns: t0, t_ns: at, seq, late_ns })
    }

    /// Whether its frames come as events (Windows.Graphics.Capture, macOS's stream): `wait` follows them.
    pub fn evented(&self) -> bool {
        #[cfg(any(windows, target_os = "macos"))]
        return self.os.is_some();
        #[cfg(target_os = "linux")]
        false
    }

    /// Waits until the window's capture has a frame after the one numbered `seq`, at most until `until`:
    /// true when one came. Only Windows' and macOS's captures number their frames; elsewhere false at
    /// once, and the caller waits on the target's presents or a short poll.
    pub fn wait(&self, seq: u64, until: Instant) -> bool {
        #[cfg(any(windows, target_os = "macos"))]
        if let Some(w) = &self.os {
            return w.wait(seq, until);
        }
        let _ = (seq, until);
        false
    }

    /// The capture's frame count now (Windows.Graphics.Capture, macOS's stream); None where frames are
    /// not events.
    pub fn seq(&self) -> Option<u64> {
        #[cfg(any(windows, target_os = "macos"))]
        return self.os.as_ref().map(|w| w.seq());
        #[cfg(target_os = "linux")]
        None
    }

    /// strobe's CPU (ms) on the capture's own threads since the last call: its frame events copying each
    /// frame out of the pool (Windows) or keeping each frame (macOS), part of what its reads cost
    /// (`overhead.pixel_pct`).
    pub fn take_cpu_ms(&self) -> f64 {
        #[cfg(any(windows, target_os = "macos"))]
        return self.os.as_ref().map_or(0.0, |w| w.take_cpu_ms());
        #[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
mod sck {
    use crate::host::macos::{CFTypeRef, CGPoint, CGRect, CGSize, cf_array, cf_i64, cf_rect};
    use crate::model::{Error, Result};
    use crate::probe::present::sck::{
        BGRA, Block, CFRelease, CFRetain, CMSampleBufferGetImageBuffer, CMSampleBufferGetSampleAttachmentsArray, CMTime, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
        CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferUnlockBaseAddress, Id, SCStreamFrameInfoDirtyRects, SCStreamFrameInfoDisplayTime,
        SCStreamFrameInfoStatus, Sel, cf_dict_value, class, class_addIvar, class_addMethod, class_addProtocol, class_getInstanceVariable, description, dispatch_queue_create, dispatch_release,
        dispatch_sync_f, objc_allocateClassPair, objc_autoreleasePoolPop, objc_autoreleasePoolPush, objc_getProtocol, objc_registerClassPair, objc_release, object_getIvar, object_setIvar, rect_of,
        sel, send, with_completion,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// The newest frame that drew something: the frame itself (retained), when it was displayed, its size
    /// in pixels, and the count so far.
    #[derive(Default)]
    struct Latest {
        image: usize,
        t_ns: u64,
        size: (usize, usize),
        count: u64,
    }

    #[derive(Default)]
    struct Shared {
        latest: Mutex<Latest>,
        cv: Condvar,
        /// The frame callbacks' CPU so far (ns).
        cpu_ns: AtomicU64,
    }

    pub struct Stream {
        stream: Id,
        output: Id,
        queue: *mut c_void,
        shared: Arc<Shared>,
        /// The window's frame (global points), the part of it the stream's source rectangle is (points
        /// from the frame's top left: every region read so far), and the frame of the display it
        /// streams from.
        frame: [f64; 4],
        area: [f64; 4],
        display: CGRect,
        /// That display's pixels per point.
        scale: f64,
    }

    // SAFETY: the stream and its output are used from the thread that owns the grabber; the frame
    // callback reaches only `Shared`, through its own reference.
    unsafe impl Send for Stream {}

    /// `stream:didOutputSampleBuffer:ofType:` of strobe's region output class: keeps the frame (no copy)
    /// when it drew something, and wakes waiters.
    extern "C" fn on_frame(this: Id, _cmd: Sel, _stream: Id, sbuf: *mut c_void, kind: isize) {
        let cpu0 = crate::host::thread_cycles();
        let shared = unsafe { object_getIvar(this, class_getInstanceVariable(output_class(), c"sink".as_ptr())) } as *const Shared;
        if kind != 0 || sbuf.is_null() || shared.is_null() {
            return;
        }
        let shared = unsafe { &*shared };
        if let Some(&info) = cf_array(unsafe { CMSampleBufferGetSampleAttachmentsArray(sbuf, 0) }).first() {
            let status = cf_i64(unsafe { cf_dict_value(info, SCStreamFrameInfoStatus) });
            let shown = cf_i64(unsafe { cf_dict_value(info, SCStreamFrameInfoDisplayTime) });
            let rects = unsafe { cf_dict_value(info, SCStreamFrameInfoDirtyRects) };
            // A stream's first frame may be empty, its changed rectangles too: the window comes next.
            let drew = rects.is_null() || cf_array(rects).into_iter().filter_map(cf_rect).any(|r| r.size.width * r.size.height > 0.0);
            let image = unsafe { CMSampleBufferGetImageBuffer(sbuf) };
            // SCFrameStatusComplete: new content (an idle window sends none).
            if let (Some(0), Some(t), true, false) = (status, shown, drew, image.is_null()) {
                unsafe { CFRetain(image as CFTypeRef) };
                let size = unsafe { (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image)) };
                let old = {
                    let mut l = shared.latest.lock().unwrap_or_else(|e| e.into_inner());
                    l.count += 1;
                    l.t_ns = crate::host::macos::mach_ns(t as u64);
                    l.size = size;
                    std::mem::replace(&mut l.image, image as usize)
                };
                if old != 0 {
                    unsafe { CFRelease(old as CFTypeRef) };
                }
                shared.cv.notify_all();
            }
        }
        shared.cpu_ns.fetch_add(crate::host::thread_cycles().saturating_sub(cpu0), Ordering::Relaxed);
    }

    /// strobe's region SCStreamOutput class, made once.
    fn output_class() -> Id {
        static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *CLASS.get_or_init(|| unsafe {
            let existing = class("StrobeRegionOutput");
            if !existing.is_null() {
                return existing as usize;
            }
            let cls = objc_allocateClassPair(class("NSObject"), c"StrobeRegionOutput".as_ptr(), 0);
            class_addIvar(cls, c"sink".as_ptr(), std::mem::size_of::<usize>(), 3, c"^v".as_ptr());
            class_addMethod(cls, sel("stream:didOutputSampleBuffer:ofType:"), on_frame as *const c_void, c"v@:@^vq".as_ptr());
            let proto = objc_getProtocol(c"SCStreamOutput".as_ptr());
            if !proto.is_null() {
                class_addProtocol(cls, proto);
            }
            objc_registerClassPair(cls);
            cls as usize
        }) as Id
    }

    /// The error a completion handler handed back, released.
    fn taken(e: Id) -> String {
        let why = description(e);
        if !e.is_null() {
            unsafe { objc_release(e) };
        }
        why
    }

    impl Stream {
        /// A stream of `area` (points from the top left of the window's frame `frame`, global points) of
        /// window `win` alone on its display, started, once its first frame came (at most a second).
        pub fn open(win: isize, frame: [f64; 4], area: [f64; 4]) -> Result<Stream> {
            if !crate::host::macos::screen_capture_allowed() {
                return Err(Error::msg("no Screen Recording"));
            }
            let w = crate::host::window_state(win).ok_or_else(|| Error::msg("the window is gone"))?;
            if crate::host::macos::unshared_windows(&[w.pid]).iter().any(|u| u.0 as isize == win) {
                return Err(Error::msg("the app keeps the window out of screen capture"));
            }
            let pool = unsafe { objc_autoreleasePoolPush() };
            let r = Stream::start(win, frame, area, w.scale().max(1.0));
            unsafe { objc_autoreleasePoolPop(pool) };
            r
        }

        fn start(win: isize, frame: [f64; 4], area: [f64; 4], scale: f64) -> Result<Stream> {
            let content = with_completion(
                Duration::from_secs(5),
                false,
                |b| send!((); class("SCShareableContent"), "getShareableContentExcludingDesktopWindows:onScreenWindowsOnly:completionHandler:", i8 = 1, i8 = 1, *mut Block = b),
            );
            let Some((content, err)) = content else { return Err(Error::msg("ScreenCaptureKit did not list the windows in 5 s")) };
            if content.is_null() {
                return Err(Error::msg(format!("ScreenCaptureKit: {}", taken(err))));
            }
            let windows: Id = send!(Id; content, "windows");
            let found = (0..send!(usize; windows, "count")).map(|k| send!(Id; windows, "objectAtIndex:", usize = k)).find(|w| send!(u32; *w, "windowID") == win as u32);
            let displays: Id = send!(Id; content, "displays");
            let (cx, cy) = (frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0);
            let display = (0..send!(usize; displays, "count")).map(|k| send!(Id; displays, "objectAtIndex:", usize = k)).find(|d| inside(rect_of(*d, "frame"), cx, cy));
            let (Some(window), Some(display)) = (found, display) else {
                unsafe { objc_release(content) };
                return Err(Error::msg("ScreenCaptureKit does not list the window on a display"));
            };
            let only: Id = send!(Id; class("NSArray"), "arrayWithObject:", Id = window);
            let filter: Id = send!(Id; class("SCContentFilter"), "alloc");
            let filter: Id = send!(Id; filter, "initWithDisplay:includingWindows:", Id = display, Id = only);
            let dframe = rect_of(display, "frame");
            unsafe { objc_release(content) };
            let queue = unsafe { dispatch_queue_create(c"strobe.pixels".as_ptr(), std::ptr::null()) };
            let mut s = Stream { stream: std::ptr::null_mut(), output: std::ptr::null_mut(), queue, shared: Arc::default(), frame, area, display: dframe, scale };
            let config = s.config();
            let stream: Id = send!(Id; class("SCStream"), "alloc");
            let stream: Id = send!(Id; stream, "initWithFilter:configuration:delegate:", Id = filter, Id = config, Id = std::ptr::null_mut());
            unsafe {
                objc_release(filter);
                objc_release(config);
            }
            if stream.is_null() {
                return Err(Error::msg("ScreenCaptureKit made no stream for the window"));
            }
            s.stream = stream;
            s.output = send!(Id; output_class(), "new");
            unsafe { object_setIvar(s.output, class_getInstanceVariable(output_class(), c"sink".as_ptr()), Arc::into_raw(s.shared.clone()) as Id) };
            let mut err: Id = std::ptr::null_mut();
            if send!(i8; stream, "addStreamOutput:type:sampleHandlerQueue:error:", Id = s.output, isize = 0, *mut c_void = queue, *mut Id = &mut err) == 0 {
                return Err(Error::msg(format!("ScreenCaptureKit would not take strobe's output: {}", taken(err))));
            }
            match with_completion(Duration::from_secs(5), true, |b| send!((); stream, "startCaptureWithCompletionHandler:", *mut Block = b)) {
                Some((_, e)) if e.is_null() => {}
                Some((_, e)) => return Err(Error::msg(format!("ScreenCaptureKit did not start: {}", taken(e)))),
                None => return Err(Error::msg("ScreenCaptureKit did not start in 5 s")),
            }
            if !s.wait(0, Instant::now() + Duration::from_secs(1)) {
                return Err(Error::msg("ScreenCaptureKit delivered no frame of the window in a second"));
            }
            Ok(s)
        }

        /// The frames' size in pixels: the area at the display's scale.
        fn pixels(&self) -> (usize, usize) {
            ((self.area[2] * self.scale).round().max(1.0) as usize, (self.area[3] * self.scale).round().max(1.0) as usize)
        }

        /// A configuration for the area: its rectangle on the display at the display's pixels, BGRA, no
        /// cursor, a frame as often as it changes, and few frames queued (strobe keeps one). The window
        /// server composes and sends only that much, and only when it changed.
        fn config(&self) -> Id {
            let (f, a, d) = (self.frame, self.area, self.display);
            let source = CGRect { origin: CGPoint { x: f[0] + a[0] - d.origin.x, y: f[1] + a[1] - d.origin.y }, size: CGSize { width: a[2], height: a[3] } };
            let (w, h) = self.pixels();
            let config: Id = send!(Id; class("SCStreamConfiguration"), "alloc");
            let config: Id = send!(Id; config, "init");
            send!((); config, "setSourceRect:", CGRect = source);
            send!((); config, "setWidth:", usize = w);
            send!((); config, "setHeight:", usize = h);
            send!((); config, "setPixelFormat:", u32 = BGRA);
            send!((); config, "setShowsCursor:", i8 = 0);
            send!((); config, "setQueueDepth:", isize = 3);
            send!((); config, "setMinimumFrameInterval:", CMTime = CMTime { value: 1, timescale: 1000, flags: 1, epoch: 0 });
            config
        }

        /// Points the stream at `region` (points in the frame) of the window's frame now: the area grows
        /// to hold a region outside it, and follows a frame that moved or resized on its display. Waits
        /// for a frame of the new source.
        fn follow(&mut self, frame: [f64; 4], region: [f64; 4]) -> Result<()> {
            let a = self.area;
            let held = region[0] >= a[0] && region[1] >= a[1] && region[0] + region[2] <= a[0] + a[2] && region[1] + region[3] <= a[1] + a[3];
            if frame == self.frame && held {
                return Ok(());
            }
            if !inside(self.display, frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0) {
                return Err(Error::msg("the window went to another display"));
            }
            let (x0, y0) = (a[0].min(region[0]).max(0.0), a[1].min(region[1]).max(0.0));
            let (x1, y1) = ((a[0] + a[2]).max(region[0] + region[2]).min(frame[2]), (a[1] + a[3]).max(region[1] + region[3]).min(frame[3]));
            if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
                return Err(Error::msg("the region is outside the window's frame"));
            }
            let seq = self.seq();
            (self.frame, self.area) = (frame, [x0, y0, x1 - x0, y1 - y0]);
            let config = self.config();
            let stream = self.stream;
            let done = with_completion(Duration::from_secs(2), true, |b| send!((); stream, "updateConfiguration:completionHandler:", Id = config, *mut Block = b));
            unsafe { objc_release(config) };
            match done {
                Some((_, e)) if e.is_null() => {}
                Some((_, e)) => return Err(Error::msg(format!("ScreenCaptureKit did not follow the window: {}", taken(e)))),
                None => return Err(Error::msg("ScreenCaptureKit did not follow the window in 2 s")),
            }
            if !self.wait(seq, Instant::now() + Duration::from_millis(500)) {
                return Err(Error::msg("ScreenCaptureKit sent no frame of the window where it moved"));
            }
            Ok(())
        }

        /// `[x, y, w, h]` (whole points in the window's frame `frame`, global points now) of the newest
        /// frame: its pixels, display time and number.
        pub fn grab(&mut self, frame: [f64; 4], region: [f64; 4]) -> Result<(crate::png::Image, u64, u64)> {
            self.follow(frame, region)?;
            let want = self.pixels();
            let until = Instant::now() + Duration::from_millis(500);
            let (image, t_ns, count) = loop {
                let l = self.shared.latest.lock().unwrap_or_else(|e| e.into_inner());
                if l.image != 0 && l.size == want {
                    unsafe { CFRetain(l.image as CFTypeRef) };
                    break (l.image as *mut c_void, l.t_ns, l.count);
                }
                // A frame of the area's size before it changed: the next one.
                let seq = l.count;
                drop(l);
                if !self.wait(seq, until) {
                    return Err(Error::msg("ScreenCaptureKit sent no frame of the area's size"));
                }
            };
            // The region from the area's top left, in the frame's pixels.
            let k = want.0 as f64 / self.area[2].max(1.0);
            let [x, y, w, h] = [region[0] - self.area[0], region[1] - self.area[1], region[2], region[3]];
            let (x0, y0) = (((x * k).round().max(0.0) as usize).min(want.0), ((y * k).round().max(0.0) as usize).min(want.1));
            let (x1, y1) = ((((x + w) * k).round().max(0.0) as usize).min(want.0), (((y + h) * k).round().max(0.0) as usize).min(want.1));
            let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
            let read = unsafe { CVPixelBufferLockBaseAddress(image, 1) } == 0;
            if read {
                let (base, stride) = unsafe { (CVPixelBufferGetBaseAddress(image), CVPixelBufferGetBytesPerRow(image)) };
                if !base.is_null() && stride >= want.0 * 4 {
                    for row in y0..y1 {
                        let line = unsafe { std::slice::from_raw_parts(base.add(row * stride + x0 * 4), (x1 - x0) * 4) };
                        for p in line.as_chunks::<4>().0 {
                            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                        }
                    }
                }
                unsafe { CVPixelBufferUnlockBaseAddress(image, 1) };
            }
            unsafe { CFRelease(image as CFTypeRef) };
            if rgba.len() != (x1 - x0) * (y1 - y0) * 4 || rgba.is_empty() {
                return Err(Error::msg("ScreenCaptureKit's frame of the window could not be read"));
            }
            Ok((crate::png::Image { width: x1 - x0, height: y1 - y0, rgba }, t_ns, count))
        }

        /// The frames kept so far.
        pub fn seq(&self) -> u64 {
            self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).count
        }

        /// The frame callbacks' CPU (ms) since the last call.
        pub fn take_cpu_ms(&self) -> f64 {
            self.shared.cpu_ns.swap(0, Ordering::Relaxed) as f64 / 1e6
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
    }

    fn inside(r: CGRect, x: f64, y: f64) -> bool {
        x >= r.origin.x && x < r.origin.x + r.size.width && y >= r.origin.y && y < r.origin.y + r.size.height
    }

    /// Stops the stream and drains its queue (no callback runs after), then lets the kept frame go.
    impl Drop for Stream {
        fn drop(&mut self) {
            let pool = unsafe { objc_autoreleasePoolPush() };
            let stream = self.stream;
            if !stream.is_null() {
                let _ = with_completion(Duration::from_secs(2), true, |b| send!((); stream, "stopCaptureWithCompletionHandler:", *mut Block = b));
            }
            unsafe {
                if !self.output.is_null() {
                    let ivar = class_getInstanceVariable(output_class(), c"sink".as_ptr());
                    let sink = object_getIvar(self.output, ivar) as *const Shared;
                    object_setIvar(self.output, ivar, std::ptr::null_mut());
                    // The queue is serial: once an empty call queued now has run, every frame before it has.
                    extern "C" fn nothing(_: *mut c_void) {}
                    dispatch_sync_f(self.queue, std::ptr::null_mut(), nothing);
                    if !sink.is_null() {
                        drop(Arc::from_raw(sink));
                    }
                    objc_release(self.output);
                }
                if !stream.is_null() {
                    objc_release(stream);
                }
                dispatch_release(self.queue);
                let image = std::mem::take(&mut self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).image);
                if image != 0 {
                    CFRelease(image as CFTypeRef);
                }
                objc_autoreleasePoolPop(pool);
            }
        }
    }
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
