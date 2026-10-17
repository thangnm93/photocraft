//! Puppet Warp smart filters (`Fltr` class `rigidTransform`, filterID 990) as Photoshop writes
//! them. Photoshop stores the deformation already solved, so it renders as a piecewise-affine
//! mesh warp ([`photocraft_algo::solved_mesh`]) without solving anything again:
//!
//! ```text
//! Fltr = rigidTransform {
//!   null (reference), rigidType (bool),
//!   puppetShapeList = [ puppetShape {          one mesh per separate region of the layer
//!     rigidType, VrsM = 1, VrsN = 0,
//!     originalVertexArray  tdta: n × (x, y) f32 little-endian, document pixels
//!     deformedVertexArray  tdta: the same n vertices after the warp
//!     indexArray           tdta: m × 3 u32 little-endian vertex indices (the triangles)
//!     pinOffsets, posFinalPins, PinP (doubles), pinVertexIndices, PnRt (longs), PnOv (bools),
//!     PnDp (doubles): the pins, for editing; meshQuality, meshExpansion, meshRigidity (long),
//!     imageResolution (double), meshBoundaryPath (pathClass: the mesh outline), selectedPin
//!   } … ],
//!   PuX0…PuX3, PuY0…PuY3 (double): the placement's corners when the warp was made, in the
//!     order of `Trnf` (top-left, top-right, bottom-right, bottom-left)
//! }
//! ```
//!
//! The vertices are in document pixels of the placed (transformed and warped) content, which is
//! what smart filters see. Only the mesh area shows: pixels outside every shape are dropped.
//! Shapes without pins keep `deformed == original`. The stored corners let the mesh follow the
//! smart object when it is moved or transformed later.
//!
//! [`COMMAND`] keeps the whole `filterFXList` item verbatim (hex in `params.psd`, like an
//! unsupported filter) and decodes the mesh from it each time it renders, so PSD export writes
//! the item back byte for byte; only `blendOptions` and `enab` are rewritten, and only when the
//! smart filter's blending or visibility changed. Anything that doesn't decode (another version,
//! lengths that aren't whole vertices or triangles, indices out of range, non-finite or absurd
//! coordinates, more than the caps below) stays an [`super::UNSUPPORTED_FILTER`].

use photocraft_algo::solved_mesh::SolvedMesh;
use photocraft_doc::SmartFilter;
use photocraft_psd::descriptor::{Descriptor, Value};
use serde_json::{Value as J, json};

use super::{blend_options, from_hex, get_desc, int, item_blending, set, text_of, to_hex};
use crate::blocks::num;

pub(super) const CLASS: &str = "rigidTransform";
pub(super) const FILTER_ID: i32 = 990;
/// Smart filter command of a Photoshop Puppet Warp (rendered from its stored mesh, not editable).
pub const COMMAND: &str = "psd.puppetWarp";
/// The `puppetShape` layout this decoder knows (`VrsM`).
const VERSION: i32 = 1;
/// Separate meshes in one filter: one per region of the layer (three in every file seen).
const MAX_SHAPES: usize = 1024;
/// Vertices and triangles over all shapes. Photoshop's meshes have hundreds to a few thousand;
/// the caps bound what a hostile file can make us allocate and draw.
const MAX_VERTICES: usize = 1 << 20;
const MAX_TRIANGLES: usize = 1 << 21;
/// Farthest a vertex or stored corner may be from the origin (px): beyond any PSB canvas.
const MAX_COORD: f64 = (1 << 20) as f64;
/// The item with all its arrays at the caps, plus room for the rest of the descriptor.
const MAX_TEMPLATE_BYTES: usize = MAX_VERTICES * 16 + MAX_TRIANGLES * 12 + (1 << 24);

/// A decoded Puppet Warp: every shape's mesh in one (triangles in shape order) and the corners
/// of the placement it was made for.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPuppet {
    pub mesh: SolvedMesh,
    /// `PuX`/`PuY`: top-left, top-right, bottom-right, bottom-left. `None` when not stored.
    pub quad: Option<[[f64; 2]; 4]>,
}

fn raw<'a>(d: &'a Descriptor, key: &str) -> Option<&'a [u8]> {
    match d.get(key)? {
        Value::RawData(b) => Some(b),
        _ => None,
    }
}

/// `n` × (x, y) f32 little-endian; `None` for a partial vertex, a non-finite or absurd value.
fn vertices(b: &[u8]) -> Option<Vec<[f64; 2]>> {
    let (pairs, rest) = b.as_chunks::<8>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|p| {
            let (words, _) = p.as_chunks::<4>();
            let mut xy = words.iter().map(|w| f64::from(f32::from_le_bytes(*w)));
            let (x, y) = (xy.next()?, xy.next()?);
            (x.abs() <= MAX_COORD && y.abs() <= MAX_COORD).then_some([x, y])
        })
        .collect()
}

/// `m` × 3 u32 little-endian indices into `n` vertices, offset by `base`; `None` for a partial
/// triangle or an index out of range.
fn triangles(b: &[u8], n: usize, base: usize) -> Option<Vec<[usize; 3]>> {
    let (tris, rest) = b.as_chunks::<12>();
    if !rest.is_empty() {
        return None;
    }
    tris.iter()
        .map(|t| {
            let (words, _) = t.as_chunks::<4>();
            let mut ix = words.iter().map(|w| usize::try_from(u32::from_le_bytes(*w)).ok().filter(|&i| i < n).map(|i| base + i));
            Some([ix.next()??, ix.next()??, ix.next()??])
        })
        .collect()
}

/// The corners `PuX0…3`/`PuY0…3`: `Ok(None)` when none is stored, `Err` when only some are or
/// one is not a number within [`MAX_COORD`].
fn corners(f: &Descriptor) -> Result<Option<[[f64; 2]; 4]>, ()> {
    let keys = ["PuX0", "PuY0", "PuX1", "PuY1", "PuX2", "PuY2", "PuX3", "PuY3"];
    let v: Vec<Option<f64>> = keys.iter().map(|k| num(f.get(k))).collect();
    if v.iter().all(Option::is_none) {
        return Ok(None);
    }
    let v: Vec<f64> = v.into_iter().map(|x| x.filter(|x| x.abs() <= MAX_COORD)).collect::<Option<_>>().ok_or(())?;
    match v.as_slice() {
        &[x0, y0, x1, y1, x2, y2, x3, y3] => Ok(Some([[x0, y0], [x1, y1], [x2, y2], [x3, y3]])),
        _ => Err(()),
    }
}

/// The mesh of a `filterFXList` item, `None` unless it is a Puppet Warp whose every shape decodes.
pub(super) fn decode(item: &Descriptor) -> Option<StoredPuppet> {
    let f = get_desc(item, "Fltr")?;
    if int(item, "filterID") != Some(FILTER_ID) || !f.class_id.is(CLASS) {
        return None;
    }
    let Some(Value::List(shapes)) = f.get("puppetShapeList") else { return None };
    if shapes.is_empty() || shapes.len() > MAX_SHAPES {
        return None;
    }
    // Sizes first, so a file at the caps is refused before anything is decoded.
    let shapes: Vec<&Descriptor> = shapes
        .iter()
        .map(|s| match s {
            Value::Descriptor(d) if d.class_id.is("puppetShape") => Some(d),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let (mut nv, mut nt) = (0usize, 0usize);
    for s in &shapes {
        if s.get("VrsM").is_some() && int(s, "VrsM") != Some(VERSION) {
            return None;
        }
        nv = nv.checked_add(raw(s, "originalVertexArray")?.len() / 8)?;
        nt = nt.checked_add(raw(s, "indexArray")?.len() / 12)?;
    }
    if nv > MAX_VERTICES || nt > MAX_TRIANGLES || nt == 0 {
        return None;
    }
    let mut mesh = SolvedMesh { original: Vec::with_capacity(nv), deformed: Vec::with_capacity(nv), tris: Vec::with_capacity(nt) };
    for s in shapes {
        let original = vertices(raw(s, "originalVertexArray")?)?;
        let deformed = vertices(raw(s, "deformedVertexArray")?)?;
        if deformed.len() != original.len() {
            return None;
        }
        mesh.tris.extend(triangles(raw(s, "indexArray")?, original.len(), mesh.original.len())?);
        mesh.original.extend(original);
        mesh.deformed.extend(deformed);
    }
    let quad = corners(f).ok()?;
    mesh.is_valid().then_some(StoredPuppet { mesh, quad })
}

/// `psd.puppetWarp` params for a Puppet Warp item whose mesh decodes: its name, filter id and
/// the item itself (`psd`, hex).
pub(super) fn import_params(item: &Descriptor) -> Option<J> {
    decode(item)?;
    let name = text_of(item, "Nm  ").unwrap_or_default().replace('&', "");
    Some(json!({"name": name, "filterID": FILTER_ID, "psd": to_hex(&item.to_bytes())}))
}

/// The item stored in `psd.puppetWarp` params.
fn template(params: &J) -> Result<Descriptor, String> {
    let raw = params.get("psd").and_then(J::as_str).ok_or("its Puppet Warp data is missing")?;
    if raw.len() > MAX_TEMPLATE_BYTES * 2 {
        return Err("its Puppet Warp data is too large".into());
    }
    let bytes = from_hex(raw).ok_or("its Puppet Warp data is not valid hex")?;
    Descriptor::from_bytes(&bytes).map_err(|e| format!("its Puppet Warp data is unreadable ({e})"))
}

/// The mesh of `psd.puppetWarp` params, `None` when the stored item is missing or doesn't decode.
pub fn stored_mesh(params: &J) -> Option<StoredPuppet> {
    decode(&template(params).ok()?)
}

/// The `filterFXList` item: the stored item, with the filter's blending and visibility written
/// over it only where they changed (so an unchanged filter goes back byte for byte).
pub(super) fn export_item(f: &SmartFilter) -> Result<Descriptor, String> {
    let mut d = template(&f.params)?;
    let (blend, opacity, visible) = item_blending(&d);
    if blend != f.blend || opacity.to_bits() != f.opacity.to_bits() {
        set(&mut d, "blendOptions", blend_options(f));
    }
    if visible != f.visible {
        set(&mut d, "enab", Value::Boolean(f.visible));
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smart_map::{UNSUPPORTED_FILTER, filter_from_item, item_for_filter};
    use photocraft_color::BlendMode;
    use photocraft_psd::descriptor::{Id, UnicodeString};

    fn f32s(v: &[[f32; 2]]) -> Vec<u8> {
        v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn u32s(v: &[[u32; 3]]) -> Vec<u8> {
        v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A `puppetShape` as Photoshop writes one (the pin lists empty, the outline left out).
    fn shape(original: &[[f32; 2]], deformed: &[[f32; 2]], tris: &[[u32; 3]]) -> Descriptor {
        Descriptor::new("puppetShape")
            .with("rigidType", Value::Boolean(true))
            .with("VrsM", Value::Integer(1))
            .with("VrsN", Value::Integer(0))
            .with("originalVertexArray", Value::RawData(f32s(original)))
            .with("deformedVertexArray", Value::RawData(f32s(deformed)))
            .with("indexArray", Value::RawData(u32s(tris)))
            .with("pinOffsets", Value::List(vec![Value::Double(0.5), Value::Double(-0.25)]))
            .with("posFinalPins", Value::List(vec![Value::Double(12.0), Value::Double(8.0)]))
            .with("pinVertexIndices", Value::List(vec![Value::Integer(1)]))
            .with("meshQuality", Value::Integer(2))
            .with("meshExpansion", Value::Integer(2))
            .with("meshRigidity", Value::Integer(2))
            .with("imageResolution", Value::Double(300.0))
            .with("selectedPin", Value::List(Vec::new()))
    }

    /// A Puppet Warp `filterFXList` item laid out as Photoshop writes it.
    fn item(shapes: Vec<Descriptor>) -> Descriptor {
        let enumv = |t: &str, v: &str| Value::Enumerated { type_id: Id::new(t), value: Id::new(v) };
        let mut fltr = Descriptor::new(CLASS)
            .with("rigidType", Value::Boolean(true))
            .with("puppetShapeList", Value::List(shapes.into_iter().map(Value::Descriptor).collect()));
        for (k, v) in [("PuX0", 10.0), ("PuX1", 110.0), ("PuX2", 110.0), ("PuX3", 10.0), ("PuY0", 5.0), ("PuY1", 5.0), ("PuY2", 85.0), ("PuY3", 85.0)] {
            fltr = fltr.with(k, Value::Double(v));
        }
        let rgb =
            |v: f64| Value::Descriptor(Descriptor::new("RGBC").with("Rd  ", Value::Double(v)).with("Grn ", Value::Double(v)).with("Bl  ", Value::Double(v)));
        Descriptor::new("filterFX")
            .with("Nm  ", Value::Text(UnicodeString::new_nul("Puppet Warp")))
            .with(
                "blendOptions",
                Value::Descriptor(
                    Descriptor::new("blendOptions").with("Opct", Value::UnitFloat { unit: *b"#Prc", value: 100.0 }).with("Md  ", enumv("BlnM", "Nrml")),
                ),
            )
            .with("enab", Value::Boolean(true))
            .with("hasoptions", Value::Boolean(true))
            .with("FrgC", rgb(0.0))
            .with("BckC", rgb(255.0))
            .with("Fltr", Value::Descriptor(fltr))
            .with("filterID", Value::Integer(FILTER_ID))
    }

    const SQUARE: [[f32; 2]; 4] = [[10.0, 5.0], [60.0, 5.0], [60.0, 45.0], [10.0, 45.0]];
    const MOVED: [[f32; 2]; 4] = [[12.0, 5.0], [64.0, 9.0], [60.0, 45.0], [10.0, 45.0]];
    const TWO: [[u32; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

    fn two_shapes() -> Descriptor {
        let far: Vec<[f32; 2]> = SQUARE.iter().map(|p| [p[0] + 50.0, p[1] + 30.0]).collect();
        item(vec![shape(&SQUARE, &MOVED, &TWO), shape(&far, &far, &[[0, 1, 2]])])
    }

    #[test]
    fn decodes_every_shape_into_one_mesh() {
        let p = decode(&two_shapes()).unwrap();
        assert_eq!(p.mesh.original.len(), 8);
        assert_eq!(p.mesh.deformed[1], [64.0, 9.0]);
        assert_eq!(p.mesh.deformed[5], [110.0, 35.0], "a shape without pins stays put");
        // The second shape's indices follow the first shape's vertices.
        assert_eq!(p.mesh.tris, vec![[0, 1, 2], [0, 2, 3], [4, 5, 6]]);
        assert_eq!(p.quad, Some([[10.0, 5.0], [110.0, 5.0], [110.0, 85.0], [10.0, 85.0]]));
    }

    #[test]
    fn puppet_warp_items_import_and_write_back_byte_for_byte() {
        let src = two_shapes();
        let f = filter_from_item(&src);
        assert_eq!(f.command, COMMAND);
        assert_eq!((f.params["name"].as_str(), f.params["filterID"].as_i64()), (Some("Puppet Warp"), Some(990)));
        assert_eq!((f.blend, f.opacity, f.visible), (BlendMode::Normal, 1.0, true));
        let back = item_for_filter(&f).unwrap();
        assert_eq!(back.to_bytes(), src.to_bytes(), "Photoshop's own `Nrml` blend code is kept");
        assert_eq!(stored_mesh(&f.params), decode(&src));
        // Through the descriptor's own bytes, as a PSD round trip reads it.
        assert_eq!(filter_from_item(&Descriptor::from_bytes(&back.to_bytes()).unwrap()), f);
    }

    #[test]
    fn re_blended_or_hidden_puppet_warps_keep_their_mesh() {
        let f = filter_from_item(&two_shapes());
        let mut g = f.clone();
        g.blend = BlendMode::Multiply;
        g.opacity = 0.4;
        g.visible = false;
        let d = item_for_filter(&g).unwrap();
        let back = filter_from_item(&Descriptor::from_bytes(&d.to_bytes()).unwrap());
        assert_eq!((back.command.as_str(), back.blend, back.opacity, back.visible), (COMMAND, BlendMode::Multiply, 0.4, false));
        assert_eq!(stored_mesh(&back.params), stored_mesh(&f.params));
        // Only the blending and visibility keys changed.
        let (a, b) = (two_shapes(), d);
        for (k, v) in &a.items {
            if !k.is("blendOptions") && !k.is("enab") {
                assert_eq!(b.get(&String::from_utf8_lossy(k.as_bytes())), Some(v));
            }
        }
        // Set back to how it was: the original bytes again.
        let restored = SmartFilter { visible: true, blend: BlendMode::Normal, opacity: 1.0, ..g };
        assert_eq!(item_for_filter(&restored).unwrap().to_bytes(), two_shapes().to_bytes());
    }

    /// Anything that doesn't decode stays an unsupported filter, written back with the same
    /// filter descriptor.
    #[test]
    fn malformed_puppet_warps_stay_unsupported() {
        let edit_shape = |f: &dyn Fn(&mut Descriptor)| {
            let mut s = shape(&SQUARE, &MOVED, &TWO);
            f(&mut s);
            item(vec![s])
        };
        let edit_fltr = |f: &dyn Fn(&mut Descriptor)| {
            let mut i = item(vec![shape(&SQUARE, &MOVED, &TWO)]);
            let mut fl = get_desc(&i, "Fltr").unwrap().clone();
            f(&mut fl);
            set(&mut i, "Fltr", Value::Descriptor(fl));
            i
        };
        let nan = [[10.0, 5.0], [f32::NAN, 5.0], [60.0, 45.0], [10.0, 45.0]];
        let far = [[10.0, 5.0], [3e7, 5.0], [60.0, 45.0], [10.0, 45.0]];
        let inf = [[10.0, 5.0], [60.0, 5.0], [60.0, f32::INFINITY], [10.0, 45.0]];
        let cases = vec![
            ("index out of range", item(vec![shape(&SQUARE, &MOVED, &[[0, 1, 4]])])),
            ("index u32::MAX", item(vec![shape(&SQUARE, &MOVED, &[[0, 1, u32::MAX]])])),
            ("fewer deformed vertices", item(vec![shape(&SQUARE, &MOVED[..3], &TWO)])),
            ("NaN vertex", item(vec![shape(&SQUARE, &nan, &TWO)])),
            ("infinite vertex", item(vec![shape(&inf, &MOVED, &TWO)])),
            ("absurd vertex", item(vec![shape(&SQUARE, &far, &TWO)])),
            ("no triangles", item(vec![shape(&SQUARE, &MOVED, &[])])),
            ("no shapes", item(vec![])),
            ("half a vertex", edit_shape(&|s| set(s, "originalVertexArray", Value::RawData(f32s(&SQUARE)[..28].to_vec())))),
            ("half a triangle", edit_shape(&|s| set(s, "indexArray", Value::RawData(u32s(&TWO)[..20].to_vec())))),
            ("vertices as a list", edit_shape(&|s| set(s, "originalVertexArray", Value::List(vec![Value::Double(1.0)])))),
            ("no deformed vertices", edit_shape(&|s| s.items.retain(|(k, _)| !k.is("deformedVertexArray")))),
            ("another version", edit_shape(&|s| set(s, "VrsM", Value::Integer(2)))),
            ("a shape of another class", edit_fltr(&|f| set(f, "puppetShapeList", Value::List(vec![Value::Descriptor(Descriptor::new("null"))])))),
            ("shapes not a list", edit_fltr(&|f| set(f, "puppetShapeList", Value::Integer(3)))),
            ("some corners only", edit_fltr(&|f| f.items.retain(|(k, _)| !k.is("PuY3")))),
            ("a NaN corner", edit_fltr(&|f| set(f, "PuX1", Value::Double(f64::NAN)))),
            ("another filter id", {
                let mut i = item(vec![shape(&SQUARE, &MOVED, &TWO)]);
                set(&mut i, "filterID", Value::Integer(991));
                i
            }),
            ("another class", edit_fltr(&|f| f.class_id = Id::new("Twrl"))),
        ];
        for (what, src) in cases {
            let f = filter_from_item(&src);
            assert_eq!(f.command, UNSUPPORTED_FILTER, "{what}");
            let out = item_for_filter(&f).unwrap();
            // By bytes: a NaN is not equal to itself.
            let fltr = |d: &Descriptor| get_desc(d, "Fltr").map(Descriptor::to_bytes);
            assert_eq!(fltr(&out), fltr(&src), "{what}");
            assert_eq!(filter_from_item(&out).params, f.params, "{what}");
        }
        // Without stored corners the mesh is used as it is.
        let bare = edit_fltr(&|f| f.items.retain(|(k, _)| !String::from_utf8_lossy(k.as_bytes()).starts_with("Pu")));
        assert_eq!(decode(&bare).map(|p| p.quad), Some(None));
    }

    /// Counts over the caps are refused from the array sizes, before anything is decoded.
    #[test]
    fn huge_puppet_warps_are_refused() {
        let many = vec![[1.0f32, 1.0]; MAX_VERTICES + 1];
        assert!(decode(&item(vec![shape(&many, &many, &[[0, 1, 2]])])).is_none());
        let shapes = (0..=MAX_SHAPES).map(|_| shape(&SQUARE, &SQUARE, &TWO)).collect();
        assert!(decode(&item(shapes)).is_none());
        let mut tris = vec![[0u32, 1, 2]; MAX_TRIANGLES / 2 + 1];
        assert!(decode(&item(vec![shape(&SQUARE, &SQUARE, &tris), shape(&SQUARE, &SQUARE, &tris)])).is_none());
        tris.truncate(4);
        assert!(decode(&item(vec![shape(&SQUARE, &SQUARE, &tris)])).is_some());
    }

    #[test]
    fn bad_params_never_render_or_export() {
        let ok = filter_from_item(&two_shapes());
        for p in [J::Null, json!({}), json!({"psd": 12}), json!({"psd": "zz"}), json!({"psd": "abc"}), json!({"psd": to_hex(b"not a descriptor")})] {
            assert!(stored_mesh(&p).is_none(), "{p}");
            assert!(item_for_filter(&SmartFilter { params: p.clone(), ..ok.clone() }).is_err(), "{p}");
        }
        assert!(stored_mesh(&json!({"psd": "0".repeat(MAX_TEMPLATE_BYTES * 2 + 2)})).is_none());
        // A readable descriptor that is not a Puppet Warp renders nothing.
        assert!(stored_mesh(&json!({"psd": to_hex(&Descriptor::new("filterFX").to_bytes())})).is_none());
    }

    /// Corrupted bytes of a Puppet Warp item never panic, whatever they decode to.
    #[test]
    fn mutated_puppet_warps_never_panic() {
        let bytes = two_shapes().to_bytes();
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut decoded = 0;
        for _ in 0..4000 {
            let mut b = bytes.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() as usize) % b.len();
                b[i] = next() as u8;
            }
            if let Ok(d) = Descriptor::from_bytes(&b) {
                let f = filter_from_item(&d);
                let _ = item_for_filter(&f);
                if let Some(p) = stored_mesh(&f.params) {
                    assert!(p.mesh.is_valid());
                    decoded += 1;
                }
            }
        }
        assert!(decoded > 0, "some mutations only touch values that still decode");
    }
}
