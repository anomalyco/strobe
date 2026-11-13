//! The `page` driver of terminal targets (SPEC §13.9): input as the bytes a terminal sends for keys,
//! the mouse and its window's size, written to the program's pseudo-terminal, and waits on the cell
//! grid strobe's terminal keeps (`target::tty`). Selectors find text on the grid (`text=`), and a
//! point is a cell, `[col, row]` from 0. Every input is stamped on the host clock when it is written,
//! and its latency is the first grid change the program drew after it (`latencies`), when strobe can
//! tell that change from an earlier input's late answer.

use crate::host;
use crate::model::{Error, Refusal, Result, ValidatorId};
use crate::target::a11y::keys::{self, Button, Key, Mods, Named};
use crate::target::a11y::{Dialect, Selector};
use crate::target::tty::{Grid, Modes, Mouse, Tty, Update};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// How long the grid must hold still before a measured input: a program still drawing then (a
/// spinner, a redraw of the last step) is not at rest, and the input is no latency sample.
pub const QUIET: Duration = Duration::from_millis(50);
/// The longest strobe waits for the grid to hold still before an input.
pub const SETTLE_MAX: Duration = Duration::from_millis(300);
/// The longest an answer may take: strobe waits this long after a run for its last input's answer,
/// and for it to hold still (a shorter wait would right-censor a slow answer's p95), and for each
/// paced key's answer. An input with no answer yet may still get one this long after it was sent, so
/// what the grid shows then is no later input's answer for sure (`latencies`).
pub const ANSWER_MAX: Duration = Duration::from_secs(1);
/// How to time a resize on Windows, where ConPTY's host redraws before the program does.
pub const RESIZE_FIX: &str = "on Windows time a resize with a phase whose until is what only the program's redraw shows (text its new layout puts somewhere new) or its state on STROBE_EVENTS (until: { state }); tty.settle times it when the program redrew after the host; tty.latency measures resizes on Linux and macOS";

/// `idle`: no grid change for this long.
pub const IDLE: Duration = Duration::from_millis(300);

/// One input: one stamped write (or resize). A gesture spanning writes has multiple inputs.
#[derive(Clone, Copy, Debug)]
pub struct Input {
    /// When its first write went out (host clock).
    pub sent_ns: u64,
    pub what: &'static str,
    /// It may be a latency sample, from `sent_ns`: one key of typing, a press, a paste, a single click,
    /// a wheel's first notch, a button or key going down, a resize (not a burst's key, a drag, a
    /// modifier going down).
    pub sample: bool,
    /// The grid held still before it (`QUIET`): without that its first change may be the program's
    /// earlier drawing.
    pub rested: bool,
    /// Something the program may answer, so what the grid shows after it is its answer until it was
    /// answered: every driver call that writes (a key, a paste, a click, a button or key going down or
    /// up, the pointer moving, a wheel's turn, a drag, a resize), including a focus click.
    pub action: bool,
    /// Sent in a burst (`burst: true`): before the input ahead of it was answered.
    pub burst: bool,
    /// The terminal's line discipline had `ECHO` on (`Tty::line_echo`): the kernel echoed it to the
    /// grid before the program read it.
    pub echoed: bool,
}

impl Input {
    pub fn of(what: &'static str) -> Input {
        Input { sent_ns: 0, what, sample: false, rested: false, action: false, burst: false, echoed: false }
    }
}

/// How a key goes out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pace {
    /// After the grid held still; with `wait`, also until its answer (typing).
    Paced { wait: bool },
    /// Without waiting: typing once the program never rests. Still a sample, so it counts as `busy`.
    Unpaced,
    /// Without waiting, no sample (`burst: true`): a block whose runs sent one has no latency.
    Burst,
}

/// What became of the inputs of a stretch (`latencies`): each sample's latency, and how many were
/// not samples, and why.
#[derive(Clone, Debug, Default)]
pub struct Latencies {
    pub ms: Vec<f64>,
    /// Each input's time to the grid's last change before it held still (`QUIET`) or the next input:
    /// what it set off, done drawing (a resize on Windows too, after its host's redraw).
    pub settle_ms: Vec<f64>,
    /// Written while the program still drew, or less than `QUIET` after its last change.
    pub busy: usize,
    /// Followed by no change before the next input.
    pub silent: usize,
    /// Sent while an earlier input, sent less than `ANSWER_MAX` before, had not changed the grid yet,
    /// and the grid then changed: that change may be the earlier input's answer.
    pub overran: usize,
    /// Sent after such a mix-up and before a second without input: what the grid showed after it may
    /// be the answer of the input before it, which took the earlier one's.
    pub after_overrun: usize,
    /// The input that could still be owed a redraw (or "mixed" for several kinds).
    cause: Option<&'static str>,
    /// Echoed by the kernel's line discipline.
    pub echoed: usize,
    /// Sent with `burst: true`.
    pub burst: usize,
    /// Resizes that were no sample: ConPTY's host redraws first (Windows).
    pub host_resizes: usize,
    /// Of those, the ones after which only the host drew (one change): no settle sample either.
    pub host_only: usize,
}

/// A point on the grid: a cell, `[col, row]` from the top left (0, 0), or
/// `{ x, y, space: "cell" | "norm" }` (`norm`: 0 to 1 of the grid's width and height).
pub fn point(v: &Value, size: (u16, u16)) -> Option<Result<(u16, u16)>> {
    let p = crate::driver::point(v)?;
    let (cols, rows) = (size.0.max(1) as f64, size.1.max(1) as f64);
    Some(p.and_then(|(x, y, space)| {
        let (c, r) = match space.as_deref() {
            None | Some("cell") => (x.floor(), y.floor()),
            Some("norm") => ((x * cols).floor(), (y * rows).floor()),
            Some(other) => return Err(Error::usage(format!("a terminal's point is in cells ([col, row], space \"cell\") or \"norm\" (0 to 1 of the grid), not space {other:?}"))),
        };
        if !(0.0..cols).contains(&c) || !(0.0..rows).contains(&r) {
            return Err(Refusal::new(ValidatorId::OutOfView, format!("cell [{x}, {y}] is off the {}×{} grid", size.0, size.1))
                .fix("give a cell inside the grid: [col, row] from 0, or { x, y, space: \"norm\" }")
                .into());
        }
        Ok((c as u16, r as u16))
    }))
}

/// Where a selector's text shows on the grid, in reading order: (row, first column, columns). `text=`
/// is each run of the text on one row; `text==` a row whose text is all of it, trimmed.
pub fn find(g: &Grid, sel: &Selector) -> Vec<(u16, u16, u16)> {
    let mut out = Vec::new();
    for r in 0..g.rows {
        let (text, at) = g.row(r);
        let chars: Vec<char> = text.chars().collect();
        let end_col = |i: usize| at.get(i).copied().unwrap_or(g.cols);
        match sel.text_filter() {
            None => {
                let first = chars.iter().position(|c| !c.is_whitespace());
                let last = chars.iter().rposition(|c| !c.is_whitespace());
                if let (Some(a), Some(b)) = (first, last) {
                    out.push((r, at[a], end_col(b + 1) - at[a]));
                }
            }
            Some((want, true)) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() && trimmed == want.trim() {
                    let a = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
                    let n = trimmed.chars().count();
                    out.push((r, at[a], end_col(a + n) - at[a]));
                }
            }
            Some((want, false)) => {
                let w: Vec<char> = want.chars().collect();
                if w.is_empty() {
                    continue;
                }
                let mut i = 0;
                while i + w.len() <= chars.len() {
                    if chars[i..i + w.len()] == w[..] {
                        out.push((r, at[i], end_col(i + w.len()) - at[i]));
                        i += w.len();
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

/// The match `sel`'s index names, among `found`.
fn pick(found: &[(u16, u16, u16)], sel: &Selector) -> Option<(u16, u16, u16)> {
    let i = sel.index();
    let at = if i < 0 { found.len().checked_sub(i.unsigned_abs() as usize)? } else { i as usize };
    found.get(at).copied()
}

pub fn parse_sel(v: &Value) -> Result<Selector> {
    Selector::parse(v, Dialect::Terminal)
}

fn timeout(opts: &Value) -> Duration {
    opts.get("timeout").and_then(Value::as_u64).map(Duration::from_millis).unwrap_or(Duration::from_secs(30))
}

/// The modifier parameter of xterm's and the progressive keyboard protocol's sequences.
fn mod_param(m: Mods) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl) + 8 * u8::from(m.meta)
}

/// Progressive keyboard enhancement flags (`CSI > flags u`).
const DISAMBIGUATE: u16 = 1;
const EVENTS: u16 = 2;
const ALL_KEYS: u16 = 8;

/// The bytes a terminal sends for a key with `m` held (`release`: its release, which only a program
/// that asked for key events gets; empty otherwise), in the encoding the program asked for: xterm's
/// (application cursor keys when set), or the progressive keyboard protocol's (`CSI … u`) once it
/// pushed flags.
pub fn key_bytes(m: Mods, k: Key, modes: &Modes, release: bool) -> Result<Vec<u8>> {
    let kitty = modes.kitty;
    let event = if release { ":3" } else { "" };
    let ev_mods = |m: Mods| if release || m.any() { format!(";{}{event}", mod_param(m)) } else { String::new() };
    let csi_u = |code: u32, m: Mods| format!("\x1b[{code}{}u", ev_mods(m)).into_bytes();
    // Functional keys: a final letter (`CSI 1;m X`, SS3 without modifiers where xterm sends it) or
    // a number with a tilde (`CSI n;m ~`).
    let letter = |fin: char, ss3: bool, m: Mods| {
        if release || m.any() {
            format!("\x1b[1;{}{event}{fin}", mod_param(m))
        } else if ss3 {
            format!("\x1bO{fin}")
        } else {
            format!("\x1b[{fin}")
        }
        .into_bytes()
    };
    let tilde = |n: u8, m: Mods| format!("\x1b[{n}{}~", ev_mods(m)).into_bytes();
    if release && kitty & EVENTS == 0 {
        return Ok(Vec::new());
    }
    let named = match k {
        Key::Named(n) => n,
        Key::Char(c) => {
            let (base, shifted) = keys::us_key(c).unwrap_or((c, false));
            let m = Mods { shift: m.shift || shifted, ..m };
            if kitty & ALL_KEYS != 0 || (kitty & DISAMBIGUATE != 0 && (m.ctrl || m.alt || m.meta)) {
                return Ok(csi_u(base.to_ascii_lowercase() as u32, m));
            }
            if release {
                return Ok(Vec::new());
            }
            let text = if m.shift { keys::us_shifted(base) } else { c };
            if m.meta {
                return Err(Error::usage(format!("Meta+{c}: a terminal sends no Meta key unless the program asks for every key (the progressive keyboard protocol); Alt is ESC before the key")));
            }
            let mut out = Vec::new();
            if m.alt {
                out.push(0x1b);
            }
            if m.ctrl {
                let b = match base.to_ascii_lowercase() {
                    l @ 'a'..='z' => l as u8 & 0x1f,
                    ' ' | '@' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '7' => 0x1f,
                    '/' => 0x1f,
                    '8' | '?' => 0x7f,
                    _ => return Err(Error::usage(format!("Control+{c}: no control code a terminal sends for it"))),
                };
                out.push(b);
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(text.encode_utf8(&mut buf).as_bytes());
            }
            return Ok(out);
        }
    };
    let all = kitty & ALL_KEYS != 0;
    let dis = kitty & DISAMBIGUATE != 0;
    let app = modes.app_cursor;
    Ok(match named {
        Named::Letter(c) | Named::Digit(c) => return key_bytes(m, Key::Char(c.to_ascii_lowercase() as char), modes, release),
        Named::Space => return key_bytes(m, Key::Char(' '), modes, release),
        Named::Enter if all || (dis && m.any()) || release => csi_u(13, m),
        Named::Enter => [if m.alt { &b"\x1b"[..] } else { b"" }, b"\r"].concat(),
        Named::Tab if all || (dis && m.any() && m != Mods { shift: true, ..Mods::default() }) || release => csi_u(9, m),
        Named::Tab if m.shift => b"\x1b[Z".to_vec(),
        Named::Tab => [if m.alt { &b"\x1b"[..] } else { b"" }, b"\t"].concat(),
        Named::Backspace if all || (dis && m.any()) || release => csi_u(127, m),
        Named::Backspace => [if m.alt { &b"\x1b"[..] } else { b"" }, if m.ctrl { b"\x08" } else { b"\x7f" }].concat(),
        Named::Escape if all || dis || release => csi_u(27, m),
        Named::Escape => [if m.alt { &b"\x1b"[..] } else { b"" }, b"\x1b"].concat(),
        Named::ArrowUp => letter('A', app, m),
        Named::ArrowDown => letter('B', app, m),
        Named::ArrowRight => letter('C', app, m),
        Named::ArrowLeft => letter('D', app, m),
        Named::Home => letter('H', app, m),
        Named::End => letter('F', app, m),
        Named::Insert => tilde(2, m),
        Named::Delete => tilde(3, m),
        Named::PageUp => tilde(5, m),
        Named::PageDown => tilde(6, m),
        Named::F(n @ 1..=4) => letter(['P', 'Q', 'R', 'S'][n as usize - 1], true, m),
        Named::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5], m),
        Named::F(n) if all => csi_u(57376 + n as u32 - 13, m),
        Named::F(n) => return Err(Error::usage(format!("F{n}: xterm's keys end at F12; a terminal sends F13 to F24 only to a program that asks for every key"))),
        Named::Shift | Named::Control | Named::Alt | Named::Meta if all => {
            let code = match named {
                Named::Shift => 57441,
                Named::Control => 57442,
                Named::Alt => 57443,
                _ => 57444,
            };
            csi_u(code, m)
        }
        Named::Shift | Named::Control | Named::Alt | Named::Meta => Vec::new(),
        Named::ContextMenu | Named::CapsLock if all => csi_u(if named == Named::CapsLock { 57358 } else { 57363 }, m),
        Named::ContextMenu | Named::CapsLock => return Err(Error::usage(format!("{named:?}: a terminal sends no bytes for it unless the program asks for every key"))),
    })
}

/// The mouse report a terminal sends for button code `b` at a cell (`release`: a button's release),
/// in the encoding the program asked for: SGR (`?1006`), else xterm's X10 bytes (cells up to 223).
fn mouse_bytes(modes: &Modes, b: u8, cell: (u16, u16), release: bool, m: Mods) -> Result<Vec<u8>> {
    let b = b + if m.shift { 4 } else { 0 } + if m.alt || m.meta { 8 } else { 0 } + if m.ctrl { 16 } else { 0 };
    let (x, y) = (cell.0 as u32 + 1, cell.1 as u32 + 1);
    if modes.sgr {
        return Ok(format!("\x1b[<{b};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes());
    }
    if x > 223 || y > 223 {
        return Err(Error::usage(format!("cell {}, {}: xterm's X10 mouse bytes reach column and row 223; the program did not ask for SGR reports (?1006)", cell.0, cell.1)));
    }
    // X10 says a release as button 3, whichever button it was; the wheel and moves keep theirs.
    let b = if release && b & 64 == 0 { (b & !3) | 3 } else { b };
    Ok(vec![0x1b, b'[', b'M', 32 + b, 32 + x as u8, 32 + y as u8])
}

fn button_code(b: Button) -> u8 {
    match b {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
    }
}

/// `MouseNotEnabled`: the program did not turn on the mouse reporting `what` needs.
fn no_mouse(call: &str, what: &str, have: Mouse) -> Error {
    let has = match have {
        Mouse::Off => "no mouse reporting".to_string(),
        m => format!("mouse reporting of {}", serde_json::to_value(m).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()),
    };
    Refusal::new(ValidatorId::MouseNotEnabled, format!("page.{call}(): {what}, and the program turned on {has}, so a terminal would send nothing"))
        .fix("drive it with keys (press, type), or wait until the program turns the mouse on (a terminal reports clicks after ?1000, drags after ?1002, every move after ?1003)")
        .into()
}

/// Everything dispatch observes at the terminal boundary. The dev fixture substitutes a virtual
/// clock and terminal here, not its own implementation of the driver's input schedule.
pub trait Terminal {
    fn instant(&self) -> Instant {
        Instant::now()
    }
    fn now_ns(&self) -> u64 {
        host::now_ns()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn host_redraw(&self) -> bool {
        cfg!(windows)
    }
    fn size(&self) -> (u16, u16);
    fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    fn line_echo(&self) -> bool;
    fn modes(&self) -> Modes;
    fn write(&self, bytes: &[u8]) -> Result<u64>;
    fn updates(&self, from: u64, to: u64) -> Vec<Update>;
    fn quiet(&self, quiet: Duration, since: u64, deadline: Instant) -> bool;
    fn first_change(&self, since: u64, deadline: Instant) -> Option<u64>;
    fn grid<R>(&self, f: impl FnOnce(&Grid) -> R) -> R;
    fn wait(&self, deadline: Instant, f: impl FnMut(&Grid, u64, usize) -> bool) -> bool;
}

impl Terminal for Tty {
    fn size(&self) -> (u16, u16) {
        self.size()
    }
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.resize(cols, rows)
    }
    fn line_echo(&self) -> bool {
        self.line_echo()
    }
    fn modes(&self) -> Modes {
        self.modes()
    }
    fn write(&self, bytes: &[u8]) -> Result<u64> {
        self.write(bytes)
    }
    fn updates(&self, from: u64, to: u64) -> Vec<Update> {
        self.updates(from, to)
    }
    fn quiet(&self, quiet: Duration, since: u64, deadline: Instant) -> bool {
        self.quiet(quiet, since, deadline)
    }
    fn first_change(&self, since: u64, deadline: Instant) -> Option<u64> {
        self.first_change(since, deadline)
    }
    fn grid<R>(&self, f: impl FnOnce(&Grid) -> R) -> R {
        self.grid(f)
    }
    fn wait(&self, deadline: Instant, f: impl FnMut(&Grid, u64, usize) -> bool) -> bool {
        self.wait(deadline, f)
    }
}

pub struct Driver<'a, T: Terminal = Tty> {
    tty: &'a T,
    /// The program's event stream (`STROBE_EVENTS`), for `until: { state }`.
    events: Option<&'a crate::target::events::Events>,
    inputs: RefCell<Vec<Input>>,
    /// Modifier keys `down` holds, which mark later keys and mouse reports.
    held: Cell<Mods>,
    /// The mouse button `down` holds.
    pressed: Cell<Option<Button>>,
    /// The cell the mouse is over, once an input put it somewhere.
    pointer: Cell<Option<(u16, u16)>>,
    /// The size the block started at: `window("restore")`.
    start_size: (u16, u16),
    /// A click that focuses an element before keys (`type(sel)`, `press({ selector })`): no sample.
    focusing: Cell<bool>,
}

impl<'a, T: Terminal> Driver<'a, T> {
    pub fn new(tty: &'a T, events: Option<&'a crate::target::events::Events>) -> Self {
        Driver {
            tty,
            events,
            inputs: RefCell::new(Vec::new()),
            held: Cell::new(Mods::default()),
            pressed: Cell::new(None),
            pointer: Cell::new(None),
            start_size: tty.size(),
            focusing: Cell::new(false),
        }
    }

    /// Every input so far.
    pub fn inputs(&self) -> Vec<Input> {
        self.inputs.borrow().clone()
    }

    /// The run's last timed input since `t0` (none sent inside `untimed`) and whether the grid has
    /// changed since it: a run may return right after its last input (a `press` without `until`),
    /// before the program draws its answer.
    pub fn last_input(&self, t0: u64, untimed: &[(u64, u64)]) -> Option<(u64, bool)> {
        // A gesture's last write: its answer may come after any of them.
        let last = self.inputs.borrow().iter().filter(|i| i.sent_ns >= t0 && !untimed.iter().any(|g| i.sent_ns >= g.0 && i.sent_ns < g.1)).map(|i| i.sent_ns).max()?;
        let answered = self.tty.updates(last, u64::MAX).iter().any(|u| u.cells > 0 && u.start_ns >= last);
        Some((last, answered))
    }

    /// Waits for the answer to the input written at `sent`: its first change, then the grid still for
    /// `QUIET`, each within `ANSWER_MAX`. Keep the answer inside the run even if it never settles.
    pub fn await_answer(&self, sent: u64) -> bool {
        if self.tty.first_change(sent, self.tty.instant() + ANSWER_MAX).is_none() {
            return false;
        }
        self.await_quiet(sent);
        true
    }

    /// Waits (within `ANSWER_MAX`) for the grid to hold still `QUIET` after the input written at
    /// `sent`: whether it did.
    pub fn await_quiet(&self, sent: u64) -> bool {
        self.tty.quiet(QUIET, sent, self.tty.instant() + ANSWER_MAX)
    }

    /// Lets go of what `down` holds (between runs): modifier keys and a mouse button.
    pub fn release_all(&self) -> Result<()> {
        if let Some(b) = self.pressed.take() {
            let at = self.pointer.get().unwrap_or((0, 0));
            if self.tty.modes().mouse != Mouse::Off {
                self.write(&mouse_bytes(&self.tty.modes(), button_code(b), at, true, self.held.get())?, Input { action: true, ..Input::of("mouse") })?;
            }
        }
        self.held.set(Mods::default());
        Ok(())
    }

    /// Before timed input, from outside a run's timed parts (setup, a reset, between runs, the end of
    /// `ctx.untimed`): waits out what an earlier input may still owe, so no answer of theirs lands in
    /// the timed part, and the grid then holds still. At once when nothing is owed.
    pub fn quiesce(&self) {
        let updates = self.tty.updates(0, u64::MAX);
        let inputs = self.inputs.borrow().clone();
        let Some(until) = owed_until(&inputs, &updates).filter(|t| *t > self.tty.now_ns()) else { return };
        let left = Duration::from_nanos(until.saturating_sub(self.tty.now_ns()));
        self.tty.sleep(left);
        let _ = self.tty.quiet(QUIET, until, self.tty.instant() + SETTLE_MAX);
    }

    /// Writes `bytes` and records the input `i` (its time, and whether the kernel echoes it, filled in).
    /// The action log gets it as `driver::sent`.
    fn write(&self, bytes: &[u8], i: Input) -> Result<u64> {
        let echoed = self.tty.line_echo();
        let at = self.tty.write(bytes)?;
        self.record(Input { sent_ns: at, echoed, ..i });
        Ok(at)
    }

    /// Records an input that went out to the program.
    fn record(&self, i: Input) {
        self.inputs.borrow_mut().push(i);
        crate::driver::sent(i.sent_ns, i.sample, None);
    }

    /// Waits for the grid to hold still before a measured input (at most `SETTLE_MAX`).
    /// Still since the last change and since the last input's last write: what an input just before
    /// caused arrives before the next one goes out.
    fn settle(&self) -> bool {
        let last = self.inputs.borrow().last().map_or(0, |i| i.sent_ns);
        self.tty.quiet(QUIET, last, self.tty.instant() + SETTLE_MAX)
    }

    /// The cell a locator names: a point, or the middle of the text a selector finds (waiting for it
    /// up to `t`).
    fn locate(&self, v: &Value, t: Duration) -> Result<(u16, u16)> {
        if let Some(p) = point(v, self.tty.size()) {
            return p;
        }
        if v.as_str() == Some("") || v.is_null() {
            return Ok(self.pointer.get().unwrap_or_else(|| {
                let (c, r) = self.tty.size();
                (c / 2, r / 2)
            }));
        }
        let sel = parse_sel(v)?;
        let mut hit = None;
        let shown = self.tty.wait(self.tty.instant() + t, |g, _, _| {
            hit = pick(&find(g, &sel), &sel);
            hit.is_some()
        });
        match hit.filter(|_| shown) {
            Some((r, c, w)) => Ok((c + w.saturating_sub(1) / 2, r)),
            None => Err(self.missed(&sel, t)),
        }
    }

    fn missed(&self, sel: &Selector, t: Duration) -> Error {
        let n = self.tty.grid(|g| find(g, sel).len());
        let why = if n > 0 && sel.index() != 0 { format!("it shows {n} times, too few for index={}", sel.index()) } else { "the grid never showed it".into() };
        let rows: Vec<String> = self.tty.grid(|g| g.text()).into_iter().filter(|l| !l.trim().is_empty()).take(6).collect();
        Error::timeout(format!("{:?}: {why} in {} s", sel.source, t.as_secs()))
            .hint(format!("the grid's first rows: {}", rows.iter().map(|r| format!("{:?}", r.trim())).collect::<Vec<_>>().join(", ")))
    }

    /// After an input: `until` a selector shows, the next grid change, or the grid at rest; or each of a
    /// list in turn, a later one from when the one before it ended.
    fn until(&self, opts: &Value, after: u64) -> Result<()> {
        crate::driver::each_until(opts, timeout(opts), |o, first| self.until_one(o, if first { after } else { self.tty.now_ns() })).map(drop)
    }

    fn until_one(&self, opts: &Value, after: u64) -> Result<()> {
        let Some(u) = opts.get("until").filter(|u| !u.is_null()) else { return Ok(()) };
        let t = timeout(opts);
        // A state the program announces on its event stream after the input.
        if let Some(name) = u.get("state").and_then(Value::as_str) {
            let events = self.events.ok_or_else(|| Error::usage("until: { state }: this launch has no event stream"))?;
            return events.wait_state(name, after, Instant::now() + t).map(|_| ()).ok_or_else(|| {
                Error::timeout(format!("until: {{ state: {name:?} }}: the program wrote no such state to $STROBE_EVENTS in {} s", t.as_secs()))
                    .hint("the program appends {\"strobe\":\"state\",\"name\":\"…\"} lines to the path in STROBE_EVENTS")
            });
        }
        match u.as_str() {
            Some("idle") => self.idle(after, t),
            Some("frame") => self.tty.first_change(after, self.tty.instant() + t).map(|_| ()).ok_or_else(|| Error::timeout(format!("until: \"frame\": the grid did not change in {} s", t.as_secs()))),
            Some("animations") => Err(Error::usage("until: \"animations\" waits for a page's running animations; a terminal has none: \"idle\" waits for the grid to hold still")),
            _ => self.locate(u, t).map(|_| ()),
        }
    }

    /// No grid change for `IDLE` since the last change and since `since_ns`.
    fn idle(&self, since_ns: u64, t: Duration) -> Result<()> {
        if self.tty.quiet(IDLE, since_ns, self.tty.instant() + t) {
            Ok(())
        } else {
            Err(Error::timeout(format!("idle: the grid kept changing for {} s (never {} ms still)", t.as_secs(), IDLE.as_millis()))
                .hint("a program that animates never rests: wait for text (waitFor) instead"))
        }
    }

    /// One key with `extra` held over what `down` holds, sent as `pace` says. False when the grid never
    /// held still before a paced key, so the caller stops waiting for rest.
    fn key(&self, extra: Mods, k: Key, pace: Pace) -> Result<(u64, bool)> {
        let bytes = key_bytes(self.held.get().or(extra), k, &self.tty.modes(), false)?;
        if bytes.is_empty() {
            return Err(Error::usage(format!("{k:?}: a terminal sends no bytes for a modifier alone (unless the program asks for every key); hold it with page.down and press the key")));
        }
        let paced = matches!(pace, Pace::Paced { .. });
        let rested = paced && self.settle();
        // An unpaced key (the program never rested) is still a sample: counted `busy`, so a block whose
        // typing fell back to it refuses rather than keep the keys before.
        let at = self.write(&bytes, Input { sample: pace != Pace::Burst, rested, action: true, burst: pace == Pace::Burst, ..Input::of("key") })?;
        // Typing: the next key must not go out before this one's answer, which may take up to
        // `ANSWER_MAX` (a shorter wait sent the next key while a slow answer was on its way). A key
        // that changed nothing by then has no answer coming, so the next key is paced as well.
        if pace == (Pace::Paced { wait: true }) {
            let _ = self.tty.first_change(at, self.tty.instant() + ANSWER_MAX);
        }
        Ok((at, rested || !paced))
    }

    /// `type(sel, text, { paste: true })`: the text as one paste, after the grid held still, in one
    /// write: one latency sample, to the paste's first change. Inside `CSI 200 ~` … `CSI 201 ~` once the
    /// program turned bracketed paste on (`?2004`), as a terminal pastes; else the text alone. A line
    /// break goes as a terminal pastes it, a carriage return.
    fn paste(&self, text: &str, burst: bool) -> Result<u64> {
        if burst {
            return Err(Error::usage("page.type(…, { paste: true, burst: true }): a paste is one input, with no keys to send apart; drop burst"));
        }
        if text.is_empty() {
            return Err(Error::usage("page.type(…, { paste: true }): no text to paste"));
        }
        if text.contains("\x1b[201~") {
            return Err(Error::usage("page.type(…, { paste: true }): the text holds ESC [201~, which ends a bracketed paste; a terminal never pastes it"));
        }
        let body = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if self.tty.modes().bracketed_paste { format!("\x1b[200~{body}\x1b[201~") } else { body };
        let rested = self.settle();
        self.write(bytes.as_bytes(), Input { sample: true, rested, action: true, ..Input::of("paste") })
    }

    fn mouse_mode(&self, call: &str, motion: bool) -> Result<Modes> {
        let m = self.tty.modes();
        let ok = match (m.mouse, motion, self.pressed.get().is_some()) {
            (Mouse::Off, _, _) => false,
            (_, false, _) => true,
            (Mouse::Motion, true, _) => true,
            (Mouse::Drags, true, true) => true,
            _ => false,
        };
        if ok {
            return Ok(m);
        }
        let what =
            if motion { if self.pressed.get().is_some() { "it moves the mouse with a button held" } else { "it moves the mouse with no button held" } } else { "it sends a mouse button or the wheel" };
        Err(no_mouse(call, what, m.mouse))
    }

    /// No click on an already focused field. Otherwise fence the focus click like any other
    /// action; it may take the whole answer window, even though it is not itself a sample.
    fn focus_click(&self, sel: &Value, o: &Value, pixels: Option<&std::path::Path>) -> Result<()> {
        if let Ok(s) = parse_sel(sel) {
            let t = timeout(o);
            self.locate(sel, t)?;
            if self.tty.grid(|g| pick(&find(g, &s), &s).is_some_and(|(r, c, w)| g.cursor.0 == r && (c..c + w.max(1)).contains(&g.cursor.1))) {
                return Ok(());
            }
        }
        self.focusing.set(true);
        let r = self.dispatch("click", &[sel.clone(), json!({ "timeout": o["timeout"] })], pixels);
        self.focusing.set(false);
        r?;
        let at = self.inputs.borrow().last().map_or(0, |i| i.sent_ns);
        if self.tty.first_change(at, self.tty.instant() + ANSWER_MAX).is_some() {
            let _ = self.tty.quiet(QUIET, at, self.tty.instant() + SETTLE_MAX);
        } else {
            self.tty.sleep(Duration::from_nanos((at + ANSWER_MAX.as_nanos() as u64).saturating_sub(self.tty.now_ns())));
        }
        Ok(())
    }

    /// One page call from the runner. `pixels`: a pixels block's folder for `snapshot`.
    pub fn dispatch(&self, call: &str, args: &[Value], pixels: Option<&std::path::Path>) -> Result<Value> {
        let opts = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
        let sel = |i: usize| args.get(i).cloned().unwrap_or(Value::String(String::new()));
        // A terminal has a grid of cells, not pixels: an image locator or a pixel wait has nothing to read.
        let pixel = |v: &Value| crate::driver::pixels::is_pixels(v) || ["until", "selector", "at"].iter().any(|k| v.get(k).is_some_and(crate::driver::pixels::is_pixels));
        if args.iter().any(pixel) {
            return Err(crate::model::Refusal::new(
                crate::model::ValidatorId::NotForTarget,
                format!("page.{call}(): an image locator or a pixel wait on a terminal target, which draws a grid of cells, not pixels"),
            )
            .fix("find the grid's text with a selector (\"text=…\"), name a cell ([col, row]), or wait until: \"idle\" or { state }")
            .into());
        }
        match call {
            "type" => {
                let o = opts(2);
                if !matches!(sel(0).as_str(), Some("")) {
                    self.focus_click(&sel(0), &o, pixels)?;
                }
                let text = args.get(1).and_then(Value::as_str).unwrap_or("");
                let burst = crate::driver::Typing::of(&o) == crate::driver::Typing::Burst;
                if o["paste"] == true {
                    let at = self.paste(text, burst)?;
                    self.until(&o, at)?;
                    return Ok(Value::Bool(true));
                }
                let delay = Duration::from_millis(o["delay"].as_u64().unwrap_or(0));
                let mut pace = if burst { Pace::Burst } else { Pace::Paced { wait: true } };
                if burst {
                    crate::driver::unmeasured();
                }
                // `until` counts from the call when it typed nothing.
                let mut last = self.tty.now_ns();
                for c in text.chars() {
                    let k = if c == '\n' { Key::Named(Named::Enter) } else { Key::Char(c) };
                    let started = self.tty.instant();
                    let (at, rested) = self.key(Mods::default(), k, pace)?;
                    last = at;
                    // A program that never rests holds every key the whole wait: pacing stops.
                    if !rested {
                        pace = Pace::Unpaced;
                    }
                    self.tty.sleep(delay.saturating_sub(self.tty.instant() - started));
                }
                self.until(&o, last)?;
            }
            "press" => {
                let o = opts(1);
                if let Some(s) = o.get("selector").filter(|s| s.as_str() != Some("") && !s.is_null()) {
                    self.focus_click(s, &o, pixels)?;
                }
                let (m, k) = keys::parse_chord(args.first().and_then(Value::as_str).unwrap_or(""))?;
                // `burst: true`: at once, a chord's next key (`g` `g`) before the last one's answer.
                let pace = if o["burst"] == true { Pace::Burst } else { Pace::Paced { wait: false } };
                if pace == Pace::Burst {
                    crate::driver::unmeasured();
                }
                let (at, _) = self.key(m, k, pace)?;
                self.until(&o, at)?;
            }
            "down" | "up" => {
                let o = opts(1);
                let name = args.first().and_then(Value::as_str).unwrap_or("");
                let down = call == "down";
                if let Some(b) = keys::mouse_button(name) {
                    let modes = self.mouse_mode(call, false)?;
                    let at = match o.get("at").filter(|v| !v.is_null()) {
                        Some(v) => self.locate(v, timeout(&o))?,
                        None => self.pointer.get().ok_or_else(|| Error::usage(format!("page.{call}(\"{name}\"): the mouse is nowhere yet; give opts.at (text, or [col, row])")))?,
                    };
                    self.pointer.set(Some(at));
                    let rested = down && self.settle();
                    // A button let go is its own call, so its own action: its answer (a drop) is owed.
                    let t = self.write(&mouse_bytes(&modes, button_code(b), at, !down, self.held.get())?, Input { sample: down, rested, action: true, ..Input::of("mouse") })?;
                    self.pressed.set(down.then_some(b));
                    self.until(&o, t)?;
                } else {
                    let (m, k) = keys::parse_chord(name)?;
                    let modifier = matches!(k, Key::Named(n) if n.modifier());
                    if modifier && m == Mods::default() {
                        let mut held = self.held.get();
                        let on = |b: &mut bool| *b = down;
                        match k {
                            Key::Named(Named::Shift) => on(&mut held.shift),
                            Key::Named(Named::Control) => on(&mut held.ctrl),
                            Key::Named(Named::Alt) => on(&mut held.alt),
                            _ => on(&mut held.meta),
                        }
                        // The keys it marks carry it; only a program that asked for every key sees it alone.
                        let bytes = key_bytes(if down { held } else { self.held.get() }, k, &self.tty.modes(), !down)?;
                        self.held.set(held);
                        if !bytes.is_empty() {
                            self.write(&bytes, Input { action: true, ..Input::of("key") })?;
                        }
                    } else {
                        let bytes = key_bytes(self.held.get().or(m), k, &self.tty.modes(), !down)?;
                        if !bytes.is_empty() {
                            let rested = down && self.settle();
                            let t = self.write(&bytes, Input { sample: down, rested, action: true, ..Input::of("key") })?;
                            self.until(&o, t)?;
                        }
                    }
                }
            }
            "click" => {
                let o = opts(1);
                let at = self.locate(&sel(0), timeout(&o))?;
                let modes = self.mouse_mode("click", false)?;
                let b = button_code(Button::parse(o["button"].as_str())?);
                let m = self.held.get().or(keys::Mods::parse_list(&o["modifiers"])?);
                let count = o["count"].as_u64().unwrap_or(1).clamp(1, 3);
                self.pointer.set(Some(at));
                // A focusing click is not a sample, but the field may redraw: its answer is owed.
                let focusing = self.focusing.get();
                let measured = count == 1 && !focusing;
                let rested = self.settle();
                let mut bytes = mouse_bytes(&modes, b, at, false, m)?;
                bytes.extend(mouse_bytes(&modes, b, at, true, m)?);
                let mut last = 0;
                for i in 0..count {
                    last = self.write(&bytes, Input { sample: measured, rested: rested && i == 0, action: true, ..Input::of("click") })?;
                    if i + 1 < count {
                        self.tty.sleep(Duration::from_millis(30));
                    }
                }
                self.until(&o, last)?;
            }
            "hover" | "move" => {
                let (at, o) = if call == "move" {
                    let n = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
                    let p = point(&json!([n(0), n(1)]), self.tty.size()).ok_or_else(|| Error::usage("page.move(col, row): two numbers, a cell from 0"))??;
                    (p, opts(2))
                } else {
                    let o = opts(1);
                    (self.locate(&sel(0), timeout(&o))?, o)
                };
                let modes = self.mouse_mode(call, true)?;
                let b = self.pressed.get().map_or(3, button_code) + 32;
                self.pointer.set(Some(at));
                let t = self.write(&mouse_bytes(&modes, b, at, false, self.held.get())?, Input { action: true, ..Input::of("pointer") })?;
                self.until(&o, t)?;
            }
            "wheel" | "scroll" => {
                let o = opts(2);
                let at = self.locate(&sel(0), timeout(&o))?;
                let modes = self.mouse_mode(call, false)?;
                let d = args.get(1).cloned().unwrap_or(Value::Null);
                // A terminal's wheel moves in notches, no pixels: `dy`/`by` notches down (negative up),
                // `dx`/`byX` right.
                let (dx, dy) = if call == "wheel" { (d["dx"].as_f64().unwrap_or(0.0), d["dy"].as_f64().unwrap_or(0.0)) } else { (d["byX"].as_f64().unwrap_or(0.0), d["by"].as_f64().unwrap_or(0.0)) };
                let steps: Vec<u8> =
                    std::iter::repeat_n(if dy < 0.0 { 64 } else { 65 }, dy.abs().round() as usize).chain(std::iter::repeat_n(if dx < 0.0 { 66 } else { 67 }, dx.abs().round() as usize)).collect();
                if steps.is_empty() {
                    return Err(Error::usage(format!("page.{call}(): a terminal's wheel moves in notches; give at least one ({} in notches)", if call == "wheel" { "dy, dx" } else { "by, byX" })));
                }
                // `scroll` paces its notches at `speed` notches a second (default 20); `wheel` sends them at once.
                let gap = if call == "scroll" { Duration::from_secs_f64(1.0 / d["speed"].as_f64().filter(|s| *s > 0.0).unwrap_or(20.0)) } else { Duration::ZERO };
                self.pointer.set(Some(at));
                let rested = self.settle();
                // `wheel`'s notches go at once: one input. `scroll`'s go `gap` apart, each an input the
                // program may answer (the first the sample): a change after the next one may be its.
                let mut bytes = Vec::new();
                let mut last = 0;
                for (i, b) in steps.iter().enumerate() {
                    bytes.extend(mouse_bytes(&modes, *b, at, false, self.held.get())?);
                    if gap > Duration::ZERO {
                        if i > 0 {
                            self.tty.sleep(gap);
                        }
                        last = self.write(&bytes, Input { sample: i == 0, rested: rested && i == 0, action: true, ..Input::of("scroll") })?;
                        bytes.clear();
                    }
                }
                if gap == Duration::ZERO {
                    last = self.write(&bytes, Input { sample: true, rested, action: true, ..Input::of("wheel") })?;
                }
                self.until(&o, last)?;
            }
            "drag" => {
                let o = opts(2);
                let from = self.locate(&sel(0), timeout(&o))?;
                let modes = self.mouse_mode("drag", false)?;
                let path = keys::parse_path(&args.get(1).cloned().unwrap_or(Value::Null))?;
                let b = button_code(Button::parse(o["button"].as_str())?);
                let duration = Duration::from_millis(o["duration"].as_u64().unwrap_or(1000));
                let held = self.pressed.get().is_some();
                let drag = Input { action: true, ..Input::of("drag") };
                let mut last = self.tty.now_ns();
                if !held {
                    last = self.write(&mouse_bytes(&modes, b, from, false, self.held.get())?, drag)?;
                    self.pressed.set(Some(Button::parse(o["button"].as_str())?));
                }
                self.mouse_mode("drag", true)?;
                let (cols, rows) = self.tty.size();
                let started = self.tty.instant();
                let mut at = from;
                // One move per cell the path enters, paced along its duration.
                let n = (duration.as_millis() / 16).max(1) as usize;
                for i in 1..=n {
                    let (dx, dy) = keys::path_at(&path, i as f64 / n as f64);
                    let cell = ((from.0 as f64 + dx).round().clamp(0.0, cols as f64 - 1.0) as u16, (from.1 as f64 + dy).round().clamp(0.0, rows as f64 - 1.0) as u16);
                    let due = duration.mul_f64(i as f64 / n as f64);
                    self.tty.sleep(due.saturating_sub(self.tty.instant() - started));
                    if cell != at {
                        at = cell;
                        last = self.write(&mouse_bytes(&modes, b + 32, at, false, self.held.get())?, drag)?;
                    }
                }
                self.pointer.set(Some(at));
                if !held {
                    last = self.write(&mouse_bytes(&modes, b, at, true, self.held.get())?, drag)?;
                    self.pressed.set(None);
                }
                self.until(&o, last)?;
            }
            "window" => {
                let o = opts(1);
                let action = args.first().cloned().unwrap_or(Value::Null);
                let (cols, rows) = match (&action, action.as_str()) {
                    (_, Some("restore")) => self.start_size,
                    (Value::Object(m), _) if m.contains_key("resize") => {
                        let r = &m["resize"];
                        let n = |k: &str| r[k].as_u64().filter(|n| (1..=u16::MAX as u64).contains(n)).map(|n| n as u16);
                        n("width").zip(n("height")).ok_or_else(|| Error::usage("page.window({ resize: { width, height } }): a terminal's size is in cells, columns and rows from 1"))?
                    }
                    _ => {
                        return Err(Refusal::new(
                            ValidatorId::NotForTarget,
                            format!("page.window({action}): a terminal's window only takes a size ({{ resize: {{ width: cols, height: rows }} }}) or \"restore\""),
                        )
                        .into());
                    }
                };
                if self.tty.host_redraw() && (o["until"] == "frame" || o["until"][0] == "frame") {
                    return Err(Refusal::new(
                        ValidatorId::NotOnThisOs,
                        "page.window(resize, { until: \"frame\" }): ConPTY's console host redraws its own buffer at a resize before the program hears of it, so the next change is the host's",
                    )
                    .fix(RESIZE_FIX)
                    .into());
                }
                let rested = self.settle();
                let at = self.tty.now_ns();
                self.tty.resize(cols, rows)?;
                // ConPTY's console host redraws its own buffer at a resize before the program hears of
                // it: the first change is the host's, so a resize is no latency sample there.
                let sample = !self.tty.host_redraw();
                self.record(Input { sent_ns: at, sample, rested, action: true, ..Input::of("resize") });
                self.until(&o, at)?;
            }
            "waitFor" => {
                let o = opts(1);
                if crate::driver::point(&sel(0)).is_some() {
                    return Err(Error::usage("page.waitFor(point): a cell is always there; wait for text on the grid (\"text=…\")"));
                }
                let deadline = Instant::now() + timeout(&o);
                self.locate(&crate::driver::with_text(&sel(0), &o), timeout(&o))?;
                if let Some(o) = crate::driver::until_after(&o, deadline) {
                    self.until(&o, self.tty.now_ns())?;
                }
            }
            "idle" => self.idle(self.tty.now_ns(), timeout(&opts(0)))?,
            "frame" => {
                let t = timeout(&opts(0));
                self.tty.first_change(self.tty.now_ns(), self.tty.instant() + t).ok_or_else(|| Error::timeout(format!("frame: the grid did not change in {} s", t.as_secs())))?;
            }
            "wait" => self.tty.sleep(Duration::from_millis(args.first().and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as u64)),
            "query" => return self.query(&sel(0)),
            "snapshot" => {
                let Some(dir) = pixels else { return Ok(Value::Bool(false)) };
                let name: String = args.first().and_then(Value::as_str).unwrap_or("").chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
                std::fs::create_dir_all(dir)?;
                let (img, text) = self.tty.grid(|g| (crate::target::tty::image::image(g), g.text().join("\n")));
                crate::store::write_atomic(&dir.join(format!("{name}.png")), &crate::png::write(&img?)?)?;
                crate::store::write_atomic(&dir.join(format!("{name}.txt")), text.as_bytes())?;
            }
            "goto" | "eval" | "act" => {
                return Err(Refusal::new(
                    ValidatorId::NotForTarget,
                    format!("page.{call}() is not a terminal's: a terminal target takes keys, the mouse (once the program turns it on) and its size (type, press, down, up, click, hover, move, wheel, scroll, drag, window, waitFor, query, idle, frame, wait, snapshot)"),
                )
                .into());
            }
            other => return Err(Error::msg(format!("unknown page call {other}"))),
        }
        Ok(Value::Bool(true))
    }

    /// `page.query(selector?)`: where the selector's text shows now (every non-empty row without
    /// one), each with its cells as `rect` ([col, row, columns, 1]).
    fn query(&self, sel: &Value) -> Result<Value> {
        let sel = if sel.is_null() || sel.as_str() == Some("") || sel.as_str() == Some("*") { Selector::every(Dialect::Terminal) } else { parse_sel(sel)? };
        let mouse = self.tty.modes().mouse != Mouse::Off;
        Ok(self.tty.grid(|g| {
            let found = find(g, &sel);
            Value::Array(
                found
                    .iter()
                    .map(|&(r, c, w)| {
                        let name: String = (c..c + w).filter_map(|x| g.cell(r, x)).filter(|cell| !cell.is_wide_continuation()).map(|cell| if cell.has_contents() { cell.contents().to_string() } else { " ".into() }).collect();
                        let focused = g.cursor.0 == r && (c..c + w.max(1)).contains(&g.cursor.1);
                        json!({ "key": "", "path": format!("text={:?}", name.trim()), "role": "text", "name": name, "rect": [c, r, w, 1], "actionable": mouse, "focused": focused, "enabled": true, "checked": null, "expanded": null, "selected": null })
                    })
                    .collect(),
            )
        }))
    }
}

/// Whether strobe can tell an input's answer from an earlier input's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pairing {
    Clear,
    /// Its action went out while an earlier action, sent less than `ANSWER_MAX` before, had changed no
    /// cells yet, and the grid changed before the next action: that change may be the earlier one's
    /// late answer. Or the change begun after it was still being drawn when the next action went out:
    /// it may hold the next one's work.
    Overran(&'static str),
    /// After an `Overran` and before a second with no action: the action before it may have taken an
    /// earlier one's answer, and this one's change may be that action's.
    AfterOverrun(&'static str),
}

/// The pairing walk's state after the inputs it read: the last write of an action still owed an
/// answer, whether a mix-up may still shift answers, and the last action's last write.
#[derive(Clone, Copy, Debug, Default)]
struct Walk {
    unanswered: Option<(u64, &'static str)>,
    tainted: Option<&'static str>,
    prev: Option<u64>,
}

/// Each input's `Pairing` (an input that is no action takes the one of the action before it), and
/// the state after them all. `inputs` in the order sent, every one of the launch (setup's and a
/// reset's too: an answer they still owed lands in a run); `changes`: each update that changed cells,
/// (start, commit), sorted by start.
///
/// An action is answered once a change begins after its last write and before the next action; an
/// action that changed nothing is owed an answer for `ANSWER_MAX` from its last write. An action is
/// `Overran` when the grid changed in its window (its first write to the next action) while an
/// earlier one was owed, or when the change that began in its window was still being drawn when the
/// next action went out (an open synchronized frame that took the next input's work too); after one,
/// every action is `AfterOverrun` until `ANSWER_MAX` passes with no action.
fn pairings(inputs: &[&Input], changes: &[(u64, u64)]) -> (Vec<Pairing>, Walk) {
    let answer_max = ANSWER_MAX.as_nanos() as u64;
    let starts: Vec<u64> = inputs.iter().filter(|i| i.action).map(|i| i.sent_ns).collect();
    let mut w = Walk::default();
    let mut now = Pairing::Clear;
    let mut k = 0;
    // The changes that begin in [from, to).
    let within = |from: u64, to: u64| &changes[changes.partition_point(|c| c.0 < from)..changes.partition_point(|c| c.0 < to)];
    let out = inputs
        .iter()
        .map(|i| {
            if !i.action {
                return now;
            }
            let next = starts.get(k + 1).copied().unwrap_or(u64::MAX);
            k += 1;
            // A second with no action after the last one's last write: whatever was owed has come.
            if w.prev.is_some_and(|p| i.sent_ns.saturating_sub(p) >= answer_max) {
                w.tainted = None;
            }
            if w.unanswered.is_some_and(|(u, _)| i.sent_ns.saturating_sub(u) >= answer_max) {
                w.unanswered = None;
            }
            w.prev = Some(i.sent_ns);
            let touched = within(i.sent_ns, next);
            let spans = next != u64::MAX && touched.iter().any(|c| c.1 >= next);
            now = match (!touched.is_empty(), w.unanswered, w.tainted) {
                (true, Some((_, kind)), _) => Pairing::Overran(kind),
                _ if spans => Pairing::Overran(i.what),
                (true, None, Some(kind)) => Pairing::AfterOverrun(kind),
                _ => Pairing::Clear,
            };
            if let Pairing::Overran(kind) = now {
                w.tainted = Some(kind);
            }
            // Answered: a change after its last write. An answered one owes nothing, a mix-up or not
            // (`tainted` keeps that).
            let end = i.sent_ns;
            w.unanswered = if answered(changes, end, next, i.what == "resize" && !i.sample) { None } else { Some((end, i.what)) };
            now
        })
        .collect();
    (out, w)
}

/// Whether an action whose last write went out at `end` was answered before the next one at `next`:
/// a change began between them. One sent while the program still drew (a change under `QUIET`
/// before it, or one open across it: an `up` right after `until: "frame"`, a drag's next move) is
/// answered only by a change after the grid held still `QUIET` again: what came sooner may be the
/// drawing it went into (an answer in pieces), and its own answer still on its way.
fn answered(changes: &[(u64, u64)], end: u64, next: u64, host_redraw: bool) -> bool {
    let quiet = QUIET.as_nanos() as u64;
    let before = changes.partition_point(|c| c.0 < end);
    // The drawing going on when it went out: the latest commit of a change begun before it.
    let mut last = changes[..before].iter().map(|c| c.1).max();
    for c in changes[before..].iter().take_while(|c| c.0 < next).skip(usize::from(host_redraw)) {
        match last {
            Some(l) if c.0 < l.saturating_add(quiet) => last = Some(l.max(c.1)),
            _ => return true,
        }
    }
    false
}

/// The cell-changing updates as `pairings` reads them.
fn changes(updates: &[Update]) -> Vec<(u64, u64)> {
    let mut c: Vec<(u64, u64)> = updates.iter().filter(|u| u.cells > 0).map(|u| (u.start_ns, u.commit_ns)).collect();
    c.sort_unstable();
    c
}

/// Until when the next action must wait for what `inputs` may still owe (`Driver::quiesce`):
/// `ANSWER_MAX` after the last write of an action still owed an answer, or after the last action once
/// a mix-up may still shift answers. None when nothing is owed.
pub fn owed_until(inputs: &[Input], updates: &[Update]) -> Option<u64> {
    let mut all: Vec<&Input> = inputs.iter().collect();
    all.sort_by_key(|i| i.sent_ns);
    let (_, w) = pairings(&all, &changes(updates));
    let answer_max = ANSWER_MAX.as_nanos() as u64;
    (if w.tainted.is_some() { w.prev } else { w.unanswered.map(|(t, _)| t) }).map(|t| t + answer_max)
}

/// Each sample's latency among `inputs` (those in `stretches`, host clock, without `gaps`): from
/// the input to the first update after it that changed cells and began before the next input was
/// written (its output, not the next one's), when strobe can tell that update is its answer. Not a
/// sample, counted by why: sent in a burst (`burst`), echoed by the kernel (`echoed`), sent while an
/// earlier input's answer may still have been on its way (`overran`), written while the program
/// still drew or less than `QUIET` after its last change (`busy`), or followed by no change before
/// the next input or the stretch's end (`silent`).
pub fn latencies(tty: &Tty, inputs: &[Input], stretches: &[&crate::metrics::Timed]) -> Latencies {
    pair(inputs, &tty.updates(0, u64::MAX), stretches)
}

/// `latencies` of a launch's inputs and grid updates (every one so far): the pairing alone, of
/// recorded or made-up timestamps (`scripts/dev/tty-pairing` feeds it fixed sequences).
pub fn pair(inputs: &[Input], updates: &[Update], stretches: &[&crate::metrics::Timed]) -> Latencies {
    pair_observing(inputs, updates, stretches, |_, _| {})
}

/// Also expose the exact input and commit selected for each candidate, so a scripted program can
/// compare ownership (not just a percentile) against its own independent log of answers.
pub fn pair_observing(inputs: &[Input], updates: &[Update], stretches: &[&crate::metrics::Timed], mut observe: impl FnMut(&Input, &Update)) -> Latencies {
    let mut out = Latencies::default();
    let mut all: Vec<&Input> = inputs.iter().collect();
    all.sort_by_key(|i| i.sent_ns);
    let (pairing, _) = pairings(&all, &changes(updates));
    let actions: Vec<u64> = all.iter().filter(|i| i.action).map(|i| i.sent_ns).collect();
    let quiet = QUIET.as_nanos() as u64;
    for crate::metrics::Timed { t0, t1, untimed: gaps } in stretches.iter().copied() {
        let ins: Vec<(&Input, Pairing)> =
            all.iter().zip(&pairing).filter(|(i, _)| i.sent_ns >= *t0 && i.sent_ns <= *t1 && !gaps.iter().any(|g| i.sent_ns >= g.0 && i.sent_ns < g.1)).map(|(i, o)| (*i, *o)).collect();
        out.host_resizes += ins.iter().filter(|(i, _)| i.what == "resize" && !i.sample).count();
        out.burst += ins.iter().filter(|(i, _)| i.burst).count();
        for (i, pairing) in ins.iter().copied().filter(|(i, _)| i.sample || i.what == "resize") {
            if i.echoed {
                out.echoed += 1;
                continue;
            }
            if let Pairing::Overran(kind) | Pairing::AfterOverrun(kind) = pairing {
                if matches!(pairing, Pairing::Overran(_)) {
                    out.overran += 1;
                } else {
                    out.after_overrun += 1;
                }
                out.cause = Some(if out.cause.is_none_or(|c| c == kind) { kind } else { "mixed" });
                continue;
            }
            // At rest when it went out, as the grid shows now: no update spanned it (a frame begun
            // before it) and the last change came `QUIET` before it.
            let drawing = updates.iter().any(|u| u.start_ns < i.sent_ns && u.commit_ns >= i.sent_ns)
                || updates.iter().filter(|u| u.cells > 0 && u.commit_ns < i.sent_ns).map(|u| u.commit_ns).max().is_some_and(|c| i.sent_ns - c < quiet);
            if !i.rested || drawing {
                out.busy += i.sample as usize;
                continue;
            }
            // Its window ends at the next action (a modifier going down in it is part of the next key).
            let next = actions.get(actions.partition_point(|s| *s <= i.sent_ns)).copied().unwrap_or(u64::MAX);
            // An untimed edge ends attribution even if the next timed part contains no new input.
            let gap = gaps.iter().find(|g| g.0 > i.sent_ns).map_or(u64::MAX, |g| g.0);
            let next = next.min(gap);
            let caused: Vec<&Update> = updates.iter().filter(|u| u.cells > 0 && u.start_ns >= i.sent_ns && u.commit_ns < next && u.commit_ns <= *t1).collect();
            match caused.first() {
                Some(u) if i.sample => {
                    observe(i, u);
                    out.ms.push((u.commit_ns - i.sent_ns) as f64 / 1e6);
                }
                None if i.sample => out.silent += 1,
                _ => {}
            }
            // A resize ConPTY's host redraws first settles only on a change after the host's: the
            // program's own redraw.
            if !i.sample && caused.len() < 2 {
                out.host_only += 1;
                continue;
            }
            // Settled: the grid held still after its last change, before the next input or the end.
            if let Some(last) = caused.last()
                && next.min(*t1).saturating_sub(last.commit_ns) >= QUIET.as_nanos() as u64
            {
                out.settle_ms.push((last.commit_ns - i.sent_ns) as f64 / 1e6);
            }
        }
    }
    out
}

/// Why a block's inputs have no latency strobe can pair with them (`AmbiguousInteraction`, which
/// refuses every `tty.latency.*` and `tty.settle.*`), first reason first: its summary after the
/// metric's name, and the fix. None: every latency of `lat` is its input's own answer.
pub fn ambiguous(lat: &Latencies) -> Option<(String, &'static str)> {
    let tried = lat.ms.len() + lat.busy;
    let ms = ANSWER_MAX.as_millis();
    if lat.burst > 0 {
        Some((
            format!("the runs sent {} input(s) with burst: true, each before the one before it answered, so no grid update is any one input's answer", lat.burst),
            "measure burst input by cpu.work, run.duration, tty.bytes or a phase, or drop burst: true (or use type with paste: true for one paste) for input latency",
        ))
    } else if lat.echoed > 0 {
        Some((
            format!("the terminal's kernel line discipline had ECHO on for {} input(s), so the grid showed the kernel's echo before the program read the input", lat.echoed),
            "wait in setup until the program enters raw mode, or time a line-mode command's answer after Enter with a phase and until: \"text=…\"",
        ))
    } else if tried > 0 && lat.busy * 2 > tried {
        // Before the overrun: a program that never rests (a spinner) also leaves each input's answer
        // unknown, and its fix is the drawing's.
        Some((
            format!(
                "the program was still drawing when {} of {tried} inputs were sent (not {} ms still after the last change), so the next update may not be their answer",
                lat.busy,
                QUIET.as_millis()
            ),
            "wait for the grid to settle before each input (until: \"idle\"), or stop spinners and animations in the target",
        ))
    } else if lat.overran + lat.after_overrun > 0 {
        let what = match (lat.overran, lat.after_overrun) {
            (0, n) => format!("{n} input(s) came less than {ms} ms after inputs outside the measured parts that went out before an earlier one changed the grid"),
            (o, 0) => format!("{o} input(s) were sent while an earlier input within {ms} ms had not yet changed the grid"),
            (o, n) => format!("{o} input(s) were sent while an earlier input within {ms} ms had not yet changed the grid, and {n} more before {ms} ms passed without input"),
        };
        let fix = match lat.cause {
            Some("scroll") => "lower scroll speed so each notch is answered before the next, or use wheel for one input; waiting after scroll cannot separate its notches",
            Some("click" | "wheel") => {
                "wait for idle after a click or wheel that redraws twice, or wait for final text or app state before the next input; the first change is not necessarily the whole answer"
            }
            Some("drag" | "mouse") => "wait for the final drop text or app state before the next key; if release draws nothing, wait a full second before measured input",
            Some("resize") => "wait for text unique to the new layout, or the program's resize-complete state, before the next key; the console host's redraw is not the program's answer",
            Some("key" | "paste") => {
                "for one redraw wait for its frame or use paced type. After a no-op wait a full second. For a debounce, redraws over 50 ms apart or an answer over 1 s, time a phase until final text or app state, not the first grid change"
            }
            _ => "wait for final text or app state before the next action, or measure throughput with cpu.work or run.duration; first grid change does not mean finished",
        };
        Some((format!("input outran the program ({what}), so late output cannot be paired with the input that caused it"), fix))
    } else {
        None
    }
}
