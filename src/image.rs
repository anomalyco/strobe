//! Image anchors and pixel waits (SPEC §9.6): where a template image shows in a capture of the
//! target, and whether a region's pixels changed or held still. Plain Rust over `png::Image`.
//!
//! Matching is zero-mean normalized cross-correlation (ZNCC) of luma, BT.601 in integers
//! (`(77 R + 150 G + 29 B) >> 8`), over the template's opaque pixels (alpha ≥ 128): first on a grid
//! 4× coarser (2× or 1× for a small template), whose best local peaks (at most 32, each at least the
//! threshold less 0.12) are then searched at full resolution 4 pixels each way. A full-resolution
//! peak at or over the threshold is a match when its colours agree too (ZNCC ignores gain and
//! offset, so a dimmed or recoloured copy would pass): the mean absolute difference per RGB channel
//! at most (1 − threshold) · 510 + 8. Matches that overlap by more than 0.3 (intersection over
//! union) keep the best one; they are ordered by score, then top to bottom, then left to right.

use crate::model::{Error, Result};
use crate::png::Image;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The least opaque pixels a template may have.
const MIN_OPAQUE: usize = 16;

/// Coarse peaks searched at full resolution, at most.
const PEAKS: usize = 32;

/// How far under the threshold a coarse peak may score and still be searched.
const COARSE_SLACK: f64 = 0.12;

/// Full-resolution search around a coarse peak, each way (pixels).
const REFINE: i64 = 4;

/// Two matches overlapping more than this (intersection over union) are one.
const NMS_IOU: f64 = 0.3;

/// BT.601 luma in integers.
fn luma(p: &[u8]) -> u8 {
    ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as u8
}

/// A bench file's image, as read: its pixels and how many of them make one device-independent pixel
/// (2 for an image exported at @2x).
pub struct Template {
    pub path: String,
    img: Image,
    per_dip: f64,
    /// The image resampled to each capture scale it met (per mille), once per block.
    scaled: RefCell<HashMap<u32, Rc<Scaled>>>,
}

/// The template at one capture's scale: luma (zero-mean over the mask, 0 outside it), RGB and mask.
struct Scaled {
    w: usize,
    h: usize,
    /// Luma less its mean over the mask; 0 outside the mask.
    t: Vec<f32>,
    /// Sum of `t²`, its square root.
    norm: f64,
    rgb: Vec<[u8; 3]>,
    mask: Vec<bool>,
    n: usize,
    full: bool,
    /// The same at the coarse level, and its factor.
    coarse: Option<Box<Scaled>>,
    factor: usize,
}

/// One place the template shows, in the capture's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
}

impl Hit {
    fn iou(&self, o: &Hit) -> f64 {
        let ix = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let iy = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if ix <= 0.0 || iy <= 0.0 {
            return 0.0;
        }
        let i = ix * iy;
        i / (self.w * self.h + o.w * o.h - i)
    }
}

/// What a search found: the matches, and the best place it scored even when none matched (for
/// `AnchorNotFound`), with why that place was no match.
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub best: Option<Hit>,
    /// The best place's colours disagreed: its mean RGB difference per channel.
    pub colour: Option<f64>,
}

impl Template {
    /// Reads `path` (a PNG) as an anchor: at least 16 opaque pixels (alpha 128 or more) whose luma is not
    /// flat (standard deviation 2 or more), else a usage error: such an image matches anywhere.
    pub fn load(path: &str, per_dip: f64) -> Result<Template> {
        if !(per_dip.is_finite() && per_dip > 0.0) {
            return Err(Error::usage(format!("image \"{path}\": scale is the image's pixels per device-independent pixel (1, or 2 for @2x), not {per_dip}")));
        }
        let bytes = std::fs::read(path).map_err(|e| Error::usage(format!("image \"{path}\": {e}")).hint("an image path is relative to the bench file's folder"))?;
        let img = crate::png::read(&bytes).map_err(|e| Error::usage(format!("image \"{path}\": {}", e.message)))?;
        let opaque: Vec<u8> = img.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128).map(|p| luma(p)).collect();
        if opaque.len() < MIN_OPAQUE {
            return Err(Error::usage(format!("image \"{path}\": {} opaque pixels (alpha 128 or more); an anchor needs at least {MIN_OPAQUE} to be told apart", opaque.len())));
        }
        let mean = opaque.iter().map(|&l| l as f64).sum::<f64>() / opaque.len() as f64;
        let sd = (opaque.iter().map(|&l| (l as f64 - mean).powi(2)).sum::<f64>() / opaque.len() as f64).sqrt();
        if sd < 2.0 {
            return Err(Error::usage(format!("image \"{path}\": its opaque pixels are one flat shade (luma deviation {sd:.1}), which matches anywhere that shade is"))
                .hint("crop the anchor to something with an edge or text in it"));
        }
        Ok(Template { path: path.to_string(), img, per_dip, scaled: RefCell::new(HashMap::new()) })
    }

    /// The template's size in device-independent pixels.
    pub fn dip_size(&self) -> (f64, f64) {
        (self.img.width as f64 / self.per_dip, self.img.height as f64 / self.per_dip)
    }

    /// The template at `scale` capture pixels per device-independent pixel.
    fn at(&self, scale: f64) -> Result<Rc<Scaled>> {
        let key = (scale * 1000.0).round() as u32;
        if let Some(s) = self.scaled.borrow().get(&key) {
            return Ok(s.clone());
        }
        let k = scale / self.per_dip;
        let (w, h) = (((self.img.width as f64 * k).round() as usize).max(1), ((self.img.height as f64 * k).round() as usize).max(1));
        let img = if (w, h) == (self.img.width, self.img.height) { Image { width: w, height: h, rgba: self.img.rgba.clone() } } else { resample(&self.img, w, h) };
        let s =
            Rc::new(Scaled::of(&img, true).ok_or_else(|| {
                Error::usage(format!("image \"{}\": at the capture's scale ({scale:.2} pixels per device-independent pixel) it keeps too few opaque pixels or no contrast", self.path))
            })?);
        self.scaled.borrow_mut().insert(key, s.clone());
        Ok(s)
    }

    /// Every place the template shows in `frame` (taken at `scale` pixels per device-independent pixel)
    /// inside `area` (frame pixels: x, y, width, height; None for all of it), at `threshold`.
    pub fn find(&self, frame: &Image, scale: f64, area: Option<[usize; 4]>, threshold: f64) -> Result<Found> {
        let t = self.at(scale)?;
        let [ax, ay, aw, ah] = area.unwrap_or([0, 0, frame.width, frame.height]);
        let (ax, ay) = (ax.min(frame.width), ay.min(frame.height));
        let (aw, ah) = (aw.min(frame.width - ax), ah.min(frame.height - ay));
        if t.w > aw || t.h > ah {
            return Ok(Found::default());
        }
        let plane = Plane::of(frame, [ax, ay, aw, ah]);
        // The coarse level: every place the coarse template fits, its local peaks.
        let mut peaks: Vec<(f64, usize, usize)> = Vec::new();
        match &t.coarse {
            Some(c) => {
                let f = t.factor;
                let cp = plane.shrink(f);
                let scores = cp.scores(c);
                let (cw, ch) = (cp.w + 1 - c.w, cp.h + 1 - c.h);
                for y in 0..ch {
                    for x in 0..cw {
                        let s = scores[y * cw + x];
                        let local = (y.saturating_sub(1)..(y + 2).min(ch)).all(|yy| (x.saturating_sub(1)..(x + 2).min(cw)).all(|xx| scores[yy * cw + xx] <= s));
                        if local {
                            peaks.push((s, x * f, y * f));
                        }
                    }
                }
            }
            None => {
                // Small enough to search at full resolution everywhere.
                let scores = plane.scores(&t);
                let (cw, ch) = (plane.w + 1 - t.w, plane.h + 1 - t.h);
                for y in 0..ch {
                    for x in 0..cw {
                        let s = scores[y * cw + x];
                        let local = (y.saturating_sub(1)..(y + 2).min(ch)).all(|yy| (x.saturating_sub(1)..(x + 2).min(cw)).all(|xx| scores[yy * cw + xx] <= s));
                        if local {
                            peaks.push((s, x, y));
                        }
                    }
                }
            }
        }
        peaks.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
        // The best peak is always searched, so a miss can say where the nearest thing was.
        let floor = threshold - COARSE_SLACK;
        let searched: Vec<(f64, usize, usize)> = peaks.iter().enumerate().filter(|(i, p)| *i == 0 || p.0 >= floor).map(|(_, p)| *p).take(PEAKS).collect();
        let (maxx, maxy) = ((plane.w - t.w) as i64, (plane.h - t.h) as i64);
        let mut found = Found::default();
        let mut cands: Vec<Hit> = Vec::new();
        for &(_, px, py) in &searched {
            let (mut best, mut at) = (f64::NEG_INFINITY, (0usize, 0usize));
            let reach = if t.coarse.is_some() { REFINE } else { 0 };
            for y in (py as i64 - reach).max(0)..=(py as i64 + reach).min(maxy) {
                for x in (px as i64 - reach).max(0)..=(px as i64 + reach).min(maxx) {
                    let s = plane.zncc(&t, x as usize, y as usize);
                    if s > best {
                        (best, at) = (s, (x as usize, y as usize));
                    }
                }
            }
            if !best.is_finite() {
                continue;
            }
            let hit = Hit { x: (ax + at.0) as f64, y: (ay + at.1) as f64, w: t.w as f64, h: t.h as f64, score: best };
            let colour = (best >= threshold).then(|| colour_delta(frame, &t, ax + at.0, ay + at.1));
            let ok = colour.is_some_and(|d| d <= (1.0 - threshold) * 510.0 + 8.0);
            if ok {
                cands.push(hit);
            }
            if found.best.is_none_or(|b| hit.score > b.score) {
                found.best = Some(hit);
                found.colour = colour.filter(|_| !ok);
            }
        }
        cands.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.y.total_cmp(&b.y)).then(a.x.total_cmp(&b.x)));
        for c in cands {
            if !found.hits.iter().any(|h| h.iou(&c) > NMS_IOU) {
                found.hits.push(c);
            }
        }
        Ok(found)
    }
}

impl Scaled {
    /// From an RGBA image; with `coarse`, its coarse level too. None when too few pixels are opaque or
    /// their luma is flat.
    fn of(img: &Image, coarse: bool) -> Option<Scaled> {
        let (w, h) = (img.width, img.height);
        let px = img.rgba.as_chunks::<4>().0;
        let mask: Vec<bool> = px.iter().map(|p| p[3] >= 128).collect();
        let n = mask.iter().filter(|m| **m).count();
        if n < 4 {
            return None;
        }
        let l: Vec<f32> = px.iter().map(|p| luma(p) as f32).collect();
        let mean = l.iter().zip(&mask).filter(|x| *x.1).map(|x| *x.0 as f64).sum::<f64>() / n as f64;
        let t: Vec<f32> = l.iter().zip(&mask).map(|(v, m)| if *m { (*v as f64 - mean) as f32 } else { 0.0 }).collect();
        let norm = t.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
        if norm < 1e-6 {
            return None;
        }
        let factor = [4usize, 2].into_iter().find(|f| w / f >= 4 && h / f >= 4).filter(|_| coarse).unwrap_or(1);
        let coarse = (factor > 1).then(|| {
            // A coarse cell is opaque when most of its pixels are (a thin stroke of a masked template
            // stays), its colour the mean of its opaque pixels only.
            let (cw, ch) = (w / factor, h / factor);
            let mut rgba = Vec::with_capacity(cw * ch * 4);
            for cy in 0..ch {
                for cx in 0..cw {
                    let (mut sum, mut opaque) = ([0u32; 3], 0u32);
                    for y in cy * factor..(cy + 1) * factor {
                        for x in cx * factor..(cx + 1) * factor {
                            let p = &px[y * w + x];
                            if p[3] >= 128 {
                                opaque += 1;
                                for c in 0..3 {
                                    sum[c] += p[c] as u32;
                                }
                            }
                        }
                    }
                    let k = opaque.max(1);
                    let most = 2 * opaque > (factor * factor) as u32;
                    rgba.extend_from_slice(&[(sum[0] / k) as u8, (sum[1] / k) as u8, (sum[2] / k) as u8, if most { 255 } else { 0 }]);
                }
            }
            Scaled::of(&Image { width: cw, height: ch, rgba }, false).map(Box::new)
        });
        let coarse = coarse.flatten();
        Some(Scaled { w, h, t, norm, rgb: px.iter().map(|p| [p[0], p[1], p[2]]).collect(), full: n == w * h, mask, n, factor: if coarse.is_some() { factor } else { 1 }, coarse })
    }
}

/// A capture's luma, with summed-area tables of it and its square for a full template's windows.
struct Plane {
    w: usize,
    h: usize,
    l: Vec<f32>,
    /// (w + 1) × (h + 1) sums of luma and of luma².
    sum: Vec<f64>,
    sq: Vec<f64>,
}

impl Plane {
    fn of(img: &Image, [ax, ay, aw, ah]: [usize; 4]) -> Plane {
        let mut l = Vec::with_capacity(aw * ah);
        for y in ay..ay + ah {
            for p in img.rgba[(y * img.width + ax) * 4..(y * img.width + ax + aw) * 4].as_chunks::<4>().0 {
                l.push(luma(p) as f32);
            }
        }
        Plane::from_luma(aw, ah, l)
    }

    fn from_luma(w: usize, h: usize, l: Vec<f32>) -> Plane {
        let mut sum = vec![0f64; (w + 1) * (h + 1)];
        let mut sq = vec![0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            let (mut rs, mut rq) = (0f64, 0f64);
            for x in 0..w {
                let v = l[y * w + x] as f64;
                rs += v;
                rq += v * v;
                sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + rs;
                sq[(y + 1) * (w + 1) + x + 1] = sq[y * (w + 1) + x + 1] + rq;
            }
        }
        Plane { w, h, l, sum, sq }
    }

    /// Box-averaged by `f` each way.
    fn shrink(&self, f: usize) -> Plane {
        let (w, h) = (self.w / f, self.h / f);
        let mut l = Vec::with_capacity(w * h);
        for cy in 0..h {
            for cx in 0..w {
                let mut s = 0f32;
                for y in cy * f..(cy + 1) * f {
                    s += self.l[y * self.w + cx * f..y * self.w + (cx + 1) * f].iter().sum::<f32>();
                }
                l.push(s / (f * f) as f32);
            }
        }
        Plane::from_luma(w, h, l)
    }

    fn rect(&self, t: &[f64], x: usize, y: usize, w: usize, h: usize) -> f64 {
        let s = self.w + 1;
        t[(y + h) * s + x + w] - t[y * s + x + w] - t[(y + h) * s + x] + t[y * s + x]
    }

    /// ZNCC of the template with its top left at (x, y); 0 where the capture is flat.
    fn zncc(&self, t: &Scaled, x: usize, y: usize) -> f64 {
        let (mut dot, mut s, mut q) = (0f64, 0f64, 0f64);
        for r in 0..t.h {
            let row = &self.l[(y + r) * self.w + x..(y + r) * self.w + x + t.w];
            let trow = &t.t[r * t.w..(r + 1) * t.w];
            dot += trow.iter().zip(row).map(|(a, b)| a * b).sum::<f32>() as f64;
            if !t.full {
                for (v, m) in row.iter().zip(&t.mask[r * t.w..(r + 1) * t.w]) {
                    if *m {
                        s += *v as f64;
                        q += (*v as f64) * (*v as f64);
                    }
                }
            }
        }
        if t.full {
            s = self.rect(&self.sum, x, y, t.w, t.h);
            q = self.rect(&self.sq, x, y, t.w, t.h);
        }
        let var = q - s * s / t.n as f64;
        if var <= 1e-3 * t.n as f64 {
            return 0.0;
        }
        dot / (t.norm * var.sqrt())
    }

    /// ZNCC at every place the template fits, row by row.
    fn scores(&self, t: &Scaled) -> Vec<f64> {
        let (cw, ch) = (self.w + 1 - t.w, self.h + 1 - t.h);
        let mut out = Vec::with_capacity(cw * ch);
        for y in 0..ch {
            for x in 0..cw {
                out.push(self.zncc(t, x, y));
            }
        }
        out
    }
}

/// The mean absolute difference per RGB channel between the template's opaque pixels and the frame's
/// under them, with its top left at (x, y).
fn colour_delta(frame: &Image, t: &Scaled, x: usize, y: usize) -> f64 {
    let mut d = 0u64;
    for r in 0..t.h {
        for c in 0..t.w {
            if !t.mask[r * t.w + c] {
                continue;
            }
            let i = ((y + r) * frame.width + x + c) * 4;
            let (a, b) = (&frame.rgba[i..i + 3], t.rgb[r * t.w + c]);
            d += (0..3).map(|k| a[k].abs_diff(b[k]) as u64).sum::<u64>();
        }
    }
    d as f64 / (3 * t.n) as f64
}

/// The source pixels each of `dst` output pixels covers when `src` shrinks to `dst`, with how much of
/// each it covers (a fractional box: 2 → 1.5 gives every output 1.33 source pixels, never 1, 1, 2).
fn spans(src: usize, dst: usize) -> Vec<Vec<(usize, f64)>> {
    let k = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            let (a, b) = (i as f64 * k, (i + 1) as f64 * k);
            (a.floor() as usize..(b.ceil() as usize).min(src)).map(|j| (j, b.min(j as f64 + 1.0) - a.max(j as f64))).filter(|(_, w)| *w > 1e-9).collect()
        })
        .collect()
}

/// `img` at `w` × `h`: an area-weighted box filter when it shrinks, bilinear when it grows, both over
/// premultiplied alpha (a transparent pixel's colour never darkens an opaque edge).
fn resample(img: &Image, w: usize, h: usize) -> Image {
    let (sw, sh) = (img.width, img.height);
    let mut rgba = Vec::with_capacity(w * h * 4);
    let px = |x: usize, y: usize| &img.rgba[(y * sw + x) * 4..(y * sw + x) * 4 + 4];
    // Sums of premultiplied colour, alpha and weight, out as straight RGBA.
    let mut put = |c: [f64; 3], a: f64, n: f64| {
        let alpha = if n > 0.0 { a / n } else { 0.0 };
        for v in c {
            rgba.push(if a > 0.0 { (v / a).round().clamp(0.0, 255.0) as u8 } else { 0 });
        }
        rgba.push((alpha * 255.0).round().clamp(0.0, 255.0) as u8);
    };
    if w <= sw && h <= sh {
        let (cols, rows) = (spans(sw, w), spans(sh, h));
        for row in &rows {
            for col in &cols {
                let (mut c, mut a, mut n) = ([0f64; 3], 0f64, 0f64);
                for &(yy, wy) in row {
                    for &(xx, wx) in col {
                        let p = px(xx, yy);
                        let (wt, al) = (wx * wy, p[3] as f64 / 255.0);
                        for i in 0..3 {
                            c[i] += p[i] as f64 * al * wt;
                        }
                        a += al * wt;
                        n += wt;
                    }
                }
                put(c, a, n);
            }
        }
    } else {
        for y in 0..h {
            let fy = ((y as f64 + 0.5) * sh as f64 / h as f64 - 0.5).clamp(0.0, (sh - 1) as f64);
            let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..w {
                let fx = ((x as f64 + 0.5) * sw as f64 / w as f64 - 0.5).clamp(0.0, (sw - 1) as f64);
                let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
                let x1 = (x0 + 1).min(sw - 1);
                let (mut c, mut a) = ([0f64; 3], 0f64);
                for (xx, yy, wt) in [(x0, y0, (1.0 - tx) * (1.0 - ty)), (x1, y0, tx * (1.0 - ty)), (x0, y1, (1.0 - tx) * ty), (x1, y1, tx * ty)] {
                    let p = px(xx, yy);
                    let al = p[3] as f64 / 255.0;
                    for i in 0..3 {
                        c[i] += p[i] as f64 * al * wt;
                    }
                    a += al * wt;
                }
                put(c, a, 1.0);
            }
        }
    }
    Image { width: w, height: h, rgba }
}

/// How two captures of one region differ: the pixels whose largest channel moved by more than
/// `delta`, and the largest move. Captures of different sizes differ everywhere.
pub fn moved(a: &Image, b: &Image, delta: u8) -> (usize, u8) {
    if a.width != b.width || a.height != b.height {
        return (b.width * b.height, 255);
    }
    let (mut n, mut max) = (0usize, 0u8);
    for (p, q) in a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0) {
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
        max = max.max(d);
        if d > delta {
            n += 1;
        }
    }
    (n, max)
}

/// `img`'s part at `[x, y, w, h]` (pixels, clipped to it).
pub fn crop(img: &Image, [x, y, w, h]: [usize; 4]) -> Image {
    let (x, y) = (x.min(img.width), y.min(img.height));
    let (w, h) = (w.min(img.width - x), h.min(img.height - y));
    let mut rgba = Vec::with_capacity(w * h * 4);
    for r in y..y + h {
        rgba.extend_from_slice(&img.rgba[(r * img.width + x) * 4..(r * img.width + x + w) * 4]);
    }
    Image { width: w, height: h, rgba }
}
