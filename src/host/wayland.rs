//! Native Wayland windows in a compositor strobe runs itself (SPEC §13.7, `target::wayland`): a
//! headless sway that only the launched app and strobe connect to. Wayland shows no client another
//! client's windows, so strobe asks the compositor: its IPC (`$SWAYSOCK`, the i3 protocol) lists each
//! window with the process that owns its connection, its place and size, whether it shows and has
//! the focus, and moves and sizes it; strobe's own Wayland client (`client`) copies the output's
//! pixels and gives input through the compositor's seat.
//!
//! A window id (`TopWindow::hwnd`) is the compositor's sequence number in the high 32 bits and sway's
//! container id in the low ones, so it never names an X11 window (XIDs are 32 bits). Coordinates are
//! the compositor's layout, which strobe sets up at scale 1: one unit is one pixel.

mod client;

pub use client::Grab;

use super::{Placement, TopWindow};
use crate::model::{Error, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// A compositor strobe runs, while its owner (`target::wayland::Session`) keeps it.
pub struct Compositor {
    seq: u32,
    /// The Wayland socket and the IPC socket.
    pub socket: PathBuf,
    ipc: PathBuf,
    /// What it composites with: `pixman` (software) or a GPU renderer.
    pub renderer: String,
    /// The compositor and its version (`sway 1.10.1`).
    pub name: String,
    /// The launch's own session bus, where its app's accessibility tree is found.
    pub bus: String,
    client: Mutex<Option<client::Client>>,
    /// strobe's keyboard as it is: its keymap and the modifiers held, given again to a client connected
    /// after an error (a new keymap also clears the modifiers).
    keyboard: Mutex<(Option<String>, u32)>,
}

static LIVE: Mutex<Vec<Weak<Compositor>>> = Mutex::new(Vec::new());
static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Makes a running compositor's windows strobe's to find (`main_window` and the rest) for as long as
/// the returned handle lives.
pub fn register(socket: &Path, ipc: &Path, renderer: &str, name: &str, bus: &str) -> Arc<Compositor> {
    let c = Arc::new(Compositor {
        seq: SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        socket: socket.to_path_buf(),
        ipc: ipc.to_path_buf(),
        renderer: renderer.to_string(),
        name: name.to_string(),
        bus: bus.to_string(),
        client: Mutex::new(None),
        keyboard: Mutex::new((None, 0)),
    });
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    live.retain(|w| w.strong_count() > 0);
    live.push(Arc::downgrade(&c));
    c
}

fn live() -> Vec<Arc<Compositor>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect()
}

/// The compositor strobe runs that shows a window of `pids`, if one does.
pub fn showing(pids: &[u32]) -> Option<Arc<Compositor>> {
    live().into_iter().find(|c| c.windows().iter().any(|w| pids.contains(&w.pid)))
}

/// Whether `hwnd` names a window of a compositor strobe runs (not an X11 window).
pub fn owns(hwnd: isize) -> bool {
    (hwnd as u64) >> 32 != 0
}

fn of(hwnd: isize) -> Option<(Arc<Compositor>, i64)> {
    let seq = ((hwnd as u64) >> 32) as u32;
    let c = live().into_iter().find(|c| c.seq == seq)?;
    Some((c, (hwnd as u64 & 0xffff_ffff) as i64))
}

/// One window as the compositor lists it.
#[derive(Clone, Debug)]
struct Win {
    id: i64,
    pid: u32,
    /// The container, and the content inside it (the client area), on the layout.
    rect: [i32; 4],
    content: [i32; 4],
    visible: bool,
    focused: bool,
    /// In the scratchpad: hidden, as a minimized window is.
    hidden: bool,
    title: String,
}

fn rect(v: &Value) -> [i32; 4] {
    ["x", "y", "width", "height"].map(|k| v[k].as_i64().unwrap_or(0) as i32)
}

impl Compositor {
    /// One IPC request (`kind`: 0 run command, 3 outputs, 4 tree) and its reply, within 2 s.
    fn ipc(&self, kind: u32, payload: &str) -> Option<Value> {
        let mut s = UnixStream::connect(&self.ipc).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        s.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
        let mut msg = b"i3-ipc".to_vec();
        msg.extend((payload.len() as u32).to_ne_bytes());
        msg.extend(kind.to_ne_bytes());
        msg.extend(payload.as_bytes());
        s.write_all(&msg).ok()?;
        let mut head = [0u8; 14];
        s.read_exact(&mut head).ok()?;
        let len = u32::from_ne_bytes(head[6..10].try_into().ok()?) as usize;
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).ok()?;
        serde_json::from_slice(&body).ok()
    }

    /// Runs compositor commands (`[con_id=7] move position 40 40`); fails with what it said.
    pub fn command(&self, cmd: &str) -> Result<()> {
        let reply = self.ipc(0, cmd).ok_or_else(|| Error::msg("the Wayland compositor did not answer (it exited?)"))?;
        match reply.as_array().into_iter().flatten().find(|r| r["success"] != true) {
            Some(r) => Err(Error::msg(format!("the Wayland compositor refused {cmd:?}: {}", r["error"].as_str().unwrap_or("?")))),
            None => Ok(()),
        }
    }

    /// Every window, bottom to top: tiled ones, then floating ones in their stacking order.
    fn windows(&self) -> Vec<Win> {
        fn walk(n: &Value, hidden: bool, out: &mut Vec<Win>) {
            let hidden = hidden || n["name"] == "__i3_scratch";
            if n["pid"].as_u64().is_some_and(|p| p > 0) {
                let r = rect(&n["rect"]);
                let w = rect(&n["window_rect"]);
                out.push(Win {
                    id: n["id"].as_i64().unwrap_or(0),
                    pid: n["pid"].as_u64().unwrap_or(0) as u32,
                    rect: r,
                    content: [r[0] + w[0], r[1] + w[1], w[2], w[3]],
                    visible: n["visible"] == true,
                    focused: n["focused"] == true,
                    hidden,
                    title: n["name"].as_str().unwrap_or("").to_string(),
                });
            }
            for k in ["nodes", "floating_nodes"] {
                for c in n[k].as_array().into_iter().flatten() {
                    walk(c, hidden, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(tree) = self.ipc(4, "") {
            walk(&tree, false, &mut out);
        }
        out
    }

    /// The output's rectangle on the layout (strobe's compositor has one).
    pub fn output(&self) -> Option<[i32; 4]> {
        let outs = self.ipc(3, "")?;
        outs.as_array()?.iter().find(|o| o["active"] == true).map(|o| rect(&o["rect"]))
    }

    fn id(&self, w: &Win) -> isize {
        ((self.seq as u64) << 32 | w.id as u64) as isize
    }

    fn top(&self, w: &Win, all: &[Win]) -> TopWindow {
        TopWindow {
            hwnd: self.id(w),
            pid: w.pid,
            rect: (w.rect[0], w.rect[1], w.rect[2], w.rect[3]),
            client: (w.content[2], w.content[3]),
            visible: w.visible || w.hidden,
            iconic: w.hidden,
            cloaked: w.visible && covered(w, all) >= 0.99,
            dpi: 96,
        }
    }

    /// Runs `f` on strobe's client of this compositor (connected on first use, again after an error).
    fn with_client<T>(&self, f: impl FnOnce(&mut client::Client) -> Result<T>) -> Result<T> {
        let mut guard = self.client.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let mut c = client::Client::connect(&self.socket)?;
            let kb = self.keyboard.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let (Some(k), mods) = kb {
                c.keymap(&k)?;
                c.modifiers(mods)?;
            }
            *guard = Some(c);
        }
        let c = guard.as_mut().ok_or_else(|| Error::msg("Wayland: no client"))?;
        let r = f(c);
        if r.is_err() {
            *guard = None;
        }
        r
    }

    /// A copy of the layout rectangle `r` (clipped to the output), waiting for it to change first with
    /// `damage`; None when nothing came within `wait`.
    pub fn grab(&self, r: [i32; 4], damage: bool, wait: Duration) -> Result<Option<Grab>> {
        let out = self.output().ok_or_else(|| Error::msg("the Wayland compositor shows no output"))?;
        let (x0, y0) = (r[0].max(out[0]), r[1].max(out[1]));
        let (x1, y1) = ((r[0] + r[2]).min(out[0] + out[2]), (r[1] + r[3]).min(out[1] + out[3]));
        if x1 <= x0 || y1 <= y0 {
            return Ok(None);
        }
        let deadline = Instant::now() + wait;
        self.with_client(|c| c.grab([x0 - out[0], y0 - out[1], x1 - x0, y1 - y0], damage, true, deadline))
    }

    /// A connection of its own that tells when a window changes, apart from the one input goes through.
    pub fn watch(self: &Arc<Compositor>) -> Result<Watch> {
        Ok(Watch { c: self.clone(), client: client::Client::connect(&self.socket)?, at: None })
    }

    /// The seat's pointer to the layout point `(x, y)`.
    pub fn pointer_to(&self, x: f64, y: f64) -> Result<u64> {
        let out = self.output().ok_or_else(|| Error::msg("the Wayland compositor shows no output"))?;
        self.with_client(|c| c.pointer_to(x - out[0] as f64, y - out[1] as f64, (out[2].max(1) as u32, out[3].max(1) as u32)))
    }

    pub fn button(&self, button: u32, down: bool) -> Result<u64> {
        self.with_client(|c| c.button(button, down))
    }

    pub fn scroll(&self, dx: f64, dy: f64) -> Result<u64> {
        self.with_client(|c| c.scroll(dx, dy))
    }

    /// Gives the seat strobe's keyboard (with `keymap`) and pointer, as a person's seat has them before
    /// an app starts: an app binds the seat's keyboard and pointer when it starts, and a window only
    /// takes keys after the compositor gave it the keyboard's focus.
    pub fn seat(&self, keymap: &str) -> Result<()> {
        self.keymap(keymap)?;
        self.with_client(|c| {
            c.add_pointer()?;
            c.sync(Instant::now() + Duration::from_secs(2))
        })
    }

    /// Gives strobe's keyboard `keymap`, and the modifiers held again (a new keymap clears them).
    pub fn keymap(&self, keymap: &str) -> Result<()> {
        let mods = {
            let kb = self.keyboard.lock().unwrap_or_else(|e| e.into_inner());
            if kb.0.as_deref() == Some(keymap) {
                return Ok(());
            }
            kb.1
        };
        self.with_client(|c| {
            c.keymap(keymap)?;
            c.modifiers(mods)
        })?;
        // Kept once given: a client connected later gets it then.
        self.keyboard.lock().unwrap_or_else(|e| e.into_inner()).0 = Some(keymap.to_string());
        Ok(())
    }

    pub fn key(&self, code: u32, down: bool) -> Result<u64> {
        self.with_client(|c| c.key(code, down))
    }

    /// The modifiers held (XKB's bits), kept for a client connected after an error.
    pub fn modifiers(&self, depressed: u32) -> Result<()> {
        self.keyboard.lock().unwrap_or_else(|e| e.into_inner()).1 = depressed;
        self.with_client(|c| c.modifiers(depressed))
    }

    /// The topmost showing window at a layout point: (window, pid, title).
    pub fn window_at(&self, x: i32, y: i32) -> Option<(isize, u32, String)> {
        let all = self.windows();
        let hit = all.iter().rev().find(|w| w.visible && x >= w.rect[0] && y >= w.rect[1] && x < w.rect[0] + w.rect[2] && y < w.rect[1] + w.rect[3])?;
        Some((self.id(hit), hit.pid, hit.title.clone()))
    }

    /// Waits until the compositor handled every input sent so far.
    pub fn sync(&self) -> Result<()> {
        self.with_client(|c| c.sync(Instant::now() + Duration::from_secs(2)))
    }
}

/// Waits for a window's content to change in the compositor (`copy_with_damage` without reading the
/// pixels), on a connection of its own.
pub struct Watch {
    c: Arc<Compositor>,
    client: client::Client,
    /// The window's owner and its content on the output, read again every 250 ms.
    at: Option<(u32, [i32; 4], Instant)>,
}

impl Watch {
    /// When the largest window of `pids` next changed (host clock), within `wait`; its owner too.
    pub fn next(&mut self, pids: &[u32], wait: Duration) -> Option<(u32, u64)> {
        let stale = self.at.as_ref().is_none_or(|a| a.2.elapsed() > Duration::from_millis(250));
        if stale {
            let w = main_window(pids).filter(|w| of(w.hwnd).is_some_and(|(c, _)| Arc::ptr_eq(&c, &self.c)));
            let out = self.c.output();
            self.at = w.zip(out).map(|(w, o)| {
                let (x, y) = client_origin(w.hwnd).unwrap_or((w.rect.0, w.rect.1));
                let (x0, y0) = (x.max(o[0]), y.max(o[1]));
                let (x1, y1) = ((x + w.client.0).min(o[0] + o[2]), (y + w.client.1).min(o[1] + o[3]));
                (w.pid, [x0 - o[0], y0 - o[1], (x1 - x0).max(1), (y1 - y0).max(1)], Instant::now())
            });
        }
        let Some((pid, r, _)) = self.at else {
            std::thread::sleep(wait.min(Duration::from_millis(50)));
            return None;
        };
        match self.client.grab(r, true, false, Instant::now() + wait) {
            Ok(g) => g.map(|g| (pid, g.at_ns)),
            Err(_) => {
                std::thread::sleep(wait.min(Duration::from_millis(50)));
                None
            }
        }
    }
}

/// The share of `w` that other processes' windows above it cover (a 16×16 grid).
fn covered(w: &Win, all: &[Win]) -> f64 {
    let above: Vec<&Win> = all.iter().skip_while(|o| o.id != w.id).skip(1).filter(|o| o.visible && o.pid != w.pid).collect();
    if above.is_empty() {
        return 0.0;
    }
    let [rx, ry, rw, rh] = w.rect;
    let mut hit = 0;
    for i in 0..16 {
        for j in 0..16 {
            let (px, py) = (rx + rw * (2 * i + 1) / 32, ry + rh * (2 * j + 1) / 32);
            if above.iter().any(|o| px >= o.rect[0] && px < o.rect[0] + o.rect[2] && py >= o.rect[1] && py < o.rect[1] + o.rect[3]) {
                hit += 1;
            }
        }
    }
    hit as f64 / 256.0
}

/// The largest showing window of `pids` (at least 100×100) in any compositor strobe runs.
pub fn main_window(pids: &[u32]) -> Option<TopWindow> {
    live().into_iter().find_map(|c| {
        let all = c.windows();
        let best = all.iter().filter(|w| pids.contains(&w.pid) && w.visible && w.rect[2] >= 100 && w.rect[3] >= 100).max_by_key(|w| w.rect[2] as i64 * w.rect[3] as i64)?;
        Some(c.top(best, &all))
    })
}

pub fn window_state(hwnd: isize) -> Option<TopWindow> {
    let (c, id) = of(hwnd)?;
    let all = c.windows();
    let w = all.iter().find(|w| w.id == id)?;
    Some(c.top(w, &all))
}

/// The showing windows of `pids`, front to back: id, title, click-through (never).
pub fn top_windows(pids: &[u32]) -> Vec<(isize, String, bool)> {
    let mut v: Vec<(isize, String, bool)> =
        live().iter().flat_map(|c| c.windows().into_iter().filter(|w| pids.contains(&w.pid) && w.visible).map(|w| (c.id(&w), w.title.clone(), false)).collect::<Vec<_>>()).collect();
    v.reverse();
    v
}

/// Where a window's client area starts on the layout.
pub fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    let (c, id) = of(hwnd)?;
    c.windows().into_iter().find(|w| w.id == id).map(|w| (w.content[0], w.content[1]))
}

/// Moves a window to `(x, y)` and with `client` sizes its content; floating, so it keeps the place.
pub fn place_window(w: &TopWindow, x: i32, y: i32, client: Option<(u32, u32)>) {
    let Some((c, id)) = of(w.hwnd) else { return };
    let size = client.map(|(cw, ch)| format!(", resize set width {cw} px height {ch} px")).unwrap_or_default();
    let _ = c.command(&format!("[con_id={id}] floating enable{size}, move position {x} px {y} px"));
}

/// The app's windows above the others: in strobe's compositor nothing else has a window, so this
/// only counts them.
pub fn raise_windows(pids: &[u32]) -> usize {
    live().iter().map(|c| c.windows().iter().filter(|w| pids.contains(&w.pid) && w.visible).count()).sum()
}

pub fn window_placement(hwnd: isize) -> Option<Placement> {
    let (c, _) = of(hwnd)?;
    let w = window_state(hwnd)?;
    let work = c.output()?.map(f64::from);
    let frame = [w.rect.0 as f64, w.rect.1 as f64, w.rect.2 as f64, w.rect.3 as f64];
    let maximized = frame.iter().zip(work).all(|(a, b)| (a - b).abs() <= 1.0);
    Some(Placement { frame, client: (w.client.0 as f64, w.client.1 as f64), work, per_dip: 1.0, maximized, unseen: [0.0; 4] })
}

/// The window's container at `frame`, its content filling it (strobe's compositor draws no borders).
pub fn set_window_frame(hwnd: isize, frame: [f64; 4]) -> Result<()> {
    let (c, id) = of(hwnd).ok_or_else(|| Error::msg("page.window(): the window closed"))?;
    let [x, y, w, h] = frame.map(|v| v.round() as i64);
    c.command(&format!("[con_id={id}] fullscreen disable, floating enable, resize set width {} px height {} px, move position {x} px {y} px", w.max(1), h.max(1)))
}

/// A frame of the whole output: sway keeps no maximized state for a floating window.
pub fn maximize_window(hwnd: isize, work: [f64; 4]) -> Result<()> {
    set_window_frame(hwnd, work)
}

/// A window's client area as the compositor composites it now.
pub fn window_image(hwnd: isize) -> Option<crate::png::Image> {
    let w = window_state(hwnd)?;
    region_image(hwnd, [0, 0, w.client.0, w.client.1])
}

/// A region of a window's client area (`rect` in its pixels, x, y, width, height) as BGRA, or with
/// `damage` once the compositor next changes it, within `wait`.
pub fn window_region(hwnd: isize, rect: [i32; 4], damage: bool, wait: Duration) -> Option<Grab> {
    let (c, id) = of(hwnd)?;
    let w = c.windows().into_iter().find(|w| w.id == id)?;
    c.grab([w.content[0] + rect[0], w.content[1] + rect[1], rect[2], rect[3]], damage, wait).ok()?
}

/// `window_region` as it shows now, as an RGBA image (a pixel anchor's or a pixel wait's read).
pub fn region_image(hwnd: isize, rect: [i32; 4]) -> Option<crate::png::Image> {
    let g = window_region(hwnd, rect, false, Duration::from_secs(2))?;
    Some(crate::png::Image { width: g.width, height: g.height, rgba: g.bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect() })
}

/// `region_image` of a window that shows and that no other window covers in the region (a pixel
/// anchor's read, as on X11): else why not.
pub fn shown_region(hwnd: isize, rect: [i32; 4]) -> std::result::Result<crate::png::Image, String> {
    let (c, id) = of(hwnd).ok_or("the window closed")?;
    let all = c.windows();
    let w = all.iter().find(|w| w.id == id).ok_or("the window closed")?;
    if !w.visible || w.hidden {
        return Err("the window is hidden or minimized, so nothing of it is on the screen".into());
    }
    let r = [w.content[0] + rect[0], w.content[1] + rect[1], rect[2], rect[3]];
    let overlaps = |o: &Win| o.rect[0] < r[0] + r[2] && r[0] < o.rect[0] + o.rect[2] && o.rect[1] < r[1] + r[3] && r[1] < o.rect[1] + o.rect[3];
    if all.iter().skip_while(|o| o.id != id).skip(1).any(|o| o.visible && overlaps(o)) {
        return Err("another window covers the region".into());
    }
    region_image(hwnd, rect).ok_or_else(|| "the compositor gave no copy of the region".into())
}

/// The largest window of `pids` as 16×16 averaged BGRA cells, from one copy of its client area.
pub fn window_pixels(pids: &[u32]) -> Option<Vec<u8>> {
    let c = showing(pids)?;
    let w = main_window(pids)?;
    let (_, id) = of(w.hwnd)?;
    let win = c.windows().into_iter().find(|x| x.id == id)?;
    if win.content[2] < 16 || win.content[3] < 16 {
        return None;
    }
    let g = c.grab(win.content, false, Duration::from_secs(2)).ok()??;
    Some(super::grid16(&g.bgra, g.width, g.height, g.width * 4))
}

/// Gives a window the seat's focus (keys go to it); false when it did not take it within 500 ms.
pub fn focus(hwnd: isize) -> Result<bool> {
    let (c, id) = of(hwnd).ok_or_else(|| Error::msg("the app's window closed"))?;
    let focused = || c.windows().iter().any(|w| w.id == id && w.focused);
    if focused() {
        return Ok(true);
    }
    c.command(&format!("[con_id={id}] focus"))?;
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        if focused() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(false)
}

/// `show`, `hide`, `minimize` or `close` through the compositor: hidden and minimized windows go to
/// the scratchpad (sway has no minimized state), close asks the app (`xdg_toplevel.close`).
pub fn window_action(hwnd: isize, action: &str) -> Result<()> {
    let (c, id) = of(hwnd).ok_or_else(|| Error::msg("the app's window closed"))?;
    let cmd = match action {
        "close" => "kill",
        "hide" | "minimize" => "move scratchpad",
        "show" => "scratchpad show, floating enable",
        other => return Err(Error::usage(format!("window({other}): show, hide, minimize, focus or close"))),
    };
    c.command(&format!("[con_id={id}] {cmd}"))
}

/// The compositor of a window strobe runs, for its input.
pub fn compositor_of(hwnd: isize) -> Option<Arc<Compositor>> {
    of(hwnd).map(|(c, _)| c)
}
