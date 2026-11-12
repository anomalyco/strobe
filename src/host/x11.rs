//! Top-level windows on Linux through X11 (`x11rb`'s pure-Rust connection to `$DISPLAY`), with the
//! window manager's EWMH properties: whose a window is (`_NET_WM_PID`), whether it is minimized
//! (`_NET_WM_STATE_HIDDEN`), the stacking order for occlusion, and placement that keeps a window on top
//! without focusing it (`_NET_WM_STATE_ABOVE`). Native Wayland windows are invisible here: Wayland
//! gives no client a view of another's windows, so strobe sees them only in a compositor it runs
//! itself (`wayland`).

use super::TopWindow;
use std::sync::Mutex;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, ImageFormat, MapState, StackMode, Window};
use x11rb::rust_connection::RustConnection;

struct X {
    conn: RustConnection,
    root: Window,
    dpi: u32,
}

static CONN: Mutex<Option<X>> = Mutex::new(None);

/// Runs `f` on the display's connection (opened once; reopened after an error).
fn with<T>(f: impl FnOnce(&X) -> Option<T>) -> Option<T> {
    std::env::var_os("DISPLAY")?;
    let mut guard = CONN.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let mut x = X { conn, root, dpi: 96 };
        x.dpi = resource_dpi(&x).unwrap_or(96);
        *guard = Some(x);
    }
    let out = f(guard.as_ref()?);
    if out.is_none() && guard.as_ref().is_some_and(|x| x.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).is_none()) {
        *guard = None;
    }
    out
}

fn atom(x: &X, name: &str) -> Option<u32> {
    Some(x.conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom)
}

fn prop32(x: &X, win: Window, name: &str) -> Vec<u32> {
    let Some(a) = atom(x, name) else { return Vec::new() };
    x.conn.get_property(false, win, a, AtomEnum::ANY, 0, 4096).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default()
}

/// `Xft.dpi` from the root window's resources (what toolkits scale by), else 96.
fn resource_dpi(x: &X) -> Option<u32> {
    let r = x.conn.get_property(false, x.root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 65536).ok()?.reply().ok()?;
    let text = String::from_utf8_lossy(&r.value).to_string();
    text.lines().find_map(|l| l.strip_prefix("Xft.dpi:")).and_then(|v| v.trim().parse::<f64>().ok()).map(|d| d.round() as u32)
}

/// Managed top-level windows, bottom to top: the window manager's stacking list, else the root's
/// children, each frame stood for by the client window in it (ICCCM's `WM_STATE`), since a manager
/// without the list (an XWayland manager) still reparents clients into frames.
fn clients(x: &X) -> Vec<Window> {
    let stacked = prop32(x, x.root, "_NET_CLIENT_LIST_STACKING");
    if !stacked.is_empty() {
        return stacked;
    }
    children(x, x.root).into_iter().map(|w| client_in(x, w, 2).unwrap_or(w)).collect()
}

fn children(x: &X, win: Window) -> Vec<Window> {
    x.conn.query_tree(win).ok().and_then(|c| c.reply().ok()).map(|t| t.children).unwrap_or_default()
}

/// The window at or under `win` (at most `depth` levels down) that has `WM_STATE`.
fn client_in(x: &X, win: Window, depth: u32) -> Option<Window> {
    if !prop32(x, win, "WM_STATE").is_empty() {
        return Some(win);
    }
    if depth == 0 {
        return None;
    }
    children(x, win).into_iter().rev().find_map(|c| client_in(x, c, depth - 1))
}

struct Info {
    win: Window,
    pid: u32,
    /// Outer rect on the root (frame included when the manager reports it).
    rect: (i32, i32, i32, i32),
    client: (i32, i32),
    viewable: bool,
    hidden: bool,
}

fn info(x: &X, win: Window) -> Option<Info> {
    let g = x.conn.get_geometry(win).ok()?.reply().ok()?;
    let at = x.conn.translate_coordinates(win, x.root, 0, 0).ok()?.reply().ok()?;
    let attrs = x.conn.get_window_attributes(win).ok()?.reply().ok()?;
    let frame = prop32(x, win, "_NET_FRAME_EXTENTS");
    let (l, r, t, b) = if frame.len() == 4 { (frame[0] as i32, frame[1] as i32, frame[2] as i32, frame[3] as i32) } else { (0, 0, 0, 0) };
    let hidden_atom = atom(x, "_NET_WM_STATE_HIDDEN");
    let hidden = hidden_atom.is_some_and(|h| prop32(x, win, "_NET_WM_STATE").contains(&h)) || prop32(x, win, "WM_STATE").first() == Some(&3);
    Some(Info {
        win,
        pid: prop32(x, win, "_NET_WM_PID").first().copied().unwrap_or(0),
        rect: (at.dst_x as i32 - l, at.dst_y as i32 - t, g.width as i32 + l + r, g.height as i32 + t + b),
        client: (g.width as i32, g.height as i32),
        viewable: attrs.map_state == MapState::VIEWABLE,
        hidden,
    })
}

/// The share of `w` that other processes' viewable windows above it cover (a 16×16 grid).
fn covered(x: &X, w: &Info, stack: &[Window]) -> f64 {
    let above: Vec<Info> = stack.iter().skip_while(|&&s| s != w.win).skip(1).filter_map(|&s| info(x, s)).filter(|o| o.viewable && o.pid != w.pid).collect();
    if above.is_empty() {
        return 0.0;
    }
    let (rx, ry, rw, rh) = w.rect;
    let mut hit = 0;
    for i in 0..16 {
        for j in 0..16 {
            let (px, py) = (rx + rw * (2 * i + 1) / 32, ry + rh * (2 * j + 1) / 32);
            if above.iter().any(|o| px >= o.rect.0 && px < o.rect.0 + o.rect.2 && py >= o.rect.1 && py < o.rect.1 + o.rect.3) {
                hit += 1;
            }
        }
    }
    hit as f64 / 256.0
}

fn top(x: &X, w: &Info, stack: &[Window]) -> TopWindow {
    TopWindow { hwnd: w.win as isize, pid: w.pid, rect: w.rect, client: w.client, visible: w.viewable, iconic: w.hidden, cloaked: w.viewable && covered(x, w, stack) >= 0.99, dpi: x.dpi }
}

/// The largest viewable window of `pids` (at least 100×100 pixels: a strip such as a menu bar is no
/// main window, whatever its area).
pub fn main_window(pids: &[u32]) -> Option<TopWindow> {
    with(|x| {
        let stack = clients(x);
        let best = stack
            .iter()
            .filter_map(|&w| info(x, w))
            .filter(|i| pids.contains(&i.pid) && i.viewable && !i.hidden && i.rect.2 >= 100 && i.rect.3 >= 100)
            .max_by_key(|i| i.rect.2 as i64 * i.rect.3 as i64)?;
        Some(top(x, &best, &stack))
    })
}

/// The monitor holding the point `at` (device-independent pixels at the display's DPI; RandR's
/// monitors, else the whole root) and the part of it the window manager's work area on the current
/// desktop covers (`_NET_WORKAREA`, one rectangle over every monitor, so cut to this one's).
pub fn work_area(at: (f64, f64)) -> Option<super::WorkArea> {
    use x11rb::protocol::randr::ConnectionExt as _;
    with(|x| {
        let s = x.dpi as f64 / 96.0;
        let (px, py) = (at.0 * s, at.1 * s);
        let g = x.conn.get_geometry(x.root).ok()?.reply().ok()?;
        let root = [0.0, 0.0, g.width as f64, g.height as f64];
        let monitors: Vec<[f64; 4]> = x
            .conn
            .randr_get_monitors(x.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.monitors.iter().map(|m| [m.x as f64, m.y as f64, m.width as f64, m.height as f64]).collect())
            .unwrap_or_default();
        let inside = |m: &[f64; 4]| px >= m[0] && px < m[0] + m[2] && py >= m[1] && py < m[1] + m[3];
        let screen = monitors.iter().find(|m| inside(m)).or(monitors.first()).copied().unwrap_or(root);
        let areas = prop32(x, x.root, "_NET_WORKAREA");
        let desktop = prop32(x, x.root, "_NET_CURRENT_DESKTOP").first().copied().unwrap_or(0) as usize;
        let area = areas.get(desktop * 4..desktop * 4 + 4).or_else(|| areas.get(..4)).map_or(root, |a| [a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64]);
        let (l, t) = (screen[0].max(area[0]), screen[1].max(area[1]));
        let (r, b) = ((screen[0] + screen[2]).min(area[0] + area[2]), (screen[1] + screen[3]).min(area[1] + area[3]));
        let work = if r > l && b > t { [l, t, r - l, b - t] } else { screen };
        Some(super::WorkArea { screen: screen.map(|v| v / s), work: work.map(|v| v / s) })
    })
}

pub fn window_state(id: isize) -> Option<TopWindow> {
    with(|x| {
        let stack = clients(x);
        let i = info(x, id as Window)?;
        Some(top(x, &i, &stack))
    })
}

/// Asks the window manager to keep `win` above other windows without focusing it (EWMH), or with no
/// manager, raises it.
fn keep_above(x: &X, win: Window) -> Option<()> {
    let (state, above) = (atom(x, "_NET_WM_STATE")?, atom(x, "_NET_WM_STATE_ABOVE")?);
    if prop32(x, x.root, "_NET_SUPPORTED").contains(&above) {
        let ev = ClientMessageEvent::new(32, win, state, [1, above, 0, 1, 0]);
        x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev).ok()?;
    } else {
        x.conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).ok()?;
    }
    Some(())
}

/// Moves the window to `(x, y)` and with `client` sizes its client area (device-independent pixels at
/// the display's DPI), kept above without focus. Through `_NET_MOVERESIZE_WINDOW` when the manager has
/// it (a managed window ignores direct moves), else directly.
pub fn place_window(w: &TopWindow, px: i32, py: i32, client: Option<(u32, u32)>) {
    with(|x| {
        let win = w.hwnd as Window;
        let scale = |v: u32| (v as f64 * w.dpi as f64 / 96.0).round() as u32;
        let size = client.map(|(cw, ch)| (scale(cw), scale(ch)));
        let moveresize = atom(x, "_NET_MOVERESIZE_WINDOW")?;
        if prop32(x, x.root, "_NET_SUPPORTED").contains(&moveresize) {
            // Gravity static (10): x, y place the client's frame; flags: x, y and, with a size, width and height.
            let flags = 10 | (1 << 8) | (1 << 9) | if size.is_some() { (1 << 10) | (1 << 11) } else { 0 } | (2 << 12);
            let (cw, ch) = size.unwrap_or((0, 0));
            let ev = ClientMessageEvent::new(32, win, moveresize, [flags, px as u32, py as u32, cw, ch]);
            x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev).ok()?;
        } else {
            let mut aux = ConfigureWindowAux::new().x(px).y(py);
            if let Some((cw, ch)) = size {
                aux = aux.width(cw).height(ch);
            }
            x.conn.configure_window(win, &aux).ok()?;
        }
        keep_above(x, win);
        x.conn.flush().ok()
    });
}

/// Whether the window manager keeps `win` maximized both ways (`_NET_WM_STATE`).
fn maximized(x: &X, win: Window) -> bool {
    let state = prop32(x, win, "_NET_WM_STATE");
    ["_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"].iter().all(|n| atom(x, n).is_some_and(|a| state.contains(&a)))
}

/// Whether the window manager keeps a maximized state (`_NET_SUPPORTED`).
fn supports_maximized(x: &X) -> bool {
    let supported = prop32(x, x.root, "_NET_SUPPORTED");
    ["_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"].iter().all(|n| atom(x, n).is_some_and(|a| supported.contains(&a)))
}

/// Adds (1) or removes (0) the maximized state through the window manager; false when it has none.
fn set_maximized(x: &X, win: Window, on: bool) -> Option<bool> {
    let (state, v, h) = (atom(x, "_NET_WM_STATE")?, atom(x, "_NET_WM_STATE_MAXIMIZED_VERT")?, atom(x, "_NET_WM_STATE_MAXIMIZED_HORZ")?);
    if !supports_maximized(x) {
        return Some(false);
    }
    let ev = ClientMessageEvent::new(32, win, state, [u32::from(on), v, h, 1, 0]);
    x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev).ok()?;
    x.conn.flush().ok()?;
    Some(true)
}

pub fn window_placement(id: isize) -> Option<super::Placement> {
    let w = window_state(id)?;
    let s = w.scale();
    let frame = [w.rect.0 as f64, w.rect.1 as f64, w.rect.2 as f64, w.rect.3 as f64];
    let work = work_area(((frame[0] + frame[2] / 2.0) / s, (frame[1] + frame[3] / 2.0) / s))?.work.map(|v| v * s);
    // Without a manager's maximized state (none, or one without it), a frame that fills the work area.
    let fills = frame.iter().zip(work).all(|(a, b)| (a - b).abs() <= 1.0);
    let max = with(|x| Some(if supports_maximized(x) { maximized(x, id as Window) } else { fills })).unwrap_or(fills);
    Some(super::Placement { frame, client: (w.client.0 as f64, w.client.1 as f64), work, per_dip: s, maximized: max, unseen: [0.0; 4] })
}

/// The frame as the client window's place and size (the frame less `_NET_FRAME_EXTENTS`), through
/// `_NET_MOVERESIZE_WINDOW` (static gravity: the client's own place) when the manager has it, else
/// directly; out of the maximized state first.
pub fn set_window_frame(id: isize, frame: [f64; 4]) -> crate::model::Result<()> {
    let ok = with(|x| {
        let win = id as Window;
        if maximized(x, win) {
            set_maximized(x, win, false)?;
            // The manager restores its own saved frame first; the new one goes after it.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while maximized(x, win) && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let i = info(x, win)?;
        let at = x.conn.translate_coordinates(win, x.root, 0, 0).ok()?.reply().ok()?;
        let (l, t) = (at.dst_x as i32 - i.rect.0, at.dst_y as i32 - i.rect.1);
        let (r, b) = (i.rect.2 - i.client.0 - l, i.rect.3 - i.client.1 - t);
        let [fx, fy, fw, fh] = frame.map(|v| v.round() as i32);
        let (cx, cy, cw, ch) = (fx + l, fy + t, (fw - l - r).max(1) as u32, (fh - t - b).max(1) as u32);
        let moveresize = atom(x, "_NET_MOVERESIZE_WINDOW")?;
        if prop32(x, x.root, "_NET_SUPPORTED").contains(&moveresize) {
            let flags = 10 | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (2 << 12);
            let ev = ClientMessageEvent::new(32, win, moveresize, [flags, cx as u32, cy as u32, cw, ch]);
            x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev).ok()?;
        } else {
            x.conn.configure_window(win, &ConfigureWindowAux::new().x(cx).y(cy).width(cw).height(ch)).ok()?;
        }
        x.conn.flush().ok()
    });
    ok.ok_or_else(|| crate::model::Error::msg("page.window(): the X server did not take the window's new frame (the window closed, or no display)"))
}

/// The window manager's maximized state, else (a manager without it) a frame of the work area.
pub fn maximize_window(id: isize, work: [f64; 4]) -> crate::model::Result<()> {
    match with(|x| set_maximized(x, id as Window, true)) {
        Some(true) => Ok(()),
        Some(false) => set_window_frame(id, work),
        None => Err(crate::model::Error::msg("page.window(\"maximize\"): the X server took no message (the window closed, or no display)")),
    }
}

/// Keeps every viewable window of `pids` above others without focusing it; returns how many.
pub fn raise_windows(pids: &[u32]) -> usize {
    with(|x| {
        let wins: Vec<Window> = clients(x).into_iter().filter_map(|w| info(x, w)).filter(|i| pids.contains(&i.pid) && i.viewable).map(|i| i.win).collect();
        for &w in &wins {
            keep_above(x, w);
        }
        x.conn.flush().ok()?;
        Some(wins.len())
    })
    .unwrap_or(0)
}

/// A window's title: `_NET_WM_NAME` (UTF-8), else `WM_NAME`.
fn title(x: &X, win: Window) -> String {
    let utf8 = atom(x, "UTF8_STRING").unwrap_or(0);
    for (name, kind) in [("_NET_WM_NAME", utf8), ("WM_NAME", AtomEnum::STRING.into())] {
        let Some(a) = atom(x, name) else { continue };
        if let Some(r) = x.conn.get_property(false, win, a, kind, 0, 1024).ok().and_then(|c| c.reply().ok()).filter(|r| !r.value.is_empty()) {
            return String::from_utf8_lossy(&r.value).to_string();
        }
    }
    String::new()
}

/// The viewable windows of `pids`, front to back: id, title, click-through (never, on X11).
pub fn top_windows(pids: &[u32]) -> Vec<(isize, String, bool)> {
    with(|x| {
        let mut v: Vec<(isize, String, bool)> =
            clients(x).into_iter().filter_map(|w| info(x, w)).filter(|i| pids.contains(&i.pid) && i.viewable && !i.hidden).map(|i| (i.win as isize, title(x, i.win), false)).collect();
        v.reverse();
        Some(v)
    })
    .unwrap_or_default()
}

/// Where a window's client area starts on the root window.
pub fn client_origin(win: isize) -> Option<(i32, i32)> {
    with(|x| {
        let at = x.conn.translate_coordinates(win as Window, x.root, 0, 0).ok()?.reply().ok()?;
        Some((at.dst_x as i32, at.dst_y as i32))
    })
}

/// The topmost viewable managed window at a root point: (window, pid, title).
pub fn window_at(px: i32, py: i32) -> Option<(isize, u32, String)> {
    with(|x| {
        let stack = clients(x);
        let hit = stack.iter().rev().filter_map(|&w| info(x, w)).find(|i| i.viewable && px >= i.rect.0 && py >= i.rect.1 && px < i.rect.0 + i.rect.2 && py < i.rect.1 + i.rect.3)?;
        Some((hit.win as isize, hit.pid, title(x, hit.win)))
    })
}

/// A window's client area as it is drawn now (`GetImage`, 32-bit pixels), for `snapshot`.
pub fn window_image(win: isize) -> Option<crate::png::Image> {
    with(|x| {
        let g = x.conn.get_geometry(win as Window).ok()?.reply().ok()?;
        let r = x.conn.get_image(ImageFormat::Z_PIXMAP, win as Window, 0, 0, g.width, g.height, !0).ok()?.reply().ok()?;
        let (w, h) = (g.width as usize, g.height as usize);
        if r.data.len() < w * h * 4 {
            return None;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for px in r.data[..w * h * 4].chunks(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Some(crate::png::Image { width: w, height: h, rgba })
    })
}

/// A part of a window's client area as it is drawn now (`GetImage` of `[x, y, w, h]`, pixels), once the
/// window is viewable and on top at the part's centre: the X server copies whatever covers a window's
/// area, so a covered part would be another window's pixels. Err says why there are none.
pub fn window_region(win: isize, [x, y, w, h]: [i32; 4]) -> Result<crate::png::Image, String> {
    let at = client_origin(win).ok_or("the window is gone or there is no display")?;
    let (cx, cy) = (at.0 + x + w / 2, at.1 + y + h / 2);
    if window_at(cx, cy).is_none_or(|t| t.0 != win) {
        let over = window_at(cx, cy).map(|t| format!("{:?} (pid {})", t.2, t.1)).unwrap_or_else(|| "nothing of the app's".into());
        return Err(format!("the window is not on top at ({cx}, {cy}): {over} is, and the X server copies what covers a window"));
    }
    with(|xc| {
        let attrs = xc.conn.get_window_attributes(win as Window).ok()?.reply().ok()?;
        if attrs.map_state != MapState::VIEWABLE {
            return None;
        }
        let r = xc.conn.get_image(ImageFormat::Z_PIXMAP, win as Window, x as i16, y as i16, w as u16, h as u16, !0).ok()?.reply().ok()?;
        let (w, h) = (w as usize, h as usize);
        if r.data.len() < w * h * 4 {
            return None;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for px in r.data[..w * h * 4].chunks(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Some(crate::png::Image { width: w, height: h, rgba })
    })
    .ok_or_else(|| "the window is not viewable (unmapped or minimized), or the X server gave no image of it".to_string())
}

/// A launch announced for X11 startup notification (the freedesktop Startup Notification protocol, the
/// launch feedback of GNOME's spinner and KDE's bouncing cursor): its id goes into the app's environment
/// (`env`), and `feedback` is when the app said `remove:` for it, which a toolkit says once it maps its
/// first window (GTK, Qt).
pub struct StartupNotice {
    pub id: String,
    pub feedback: super::Feedback,
}

impl StartupNotice {
    /// The variable the app reads its id from.
    pub fn env(&self) -> (String, String) {
        ("DESKTOP_STARTUP_ID".into(), self.id.clone())
    }
}

/// Announces a launch on `$DISPLAY` before it starts: a new id, sent as `new:` with `SILENT=1`, so a
/// desktop that shows launch feedback shows none for it (rule 10: nothing changes on the person's
/// screen), and a watch on a connection of its own, already listening when this returns, for the app's
/// `remove:` of that id. The time is when strobe read the message, on `now_ns`'s clock. At `deadline`,
/// or when the watch is dropped, strobe says `remove:` itself, as a launcher whose app never did must.
/// `NoDisplay` without a display to connect to.
pub fn announce_startup(deadline: std::time::Instant) -> Result<StartupNotice, super::NoFeedback> {
    use super::NoFeedback;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, CreateWindowAux, WindowClass};
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
        return Err(NoFeedback::NoDisplay);
    }
    let (conn, screen) = x11rb::connect(None).map_err(|_| NoFeedback::NoDisplay)?;
    fn unreadable<E>(_: E) -> NoFeedback {
        NoFeedback::Unreadable
    }
    let root = conn.setup().roots.get(screen).ok_or(NoFeedback::Unreadable)?.root;
    let intern = |name: &str| conn.intern_atom(false, name.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom).ok_or(NoFeedback::Unreadable);
    let atoms = Startup { begin: intern("_NET_STARTUP_INFO_BEGIN")?, more: intern("_NET_STARTUP_INFO")?, root, from: conn.generate_id().map_err(unreadable)? };
    // The window the messages come from (the spec groups a message's pieces by it): never mapped.
    conn.create_window(0, atoms.from, root, -100, -100, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1)).map_err(unreadable)?;
    // Messages go to the root window to whoever selected PropertyChangeMask on it: this connection's
    // own selection, which no other client's changes.
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).map_err(unreadable)?;
    let id = format!("strobe-{}-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed), super::now_ns());
    // Its round trip also puts the selection in place before the app can start.
    atoms.say(&conn, &format!("new: ID={} NAME={} SCREEN={screen} SILENT=1", quote(&id), quote("launch")))?;
    let want = id.clone();
    let feedback = super::Feedback::start(move |stop| atoms.wait_remove(&conn, &want, deadline, stop));
    Ok(StartupNotice { id, feedback })
}

/// One announcing connection's atoms and windows.
struct Startup {
    begin: u32,
    more: u32,
    root: Window,
    from: Window,
}

impl Startup {
    /// Sends one message: its bytes and a NUL in 20-byte pieces, the first as `_NET_STARTUP_INFO_BEGIN`,
    /// then waits for a round trip: an X server drops the requests of a connection that closes before
    /// it read them (Xvfb dropped the `remove:` a watch said as it ended).
    fn say(&self, conn: &RustConnection, text: &str) -> Result<(), super::NoFeedback> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        for (i, piece) in bytes.chunks(20).enumerate() {
            let mut data = [0u8; 20];
            data[..piece.len()].copy_from_slice(piece);
            let ev = ClientMessageEvent::new(8, self.from, if i == 0 { self.begin } else { self.more }, data);
            conn.send_event(false, self.root, EventMask::PROPERTY_CHANGE, ev).map_err(|_| super::NoFeedback::Unreadable)?;
        }
        conn.get_input_focus().map_err(|_| super::NoFeedback::Unreadable)?.reply().map(drop).map_err(|_| super::NoFeedback::Unreadable)
    }

    /// Reads messages until a `remove:` of `id`, `deadline`, or `stop`; strobe's own `remove:` ends a
    /// launch whose app gave none.
    fn wait_remove(&self, conn: &RustConnection, id: &str, deadline: std::time::Instant, stop: &std::sync::atomic::AtomicBool) -> Result<u64, super::NoFeedback> {
        use std::os::fd::AsRawFd;
        use x11rb::protocol::Event;
        let fd = conn.stream().as_raw_fd();
        let mut pieces: std::collections::HashMap<Window, Vec<u8>> = std::collections::HashMap::new();
        loop {
            while let Ok(Some(ev)) = conn.poll_for_event() {
                let at = super::now_ns();
                let Event::ClientMessage(m) = ev else { continue };
                // A message's first piece starts it again; a later one goes on with its sender's.
                if m.type_ == self.begin {
                    pieces.insert(m.window, Vec::new());
                } else if m.type_ != self.more || !pieces.contains_key(&m.window) {
                    continue;
                }
                let buf = pieces.entry(m.window).or_default();
                buf.extend_from_slice(&m.data.as_data8());
                let Some(end) = buf.iter().position(|b| *b == 0) else { continue };
                let message = String::from_utf8_lossy(&buf[..end]).to_string();
                pieces.remove(&m.window);
                if startup_message(&message).is_some_and(|(kind, keys)| kind == "remove" && keys.iter().any(|(k, v)| k == "ID" && v == id)) {
                    return Ok(at);
                }
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = self.say(conn, &format!("remove: ID={}", quote(id)));
                return Err(super::NoFeedback::Deadline);
            }
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let ms = left.as_millis().clamp(1, 50) as i32;
            if unsafe { libc::poll(&mut p, 1, ms) } < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return Err(super::NoFeedback::Unreadable);
            }
            if p.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                return Err(super::NoFeedback::Unreadable);
            }
        }
    }
}

/// A value quoted as the startup-notification spec quotes one.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A startup-notification message as its kind (`new`, `change`, `remove`) and its `KEY=value` pairs
/// (values quoted or bare, backslash escapes); None when it has no `kind:`.
pub fn startup_message(text: &str) -> Option<(String, Vec<(String, String)>)> {
    let (kind, rest) = text.split_once(':')?;
    let mut keys = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ' ').is_some() {}
        let key: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=' && *c != ' ')).collect();
        if key.is_empty() || chars.next() != Some('=') {
            break;
        }
        let mut value = String::new();
        let mut quoted = false;
        while let Some(c) = chars.next() {
            match c {
                '"' => quoted = !quoted,
                '\\' => value.extend(chars.next()),
                ' ' if !quoted => break,
                c => value.push(c),
            }
        }
        keys.push((key, value));
    }
    Some((kind.trim().to_string(), keys))
}

/// The largest window of `pids` as 16×16 averaged BGRA cells, from its own pixels (`GetImage` of the
/// one-pixel row through each cell row's middle, so a sample moves a few kilobytes, not the window).
pub fn window_pixels(pids: &[u32]) -> Option<Vec<u8>> {
    let w = main_window(pids)?;
    with(|x| {
        let (cw, ch) = (w.client.0.max(0) as u16, w.client.1.max(0) as u16);
        if cw < 16 || ch < 16 {
            return None;
        }
        let rows: Vec<u16> = (0..16).map(|i| ((2 * i + 1) as u32 * ch as u32 / 32) as u16).collect();
        let mut strips = Vec::with_capacity(rows.len());
        for &y in &rows {
            let r = x.conn.get_image(ImageFormat::Z_PIXMAP, w.hwnd as Window, 0, y as i16, cw, 1, !0).ok()?.reply().ok()?;
            if r.data.len() < cw as usize * 4 {
                return None;
            }
            strips.extend_from_slice(&r.data[..cw as usize * 4]);
        }
        Some(super::grid16(&strips, cw as usize, 16, cw as usize * 4))
    })
}
