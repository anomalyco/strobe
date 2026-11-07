//! Input to a native app on Wayland (SPEC §13.7), in the compositor strobe runs for its launch
//! (`target::wayland`): a virtual pointer and a virtual keyboard on the compositor's seat, so the
//! compositor hit-tests the pointer, gives the window under it the motion and buttons, and sends keys
//! to the window with the seat's focus, the path a person's devices take. Each input is stamped on
//! the host clock just before it is written.
//!
//! - The keyboard is a US layout strobe writes itself (XKB text, real evdev key codes, so an app
//!   that reads key codes sees a US keyboard's), with modifier keys that set the modifiers as a
//!   person's do. A character a US keyboard has no key for goes to one of a few spare keys the
//!   layout maps to it for the moment (the layout is sent again when that changes).
//! - Before keys strobe gives the app's window the seat's focus and refuses when it does not take it.
//! - The wheel scrolls continuously, by the pixels asked for, in one pointer frame per step.

use super::input::Target;
use super::keys::{Button, Key, Mods, Named, char_keysym, keysym, us_key, us_shifted};
use crate::host::wayland::{self as wl, Compositor};
use crate::model::{Error, Result};
use std::sync::{Arc, Mutex};

/// evdev's codes for the keys of a US keyboard that types `c` unshifted.
fn evdev_char(c: char) -> Option<u32> {
    const ROWS: &[(&str, u32)] = &[("1234567890-=", 2), ("qwertyuiop[]", 16), ("asdfghjkl;'`", 30), ("zxcvbnm,./", 44)];
    if c == ' ' {
        return Some(57);
    }
    if c == '\\' {
        return Some(43);
    }
    ROWS.iter().find_map(|(row, first)| row.chars().position(|k| k == c).map(|i| first + i as u32))
}

/// evdev's code for a named key.
fn evdev_named(n: Named) -> Option<u32> {
    Some(match n {
        Named::Enter => 28,
        Named::Tab => 15,
        Named::Escape => 1,
        Named::Backspace => 14,
        Named::Delete => 111,
        Named::Insert => 110,
        Named::Home => 102,
        Named::End => 107,
        Named::PageUp => 104,
        Named::PageDown => 109,
        Named::ArrowUp => 103,
        Named::ArrowDown => 108,
        Named::ArrowLeft => 105,
        Named::ArrowRight => 106,
        Named::Space => 57,
        Named::Shift => 42,
        Named::Control => 29,
        Named::Alt => 56,
        Named::Meta => 125,
        Named::ContextMenu => 127,
        Named::CapsLock => 58,
        Named::F(k @ 1..=10) => 58 + k as u32,
        Named::F(11) => 87,
        Named::F(12) => 88,
        Named::F(k @ 13..=24) => 170 + k as u32,
        Named::F(_) => return None,
        Named::Letter(c) | Named::Digit(c) => return evdev_char(c.to_ascii_lowercase() as char),
    })
}

/// Spare keys for characters a US keyboard has none for (evdev codes no keyboard sends).
const SPARE: std::ops::Range<u32> = 0x2e0..0x2f0;

/// The XKB keymap: every key of `named` and the US rows, then the spare keys with `spare`'s keysyms.
fn keymap(spare: &[u32]) -> String {
    let mut keys: Vec<(u32, Vec<u32>)> = Vec::new();
    for c in "1234567890-=qwertyuiop[]asdfghjkl;'`\\zxcvbnm,./ ".chars() {
        if let Some(code) = evdev_char(c) {
            let shifted = us_shifted(c);
            keys.push((code, if shifted == c { vec![char_keysym(c)] } else { vec![char_keysym(c), char_keysym(shifted)] }));
        }
    }
    let named = [
        Named::Enter,
        Named::Tab,
        Named::Escape,
        Named::Backspace,
        Named::Delete,
        Named::Insert,
        Named::Home,
        Named::End,
        Named::PageUp,
        Named::PageDown,
        Named::ArrowUp,
        Named::ArrowDown,
        Named::ArrowLeft,
        Named::ArrowRight,
        Named::Shift,
        Named::Control,
        Named::Alt,
        Named::Meta,
        Named::ContextMenu,
        Named::CapsLock,
    ];
    for n in named.into_iter().chain((1..=24).map(Named::F)) {
        if let Some(code) = evdev_named(n) {
            keys.push((code, vec![keysym(n)]));
        }
    }
    for (k, sym) in SPARE.zip(spare) {
        keys.push((k, vec![*sym]));
    }
    let mut out = String::from("xkb_keymap {\nxkb_keycodes \"strobe\" {\nminimum = 8;\nmaximum = 1023;\n");
    for (code, _) in &keys {
        out += &format!("<I{0}> = {0};\n", code + 8);
    }
    out += "};\nxkb_types \"strobe\" { include \"complete\" };\nxkb_compat \"strobe\" { include \"complete\" };\nxkb_symbols \"strobe\" {\n";
    for (code, syms) in &keys {
        out += &format!("key <I{}> {{ [ {} ] }};\n", code + 8, syms.iter().map(|s| format!("0x{s:x}")).collect::<Vec<_>>().join(", "));
    }
    for (m, n) in [("Shift", Named::Shift), ("Control", Named::Control), ("Mod1", Named::Alt), ("Mod4", Named::Meta), ("Lock", Named::CapsLock)] {
        out += &format!("modifier_map {m} {{ <I{}> }};\n", evdev_named(n).unwrap_or(0) + 8);
    }
    out += "};\n};\n";
    out
}

#[derive(Default)]
struct State {
    /// Keys held across calls (`page.down`) and modifiers a click pressed, as evdev codes.
    held: Vec<u32>,
    click_mods: Vec<u32>,
    /// Buttons down.
    buttons: Vec<u32>,
    /// The keysym on each spare key, and the next spare key to take.
    spare: Vec<u32>,
    next: usize,
    /// The modifier keys down, as XKB's modifier bits.
    mods: u32,
}

/// XKB's modifier bit a modifier key sets: Shift, Control, Mod1 (Alt) and Mod4 (Super).
fn modifier_bit(code: u32) -> Option<u32> {
    match code {
        42 => Some(1),
        29 => Some(4),
        56 => Some(8),
        125 => Some(64),
        _ => None,
    }
}

pub struct Poster {
    /// The compositor of the windows driven, once one is (its keyboard has strobe's keymap from then).
    c: Mutex<Option<Arc<Compositor>>>,
    state: Mutex<State>,
}

fn button_code(b: Button) -> u32 {
    match b {
        Button::Left => 0x110,
        Button::Right => 0x111,
        Button::Middle => 0x112,
    }
}

/// Gives a compositor's seat strobe's keyboard (the keymap a poster starts with) and pointer, before
/// the app launches on it.
pub fn prepare_seat(c: &Compositor) -> Result<()> {
    c.seat(&keymap(&vec![0x20; SPARE.len()]))
}

impl Poster {
    pub fn new() -> Poster {
        Poster { c: Mutex::new(None), state: Mutex::new(State { spare: vec![0x20; SPARE.len()], ..State::default() }) }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The compositor of window `win`, its keyboard given strobe's keymap when it is new to this poster.
    fn bind(&self, win: isize) -> Result<Arc<Compositor>> {
        let c = wl::compositor_of(win).ok_or_else(|| Error::msg("the app's window closed"))?;
        let mut bound = self.c.lock().unwrap_or_else(|e| e.into_inner());
        if bound.as_ref().is_none_or(|b| !Arc::ptr_eq(b, &c)) {
            c.keymap(&keymap(&self.st().spare))?;
            *bound = Some(c.clone());
        }
        Ok(c)
    }

    /// A key edge; a modifier key's also sets the modifiers, after it, as a keyboard's does (a virtual
    /// keyboard's keys leave them to its client). Returns when the key was sent.
    fn key(&self, c: &Compositor, st: &mut State, code: u32, down: bool) -> Result<u64> {
        let at = c.key(code, down)?;
        if let Some(bit) = modifier_bit(code) {
            st.mods = if down { st.mods | bit } else { st.mods & !bit };
            c.modifiers(st.mods)?;
        }
        Ok(at)
    }

    /// A key's evdev code and the modifiers it needs: a US key, Shift for its shifted character, or a
    /// spare key mapped to the character now (the keymap goes again, and with it the modifiers held).
    fn resolve(&self, comp: &Compositor, st: &mut State, key: Key, mods: Mods) -> Result<(u32, Mods)> {
        match key {
            Key::Named(n) => Ok((evdev_named(n).ok_or_else(|| Error::usage(format!("strobe's Wayland keyboard has no key for {n:?}")))?, mods)),
            Key::Char('\n' | '\r') => Ok((28, mods)),
            Key::Char('\t') => Ok((15, mods)),
            Key::Char(c) => {
                if let Some((base, shift)) = us_key(c)
                    && let Some(code) = evdev_char(base)
                {
                    return Ok((code, mods.or(Mods { shift, ..Mods::default() })));
                }
                let sym = char_keysym(c);
                let i = match st.spare.iter().position(|s| *s == sym) {
                    Some(i) => i,
                    None => {
                        let i = st.next % SPARE.len();
                        st.next += 1;
                        st.spare[i] = sym;
                        comp.keymap(&keymap(&st.spare))?;
                        i
                    }
                };
                Ok((SPARE.start + i as u32, mods))
            }
        }
    }

    /// The app's window at layout point (x, y) of compositor `c` (the one showing the app), or why a
    /// click there would reach something else.
    pub fn target(&self, c: &Compositor, pids: &[u32], x: f64, y: f64) -> Result<Target> {
        let (x, y) = (x.round() as i32, y.round() as i32);
        let (win, pid, title) = c.window_at(x, y).ok_or_else(|| Error::msg(format!("no window at ({x}, {y})")))?;
        if !pids.contains(&pid) {
            return Err(Error::refused("WindowHidden", format!("another window covers the point ({x}, {y}): {title:?} of process {pid}"), "keep the app's window uncovered while strobe drives it"));
        }
        Ok(Target { win, x, y })
    }

    /// A point in `top`'s client area (device-independent pixels, one per layout unit) on the layout.
    pub fn client_point(&self, top: isize, x: f64, y: f64) -> Result<(f64, f64)> {
        let (ox, oy) = wl::client_origin(top).ok_or_else(|| Error::msg("the app's window closed"))?;
        Ok((ox as f64 + x, oy as f64 + y))
    }

    pub fn frame(&self, top: isize) -> Result<(f64, f64, f64, f64)> {
        let r = wl::window_state(top).ok_or_else(|| Error::msg("the app's window closed"))?.rect;
        Ok((r.0 as f64, r.1 as f64, r.2 as f64, r.3 as f64))
    }

    /// The seat's pointer is strobe's own: nothing of a person's to remember.
    pub fn park(&self, _x: f64, _y: f64) -> Result<bool> {
        Ok(true)
    }

    pub fn move_to(&self, t: Target, _buttons: usize) -> Result<u64> {
        self.bind(t.win)?.pointer_to(t.x as f64, t.y as f64)
    }

    fn mod_keys(&self, mods: Mods) -> Vec<u32> {
        mods.keys().into_iter().filter_map(evdev_named).collect()
    }

    /// A button edge at `t`, after a move there. Modifiers go down before the press and up after the
    /// release. Returns when the button was sent.
    pub fn button(&self, t: Target, b: Button, down: bool, _dbl: bool, mods: Mods, _top: isize) -> Result<u64> {
        let c = self.bind(t.win)?;
        let mut st = self.st();
        c.pointer_to(t.x as f64, t.y as f64)?;
        if down {
            for k in self.mod_keys(mods) {
                if !st.held.contains(&k) {
                    self.key(&c, &mut st, k, true)?;
                    st.click_mods.push(k);
                }
            }
        }
        let code = button_code(b);
        let at = c.button(code, down)?;
        if down {
            st.buttons.push(code);
        } else {
            st.buttons.retain(|x| *x != code);
            for k in std::mem::take(&mut st.click_mods).into_iter().rev() {
                self.key(&c, &mut st, k, false)?;
            }
        }
        Ok(at)
    }

    /// A scroll of `dx`, `dy` pixels at `t` (+ is right and down). Returns when it was sent.
    pub fn wheel(&self, t: Target, dx: f64, dy: f64) -> Result<u64> {
        let c = self.bind(t.win)?;
        c.pointer_to(t.x as f64, t.y as f64)?;
        c.scroll(dx, dy)
    }

    /// Gives `top` the seat's focus, or refuses.
    fn activate(&self, top: isize) -> Result<()> {
        if wl::focus(top)? {
            return Ok(());
        }
        Err(Error::refused(
            "WindowHidden",
            "keys go to the window with the seat's focus, and the app's window did not take it in the compositor strobe runs",
            "let the app's window be focused while strobe types into it",
        ))
    }

    /// Presses and releases a key with modifiers in `top` (given the focus first). Returns when the
    /// key went down.
    pub fn press(&self, top: isize, _to: Option<isize>, mods: Mods, key: Key) -> Result<u64> {
        let c = self.bind(top)?;
        self.activate(top)?;
        let mut st = self.st();
        let (code, mods) = self.resolve(&c, &mut st, key, mods)?;
        let downs: Vec<u32> = self.mod_keys(mods).into_iter().filter(|k| !st.held.contains(k)).collect();
        for &k in &downs {
            self.key(&c, &mut st, k, true)?;
        }
        let at = self.key(&c, &mut st, code, true)?;
        self.key(&c, &mut st, code, false)?;
        for &k in downs.iter().rev() {
            self.key(&c, &mut st, k, false)?;
        }
        Ok(at)
    }

    /// Holds a key across calls (`page.down`) until `release`.
    pub fn hold(&self, top: isize, key: Key) -> Result<u64> {
        let c = self.bind(top)?;
        self.activate(top)?;
        let mut st = self.st();
        let (code, _) = self.resolve(&c, &mut st, key, Mods::default())?;
        if st.held.contains(&code) {
            return Ok(crate::host::now_ns());
        }
        let at = self.key(&c, &mut st, code, true)?;
        st.held.push(code);
        Ok(at)
    }

    pub fn release(&self, top: isize, key: Key) -> Result<u64> {
        let c = self.bind(top)?;
        let mut st = self.st();
        let (code, _) = self.resolve(&c, &mut st, key, Mods::default())?;
        let Some(i) = st.held.iter().position(|k| *k == code) else { return Ok(crate::host::now_ns()) };
        st.held.remove(i);
        self.key(&c, &mut st, code, false)
    }

    /// Releases every held key and button (a block's end), on the compositor it drove.
    pub fn release_all(&self) {
        let Some(c) = self.c.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        let mut st = self.st();
        for b in std::mem::take(&mut st.buttons) {
            let _ = c.button(b, false);
        }
        let keys: Vec<u32> = std::mem::take(&mut st.held).into_iter().rev().chain(std::mem::take(&mut st.click_mods)).collect();
        for k in keys {
            let _ = self.key(&c, &mut st, k, false);
        }
    }

    /// `show`, `hide`, `minimize`, `focus` or `close` through the compositor; returns when it was asked.
    pub fn window(&self, top: isize, action: &str) -> Result<u64> {
        let at = crate::host::now_ns();
        match action {
            "focus" => self.activate(top)?,
            other => wl::window_action(top, other)?,
        }
        Ok(at)
    }
}

impl Drop for Poster {
    fn drop(&mut self) {
        self.release_all();
    }
}
