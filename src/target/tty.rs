//! Terminal targets (SPEC §13.9): a program on a pseudo-terminal strobe owns (ConPTY on Windows, a
//! Unix pseudo-terminal elsewhere), its output parsed into a cell grid by a terminal emulator inside
//! strobe (`vt100`). One reader thread per terminal reads the program's output as it comes, answers
//! its terminal queries at once (so a program that asks what the terminal is never stalls), keeps
//! the grid a terminal would show, and stamps each change of it on the host clock. Nothing needs a
//! display: it runs on a locked session and on a headless server alike.

pub mod image;

use crate::host;
use crate::model::{Error, Result};
use serde::Serialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

/// A cell's size in pixels: what `CSI 14 t` and `CSI 16 t` answer, and what snapshots draw.
pub const CELL_W: usize = 8;
pub const CELL_H: usize = 16;

/// A synchronized frame left open this long ends at the next read, as terminals time them out: what it
/// drew commits as a plain update, never a frame.
const SYNC_TIMEOUT_NS: u64 = 1_000_000_000;

/// One change of the grid, stamped on the host clock: the bytes of one read that changed cells, or one
/// synchronized frame (`CSI ? 2026 h` … `CSI ? 2026 l`), whatever it changed.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Update {
    /// When the read that began it returned.
    pub start_ns: u64,
    /// When the grid held it: the read that completed it returned (a frame: the one holding its end).
    pub commit_ns: u64,
    /// Cells whose character or attributes it changed.
    pub cells: u32,
    pub synced: bool,
}

/// The mouse reporting the program turned on (`CSI ? 1000 h` and the others).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mouse {
    Off,
    /// Presses (and releases): `?9`, `?1000`.
    Clicks,
    /// Clicks, and moves while a button is held: `?1002`.
    Drags,
    /// Clicks and every move: `?1003`.
    Motion,
}

/// The input modes the program set, which decide how strobe encodes keys and the mouse.
#[derive(Clone, Copy, Debug)]
pub struct Modes {
    pub mouse: Mouse,
    /// SGR mouse encoding (`?1006`); otherwise the X10 bytes.
    pub sgr: bool,
    /// Application cursor keys (`?1`): arrows as `ESC O A`.
    pub app_cursor: bool,
    /// The progressive keyboard enhancement flags the program pushed (`CSI > f u`), 0 when none.
    pub kitty: u16,
    /// Bracketed paste (`?2004`): a paste comes between `CSI 200 ~` and `CSI 201 ~`.
    pub bracketed_paste: bool,
}

/// The program to run.
#[derive(Clone, Debug)]
pub enum Run {
    /// A command line: Windows hands it to `CreateProcess` as it is; Unix runs it with `sh -c 'exec …'`
    /// (no shell process stays between strobe and the program).
    Line(String),
    /// A program and its arguments, as they are.
    Argv(Vec<String>),
}

impl Run {
    pub fn text(&self) -> String {
        match self {
            Run::Line(s) => s.clone(),
            Run::Argv(a) => a.join(" "),
        }
    }
}

pub struct Spec {
    pub run: Run,
    pub cwd: PathBuf,
    /// Added to strobe's environment, after `unset` removed what it names (`PREFIX_*` for a family).
    pub env: Vec<(String, String)>,
    pub unset: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    /// The isolated home it launches for: a stop of strobe ends it, its job found with the home's
    /// processes (Windows; Unix finds the stamp and the process group).
    pub home: Option<PathBuf>,
}

/// Unix: a temporary folder of the program's own inside its throwaway home (`TMPDIR`, mode 0700), so
/// what it leaves there goes with the home and never meets the user's `/tmp`. Windows: none (the home
/// already sets `TEMP` and `TMP`).
pub fn private_tmp(home: &std::path::Path) -> Result<Vec<(String, String)>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let dir = home.join(".tmp");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        Ok(vec![("TMPDIR".into(), dir.display().to_string())])
    }
    #[cfg(windows)]
    {
        let _ = home;
        Ok(Vec::new())
    }
}

/// What the reader thread keeps: the emulator, and what it counted.
struct State {
    parser: vt100::Parser<Tracker>,
    /// Every read: when it returned and its bytes.
    reads: Vec<(u64, u32)>,
    eof: bool,
    /// The reader thread's CPU so far, as of its last read (`host::thread_cycles` units): its system
    /// calls, parsing and answers.
    cycles: u64,
}

/// The emulator's callbacks: the grid a terminal shows (committed at the end of each read, or at the
/// end of a synchronized frame), the updates, and the answers to queries, written after each read.
struct Tracker {
    /// The read being processed: when it returned.
    now: u64,
    rows: u16,
    cols: u16,
    committed: Vec<vt100::Cell>,
    cursor: (u16, u16),
    cursor_on: bool,
    /// An update begun and not committed yet: its first read's time.
    pending: Option<u64>,
    /// A synchronized frame open since then.
    sync: Option<u64>,
    sync_timeouts: u32,
    updates: Vec<Update>,
    /// The last update that changed cells.
    last_change: u64,
    replies: Vec<u8>,
    /// Progressive keyboard enhancement flags, pushed and popped by the program.
    kitty: Vec<u16>,
    title: String,
}

fn cells(screen: &vt100::Screen) -> Vec<vt100::Cell> {
    let (rows, cols) = screen.size();
    let mut out = Vec::with_capacity(rows.get() as usize * cols.get() as usize);
    for r in 0..rows.get() {
        for c in 0..cols.get() {
            if let Some(cell) = screen.cell(r, c) {
                out.push(cell.clone());
            }
        }
    }
    out
}

impl Tracker {
    /// The cells that differ from the committed grid, which then takes them.
    fn diff(&mut self, screen: &vt100::Screen) -> u32 {
        let (rows, cols) = screen.size();
        if (rows.get(), cols.get()) != (self.rows, self.cols) {
            let all = cells(screen);
            let n = all.len() as u32;
            (self.rows, self.cols, self.committed) = (rows.get(), cols.get(), all);
            return n;
        }
        let mut n = 0;
        for r in 0..self.rows {
            for c in 0..self.cols {
                let i = r as usize * self.cols as usize + c as usize;
                if let Some(cell) = screen.cell(r, c)
                    && self.committed.get(i) != Some(cell)
                {
                    self.committed[i] = cell.clone();
                    n += 1;
                }
            }
        }
        n
    }

    /// The grid as it is now is what the terminal shows: an update when cells changed, and always for a
    /// synchronized frame.
    fn commit(&mut self, screen: &vt100::Screen, synced: bool) {
        let cells = self.diff(screen);
        self.cursor = screen.cursor_position();
        self.cursor_on = !screen.hide_cursor();
        let start = self.pending.take().unwrap_or(self.now);
        let start = if synced { self.sync.take().unwrap_or(start) } else { start };
        if synced || cells > 0 {
            self.updates.push(Update { start_ns: start, commit_ns: self.now, cells, synced });
        }
        if cells > 0 {
            self.last_change = self.now;
        }
    }

    fn reply(&mut self, s: &str) {
        self.replies.extend_from_slice(s.as_bytes());
    }

    /// DECRQM's answer for a private mode: 1 set, 2 reset, 0 unknown.
    fn mode_state(&self, screen: &vt100::Screen, mode: u16) -> u8 {
        use vt100::MouseProtocolMode as M;
        let set = |b: bool| if b { 1 } else { 2 };
        match mode {
            1 => set(screen.application_cursor()),
            25 => set(!screen.hide_cursor()),
            1049 | 47 | 1047 => set(screen.alternate_screen()),
            2004 => set(screen.bracketed_paste()),
            1000 => set(screen.mouse_protocol_mode() == M::PressRelease),
            1002 => set(screen.mouse_protocol_mode() == M::ButtonMotion),
            1003 => set(screen.mouse_protocol_mode() == M::AnyMotion),
            1006 => set(screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr),
            2026 => set(self.sync.is_some()),
            _ => 0,
        }
    }
}

impl vt100::Callbacks for Tracker {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
        let p = |i: usize| params.get(i).and_then(|p| p.first()).copied().unwrap_or(0);
        let (rows, cols) = (self.rows as usize, self.cols as usize);
        match (i1, i2, c) {
            // Synchronized output: the frame shows whole at its end.
            (Some(b'?'), None, 'h') if params.iter().any(|p| p.first() == Some(&2026)) => {
                if self.sync.is_none() {
                    if self.pending.is_some() {
                        self.commit(screen, false);
                    }
                    self.sync = Some(self.now);
                    self.pending = Some(self.now);
                }
            }
            (Some(b'?'), None, 'l') if params.iter().any(|p| p.first() == Some(&2026)) => {
                if self.sync.is_some() {
                    self.commit(screen, true);
                }
            }
            // Primary device attributes: a VT220 with colour.
            (None, None, 'c') if p(0) == 0 => self.reply("\x1b[?62;22c"),
            // Secondary device attributes.
            (Some(b'>'), None, 'c') => self.reply("\x1b[>1;10;0c"),
            // Status report, cursor position (1-based).
            (None, None, 'n') if p(0) == 5 => self.reply("\x1b[0n"),
            (None, None, 'n') if p(0) == 6 => {
                let (r, c) = screen.cursor_position();
                self.reply(&format!("\x1b[{};{}R", r + 1, c + 1));
            }
            (Some(b'?'), None, 'n') if p(0) == 6 => {
                let (r, c) = screen.cursor_position();
                self.reply(&format!("\x1b[?{};{}R", r + 1, c + 1));
            }
            // Mode reports (DECRQM), private and ANSI.
            (Some(b'?'), Some(b'$'), 'p') => {
                let m = p(0);
                let s = self.mode_state(screen, m);
                self.reply(&format!("\x1b[?{m};{s}$y"));
            }
            (Some(b'$'), None, 'p') => self.reply(&format!("\x1b[{};0$y", p(0))),
            // Progressive keyboard enhancement: query, push, pop, set.
            (Some(b'?'), None, 'u') => {
                let f = self.kitty.last().copied().unwrap_or(0);
                self.reply(&format!("\x1b[?{f}u"));
            }
            (Some(b'>'), None, 'u') => self.kitty.push(p(0)),
            (Some(b'<'), None, 'u') => {
                for _ in 0..p(0).max(1) {
                    self.kitty.pop();
                }
            }
            (Some(b'='), None, 'u') => {
                let (f, how) = (p(0), params.get(1).and_then(|x| x.first()).copied().unwrap_or(1));
                let cur = self.kitty.pop().unwrap_or(0);
                self.kitty.push(match how {
                    2 => cur | f,
                    3 => cur & !f,
                    _ => f,
                });
            }
            // Window reports: text area and cell size in pixels, and the grid in cells.
            (None, None, 't') if p(0) == 14 => self.reply(&format!("\x1b[4;{};{}t", rows * CELL_H, cols * CELL_W)),
            (None, None, 't') if p(0) == 16 => self.reply(&format!("\x1b[6;{CELL_H};{CELL_W}t")),
            (None, None, 't') if p(0) == 18 => self.reply(&format!("\x1b[8;{rows};{cols}t")),
            // The terminal's name and version (XTVERSION).
            (Some(b'>'), None, 'q') => self.reply(&format!("\x1bP>|strobe {}\x1b\\", env!("CARGO_PKG_VERSION"))),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // Default foreground (white) and background (black), as a dark terminal answers.
        match params {
            [b"10", b"?"] => self.reply("\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
            [b"11", b"?"] => self.reply("\x1b]11;rgb:0000/0000/0000\x1b\\"),
            [b"12", b"?"] => self.reply("\x1b]12;rgb:ffff/ffff/ffff\x1b\\"),
            [b"4", n, b"?"] => {
                if let Some(i) = std::str::from_utf8(n).ok().and_then(|n| n.parse::<u8>().ok()) {
                    let (r, g, b) = palette(i);
                    self.reply(&format!("\x1b]4;{i};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\"));
                }
            }
            _ => {}
        }
    }
}

/// The 256-colour palette (xterm's): 16 system colours, a 6×6×6 cube, 24 greys.
pub fn palette(i: u8) -> (u8, u8, u8) {
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match i {
        0..=15 => SYSTEM[i as usize],
        16..=231 => {
            let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
            let n = i - 16;
            (v(n / 36), v(n / 6 % 6), v(n % 6))
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            (g, g, g)
        }
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// The committed grid, as a terminal shows it: rows of cells, the cursor, and the size.
pub struct Grid<'a> {
    pub cells: &'a [vt100::Cell],
    pub rows: u16,
    pub cols: u16,
    pub cursor: (u16, u16),
    /// The program shows the cursor (`?25`).
    pub cursor_on: bool,
    pub title: &'a str,
}

impl Grid<'_> {
    pub fn cell(&self, row: u16, col: u16) -> Option<&vt100::Cell> {
        (row < self.rows && col < self.cols).then(|| &self.cells[row as usize * self.cols as usize + col as usize])
    }

    /// A row's text and, per character, the column it starts at; a wide character's second cell adds
    /// nothing and an empty cell is a space.
    pub fn row(&self, row: u16) -> (String, Vec<u16>) {
        let mut text = String::new();
        let mut at = Vec::new();
        for c in 0..self.cols {
            let Some(cell) = self.cell(row, c) else { break };
            if cell.is_wide_continuation() {
                continue;
            }
            let s = if cell.has_contents() { cell.contents() } else { " " };
            for ch in s.chars() {
                text.push(ch);
                at.push(c);
            }
        }
        (text, at)
    }

    /// Every row's text, trailing spaces trimmed.
    pub fn text(&self) -> Vec<String> {
        (0..self.rows).map(|r| self.row(r).0.trim_end().to_string()).collect()
    }
}

/// The program's process, with its group.
struct Child {
    #[cfg(unix)]
    owned: host::spawn::Owned,
    /// The terminal's controlling end, for its size.
    #[cfg(unix)]
    master: std::os::fd::OwnedFd,
    #[cfg(windows)]
    win: win::Proc,
}

/// A program on a pseudo-terminal strobe owns, with the grid its output draws.
pub struct Tty {
    shared: Arc<Shared>,
    writer: Arc<Mutex<std::fs::File>>,
    reader: Option<std::thread::JoinHandle<()>>,
    child: Mutex<Child>,
    pub pid: u32,
    /// The pseudo-console's host process (Windows: the `conhost` that ConPTY starts), whose CPU is
    /// the terminal's, never the program's. None on Unix: the kernel is the terminal there.
    pub host_pids: Vec<u32>,
    /// When the program was started (host clock): after strobe set up its terminal (the pseudo-terminal,
    /// the console host, the reader thread), so a launch is the program's. Windows: right before the
    /// suspended process resumes. Unix: the owned child's release stamp (Linux: immediately before
    /// exec, after cgroup placement and CPU affinity; macOS: immediately before spawn).
    pub started_ns: u64,
}

impl Tty {
    /// Starts `spec.run` on a new pseudo-terminal of `spec.cols` × `spec.rows`, pinned to `cpus`; the
    /// reader thread on `reader_cpus` when given (strobe's own, where the creating thread's pin is no
    /// new thread's: Windows).
    pub fn spawn(spec: &Spec, cpus: Option<u64>, reader_cpus: Option<u64>) -> Result<Tty> {
        Self::spawn_observed(spec, cpus, reader_cpus, |_| ()).map(|(tty, ())| tty)
    }

    /// A measured launch can start observing the identified console host before the program runs.
    pub(crate) fn spawn_observed<T>(spec: &Spec, cpus: Option<u64>, reader_cpus: Option<u64>, observe: impl FnOnce(&[u32]) -> T) -> Result<(Tty, T)> {
        if spec.cols == 0 || spec.rows == 0 {
            return Err(Error::usage("a terminal needs at least one column and one row"));
        }
        let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !crate::target::isolate::env_unset(&spec.unset, k)).collect();
        // A terminal names itself; the target's own `env` may say otherwise.
        env.retain(|(k, _)| !crate::target::isolate::env_name_is(k, "TERM"));
        env.push(("TERM".into(), "xterm-256color".into()));
        for (k, v) in &spec.env {
            env.retain(|(e, _)| !crate::target::isolate::env_name_is(e, k));
            env.push((k.clone(), v.clone()));
        }
        let (pending, read, write, host_pids) = open(spec, &env, cpus)?;
        let set_up = host::now_ns();
        let parser = vt100::Parser::new_with_callbacks(
            std::num::NonZeroU16::new(spec.rows).unwrap_or(vt100::Parser::DEFAULT_ROWS),
            std::num::NonZeroU16::new(spec.cols).unwrap_or(vt100::Parser::DEFAULT_COLS),
            0,
            Tracker {
                now: set_up,
                rows: 0,
                cols: 0,
                committed: Vec::new(),
                cursor: (0, 0),
                cursor_on: true,
                pending: None,
                sync: None,
                sync_timeouts: 0,
                updates: Vec::new(),
                last_change: set_up,
                replies: Vec::new(),
                kitty: Vec::new(),
                title: String::new(),
            },
        );
        let mut state = State { parser, reads: Vec::new(), eof: false, cycles: 0 };
        let (screen, tr) = state.parser.screen_and_callbacks_mut();
        tr.diff(screen);
        let shared = Arc::new(Shared { state: Mutex::new(state), changed: Condvar::new() });
        let writer = Arc::new(Mutex::new(write));
        let (s, w) = (shared.clone(), writer.clone());
        // The reader reads before the program runs: its first output is stamped when it comes.
        let reader = std::thread::Builder::new().name("strobe-tty".into()).spawn(move || {
            if let Some(m) = reader_cpus {
                host::set_thread_affinity(m);
            }
            read_loop(read, &s, &w)
        })?;
        let observation = observe(&host_pids);
        let (child, pid, started_ns) = start(pending, spec)?;
        Ok((Tty { shared, writer, reader: Some(reader), child: Mutex::new(child), pid, host_pids, started_ns }, observation))
    }

    /// The terminal's line discipline echoes input itself (Unix `ECHO`, or `ECHONL` in canonical mode):
    /// the grid then shows the kernel's echo before the program reads the input. False on Windows,
    /// where no other process can read the console's input mode (SPEC §13.9: programs there are
    /// measured in raw mode).
    pub fn line_echo(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let c = self.child.lock().unwrap_or_else(|e| e.into_inner());
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(c.master.as_raw_fd(), &mut t) } != 0 {
                return false;
            }
            t.c_lflag & libc::ECHO != 0 || (t.c_lflag & libc::ECHONL != 0 && t.c_lflag & libc::ICANON != 0)
        }
        #[cfg(windows)]
        {
            false
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes input to the program; returns the host clock just before the write.
    pub fn write(&self, bytes: &[u8]) -> Result<u64> {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let at = host::now_ns();
        w.write_all(bytes).and_then(|_| w.flush()).map_err(|e| Error::msg(format!("the terminal's input: {e} (did the program exit?)")))?;
        Ok(at)
    }

    /// Sizes the terminal (the program gets `SIGWINCH`, or ConPTY's resize) and the grid.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let (Some(r), Some(c)) = (std::num::NonZeroU16::new(rows), std::num::NonZeroU16::new(cols)) else {
            return Err(Error::usage("page.resize(cols, rows): at least one column and one row"));
        };
        {
            // The grid takes the size at once, as a terminal's window does: that is the terminal's
            // change, no update of the program's (its redraw comes after it).
            let mut st = self.state();
            st.parser.set_size(r, c);
            let (screen, tr) = st.parser.screen_and_callbacks_mut();
            tr.diff(screen);
            tr.cursor = screen.cursor_position();
        }
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        resize(&child, cols, rows)
    }

    /// Reads the committed grid.
    pub fn grid<R>(&self, f: impl FnOnce(&Grid) -> R) -> R {
        let st = self.state();
        let tr = st.parser.callbacks();
        f(&Grid { cells: &tr.committed, rows: tr.rows, cols: tr.cols, cursor: tr.cursor, cursor_on: tr.cursor_on, title: &tr.title })
    }

    pub fn modes(&self) -> Modes {
        use vt100::MouseProtocolMode as M;
        let st = self.state();
        let s = st.parser.screen();
        Modes {
            mouse: match s.mouse_protocol_mode() {
                M::None => Mouse::Off,
                M::Press | M::PressRelease => Mouse::Clicks,
                M::ButtonMotion => Mouse::Drags,
                M::AnyMotion => Mouse::Motion,
            },
            sgr: s.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr,
            app_cursor: s.application_cursor(),
            kitty: st.parser.callbacks().kitty.last().copied().unwrap_or(0),
            bracketed_paste: s.bracketed_paste(),
        }
    }

    /// Waits until `done` holds on the committed grid (checked now and at each read), the program's
    /// output ends (false), or `deadline` (false). `done` gets the grid, the time of the last change and
    /// how many updates there were.
    pub fn wait(&self, deadline: Instant, mut done: impl FnMut(&Grid, u64, usize) -> bool) -> bool {
        let mut st = self.state();
        loop {
            {
                let tr = st.parser.callbacks();
                let g = Grid { cells: &tr.committed, rows: tr.rows, cols: tr.cols, cursor: tr.cursor, cursor_on: tr.cursor_on, title: &tr.title };
                if done(&g, tr.last_change, tr.updates.len()) {
                    return true;
                }
            }
            if st.eof {
                return false;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
            st = self.shared.changed.wait_timeout(st, left).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Waits until the grid has held still for `quiet` since its last change and since `since_ns` (an
    /// input: what it causes may not have come yet), true, or until `deadline` (false). Woken by each
    /// read, and by the time the grid would have held still that long: no polling.
    pub fn quiet(&self, quiet: std::time::Duration, since_ns: u64, deadline: Instant) -> bool {
        let quiet_ns = quiet.as_nanos() as u64;
        let mut st = self.state();
        loop {
            let still = host::now_ns().saturating_sub(st.parser.callbacks().last_change.max(since_ns));
            // A synchronized frame left open is the program drawing, whatever the grid shows yet: woken
            // by its end's read.
            let open = st.parser.callbacks().sync.is_some();
            if still >= quiet_ns && !open {
                return true;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
            let wait = if open { left } else { left.min(std::time::Duration::from_nanos(quiet_ns - still)) };
            st = self.shared.changed.wait_timeout(st, wait).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Waits for the first update that changed cells and began with a read after `after_ns` (output
    /// the program wrote after an input written then), until `deadline`: its commit time.
    pub fn first_change(&self, after_ns: u64, deadline: Instant) -> Option<u64> {
        let mut st = self.state();
        loop {
            let found = st.parser.callbacks().updates.iter().rev().take_while(|u| u.start_ns >= after_ns).filter(|u| u.cells > 0).last().map(|u| u.commit_ns);
            if found.is_some() || st.eof {
                return found;
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            st = self.shared.changed.wait_timeout(st, left).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// The grid's size now: (columns, rows).
    pub fn size(&self) -> (u16, u16) {
        let st = self.state();
        let tr = st.parser.callbacks();
        (tr.cols, tr.rows)
    }

    /// When the grid last changed, and how many updates there were so far.
    pub fn last_change(&self) -> (u64, usize) {
        let st = self.state();
        let tr = st.parser.callbacks();
        (tr.last_change, tr.updates.len())
    }

    /// The updates committed in `[t0, t1]`.
    pub fn updates(&self, t0: u64, t1: u64) -> Vec<Update> {
        self.state().parser.callbacks().updates.iter().filter(|u| u.commit_ns >= t0 && u.commit_ns <= t1).copied().collect()
    }

    /// The bytes the program wrote that arrived in `[t0, t1]`.
    pub fn bytes(&self, t0: u64, t1: u64) -> u64 {
        self.state().reads.iter().filter(|r| r.0 >= t0 && r.0 <= t1).map(|r| r.1 as u64).sum()
    }

    /// The reader thread's whole CPU so far, as of its last read (`host::thread_cycles` units).
    pub fn reader_cycles(&self) -> u64 {
        self.state().cycles
    }

    /// Synchronized frames that never ended within a second (ended by strobe).
    pub fn sync_timeouts(&self) -> u32 {
        self.state().parser.callbacks().sync_timeouts
    }

    /// The program's output has ended (it exited, or closed the terminal).
    pub fn ended(&self) -> bool {
        self.state().eof
    }

    /// The program's exit code once it has exited.
    pub fn exited(&self) -> Option<i32> {
        let mut c = self.child.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(unix)]
        {
            c.owned.exited()
        }
        #[cfg(windows)]
        {
            c.win.exited()
        }
    }

    /// Ends the program's group, then the terminal, and waits for the reader to see its end.
    pub fn close(mut self) {
        self.end();
    }

    fn end(&mut self) {
        {
            let mut c = self.child.lock().unwrap_or_else(|e| e.into_inner());
            #[cfg(unix)]
            c.owned.end();
            #[cfg(windows)]
            c.win.end();
        }
        if let Some(r) = self.reader.take() {
            // The reader ends at the terminal's end of output; one held open by a leftover reader of
            // the terminal (a detached grandchild) is left to the process's own end.
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            while !r.is_finished() && Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if r.is_finished() {
                let _ = r.join();
            }
        }
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        self.end();
    }
}

/// The reader thread: each read is processed whole, its answers written, and its waiters woken.
fn read_loop(mut read: std::fs::File, shared: &Shared, writer: &Mutex<std::fs::File>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match read.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let t = host::now_ns();
        let replies = {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            st.reads.push((t, n as u32));
            {
                let tr = st.parser.callbacks_mut();
                tr.now = t;
                if tr.sync.is_some_and(|s| t.saturating_sub(s) > SYNC_TIMEOUT_NS) {
                    tr.sync_timeouts += 1;
                }
            }
            // A frame left open past the timeout is no frame: what it drew shows as a plain update.
            if st.parser.callbacks().sync.is_some_and(|s| t.saturating_sub(s) > SYNC_TIMEOUT_NS) {
                let (screen, tr) = st.parser.screen_and_callbacks_mut();
                tr.sync = None;
                tr.commit(screen, false);
            }
            st.parser.callbacks_mut().pending.get_or_insert(t);
            st.parser.process(&buf[..n]);
            // Outside a frame the read's end is what shows, bytes after a frame's end in it too.
            let (screen, tr) = st.parser.screen_and_callbacks_mut();
            if tr.sync.is_none() {
                tr.commit(screen, false);
            }
            std::mem::take(&mut tr.replies)
        };
        if !replies.is_empty() {
            let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
            let _ = w.write_all(&replies).and_then(|_| w.flush());
        }
        {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            // The thread's whole CPU so far: its reads' system calls too, not only its parsing.
            st.cycles = host::thread_cycles();
        }
        shared.changed.notify_all();
    }
    shared.state.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
    shared.changed.notify_all();
}

/// The program set up on its terminal and not running yet: `start` runs it.
#[cfg(unix)]
struct Pending {
    cmd: std::process::Command,
    master: std::os::fd::OwnedFd,
    cpus: Option<u64>,
}

#[cfg(windows)]
struct Pending(win::Proc);

type Opened = (Pending, std::fs::File, std::fs::File, Vec<u32>);

/// Runs the pending program: the host clock when it began (`Tty::started_ns`).
#[cfg(unix)]
fn start(p: Pending, spec: &Spec) -> Result<(Child, u32, u64)> {
    let Pending { mut cmd, master, cpus } = p;
    let owned = host::spawn::spawn_owned_terminal(&mut cmd, cpus, spec.home.as_deref()).map_err(|e| Error::msg(format!("start {}: {e}", spec.run.text())))?;
    let at = owned.released_ns();
    drop(cmd);
    let pid = owned.pid();
    Ok((Child { owned, master }, pid, at))
}

#[cfg(windows)]
fn start(p: Pending, _: &Spec) -> Result<(Child, u32, u64)> {
    let mut win = p.0;
    let at = win.resume();
    let pid = win.pid;
    Ok((Child { win }, pid, at))
}

#[cfg(unix)]
fn open(spec: &Spec, env: &[(String, String)], cpus: Option<u64>) -> Result<Opened> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    let mut ws = libc::winsize { ws_row: spec.rows, ws_col: spec.cols, ws_xpixel: spec.cols * CELL_W as u16, ws_ypixel: spec.rows * CELL_H as u16 };
    // Linux takes the size as const, macOS as mut: a raw pointer suits both.
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &raw mut ws) } != 0 {
        return Err(Error::msg(format!("openpty: {}", std::io::Error::last_os_error())));
    }
    // Neither end leaks into what strobe starts later: the program gets the terminal as its stdio.
    unsafe {
        libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut cmd = match &spec.run {
        Run::Argv(a) if !a.is_empty() => {
            let mut c = std::process::Command::new(&a[0]);
            c.args(&a[1..]);
            c
        }
        Run::Argv(_) => return Err(Error::usage("tty(): an empty command")),
        Run::Line(l) => {
            let mut c = std::process::Command::new("/bin/sh");
            c.arg("-c").arg(format!("exec {l}"));
            c
        }
    };
    cmd.env_clear().envs(env.iter().cloned()).current_dir(&spec.cwd);
    cmd.stdin(slave.try_clone()?).stdout(slave.try_clone()?).stderr(slave);
    let read = std::fs::File::from(master.try_clone()?);
    let write = std::fs::File::from(master.try_clone()?);
    Ok((Pending { cmd, master, cpus }, read, write, Vec::new()))
}

#[cfg(unix)]
fn resize(child: &Child, cols: u16, rows: u16) -> Result<()> {
    use std::os::fd::AsRawFd;
    let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: cols * CELL_W as u16, ws_ypixel: rows * CELL_H as u16 };
    // The kernel sends the terminal's foreground process group SIGWINCH.
    if unsafe { libc::ioctl(child.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) } != 0 {
        return Err(Error::msg(format!("resize the terminal: {}", std::io::Error::last_os_error())));
    }
    Ok(())
}

#[cfg(windows)]
fn open(spec: &Spec, env: &[(String, String)], cpus: Option<u64>) -> Result<Opened> {
    let (win, read, write, host_pids) = win::Proc::open(spec, env, cpus)?;
    Ok((Pending(win), read, write, host_pids))
}

#[cfg(windows)]
fn resize(child: &Child, cols: u16, rows: u16) -> Result<()> {
    child.win.resize(cols, rows)
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole};
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, GetProcessAffinityMask,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        SetProcessAffinityMask, TerminateProcess, UpdateProcThreadAttribute,
    };

    pub struct Proc {
        pub pid: u32,
        process: HANDLE,
        /// Its first thread, held suspended until `resume`.
        thread: HANDLE,
        job: Option<HANDLE>,
        hpc: Option<HPCON>,
    }

    // The handles are the process's own, used under the Tty's lock.
    unsafe impl Send for Proc {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The command line with a relative program path (one holding `/` or `\`) made absolute from `cwd`
    /// (`.exe` added when it has no extension and that file is there): CreateProcessW would resolve it from
    /// strobe's own folder. A bare name is left to the search Windows does.
    fn from_cwd(line: &str, cwd: &std::path::Path) -> String {
        let line = line.trim_start();
        let (program, rest) = match line.strip_prefix('"') {
            Some(q) => q.split_once('"').unwrap_or((q, "")),
            None => line.split_once(char::is_whitespace).unwrap_or((line, "")),
        };
        let path = std::path::Path::new(program);
        if !path.is_relative() || !program.contains(['/', '\\']) {
            return line.to_string();
        }
        let joined = cwd.join(path);
        let exe = joined.with_extension("exe");
        let joined = if path.extension().is_none() && exe.is_file() { exe } else { joined };
        let shown = joined.display().to_string().replace('/', "\\");
        if rest.is_empty() { quote(&shown) } else { format!("{} {}", quote(&shown), rest.trim_start()) }
    }

    /// One argument as `CommandLineToArgvW` reads it back.
    fn quote(a: &str) -> String {
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            return a.to_string();
        }
        let mut out = String::from("\"");
        let mut slashes = 0;
        for c in a.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.push_str(&"\\".repeat(slashes * 2 + 1));
                    out.push('"');
                    slashes = 0;
                }
                c => {
                    out.push_str(&"\\".repeat(slashes));
                    out.push(c);
                    slashes = 0;
                }
            }
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
        out
    }

    /// The pseudo-console host processes strobe has now (ConPTY starts one per console, as strobe's child).
    fn hosts() -> Vec<u32> {
        host::process_tree(std::process::id())
            .into_iter()
            .filter(|p| p.parent == std::process::id() && (p.name.eq_ignore_ascii_case("conhost.exe") || p.name.eq_ignore_ascii_case("OpenConsole.exe")))
            .map(|p| p.pid)
            .collect()
    }

    impl Proc {
        pub fn open(spec: &Spec, env: &[(String, String)], cpus: Option<u64>) -> Result<(Proc, std::fs::File, std::fs::File, Vec<u32>)> {
            let before = hosts();
            unsafe {
                let (mut in_read, mut in_write, mut out_read, mut out_write): (HANDLE, HANDLE, HANDLE, HANDLE) =
                    (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 || CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                    return Err(Error::msg(format!("the terminal's pipes: {}", std::io::Error::last_os_error())));
                }
                let mut hpc: HPCON = 0;
                let hr = CreatePseudoConsole(COORD { X: spec.cols as i16, Y: spec.rows as i16 }, in_read, out_write, 0, &mut hpc);
                // The console host holds its own ends now.
                CloseHandle(in_read);
                CloseHandle(out_write);
                let read = std::fs::File::from_raw_handle(out_read as _);
                let write = std::fs::File::from_raw_handle(in_write as _);
                if hr < 0 {
                    return Err(Error::msg(format!("CreatePseudoConsole failed (0x{hr:08x}): ConPTY needs Windows 10 1809 or later")));
                }
                let host_pids: Vec<u32> = hosts().into_iter().filter(|p| !before.contains(p)).collect();
                if let Some(mask) = cpus.filter(|m| *m != 0) {
                    // Its cycles are subtracted from busy time on these CPUs. Never pin an
                    // unidentified process or proceed with the host running on the runner's CPUs.
                    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION};
                    let pinned = if let [pid] = host_pids.as_slice() {
                        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION, 0, *pid);
                        if h.is_null() {
                            false
                        } else {
                            let (mut proc_mask, mut sys_mask) = (0, 0);
                            let ok = hosts().contains(pid) && GetProcessAffinityMask(h, &mut proc_mask, &mut sys_mask) != 0 && SetProcessAffinityMask(h, mask as usize & sys_mask) != 0;
                            CloseHandle(h);
                            ok
                        }
                    } else {
                        false
                    };
                    if !pinned {
                        ClosePseudoConsole(hpc);
                        return Err(Error::refused("RunIncomplete", "could not identify and pin the new console host on the target's CPUs", "retry the terminal launch or use --pin none"));
                    }
                }
                let mut size = 0usize;
                InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
                let mut list = vec![0u8; size];
                let attrs = list.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
                if InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) == 0
                    || UpdateProcThreadAttribute(
                        attrs,
                        0,
                        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                        hpc as *const std::ffi::c_void,
                        std::mem::size_of::<HPCON>(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    ) == 0
                {
                    ClosePseudoConsole(hpc);
                    return Err(Error::msg(format!("the terminal's attribute list: {}", std::io::Error::last_os_error())));
                }
                let mut si: STARTUPINFOEXW = std::mem::zeroed();
                si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
                // Invalid handles, not strobe's own: a child given none of its parent's (redirected)
                // stdio takes the pseudo-console's.
                si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
                si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
                si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
                si.lpAttributeList = attrs;
                let line = match &spec.run {
                    Run::Line(l) => l.clone(),
                    Run::Argv(a) if !a.is_empty() => a.iter().map(|x| quote(x)).collect::<Vec<_>>().join(" "),
                    Run::Argv(_) => {
                        ClosePseudoConsole(hpc);
                        return Err(Error::usage("tty(): an empty command"));
                    }
                };
                let line = from_cwd(&line, &spec.cwd);
                let mut cmdline = wide(&line);
                let mut sorted: Vec<&(String, String)> = env.iter().collect();
                sorted.sort_by_key(|(k, _)| k.to_uppercase());
                let mut block: Vec<u16> = sorted.iter().flat_map(|(k, v)| format!("{k}={v}").encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>()).collect();
                block.push(0);
                let cwd = wide(&spec.cwd.display().to_string());
                let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
                let ok = CreateProcessW(
                    std::ptr::null(),
                    cmdline.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                    block.as_ptr() as *const std::ffi::c_void,
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                );
                DeleteProcThreadAttributeList(attrs);
                if ok == 0 {
                    let e = std::io::Error::last_os_error();
                    ClosePseudoConsole(hpc);
                    return Err(Error::msg(format!("start {line}: {e}")));
                }
                // One job holds the program and what it starts (each process held from its start,
                // `host::jobs`); Windows ends it with strobe. strobe's own job may refuse a nested one:
                // then the tree and the home's stamp find it.
                let job = match host::spawn::new_job().map(|j| (j, host::spawn::assign(j, pi.hProcess as isize, pi.dwProcessId))) {
                    Some((job, Ok(true))) => Some(job as HANDLE),
                    Some((job, Ok(false))) => {
                        host::spawn::close_job(job);
                        None
                    }
                    // Closing the launch's job ends the program, which has not run yet.
                    Some((job, Err(e))) => {
                        host::spawn::close_job(job);
                        CloseHandle(pi.hThread);
                        CloseHandle(pi.hProcess);
                        ClosePseudoConsole(hpc);
                        return Err(Error::msg(format!("start {line}: {e}")));
                    }
                    None => None,
                };
                if let Some(mask) = cpus
                    && let Err(e) = host::spawn::pin_child(pi.hProcess, mask)
                {
                    TerminateProcess(pi.hProcess, 1);
                    if let Some(j) = job {
                        host::spawn::close_job(j as isize);
                    }
                    CloseHandle(pi.hThread);
                    CloseHandle(pi.hProcess);
                    ClosePseudoConsole(hpc);
                    return Err(Error::msg(format!("pin terminal program to the requested CPUs: {e}")));
                }
                // A stop of strobe ends it with the launches it owns (`spawn::end_launches`).
                host::spawn::list_launch(pi.dwProcessId, job.map(|j| j as isize), spec.home.as_deref());
                Ok((Proc { pid: pi.dwProcessId, process: pi.hProcess, thread: pi.hThread, job, hpc: Some(hpc) }, read, write, host_pids))
            }
        }

        /// Lets the program run: the host clock right before.
        pub fn resume(&mut self) -> u64 {
            let at = host::now_ns();
            unsafe {
                ResumeThread(self.thread);
                CloseHandle(self.thread);
            }
            self.thread = std::ptr::null_mut();
            at
        }

        pub fn exited(&mut self) -> Option<i32> {
            let mut code = 0u32;
            // STILL_ACTIVE (259) while it runs.
            (unsafe { GetExitCodeProcess(self.process, &mut code) } != 0 && code != 259).then_some(code as i32)
        }

        pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
            let Some(hpc) = self.hpc else { return Ok(()) };
            let hr = unsafe { ResizePseudoConsole(hpc, COORD { X: cols as i16, Y: rows as i16 }) };
            if hr < 0 { Err(Error::msg(format!("ResizePseudoConsole failed (0x{hr:08x})"))) } else { Ok(()) }
        }

        /// The program's tree and job, then the console: its host exits and the reader sees the end.
        pub fn end(&mut self) {
            // Once: after it the pid may name another process.
            if self.process.is_null() {
                return;
            }
            host::spawn::unlist_launch(self.pid, self.job.map(|j| j as isize));
            // strobe's own child, its handle still held (`self.process`): its pid names it.
            host::kill_launch(self.pid, None);
            unsafe {
                if let Some(job) = self.job.take() {
                    TerminateJobObject(job, 1);
                    host::spawn::close_job(job as isize);
                }
                if let Some(hpc) = self.hpc.take() {
                    ClosePseudoConsole(hpc);
                }
                if !self.process.is_null() {
                    CloseHandle(self.process);
                    self.process = std::ptr::null_mut();
                }
            }
        }
    }

    impl Drop for Proc {
        fn drop(&mut self) {
            self.end();
            if !self.thread.is_null() {
                unsafe { CloseHandle(self.thread) };
            }
        }
    }
}
