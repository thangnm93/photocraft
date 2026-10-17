//! Mesh deformations stored already solved: an original and a deformed position for every vertex
//! of a triangle mesh, as Photoshop's Puppet Warp smart filter keeps its result. Rendering is
//! piecewise affine (each triangle of the original mesh is mapped onto the same triangle of the
//! deformed one, through [`crate::warp::warp_triangles`]), so nothing is solved again and the
//! result depends only on the stored vertices.
//!
//! Only the mesh area is drawn: pixels outside every triangle are dropped, as Photoshop shows a
//! puppet-warped layer. Where the deformed mesh folds over itself, later triangles draw over
//! earlier ones.

use photocraft_color::PixelFormat;
use photocraft_raster::Surface;

use crate::transform::Interp;

/// A triangle mesh and its deformation, in document pixels (pixel `(x, y)` covers
/// `x..x + 1 × y..y + 1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SolvedMesh {
    /// Where each vertex was before the deformation.
    pub original: Vec<[f64; 2]>,
    /// Where each vertex is after it (same length and order as `original`).
    pub deformed: Vec<[f64; 2]>,
    /// Vertex indices, in draw order.
    pub tris: Vec<[usize; 3]>,
}

impl SolvedMesh {
    /// Both vertex lists have the same length, every coordinate is finite and every triangle names
    /// existing vertices.
    pub fn is_valid(&self) -> bool {
        self.original.len() == self.deformed.len()
            && self.original.iter().chain(&self.deformed).flatten().all(|v| v.is_finite())
            && self.tris.iter().flatten().all(|&i| i < self.original.len())
    }

    /// Moves both the original and the deformed mesh through `f` (document → document): the
    /// deformation follows content that moved since it was made.
    pub fn map(&mut self, f: impl Fn(f64, f64) -> (f64, f64)) {
        for p in self.original.iter_mut().chain(self.deformed.iter_mut()) {
            let (x, y) = f(p[0], p[1]);
            *p = [x, y];
        }
    }

    /// The bounds `[x0, y0, x1, y1]` of the deformed mesh, `None` without vertices.
    fn deformed_bounds(&self) -> Option<[f64; 4]> {
        let first = self.deformed.first()?;
        Some(self.deformed.iter().fold([first[0], first[1], first[0], first[1]], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]))
    }
}

/// Deforms `src` by a solved mesh: the pixels under each original triangle land on its deformed
/// triangle, resampled with `interp`; pixels outside the mesh are dropped. The result has `src`'s
/// format with alpha. `None` when the mesh is not valid ([`SolvedMesh::is_valid`]) or its
/// deformed bounds span more than [`crate::puppet::MAX_WARP_PIXELS`].
pub fn warp_solved(src: &Surface, mesh: &SolvedMesh, interp: Interp) -> Option<Surface> {
    if !mesh.is_valid() {
        return None;
    }
    let Some([x0, y0, x1, y1]) = mesh.deformed_bounds() else {
        let fmt = src.format();
        return Some(Surface::new(PixelFormat::new(fmt.mode, fmt.sample, true)));
    };
    let area = (x1 - x0 + 1.0) * (y1 - y0 + 1.0);
    if area.is_nan() || area > crate::puppet::MAX_WARP_PIXELS {
        return None;
    }
    let verts: Vec<([f64; 2], [f64; 2])> = mesh.deformed.iter().copied().zip(mesh.original.iter().copied()).collect();
    Some(crate::warp::warp_triangles(src, src.content_bounds(), &verts, &mesh.tris, interp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::Homography;
    use photocraft_color::{ColorMode, SampleType};
    use photocraft_geom::Rect;

    const DEPTHS: [SampleType; 3] = [SampleType::U8, SampleType::U16, SampleType::F32];

    /// Opaque content over `r`, a different colour per pixel (smooth, so resampling stays near).
    fn content(st: SampleType, r: Rect) -> Surface {
        let mut s = Surface::new(PixelFormat::new(ColorMode::Rgb, st, true));
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                s.write_pixel(x, y, &[(x - r.x0) as f32 / r.width() as f32, (y - r.y0) as f32 / r.height() as f32, 0.25 + 0.5 * ((x + y) % 2) as f32, 1.0]);
            }
        }
        s
    }

    /// A grid mesh over `r` (`n` × `n` cells, two triangles each), deformed by `f`.
    fn grid(r: [f64; 4], n: usize, f: impl Fn([f64; 2]) -> [f64; 2]) -> SolvedMesh {
        let mut m = SolvedMesh::default();
        for j in 0..=n {
            for i in 0..=n {
                let p = [r[0] + (r[2] - r[0]) * i as f64 / n as f64, r[1] + (r[3] - r[1]) * j as f64 / n as f64];
                m.original.push(p);
                m.deformed.push(f(p));
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = j * (n + 1) + i;
                m.tris.push([a, a + 1, a + n + 2]);
                m.tris.push([a, a + n + 2, a + n + 1]);
            }
        }
        m
    }

    fn max_diff(a: &Surface, b: &Surface, r: Rect) -> f32 {
        a.read_region(r).iter().zip(b.read_region(r)).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn identity_mesh_leaves_the_content_alone() {
        for st in DEPTHS {
            let r = Rect::new(4, 3, 40, 30);
            let src = content(st, r);
            let m = grid([4.0, 3.0, 40.0, 30.0], 5, |p| p);
            let out = warp_solved(&src, &m, Interp::Bicubic).unwrap();
            assert_eq!(out.content_bounds(), r, "{st:?}");
            assert!(max_diff(&out, &src, r.inflate(2)) <= 1e-6, "{st:?}: {}", max_diff(&out, &src, r));
        }
    }

    #[test]
    fn an_affine_deformation_is_that_affine_transform() {
        for st in DEPTHS {
            let r = Rect::new(10, 10, 50, 40);
            let src = content(st, r);
            // A whole-pixel move is exact.
            let moved = warp_solved(&src, &grid([10.0, 10.0, 50.0, 40.0], 4, |p| [p[0] + 7.0, p[1] - 3.0]), Interp::Bicubic).unwrap();
            let shifted = crate::resample::translate_surface(&src, 7, -3);
            assert_eq!(moved.content_bounds(), shifted.content_bounds(), "{st:?}");
            assert!(max_diff(&moved, &shifted, shifted.content_bounds()) <= 1e-6, "{st:?}");
            // Scaled, rotated and moved: the same pixels as the projective resampler, inside the
            // content (its edges are cut by the mesh outline rather than antialiased).
            let (c, s) = (0.3f64.cos(), 0.3f64.sin());
            let h = Homography([1.4 * c, -1.4 * s, 30.0, 1.4 * s, 1.4 * c, -5.0, 0.0, 0.0, 1.0]);
            let warped = warp_solved(
                &src,
                &grid([10.0, 10.0, 50.0, 40.0], 6, |p| {
                    let (x, y) = h.apply(p[0], p[1]);
                    [x, y]
                }),
                Interp::Bicubic,
            )
            .unwrap();
            let reference = crate::transform::warp_surface(&src, r, &h, Interp::Bicubic).unwrap();
            let (cx, cy) = h.apply(30.0, 25.0);
            let inner = Rect::new(cx as i32 - 12, cy as i32 - 8, cx as i32 + 12, cy as i32 + 8);
            assert!(max_diff(&warped, &reference, inner) < 2e-3, "{st:?}: {}", max_diff(&warped, &reference, inner));
        }
    }

    /// Two triangles that trade places: each one's pixels end up where the other was.
    #[test]
    fn swapped_triangles_trade_their_pixels() {
        for st in DEPTHS {
            let mut src = Surface::new(PixelFormat::new(ColorMode::Rgb, st, true));
            src.fill_rect(Rect::new(0, 0, 20, 20), &[1.0, 0.0, 0.0, 1.0]);
            src.fill_rect(Rect::new(40, 0, 60, 20), &[0.0, 0.0, 1.0, 1.0]);
            let left = [[0.0, 0.0], [20.0, 0.0], [0.0, 20.0]];
            let right = [[40.0, 0.0], [60.0, 0.0], [40.0, 20.0]];
            let m = SolvedMesh { original: [left, right].concat(), deformed: [right, left].concat(), tris: vec![[0, 1, 2], [3, 4, 5]] };
            let out = warp_solved(&src, &m, Interp::Bilinear).unwrap();
            assert_eq!(out.pixel(4, 4), vec![0.0, 0.0, 1.0, 1.0], "{st:?}");
            assert_eq!(out.pixel(44, 4), vec![1.0, 0.0, 0.0, 1.0], "{st:?}");
            // The other half of each square is outside the mesh.
            assert_eq!(out.pixel(16, 16)[3], 0.0, "{st:?}");
        }
    }

    #[test]
    fn pixels_outside_the_mesh_are_dropped() {
        for st in DEPTHS {
            let src = content(st, Rect::new(0, 0, 40, 40));
            // A mesh over the left half only, left where it is.
            let out = warp_solved(&src, &grid([0.0, 0.0, 20.0, 40.0], 2, |p| p), Interp::Bicubic).unwrap();
            assert_eq!(out.content_bounds(), Rect::new(0, 0, 20, 40), "{st:?}");
            // No triangles: nothing is shown.
            let empty = SolvedMesh { original: vec![[0.0, 0.0]], deformed: vec![[0.0, 0.0]], tris: Vec::new() };
            assert!(warp_solved(&src, &empty, Interp::Bicubic).unwrap().content_bounds().is_empty());
            assert!(warp_solved(&src, &SolvedMesh::default(), Interp::Bicubic).unwrap().content_bounds().is_empty());
        }
    }

    /// Where the deformed mesh folds over itself, the later triangle is on top.
    #[test]
    fn later_triangles_draw_on_top() {
        let mut src = Surface::new(PixelFormat::new(ColorMode::Rgb, SampleType::U8, true));
        src.fill_rect(Rect::new(0, 0, 10, 10), &[1.0, 0.0, 0.0, 1.0]);
        src.fill_rect(Rect::new(30, 0, 40, 10), &[0.0, 1.0, 0.0, 1.0]);
        let a = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let b = [[30.0, 0.0], [40.0, 0.0], [40.0, 10.0], [30.0, 10.0]];
        let on = [[15.0, 0.0], [25.0, 0.0], [25.0, 10.0], [15.0, 10.0]];
        let tris = vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]];
        let m = SolvedMesh { original: [a, b].concat(), deformed: [on, on].concat(), tris };
        assert_eq!(warp_solved(&src, &m, Interp::Bilinear).unwrap().pixel(20, 5), vec![0.0, 1.0, 0.0, 1.0]);
        let mut back = m.clone();
        back.tris.reverse();
        assert_eq!(warp_solved(&src, &back, Interp::Bilinear).unwrap().pixel(20, 5), vec![1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn map_moves_both_meshes() {
        let mut m = grid([0.0, 0.0, 10.0, 10.0], 1, |p| [p[0] + 1.0, p[1]]);
        m.map(|x, y| (x * 2.0, y + 5.0));
        assert_eq!(m.original[3], [20.0, 15.0]);
        assert_eq!(m.deformed[3], [22.0, 15.0]);
    }

    /// Hostile meshes are refused, never indexed out of range or rendered unbounded.
    #[test]
    fn invalid_or_huge_meshes_are_refused() {
        let src = content(SampleType::U8, Rect::new(0, 0, 8, 8));
        let ok = grid([0.0, 0.0, 8.0, 8.0], 1, |p| p);
        let mut out_of_range = ok.clone();
        out_of_range.tris.push([0, 1, 4]);
        let mut short = ok.clone();
        short.deformed.pop();
        let mut nan = ok.clone();
        nan.deformed[1] = [f64::NAN, 0.0];
        let mut inf = ok.clone();
        inf.original[2] = [0.0, f64::INFINITY];
        let mut huge = ok.clone();
        huge.deformed[3] = [1e9, 1e9];
        for m in [out_of_range, short, nan, inf] {
            assert!(!m.is_valid(), "{m:?}");
            assert!(warp_solved(&src, &m, Interp::Bicubic).is_none(), "{m:?}");
        }
        // Valid, but its deformed bounds span ~10^18 px.
        assert!(huge.is_valid());
        assert!(warp_solved(&src, &huge, Interp::Bicubic).is_none());
        assert!(warp_solved(&src, &ok, Interp::Bicubic).is_some());
        // An empty source gives an empty result.
        assert!(warp_solved(&Surface::new(src.format()), &ok, Interp::Bicubic).unwrap().content_bounds().is_empty());
    }
}
