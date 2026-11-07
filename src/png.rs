//! 8-bit PNGs, not interlaced (Chromium's screenshots, and the images a bench file's image locators name: RGB,
//! RGBA, grey, grey with alpha, or a palette with its transparency): read for `--pixels` and image anchors, and
//! a diff image written back. Inflate and deflate are flate2's.

use crate::model::{Error, Result};
use std::io::{Read, Write};

pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGBA, row by row.
    pub rgba: Vec<u8>,
}

pub fn read(bytes: &[u8]) -> Result<Image> {
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(Error::msg("not a PNG"));
    }
    let (mut width, mut height, mut channels, mut colour) = (0usize, 0usize, 0usize, 0u8);
    let (mut palette, mut alpha): (Vec<[u8; 3]>, Vec<u8>) = (Vec::new(), Vec::new());
    let mut idat = Vec::new();
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap_or_default()) as usize;
        let kind = &bytes[at + 4..at + 8];
        let data = bytes.get(at + 8..at + 8 + len).ok_or_else(|| Error::msg("truncated PNG"))?;
        match kind {
            b"IHDR" => {
                width = u32::from_be_bytes(data[0..4].try_into().unwrap_or_default()) as usize;
                height = u32::from_be_bytes(data[4..8].try_into().unwrap_or_default()) as usize;
                if data[8] != 8 || data[12] != 0 {
                    return Err(Error::msg("PNG: only 8-bit, non-interlaced images"));
                }
                colour = data[9];
                channels = match colour {
                    0 | 3 => 1,
                    4 => 2,
                    2 => 3,
                    6 => 4,
                    t => return Err(Error::msg(format!("PNG: colour type {t}"))),
                };
            }
            b"PLTE" => palette = data.as_chunks::<3>().0.to_vec(),
            b"tRNS" => alpha = data.to_vec(),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&idat[..]).read_to_end(&mut raw).map_err(|e| Error::msg(format!("PNG: {e}")))?;
    let stride = width * channels;
    if raw.len() < height * (stride + 1) {
        return Err(Error::msg("PNG: image data too short"));
    }
    let mut out = vec![0u8; width * height * 4];
    let mut prev = vec![0u8; stride];
    let mut row = vec![0u8; stride];
    for y in 0..height {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let filter = line[0];
        for x in 0..stride {
            let a = if x >= channels { row[x - channels] as i16 } else { 0 };
            let b = prev[x] as i16;
            let c = if x >= channels { prev[x - channels] as i16 } else { 0 };
            let predictor = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                f => return Err(Error::msg(format!("PNG: filter {f}"))),
            };
            row[x] = line[1 + x].wrapping_add(predictor as u8);
        }
        for x in 0..width {
            let (s, d) = (x * channels, (y * width + x) * 4);
            let px = match colour {
                0 => [row[s], row[s], row[s], 255],
                4 => [row[s], row[s], row[s], row[s + 1]],
                3 => {
                    let i = row[s] as usize;
                    let [r, g, b] = palette.get(i).copied().ok_or_else(|| Error::msg("PNG: a palette index past its palette"))?;
                    [r, g, b, alpha.get(i).copied().unwrap_or(255)]
                }
                2 => [row[s], row[s + 1], row[s + 2], 255],
                _ => [row[s], row[s + 1], row[s + 2], row[s + 3]],
            };
            out[d..d + 4].copy_from_slice(&px);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    Ok(Image { width, height, rgba: out })
}

pub fn write(img: &Image) -> Result<Vec<u8>> {
    let mut raw = Vec::with_capacity(img.height * (img.width * 4 + 1));
    for y in 0..img.height {
        raw.push(0);
        raw.extend_from_slice(&img.rgba[y * img.width * 4..(y + 1) * img.width * 4]);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw)?;
    let idat = z.finish()?;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(img.width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(img.height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(&b"IHDR"[..], &ihdr[..]), (b"IDAT", &idat[..]), (b"IEND", &[][..])] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    Ok(out)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// How two snapshots differ: pixels with a channel more than `tolerance` apart, the largest channel difference,
/// and an image of head dimmed to grey with every such pixel red.
pub struct Diff {
    pub differing: usize,
    pub total: usize,
    pub max_delta: u8,
    pub image: Option<Image>,
}

pub fn diff(base: &Image, head: &Image, tolerance: u8) -> Diff {
    if base.width != head.width || base.height != head.height {
        return Diff { differing: head.width * head.height, total: head.width * head.height, max_delta: 255, image: None };
    }
    let mut differing = 0;
    let mut max_delta = 0u8;
    let mut out = vec![0u8; head.rgba.len()];
    for (i, (b, h)) in base.rgba.as_chunks::<4>().0.iter().zip(head.rgba.as_chunks::<4>().0.iter()).enumerate() {
        let d = (0..4).map(|c| b[c].abs_diff(h[c])).max().unwrap_or(0);
        let o = &mut out[i * 4..i * 4 + 4];
        max_delta = max_delta.max(d);
        if d > tolerance {
            differing += 1;
            o.copy_from_slice(&[255, 0, 0, 255]);
        } else {
            let grey = ((h[0] as u16 + h[1] as u16 + h[2] as u16) / 9) as u8;
            o.copy_from_slice(&[grey, grey, grey, 255]);
        }
    }
    Diff { differing, total: head.width * head.height, max_delta, image: (differing > 0).then_some(Image { width: head.width, height: head.height, rgba: out }) }
}
