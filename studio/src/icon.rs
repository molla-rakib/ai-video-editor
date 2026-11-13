//! The app icon: the motion logo from the title bar (lime-to-mint tile, play shape,
//! speed trails), rasterized at any size. Plain std only, because `build.rs` also
//! uses this file to embed the Windows `.ico`.

/// Sizes in the Windows `.ico` and the Linux icon theme.
pub const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

const ACCENT: [f32; 3] = [0xD6 as f32 / 255.0, 1.0, 0.0];
const BRAND_2: [f32; 3] = [0x2E as f32 / 255.0, 0xF2 as f32 / 255.0, 0xA2 as f32 / 255.0];
const INK: [f32; 3] = [0x0A as f32 / 255.0, 0x0B as f32 / 255.0, 0x07 as f32 / 255.0];

/// Signed distance to a rounded box centered at (cx, cy) with half size (hw, hh).
fn round_box(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = ((x - cx).abs() - hw + r, (y - cy).abs() - hh + r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Signed distance to the play triangle (positive outside).
fn play(x: f32, y: f32) -> f32 {
    let pts: [(f32, f32); 3] = [(0.42, 0.27), (0.78, 0.5), (0.42, 0.73)];
    let mut d = f32::MIN;
    for i in 0..3 {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[(i + 1) % 3];
        // Outward normal of edge a→b for a clockwise triangle in y-down space.
        let (nx, ny) = (by - ay, ax - bx);
        let len = (nx * nx + ny * ny).sqrt();
        d = d.max(((x - ax) * nx + (y - ay) * ny) / len);
    }
    d
}

/// Straight-alpha RGBA at `size`×`size`, 4×4 supersampled.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    const N: u32 = 4;
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..N {
                for sx in 0..N {
                    // Sample position in 0..1 units of the icon.
                    let x = (px as f32 + (sx as f32 + 0.5) / N as f32) / s;
                    let y = (py as f32 + (sy as f32 + 0.5) / N as f32) / s;
                    if round_box(x, y, 0.5, 0.5, 0.5, 0.5, 0.3) > 0.0 {
                        continue;
                    }
                    // Tile: diagonal gradient.
                    let t = ((x + y) / 2.0).clamp(0.0, 1.0);
                    let mut c = [ACCENT[0] + (BRAND_2[0] - ACCENT[0]) * t, ACCENT[1] + (BRAND_2[1] - ACCENT[1]) * t, ACCENT[2] + (BRAND_2[2] - ACCENT[2]) * t];
                    let over = |c: &mut [f32; 3], a: f32| {
                        for k in 0..3 {
                            c[k] = c[k] * (1.0 - a) + INK[k] * a;
                        }
                    };
                    // Speed trails behind the play shape.
                    for (i, (cy, len)) in [(0.36f32, 0.20f32), (0.5, 0.28), (0.64, 0.20)].into_iter().enumerate() {
                        if round_box(x, y, 0.40 - len / 2.0, cy, len / 2.0, 0.035, 0.035) <= 0.0 {
                            over(&mut c, if i == 1 { 0.85 } else { 0.5 });
                        }
                    }
                    if play(x, y) <= 0.0 {
                        over(&mut c, 1.0);
                    }
                    for k in 0..3 {
                        acc[k] += c[k];
                    }
                    acc[3] += 1.0;
                }
            }
            let i = ((py * size + px) * 4) as usize;
            if acc[3] > 0.0 {
                for k in 0..3 {
                    out[i + k] = (acc[k] / acc[3] * 255.0).round() as u8;
                }
                out[i + 3] = (acc[3] / (N * N) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}

/// A Windows `.ico` with every size up to 256 as 32-bit bitmaps.
pub fn ico() -> Vec<u8> {
    let sizes: Vec<u32> = SIZES.iter().copied().filter(|&s| s <= 256).collect();
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp(s)).collect();
    let mut out = Vec::new();
    out.extend([0u16, 1, sizes.len() as u16].iter().flat_map(|v| v.to_le_bytes()));
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend([dim, dim, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((img.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    out
}

/// One `.ico` entry: BITMAPINFOHEADER, bottom-up BGRA pixels, then the (empty) AND mask.
fn bmp(size: u32) -> Vec<u8> {
    let px = rgba(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    for v in [40u32, size, size * 2] {
        out.extend(v.to_le_bytes());
    }
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    for v in [0u32, size * size * 4 + mask_row * size, 0, 0, 0, 0] {
        out.extend(v.to_le_bytes());
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend([px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    out.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_a_transparent_corner_and_a_dark_play_shape() {
        let px = rgba(64);
        let at = |x: u32, y: u32| &px[((y * 64 + x) * 4) as usize..][..4];
        assert_eq!(at(0, 0)[3], 0, "rounded corner is transparent");
        assert!(at(38, 32)[0] < 40 && at(38, 32)[3] == 255, "the play shape is dark ink");
        assert!(at(32, 6)[1] > 200, "the tile is bright lime/mint");
    }

    #[test]
    fn ico_lists_every_size() {
        let ico = ico();
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]) as usize, SIZES.iter().filter(|&&s| s <= 256).count());
    }
}
