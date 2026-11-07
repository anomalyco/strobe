//! strobe's own client of the compositor it runs (`super`): the output's pixels through
//! `zwlr_screencopy_manager_v1` into a reusable shared-memory buffer, and input through a virtual
//! pointer (`zwlr_virtual_pointer_v1`) and a virtual keyboard (`zwp_virtual_keyboard_v1`), each a
//! device of the compositor's seat: the compositor hit-tests the pointer and sends keys to the
//! window with the seat's focus, the path a person's mouse and keyboard take.

use crate::model::{Error, Result};
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_callback, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1};

/// What one screencopy frame said so far.
#[derive(Default)]
struct Frame {
    /// Shared-memory formats it offered: (format, width, height, stride).
    shm: Vec<(wl_shm::Format, u32, u32, u32)>,
    /// All buffer offers are in (version 3), or the one offer came (earlier versions).
    offered: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    frame: Frame,
    /// The frame being copied.
    current: Option<wayland_client::backend::ObjectId>,
    synced: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(state: &mut Self, _: &wl_callback::WlCallback, _: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.synced = true;
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(state: &mut Self, frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, event: zwlr_screencopy_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_screencopy_frame_v1::Event;
        // A frame given up on (its deadline passed) may still answer: only the current one counts.
        if state.current.as_ref() != Some(&frame.id()) {
            return;
        }
        let f = &mut state.frame;
        match event {
            Event::Buffer { format: WEnum::Value(format), width, height, stride } => {
                f.shm.push((format, width, height, stride));
                // Before version 3 the one offer is all there is.
                if frame.version() < 3 {
                    f.offered = true;
                }
            }
            Event::BufferDone => f.offered = true,
            Event::Flags { flags: WEnum::Value(flags) } => f.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert),
            Event::Ready { .. } => f.ready = true,
            Event::Failed => f.failed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wl_output::WlOutput);
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ZwlrScreencopyManagerV1);
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);

/// The shared-memory buffer frames are copied into, kept while the frames keep its shape.
struct Shm {
    file: File,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    shape: (wl_shm::Format, u32, u32, u32),
}

/// One copy of an output region: BGRA rows top down, and when it was ready (host clock).
pub struct Grab {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
    pub at_ns: u64,
}

pub struct Client {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    globals: GlobalList,
    shm: wl_shm::WlShm,
    output: wl_output::WlOutput,
    seat: wl_seat::WlSeat,
    screencopy: Option<ZwlrScreencopyManagerV1>,
    buffer: Option<Shm>,
    pointer: Option<ZwlrVirtualPointerV1>,
    keyboard: Option<ZwpVirtualKeyboardV1>,
}

fn wl_err(what: &str) -> impl Fn(String) -> Error + '_ {
    move |e| Error::msg(format!("Wayland {what}: {e}"))
}

/// Milliseconds on the host clock, as Wayland input events carry them.
fn ms() -> u32 {
    (crate::host::now_ns() / 1_000_000) as u32
}

fn memfd(len: usize) -> Result<File> {
    let fd = unsafe { libc::memfd_create(c"strobe-wayland".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(Error::msg(format!("Wayland: memfd_create: {}", std::io::Error::last_os_error())));
    }
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    file.set_len(len as u64)?;
    Ok(file)
}

impl Client {
    /// Connects to the compositor's socket and binds what strobe uses of it.
    pub fn connect(socket: &Path) -> Result<Client> {
        let stream = UnixStream::connect(socket).map_err(|e| Error::msg(format!("Wayland: connect to {}: {e}", socket.display())))?;
        let conn = Connection::from_socket(stream).map_err(|e| wl_err("connect")(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| wl_err("registry")(e.to_string()))?;
        let qh = queue.handle();
        let bind = |what: &str| Error::msg(format!("Wayland: the compositor has no {what}"));
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| bind("wl_shm"))?;
        let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).map_err(|_| bind("wl_output"))?;
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).map_err(|_| bind("wl_seat"))?;
        Ok(Client { conn, queue, qh, state: State::default(), globals, shm, output, seat, screencopy: None, buffer: None, pointer: None, keyboard: None })
    }

    /// Dispatches events until `done` or `deadline`, waiting on the socket between reads.
    fn pump(&mut self, deadline: Instant, done: impl Fn(&State) -> bool) -> Result<bool> {
        loop {
            self.queue.dispatch_pending(&mut self.state).map_err(|e| wl_err("dispatch")(e.to_string()))?;
            if done(&self.state) {
                return Ok(true);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            self.queue.flush().map_err(|e| wl_err("flush")(e.to_string()))?;
            let Some(guard) = self.queue.prepare_read() else { continue };
            let mut fds = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let n = unsafe { libc::poll(&mut fds, 1, left.as_millis().clamp(1, i32::MAX as u128) as i32) };
            if n > 0 {
                guard.read().map_err(|e| wl_err("read")(e.to_string()))?;
            }
        }
    }

    fn flush(&self) -> Result<()> {
        self.queue.flush().map_err(|e| wl_err("flush")(e.to_string()))
    }

    /// A copy of the output's region `rect` (x, y, width, height in the output's coordinates). With
    /// `damage`, the copy waits for the region to change first (`copy_with_damage`); without `read`,
    /// only when it was ready is kept (no pixels). None when the compositor gave nothing before `deadline`.
    pub fn grab(&mut self, rect: [i32; 4], damage: bool, read: bool, deadline: Instant) -> Result<Option<Grab>> {
        let manager = match &self.screencopy {
            Some(m) => m.clone(),
            None => {
                let m: ZwlrScreencopyManagerV1 =
                    self.globals.bind(&self.qh, 1..=3, ()).map_err(|_| Error::msg("the compositor has no zwlr_screencopy_manager_v1: strobe reads its pixels through it"))?;
                self.screencopy = Some(m.clone());
                m
            }
        };
        self.state.frame = Frame::default();
        let frame = manager.capture_output_region(0, &self.output, rect[0], rect[1], rect[2].max(1), rect[3].max(1), &self.qh, ());
        self.state.current = Some(frame.id());
        let result = self.copy(&frame, damage, read, deadline);
        self.state.current = None;
        frame.destroy();
        let _ = self.flush();
        result
    }

    fn copy(&mut self, frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, damage: bool, read: bool, deadline: Instant) -> Result<Option<Grab>> {
        if !self.pump(deadline, |s| s.frame.offered || s.frame.failed)? || self.state.frame.failed {
            return Ok(None);
        }
        // Little-endian 32-bit pixels: B, G, R, A in memory, or R, G, B, A for the `*bgr*` formats.
        use wl_shm::Format::{Abgr8888, Argb8888, Xbgr8888, Xrgb8888};
        let Some(&shape) = self.state.frame.shm.iter().find(|s| matches!(s.0, Argb8888 | Xrgb8888 | Abgr8888 | Xbgr8888)) else {
            return Err(Error::msg("Wayland screencopy: the compositor offered no 32-bit RGB shared-memory format"));
        };
        let (format, width, height, stride) = shape;
        let len = stride as usize * height as usize;
        if self.buffer.as_ref().is_none_or(|b| b.shape != shape) {
            if let Some(old) = self.buffer.take() {
                old.buffer.destroy();
                old.pool.destroy();
            }
            let file = memfd(len)?;
            let pool = self.shm.create_pool(file.as_fd(), len as i32, &self.qh, ());
            let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, &self.qh, ());
            self.buffer = Some(Shm { file, pool, buffer, shape });
        }
        let Some(shm) = &self.buffer else { return Ok(None) };
        if damage {
            frame.copy_with_damage(&shm.buffer);
        } else {
            frame.copy(&shm.buffer);
        }
        if !self.pump(deadline, |s| s.frame.ready || s.frame.failed)? || self.state.frame.failed {
            return Ok(None);
        }
        let at_ns = crate::host::now_ns();
        let (w, h) = (width as usize, height as usize);
        if !read {
            return Ok(Some(Grab { width: w, height: h, bgra: Vec::new(), at_ns }));
        }
        let Some(shm) = &self.buffer else { return Ok(None) };
        let mut raw = vec![0u8; len];
        shm.file.read_exact_at(&mut raw, 0)?;
        let mut bgra = Vec::with_capacity(w * h * 4);
        let rgb = matches!(format, Abgr8888 | Xbgr8888);
        for y in 0..h {
            let row = if self.state.frame.y_invert { h - 1 - y } else { y };
            for px in raw[row * stride as usize..row * stride as usize + w * 4].as_chunks::<4>().0 {
                bgra.extend_from_slice(&if rgb { [px[2], px[1], px[0], 255] } else { [px[0], px[1], px[2], 255] });
            }
        }
        Ok(Some(Grab { width: w, height: h, bgra, at_ns }))
    }

    fn pointer(&mut self) -> Result<&ZwlrVirtualPointerV1> {
        if self.pointer.is_none() {
            let manager: ZwlrVirtualPointerManagerV1 =
                self.globals.bind(&self.qh, 1..=2, ()).map_err(|_| Error::msg("the compositor has no zwlr_virtual_pointer_manager_v1: strobe's pointer input goes through it"))?;
            let p = if manager.version() >= 2 {
                manager.create_virtual_pointer_with_output(Some(&self.seat), Some(&self.output), &self.qh, ())
            } else {
                manager.create_virtual_pointer(Some(&self.seat), &self.qh, ())
            };
            self.pointer = Some(p);
        }
        self.pointer.as_ref().ok_or_else(|| Error::msg("Wayland: no virtual pointer"))
    }

    /// Adds strobe's pointer to the seat (no motion yet).
    pub fn add_pointer(&mut self) -> Result<()> {
        self.pointer()?;
        self.flush()
    }

    /// The pointer to `(x, y)` on the output of size `extent`; returns when it was sent.
    pub fn pointer_to(&mut self, x: f64, y: f64, extent: (u32, u32)) -> Result<u64> {
        let p = self.pointer()?;
        let at = crate::host::now_ns();
        p.motion_absolute(ms(), x.round().max(0.0) as u32, y.round().max(0.0) as u32, extent.0, extent.1);
        p.frame();
        self.flush()?;
        Ok(at)
    }

    /// A button edge (`BTN_LEFT` 0x110 and on); returns when it was sent.
    pub fn button(&mut self, button: u32, down: bool) -> Result<u64> {
        let p = self.pointer()?;
        let at = crate::host::now_ns();
        p.button(ms(), button, if down { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released });
        p.frame();
        self.flush()?;
        Ok(at)
    }

    /// A continuous scroll of `dx`, `dy` surface pixels (+ is right and down) in one frame.
    pub fn scroll(&mut self, dx: f64, dy: f64) -> Result<u64> {
        let p = self.pointer()?;
        let at = crate::host::now_ns();
        let t = ms();
        p.axis_source(wl_pointer::AxisSource::Continuous);
        if dy != 0.0 {
            p.axis(t, wl_pointer::Axis::VerticalScroll, dy);
        }
        if dx != 0.0 {
            p.axis(t, wl_pointer::Axis::HorizontalScroll, dx);
        }
        p.frame();
        self.flush()?;
        Ok(at)
    }

    /// Gives the virtual keyboard `keymap` (XKB text), making the keyboard first.
    pub fn keymap(&mut self, keymap: &str) -> Result<()> {
        if self.keyboard.is_none() {
            let manager: ZwpVirtualKeyboardManagerV1 =
                self.globals.bind(&self.qh, 1..=1, ()).map_err(|_| Error::msg("the compositor has no zwp_virtual_keyboard_manager_v1: strobe's keys go through it"))?;
            self.keyboard = Some(manager.create_virtual_keyboard(&self.seat, &self.qh, ()));
        }
        let bytes = keymap.as_bytes();
        let file = memfd(bytes.len() + 1)?;
        file.write_all_at(bytes, 0)?;
        let kb = self.keyboard.as_ref().ok_or_else(|| Error::msg("Wayland: no virtual keyboard"))?;
        // Format 1: XKB v1 text, NUL-terminated.
        kb.keymap(1, file.as_fd(), bytes.len() as u32 + 1);
        self.sync(Instant::now() + Duration::from_secs(2))
    }

    /// Waits until the compositor has handled every request sent so far (`wl_display.sync`).
    pub fn sync(&mut self, deadline: Instant) -> Result<()> {
        self.state.synced = false;
        self.conn.display().sync(&self.qh, ());
        if self.pump(deadline, |s| s.synced)? { Ok(()) } else { Err(Error::msg("Wayland: the compositor did not answer within 2 s")) }
    }

    /// The modifiers held (XKB's real modifier bits), which a virtual keyboard sets itself: the
    /// compositor does not work them out from its key events.
    pub fn modifiers(&mut self, depressed: u32) -> Result<()> {
        let kb = self.keyboard.as_ref().ok_or_else(|| Error::msg("Wayland: the virtual keyboard has no keymap yet"))?;
        kb.modifiers(depressed, 0, 0, 0);
        self.flush()
    }

    /// A key edge by its evdev code; returns when it was sent.
    pub fn key(&mut self, code: u32, down: bool) -> Result<u64> {
        let kb = self.keyboard.as_ref().ok_or_else(|| Error::msg("Wayland: the virtual keyboard has no keymap yet"))?;
        let at = crate::host::now_ns();
        kb.key(ms(), code, u32::from(down));
        self.flush()?;
        Ok(at)
    }
}
