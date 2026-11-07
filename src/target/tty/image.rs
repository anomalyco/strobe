//! A terminal's grid as pixels, for `--pixels` (§9.9): each cell its colours, its character in the embedded
//! font (`crate::font`) centred in it, box drawing and block elements drawn as shapes. The same grid gives
//! the same pixels on every machine.

use super::{Grid, palette};
use crate::model::{Error, Result};
use resvg::{tiny_skia, usvg};

/// A cell's size in the image (pixels): twice the size the terminal reports, to be read.
const W: f32 = 16.0;
const H: f32 = 32.0;
const FG: (u8, u8, u8) = (229, 229, 229);
const BG: (u8, u8, u8) = (0, 0, 0);

fn rgb(c: vt100::Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => palette(i),
        vt100::Color::Rgb(r, g, b) => (r, g, b),
    }
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// A box-drawing character's arms (up, right, down, left), each 0 none, 1 light, 2 heavy (a double line
/// is drawn heavy); None for one drawn as text.
fn arms(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' | '┄' | '┈' | '╌' => [0, 1, 0, 1],
        '━' | '┅' | '┉' | '╍' | '═' => [0, 2, 0, 2],
        '│' | '┆' | '┊' | '╎' => [1, 0, 1, 0],
        '┃' | '┇' | '┋' | '╏' | '║' => [2, 0, 2, 0],
        '┌' | '╭' => [0, 1, 1, 0],
        '┐' | '╮' => [0, 0, 1, 1],
        '└' | '╰' => [1, 1, 0, 0],
        '┘' | '╯' => [1, 0, 0, 1],
        '├' => [1, 1, 1, 0],
        '┤' => [1, 0, 1, 1],
        '┬' => [0, 1, 1, 1],
        '┴' => [1, 1, 0, 1],
        '┼' => [1, 1, 1, 1],
        '┏' | '╔' => [0, 2, 2, 0],
        '┓' | '╗' => [0, 0, 2, 2],
        '┗' | '╚' => [2, 2, 0, 0],
        '┛' | '╝' => [2, 0, 0, 2],
        '┣' | '╠' => [2, 2, 2, 0],
        '┫' | '╣' => [2, 0, 2, 2],
        '┳' | '╦' => [0, 2, 2, 2],
        '┻' | '╩' => [2, 2, 0, 2],
        '╋' | '╬' => [2, 2, 2, 2],
        '╴' => [0, 0, 0, 1],
        '╵' => [1, 0, 0, 0],
        '╶' => [0, 1, 0, 0],
        '╷' => [0, 0, 1, 0],
        '╸' => [0, 0, 0, 2],
        '╹' => [2, 0, 0, 0],
        '╺' => [0, 2, 0, 0],
        '╻' => [0, 0, 2, 0],
        _ => return None,
    })
}

/// A block element's filled parts of the cell, as (x, y, w, h) fractions, and its opacity; None for
/// another character.
fn block(c: char) -> Option<(Vec<[f32; 4]>, f32)> {
    let quads = |ul: bool, ur: bool, ll: bool, lr: bool| {
        [(ul, [0.0, 0.0]), (ur, [0.5, 0.0]), (ll, [0.0, 0.5]), (lr, [0.5, 0.5])].into_iter().filter(|q| q.0).map(|(_, [x, y])| [x, y, 0.5, 0.5]).collect::<Vec<_>>()
    };
    Some(match c as u32 {
        0x2580 => (vec![[0.0, 0.0, 1.0, 0.5]], 1.0),
        n @ 0x2581..=0x2588 => {
            let f = (n - 0x2580) as f32 / 8.0;
            (vec![[0.0, 1.0 - f, 1.0, f]], 1.0)
        }
        n @ 0x2589..=0x258f => (vec![[0.0, 0.0, (8 - (n - 0x2588)) as f32 / 8.0, 1.0]], 1.0),
        0x2590 => (vec![[0.5, 0.0, 0.5, 1.0]], 1.0),
        n @ 0x2591..=0x2593 => (vec![[0.0, 0.0, 1.0, 1.0]], (n - 0x2590) as f32 / 4.0),
        0x2594 => (vec![[0.0, 0.0, 1.0, 0.125]], 1.0),
        0x2595 => (vec![[0.875, 0.0, 0.125, 1.0]], 1.0),
        0x2596 => (quads(false, false, true, false), 1.0),
        0x2597 => (quads(false, false, false, true), 1.0),
        0x2598 => (quads(true, false, false, false), 1.0),
        0x2599 => (quads(true, false, true, true), 1.0),
        0x259a => (quads(true, false, false, true), 1.0),
        0x259b => (quads(true, true, true, false), 1.0),
        0x259c => (quads(true, true, false, true), 1.0),
        0x259d => (quads(false, true, false, false), 1.0),
        0x259e => (quads(false, true, true, false), 1.0),
        0x259f => (quads(false, true, true, true), 1.0),
        _ => return None,
    })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The grid as an SVG of `cols * W` by `rows * H`.
fn svg(g: &Grid) -> String {
    let (w, h) = (g.cols as f32 * W, g.rows as f32 * H);
    let mut out = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="{w}" height="{h}" fill="{}"/>"#, hex(BG));
    for r in 0..g.rows {
        for c in 0..g.cols {
            let Some(cell) = g.cell(r, c) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (rgb(cell.fgcolor(), FG), rgb(cell.bgcolor(), BG));
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let span = if cell.is_wide() { 2.0 } else { 1.0 };
            let (x, y) = (c as f32 * W, r as f32 * H);
            if bg != BG {
                out += &format!(r#"<rect x="{x}" y="{y}" width="{}" height="{H}" fill="{}"/>"#, W * span, hex(bg));
            }
            let text = cell.contents();
            let mut chars = text.chars();
            let first = chars.next();
            let one = first.filter(|_| chars.next().is_none());
            if let Some(a) = one.and_then(arms) {
                let (cx, cy) = (x + W / 2.0, y + H / 2.0);
                for (i, weight) in a.iter().enumerate().filter(|(_, w)| **w > 0) {
                    let t = if *weight == 2 { 4.0 } else { 2.0 };
                    let (rx, ry, rw, rh) = match i {
                        0 => (cx - t / 2.0, y, t, H / 2.0 + t / 2.0),
                        1 => (cx - t / 2.0, cy - t / 2.0, W / 2.0 + t / 2.0, t),
                        2 => (cx - t / 2.0, cy - t / 2.0, t, H / 2.0 + t / 2.0),
                        _ => (x, cy - t / 2.0, W / 2.0 + t / 2.0, t),
                    };
                    out += &format!(r#"<rect x="{rx}" y="{ry}" width="{rw}" height="{rh}" fill="{}"/>"#, hex(fg));
                }
            } else if let Some((parts, alpha)) = one.and_then(block) {
                for [px, py, pw, ph] in parts {
                    out += &format!(r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" fill-opacity="{alpha}"/>"#, x + px * W, y + py * H, pw * W, ph * H, hex(fg));
                }
            } else if !text.trim().is_empty() {
                out += &format!(
                    r#"<text x="{}" y="{}" font-family="Inter" font-size="22" font-weight="{}" text-anchor="middle" fill="{}">{}</text>"#,
                    x + W * span / 2.0,
                    y + H * 0.72,
                    if cell.bold() { 600 } else { 400 },
                    hex(fg),
                    esc(text)
                );
            }
            if cell.underline() {
                out += &format!(r#"<rect x="{x}" y="{}" width="{}" height="2" fill="{}"/>"#, y + H - 3.0, W * span, hex(fg));
            }
        }
    }
    // A shown cursor, as a hollow box: where the program left it is part of what it shows.
    if g.cursor_on {
        out += &format!(r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{}" stroke-width="2"/>"#, g.cursor.1 as f32 * W + 1.0, g.cursor.0 as f32 * H + 1.0, W - 2.0, H - 2.0, hex(FG));
    }
    out + "</svg>"
}

/// The grid's pixels.
pub fn image(g: &Grid) -> Result<crate::png::Image> {
    let tree = usvg::Tree::from_str(&svg(g), &crate::font::options()).map_err(|e| Error::msg(format!("terminal snapshot: {e}")))?;
    let size = tree.size();
    let (w, h) = (size.width().round() as u32, size.height().round() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or_else(|| Error::msg("terminal snapshot: an empty grid"))?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Ok(crate::png::Image { width: w as usize, height: h as usize, rgba })
}
