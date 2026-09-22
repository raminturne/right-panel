//! Platform-neutral helpers: tiny PNG encoder, base64, tray icon pixels, names.

use std::path::Path;

pub fn display_name(path: &str) -> String {
    let p = Path::new(path.trim_end_matches('/'));
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/* ---- tiny PNG encoder (uncompressed deflate, fine for 64px icons) ---- */
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 { c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 }; }
    }
    !c
}

pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
    for row in rgba.chunks((w * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, ch) in chunks.iter().enumerate() {
        z.push((i == chunks.len() - 1) as u8);
        let l = ch.len() as u16;
        z.extend_from_slice(&l.to_le_bytes());
        z.extend_from_slice(&(!l).to_le_bytes());
        z.extend_from_slice(ch);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw { a = (a + x as u32) % 65521; b = (b + a) % 65521; }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() { s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char); } else { s.push('='); }
        }
    }
    s
}

/// Tray icon: black liquid drop with a white ring, 32×32 RGBA.
pub fn tray_rgba() -> Vec<u8> {
    let mut px = vec![0u8; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let d = (dx * dx + dy * dy).sqrt();
            let i = (y * 32 + x) * 4;
            let edge = (15.5 - d).clamp(0.0, 1.0);
            if edge > 0.0 {
                let ring = (1.0 - ((d - 8.0).abs() - 1.4)).clamp(0.0, 1.0);
                let v = (ring * 255.0) as u8;
                px[i..i + 4].copy_from_slice(&[v, v, v, (edge * 255.0) as u8]);
            }
        }
    }
    px
}

/// Cheap stable hash, used to spot a clipboard image we already have.
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for chunk in bytes.chunks(97) {
        for &b in chunk {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h ^ bytes.len() as u64
}

/// Nearest-neighbour downscale so the UI can show a small preview.
pub fn thumbnail(w: u32, h: u32, rgba: &[u8], max: u32) -> (u32, u32, Vec<u8>) {
    if w <= max && h <= max {
        return (w, h, rgba.to_vec());
    }
    let scale = (max as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        for x in 0..tw {
            let src = ((y * h / th) as usize * w as usize + (x * w / tw) as usize) * 4;
            let dst = ((y * tw + x) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    (tw, th, out)
}
