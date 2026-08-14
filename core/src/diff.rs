//! Cheap frame-change detection. Rather than hashing every pixel, we sample a
//! coarse grid of cells and compute the average color of each cell. Two frames
//! are compared by the fraction of cells that changed noticeably. This is far
//! cheaper than a full OCR pass and lets the pipeline skip OCR on static
//! content.

use crate::types::Frame;

/// Cells per side in the sampling grid. 24x24 = 576 samples per frame.
pub const SAMPLE_GRID: u32 = 24;

/// A per-cell byte difference above this triggers "cell changed".
const CELL_CHANGE_DELTA: u8 = 24;

/// Compact, comparable representation of a frame's visual content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSignature {
    cells: Vec<u8>,
}

impl FrameSignature {
    pub fn compute(frame: &Frame) -> Self {
        let cells = sample_grid(frame);
        Self { cells }
    }

    pub fn cells(&self) -> &[u8] {
        &self.cells
    }
}

/// Fraction (0.0..=1.0) of sampled cells that changed between two signatures.
pub fn changed_fraction(a: &FrameSignature, b: &FrameSignature) -> f32 {
    if a.cells.len() != b.cells.len() {
        return 1.0;
    }
    let n = a.cells.len() as f32;
    let mut changed = 0u32;
    for (x, y) in a.cells.iter().zip(b.cells.iter()) {
        if x.abs_diff(*y) >= CELL_CHANGE_DELTA {
            changed += 1;
        }
    }
    changed as f32 / n
}

/// Sample the frame into a `SAMPLE_GRID x SAMPLE_GRID` grid of average colors.
/// Each cell contributes one byte (luminance) so comparisons are fast.
fn sample_grid(frame: &Frame) -> Vec<u8> {
    let w = frame.width.max(1);
    let h = frame.height.max(1);
    let stride = frame.stride();
    let mut out = Vec::with_capacity((SAMPLE_GRID * SAMPLE_GRID) as usize);

    let cell_w = w as f32 / SAMPLE_GRID as f32;
    let cell_h = h as f32 / SAMPLE_GRID as f32;

    for gy in 0..SAMPLE_GRID {
        for gx in 0..SAMPLE_GRID {
            let sx = (gx as f32 * cell_w) as u32;
            let sy = (gy as f32 * cell_h) as u32;
            let ex = (((gx as f32 + 1.0) * cell_w) as u32).min(w);
            let ey = (((gy as f32 + 1.0) * cell_h) as u32).min(h);
            if ex <= sx || ey <= sy {
                out.push(0);
                continue;
            }
            let mut r = 0u64;
            let mut g = 0u64;
            let mut b = 0u64;
            let mut count = 0u64;
            for y in sy..ey {
                let row = &frame.pixels[y as usize * stride..(y as usize + 1) * stride];
                for x in sx..ex {
                    let i = x as usize * 4;
                    b += row[i] as u64;
                    g += row[i + 1] as u64;
                    r += row[i + 2] as u64;
                    count += 1;
                }
            }
            if count == 0 {
                out.push(0);
                continue;
            }
            // Luminance ~ (0.3 R + 0.59 G + 0.11 B)
            let lum =
                (30 * r + 59 * g + 11 * b) / (100 * count);
            out.push(lum as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_filled(w: u32, h: u32, fill: (u8, u8, u8)) -> Frame {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            pixels.push(fill.2);
            pixels.push(fill.1);
            pixels.push(fill.0);
            pixels.push(255);
        }
        Frame {
            width: w,
            height: h,
            pixels,
            timestamp: 0.0,
        }
    }

    #[test]
    fn identical_frames_no_change() {
        let a = frame_filled(800, 600, (200, 150, 100));
        let b = frame_filled(800, 600, (200, 150, 100));
        let sa = FrameSignature::compute(&a);
        let sb = FrameSignature::compute(&b);
        assert_eq!(changed_fraction(&sa, &sb), 0.0);
    }

    #[test]
    fn different_frames_change() {
        let a = frame_filled(800, 600, (200, 150, 100));
        let b = frame_filled(800, 600, (10, 10, 10));
        let sa = FrameSignature::compute(&a);
        let sb = FrameSignature::compute(&b);
        let f = changed_fraction(&sa, &sb);
        assert!(f > 0.9, "fraction {f}");
    }

    #[test]
    fn partial_change_is_measured() {
        let a = frame_filled(400, 300, (100, 100, 100));
        let mut b = a.clone();
        // Paint the top half differently in b.
        for y in 0..150 {
            for x in 0..400 {
                let i = (y * 400 + x) * 4;
                b.pixels[i] = 255;
                b.pixels[i + 1] = 255;
                b.pixels[i + 2] = 255;
            }
        }
        let sa = FrameSignature::compute(&a);
        let sb = FrameSignature::compute(&b);
        let f = changed_fraction(&sa, &sb);
        assert!((f - 0.5).abs() < 0.3, "fraction {f}");

        // A localized 50x50 change in the bottom half is still detected.
        let mut a2 = a.clone();
        for y in 180..230 {
            for x in 200..250 {
                let i = (y * 400 + x) * 4;
                a2.pixels[i] = 0;
                a2.pixels[i + 1] = 255;
                a2.pixels[i + 2] = 255;
            }
        }
        let s1 = FrameSignature::compute(&a);
        let s2 = FrameSignature::compute(&a2);
        assert!(changed_fraction(&s1, &s2) > 0.0);
    }
}
