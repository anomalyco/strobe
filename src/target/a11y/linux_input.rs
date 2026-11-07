//! Input to a native app on Linux (SPEC §13.7): XTest on the session's X server (`xtest`), or the seat
//! of the Wayland compositor strobe runs for the launch (`wl_input`), whichever holds the window.

pub use super::keys::{Button, Key, Mods, Named, parse_chord};
use super::{wl_input, xtest};
use crate::host::wayland::owns;
use crate::model::Result;
use std::sync::Mutex;

/// Where a pointer input lands: the app's window at that screen point.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub win: isize,
    pub x: i32,
    pub y: i32,
}

impl Target {
    /// The same window at another screen point: the server sends the held button's motion to the
    /// window that has it.
    pub fn moved(self, x: f64, y: f64) -> Target {
        Target { x: x.round() as i32, y: y.round() as i32, ..self }
    }

    /// The top-level window the point hit-tested to.
    pub fn top(&self) -> isize {
        self.win
    }

    /// The screen point, in whole pixels.
    pub fn point(&self) -> (i64, i64) {
        (self.x as i64, self.y as i64)
    }
}

/// Gives the seat of a compositor strobe runs its keyboard and pointer, before the app starts there.
pub fn prepare_seat(c: &crate::host::wayland::Compositor) -> Result<()> {
    wl_input::prepare_seat(c)
}

/// The server knows which button strobe's device holds: motion carries it without a flag.
pub fn held_flag(_b: Button) -> usize {
    0
}

/// Input to whichever holds each window: the window's id says which (`host::wayland::owns`), and a
/// point is the app's on the compositor that shows its windows, else the X server's. XTest connects
/// when an X11 window is first driven.
pub struct Poster {
    x11: Mutex<Option<Box<xtest::Poster>>>,
    wayland: wl_input::Poster,
}

impl Poster {
    pub fn start() -> Result<Poster> {
        Ok(Poster { x11: Mutex::new(None), wayland: wl_input::Poster::new() })
    }

    /// Runs `f` on the X11 poster, connected on first use.
    fn x11<T>(&self, f: impl FnOnce(&xtest::Poster) -> Result<T>) -> Result<T> {
        let mut g = self.x11.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(Box::new(xtest::Poster::start()?));
        }
        match g.as_deref() {
            Some(p) => f(p),
            None => Err(crate::model::Error::msg("X11 input did not start")),
        }
    }

    pub fn target(&self, pids: &[u32], x: f64, y: f64) -> Result<Target> {
        match crate::host::wayland::showing(pids) {
            Some(c) => self.wayland.target(&c, pids, x, y),
            None => self.x11(|p| p.target(pids, x, y)),
        }
    }

    pub fn client_point(&self, top: isize, x: f64, y: f64) -> Result<(f64, f64)> {
        if owns(top) { self.wayland.client_point(top, x, y) } else { self.x11(|p| p.client_point(top, x, y)) }
    }

    pub fn frame(&self, top: isize) -> Result<(f64, f64, f64, f64)> {
        if owns(top) { self.wayland.frame(top) } else { self.x11(|p| p.frame(top)) }
    }

    /// Remembers where the user left the X11 cursor (strobe's compositor's pointer is strobe's own).
    pub fn park(&self, x: f64, y: f64) -> Result<bool> {
        match self.x11.lock().unwrap_or_else(|e| e.into_inner()).as_deref() {
            Some(p) => p.park(x, y),
            None => self.wayland.park(x, y),
        }
    }

    pub fn move_to(&self, t: Target, buttons: usize) -> Result<u64> {
        if owns(t.win) { self.wayland.move_to(t, buttons) } else { self.x11(|p| p.move_to(t, buttons)) }
    }

    pub fn button(&self, t: Target, b: Button, down: bool, dbl: bool, mods: Mods, top: isize) -> Result<u64> {
        if owns(t.win) { self.wayland.button(t, b, down, dbl, mods, top) } else { self.x11(|p| p.button(t, b, down, dbl, mods, top)) }
    }

    pub fn wheel(&self, t: Target, dx: f64, dy: f64) -> Result<u64> {
        if owns(t.win) { self.wayland.wheel(t, dx, dy) } else { self.x11(|p| p.wheel(t, dx, dy)) }
    }

    pub fn press(&self, top: isize, to: Option<isize>, mods: Mods, key: Key) -> Result<u64> {
        if owns(top) { self.wayland.press(top, to, mods, key) } else { self.x11(|p| p.press(top, to, mods, key)) }
    }

    pub fn hold(&self, top: isize, key: Key) -> Result<u64> {
        if owns(top) { self.wayland.hold(top, key) } else { self.x11(|p| p.hold(top, key)) }
    }

    pub fn release(&self, top: isize, key: Key) -> Result<u64> {
        if owns(top) { self.wayland.release(top, key) } else { self.x11(|p| p.release(top, key)) }
    }

    pub fn release_all(&self, top: isize) {
        self.wayland.release_all();
        if let Some(p) = self.x11.lock().unwrap_or_else(|e| e.into_inner()).as_deref() {
            p.release_all(top);
        }
    }

    pub fn window(&self, top: isize, action: &str) -> Result<u64> {
        if owns(top) { self.wayland.window(top, action) } else { self.x11(|p| p.window(top, action)) }
    }
}
