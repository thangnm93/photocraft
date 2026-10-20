//! Displacements stored per node of a regular grid, as Photoshop's Liquify smart filter keeps its
//! result: every output pixel shows the source at its own position plus the displacement there,
//! the displacement interpolated bilinearly between the nodes (an inverse map, so the result has
//! no holes). Nothing is solved or replayed: the result depends only on the stored nodes.

use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// A grid of displacements over the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplacementMesh {
    /// Document position of node (0, 0) (pixel (x, y) covers `x..x + 1 × y..y + 1`).
    pub origin: [f64; 2],
    /// Distance between neighbouring nodes along x and y (px).
    pub cell: [f64; 2],
    /// Nodes per row.
    pub cols: usize,
    /// Rows of nodes.
    pub rows: usize,
    /// Per node, row-major: where the output at the node reads the source, relative to it (px).
    pub d: Vec<[f32; 2]>,
}

impl DisplacementMesh {
    /// Whether the mesh can be sampled: at least one node, one displacement per node, positive
    /// finite cells and finite values.
    pub fn is_valid(&self) -> bool {
        self.cols > 0
            && self.rows > 0
            && self.cols.checked_mul(self.rows) == Some(self.d.len())
            && self.cell.iter().all(|c| c.is_finite() && *c > 0.0)
            && self.origin.iter().all(|o| o.is_finite())
            && self.d.iter().flatten().all(|v| v.is_finite())
    }

    /// The displacement at document point `(x, y)`: bilinear between the four nodes around it,
    /// the nearest edge's past the outermost nodes. Only for a valid mesh.
    pub fn at(&self, x: f64, y: f64) -> [f64; 2] {
        // Grid coordinates, clamped to the nodes; the cell holding them and the weights inside it.
        let axis = |p: f64, o: f64, c: f64, n: usize| {
            let g = ((p - o) / c).clamp(0.0, (n - 1) as f64);
            let g = if g.is_nan() { 0.0 } else { g };
            let i = (g.floor() as usize).min(n.saturating_sub(2));
            (i, (i + 1).min(n - 1), g - i as f64)
        };
        let (i0, i1, tx) = axis(x, self.origin[0], self.cell[0], self.cols);
        let (j0, j1, ty) = axis(y, self.origin[1], self.cell[1], self.rows);
        let at = |i: usize, j: usize| self.d[j * self.cols + i];
        let (a, b, c, e) = (at(i0, j0), at(i1, j0), at(i0, j1), at(i1, j1));
        let l = |k: usize| (f64::from(a[k]) * (1.0 - tx) + f64::from(b[k]) * tx) * (1.0 - ty) + (f64::from(c[k]) * (1.0 - tx) + f64::from(e[k]) * tx) * ty;
        [l(0), l(1)]
    }
}

/// Farthest from the origin a sample may land (px); anything beyond reads nothing.
const MAX_REACH: f64 = (1u64 << 30) as f64;
/// Most source pixels a tile reads in one block; a tile that would read more (a hostile mesh
/// spreading it across a huge source) reads its samples one by one.
const MAX_FOOTPRINT: i64 = 1 << 20;

/// Resamples `src` through `mesh` inside `area`: each pixel there shows the source at its centre
/// plus the mesh's displacement (bilinear, premultiplied; the source's default pixel, transparent
/// for layers, where it has nothing). Pixels outside `area` are kept, and so are tiles the mesh
/// doesn't move. `None` when the mesh is not valid or `area` is larger than
/// [`crate::puppet::MAX_WARP_PIXELS`].
pub fn warp_displacement(src: &Surface, mesh: &DisplacementMesh, area: Rect) -> Option<Surface> {
    if !mesh.is_valid() || f64::from(area.width()) * f64::from(area.height()) > crate::puppet::MAX_WARP_PIXELS {
        return None;
    }
    let mut out = src.clone();
    if area.is_empty() {
        return Some(out);
    }
    let fmt = src.format();
    let n = fmt.channels();
    let alpha = fmt.alpha;
    let premul = |p: &mut [f32]| {
        if alpha {
            let a = p[n - 1];
            for v in &mut p[..n - 1] {
                *v *= a;
            }
        }
    };
    let mut outside = src.default_pixel();
    premul(&mut outside);
    // Where the source has anything but its default pixel (one pixel more for the bilinear taps).
    let content = src.content_bounds().inflate(1);
    let snap = |t: f64| if (t - t.round()).abs() < 1e-7 { t.round() } else { t };
    let tiles: Vec<Rect> = area.tiles().map(|t| t.rect().intersect(&area)).filter(|r| !r.is_empty()).collect();
    let work = |t: &Rect| -> Option<(Rect, Vec<f32>)> {
        let (w, h) = (t.width() as usize, t.height() as usize);
        // Source position per pixel, in source pixel units (pixel `(x, y)`'s centre is `(x, y)`).
        let mut pos = vec![[f64::NAN; 2]; w * h];
        let mut moved = false;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for y in 0..h {
            let py = f64::from(t.y0) + y as f64 + 0.5;
            for x in 0..w {
                let px = f64::from(t.x0) + x as f64 + 0.5;
                let d = mesh.at(px, py);
                moved |= d[0] != 0.0 || d[1] != 0.0;
                let (u, v) = (snap(px + d[0] - 0.5), snap(py + d[1] - 0.5));
                if u.abs() < MAX_REACH && v.abs() < MAX_REACH {
                    pos[y * w + x] = [u, v];
                    (x0, y0, x1, y1) = (x0.min(u), y0.min(v), x1.max(u), y1.max(v));
                }
            }
        }
        if !moved {
            return None;
        }
        // The source block the samples read, clipped to the source's content.
        let foot = if x0 <= x1 {
            Rect::new(x0.floor() as i32, y0.floor() as i32, x1.floor() as i32 + 2, y1.floor() as i32 + 2).intersect(&content)
        } else {
            Rect::default()
        };
        let block = (!foot.is_empty() && i64::from(foot.width()) * i64::from(foot.height()) <= MAX_FOOTPRINT).then(|| {
            let mut px = src.read_region(foot);
            px.chunks_exact_mut(n).for_each(premul);
            px
        });
        let mut one = vec![0.0f32; n];
        let mut tap = |x: i32, y: i32, out: &mut [f64], k: f64| {
            if k == 0.0 {
                return;
            }
            let v: &[f32] = if !(x >= foot.x0 && x < foot.x1 && y >= foot.y0 && y < foot.y1) {
                &outside
            } else if let Some(b) = &block {
                let i = ((y - foot.y0) as usize * foot.width() as usize + (x - foot.x0) as usize) * n;
                &b[i..i + n]
            } else {
                src.read_pixel(x, y, &mut one);
                premul(&mut one);
                &one
            };
            for (o, s) in out.iter_mut().zip(v) {
                *o += f64::from(*s) * k;
            }
        };
        let mut outp = vec![0.0f32; w * h * n];
        let mut acc = vec![0.0f64; n];
        for (i, p) in pos.iter().enumerate() {
            acc.fill(0.0);
            if p[0].is_nan() {
                acc.iter_mut().zip(&outside).for_each(|(a, o)| *a = f64::from(*o));
            } else {
                let (ix, iy) = (p[0].floor(), p[1].floor());
                let (fx, fy) = (p[0] - ix, p[1] - iy);
                let (ix, iy) = (ix as i32, iy as i32);
                tap(ix, iy, &mut acc, (1.0 - fx) * (1.0 - fy));
                tap(ix + 1, iy, &mut acc, fx * (1.0 - fy));
                tap(ix, iy + 1, &mut acc, (1.0 - fx) * fy);
                tap(ix + 1, iy + 1, &mut acc, fx * fy);
            }
            let o = &mut outp[i * n..(i + 1) * n];
            if alpha {
                let a = acc[n - 1].clamp(0.0, 1.0);
                o[n - 1] = a as f32;
                for c in 0..n - 1 {
                    o[c] = if a > 0.0 { (acc[c] / a).clamp(0.0, 1.0) as f32 } else { 0.0 };
                }
            } else {
                for c in 0..n {
                    o[c] = acc[c] as f32;
                }
            }
        }
        Some((*t, outp))
    };
    #[cfg(not(target_arch = "wasm32"))]
    let done: Vec<(Rect, Vec<f32>)> = {
        use rayon::prelude::*;
        tiles.par_iter().filter_map(work).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let done: Vec<(Rect, Vec<f32>)> = tiles.iter().filter_map(work).collect();
    for (r, v) in done {
        out.write_region(r, &v);
    }
    out.prune();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::{ColorMode, PixelFormat, SampleType};

    const DEPTHS: [SampleType; 3] = [SampleType::U8, SampleType::U16, SampleType::F32];

    /// Opaque content over `r`: red and green ramp with x and y, blue alternating.
    fn content(st: SampleType, r: Rect) -> Surface {
        let mut s = Surface::new(PixelFormat::new(ColorMode::Rgb, st, true));
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                s.write_pixel(x, y, &[(x - r.x0) as f32 / r.width() as f32, (y - r.y0) as f32 / r.height() as f32, 0.25 + 0.5 * ((x + y) % 2) as f32, 1.0]);
            }
        }
        s
    }

    /// A `cols` × `rows` mesh, nodes every 4 px from `origin`, displaced by `f(i, j)`.
    fn mesh(origin: [f64; 2], cols: usize, rows: usize, f: impl Fn(usize, usize) -> [f32; 2]) -> DisplacementMesh {
        let d = (0..rows).flat_map(|j| (0..cols).map(move |i| (i, j))).map(|(i, j)| f(i, j)).collect();
        DisplacementMesh { origin, cell: [4.0, 4.0], cols, rows, d }
    }

    fn max_diff(a: &Surface, b: &Surface, r: Rect) -> f32 {
        a.read_region(r).iter().zip(b.read_region(r)).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn a_still_mesh_leaves_the_pixels_alone() {
        for st in DEPTHS {
            let r = Rect::new(3, 2, 41, 33);
            let src = content(st, r);
            let out = warp_displacement(&src, &mesh([1.5, 0.5], 12, 10, |_, _| [0.0, 0.0]), r.inflate(4)).unwrap();
            assert_eq!(out.content_bounds(), r, "{st:?}");
            assert!(max_diff(&out, &src, r.inflate(4)) <= 1e-6, "{st:?}");
        }
    }

    /// The output reads the source at `p + d`: a uniform displacement of whole pixels moves the
    /// content by `-d`, exactly, inside the area.
    #[test]
    fn a_uniform_displacement_moves_the_content_the_other_way() {
        for st in DEPTHS {
            let r = Rect::new(10, 10, 50, 40);
            let src = content(st, r);
            let area = Rect::new(0, 0, 64, 56);
            let out = warp_displacement(&src, &mesh([0.0, 0.0], 17, 15, |_, _| [3.0, -2.0]), area).unwrap();
            let moved = crate::resample::translate_surface(&src, -3, 2);
            assert_eq!(out.content_bounds(), moved.content_bounds(), "{st:?}");
            assert!(max_diff(&out, &moved, area) <= 1e-6, "{st:?}: {}", max_diff(&out, &moved, area));
        }
    }

    /// Half a pixel along x reads halfway between two source pixels (bilinear).
    #[test]
    fn fractional_displacements_sample_bilinearly() {
        for st in DEPTHS {
            let r = Rect::new(0, 0, 32, 32);
            let src = content(st, r);
            let out = warp_displacement(&src, &mesh([0.0, 0.0], 9, 9, |_, _| [0.5, 0.25]), r).unwrap();
            let (a, b, c, e) = (src.pixel(10, 12), src.pixel(11, 12), src.pixel(10, 13), src.pixel(11, 13));
            let tol = if st == SampleType::U8 { 1.0 / 255.0 } else { 1e-4 };
            for k in 0..3 {
                let want = (a[k] * 0.5 + b[k] * 0.5) * 0.75 + (c[k] * 0.5 + e[k] * 0.5) * 0.25;
                assert!((out.pixel(10, 12)[k] - want).abs() <= tol, "{st:?} channel {k}: {} vs {want}", out.pixel(10, 12)[k]);
            }
        }
    }

    /// Between nodes the displacement is interpolated bilinearly; past the outermost nodes it
    /// stays at theirs.
    #[test]
    fn the_field_is_bilinear_between_nodes_and_clamped_past_them() {
        let m = mesh([2.0, 2.0], 3, 2, |i, j| [(i * 4) as f32, (j * 8) as f32]);
        assert!(m.is_valid());
        assert_eq!(m.at(2.0, 2.0), [0.0, 0.0]);
        assert_eq!(m.at(4.0, 2.0), [2.0, 0.0]);
        assert_eq!(m.at(7.0, 5.0), [5.0, 6.0]);
        // Past the grid on every side: the nearest edge.
        assert_eq!(m.at(-30.0, -1.0), [0.0, 0.0]);
        assert_eq!(m.at(40.0, 9.0), [8.0, 8.0]);
        assert_eq!(m.at(6.0, 100.0), [4.0, 8.0]);
        // A single node displaces everything by it.
        let one = mesh([0.0, 0.0], 1, 1, |_, _| [1.5, -2.5]);
        assert_eq!(one.at(-7.0, 33.0), [1.5, -2.5]);
    }

    /// Only the area is resampled: outside it the source stays, even where the mesh moves things.
    #[test]
    fn pixels_outside_the_area_are_kept() {
        for st in DEPTHS {
            let r = Rect::new(0, 0, 40, 40);
            let src = content(st, r);
            let area = Rect::new(0, 0, 20, 40);
            let out = warp_displacement(&src, &mesh([0.0, 0.0], 11, 11, |_, _| [-6.0, 0.0]), area).unwrap();
            assert!(max_diff(&out, &src, Rect::new(20, 0, 40, 40)) <= 1e-6, "{st:?}");
            // Inside, the left edge reads past the content: transparent.
            assert_eq!(out.pixel(2, 10)[3], 0.0, "{st:?}");
            assert_eq!(out.pixel(10, 10), src.pixel(4, 10), "{st:?}");
        }
    }

    /// Content can be pulled into the area from beyond it, and edges stay antialiased.
    #[test]
    fn content_is_pulled_in_from_anywhere_in_the_source() {
        let st = SampleType::U8;
        let src = content(st, Rect::new(30, 0, 40, 10));
        let out = warp_displacement(&src, &mesh([0.0, 0.0], 3, 3, |_, _| [25.0, 0.0]), Rect::new(0, 0, 10, 10)).unwrap();
        assert_eq!(out.pixel(6, 4), src.pixel(31, 4));
        assert_eq!(out.pixel(4, 4)[3], 0.0);
        // Half a pixel off the content's edge: half covered, colour kept (premultiplied).
        let half = warp_displacement(&src, &mesh([0.0, 0.0], 3, 3, |_, _| [25.5, 0.0]), Rect::new(0, 0, 10, 10)).unwrap();
        let p = half.pixel(4, 4);
        assert!((p[3] - 0.5).abs() <= 1.0 / 255.0, "{p:?}");
        assert!((p[2] - src.pixel(30, 4)[2]).abs() <= 1.0 / 255.0, "{p:?}");
    }

    /// Surfaces without alpha resample too (the default pixel shows past their content).
    #[test]
    fn opaque_surfaces_resample() {
        let fmt = PixelFormat::new(ColorMode::Grayscale, SampleType::F32, false);
        let mut src = Surface::with_default(fmt, &[0.5]);
        for x in 0..16 {
            src.fill_rect(Rect::new(x, 0, x + 1, 16), &[x as f32 / 16.0]);
        }
        let out = warp_displacement(&src, &mesh([0.0, 0.0], 5, 5, |_, _| [2.0, 0.0]), Rect::new(0, 0, 16, 16)).unwrap();
        assert_eq!(out.format(), fmt);
        assert_eq!(out.pixel(3, 3), vec![5.0 / 16.0]);
    }

    /// Hostile meshes and areas are refused, never indexed out of range or allocated unbounded.
    #[test]
    fn invalid_meshes_and_huge_areas_are_refused() {
        let src = content(SampleType::U8, Rect::new(0, 0, 8, 8));
        let ok = mesh([0.0, 0.0], 3, 3, |_, _| [1.0, 0.0]);
        let mut short = ok.clone();
        short.d.pop();
        let mut nan = ok.clone();
        nan.d[4] = [f32::NAN, 0.0];
        let mut inf = ok.clone();
        inf.origin = [f64::INFINITY, 0.0];
        let mut flat = ok.clone();
        flat.cell = [0.0, 4.0];
        let mut negative = ok.clone();
        negative.cell = [4.0, -4.0];
        let empty = DisplacementMesh::default();
        for m in [short, nan, inf, flat, negative, empty] {
            assert!(!m.is_valid(), "{m:?}");
            assert!(warp_displacement(&src, &m, Rect::new(0, 0, 8, 8)).is_none(), "{m:?}");
        }
        assert!(warp_displacement(&src, &ok, Rect::new(-(1 << 20), -(1 << 20), 1 << 20, 1 << 20)).is_none());
        // A displacement far past the source reads nothing, without reading that far.
        let far = mesh([0.0, 0.0], 3, 3, |_, _| [1e30, -1e30]);
        assert!(warp_displacement(&src, &far, Rect::new(0, 0, 8, 8)).unwrap().content_bounds().is_empty());
        // An empty area changes nothing.
        assert_eq!(warp_displacement(&src, &ok, Rect::new(0, 0, 0, 0)).unwrap(), src);
    }
}
