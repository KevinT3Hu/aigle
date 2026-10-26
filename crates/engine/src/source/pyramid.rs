//! A5: area-filtered source pyramid, RGB f16 on the CPU (doc 02 §3, doc 03 §3).
//!
//! Levels are contiguous row-major images. A pyramid that would take more than half of the
//! effective budget is *spilled* (doc 03 §7.3): each level moves into a file under
//! `<cache>/source/` mapped with `mmap`, so the OS pages it in and out; the file is unlinked
//! at once, so it disappears with the mapping (also after a crash).
//!
//! Not tiled on the CPU (doc 03 §4.1 deviation): the GPU reads levels through its own tile
//! atlas (doc 03 §4.2), the CPU kernels address levels directly, and spilling whole levels
//! gives the same out-of-core behaviour. Levels are shared by every view and job of a
//! session.

use std::path::Path;

use aigle_kernels::Rgb;
use aigle_kernels::resample::Rgb2D;
use half::f16;
use rayon::prelude::*;

/// Pixel storage of a level: on the heap, or in a spilled, memory-mapped file.
pub enum Pixels {
    Heap(Vec<[f16; 3]>),
    Mapped(memmap2::MmapMut),
}

impl Pixels {
    pub fn is_mapped(&self) -> bool {
        matches!(self, Pixels::Mapped(_))
    }
}

impl From<Vec<[f16; 3]>> for Pixels {
    fn from(v: Vec<[f16; 3]>) -> Pixels {
        Pixels::Heap(v)
    }
}

impl std::ops::Deref for Pixels {
    type Target = [[f16; 3]];
    fn deref(&self) -> &[[f16; 3]] {
        match self {
            Pixels::Heap(v) => v,
            Pixels::Mapped(m) => bytemuck::cast_slice(&m[..]),
        }
    }
}

impl std::ops::DerefMut for Pixels {
    fn deref_mut(&mut self) -> &mut [[f16; 3]] {
        match self {
            Pixels::Heap(v) => v,
            Pixels::Mapped(m) => bytemuck::cast_slice_mut(&mut m[..]),
        }
    }
}

impl PartialEq for Pixels {
    fn eq(&self, o: &Pixels) -> bool {
        **self == **o
    }
}

/// Cloning a spilled level reads it into the heap.
impl Clone for Pixels {
    fn clone(&self) -> Pixels {
        Pixels::Heap(self.to_vec())
    }
}

/// Move `px` into an unlinked, memory-mapped file in `dir`.
#[allow(unsafe_code)]
fn map_file(dir: &Path, px: &[[f16; 3]]) -> std::io::Result<memmap2::MmapMut> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!(
        "level-{}-{}.f16",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let bytes: &[u8] = bytemuck::cast_slice(px);
    let res = (|| {
        file.set_len(bytes.len().max(1) as u64)?;
        // SAFETY: the file was created exclusively by this process (`create_new`) and is
        // unlinked below before anyone else can open it by name, so no other process or
        // mapping can modify it while this mapping lives.
        let mut m = unsafe {
            memmap2::MmapOptions::new()
                .len(bytes.len())
                .map_mut(&file)?
        };
        m.copy_from_slice(bytes);
        Ok(m)
    })();
    let _ = std::fs::remove_file(&path);
    res
}

/// One pyramid level. Level `L` pixel `i` covers source pixels `[i·2^L, (i+1)·2^L)`.
#[derive(Clone)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub data: Pixels,
}

impl Level {
    pub fn from_f32(width: u32, height: u32, rgb: &[Rgb]) -> Level {
        assert_eq!(rgb.len(), width as usize * height as usize);
        let data: Vec<_> = rgb.par_iter().map(|p| p.map(f16::from_f32)).collect();
        Level {
            width,
            height,
            data: data.into(),
        }
    }

    pub fn bytes(&self) -> u64 {
        self.data.len() as u64 * 6
    }

    /// 2×2 box downsample; odd edges replicate the last row/column.
    fn downsample(&self) -> Level {
        let w = self.width.div_ceil(2);
        let h = self.height.div_ceil(2);
        let mut data = vec![[f16::ZERO; 3]; w as usize * h as usize];
        data.par_chunks_mut(w as usize)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, out) in row.iter_mut().enumerate() {
                    *out = self.down_px(x, y);
                }
            });
        Level {
            width: w,
            height: h,
            data: data.into(),
        }
    }

    /// Pixel `(x, y)` of the 2×2 downsample.
    #[inline]
    fn down_px(&self, x: usize, y: usize) -> [f16; 3] {
        let sw = self.width as usize;
        let (y0, y1) = (2 * y, (2 * y + 1).min(self.height as usize - 1));
        let (x0, x1) = (2 * x, (2 * x + 1).min(sw - 1));
        let at = |yy: usize, xx: usize| self.data[yy * sw + xx];
        let (a, b, c, d) = (at(y0, x0), at(y0, x1), at(y1, x0), at(y1, x1));
        core::array::from_fn(|k| {
            let s = a[k].to_f32() + b[k].to_f32() + c[k].to_f32() + d[k].to_f32();
            f16::from_f32(s * 0.25)
        })
    }
}

impl Rgb2D for Level {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    #[inline]
    fn load(&self, x: u32, y: u32) -> Rgb {
        let p = self.data[y as usize * self.width as usize + x as usize];
        [p[0].to_f32(), p[1].to_f32(), p[2].to_f32()]
    }
}

pub struct Pyramid {
    pub levels: Vec<Level>,
}

/// A full-precision level 0 for 16-bit exports (doc 03 §9): RGB f32, not part of a pyramid.
pub struct Level32 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<Rgb>,
}

impl Level32 {
    pub fn bytes(&self) -> u64 {
        self.data.len() as u64 * 12
    }
}

impl Pyramid {
    /// Build levels down to a long edge of ≤ 64 px.
    pub fn build(level0: Level) -> Pyramid {
        let mut levels = vec![level0];
        while levels.last().is_some_and(|l| l.width.max(l.height) > 64) {
            let next = levels.last().expect("non-empty").downsample();
            levels.push(next);
        }
        Pyramid { levels }
    }

    pub fn bytes(&self) -> u64 {
        self.levels.iter().map(Level::bytes).sum()
    }

    /// Bytes on the heap (not spilled).
    pub fn heap_bytes(&self) -> u64 {
        self.levels
            .iter()
            .filter(|l| !l.data.is_mapped())
            .map(Level::bytes)
            .sum()
    }

    /// Move every level into memory-mapped files in `dir` (doc 03 §7.3). On an I/O error
    /// the remaining levels stay on the heap.
    pub fn spill(&mut self, dir: &Path) -> std::io::Result<()> {
        for l in &mut self.levels {
            if !l.data.is_mapped() {
                l.data = Pixels::Mapped(map_file(dir, &l.data)?);
            }
        }
        Ok(())
    }

    /// Recompute the coarser levels over level-0 rectangles `(x0, y0, x1, y1)` (half-open)
    /// after level 0 changed inside them; the result equals a full rebuild.
    pub fn update(&mut self, rects: &[(u32, u32, u32, u32)]) {
        let mut rects: Vec<_> = rects.to_vec();
        for l in 1..self.levels.len() {
            let (w, h) = (self.levels[l].width, self.levels[l].height);
            let mut next: Vec<_> = rects
                .iter()
                .map(|&(x0, y0, x1, y1)| {
                    (x0 / 2, y0 / 2, x1.div_ceil(2).min(w), y1.div_ceil(2).min(h))
                })
                .collect();
            next.sort_unstable();
            next.dedup();
            let (fine, coarse) = self.levels.split_at_mut(l);
            let src = &fine[l - 1];
            let done: Vec<_> = next
                .par_iter()
                .map(|&(x0, y0, x1, y1)| {
                    let mut v = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            v.push(src.down_px(x as usize, y as usize));
                        }
                    }
                    v
                })
                .collect();
            let dst = &mut coarse[0];
            for (&(x0, y0, x1, y1), v) in next.iter().zip(done) {
                let mut it = v.into_iter();
                for y in y0..y1 {
                    for x in x0..x1 {
                        dst.data[(y * w + x) as usize] = it.next().expect("sized");
                    }
                }
            }
            rects = next;
        }
    }

    /// Coarsest level whose resolution is still ≥ the grid's: `2^-L ≥ scale`
    /// (scale = grid px per level-0 source px).
    pub fn level_for_scale(&self, scale: f64) -> usize {
        let mut l = 0;
        while l + 1 < self.levels.len() && 0.5f64.powi(l as i32 + 1) >= scale {
            l += 1;
        }
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_selects_levels() {
        let rgb = vec![[0.5f32, 0.25, 1.0]; 1000 * 600];
        let p = Pyramid::build(Level::from_f32(1000, 600, &rgb));
        assert_eq!(p.levels[1].width, 500);
        assert!(p.levels.last().unwrap().width <= 64);
        assert_eq!(p.levels[3].load(10, 10), [0.5, 0.25, 1.0]);
        assert_eq!(p.level_for_scale(1.0), 0);
        assert_eq!(p.level_for_scale(0.6), 0);
        assert_eq!(p.level_for_scale(0.5), 1);
        assert_eq!(p.level_for_scale(0.2), 2);
    }

    #[test]
    fn partial_update_matches_rebuild() {
        let (w, h) = (300u32, 200u32);
        let rgb: Vec<Rgb> = (0..w * h)
            .map(|i| [(i % 7) as f32 * 0.1, 0.5, 0.2])
            .collect();
        let mut p = Pyramid::build(Level::from_f32(w, h, &rgb));
        for y in 70..135 {
            for x in 130..199 {
                p.levels[0].data[(y * w + x) as usize] = [f16::from_f32(3.0); 3];
            }
        }
        p.update(&[(128, 64, 199, 128), (128, 128, 199, 135)]);
        let full = Pyramid::build(Level {
            width: w,
            height: h,
            data: p.levels[0].data.clone(),
        });
        for (a, b) in p.levels.iter().zip(&full.levels) {
            assert!(*a.data == *b.data);
        }
    }

    #[test]
    fn spilled_levels_are_identical_and_writable() {
        let (w, h) = (300u32, 200u32);
        let rgb: Vec<Rgb> = (0..w * h)
            .map(|i| [(i % 11) as f32 * 0.1, 0.5, (i % 3) as f32])
            .collect();
        let mut heap = Pyramid::build(Level::from_f32(w, h, &rgb));
        let mut p = Pyramid::build(Level::from_f32(w, h, &rgb));
        let dir = std::env::temp_dir().join(format!("aigle-spill-test-{}", std::process::id()));
        p.spill(&dir).unwrap();
        assert_eq!(p.heap_bytes(), 0);
        assert!(p.levels.iter().all(|l| l.data.is_mapped()));
        for (a, b) in p.levels.iter().zip(&heap.levels) {
            assert!(*a.data == *b.data);
        }
        // The files are unlinked at once.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        // A4 re-runs write into level 0 and update the levels above.
        for q in [&mut p, &mut heap] {
            q.levels[0].data[0] = [f16::from_f32(2.0); 3];
            q.update(&[(0, 0, 1, 1)]);
        }
        assert!(
            p.levels
                .iter()
                .zip(&heap.levels)
                .all(|(a, b)| a.data == b.data)
        );
        assert_ne!(
            p.levels[1].data[0],
            Pyramid::build(Level::from_f32(w, h, &rgb)).levels[1].data[0]
        );
        let _ = std::fs::remove_dir(&dir);
    }
}
