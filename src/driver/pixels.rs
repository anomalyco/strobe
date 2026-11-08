//! Image anchors and pixel waits (SPEC §9.6), one engine for every driver that can see its target's
//! pixels: a native app's window from outside (`host::capture`), a page through its browser's
//! screenshots outside timed spans or its window from outside inside them. For apps whose UI is drawn
//! with no accessibility tree (a game, a canvas, a GPU toolkit's own widgets), where a selector finds
//! nothing.
//!
//! - An image locator `{ image, region?, threshold?, scale? }` stands where a selector does: input aims
//!   at the centre of its one match (`AnchorNotFound` when none shows in the call's timeout,
//!   `AnchorAmbiguous` when two or more still show 50 ms later: fail fast rather than guess).
//! - `until` and `waitFor` take it (any match), or `{ pixels: { region?, changed | stable | matches } }`:
//!   the region changed from what it showed before the input (`changed`: more than `pct`% of its pixels,
//!   and at least 16, moved by more than `delta` in a channel), held still (`stable`: no channel moved by
//!   more than 8 for `ms`), or shows an image (`matches`).
//! - A wait after an input is its pixel proof: the last capture that lacked the change and the first that
//!   had it bound the frame that showed it (`interaction.*` from the OS presents between them).
//! - Every capture is a read: `driver.pixel_reads`, `driver.pixel_ms`, and strobe's CPU in them
//!   (`overhead.pixel_pct`).

use crate::host::capture::Shot;
use crate::image::{Found, Template};
use crate::model::{Error, Refusal, Result, ValidatorId};
use crate::png::Image;
use crate::store::actions::Hit;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A region of the target's own area (a window's client area in device-independent pixels, a page's
/// viewport in CSS pixels), or of 0 to 1 of it (`norm`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Region {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    norm: bool,
}

impl Region {
    fn parse(v: &Value) -> Result<Option<Region>> {
        let num = |x: &Value| x.as_f64().filter(|f| f.is_finite());
        let bad = || Error::usage(format!("a region is [x, y, width, height] or {{ x, y, width, height, space? }} (numbers; space \"dip\" or \"norm\"), not {v}"));
        let r = match v {
            Value::Null => return Ok(None),
            Value::Array(a) if a.len() == 4 => {
                let n: Vec<f64> = a.iter().filter_map(num).collect();
                if n.len() != 4 {
                    return Err(bad());
                }
                Region { x: n[0], y: n[1], w: n[2], h: n[3], norm: false }
            }
            Value::Object(o) => {
                let get = |k: &str| o.get(k).and_then(num).ok_or_else(bad);
                let norm = match o.get("space").and_then(Value::as_str) {
                    None | Some("dip" | "css") => false,
                    Some("norm") => true,
                    Some(_) => return Err(bad()),
                };
                Region { x: get("x")?, y: get("y")?, w: get("width")?, h: get("height")?, norm }
            }
            _ => return Err(bad()),
        };
        if r.w <= 0.0 || r.h <= 0.0 {
            return Err(Error::usage(format!("a region has a width and height over 0, not {v}")));
        }
        Ok(Some(r))
    }

    /// `[x, y, w, h]` in the area's own units, for an area of `size`.
    fn rect(self, size: (f64, f64)) -> [f64; 4] {
        if self.norm { [self.x * size.0, self.y * size.1, self.w * size.0, self.h * size.1] } else { [self.x, self.y, self.w, self.h] }
    }
}

/// `region` (or all of the area) as a rect, refused `OutOfView` when nothing of it is in the area.
fn area(region: Option<Region>, size: (f64, f64), what: &str) -> Result<[f64; 4]> {
    let r = region.map_or([0.0, 0.0, size.0, size.1], |r| r.rect(size));
    let (x0, y0, x1, y1) = (r[0].max(0.0), r[1].max(0.0), (r[0] + r[2]).min(size.0), (r[1] + r[3]).min(size.1));
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return Err(Refusal::new(ValidatorId::OutOfView, format!("the region [{:.0}, {:.0}, {:.0}, {:.0}] is off the {:.0}×{:.0} {what}", r[0], r[1], r[2], r[3], size.0, size.1))
            .fix(format!("give a region inside the {what}: [x, y, width, height] from its top left, or {{ x, y, width, height, space: \"norm\" }} from 0 to 1"))
            .into());
    }
    Ok([x0, y0, x1 - x0, y1 - y0])
}

/// An image locator: the image (an absolute path, the runner resolved it against the bench file's folder),
/// the name the bench file gave it, where to look, how alike, and its pixels per device-independent pixel.
#[derive(Clone, Debug)]
pub(crate) struct Anchor {
    path: String,
    pub name: String,
    region: Option<Region>,
    threshold: f64,
    scale: f64,
}

/// What a pixel wait waits for.
#[derive(Clone, Debug)]
pub(crate) enum Cond {
    Changed { delta: u8, pct: f64 },
    Stable { ms: u64 },
    Matches(Anchor),
}

/// An `until` or `waitFor` on pixels: an image showing, or `{ pixels }`.
#[derive(Clone, Debug)]
pub(crate) struct Look {
    region: Option<Region>,
    cond: Cond,
}

fn anchor_of(o: &serde_json::Map<String, Value>, v: &Value) -> Result<Anchor> {
    let path = o
        .get("image")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Error::usage(format!("an image locator is {{ image: \"<file>.png\", region?, threshold?, scale? }}, not {v}")))?;
    if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "image" | "as" | "region" | "threshold" | "scale")) {
        return Err(Error::usage(format!("an image locator takes image, region, threshold and scale, not {k}")));
    }
    let threshold = match o.get("threshold") {
        None | Some(Value::Null) => 0.95,
        Some(t) => t.as_f64().filter(|t| *t > 0.0 && *t <= 1.0).ok_or_else(|| Error::usage(format!("an image locator's threshold is over 0 and at most 1, not {t}")))?,
    };
    let scale = match o.get("scale") {
        None | Some(Value::Null) => 1.0,
        Some(s) => s
            .as_f64()
            .filter(|s| *s > 0.0 && s.is_finite())
            .ok_or_else(|| Error::usage(format!("an image locator's scale is the image's pixels per device-independent pixel (1, or 2 for @2x), not {s}")))?,
    };
    Ok(Anchor { path: path.to_string(), name: o.get("as").and_then(Value::as_str).unwrap_or(path).to_string(), region: Region::parse(o.get("region").unwrap_or(&Value::Null))?, threshold, scale })
}

/// `{ image, … }` in a selector's place; None for anything else.
pub(crate) fn anchor(v: &Value) -> Option<Result<Anchor>> {
    let o = v.as_object().filter(|o| o.contains_key("image"))?;
    Some(anchor_of(o, v))
}

/// An image locator or `{ pixels: { … } }`, as an `until` or `waitFor` takes; None for anything else.
pub(crate) fn look(v: &Value) -> Option<Result<Look>> {
    if let Some(a) = anchor(v) {
        return Some(a.map(|a| Look { region: a.region, cond: Cond::Matches(a) }));
    }
    let p = v.as_object().filter(|o| o.len() == 1)?.get("pixels")?;
    Some((|| {
        let o = p.as_object().ok_or_else(|| Error::usage(format!("pixels is {{ region?, changed | stable | matches }}, not {p}")))?;
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "region" | "changed" | "stable" | "matches" | "as" | "threshold" | "scale")) {
            return Err(Error::usage(format!("pixels takes region, changed, stable, matches, threshold and scale, not {k}")));
        }
        let given: Vec<&str> = ["changed", "stable", "matches"].into_iter().filter(|k| o.get(*k).is_some_and(|v| !v.is_null() && *v != false)).collect();
        if given.len() != 1 {
            return Err(Error::usage(format!("pixels waits for exactly one of changed, stable or matches, not {}", if given.is_empty() { "none".to_string() } else { given.join(" and ") })));
        }
        let region = Region::parse(o.get("region").unwrap_or(&Value::Null))?;
        let cond = match given[0] {
            "changed" => {
                let c = &o["changed"];
                let delta = c.get("delta").and_then(Value::as_f64).unwrap_or(8.0);
                let pct = c.get("pct").and_then(Value::as_f64).unwrap_or(0.5);
                if !(0.0..255.0).contains(&delta) || !(0.0..=100.0).contains(&pct) {
                    return Err(Error::usage(format!("pixels.changed is true or {{ delta (0 to 254), pct (0 to 100) }}, not {c}")));
                }
                Cond::Changed { delta: delta as u8, pct }
            }
            "stable" => Cond::Stable {
                ms: match &o["stable"] {
                    Value::Bool(true) => 300,
                    s => s.as_u64().filter(|ms| *ms > 0).ok_or_else(|| Error::usage(format!("pixels.stable is true (300 ms) or milliseconds, not {s}")))?,
                },
            },
            _ => {
                let mut m = serde_json::Map::new();
                m.insert("image".into(), o["matches"].clone());
                for k in ["as", "threshold", "scale"] {
                    if let Some(x) = o.get(k) {
                        m.insert(k.into(), x.clone());
                    }
                }
                let mut a = anchor_of(&m, p)?;
                a.region = region;
                Cond::Matches(a)
            }
        };
        Ok(Look { region, cond })
    })())
}

/// Whether an argument is a pixel locator or wait, or a list of waits with one (for the drivers that have none:
/// a terminal).
pub(crate) fn is_pixels(v: &Value) -> bool {
    anchor(v).is_some() || look(v).is_some() || v.as_array().is_some_and(|l| l.iter().any(|u| anchor(u).is_some() || look(u).is_some()))
}

/// What can show a target's pixels: its own area's size, a capture of a rect of it, and a wait for what
/// may come next.
pub(crate) trait Source {
    /// The area's size: a window's client area (device-independent pixels), a page's viewport (CSS pixels).
    fn size(&self) -> Result<(f64, f64)>;
    /// The pixels of `r` (in the area's units) as they show now.
    fn grab(&self, r: [f64; 4]) -> Result<Shot>;
    /// Until the target may show something other than `after` (a frame after it), at most `until`.
    fn wake(&self, after: &Shot, until: Instant);
    /// The source's frame count now, where frames come as events (Windows.Graphics.Capture): a still
    /// window sends none, so an unchanged count says the pixels held without reading them. None elsewhere.
    fn seq(&self) -> Option<u64> {
        None
    }
    /// What the area is called in messages.
    fn what(&self) -> &'static str {
        "window's client area"
    }
}

/// A window's client area read from outside with no driving session (`resetPixels` before a case drove
/// the app, `look`): its capture opened on first use, kept in the cell for the next read.
pub(crate) struct Window<'a> {
    pub cell: &'a RefCell<Option<crate::host::capture::Grabber>>,
    pub win: isize,
}

impl Window<'_> {
    fn with<T>(&self, f: impl FnOnce(&mut crate::host::capture::Grabber) -> Result<T>) -> Result<T> {
        let mut g = self.cell.borrow_mut();
        if g.as_ref().is_none_or(|g| g.window() != self.win) {
            *g = Some(crate::host::capture::Grabber::open(self.win)?);
        }
        f(g.as_mut().ok_or_else(|| Error::msg("internal: the window's capture"))?)
    }
}

impl Source for Window<'_> {
    fn size(&self) -> Result<(f64, f64)> {
        self.with(|g| g.size())
    }

    fn grab(&self, r: [f64; 4]) -> Result<Shot> {
        self.with(|g| g.grab(r))
    }

    fn wake(&self, after: &Shot, until: Instant) {
        if let Some(g) = self.cell.borrow().as_ref().filter(|g| g.evented()) {
            g.wait(after.seq, until);
            return;
        }
        std::thread::sleep(until.min(Instant::now() + Duration::from_millis(30)).saturating_duration_since(Instant::now()));
    }

    fn seq(&self) -> Option<u64> {
        self.cell.borrow().as_ref().and_then(|g| g.seq())
    }
}

/// What a wait proved: the last capture that lacked what it waited for (its start; none when the first
/// capture had it) and the first that had it (its end). `timing`: a change or a match, which bound the
/// frame that showed it (a stable wait proves nothing of when).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Proof {
    pub lacked: Option<u64>,
    pub had: u64,
    pub timing: bool,
}

/// Where an image locator aimed: its match's centre and box in the area's units.
pub(crate) struct Aim {
    pub x: f64,
    pub y: f64,
    pub rect: [f64; 4],
}

thread_local! {
    /// The capture an anchor was last not found in, for the failed step's `failed-<step>-anchor.png`.
    static FAILED: RefCell<Option<Image>> = const { RefCell::new(None) };
}

/// The capture the last `AnchorNotFound` searched, taken.
pub fn take_failed() -> Option<Image> {
    FAILED.take()
}

/// A block's image engine: its templates at each scale they met, and its reads.
#[derive(Default)]
pub(crate) struct Pixels {
    templates: RefCell<HashMap<(String, u64), Rc<Template>>>,
    /// Captures since the last `take_reads`, their wall time and strobe's CPU in them (ms).
    reads: Cell<(u64, f64, f64)>,
}

impl Pixels {
    fn template(&self, a: &Anchor) -> Result<Rc<Template>> {
        let key = (a.path.clone(), a.scale.to_bits());
        if let Some(t) = self.templates.borrow().get(&key) {
            return Ok(t.clone());
        }
        let t = Rc::new(Template::load(&a.path, a.scale).map_err(|e| Error { message: e.message.replace(&format!("\"{}\"", a.path), &format!("\"{}\"", a.name)), ..e })?);
        self.templates.borrow_mut().insert(key, t.clone());
        Ok(t)
    }

    /// The captures (and matches) since the last call: count, wall ms, strobe's CPU ms.
    pub fn take_reads(&self) -> (u64, f64, f64) {
        self.reads.replace((0, 0.0, 0.0))
    }

    /// Runs one read (a capture and what it does with it), counted.
    fn counted<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let r = self.charged(f);
        let (n, ms, c) = self.reads.get();
        self.reads.set((n + 1, ms, c));
        r
    }

    /// Runs pixel work that is no read of its own (a template's first decode): its wall time and CPU
    /// count with the reads.
    fn charged<T>(&self, f: impl FnOnce() -> T) -> T {
        let (at, cpu) = (Instant::now(), crate::probe::video::ThreadCpu::start());
        let r = f();
        let (n, ms, c) = self.reads.get();
        self.reads.set((n, ms + at.elapsed().as_secs_f64() * 1e3, c + cpu.ms().unwrap_or(0.0)));
        r
    }

    /// CPU (ms) of a window capture strobe closes or replaces (its open and frame events), kept for the
    /// next `take_reads`: a run that read two windows pays for both.
    pub fn bank_cpu(&self, ms: f64) {
        let (n, w, c) = self.reads.get();
        self.reads.set((n, w, c + ms));
    }

    /// One capture of `look`'s region.
    fn shoot(&self, src: &dyn Source, rect: [f64; 4]) -> Result<Shot> {
        self.counted(|| src.grab(rect))
    }

    /// The matches of `a` now: in the area's units, best first, and the search.
    fn find(&self, src: &dyn Source, a: &Anchor, size: (f64, f64)) -> Result<(Vec<Aim>, Found, Shot, [f64; 4])> {
        let t = self.charged(|| self.template(a))?;
        let rect = area(a.region, size, src.what())?;
        let (shot, found) = self.counted(|| {
            let shot = src.grab(rect)?;
            let found = t.find(&shot.img, shot.scale, None, a.threshold)?;
            Ok((shot, found))
        })?;
        let aims = found
            .hits
            .iter()
            .map(|h| {
                let r = [rect[0] + h.x / shot.scale, rect[1] + h.y / shot.scale, h.w / shot.scale, h.h / shot.scale];
                Aim { x: r[0] + r[2] / 2.0, y: r[1] + r[3] / 2.0, rect: r }
            })
            .collect();
        Ok((aims, found, shot, rect))
    }

    /// `AnchorNotFound`, with the best place's score and where, and the capture kept for the step.
    fn not_found(&self, a: &Anchor, found: &Found, shot: Shot, rect: [f64; 4], t: Duration, src: &dyn Source) -> Error {
        let best = match found.best {
            Some(b) => format!(
                "; the best place scored {:.3} at ({:.0}, {:.0}) in the {}{}",
                b.score,
                rect[0] + b.x / shot.scale,
                rect[1] + b.y / shot.scale,
                src.what(),
                found.colour.map(|c| format!(", its colours {c:.0} apart per channel")).unwrap_or_default()
            ),
            None => "; the image is larger than where strobe looked".to_string(),
        };
        FAILED.set(Some(shot.img));
        Refusal::new(ValidatorId::AnchorNotFound, format!("image \"{}\": no match at threshold {} in {:.1} s{best}", a.name, a.threshold, t.as_secs_f64()))
            .fix("compare the image with the capture saved for the failed step (failed-<step>-anchor.png); give scale: 2 for an image exported at @2x, a region to look in, or a lower threshold")
            .into()
    }

    /// Where input for `a` goes: its one match, waited for up to `t`. Two or more matches are read again
    /// 50 ms later (an animation passing over a lookalike) and still two or more refuse.
    pub fn aim(&self, src: &dyn Source, a: &Anchor, t: Duration) -> Result<Aim> {
        let deadline = Instant::now() + t;
        let size = src.size()?;
        let mut again = false;
        loop {
            let (mut aims, found, shot, rect) = self.find(src, a, size)?;
            match aims.len() {
                1 => {
                    let m = aims.remove(0);
                    super::aimed(Hit { role: "image".into(), key: Some(a.name.clone()), name: String::new(), point: Some([m.x, m.y]), rect: Some(m.rect), scrolled: false });
                    return Ok(m);
                }
                n if n >= 2 && again => {
                    let at: Vec<String> = aims.iter().take(4).map(|m| format!("({:.0}, {:.0})", m.x, m.y)).collect();
                    return Err(Refusal::new(ValidatorId::AnchorAmbiguous, format!("image \"{}\": {n} places match it (at {}), so input aimed at it would be a guess", a.name, at.join(", ")))
                        .fix("narrow it with a region, crop the image to what only the one place has, raise the threshold, or page.query({ image }) in setup and aim at the point")
                        .into());
                }
                0 if Instant::now() >= deadline => return Err(self.not_found(a, &found, shot, rect, t, src)),
                0 => src.wake(&shot, deadline.min(Instant::now() + Duration::from_millis(250))),
                _ => {
                    again = true;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    /// Every match of `a` now (no wait, no refusal on several): `page.query({ image })`.
    pub fn query(&self, src: &dyn Source, a: &Anchor) -> Result<Value> {
        let (aims, found, _, _) = self.find(src, a, src.size()?)?;
        Ok(Value::Array(
            aims.iter()
                .zip(&found.hits)
                .map(|(m, h)| {
                    json!({ "key": a.name, "path": a.name, "role": "image", "name": "", "rect": m.rect, "point": [m.x, m.y], "score": (h.score * 1000.0).round() / 1000.0, "actionable": null, "focused": false, "enabled": true, "checked": null, "expanded": null, "selected": null })
                })
                .collect(),
        ))
    }

    /// Whether `a` shows anywhere within `t` (`expect.visible`).
    pub fn shows(&self, src: &dyn Source, a: &Anchor, t: Duration) -> Result<bool> {
        let deadline = Instant::now() + t;
        let size = src.size()?;
        loop {
            let (aims, _, shot, _) = self.find(src, a, size)?;
            if !aims.is_empty() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            src.wake(&shot, deadline.min(Instant::now() + Duration::from_millis(250)));
        }
    }

    /// Before an input whose `until` is a pixel wait: the region as it shows (`changed` compares with it),
    /// and whether it already has what the wait wants (then the wait proves nothing of the input).
    pub fn before(&self, src: &dyn Source, l: &Look) -> Result<Before> {
        let size = src.size()?;
        Ok(match &l.cond {
            Cond::Matches(a) => {
                let (aims, _, shot, _) = self.find(src, a, size)?;
                Before { shot, had: !aims.is_empty() }
            }
            _ => Before { shot: self.shoot(src, area(l.region, size, src.what())?)?, had: false },
        })
    }

    /// Waits up to `t` for `l`, from `before` (taken before the input; without it the first capture is
    /// the baseline `changed` compares with).
    pub fn wait(&self, src: &dyn Source, l: &Look, before: Option<Before>, t: Duration) -> Result<Proof> {
        let deadline = Instant::now() + t;
        let size = src.size()?;
        let rect = area(l.region, size, src.what())?;
        let mut lacked = before.as_ref().filter(|b| !b.had).map(|b| b.shot.t0_ns);
        let already = before.as_ref().is_some_and(|b| b.had);
        match &l.cond {
            Cond::Matches(a) => loop {
                let (aims, found, shot, rect) = self.find(src, a, size)?;
                if !aims.is_empty() {
                    return Ok(Proof { lacked, had: shot.t_ns + shot.late_ns, timing: !already });
                }
                if Instant::now() >= deadline {
                    return Err(self.not_found(a, &found, shot, rect, t, src));
                }
                lacked = Some(shot.t0_ns);
                src.wake(&shot, deadline.min(Instant::now() + Duration::from_millis(250)));
            },
            Cond::Changed { delta, pct } => {
                let base = match before {
                    Some(b) => b.shot,
                    None => self.shoot(src, rect)?,
                };
                lacked = Some(base.t0_ns);
                let total = base.img.width * base.img.height;
                let need = ((*pct / 100.0) * total as f64).ceil().max(16.0).min(total as f64) as usize;
                let mut last = base;
                loop {
                    src.wake(&last, deadline.min(Instant::now() + Duration::from_millis(250)));
                    let shot = self.shoot(src, rect)?;
                    // Against the baseline, which `last` keeps while its time moves on with each capture.
                    let (n, _) = crate::image::moved(&last.img, &shot.img, *delta);
                    if n >= need {
                        return Ok(Proof { lacked, had: shot.t_ns + shot.late_ns, timing: true });
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::timeout(format!(
                            "pixels changed: {n} of the region's {total} pixels moved by more than {delta} in {:.1} s, and it waits for {need} ({pct}%, at least 16)",
                            t.as_secs_f64()
                        ))
                        .hint("check the region (page.query({ image }) in setup finds where things are), or wait for an image with until: { image }"));
                    }
                    lacked = Some(shot.t0_ns);
                    last = Shot { img: last.img, ..shot };
                }
            }
            Cond::Stable { ms } => {
                let mut last = match before {
                    Some(b) => b.shot,
                    None => self.shoot(src, rect)?,
                };
                // Held since the frame that last changed it, and never since before the wait began (a
                // compositor's frame of a window that held still for a while is old, and the input's own
                // frame may still be on its way).
                let hold = ms * 1_000_000;
                let mut since = last.t_ns.max(crate::host::now_ns());
                loop {
                    let left = Duration::from_nanos((since + hold).saturating_sub(crate::host::now_ns()));
                    match src.seq() {
                        // Frames as events: sleep until the next one, or until it held long enough; no frame
                        // means the pixels held, with nothing read.
                        Some(_) => {
                            src.wake(&last, deadline.min(Instant::now() + left));
                            if src.seq() == Some(last.seq) {
                                if crate::host::now_ns() >= since + hold {
                                    return Ok(Proof { lacked: None, had: since, timing: false });
                                }
                                if Instant::now() >= deadline {
                                    return Err(unstable(*ms, t));
                                }
                                continue;
                            }
                        }
                        None => src.wake(&last, deadline.min(Instant::now() + left.min(Duration::from_millis(50)))),
                    }
                    let shot = self.shoot(src, rect)?;
                    let (_, max) = crate::image::moved(&last.img, &shot.img, 8);
                    if max > 8 {
                        since = shot.t_ns.max(since);
                    } else if crate::host::now_ns() >= since + hold {
                        return Ok(Proof { lacked: None, had: since, timing: false });
                    }
                    if Instant::now() >= deadline {
                        return Err(unstable(*ms, t));
                    }
                    last = shot;
                }
            }
        }
    }
}

fn unstable(ms: u64, t: Duration) -> Error {
    Error::timeout(format!("pixels stable: the region never held still for {ms} ms in {:.1} s", t.as_secs_f64()))
        .hint("an animation, a caret or a spinner in the region keeps it moving: narrow the region, or wait for an image")
}

/// A capture before an input, and whether it already had what the wait after it wants.
pub(crate) struct Before {
    shot: Shot,
    had: bool,
}

/// `resetPixels` (SPEC §9.6): what a run starts from, in pixels, for an app whose state shows in no
/// accessibility tree: a region (default all of the area), parts of it left out (a clock, a spinner), and
/// how far a channel may move and still be the same (default 8).
#[derive(Clone, Debug)]
pub(crate) struct StartPixels {
    region: Option<Region>,
    ignore: Vec<Region>,
    pub tolerance: u8,
}

impl StartPixels {
    /// `true`, or `{ region?, ignore?, tolerance? }`; None for `false` or nothing.
    pub(crate) fn parse(v: &Value) -> Result<Option<StartPixels>> {
        match v {
            Value::Null | Value::Bool(false) => Ok(None),
            Value::Bool(true) => Ok(Some(StartPixels { region: None, ignore: Vec::new(), tolerance: 8 })),
            Value::Object(o) => {
                if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "region" | "ignore" | "tolerance")) {
                    return Err(Error::usage(format!("resetPixels takes region, ignore and tolerance, not {k}")));
                }
                let ignore = match o.get("ignore") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(a)) => a.iter().map(|r| Region::parse(r)?.ok_or_else(|| Error::usage("resetPixels.ignore: a list of regions"))).collect::<Result<_>>()?,
                    Some(x) => return Err(Error::usage(format!("resetPixels.ignore is a list of regions, not {x}"))),
                };
                let tolerance = o.get("tolerance").and_then(Value::as_f64).unwrap_or(8.0);
                if !(0.0..255.0).contains(&tolerance) {
                    return Err(Error::usage(format!("resetPixels.tolerance is 0 to 254, not {tolerance}")));
                }
                Ok(Some(StartPixels { region: Region::parse(o.get("region").unwrap_or(&Value::Null))?, ignore, tolerance: tolerance as u8 }))
            }
            x => Err(Error::usage(format!("resetPixels is true or {{ region?, ignore?, tolerance? }}, not {x}"))),
        }
    }

    /// The region as it shows now, its ignored parts blanked; outside every timed span.
    pub(crate) fn read(&self, src: &dyn Source) -> Result<Image> {
        let size = src.size()?;
        let rect = area(self.region, size, src.what())?;
        let mut shot = src.grab(rect)?;
        for r in &self.ignore {
            let [x, y, w, h] = r.rect(size);
            let k = shot.scale;
            let (x0, y0) = (((x - rect[0]) * k).floor().max(0.0) as usize, ((y - rect[1]) * k).floor().max(0.0) as usize);
            let (x1, y1) = ((((x + w - rect[0]) * k).ceil().max(0.0) as usize).min(shot.img.width), (((y + h - rect[1]) * k).ceil().max(0.0) as usize).min(shot.img.height));
            for row in y0..y1 {
                for col in x0..x1 {
                    let i = (row * shot.img.width + col) * 4;
                    shot.img.rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        Ok(shot.img)
    }
}
