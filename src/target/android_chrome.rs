//! A page in Chrome on an Android phone or emulator over adb (SPEC 13.1 "Chrome on a phone", `--mobile`):
//! the phone's own Chrome, its own screen and touch, no emulation. strobe reaches Chrome's DevTools socket
//! (`@chrome_devtools_remote`) through an adb forward and drives one tab of its own over CDP, so every
//! page path (the driver, the trace, profiles, heap snapshots) works as on a desktop Chromium; what it
//! reads from this computer's processes is not the phone's (`validity::phone_refusal`).
//!
//! Never a person's browser. Force-stopping a channel ends its incognito tabs, its installed web apps,
//! Custom Tabs it shows inside other apps and its downloads, and clearing an origin signs its person out:
//! on a phone (not an emulator) strobe never touches Chrome itself (`com.android.chrome`) and drives only
//! a channel (Canary, Dev, Beta) a person gave it once for that phone (`strobe up --mobile=<serial>
//! --channel canary`, `opted_in`). A channel that holds tabs a person opened is refused `LiveChannel` (or
//! exit 3 while it runs and holds the DevTools socket) and left as it was. No `pm clear`, no `am
//! set-debug-app`; Chrome's command-line file only on a debuggable device (an emulator), and only where
//! none is (`adb::CommandLine`).

use super::cdp::Cdp;
use super::chromium::{self, Browser, Window};
use crate::host::adb::{self, Adb, CommandLine, Device, Facts, Held, PhoneSlot, StayAwake, Tunnel};
use crate::host::paths;
use crate::model::{Error, Refusal, Result, ValidatorId};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Chrome's channels on Android: the one people run, then the ones made for testing.
pub const CHANNELS: [(&str, &str); 4] = [("com.android.chrome", "Chrome"), ("com.chrome.beta", "Chrome Beta"), ("com.chrome.dev", "Chrome Dev"), ("com.chrome.canary", "Chrome Canary")];
/// Chrome itself: on a phone it is a person's browser, which strobe never drives.
const STABLE: &str = "com.android.chrome";
/// The channels strobe may drive on a phone once a person gave it one, in the order it takes them.
const DEDICATED: [&str; 3] = ["com.chrome.canary", "com.chrome.dev", "com.chrome.beta"];
/// The channels `strobe up --mobile --channel <name>` gives strobe.
pub const CHANNEL_NAMES: [(&str, &str); 3] = [("canary", "com.chrome.canary"), ("dev", "com.chrome.dev"), ("beta", "com.chrome.beta")];
/// Every channel's launcher activity.
const ACTIVITY: &str = "com.google.android.apps.chrome.Main";
/// Every channel's DevTools socket: one name, so only one channel answers at a time.
const SOCKET: &str = "chrome_devtools_remote";
/// What a debuggable device's Chrome gets on its command line: no first-run screens, which keep the
/// DevTools socket closed.
const FIRST_RUN_OFF: &str = "_ --disable-fre --no-default-browser-check --no-first-run";
/// How long a hot phone may cool before a timed block refuses `Thermal`.
const COOLDOWN: Duration = Duration::from_secs(180);

/// The phone a `--mobile` run uses, and what it was when picked.
#[derive(Clone)]
pub struct Phone {
    pub device: Device,
    pub facts: Facts,
    /// The phone in a file name and an experiment's identity (`Device::key`): its hardware serial,
    /// the same over USB and Wi-Fi.
    pub key: String,
}

/// The phone `--mobile` names: its serial (`--mobile=<serial>`), else the one attached (`ANDROID_SERIAL`).
pub fn pick(serial: &str) -> Result<Phone> {
    let device = Adb::connect()?.pick((!serial.is_empty()).then_some(serial))?;
    let facts = device.facts()?;
    let key = device.key();
    Ok(Phone { device, facts, key })
}

/// What a `--mobile` run measures in, for an experiment's identity and its plan's history: the phone
/// (its key, model and Android build) and the Chrome channel and version a launch would take (the first
/// channel strobe may drive there). Read without starting anything.
pub fn identity(phone: &Phone) -> String {
    let d = &phone.device;
    let installed = installed(d).unwrap_or_default();
    let channel = if phone.facts.emulator.is_some() { installed.first().copied() } else { opted_in(d).into_iter().find(|p| installed.contains(p)) };
    let version = channel.and_then(|p| d.sh(&format!("dumpsys package {p} | grep -m 1 versionName=")).ok()).map(|v| v.trim().trim_start_matches("versionName=").to_string()).unwrap_or_default();
    format!("phone {} ({} {}, {}) · {} {version}", phone.key, phone.facts.manufacturer, phone.facts.model, phone.facts.fingerprint, channel.unwrap_or("no channel"))
}

fn name(package: &str) -> &str {
    CHANNELS.iter().find(|(p, _)| *p == package).map_or(package, |(_, n)| n)
}

/// Where the channels people gave strobe are kept, by phone (`Device::key`).
fn opt_ins() -> PathBuf {
    paths::state_dir().join("phones.json")
}

/// The channels a person gave strobe on this phone (`strobe up --mobile --channel`), in `DEDICATED` order.
pub fn opted_in(d: &Device) -> Vec<&'static str> {
    given(&d.key())
}

/// `opted_in` by the phone's key (`Device::key`, `adb::key_of`).
pub fn given(key: &str) -> Vec<&'static str> {
    let all: Value = std::fs::read(opt_ins()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let mine: Vec<String> = all[key]["channels"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    DEDICATED.into_iter().filter(|p| mine.iter().any(|m| m == p)).collect()
}

/// The channels' names, for a line (`Chrome Canary, Chrome Beta`).
pub fn names(packages: &[&str]) -> String {
    packages.iter().map(|p| name(p)).collect::<Vec<_>>().join(", ")
}

/// `strobe up --mobile=<serial> --channel <name>`: gives strobe one Chrome channel on this phone, one that
/// is installed, has finished its first run and is kept for strobe: it may force-stop it, open and close
/// its own tab there and clear the origins it loads. Never Chrome itself. Returns what it gave.
pub fn opt_in(phone: &Phone, channel: &str) -> Result<String> {
    let d = &phone.device;
    let Some((_, package)) = CHANNEL_NAMES.iter().find(|(n, _)| *n == channel) else {
        return Err(Error::usage(format!("--channel: canary, dev or beta, not {channel}: strobe never drives Chrome itself on a phone (it is a person's browser)")));
    };
    if !installed(d)?.contains(package) {
        return Err(Error::new(3, format!("{} is not installed on {}", name(package), d.serial))
            .hint(format!("install {} from the Play Store, open it once and finish its first run, then run this again", name(package))));
    }
    let mut all: Value = std::fs::read(opt_ins()).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let entry = &mut all[d.key()];
    let mut list: Vec<String> = entry["channels"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if !list.iter().any(|p| p == package) {
        list.push(package.to_string());
    }
    *entry = json!({ "serial": d.serial, "model": format!("{} {}", phone.facts.manufacturer, phone.facts.model), "channels": list });
    paths::write_atomic(&opt_ins(), &serde_json::to_vec_pretty(&all)?)?;
    Ok(format!(
        "{} on {} ({} {}) is strobe's: --mobile runs force-stop it, open and close their own tab and clear the origins they load",
        name(package),
        d.serial,
        phone.facts.manufacturer,
        phone.facts.model
    ))
}

/// What strobe holds on the phone for one launch, given back when dropped: its tab is closed and the
/// channel stopped, then the reverses and the forward go, its command line (if it wrote one), the phone's
/// stay-awake setting and its slot.
pub struct Launch {
    device: Device,
    pub facts: Facts,
    pub package: String,
    /// The tab strobe opened (its DevTools target).
    pub tab: String,
    /// What a command-line file strobe found there holds (kept as it was).
    pub kept_command_line: Option<String>,
    cdp: Arc<Cdp>,
    journal: PathBuf,
    /// The address strobe last loaded in its tab (`reach`), journaled with it.
    url: Mutex<String>,
    reverses: Mutex<Vec<Tunnel>>,
    _forward: Tunnel,
    _written: Option<CommandLine>,
    _awake: StayAwake,
    _slot: PhoneSlot,
}

/// strobe's tab as its journal names it (`<state>/adb/chrome-<phone>.json`): its channel, its DevTools
/// id while it is loaded, and the address strobe loaded in it. A tab Chrome restores after a restart
/// has another id (its tab number), so a tab strobe left behind is also known by that address.
struct Left {
    package: String,
    tab: String,
    url: String,
    /// strobe closed it (and saw it go): only its id names it then. One a killed strobe left open is also
    /// known by its address, which it still shows after Chrome restarted it under another id.
    closed: bool,
}

impl Left {
    /// Whether a listed tab (`/json/list`: its id and address) is this one. A closed tab's address is
    /// never matched: a person may have opened that address since.
    fn is(&self, package: &str, id: &str, url: &str) -> bool {
        // As asked (`https://example.com`) or as the browser keeps it (`https://example.com/`).
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
        self.package == package && (self.tab == id || (!self.closed && !self.url.is_empty() && self.url != "about:blank" && same(&self.url, url)))
    }
}

impl Launch {
    fn write_journal(&self, closed: bool) {
        let url = self.url.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let body = json!({ "owner": crate::host::spawn::owner_stamp(), "serial": self.device.serial, "package": self.package, "tab": self.tab, "url": url, "closed": closed });
        let _ = paths::write_owner_only(&self.journal, body.to_string().as_bytes());
    }
}

impl Drop for Launch {
    /// Closes strobe's tab and waits (up to 2 s) until the browser no longer lists it, then stops the
    /// channel. Chrome saves its tabs a moment after one closes: stopped first, it brought a closed tab
    /// back at its next start. The journal stays, marked closed, with the tab's last address: one that
    /// comes back anyway is known as strobe's and closed by the next launch.
    fn drop(&mut self) {
        if let Some(u) =
            self.cdp.send_timeout("Target.getTargetInfo", json!({ "targetId": self.tab }), None, Duration::from_secs(2)).ok().and_then(|i| i["targetInfo"]["url"].as_str().map(str::to_string))
        {
            *self.url.lock().unwrap_or_else(|e| e.into_inner()) = u;
        }
        let _ = self.cdp.send_timeout("Target.closeTarget", json!({ "targetId": self.tab }), None, Duration::from_secs(5));
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            let listed = self.cdp.send_timeout("Target.getTargets", json!({}), None, Duration::from_secs(2));
            if listed.is_ok_and(|l| !l["targetInfos"].as_array().into_iter().flatten().any(|t| t["targetId"] == self.tab.as_str())) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));
        let _ = self.device.sh(&format!("am force-stop {}", self.package));
        self.write_journal(true);
    }
}

/// Opens strobe's tab in a Chrome channel of `phone` and connects to it: `what` names the run in the
/// phone's slot. The phone is read afresh for each launch (`Device::facts`), and read again once its
/// slot is held: a `timed` run refuses what describes no phone at speed (`validity::phone_timed`) and a
/// hot phone (`Thermal`, which the caller waits out with `cool`, holding nothing). Asleep, the phone is
/// woken and its lock screen dismissed; one locked with a PIN waits `unlock_wait` for a person
/// (`DeviceAsleep`). The facts the launch keeps (`Launch::facts`, a block's `phone`) are its own.
pub fn launch(phone: &Phone, what: &str, timed: bool, unlock_wait: Duration) -> Result<Browser> {
    let d = &phone.device;
    let timed_ok = |f: &Facts| -> Result<()> {
        if timed {
            crate::validity::phone_timed(f)?;
            if let Some(why) = crate::validity::phone_hot(f) {
                return Err(crate::validity::thermal_refusal(&d.serial, &why, "before the block").into());
            }
        }
        Ok(())
    };
    timed_ok(&d.facts()?)?;
    let slot = PhoneSlot::take(d, what, Duration::from_secs(600))?;
    // Another strobe may have held the phone meanwhile: what the block runs on is read now.
    let facts = d.facts()?;
    timed_ok(&facts)?;
    let awake = StayAwake::hold(d)?;
    let facts = wake(d, facts, unlock_wait)?;
    let installed = installed(d)?;
    // A phone's own Chrome is a person's: strobe drives only a channel a person gave it there. An
    // emulator's channels are all strobe may drive.
    let candidates: Vec<&'static str> = if facts.emulator.is_some() { installed.clone() } else { opted_in(d).into_iter().filter(|p| installed.contains(p)).collect() };
    if candidates.is_empty() {
        let given = if facts.emulator.is_some() { String::new() } else { format!(", then `strobe up --mobile={} --channel canary` gives it to strobe (once per phone)", d.serial) };
        return Err(Error::new(3, format!("{} has no Chrome channel strobe may drive", d.serial))
            .hint(format!("install Chrome Canary (or Dev, Beta) on the phone for strobe alone, open it once and finish its first run{given}; strobe never drives a phone's own Chrome")));
    }
    let (written, kept) = if facts.debuggable {
        match CommandLine::hold(d, FIRST_RUN_OFF)? {
            Held::Written(c) => (Some(c), None),
            Held::Kept(t) => (None, Some(t)),
        }
    } else {
        (None, None)
    };
    let journal = paths::sub("adb").join(format!("chrome-{}.json", d.key()));
    // The last tab strobe opened on this phone, named in its journal (closed, or a killed strobe's): strobe's
    // to close if it is still there, never a person's.
    let left: Option<Left> =
        std::fs::read_to_string(&journal).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|v| v["closed"] == true || !v["owner"].as_str().is_some_and(adb::alive)).and_then(|v| {
            Some(Left { package: v["package"].as_str()?.to_string(), tab: v["tab"].as_str()?.to_string(), url: v["url"].as_str().unwrap_or("").to_string(), closed: v["closed"] == true })
        });
    let (package, forward, stale) = channel(d, &installed, &candidates, left.as_ref(), kept.is_some(), written.is_some())?;
    let port = forward.port().ok_or_else(|| Error::msg("internal: the DevTools forward has no port"))?;
    let version: Value = serde_json::from_str(&chromium::http_get(&format!("127.0.0.1:{port}"), "/json/version")?)?;
    let path = version["webSocketDebuggerUrl"].as_str().and_then(|u| u.strip_prefix("ws://")).and_then(|r| r.find('/').map(|i| r[i..].to_string())).unwrap_or_else(|| "/devtools/browser".into());
    let ws = format!("ws://127.0.0.1:{port}{path}");
    let cdp = Cdp::connect(&ws)?;
    let product = cdp.send("Browser.getVersion", json!({}), None)?["product"].as_str().unwrap_or("").to_string();
    // A restored tab is not loaded until shown, and only a loaded one closes.
    for t in &stale {
        if cdp.send("Target.activateTarget", json!({ "targetId": t }), None).is_ok() {
            let _ = cdp.send("Target.closeTarget", json!({ "targetId": t }), None);
        }
    }
    let tab = cdp.send("Target.createTarget", json!({ "url": "about:blank" }), None)?["targetId"].as_str().unwrap_or("").to_string();
    if tab.is_empty() {
        return Err(Error::msg(format!("{} ({package}) opened no tab for strobe", d.serial)));
    }
    let _ = cdp.send("Target.activateTarget", json!({ "targetId": tab }), None);
    let body = json!({ "owner": crate::host::spawn::owner_stamp(), "serial": d.serial, "package": package, "tab": tab, "url": "about:blank" });
    paths::write_owner_only(&journal, body.to_string().as_bytes()).map_err(|e| Error::msg(format!("could not journal strobe's tab on {}: {e}", d.serial)))?;
    // The screen in CSS pixels at its density: the page's own size is read once it loads.
    let window = facts.display.as_ref().filter(|s| s.density > 0).map_or_else(Window::default, |s| {
        let scale = s.density as f64 / 160.0;
        Window { x: 0, y: 0, width: (s.width as f64 / scale).round() as u32, height: (s.height as f64 / scale).round() as u32, scale: Some(scale) }
    });
    let launch = Launch {
        device: d.clone(),
        facts,
        package: package.clone(),
        tab,
        kept_command_line: kept,
        cdp: cdp.clone(),
        journal,
        url: Mutex::new("about:blank".into()),
        reverses: Mutex::new(Vec::new()),
        _forward: forward,
        _written: written,
        _awake: awake,
        _slot: slot,
    };
    Ok(Browser {
        cdp,
        ws,
        child: None,
        pid: 0,
        profile: None,
        fresh: false,
        exe: PathBuf::from(&package),
        version: format!("{product} ({})", name(&package)),
        window,
        embedded: false,
        // `navigator.webdriver` false in strobe's tab, as on a desktop launch.
        as_person: true,
        phone: Some(Box::new(launch)),
    })
}

impl Launch {
    /// Before strobe loads `url` in its tab: the address is journaled with the tab (a strobe killed before
    /// its tab closed leaves it behind there), and reached (`reach`).
    pub fn load(&self, url: &str) -> Result<()> {
        *self.url.lock().unwrap_or_else(|e| e.into_inner()) = url.to_string();
        self.write_journal(false);
        self.reach(url)
    }

    /// A page on the phone reaches a server on this computer's loopback (`http://127.0.0.1:<port>/…`, a
    /// `serve` or a fixture): the same port on the phone's loopback, reversed to this computer's, so the
    /// page loads the same address. Nothing for any other address.
    pub fn reach(&self, url: &str) -> Result<()> {
        let Some(port) = loopback_port(url) else { return Ok(()) };
        let mut held = self.reverses.lock().unwrap_or_else(|e| e.into_inner());
        let spec = format!("tcp:{port}");
        if held.iter().any(|t| t.listen() == spec) {
            return Ok(());
        }
        held.push(self.device.reverse(&spec, &spec)?);
        Ok(())
    }

    /// `Thermal` when the phone got hot during a timed block: the block describes a throttling phone.
    pub fn still_cool(&self) -> Result<()> {
        let f = self.device.facts()?;
        match crate::validity::phone_hot(&f) {
            Some(why) => Err(crate::validity::thermal_refusal(&self.device.serial, &why, "after the block's runs").into()),
            None => Ok(()),
        }
    }

    /// The phone's hardware serial (else its serial), for its own speed reference.
    pub fn key(&self) -> String {
        self.facts.hardware_serial.clone().unwrap_or_else(|| self.device.serial.clone())
    }

    /// What a recording and a block keep of the phone: what it is, how it is connected, the channel, its
    /// state, and the page's screen as the page sees it (`screen`).
    pub fn meta(&self, screen: Value) -> Value {
        let f = &self.facts;
        json!({
            "serial": f.serial, "manufacturer": f.manufacturer, "model": f.model, "soc": f.soc, "android": f.android, "sdk": f.sdk, "fingerprint": f.fingerprint,
            "build_type": f.build_type, "emulator": f.emulator, "abi": f.abi, "transport": f.transport, "display": f.display, "battery": f.battery, "thermal": f.thermal,
            "package": self.package, "channel": name(&self.package), "command_line_kept": self.kept_command_line, "screen": screen,
        })
    }
}

/// The page's screen as it sees it: its viewport (`innerWidth`, `innerHeight`), the device pixels per CSS
/// pixel and the screen (`screen.width`, `screen.height`): the phone's own, never emulated.
pub fn screen(page: &chromium::Page) -> Value {
    page.eval("({ viewport: [innerWidth, innerHeight], dpr: devicePixelRatio, screen: [screen.width, screen.height] })").unwrap_or(Value::Null)
}

/// Clears what the page's origin left in the browser before strobe loads it: its storage (cookies, local
/// and session storage, IndexedDB, caches, service workers) and the HTTP cache, which a fresh profile has
/// empty on a desktop. The channel holds no person's tabs (`LiveChannel`).
pub fn clear_origin(page: &chromium::Page, url: &str) -> Result<()> {
    page.send("Network.clearBrowserCache", json!({}))?;
    if let Some(origin) = origin_of(url) {
        page.send("Storage.clearDataForOrigin", json!({ "origin": origin, "storageTypes": "all" }))?;
    }
    Ok(())
}

/// `scheme://host[:port]` of an http(s) URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    Some(format!("{scheme}://{}", rest.split(['/', '?', '#']).next()?))
}

/// The port of an http(s) or ws URL on this computer's loopback, given explicitly (a phone's apps may
/// not listen below 1024).
fn loopback_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https" | "ws" | "wss") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?;
    let (name, port) = host.rsplit_once(':')?;
    matches!(name, "127.0.0.1" | "localhost" | "[::1]").then(|| port.parse().ok()).flatten()
}

/// A local page (`file://…`) as the phone loads it: its folder served on this computer's loopback
/// (`http::serve_folder`), the same path below it. Any other address as it is.
pub fn served(url: &str) -> Result<String> {
    let Some(rest) = url.strip_prefix("file://") else { return Ok(url.to_string()) };
    let (path, tail) = match rest.find(['?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mut bytes = Vec::new();
    let mut it = path.bytes();
    while let Some(b) = it.next() {
        let hex = |c: Option<u8>| c.and_then(|c| (c as char).to_digit(16));
        match b {
            b'%' => match (hex(it.next()), hex(it.next())) {
                (Some(h), Some(l)) => bytes.push((h * 16 + l) as u8),
                _ => return Err(Error::usage(format!("{url}: a bad escape in its path"))),
            },
            _ => bytes.push(b),
        }
    }
    let decoded = String::from_utf8_lossy(&bytes).to_string();
    // `file:///C:/…` on Windows.
    let decoded = if cfg!(windows) && decoded.len() > 2 && decoded.as_bytes()[2] == b':' { decoded[1..].to_string() } else { decoded };
    let file = std::path::Path::new(&decoded);
    let (Some(dir), Some(leaf)) = (file.parent(), file.file_name()) else { return Err(Error::usage(format!("{url}: no file to serve"))) };
    let port = crate::host::http::serve_folder(dir).map_err(|e| Error::msg(format!("could not serve {} for the phone: {e}", dir.display())))?;
    let leaf: String = leaf.to_string_lossy().bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    Ok(format!("http://127.0.0.1:{port}/{leaf}{tail}"))
}

/// The Chrome channels installed on the phone, in `CHANNELS` order.
fn installed(d: &Device) -> Result<Vec<&'static str>> {
    let list = d.sh("pm list packages")?;
    let have: Vec<&str> = list.lines().filter_map(|l| l.trim().strip_prefix("package:")).collect();
    Ok(CHANNELS.iter().map(|(p, _)| *p).filter(|p| have.contains(p)).collect())
}

fn running(d: &Device, package: &str) -> bool {
    d.shell(&format!("pidof {package}")).is_ok_and(|o| o.ok() && !o.stdout_text().trim().is_empty())
}

/// Starts a channel as its launcher does: it restores its own tabs and opens none.
fn start(d: &Device, package: &str) -> Result<()> {
    d.sh(&format!("am start -W -a android.intent.action.MAIN -c android.intent.category.LAUNCHER -n {package}/{ACTIVITY}")).map(drop)
}

fn stop(d: &Device, package: &str) {
    let _ = d.sh(&format!("am force-stop {package}"));
}

/// A forward to the DevTools socket once `package` answers on it, within `wait`. None when the socket
/// never opened (a channel still on its first-run screens) or another channel holds it.
/// Who answers on the DevTools socket.
enum Socket {
    /// The channel asked for, through this forward.
    Ours(Tunnel),
    /// Nobody, within the wait: a channel still on its first-run screens.
    Closed,
    /// Another app (its package, as it names itself): only one may hold the socket's name.
    Other(String),
}

/// A forward to the DevTools socket once `package` answers on it (its `/json/version` names it as its
/// `Android-Package`), within `wait`.
fn devtools(d: &Device, package: &str, wait: Duration) -> Result<Socket> {
    let until = Instant::now() + wait;
    loop {
        let open = d.shell(&format!("grep -q '@{SOCKET}$' /proc/net/unix")).is_ok_and(|o| o.ok());
        if open {
            let fwd = d.forward("tcp:0", &format!("localabstract:{SOCKET}"))?;
            let port = fwd.port().unwrap_or(0);
            if let Ok(v) = chromium::http_get(&format!("127.0.0.1:{port}"), "/json/version").and_then(|b| serde_json::from_str::<Value>(&b).map_err(Error::from)) {
                return Ok(match v["Android-Package"].as_str() {
                    Some(p) if p == package => Socket::Ours(fwd),
                    Some(p) => Socket::Other(name(p).to_string()),
                    None => Socket::Other(format!("an app that names no package ({})", v["Browser"].as_str().unwrap_or("a browser"))),
                });
            }
        }
        if Instant::now() >= until {
            return Ok(Socket::Closed);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A tab's address that holds nothing of a person's: a blank page or a new tab page. An empty address
/// is not blank: a tab Chrome restored and has not given its address yet may be anything.
fn blank(url: &str) -> bool {
    url == "about:blank" || url.starts_with("chrome://newtab") || url.starts_with("chrome-native://newtab")
}

/// The tabs of `package` behind `fwd` (`/json/list`): the origins of those a person opened (page targets
/// that are not blank and not strobe's), and the ids of strobe's own that are still there (`left`). A tab
/// with no address yet is read again, for up to 3 s after a start; one that keeps none is a person's.
fn tabs(fwd: &Tunnel, package: &str, left: Option<&Left>) -> Result<(Vec<String>, Vec<String>)> {
    let until = Instant::now() + Duration::from_secs(3);
    let list = loop {
        let body = chromium::http_get(&format!("127.0.0.1:{}", fwd.port().unwrap_or(0)), "/json/list")?;
        let list: Value = serde_json::from_str(&body)?;
        let unnamed = list.as_array().into_iter().flatten().any(|t| t["type"] == "page" && t["url"].as_str().is_none_or(str::is_empty));
        if !unnamed || Instant::now() >= until {
            break list;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let (mut theirs, mut ours) = (Vec::new(), Vec::new());
    for t in list.as_array().into_iter().flatten().filter(|t| t["type"] == "page") {
        let (id, url) = (t["id"].as_str().unwrap_or(""), t["url"].as_str().unwrap_or(""));
        if left.is_some_and(|l| l.is(package, id, url)) {
            ours.push(id.to_string());
        } else if url.is_empty() {
            theirs.push("a tab whose address Chrome gave no reader".to_string());
        } else if !blank(url) {
            theirs.push(origin_of(url).unwrap_or_else(|| url.split(['?', '#']).next().unwrap_or(url).to_string()));
        }
    }
    Ok((theirs, ours))
}

/// Exit 3 for a channel that runs and holds the DevTools socket, which strobe will not stop: `why` says
/// whose it is.
fn holds_socket(d: &Device, package: &str, why: &str, wanted: &[&str]) -> Error {
    Error::new(
        3,
        format!(
            "{} runs on {} ({why}) and holds Chrome's DevTools socket, which {} answers on: strobe stops no channel it may not drive",
            name(package),
            d.serial,
            wanted.iter().map(|p| name(p)).collect::<Vec<_>>().join(", ")
        ),
    )
    .hint(format!("close {} on the phone (Recents, swipe it away), then run again", name(package)))
}

/// The channel strobe drives, of `candidates` (the installed channels it may drive), freshly started,
/// with its DevTools forward and the ids of strobe's own tabs an interrupted or killed strobe left there
/// (to close). Only one channel holds the DevTools socket at a time, so one that runs is read first and
/// any other is refused while it runs: a channel strobe may not drive (exit 3, naming it), or one of its
/// candidates with a person's tabs (exit 3: it holds the socket). One that does not run is started as
/// its launcher starts it (restoring its tabs, opening none) to read its tabs: with a person's it is
/// refused `LiveChannel` and stopped again. One whose socket never opens is on its first-run screens
/// (exit 3).
fn channel(d: &Device, installed: &[&'static str], candidates: &[&'static str], left: Option<&Left>, kept_command_line: bool, wrote_command_line: bool) -> Result<(String, Tunnel, Vec<String>)> {
    let live: Vec<&str> = installed.iter().copied().filter(|p| running(d, p)).collect();
    if live.len() > 1 {
        return Err(Error::new(3, format!("{} run on {} at once, and DevTools reaches only one of them", live.iter().map(|p| name(p)).collect::<Vec<_>>().join(" and "), d.serial))
            .hint("close all but one of them on the phone (Recents, swipe them away)"));
    }
    if let Some(l) = live.first().filter(|l| !candidates.contains(l)) {
        let why = if *l == STABLE { "a phone's own Chrome is a person's" } else { "it is not one strobe was given on this phone" };
        return Err(holds_socket(d, l, why, candidates));
    }
    let order: Vec<&str> = live.iter().copied().chain(candidates.iter().copied().filter(|p| !live.contains(p))).collect();
    let (mut people, mut first_run): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for package in order {
        let was_running = live.contains(&package);
        if !was_running {
            start(d, package)?;
        }
        let mut socket = devtools(d, package, Duration::from_secs(if was_running { 5 } else { 30 }))?;
        // A debuggable device's Chrome that ran before strobe wrote its command line (an emulator starts
        // Chrome at boot) is still on its first-run screens: started again, it reads the command line.
        if matches!(socket, Socket::Closed) && was_running && wrote_command_line {
            stop(d, package);
            start(d, package)?;
            socket = devtools(d, package, Duration::from_secs(30))?;
        }
        let fwd = match socket {
            Socket::Ours(fwd) => fwd,
            Socket::Other(holder) => {
                if !was_running {
                    stop(d, package);
                }
                return Err(Error::new(3, format!("{holder} holds Chrome's DevTools socket on {}, so {} cannot answer on it", d.serial, name(package)))
                    .hint(format!("close {holder} on the phone, then run again")));
            }
            Socket::Closed => {
                if !was_running {
                    stop(d, package);
                }
                first_run.push(name(package).to_string());
                // Another channel cannot open the socket while this one runs.
                if was_running {
                    break;
                }
                continue;
            }
        };
        let (theirs, ours) = tabs(&fwd, package, left)?;
        if !theirs.is_empty() {
            drop(fwd);
            let held = format!("{} tab{} a person opened ({})", theirs.len(), if theirs.len() == 1 { "" } else { "s" }, theirs[0]);
            if was_running {
                return Err(Error::new(3, format!("{} runs on {} with {held} and holds Chrome's DevTools socket: strobe stops no channel that holds a person's tabs", name(package), d.serial))
                    .hint(format!("close {}'s tabs, or close it on the phone, then run again", name(package))));
            }
            stop(d, package);
            people.push(format!("{} holds {held}", name(package)));
            continue;
        }
        if !was_running {
            return Ok((package.to_string(), fwd, ours));
        }
        // Blank or strobe's own tabs: a fresh start, as every block's browser is.
        drop(fwd);
        stop(d, package);
        start(d, package)?;
        return match devtools(d, package, Duration::from_secs(30))? {
            Socket::Ours(fwd) => {
                let ours = tabs(&fwd, package, left)?.1;
                Ok((package.to_string(), fwd, ours))
            }
            Socket::Other(holder) => {
                Err(Error::new(3, format!("{holder} took Chrome's DevTools socket on {} while strobe restarted {}", d.serial, name(package))).hint(format!("close {holder} on the phone")))
            }
            Socket::Closed => Err(Error::new(3, format!("{} on {} did not open its DevTools socket after a restart", name(package), d.serial)).hint("open it on the phone once, then run again")),
        };
    }
    if !people.is_empty() {
        return Err(Refusal::new(ValidatorId::LiveChannel, format!("every Chrome channel strobe may drive on {} holds tabs a person opened: {}", d.serial, people.join("; ")))
            .fix("strobe drives a channel no one browses: close those tabs on the phone, or give strobe a channel of its own (install Chrome Canary, open it once, then strobe up --mobile=<serial> --channel canary)")
            .into());
    }
    let kept = if kept_command_line { format!(" (a {} is kept as it was)", adb::CHROME_COMMAND_LINE) } else { String::new() };
    Err(Error::new(3, format!("{} on {} never opened its DevTools socket: still on its first-run screens{kept}", first_run.join(", "), d.serial))
        .hint("open that Chrome on the phone once and finish its first run, with USB debugging on"))
}

/// The wait for a hot phone (`validity::phone_hot`) to cool before a timed block, holding nothing of
/// strobe's (the caller gave its slots back): read every 15 s for up to 3 minutes. Whether it cooled.
pub fn cool(d: &Device) -> Result<bool> {
    let until = Instant::now() + COOLDOWN;
    let mut said = false;
    loop {
        let Some(why) = crate::validity::phone_hot(&d.facts()?) else { return Ok(true) };
        if Instant::now() >= until {
            return Ok(false);
        }
        if crate::app::stop_requested() {
            return Err(crate::app::stop_error());
        }
        if !said {
            crate::app::progress(format!("  {} is hot ({why}): waiting up to 3 minutes for it to cool", d.serial));
            said = true;
        }
        std::thread::sleep(Duration::from_secs(15));
    }
}

/// A phone whose screen is off or shows its lock screen: woken, and a lock screen without a PIN
/// dismissed; one with a PIN, pattern or password waits `wait` for a person (`DeviceAsleep`).
fn wake(d: &Device, facts: Facts, wait: Duration) -> Result<Facts> {
    let asleep = |f: &Facts| f.screen_on == Some(false) || f.keyguard == Some(true);
    if !asleep(&facts) {
        return Ok(facts);
    }
    let secure = facts.secure_lock == Some(true);
    let _ = d.sh("input keyevent KEYCODE_WAKEUP");
    if !secure {
        let _ = d.sh("wm dismiss-keyguard");
    }
    if secure {
        crate::app::progress(format!("  {} is locked with a PIN: waiting{} for a person to unlock it", d.serial, if wait.is_zero() { String::new() } else { format!(" up to {} s", wait.as_secs()) }));
    }
    let until = Instant::now() + wait.max(Duration::from_secs(3));
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let f = d.facts()?;
        if !asleep(&f) {
            return Ok(f);
        }
        if crate::app::stop_requested() {
            return Err(crate::app::stop_error());
        }
        if Instant::now() >= until {
            let what = if f.screen_on == Some(false) { "its screen stays off" } else { "it shows its lock screen" };
            let fix = if secure {
                "unlock the phone (its PIN, pattern or password needs a person); a bench waits for that with --unlock-wait"
            } else {
                "wake the phone and dismiss its lock screen (or set its screen lock to None for a phone kept for strobe)"
            };
            return Err(Refusal::new(ValidatorId::DeviceAsleep, format!("{} is asleep: {what}", d.serial)).fix(fix).into());
        }
    }
}
