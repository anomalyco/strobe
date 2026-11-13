//! Apple Web Inspector Remote (`com.apple.webinspector`) over an iOS lockdownd session: listing
//! inspectable Safari tabs and WKWebViews, read-only page attach (`WIRAutomaticallyPause = false`),
//! `Target` multiplexing (`Target.targetCreated`, `didCommitProvisionalTarget`,
//! `sendMessageToTarget` / `dispatchMessageFromTarget` with at most 8 commands in flight),
//! `DebuggerAttached` refusal, and an unconditional `_rpc_forwardDidClose:` on detach and `Drop`.

use crate::host;
use crate::host::ios::{self, Connected, Device, Lockdown, Service};
use crate::model::{Error, Result};
use base64::Engine as _;
use plist::{Dictionary, Value as Plist};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

const WIR_SERVICE: &str = "com.apple.webinspector";
pub const SAFARI_BUNDLE: &str = "com.apple.mobilesafari";
/// Maximum inner WebKit Inspector commands in flight on one page target at once.
pub const MAX_IN_FLIGHT: usize = 8;

/// One inspectable page reported by `com.apple.webinspector`.
#[derive(Clone, Debug, Serialize)]
pub struct InspectablePage {
    /// Canonical strobe handle (`wir:<short_udid>/<app>/<page_id>`).
    pub handle: String,
    pub udid: String,
    pub short_udid: String,
    /// WIR routing identifier (`PID:123` or bundle id) used in `_rpc_forward*` messages.
    pub app_id: String,
    pub bundle_id: String,
    pub app_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_pid: Option<u32>,
    pub app_active: bool,
    pub page_id: u64,
    pub page_type: String,
    pub title: String,
    pub url: String,
    pub debugger_attached: bool,
    #[serde(skip)]
    local_debugger: bool,
}

#[derive(Clone, Debug)]
struct AppEntry {
    app_id: String,
    bundle_id: String,
    name: String,
    pid: Option<u32>,
    active: bool,
    proxy: bool,
}

/// One unpacked WebKit Inspector event from the attached page target.
#[derive(Clone, Debug)]
pub struct WirEvent {
    pub host_ns: u64,
    pub method: String,
    pub params: Value,
}

/// One console or exception entry collected from WebKit's `Console` domain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub at_ms: f64,
    pub source: String,
    pub level: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default = "one_u64")]
    pub repeat_count: u64,
}

fn one_u64() -> u64 {
    1
}

/// Generates an uppercase v4 UUID from strobe's OS CSPRNG helper.
fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn send_rpc(svc: &mut Service, selector: &str, args: Dictionary) -> Result<()> {
    let mut m = Dictionary::new();
    m.insert("__selector".into(), selector.into());
    m.insert("__argument".into(), Plist::Dictionary(args));
    svc.send(&Plist::Dictionary(m), true)
}

fn plist_u64(v: &Plist) -> Option<u64> {
    v.as_unsigned_integer().or_else(|| v.as_signed_integer().and_then(|i| u64::try_from(i).ok()))
}

fn plist_active(v: Option<&Plist>) -> bool {
    v.is_some_and(|x| x.as_boolean().unwrap_or_else(|| plist_u64(x).is_some_and(|n| n > 0)))
}

fn parse_app_pid(app_id: &str) -> Option<u32> {
    app_id.strip_prefix("PID:").and_then(|s| s.trim().parse::<u32>().ok())
}

fn parse_app(fallback_key: &str, d: &Dictionary) -> Option<AppEntry> {
    let app_id = d.get("WIRApplicationIdentifierKey").and_then(Plist::as_string).unwrap_or(fallback_key).to_string();
    if app_id.is_empty() {
        return None;
    }
    let bundle_id = d.get("WIRApplicationBundleIdentifierKey").and_then(Plist::as_string).unwrap_or(&app_id).to_string();
    let raw_name = d.get("WIRApplicationNameKey").and_then(Plist::as_string).unwrap_or("");
    let name = if bundle_id == SAFARI_BUNDLE {
        "Safari".to_string()
    } else if !raw_name.is_empty() {
        raw_name.to_string()
    } else {
        bundle_id.clone()
    };
    let proxy = d.get("WIRIsApplicationProxyKey").and_then(Plist::as_boolean).unwrap_or(false);
    let active = plist_active(d.get("WIRIsApplicationActiveKey"));
    Some(AppEntry { pid: parse_app_pid(&app_id), app_id, bundle_id, name, active, proxy })
}

fn app_handle_segment(bundle_id: &str) -> &str {
    if bundle_id == SAFARI_BUNDLE { "safari" } else { bundle_id }
}

fn is_web_page_type(ty: &str) -> bool {
    matches!(ty, "WIRTypeWebPage" | "WIRTypePage" | "WIRTypeWeb")
}

fn parse_listing(udid: &str, conn_id: &str, app: &AppEntry, listing: &Dictionary) -> Vec<InspectablePage> {
    let short = ios::short_udid(udid);
    let seg = app_handle_segment(&app.bundle_id);
    let mut out = Vec::new();
    for (k, v) in listing {
        let Some(d) = v.as_dictionary() else { continue };
        let page_type = d.get("WIRTypeKey").and_then(Plist::as_string).unwrap_or("WIRTypeWebPage");
        if !is_web_page_type(page_type) {
            continue;
        }
        let Some(page_id) = d.get("WIRPageIdentifierKey").and_then(plist_u64).or_else(|| k.parse::<u64>().ok()) else { continue };
        let title = d.get("WIRTitleKey").and_then(Plist::as_string).unwrap_or("").to_string();
        let raw_url = d.get("WIRURLKey").and_then(Plist::as_string).unwrap_or("");
        let url = ios::logs::scrub_url(raw_url, false);
        let local_dbg = d.get("WIRHasLocalDebuggerKey").and_then(Plist::as_boolean).unwrap_or(false);
        let other_conn = d.get("WIRConnectionIdentifierKey").and_then(Plist::as_string).filter(|c| !c.is_empty() && *c != conn_id).is_some();
        out.push(InspectablePage {
            handle: format!("wir:{short}/{seg}/{page_id}"),
            udid: udid.to_string(),
            short_udid: short.clone(),
            app_id: app.app_id.clone(),
            bundle_id: app.bundle_id.clone(),
            app_name: app.name.clone(),
            app_pid: app.pid,
            app_active: app.active,
            page_id,
            page_type: page_type.to_string(),
            title,
            url,
            debugger_attached: local_dbg || other_conn,
            local_debugger: local_dbg,
        });
    }
    out.sort_by_key(|p| p.page_id);
    out
}

fn start_wir(l: &mut Lockdown) -> Result<Service> {
    match l.start_service(WIR_SERVICE) {
        Ok(s) => Ok(s),
        Err(e) if e.message.contains("PasswordProtected") => Err(Error::refused("DeviceAsleep", "the phone is locked, so Web Inspector cannot start", "unlock the phone and run again")),
        Err(e) if e.message.contains("InvalidService") => {
            Err(Error::msg(format!("{}: Web Inspector is not available over lockdownd on this iOS build", e.message)).hint("ensure Settings › Apps › Safari › Advanced › Web Inspector is turned on"))
        }
        Err(e) => Err(e),
    }
}

/// Discovers connected inspectable apps and their open web pages on an open `webinspector` service.
fn discover_pages(svc: &mut Service, udid: &str, conn_id: &str, budget: Duration, safari_fast: bool) -> Result<(Vec<AppEntry>, Vec<InspectablePage>)> {
    let overall = Instant::now() + budget;
    svc.set_deadline(Some(overall));
    let mut id_arg = Dictionary::new();
    id_arg.insert("WIRConnectionIdentifierKey".into(), conn_id.into());
    send_rpc(svc, "_rpc_reportIdentifier:", id_arg.clone())?;
    send_rpc(svc, "_rpc_getConnectedApplications:", id_arg)?;

    let mut apps: HashMap<String, AppEntry> = HashMap::new();
    let mut got_app_list = false;
    let app_wait = overall.min(Instant::now() + Duration::from_millis(1500));
    while Instant::now() < app_wait {
        svc.set_deadline(Some(app_wait));
        let Ok(msg) = svc.recv() else { break };
        let sel = msg.get("__selector").and_then(Plist::as_string).unwrap_or("");
        let Some(arg) = msg.get("__argument").and_then(Plist::as_dictionary) else { continue };
        match sel {
            "_rpc_reportConnectedApplicationList:" => {
                if let Some(dict) = arg.get("WIRApplicationDictionaryKey").and_then(Plist::as_dictionary) {
                    for (k, v) in dict {
                        if let Some(d) = v.as_dictionary()
                            && let Some(a) = parse_app(k, d)
                        {
                            apps.insert(a.app_id.clone(), a);
                        }
                    }
                }
                got_app_list = true;
                break;
            }
            "_rpc_applicationConnected:" | "_rpc_applicationUpdated:" => {
                if let Some(a) = parse_app("", arg) {
                    apps.insert(a.app_id.clone(), a);
                }
            }
            _ => {}
        }
    }
    if !got_app_list && apps.is_empty() {
        return Err(Error::msg("Web Inspector started on the phone, but sent no application list").hint("turn on Settings › Apps › Safari › Advanced › Web Inspector on the phone, then run again"));
    }

    svc.set_deadline(Some(overall));
    let inspectable_apps: Vec<AppEntry> = apps.values().filter(|a| !a.proxy).cloned().collect();
    let safari_app_id = inspectable_apps.iter().find(|a| a.bundle_id == SAFARI_BUNDLE).map(|a| a.app_id.clone());
    let mut pending: HashSet<String> = HashSet::new();
    for a in &inspectable_apps {
        let mut f = Dictionary::new();
        f.insert("WIRConnectionIdentifierKey".into(), conn_id.into());
        f.insert("WIRApplicationIdentifierKey".into(), a.app_id.as_str().into());
        if send_rpc(svc, "_rpc_forwardGetListing:", f).is_ok() {
            pending.insert(a.app_id.clone());
        }
    }

    let mut by_app: HashMap<String, Vec<InspectablePage>> = HashMap::new();
    let list_wait = overall.min(Instant::now() + Duration::from_millis(900));
    while !pending.is_empty() && Instant::now() < list_wait {
        svc.set_deadline(Some(list_wait));
        let Ok(msg) = svc.recv() else { break };
        let sel = msg.get("__selector").and_then(Plist::as_string).unwrap_or("");
        let Some(arg) = msg.get("__argument").and_then(Plist::as_dictionary) else { continue };
        match sel {
            "_rpc_applicationSentListing:" => {
                let Some(aid) = arg.get("WIRApplicationIdentifierKey").and_then(Plist::as_string) else { continue };
                if let Some(app) = apps.get(aid)
                    && let Some(listing) = arg.get("WIRListingKey").and_then(Plist::as_dictionary)
                {
                    by_app.insert(aid.to_string(), parse_listing(udid, conn_id, app, listing));
                }
                pending.remove(aid);
                // Suspended background WKWebView apps never answer `_rpc_forwardGetListing:`: once
                // MobileSafari has reported non-empty tabs and the caller asked for Safari (or default
                // `wir:` when Safari has tabs), do not block 900 ms on background apps.
                if safari_fast
                    && let Some(sid) = &safari_app_id
                    && !pending.contains(sid)
                    && by_app.get(sid).is_some_and(|p| !p.is_empty())
                {
                    break;
                }
            }
            "_rpc_applicationConnected:" | "_rpc_applicationUpdated:" => {
                if let Some(a) = parse_app("", arg)
                    && !a.proxy
                    && !apps.contains_key(&a.app_id)
                {
                    svc.set_deadline(Some(overall));
                    let mut f = Dictionary::new();
                    f.insert("WIRConnectionIdentifierKey".into(), conn_id.into());
                    f.insert("WIRApplicationIdentifierKey".into(), a.app_id.as_str().into());
                    if send_rpc(svc, "_rpc_forwardGetListing:", f).is_ok() {
                        pending.insert(a.app_id.clone());
                    }
                    apps.insert(a.app_id.clone(), a);
                }
            }
            _ => {}
        }
    }
    svc.set_deadline(None);

    let mut pages: Vec<InspectablePage> = by_app.into_values().flatten().collect();
    pages.sort_by(|a, b| {
        (b.bundle_id == SAFARI_BUNDLE)
            .cmp(&(a.bundle_id == SAFARI_BUNDLE))
            .then_with(|| b.app_active.cmp(&a.app_active))
            .then_with(|| a.bundle_id.cmp(&b.bundle_id))
            .then_with(|| a.page_id.cmp(&b.page_id))
    });
    let mut app_list: Vec<AppEntry> = apps.into_values().filter(|a| !a.proxy).collect();
    app_list.sort_by(|a, b| (b.bundle_id == SAFARI_BUNDLE).cmp(&(a.bundle_id == SAFARI_BUNDLE)).then_with(|| a.bundle_id.cmp(&b.bundle_id)));
    Ok((app_list, pages))
}

/// Lists inspectable pages on a USB-connected, unlocked phone for `strobe status` within a shared `deadline`.
pub fn list_pages_for_udid_until(udid: &str, deadline: Instant) -> Result<Vec<InspectablePage>> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Ok(Vec::new());
    }
    let dl = Some(deadline);
    let devices = ios::list(dl)?;
    let Some(dev) = devices.into_iter().find(|d| d.udid == udid && d.connection == ios::Connection::Usb) else {
        return Ok(Vec::new());
    };
    let mut l = ios::session_for(&dev, dl)?;
    let mut svc = start_wir(&mut l)?;
    let conn_id = uuid_v4();
    let res = discover_pages(&mut svc, udid, &conn_id, deadline.saturating_duration_since(Instant::now()), false).map(|(_, pages)| pages);
    svc.close();
    res
}

fn format_choices(pages: &[InspectablePage]) -> String {
    pages
        .iter()
        .take(8)
        .map(|p| {
            let what = if !p.url.is_empty() {
                p.url.as_str()
            } else if !p.title.is_empty() {
                p.title.as_str()
            } else {
                "(blank)"
            };
            format!("{} ({what})", p.handle)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_blank_url(u: &str) -> bool {
    u.is_empty() || u == "about:blank" || u.starts_with("favorites://") || u.starts_with("safari-resource:")
}

fn choose_page(pages: &[InspectablePage], apps: &[AppEntry], page_sel: &str) -> Result<InspectablePage> {
    if pages.is_empty() {
        if apps.is_empty() {
            return Err(Error::usage("no inspectable app is running on the phone").hint("open a page in Safari on the phone, and check Settings › Apps › Safari › Advanced › Web Inspector is ON"));
        }
        return Err(Error::usage("Safari is running on the phone, but reports no inspectable tab")
            .hint("open a regular tab in Safari on the phone, unlock the screen, and check Settings › Apps › Safari › Advanced › Web Inspector is ON"));
    }

    let sel = page_sel.trim().trim_matches('/');
    if sel.is_empty() {
        let safari: Vec<&InspectablePage> = pages.iter().filter(|p| p.bundle_id == SAFARI_BUNDLE).collect();
        let pool: Vec<&InspectablePage> = if !safari.is_empty() {
            safari
        } else {
            let active: Vec<&InspectablePage> = pages.iter().filter(|p| p.app_active).collect();
            if active.is_empty() { pages.iter().collect() } else { active }
        };
        let non_blank: Vec<&InspectablePage> = pool.iter().copied().filter(|p| !is_blank_url(&p.url)).collect();
        let candidates = if non_blank.is_empty() { pool } else { non_blank };
        return match candidates.as_slice() {
            [only] => Ok((*only).clone()),
            _ => Err(Error::usage(format!("{} inspectable tabs are open on the phone; bare wir: refuses to guess on a personal device", candidates.len()))
                .hint(format!("name the tab to inspect: {}", format_choices(&candidates.into_iter().cloned().collect::<Vec<_>>())))),
        };
    }

    // Numeric page id (`wir:1`).
    if let Ok(pid) = sel.parse::<u64>() {
        let hits: Vec<&InspectablePage> = pages.iter().filter(|p| p.page_id == pid).collect();
        if let [only] = hits.as_slice() {
            return Ok((*only).clone());
        }
        if let Some(safari_hit) = hits.iter().copied().find(|p| p.bundle_id == SAFARI_BUNDLE) {
            return Ok(safari_hit.clone());
        }
        if !hits.is_empty() {
            return Err(Error::usage(format!("page {pid} exists in multiple apps")).hint(format!("choose one: {}", format_choices(pages))));
        }
    }

    // `<app>/<page_id>` (`wir:safari/1`, `wir:com.example.app/1`).
    if let Some((app_q, page_q)) = sel.rsplit_once('/')
        && let Ok(pid) = page_q.parse::<u64>()
    {
        let aq = app_q.to_ascii_lowercase();
        let hits: Vec<&InspectablePage> = pages
            .iter()
            .filter(|p| {
                p.page_id == pid
                    && (app_handle_segment(&p.bundle_id).eq_ignore_ascii_case(&aq)
                        || p.bundle_id.eq_ignore_ascii_case(&aq)
                        || p.app_name.eq_ignore_ascii_case(&aq)
                        || p.app_id.eq_ignore_ascii_case(&aq))
            })
            .collect();
        if let [only] = hits.as_slice() {
            return Ok((*only).clone());
        }
    }

    // Substring match against handle, URL, title or bundle id.
    let q = sel.to_ascii_lowercase();
    let hits: Vec<InspectablePage> = pages
        .iter()
        .filter(|p| p.handle.to_ascii_lowercase().contains(&q) || p.url.to_ascii_lowercase().contains(&q) || p.title.to_ascii_lowercase().contains(&q) || p.bundle_id.to_ascii_lowercase().contains(&q))
        .cloned()
        .collect();
    match hits.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(Error::usage(format!("no inspectable page matches wir:{sel}")).hint(format!("open pages: {}", format_choices(pages)))),
        _ => Err(Error::usage(format!("{} inspectable pages match wir:{sel}", hits.len())).hint(format!("narrow the selector: {}", format_choices(&hits)))),
    }
}

/// Splits `wir:[selector]` into `(optional_udid_filter, page_selector)` by checking connected devices.
fn split_selector<'a>(raw: &'a str, known_udids: &[String]) -> (Option<&'a str>, &'a str) {
    let s = raw.strip_prefix("wir:").unwrap_or(raw).trim_matches('/');
    if s.is_empty() {
        return (None, "");
    }
    if known_udids.iter().any(|u| ios::matches_udid(u, s)) {
        return (Some(s), "");
    }
    if let Some((first, rest)) = s.split_once('/')
        && known_udids.iter().any(|u| ios::matches_udid(u, first))
    {
        return (Some(first), rest);
    }
    (None, s)
}

/// An attached, strictly read-only WebKit Inspector session on one page of a USB-connected iPhone.
/// Sends `_rpc_forwardDidClose:` on both `close()` and `Drop`.
pub struct Session {
    pub phone: Connected,
    pub page: InspectablePage,
    lockdown: Option<Lockdown>,
    svc: Service,
    conn_id: String,
    sender_id: String,
    target_id: Option<String>,
    target_swaps: u64,
    direct_mode: bool,
    next_outer_id: u64,
    next_inner_id: u64,
    in_flight: HashSet<u64>,
    outer_to_inner: HashMap<u64, u64>,
    replies: HashMap<u64, std::result::Result<Value, Error>>,
    events: VecDeque<WirEvent>,
    disconnected: Option<String>,
    stop: fn() -> bool,
    closed: bool,
}

impl Session {
    /// Attaches read-only (`WIRAutomaticallyPause = false`) to the page selected by `target` (`wir:[sel]`).
    /// `stop` comes from the caller (`app::stop_requested`) so `target` stays independent of `app`
    /// (SPEC §4.3) and never installs a signal handler inside the daemon.
    pub fn attach(target: &str, diagnostics: bool, timeout: Duration, stop: fn() -> bool) -> Result<Self> {
        let overall = Instant::now() + timeout;
        let mut last_err = None;
        for _attempt in 0..8 {
            if stop() {
                return Err(Error::new(130, "stopped (Ctrl-C, SIGTERM or strobe bench --stop)"));
            }
            if Instant::now() >= overall {
                break;
            }
            match Self::attach_once(target, diagnostics, overall, stop) {
                Ok(s) => return Ok(s),
                Err(e) if e.refusal.is_some() || e.code == 2 || e.code == 130 => return Err(e),
                Err(e) => {
                    last_err = Some(e);
                    if Instant::now() + Duration::from_millis(200) >= overall {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Error::timeout("timed out attaching to Web Inspector on the phone")))
    }

    fn attach_once(target: &str, diagnostics: bool, overall: Instant, stop: fn() -> bool) -> Result<Self> {
        let usb_deadline = Some(overall.min(Instant::now() + Duration::from_millis(4500)));
        let devices = ios::list(usb_deadline)?;
        let known_udids: Vec<String> = devices.iter().map(|d| d.udid.clone()).collect();
        let (udid_hint, page_sel) = split_selector(target, &known_udids);
        let (mut lockdown, phone) = ios::open_usb(udid_hint, diagnostics, usb_deadline)?;
        let mut svc = start_wir(&mut lockdown)?;
        let conn_id = uuid_v4();
        let disc_budget = overall.saturating_duration_since(Instant::now()).min(Duration::from_millis(3000));
        if disc_budget.is_zero() {
            svc.close();
            return Err(Error::timeout("the device did not answer in time"));
        }
        let safari_fast = page_sel.is_empty() || page_sel == "safari" || page_sel.starts_with("safari/");
        let (apps, pages) = discover_pages(&mut svc, &phone.udid, &conn_id, disc_budget, safari_fast)?;
        let mut page = choose_page(&pages, &apps, page_sel)?;
        // If a remote WIRConnectionIdentifierKey is still clearing over XPC from an immediately
        // preceding detach (within milliseconds), wait up to 600 ms for MobileSafari's updated listing.
        if page.debugger_attached && !page.local_debugger {
            let settle_until = Instant::now() + Duration::from_millis(600);
            while page.debugger_attached && Instant::now() < settle_until {
                svc.set_deadline(Some(settle_until));
                let Ok(msg) = svc.recv() else { break };
                if msg.get("__selector").and_then(Plist::as_string) == Some("_rpc_applicationSentListing:")
                    && let Some(arg) = msg.get("__argument").and_then(Plist::as_dictionary)
                    && arg.get("WIRApplicationIdentifierKey").and_then(Plist::as_string) == Some(page.app_id.as_str())
                    && let Some(listing) = arg.get("WIRListingKey").and_then(Plist::as_dictionary)
                {
                    let app = AppEntry { app_id: page.app_id.clone(), bundle_id: page.bundle_id.clone(), name: page.app_name.clone(), pid: page.app_pid, active: page.app_active, proxy: false };
                    if let Some(updated) = parse_listing(&phone.udid, &conn_id, &app, listing).into_iter().find(|p| p.page_id == page.page_id) {
                        page = updated;
                    }
                }
            }
        }
        if page.debugger_attached {
            svc.close();
            return Err(Error::refused(
                "DebuggerAttached",
                format!("{} already has a debugger attached", page.handle),
                "close Safari's Web Inspector (or any other debugger attached to this tab) and run again",
            ));
        }

        let sender_id = uuid_v4();
        let mut setup = Dictionary::new();
        setup.insert("WIRConnectionIdentifierKey".into(), conn_id.as_str().into());
        setup.insert("WIRApplicationIdentifierKey".into(), page.app_id.as_str().into());
        setup.insert("WIRPageIdentifierKey".into(), page.page_id.into());
        setup.insert("WIRSenderKey".into(), sender_id.as_str().into());
        setup.insert("WIRAutomaticallyPause".into(), false.into());
        setup.insert("WIRMessageDataTypeChunkSupportedKey".into(), 0u64.into());
        svc.set_deadline(Some(Instant::now() + Duration::from_secs(3)));
        send_rpc(&mut svc, "_rpc_forwardSocketSetup:", setup)?;

        let direct_mode = page.page_type == "WIRTypeWeb";
        let mut session = Session {
            phone,
            page,
            lockdown: Some(lockdown),
            svc,
            conn_id,
            sender_id,
            target_id: None,
            target_swaps: 0,
            direct_mode,
            next_outer_id: 1,
            next_inner_id: 1,
            in_flight: HashSet::new(),
            outer_to_inner: HashMap::new(),
            replies: HashMap::new(),
            events: VecDeque::new(),
            disconnected: None,
            stop,
            closed: false,
        };

        if !session.direct_mode {
            let wait_until = Instant::now() + Duration::from_millis(2500);
            while session.target_id.is_none() {
                if (session.stop)() {
                    return Err(Error::new(130, "stopped (Ctrl-C, SIGTERM or strobe bench --stop)"));
                }
                if Instant::now() >= wait_until {
                    return Err(Error::timeout(format!("{} accepted the socket setup, but sent no Target.targetCreated in 2.5 s", session.page.handle))
                        .hint("keep Safari open and unlocked on the phone, and close any other Web Inspector window"));
                }
                session.pump_once(wait_until)?;
            }
        }
        session.svc.set_deadline(None);
        Ok(session)
    }

    pub fn stopped(&self) -> bool {
        (self.stop)()
    }

    /// Number of `WebContent` page-target swaps or destructions since attach.
    pub fn target_swaps(&self) -> u64 {
        self.target_swaps
    }

    pub fn usb_device(&self) -> &Device {
        &self.phone.device
    }

    /// Reads the phone's current wall-clock time (`TimeIntervalSince1970`), or `None` if unavailable.
    pub fn wall_clock_s(&mut self) -> Option<f64> {
        self.lockdown.as_mut().and_then(ios::wall_clock_s)
    }

    /// Re-reads the phone's battery temperature to catch mid-recording overheating.
    pub fn refresh_battery(&mut self) {
        if let Some(l) = &mut self.lockdown {
            self.phone.battery = ios::battery(l, true);
        }
    }

    /// Sends one WebKit Inspector command without waiting for its reply, respecting `MAX_IN_FLIGHT`.
    pub fn send_async(&mut self, method: &str, params: Value) -> Result<u64> {
        let limit = Instant::now() + Duration::from_secs(10);
        while self.in_flight.len() >= MAX_IN_FLIGHT {
            self.check_alive()?;
            if Instant::now() >= limit {
                return Err(Error::timeout("Web Inspector target did not drain in-flight commands in time"));
            }
            self.pump_once(limit)?;
        }
        self.check_alive()?;
        let id = self.next_inner_id;
        self.next_inner_id += 1;

        let mut inner = Map::new();
        inner.insert("id".into(), json!(id));
        inner.insert("method".into(), json!(method));
        if match &params {
            Value::Null => false,
            Value::Object(o) => !o.is_empty(),
            _ => true,
        } {
            inner.insert("params".into(), params);
        }
        let inner_val = Value::Object(inner);

        let wire_bytes = if self.direct_mode {
            serde_json::to_vec(&inner_val)?
        } else {
            let tid = self.target_id.as_deref().ok_or_else(|| Error::refused("RunIncomplete", "the inspected page target closed", "keep the tab open on the phone and run again"))?;
            let outer_id = self.next_outer_id;
            self.next_outer_id += 1;
            self.outer_to_inner.insert(outer_id, id);
            serde_json::to_vec(&json!({
                "id": outer_id,
                "method": "Target.sendMessageToTarget",
                "params": {
                    "targetId": tid,
                    "message": serde_json::to_string(&inner_val)?,
                }
            }))?
        };

        let mut arg = Dictionary::new();
        arg.insert("WIRConnectionIdentifierKey".into(), self.conn_id.as_str().into());
        arg.insert("WIRApplicationIdentifierKey".into(), self.page.app_id.as_str().into());
        arg.insert("WIRPageIdentifierKey".into(), self.page.page_id.into());
        arg.insert("WIRSessionIdentifierKey".into(), self.sender_id.as_str().into());
        arg.insert("WIRSenderKey".into(), self.sender_id.as_str().into());
        arg.insert("WIRSocketDataKey".into(), Plist::Data(wire_bytes));
        self.svc.set_deadline(Some(Instant::now() + Duration::from_secs(5)));
        if let Err(e) = send_rpc(&mut self.svc, "_rpc_forwardSocketData:", arg) {
            self.disconnected = Some(e.message.clone());
            return Err(disconnect_error(&self.page.short_udid, &e.message, (self.stop)()));
        }
        self.in_flight.insert(id);
        Ok(id)
    }

    /// Sends a WebKit Inspector command and waits up to `timeout` for its reply.
    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.send_async(method, params)?;
        self.wait_reply(id, timeout)
    }

    pub fn wait_reply(&mut self, id: u64, timeout: Duration) -> Result<Value> {
        let until = Instant::now() + timeout;
        loop {
            if let Some(res) = self.replies.remove(&id) {
                return res;
            }
            self.check_alive()?;
            if Instant::now() >= until {
                self.in_flight.remove(&id);
                return Err(Error::timeout("Web Inspector command timed out waiting for the phone"));
            }
            self.pump_once(until)?;
        }
    }

    /// Pumps incoming WIR messages until `until` (or until interrupted by Ctrl-C), buffering events.
    pub fn pump_for(&mut self, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.check_alive()?;
            let step = until.min(Instant::now() + Duration::from_millis(100));
            match self.pump_once_quiet(step) {
                Ok(()) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Drains all buffered target events received so far.
    pub fn drain_events(&mut self) -> Vec<WirEvent> {
        self.events.drain(..).collect()
    }

    /// Evaluates a JavaScript expression in the inspected page without mutating page globals or
    /// console history, awaiting a returned Promise via `Runtime.awaitPromise` when needed.
    pub fn eval(&mut self, expression: &str, timeout: Duration) -> Result<Value> {
        let r = self.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "doNotPauseOnExceptionsAndMuteConsole": true,
                "saveResult": false,
            }),
            timeout,
        )?;
        if r["wasThrown"] == true {
            let desc = r["result"]["description"].as_str().unwrap_or("JavaScript evaluation threw");
            return Err(Error::msg(format!("Runtime.evaluate: {desc}")));
        }
        let res = &r["result"];
        if res["type"] == "object"
            && res["className"] == "Promise"
            && let Some(oid) = res["objectId"].as_str()
        {
            let awaited = self.call(
                "Runtime.awaitPromise",
                json!({
                    "promiseObjectId": oid,
                    "returnByValue": true,
                    "saveResult": false,
                }),
                timeout,
            );
            let _ = self.call("Runtime.releaseObject", json!({ "objectId": oid }), Duration::from_secs(2));
            let awaited = awaited?;
            if awaited["wasThrown"] == true {
                let desc = awaited["result"]["description"].as_str().unwrap_or("awaited Promise rejected");
                return Err(Error::msg(format!("Runtime.awaitPromise: {desc}")));
            }
            return Ok(awaited["result"]["value"].clone());
        }
        Ok(res["value"].clone())
    }

    /// Captures a PNG from the device via `Page.snapshotRect` (`coordinateSystem`: `"Viewport"` or `"Page"`).
    pub fn snapshot_rect(&mut self, x: i64, y: i64, width: i64, height: i64, coordinate_system: &str, timeout: Duration) -> Result<Vec<u8>> {
        let r = self.call(
            "Page.snapshotRect",
            json!({
                "x": x,
                "y": y,
                "width": width.max(1),
                "height": height.max(1),
                "coordinateSystem": coordinate_system,
            }),
            timeout,
        )?;
        let data_url = r["dataURL"].as_str().ok_or_else(|| Error::msg("Page.snapshotRect returned no dataURL"))?;
        let b64 = data_url.split_once(',').map_or(data_url, |(_, b)| b);
        base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| Error::msg(format!("Page.snapshotRect PNG decode: {e}")))
    }

    /// Explicitly sends `_rpc_forwardDidClose:` and releases the lockdownd session.
    pub fn close(&mut self) {
        self.send_did_close();
        self.lockdown = None;
    }

    fn send_did_close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if self.disconnected.is_none() {
            self.svc.set_deadline(Some(Instant::now() + Duration::from_millis(300)));
            let mut arg = Dictionary::new();
            arg.insert("WIRConnectionIdentifierKey".into(), self.conn_id.as_str().into());
            arg.insert("WIRApplicationIdentifierKey".into(), self.page.app_id.as_str().into());
            arg.insert("WIRPageIdentifierKey".into(), self.page.page_id.into());
            arg.insert("WIRSenderKey".into(), self.sender_id.as_str().into());
            if send_rpc(&mut self.svc, "_rpc_forwardDidClose:", arg).is_ok() {
                // Wait briefly for MobileSafari's updated listing confirming detach before closing TLS.
                let ack_until = Instant::now() + Duration::from_millis(120);
                while Instant::now() < ack_until {
                    self.svc.set_deadline(Some(ack_until));
                    let Ok(msg) = self.svc.recv() else { break };
                    if msg.get("__selector").and_then(Plist::as_string) == Some("_rpc_applicationSentListing:")
                        && let Some(a) = msg.get("__argument").and_then(Plist::as_dictionary)
                        && a.get("WIRApplicationIdentifierKey").and_then(Plist::as_string) == Some(self.page.app_id.as_str())
                        && let Some(listing) = a.get("WIRListingKey").and_then(Plist::as_dictionary)
                    {
                        let still_ours = listing.iter().any(|(k, v)| {
                            let Some(d) = v.as_dictionary() else { return false };
                            let pid = d.get("WIRPageIdentifierKey").and_then(plist_u64).or_else(|| k.parse::<u64>().ok());
                            pid == Some(self.page.page_id) && d.get("WIRConnectionIdentifierKey").and_then(Plist::as_string) == Some(self.conn_id.as_str())
                        });
                        if !still_ours {
                            break;
                        }
                    }
                }
            }
        }
        self.svc.close();
    }

    fn check_alive(&self) -> Result<()> {
        if (self.stop)() {
            return Err(Error::new(130, "stopped (Ctrl-C, SIGTERM or strobe bench --stop)"));
        }
        if let Some(why) = &self.disconnected {
            return Err(disconnect_error(&self.page.short_udid, why, false));
        }
        Ok(())
    }

    fn pump_once(&mut self, until: Instant) -> Result<()> {
        let step = until.min(Instant::now() + Duration::from_millis(200));
        self.pump_once_quiet(step)
    }

    fn pump_once_quiet(&mut self, step_deadline: Instant) -> Result<()> {
        self.svc.set_deadline(Some(step_deadline));
        let msg = match self.svc.recv() {
            Ok(m) => m,
            Err(e) if e.code == 124 => return Ok(()),
            Err(e) => {
                self.disconnected = Some(e.message.clone());
                return Err(disconnect_error(&self.page.short_udid, &e.message, (self.stop)()));
            }
        };
        self.handle_rpc(msg)
    }

    fn handle_rpc(&mut self, msg: Dictionary) -> Result<()> {
        let sel = msg.get("__selector").and_then(Plist::as_string).unwrap_or("");
        let Some(arg) = msg.get("__argument").and_then(Plist::as_dictionary) else { return Ok(()) };
        match sel {
            "_rpc_applicationSentData:" => {
                if let Some(dest) = arg.get("WIRDestinationKey").and_then(Plist::as_string)
                    && !dest.is_empty()
                    && dest != self.sender_id
                {
                    return Ok(());
                }
                let raw = match arg.get("WIRMessageDataKey") {
                    Some(Plist::Data(b)) => b.as_slice(),
                    Some(Plist::String(s)) => s.as_bytes(),
                    _ => return Ok(()),
                };
                if !crate::store::shallow(raw) {
                    return Err(Error::msg(format!("WIR JSON: nested more than {} levels deep", crate::store::MAX_JSON_DEPTH)));
                }
                let mut de = serde_json::Deserializer::from_slice(raw);
                de.disable_recursion_limit();
                let payload: Value = serde::Deserialize::deserialize(&mut de).map_err(|e| Error::msg(format!("WIR JSON: {e}")))?;
                self.handle_outer_json(payload)?;
            }
            "_rpc_applicationDisconnected:" => {
                if arg.get("WIRApplicationIdentifierKey").and_then(Plist::as_string) == Some(self.page.app_id.as_str()) {
                    self.disconnected = Some("Safari closed or suspended while attached (is the phone locked?)".into());
                }
            }
            "_rpc_applicationSentListing:" => {
                if arg.get("WIRApplicationIdentifierKey").and_then(Plist::as_string) == Some(self.page.app_id.as_str())
                    && let Some(listing) = arg.get("WIRListingKey").and_then(Plist::as_dictionary)
                {
                    let still_there =
                        listing.iter().any(|(k, v)| v.as_dictionary().and_then(|d| d.get("WIRPageIdentifierKey")).and_then(plist_u64).or_else(|| k.parse::<u64>().ok()) == Some(self.page.page_id));
                    if !still_there {
                        self.disconnected = Some("the inspected tab closed on the phone".into());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_outer_json(&mut self, payload: Value) -> Result<()> {
        if self.direct_mode {
            self.handle_inner_json(payload);
            return Ok(());
        }
        match payload["method"].as_str() {
            Some("Target.targetCreated") => {
                let info = &payload["params"]["targetInfo"];
                if let Some(tid) = info["targetId"].as_str() {
                    let ty = info["type"].as_str().unwrap_or("page");
                    let provisional = info["isProvisional"].as_bool().unwrap_or(false);
                    // Never leave a target paused if WebKit created it paused.
                    if info["isPaused"].as_bool().unwrap_or(false) {
                        let _ = self.send_outer_raw(&json!({
                            "id": self.next_outer_id,
                            "method": "Target.resume",
                            "params": { "targetId": tid },
                        }));
                        self.next_outer_id += 1;
                    }
                    if ty == "page" && (!provisional || self.target_id.is_none()) {
                        if self.target_id.as_deref().is_some_and(|prev| prev != tid) {
                            self.target_swaps += 1;
                        }
                        self.target_id = Some(tid.to_string());
                    }
                }
            }
            Some("Target.didCommitProvisionalTarget") => {
                if let Some(new_id) = payload["params"]["newTargetId"].as_str() {
                    self.target_swaps += 1;
                    self.target_id = Some(new_id.to_string());
                }
            }
            Some("Target.targetDestroyed") => {
                if let Some(gone) = payload["params"]["targetId"].as_str()
                    && self.target_id.as_deref() == Some(gone)
                {
                    self.target_swaps += 1;
                    self.target_id = None;
                }
            }
            Some("Target.dispatchMessageFromTarget") => {
                if let Some(msg_str) = payload["params"]["message"].as_str().filter(|m| crate::store::shallow(m.as_bytes())) {
                    let mut de = serde_json::Deserializer::from_str(msg_str);
                    de.disable_recursion_limit();
                    if let Ok(inner) = serde::Deserialize::deserialize(&mut de) {
                        self.handle_inner_json(inner);
                    }
                }
            }
            None => {
                if let Some(outer_id) = payload["id"].as_u64()
                    && let Some(inner_id) = self.outer_to_inner.remove(&outer_id)
                    && let Some(err) = payload.get("error")
                    && !err.is_null()
                {
                    self.in_flight.remove(&inner_id);
                    let msg = err["message"].as_str().unwrap_or("Target.sendMessageToTarget error");
                    self.replies.insert(inner_id, Err(Error::msg(msg.to_string())));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_inner_json(&mut self, inner: Value) {
        if let Some(id) = inner["id"].as_u64()
            && inner.get("method").is_none()
        {
            self.in_flight.remove(&id);
            if let Some(err) = inner.get("error").filter(|e| !e.is_null()) {
                let msg = err["message"].as_str().unwrap_or("Web Inspector protocol error");
                self.replies.insert(id, Err(Error::msg(msg.to_string())));
            } else {
                self.replies.insert(id, Ok(inner.get("result").cloned().unwrap_or(Value::Null)));
            }
            return;
        }
        if let Some(method) = inner["method"].as_str() {
            self.events.push_back(WirEvent { host_ns: host::now_ns(), method: method.to_string(), params: inner.get("params").cloned().unwrap_or(Value::Null) });
        }
    }

    fn send_outer_raw(&mut self, outer: &Value) -> Result<()> {
        let mut arg = Dictionary::new();
        arg.insert("WIRConnectionIdentifierKey".into(), self.conn_id.as_str().into());
        arg.insert("WIRApplicationIdentifierKey".into(), self.page.app_id.as_str().into());
        arg.insert("WIRPageIdentifierKey".into(), self.page.page_id.into());
        arg.insert("WIRSessionIdentifierKey".into(), self.sender_id.as_str().into());
        arg.insert("WIRSenderKey".into(), self.sender_id.as_str().into());
        arg.insert("WIRSocketDataKey".into(), Plist::Data(serde_json::to_vec(outer)?));
        self.svc.set_deadline(Some(Instant::now() + Duration::from_secs(5)));
        send_rpc(&mut self.svc, "_rpc_forwardSocketData:", arg)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.send_did_close();
    }
}

fn disconnect_error(short_udid: &str, why: &str, stopped: bool) -> Error {
    if stopped {
        return Error::new(130, "stopped (Ctrl-C, SIGTERM or strobe bench --stop)");
    }
    Error::refused("RunIncomplete", format!("lost connection to the phone ({short_udid}): {why}"), "keep the phone unlocked on the USB cable with the tab open in Safari, then run again")
}

/// Folds `Console.messageAdded`, `Console.messageRepeatCountUpdated`, and `Runtime.exceptionThrown`
/// events from `events` into `out`.
pub fn collect_console_events(events: &[WirEvent], t0_ns: u64, out: &mut Vec<ConsoleEntry>) {
    for ev in events {
        let at_ms = ev.host_ns.saturating_sub(t0_ns) as f64 / 1e6;
        match ev.method.as_str() {
            "Console.messageAdded" => {
                let m = &ev.params["message"];
                let source = m["source"].as_str().unwrap_or("console-api").to_string();
                let level = m["level"].as_str().unwrap_or("log").to_string();
                let mut text = m["text"].as_str().unwrap_or("").to_string();
                if let Some(params) = m["parameters"].as_array()
                    && !params.is_empty()
                {
                    let rendered: Vec<String> = params
                        .iter()
                        .map(|a| match a["value"].as_str() {
                            Some(s) => s.to_string(),
                            None if !a["value"].is_null() => a["value"].to_string(),
                            None => a["description"].as_str().or(a["type"].as_str()).unwrap_or("").to_string(),
                        })
                        .collect();
                    let joined = rendered.join(" ");
                    if !joined.is_empty() {
                        text = joined;
                    }
                }
                let url = m["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string);
                let line = m["line"].as_i64().filter(|&l| l > 0);
                let column = m["column"].as_i64().filter(|&c| c > 0);
                let repeat_count = m["repeatCount"].as_u64().unwrap_or(1).max(1);
                out.push(ConsoleEntry { at_ms, source, level, text, url, line, column, repeat_count });
            }
            "Console.messageRepeatCountUpdated" => {
                if let (Some(last), Some(c)) = (out.last_mut(), ev.params["count"].as_u64()) {
                    last.repeat_count = c.max(1);
                }
            }
            "Runtime.exceptionThrown" => {
                let d = &ev.params["exceptionDetails"];
                let text = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception").to_string();
                let url = d["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string);
                let line = d["lineNumber"].as_i64().map(|l| l + 1);
                let column = d["columnNumber"].as_i64().map(|c| c + 1);
                out.push(ConsoleEntry { at_ms, source: "javascript".into(), level: "error".into(), text, url, line, column, repeat_count: 1 });
            }
            _ => {}
        }
    }
}
