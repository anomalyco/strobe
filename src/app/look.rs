//! `strobe look <url>`: a page in a headless browser, nothing measured: its title, address, headings,
//! visible text, console messages and uncaught errors, and a screenshot. For checking that a UI
//! loads and says what it should (an agent looking at a page while the session is locked) without
//! writing a browser script.

use super::{Output, fmt, progress};
use crate::cli::{Args, duration_ms};
use crate::model::{Error, Result};
use crate::target::a11y::{Dialect, Selector};
use crate::target::chromium::{self, Window};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A local file is a `file://` address; anything else goes as given.
pub(super) fn address(target: &str) -> String {
    let p = Path::new(target);
    if !target.contains("://") && !target.starts_with("about:") && !target.starts_with("data:") && p.exists() {
        let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = abs.display().to_string().replace('\\', "/");
        let s = s.trim_start_matches("//?/");
        // A path's own `%`, spaces, `?` and `#` are not a URL's: escaped, they stay part of the path.
        let s = &s.replace('%', "%25").replace(' ', "%20").replace('?', "%3F").replace('#', "%23");
        return if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") };
    }
    target.to_string()
}

/// `--until image=<png>[,threshold=<t>][,scale=<s>]`: an image locator, its path relative to where strobe
/// runs; anything else is a selector.
fn image_until(u: &str) -> Option<Value> {
    let rest = u.strip_prefix("image=")?;
    let mut parts = rest.split(',');
    let file = parts.next().unwrap_or("");
    let abs = std::fs::canonicalize(file).map(|p| p.display().to_string().trim_start_matches(r"\\?\").to_string()).unwrap_or_else(|_| file.to_string());
    let mut v = json!({ "image": abs, "as": file });
    for p in parts {
        if let Some((k, x)) = p.split_once('=')
            && let Ok(n) = x.trim().parse::<f64>()
        {
            v[k.trim()] = json!(n);
        }
    }
    Some(v)
}

/// An image that never showed (`AnchorNotFound`): its message, and where the capture it was searched in
/// is kept (strobe's run folder), to hold the image against.
fn anchor_problem(e: Error) -> String {
    let kept = crate::driver::pixels::take_failed().and_then(|img| {
        let path = crate::host::paths::run().join(format!("look-anchor-{}.png", crate::host::now_ns()));
        std::fs::write(&path, crate::png::write(&img).ok()?).ok().map(|_| path)
    });
    match kept {
        Some(p) => format!("{} (capture searched: {})", e.message, p.display()),
        None => e.message,
    }
}

/// The matches of an image locator, as look lists them: `3 matches · best 0.982 at [x, y, w, h]`.
fn image_lines(found: &Value) -> (String, Vec<Value>) {
    let all: Vec<Value> = found.as_array().cloned().unwrap_or_default();
    let mut text = format!("image    {} match{}\n", all.len(), if all.len() == 1 { "" } else { "es" });
    for m in all.iter().take(10) {
        let r: Vec<String> = m["rect"].as_array().into_iter().flatten().map(|x| format!("{:.0}", x.as_f64().unwrap_or(0.0))).collect();
        text += &format!("  score {:.3} at [{}]\n", m["score"].as_f64().unwrap_or(0.0), r.join(", "));
    }
    (text, all)
}

fn console_text(args: &Value) -> String {
    args.as_array()
        .into_iter()
        .flatten()
        .map(|a| match a["value"].as_str() {
            Some(s) => s.to_string(),
            None if !a["value"].is_null() => a["value"].to_string(),
            None => a["description"].as_str().or(a["type"].as_str()).unwrap_or("").to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The app's executable is not there: says so, and names the newest build of it a bench made (a
/// directory or git-ref variant's build output, kept in strobe's `builds/out`), to look at instead.
fn missing(app: &crate::target::isolate::App, profile: Option<&str>) -> Error {
    let name = app.exe.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let newest = std::fs::read_dir(crate::host::paths::state_dir().join("builds").join("out"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|d| d.path().join(&name))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max();
    let e = Error::usage(format!("{} does not exist, so there is no {} to look at", app.exe.display(), app.id));
    let with = profile.map(|id| format!(" --app {id}")).unwrap_or_default();
    match newest {
        Some((_, p)) => e.hint(format!("the newest build of it a bench made is {}: strobe look exe:{}{with}", p.display(), p.display())),
        None => e.hint("install the app, or build it (a bench of a directory or git ref builds it with strobe.json's build), then look at exe:<path> --app <id>"),
    }
}

/// `strobe look app:<id> | exe:<path>`: the native app launched in an isolated home as a bench
/// launches it, nothing measured, then closed: its windows' titles and what its accessibility tree
/// shows (the window names, open dialogs and headings a case's `expect` checks, and the keyed
/// elements selectors name). The tree reads on a locked session too, so an `expect` can be checked
/// before the next unlock.
fn native(args: &Args, target: &str) -> Result<Output> {
    use crate::target::{isolate, native as nt};
    let config = crate::config::load(&crate::config::project_root(&std::env::current_dir()?))?;
    let mut app = match (target.strip_prefix("app:"), target.strip_prefix("exe:"), args.get("app")) {
        (Some(id), _, _) => isolate::app(id, &config)?,
        (_, Some(path), Some(id)) => isolate::app(id, &config)?.with_exe(Path::new(path)),
        (_, Some(path), None) => isolate::App::bare(Path::new(path)),
        _ => return Err(Error::usage("look app:<id> or exe:<path>")),
    }
    .as_kind(isolate::AppKind::Native)
    .with_display(args.has("wayland").then_some(isolate::Display::Wayland));
    for page_only in ["element", "full", "size", "scale", "chars", "headed", "out"] {
        if args.has(page_only) {
            return Err(Error::usage(format!("--{page_only} is for a page; a native app's look reads its accessibility tree and takes no screenshot")));
        }
    }
    // `--build`: this checkout's own build of the app (strobe.json `build`), as a bench of it runs.
    if args.has("build") {
        if target.starts_with("exe:") {
            return Err(Error::usage("--build builds the app of an app:<id> profile from this checkout; exe:<path> names the executable already"));
        }
        app = app.with_exe(&super::bench::build_native(&std::env::current_dir()?)?);
    }
    if !app.exe.is_file() {
        return Err(missing(&app, target.strip_prefix("app:").or(args.get("app"))));
    }
    let wait = Duration::from_millis(args.get("wait").map(duration_ms).transpose()?.unwrap_or(2000));
    let timeout = Duration::from_millis(args.get("timeout").map(duration_ms).transpose()?.unwrap_or(60_000));
    let keys = args.num("keys")?.unwrap_or(40.0).max(0.0) as usize;
    for l in crate::target::isolate::reap_dead_owners() {
        progress(l);
    }
    let home = isolate::prepare_home(&app, args.get("home").map(Path::new), None)?;
    // A look stopped midway (Ctrl-C, SIGTERM) closes the app instead of leaving it running.
    let stop = isolate::end_on_stop(Some(&home));
    if let Some((what, _)) = home.headless() {
        progress(format!("look: on {what}, a Wayland compositor strobe started for this launch"));
    }
    progress(format!("look: launching {} in an isolated home", app.exe.display()));
    let (mut n, guard) = match nt::launch(&app, &home, &[], false, None) {
        Ok(x) => x,
        Err(e) => {
            drop(stop);
            isolate::release_home(&home.dir);
            if home.fresh {
                isolate::remove_home(&home.dir);
            }
            return Err(e);
        }
    };
    let result = (|| -> Result<Output> {
        let started = Instant::now();
        let mut problems: Vec<String> = Vec::new();
        let main = if app.windowless {
            None
        } else {
            let w = n.wait_window(timeout, &guard)?;
            Some(w.hwnd)
        };
        let window_ms = started.elapsed().as_secs_f64() * 1e3;
        let session = main.map(|m| crate::driver::native::Session::open(&n.pids(), m));
        let until = args.get("until").map(|u| image_until(u).unwrap_or_else(|| Value::String(u.to_string())));
        match (&session, &until) {
            // An image that never showed says where the nearest thing to it was (`AnchorNotFound`).
            (Some(Ok(se)), Some(u)) if u.get("image").is_some() => {
                if let Err(e) = se.dispatch("waitFor", &[u.clone(), json!({ "timeout": timeout.as_millis() as u64 })], &std::cell::RefCell::new(None), None) {
                    problems.push(anchor_problem(e));
                }
            }
            (Some(Ok(se)), Some(sel)) if !se.shows(sel, timeout)? => problems.push(format!("{:?} never showed in {} s", args.get("until").unwrap_or(""), timeout.as_secs())),
            _ => {}
        }
        std::thread::sleep(wait);
        // An image: every place it shows now, read from outside.
        let images = match (&session, &until) {
            (Some(Ok(se)), Some(u)) if u.get("image").is_some() => Some(image_lines(&se.dispatch("query", std::slice::from_ref(u), &std::cell::RefCell::new(None), None)?)),
            _ => None,
        };
        let windows = crate::host::top_windows(&n.pids());
        let mut titles: Vec<String> =
            windows.iter().filter(|w| Some(w.0) == main).chain(windows.iter().filter(|w| Some(w.0) != main)).map(|w| w.1.trim().to_string()).filter(|t| !t.is_empty()).collect();
        let (mut headings, mut keyed) = (Vec::new(), Vec::new());
        match &session {
            Some(Ok(se)) => {
                match se.screen(Duration::from_secs(5)) {
                    Ok((names, h)) => {
                        for t in names {
                            if !titles.contains(&t) {
                                titles.push(t);
                            }
                        }
                        headings = h;
                    }
                    Err(e) => problems.push(format!("accessibility: {}", e.message)),
                }
                keyed = se.keys(keys).unwrap_or_default();
            }
            Some(Err(e)) => problems.push(format!("accessibility: {}", e.message)),
            None => problems.push("the app has no window (a windowless profile): nothing to read".into()),
        }
        let locked = crate::host::session_locked();
        let mut text = format!("{} · {}\n", titles.first().map(String::as_str).unwrap_or("(no title)"), app.exe.display());
        text += &format!(
            "window   {}{}\n",
            if main.is_some() { format!("after {}", fmt::value(window_ms, "ms")) } else { "none".into() },
            match (locked, cfg!(windows)) {
                (true, true) => " · session locked (the tree and the window's pixels still read; pointer input does not)",
                (true, false) => " · session locked (the tree still reads; pixels and input do not)",
                _ => "",
            }
        );
        for p in &problems {
            text += &format!("!        {p}\n");
        }
        for (i, t) in titles.iter().enumerate() {
            text += &format!("{}{t}\n", if i == 0 { "titles   " } else { "         " });
        }
        for (i, h) in headings.iter().enumerate() {
            text += &format!("{}{h}\n", if i == 0 { "headings " } else { "         " });
        }
        if headings.is_empty() && matches!(session, Some(Ok(_))) {
            text += "headings none: the tree marks no open dialog or heading, so an `expect.heading` would refuse this screen\n";
        }
        if !keyed.is_empty() {
            text += &format!("keys     {} visible keyed elements{}\n", keyed.len(), if keyed.len() == keys { format!(" (the first {keys}; --keys N for more)") } else { String::new() });
            for (path, role, name) in &keyed {
                text += &format!("  {path:<32} {role:<12} {name}\n");
            }
        }
        if let Some((t, _)) = &images {
            text += t;
        }
        text += "expect   a case's `expect: { title, heading }` matches one of the titles, and one of the headings\n";
        let json = json!({
            "target": target, "exe": app.exe, "window_ms": main.map(|_| window_ms), "session_locked": locked, "titles": titles, "headings": headings,
            "keys": keyed.iter().map(|(p, r, n)| json!({ "path": p, "role": r, "name": n })).collect::<Vec<_>>(), "problems": problems,
            "image": images.map(|(_, all)| all),
        });
        let mut o = Output::new(text, json);
        if !problems.is_empty() {
            o.code = 124;
        }
        Ok(o)
    })();
    // The stop watcher ends first: a stop now no longer races this close.
    drop(stop);
    n.close(home.fresh);
    result
}

/// `strobe look tty:<command>`: the program on strobe's own terminal (`target::tty`) in an isolated
/// home, as a tty() target's block starts it, nothing measured: its grid once it showed `--until` and
/// held still, the modes it set (mouse, keyboard protocol), and when it first drew. `--press` and
/// `--type` send input first, to see what a case's steps would show.
fn terminal(args: &Args, line: &str) -> Result<Output> {
    use crate::driver::tty as drv;
    use crate::target::{isolate, tty};
    for page_only in ["element", "full", "scale", "chars", "headed", "keys", "build", "app", "wayland"] {
        if args.has(page_only) {
            return Err(Error::usage(format!("--{page_only} is not a terminal's: look tty:<command> takes --size COLSxROWS, --until, --press, --type, --wait, --timeout, --home, --out <png>")));
        }
    }
    let (cols, rows) = match args.get("size") {
        Some(s) => {
            let (c, r) = s.split_once(['x', 'X', '×']).ok_or_else(|| Error::usage(format!("--size COLSxROWS (cells), not {s}")))?;
            (c.trim().parse().map_err(|_| Error::usage(format!("--size: {c}")))?, r.trim().parse().map_err(|_| Error::usage(format!("--size: {r}")))?)
        }
        None => (120u16, 40u16),
    };
    let wait = Duration::from_millis(args.get("wait").map(duration_ms).transpose()?.unwrap_or(1000));
    let timeout = Duration::from_millis(args.get("timeout").map(duration_ms).transpose()?.unwrap_or(30_000));
    if args.get("until").and_then(image_until).is_some() {
        return Err(crate::model::Refusal::new(crate::model::ValidatorId::NotForTarget, "--until image=: a terminal program draws a grid of cells, not pixels")
            .fix("wait for its text: --until \"text=…\"")
            .into());
    }
    let until = args.get("until").map(|u| drv::parse_sel(&Value::String(u.to_string()))).transpose()?;
    for l in isolate::reap_dead_owners() {
        progress(l);
    }
    let home = isolate::prepare_home(&isolate::App::bare(Path::new("tty")), args.get("home").map(Path::new), None)?;
    let stop = isolate::end_on_stop(Some(&home));
    // As a block's: the program gets its event stream, and look says what came on it.
    let events = crate::target::events::Events::open(&home.dir, None)?;
    let mut env = home.env.clone();
    env.extend(tty::private_tmp(&home.dir)?);
    env.push(events.env());
    // The user's own Documents, Desktop and shared seeds, held to what they were (rule 10).
    let mut guard = isolate::Guard::take(&isolate::App::bare(Path::new("tty")));
    guard.shared = home.shared.clone();
    let spec = tty::Spec { run: tty::Run::Line(home.fill(line)), cwd: std::env::current_dir()?, env, unset: Vec::new(), cols, rows, home: Some(home.dir.clone()) };
    progress(format!("look: {line} on a {cols}×{rows} terminal in an isolated home"));
    let result = (|| -> Result<Output> {
        let t = tty::Tty::spawn(&spec, None, None)?;
        let started = t.started_ns;
        let mut problems: Vec<String> = Vec::new();
        let mut shown_at = None;
        if let Some(sel) = &until {
            let ok = t.wait(Instant::now() + timeout, |g, last, _| {
                let hit = !drv::find(g, sel).is_empty();
                if hit {
                    shown_at = Some(last);
                }
                hit
            });
            if !ok {
                problems.push(format!("{:?} never showed in {} s{}", sel.source, timeout.as_secs(), if t.ended() { " (the program exited)" } else { "" }));
            }
        }
        let d = drv::Driver::new(&t, None);
        if let Some(k) = args.get("press") {
            d.dispatch("press", &[json!(k), json!({ "timeout": timeout.as_millis() as u64 })], None)?;
        }
        if let Some(text) = args.get("type") {
            d.dispatch("type", &[json!(""), json!(text), json!({ "timeout": timeout.as_millis() as u64 })], None)?;
        }
        // Then until it holds still for `--wait` (at most the timeout).
        if !t.quiet(wait, crate::host::now_ns(), Instant::now() + timeout) {
            problems.push(format!("the grid never held still for {} ms in {} s", wait.as_millis(), timeout.as_secs()));
        }
        let first = t.updates(started, u64::MAX).first().map(|u| (u.commit_ns - started) as f64 / 1e6);
        let ups = t.updates(started, u64::MAX);
        let modes = t.modes();
        let (title, text) = t.grid(|g| (g.title.to_string(), g.text()));
        // `--out`: the grid drawn as `--pixels` snapshots draw it.
        if let Some(out) = args.get("out") {
            let img = t.grid(tty::image::image)?;
            std::fs::write(out, crate::png::write(&img)?)?;
            progress(format!("look: the grid drawn to {out}"));
        }
        let ended = t.ended().then(|| t.exited()).flatten();
        let shown_ms = shown_at.map(|a| (a - started) as f64 / 1e6);
        let mut out = format!("{} · {line}\n", if title.is_empty() { "(no title)" } else { &title });
        out += &format!(
            "drew     first after {} · {} updates ({} synchronized frames) · {} bytes{}\n",
            first.map_or("never".into(), |ms| fmt::value(ms, "ms")),
            ups.len(),
            ups.iter().filter(|u| u.synced).count(),
            t.bytes(started, u64::MAX),
            shown_ms.map_or(String::new(), |ms| format!(" · --until showed after {}", fmt::value(ms, "ms")))
        );
        out += &format!(
            "modes    mouse {} · keyboard {} · cursor keys {}\n",
            serde_json::to_value(modes.mouse).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default() + if modes.sgr { " (SGR)" } else { "" },
            if modes.kitty == 0 { "xterm".to_string() } else { format!("progressive flags {}", modes.kitty) },
            if modes.app_cursor { "application" } else { "normal" }
        );
        if let Some(code) = ended {
            problems.push(format!("the program exited with {code}"));
        }
        let got = events.between(0, u64::MAX);
        if !got.is_empty() || !events.bad().is_empty() {
            out += &format!(
                "events   {} on $STROBE_EVENTS ({}){}\n",
                got.len(),
                got.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", "),
                if events.bad().is_empty() { String::new() } else { format!(" · {} lines that are no event", events.bad().len()) }
            );
        }
        for p in &problems {
            out += &format!("!        {p}\n");
        }
        let last = text.iter().rposition(|r| !r.trim().is_empty()).map_or(0, |i| i + 1);
        out += &format!("screen   {cols}×{rows}\n");
        for r in &text[..last] {
            out += &format!("  │{r}\n");
        }
        let json = json!({
            "target": format!("tty:{line}"), "title": title, "cols": cols, "rows": rows, "first_ms": first, "until_ms": shown_ms, "updates": ups.len(),
            "frames": ups.iter().filter(|u| u.synced).count(), "bytes": t.bytes(started, u64::MAX), "mouse": modes.mouse, "sgr": modes.sgr, "kitty": modes.kitty,
            "screen": text, "exited": ended, "problems": problems,
        });
        drop(d);
        t.close();
        let mut o = Output::new(out, json);
        if !problems.is_empty() {
            o.code = 124;
        }
        Ok(o)
    })();
    drop(stop);
    drop(events);
    isolate::end_launch(None, Some(&home.dir), true);
    result.and_then(|o| guard.check("after the look").map(|_| o))
}

pub fn run(args: &Args) -> Result<Output> {
    let target = args
        .pos
        .first()
        .ok_or_else(|| Error::usage("look needs a page or an app: strobe look <url | file | wir:[sel] | app:<id> | exe:<path> | tty:<command>> [--until <css>] [--wait 2s] [--out shot.png]"))?;
    if target == "wir" || target.starts_with("wir:") {
        return ios_wir(args, target);
    }
    if target.starts_with("app:") || target.starts_with("exe:") {
        return native(args, target);
    }
    if let Some(line) = target.strip_prefix("tty:") {
        if args.has("mobile") {
            return Err(Error::usage("--mobile opens a page in Chrome on a phone; a terminal program (tty:) runs on strobe's own terminal, on this machine"));
        }
        return terminal(args, line);
    }
    for tty_only in ["type", "press"] {
        if args.has(tty_only) {
            return Err(Error::usage(format!("--{tty_only} is for a terminal program (look tty:<command>)")));
        }
    }
    if args.has("full") && args.has("element") {
        return Err(Error::usage("--full shoots the whole page and --element one element of it: give one"));
    }
    for app_only in ["keys", "build", "app", "home", "wayland"] {
        if args.has(app_only) {
            return Err(Error::usage(format!("--{app_only} is for a native app (look app:<id> or exe:<path>), not a page")));
        }
    }
    // `--mobile`: the page in Chrome on a phone, at the phone's own screen.
    let mobile = args.get("mobile").map(str::to_string);
    if mobile.is_some()
        && let Some(f) = ["size", "scale", "headed"].iter().find(|f| args.has(f))
    {
        return Err(Error::usage(format!("--{f} sizes a desktop browser's page; on a phone (--mobile) the page is the phone's own screen")));
    }
    let url = address(target);
    let url = if mobile.is_some() { crate::target::android_chrome::served(&url)? } else { url };
    let (width, height) = match args.get("size") {
        Some(s) => {
            let (w, h) = s.split_once(['x', 'X', '×']).ok_or_else(|| Error::usage(format!("--size WIDTHxHEIGHT, not {s}")))?;
            (w.trim().parse().map_err(|_| Error::usage(format!("--size: {w}")))?, h.trim().parse().map_err(|_| Error::usage(format!("--size: {h}")))?)
        }
        None => (1280u32, 800u32),
    };
    let wait = args.get("wait").map(duration_ms).transpose()?.unwrap_or(1000);
    let chars = args.num("chars")?.unwrap_or(4000.0).max(0.0) as usize;
    let timeout = Duration::from_millis(args.get("timeout").map(duration_ms).transpose()?.unwrap_or(30_000));
    let window = Window { x: 0, y: 0, width, height, scale: args.num("scale")? };
    for l in crate::target::isolate::reap_dead_owners() {
        progress(l);
    }
    // A look stopped midway ends the browser it launched (its profile goes with the next strobe).
    let stop = crate::target::isolate::end_on_stop(None);
    let browser = match &mobile {
        Some(serial) => {
            let phone = crate::target::android_chrome::pick(serial)?;
            progress(format!("look: {} {} ({})", phone.facts.manufacturer, phone.facts.model, phone.device.serial));
            crate::target::android_chrome::launch(&phone, "look", false, Duration::ZERO)?
        }
        None => chromium::launch(&window, !args.has("headed"), None, &[])?,
    };
    let result = (|| -> Result<Output> {
        let page = browser.page()?;
        let events = browser.cdp.events();
        page.send("Runtime.enable", json!({}))?;
        page.send("Log.enable", json!({}))?;
        // A look on a phone loads the page as the channel has it: it changes nothing of the channel's data.
        if let Some(phone) = &browser.phone {
            phone.load(&url)?;
        } else if !args.has("headed") {
            page.send("Emulation.setDeviceMetricsOverride", json!({ "width": width, "height": height, "deviceScaleFactor": window.scale.unwrap_or(1.0), "mobile": false }))?;
        }
        progress(format!("look {url}"));
        let started = Instant::now();
        let loaded = page.navigate(&url, timeout);
        let load_ms = started.elapsed().as_secs_f64() * 1e3;
        let mut problems: Vec<String> = Vec::new();
        match loaded {
            Err(e) if e.code == 255 => return Err(e),
            Err(e) => problems.push(format!("the load event never came: {}", e.message)),
            Ok(()) => {}
        }
        // The same selectors and probe as a bench's page calls and `expect`, so what look says is what they see.
        let image = args.get("until").and_then(image_until);
        if let Some(sel) = args.get("until") {
            match &image {
                // An image that never showed says where the nearest thing to it was (`AnchorNotFound`).
                Some(v) => {
                    if let Some(Err(e)) = crate::driver::page_image_wait(&page.clone().into(), v, timeout) {
                        problems.push(anchor_problem(e));
                    }
                }
                None if !crate::driver::shows(&page.clone().into(), &Selector::parse(&Value::String(sel.to_string()), Dialect::Page)?, timeout)? => {
                    problems.push(format!("{sel:?} never showed in {} s", timeout.as_secs()));
                }
                None => {}
            }
        }
        std::thread::sleep(Duration::from_millis(wait));
        // An image: every place it shows now, through the browser's screenshot.
        let images = match &image {
            Some(v) => crate::driver::page_image_query(&page.clone().into(), v).map(|r| r.map(|f| image_lines(&f))).transpose()?,
            None => None,
        };
        let info = crate::driver::screen(&page.clone().into(), 6, chars, Duration::from_secs(10))?;
        // Everything the page said while it loaded and settled.
        let mut console: Vec<Value> = Vec::new();
        for ev in events.try_iter().filter(|e| e["sessionId"].as_str() == Some(page.session.as_str())) {
            let p = &ev["params"];
            match ev["method"].as_str().unwrap_or("") {
                "Runtime.consoleAPICalled" => console.push(json!({ "level": p["type"], "text": console_text(&p["args"]) })),
                "Runtime.exceptionThrown" => {
                    let d = &p["exceptionDetails"];
                    let text = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception").lines().next().unwrap_or("").to_string();
                    console.push(json!({ "level": "exception", "text": text, "where": format!("{}:{}", d["url"].as_str().unwrap_or(""), d["lineNumber"].as_u64().unwrap_or(0) + 1) }));
                }
                "Log.entryAdded" => console.push(json!({ "level": p["entry"]["level"], "text": p["entry"]["text"], "where": p["entry"]["url"] })),
                _ => {}
            }
        }
        use base64::Engine;
        // `--element`: the screenshot is that element's box on the page (scrolled to or not), so one
        // section of a long page reads at full size.
        let clip = match args.get("element") {
            Some(sel) => {
                let sel = Selector::parse(&Value::String(sel.to_string()), Dialect::Page)?;
                match crate::driver::element_box(&page.clone().into(), &sel, Duration::from_secs(10))? {
                    Some([x, y, w, h]) if w >= 1.0 && h >= 1.0 => Some(json!({ "x": x, "y": y, "width": w, "height": h, "scale": 1 })),
                    Some(_) => return Err(Error::usage(format!("--element {:?}: the element has no size to show (0×0)", sel.source))),
                    None => {
                        return Err(Error::usage(format!("--element {:?}: no visible element matches it", sel.source))
                            .hint("strobe look <page> lists the headings and text; a selector is CSS, then role=, text=, index="));
                    }
                }
            }
            None => None,
        };
        let mut params = json!({ "format": "png", "captureBeyondViewport": args.has("full") || clip.is_some() });
        if let Some(c) = &clip {
            params["clip"] = c.clone();
        }
        let shot = page.send_timeout("Page.captureScreenshot", params, Duration::from_secs(15))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str().unwrap_or("")).map_err(|e| Error::msg(format!("screenshot: {e}")))?;
        let out: PathBuf = match args.get("out") {
            Some(p) => PathBuf::from(p),
            None => crate::host::paths::run().join(format!("look-{}.png", crate::host::now_ns())),
        };
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        crate::host::paths::write_owner_only(&out, &bytes)?;
        let errors = console.iter().filter(|c| matches!(c["level"].as_str(), Some("error" | "exception"))).count();
        let mut text = format!("{} · {}\n", info["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("(no title)"), info["url"].as_str().unwrap_or(&url));
        // A phone's page is its own screen, read from the page.
        let phone = browser.phone.as_ref().map(|p| p.meta(crate::target::android_chrome::screen(&page)));
        let size = match &phone {
            Some(p) => format!(
                "{}×{} @{}x · {} {} · {}",
                p["screen"]["viewport"][0],
                p["screen"]["viewport"][1],
                p["screen"]["dpr"],
                p["manufacturer"].as_str().unwrap_or(""),
                p["model"].as_str().unwrap_or(""),
                browser.version
            ),
            None => format!("{width}×{height}"),
        };
        text += &format!("load     {} · {} · {size}\n", fmt::value(load_ms, "ms"), info["ready"].as_str().unwrap_or("?"));
        for p in &problems {
            text += &format!("!        {p}\n");
        }
        let headings: Vec<&str> = info["headings"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        for (i, h) in headings.iter().enumerate() {
            text += &format!("{}{h}\n", if i == 0 { "headings " } else { "         " });
        }
        if headings.is_empty() {
            text += "headings none: the page shows no open dialog, h1, h2 or level 1-2 role=heading, so an `expect.heading` would refuse it\n";
        }
        text += &format!("console  {} messages, {errors} errors\n", console.len());
        for c in console.iter().take(20) {
            let at = c["where"].as_str().filter(|w| !w.is_empty()).map(|w| format!("  ({w})")).unwrap_or_default();
            text += &format!("  {:<10}{}{at}\n", c["level"].as_str().unwrap_or(""), c["text"].as_str().unwrap_or("").lines().next().unwrap_or(""));
        }
        if console.len() > 20 {
            text += &format!("  … {} more (--json lists them all)\n", console.len() - 20);
        }
        let body = info["text"].as_str().unwrap_or("");
        let total = info["chars"].as_u64().unwrap_or(0);
        text += &format!("text     {total} characters{}\n", if (total as usize) > body.len() { format!(", the first {} (--chars N for more)", body.chars().count()) } else { String::new() });
        for l in body.lines() {
            text += &format!("  {l}\n");
        }
        text += &format!(
            "shot     {}{}\n",
            out.display(),
            clip.as_ref().map(|c| format!(" ({} only, {:.0}×{:.0})", args.get("element").unwrap_or(""), c["width"].as_f64().unwrap_or(0.0), c["height"].as_f64().unwrap_or(0.0))).unwrap_or_default()
        );
        if let Some((t, _)) = &images {
            text += t;
        }
        text += "expect   a case's `expect: { title, heading }` matches the title, and one of the headings\n";
        let json = json!({ "url": info["url"], "title": info["title"], "load_ms": load_ms, "ready": info["ready"], "problems": problems, "headings": info["headings"], "console": console, "errors": errors, "text": body, "chars": total, "screenshot": out, "clip": clip, "phone": phone, "image": images.map(|(_, all)| all) });
        let mut o = Output::new(text, json);
        // The page never loaded, or never showed `--until`: a timeout. Its own console errors are the
        // page's, and listed.
        if !problems.is_empty() {
            o.code = 124;
        }
        Ok(o)
    })();
    drop(stop);
    browser.close();
    result
}

/// Pure, side-effect-free selector matcher and screen reader evaluated in an attached iOS Safari tab:
/// leaves no globals on `window`, patches no DOM methods, and never scrolls the user's viewport.
const WIR_READ_SCRIPT: &str = r#"((max, chars, elSpec) => {
  const visible = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && (el.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true) }
  const implicit = (el) => {
    const type = (el.getAttribute("type") ?? "").toLowerCase()
    switch (el.localName) {
      case "a": case "area": return el.hasAttribute("href") ? "link" : ""
      case "button": case "summary": return "button"
      case "input": return { button: "button", submit: "button", reset: "button", image: "button", checkbox: "checkbox", radio: "radio", range: "slider", number: "spinbutton", search: "searchbox" }[type]
        ?? (["hidden", "file", "color", "date", "datetime-local", "month", "time", "week"].includes(type) ? "" : "textbox")
      case "textarea": return "textbox"
      case "select": return el.multiple || el.size > 1 ? "listbox" : "combobox"
      case "option": return "option"
      case "h1": case "h2": case "h3": case "h4": case "h5": case "h6": return "heading"
      case "img": return el.getAttribute("alt") === "" ? "presentation" : "img"
      case "ul": case "ol": case "menu": return "list"
      case "li": return "listitem"
      case "nav": return "navigation"
      case "main": return "main"
      case "aside": return "complementary"
      case "header": return "banner"
      case "footer": return "contentinfo"
      case "dialog": return "dialog"
      case "table": return "table"
      case "tr": return "row"
      case "td": return "cell"
      case "th": return "columnheader"
      case "progress": return "progressbar"
      case "hr": return "separator"
      case "form": return "form"
      case "article": return "article"
      case "output": return "status"
    }
    return ""
  }
  const alias = { image: "img", edit: "textbox" }
  const roleOf = (el) => (el.getAttribute("role") ?? "").trim().split(/\s+/)[0].toLowerCase() || implicit(el)
  const isRole = (el, want) => { const r = roleOf(el), w = alias[want] ?? want; return r === w || (w === "textbox" && r === "searchbox") }
  const has = (el, t, exact) => t == null || [el.textContent ?? "", el.getAttribute("aria-label") ?? "", typeof el.value === "string" ? el.value : ""].some((x) => (exact ? x.trim() === t.trim() : x.includes(t)))
  const up = (el) => el.parentElement ?? el.getRootNode().host ?? null
  const query = (root, [first, ...rest]) => {
    const found = [...root.querySelectorAll(first)]
    return rest.length ? found.flatMap((h) => (h.shadowRoot ? query(h.shadowRoot, rest) : [])) : found
  }
  const deepAll = (root, out) => { for (const el of root.querySelectorAll("*")) { out.push(el); if (el.shadowRoot) deepAll(el.shadowRoot, out) } return out }
  const find = (q) => {
    const all = q.css == null || q.css === "*" ? deepAll(document, []) : query(document, q.css.split(">>>").map((p) => p.trim()))
    const found = all.filter((el) => (q.role == null || isRole(el, q.role)) && (!q.focus || el.matches(":focus")) && has(el, q.text, q.exact) && visible(el))
    if (!found.length) return null
    if (q.text != null) {
      const outer = new Set()
      for (const el of found) for (let p = up(el); p && !outer.has(p); p = up(p)) outer.add(p)
      const inner = found.filter((el) => !outer.has(el))
      return inner.at(q.index ?? 0) ?? null
    }
    return found.at(q.index ?? 0) ?? null
  }
  const text = (e) => (e.getAttribute("aria-label") || e.innerText || e.textContent || "").trim().replace(/\s+/g, " ").slice(0, 80)
  const out = []
  for (const d of document.querySelectorAll("dialog[open], [role=dialog], [role=alertdialog], [aria-modal=true]"))
    if (visible(d)) out.push(text(d.querySelector("h1, h2, h3, [role=heading]") ?? d))
  for (const h of document.querySelectorAll("h1, h2, [role=heading]"))
    if (Number(h.getAttribute("aria-level") ?? (h.localName === "h1" ? 1 : 2)) <= 2 && visible(h)) out.push(text(h))
  const body = chars > 0 ? (document.body?.innerText ?? "").replace(/\n{3,}/g, "\n\n").trim() : ""
  let box = null
  if (elSpec) {
    const el = find(elSpec)
    if (el) { const r = el.getBoundingClientRect(); box = [r.left + scrollX, r.top + scrollY, r.width, r.height] }
  }
  const de = document.documentElement
  return JSON.stringify({
    title: document.title,
    url: location.href,
    ready: document.readyState,
    viewport: [Math.round(innerWidth), Math.round(innerHeight)],
    dpr: devicePixelRatio || 1,
    doc: [Math.round(Math.max(de?.scrollWidth ?? 0, innerWidth)), Math.round(Math.max(de?.scrollHeight ?? 0, innerHeight))],
    headings: [...new Set(out.filter(Boolean))].slice(0, max),
    text: body.slice(0, chars),
    chars: body.length,
    box,
  })
})"#;

/// `strobe look wir:[selector]`: strictly read-only snapshot of an open iOS Safari tab or WKWebView over
/// USB. Reads title, URL, headings, buffered and live console messages, visible text, and a PNG via
/// `Page.snapshotRect`; never navigates, reloads, scrolls or mutates the page, and always detaches cleanly.
fn ios_wir(args: &Args, target: &str) -> Result<Output> {
    use crate::host::ios::logs::{redact_text, scrub_url};
    use crate::target::wir;

    for bad in ["size", "scale", "headed"] {
        if args.has(bad) {
            return Err(Error::usage(format!("--{bad} sizes a desktop browser's window; look wir: reads the phone's own Safari tab")));
        }
    }
    if args.has("mobile") {
        return Err(Error::usage("--mobile opens a page in Chrome on an Android phone; wir: reads an open Safari tab on an iPhone"));
    }
    for bad in ["type", "press"] {
        if args.has(bad) {
            return Err(Error::usage(format!("--{bad} sends input; look wir: is strictly read-only")));
        }
    }
    for bad in ["keys", "build", "app", "home", "wayland"] {
        if args.has(bad) {
            return Err(Error::usage(format!("--{bad} is for a native app (look app:<id> or exe:<path>), not wir:")));
        }
    }
    if args.has("full") && args.has("element") {
        return Err(Error::usage("--full shoots the whole page and --element one element of it: give one"));
    }
    if args.get("until").and_then(image_until).is_some() {
        return Err(crate::model::Refusal::new(crate::model::ValidatorId::NotForTarget, "--until image=: look wir: inspects an open Safari tab over USB without polling screenshots in a loop")
            .fix("wait for a DOM or role selector (--until \"role=heading …\"), or take one screenshot with strobe look wir: --out shot.png")
            .into());
    }

    let wait_ms = args.get("wait").map(duration_ms).transpose()?.unwrap_or(200);
    let chars = args.num("chars")?.unwrap_or(4000.0).max(0.0) as usize;
    let timeout = Duration::from_millis(args.get("timeout").map(duration_ms).transpose()?.unwrap_or(15_000));
    let until_sel = args.get("until").map(|s| Selector::parse(&Value::String(s.to_string()), Dialect::Page)).transpose()?;
    let elem_sel = args.get("element").map(|s| Selector::parse(&Value::String(s.to_string()), Dialect::Page)).transpose()?;

    let t0_ns = crate::host::now_ns();
    let started = Instant::now();
    let mut session = wir::Session::attach(target, false, timeout, super::stop_requested)?;
    let handle = session.page.handle.clone();
    progress(format!("look {handle}"));

    let mut problems: Vec<String> = Vec::new();
    let mut raw_console: Vec<wir::ConsoleEntry> = Vec::new();

    // Console.enable flushes buffered messages first, then streams live ones (Runtime.evaluate needs no Runtime.enable).
    let _ = session.call("Console.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Page.enable", Value::Null, Duration::from_secs(5));

    if let Some(sel) = &until_sel {
        let deadline = Instant::now() + timeout;
        let check_expr = format!("JSON.parse({WIR_READ_SCRIPT}(0, 0, {})).box !== null", sel.page_spec());
        let mut matched = false;
        while Instant::now() < deadline {
            if session.eval(&check_expr, Duration::from_secs(5))? == Value::Bool(true) {
                matched = true;
                break;
            }
            session.pump_for(Duration::from_millis(100))?;
        }
        if !matched {
            problems.push(format!("{:?} never showed in {} s", sel.source, timeout.as_secs()));
        }
    }

    if wait_ms > 0 {
        session.pump_for(Duration::from_millis(wait_ms))?;
    }

    let read_expr = format!("{WIR_READ_SCRIPT}(6, {chars}, {})", elem_sel.as_ref().map_or_else(|| "null".to_string(), |s| s.page_spec().to_string()));
    let info_raw = session.eval(&read_expr, Duration::from_secs(10))?;
    let info: Value = info_raw.as_str().and_then(|s| serde_json::from_str(s).ok()).ok_or_else(|| Error::msg("the Safari tab's screen did not read"))?;
    let attach_ms = started.elapsed().as_secs_f64() * 1e3;

    let vp_w = info["viewport"][0].as_i64().unwrap_or(390).max(1);
    let vp_h = info["viewport"][1].as_i64().unwrap_or(844).max(1);
    let dpr = info["dpr"].as_f64().unwrap_or(1.0);
    let doc_w = info["doc"][0].as_i64().unwrap_or(vp_w).max(1);
    let doc_h = info["doc"][1].as_i64().unwrap_or(vp_h).clamp(1, 16_384);

    let (sx, sy, sw, sh, coord, clip) = if let Some(sel) = &elem_sel {
        match info["box"].as_array() {
            Some(b) if b.len() == 4 => {
                let (x, y, w, h) = (b[0].as_f64().unwrap_or(0.0), b[1].as_f64().unwrap_or(0.0), b[2].as_f64().unwrap_or(0.0), b[3].as_f64().unwrap_or(0.0));
                if w < 1.0 || h < 1.0 {
                    return Err(Error::usage(format!("--element {:?}: the element has no size to show (0×0)", sel.source)));
                }
                (x.floor() as i64, y.floor() as i64, w.ceil() as i64, h.ceil() as i64, "Page", Some(json!({ "x": x, "y": y, "width": w, "height": h })))
            }
            _ => {
                return Err(Error::usage(format!("--element {:?}: no visible element matches it", sel.source))
                    .hint(format!("strobe look {handle} lists the headings and text; a selector is CSS, then role=, text=, index=")));
            }
        }
    } else if args.has("full") {
        (0, 0, doc_w, doc_h, "Page", None)
    } else {
        (0, 0, vp_w, vp_h, "Viewport", None)
    };

    let png_bytes = session.snapshot_rect(sx, sy, sw, sh, coord, Duration::from_secs(15))?;
    let events = session.drain_events();
    wir::collect_console_events(&events, t0_ns, &mut raw_console);
    let phone_meta = json!({
        "platform": "ios",
        "udid": session.phone.udid,
        "short_udid": crate::host::ios::short_udid(&session.phone.udid),
        "model": session.phone.product_type,
        "ios": session.phone.ios,
        "app": session.page.app_name,
        "bundle_id": session.page.bundle_id,
        "page_id": session.page.page_id,
        "handle": handle,
        "transport": "usb",
        "battery": session.phone.battery,
        "screen": { "viewport": [vp_w, vp_h], "dpr": dpr },
    });
    // Detach immediately so attached time on the developer's phone stays minimal.
    session.close();

    let out_path: PathBuf = match args.get("out") {
        Some(p) => PathBuf::from(p),
        None => crate::host::paths::run().join(format!("look-{}.png", crate::host::now_ns())),
    };
    if let Some(dir) = out_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    crate::host::paths::write_owner_only(&out_path, &png_bytes)?;

    let url = scrub_url(info["url"].as_str().unwrap_or(&session.page.url), false);
    let title = info["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("(no title)");
    let console: Vec<Value> = raw_console
        .iter()
        .map(|c| {
            let loc = match (&c.url, c.line) {
                (Some(u), Some(l)) => format!("{}:{l}", scrub_url(u, false)),
                (Some(u), None) => scrub_url(u, false),
                _ => String::new(),
            };
            json!({
                "level": c.level,
                "source": c.source,
                "text": redact_text(&c.text, false),
                "where": loc,
                "repeat": c.repeat_count,
            })
        })
        .collect();
    let errors = console.iter().filter(|c| matches!(c["level"].as_str(), Some("error" | "exception"))).count();

    let model_label = phone_meta["model"].as_str().unwrap_or("iPhone");
    let ios_label = phone_meta["ios"].as_str().map(|v| format!("iOS {v}")).unwrap_or_else(|| "iOS".into());
    let mut text = format!("{title} · {url}\n");
    text += &format!("page     {handle} · {} · {vp_w}×{vp_h} @{dpr}x · {model_label} · {ios_label} · read in {}\n", info["ready"].as_str().unwrap_or("?"), fmt::value(attach_ms, "ms"));
    for p in &problems {
        text += &format!("!        {p}\n");
    }
    let headings: Vec<&str> = info["headings"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    for (i, h) in headings.iter().enumerate() {
        text += &format!("{}{h}\n", if i == 0 { "headings " } else { "         " });
    }
    if headings.is_empty() {
        text += "headings none: the page shows no open dialog, h1, h2 or level 1-2 role=heading\n";
    }
    text += &format!("console  {} messages, {errors} errors\n", console.len());
    for c in console.iter().take(20) {
        let at = c["where"].as_str().filter(|w| !w.is_empty()).map(|w| format!("  ({w})")).unwrap_or_default();
        let rep = c["repeat"].as_u64().filter(|&r| r > 1).map(|r| format!(" ×{r}")).unwrap_or_default();
        text += &format!("  {:<10}{}{rep}{at}\n", c["level"].as_str().unwrap_or(""), c["text"].as_str().unwrap_or("").lines().next().unwrap_or(""));
    }
    if console.len() > 20 {
        text += &format!("  … {} more (--json lists them all)\n", console.len() - 20);
    }
    let body = redact_text(info["text"].as_str().unwrap_or(""), false);
    let total = info["chars"].as_u64().unwrap_or(0);
    text += &format!("text     {total} characters{}\n", if (total as usize) > body.len() { format!(", the first {} (--chars N for more)", body.chars().count()) } else { String::new() });
    for l in body.lines() {
        text += &format!("  {l}\n");
    }
    text += &format!(
        "shot     {}{}\n",
        out_path.display(),
        clip.as_ref().map(|c| format!(" ({} only, {:.0}×{:.0})", args.get("element").unwrap_or(""), c["width"].as_f64().unwrap_or(0.0), c["height"].as_f64().unwrap_or(0.0))).unwrap_or_default()
    );

    let json = json!({
        "target": handle,
        "url": url,
        "title": info["title"],
        "attach_ms": attach_ms,
        "ready": info["ready"],
        "problems": problems,
        "headings": info["headings"],
        "console": console,
        "errors": errors,
        "text": body,
        "chars": total,
        "screenshot": out_path,
        "clip": clip,
        "phone": phone_meta,
    });
    let mut o = Output::new(text, json);
    if !problems.is_empty() {
        o.code = 124;
    }
    Ok(o)
}
