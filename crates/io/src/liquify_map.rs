//! Liquify smart filters (`Fltr` class `LqFy`, filterID `LqFy` as a big-endian code) as Photoshop
//! writes them. Photoshop stores the displacement mesh the Liquify dialog produced, so the filter
//! renders from it ([`photocraft_algo::displacement_mesh`]) without replaying any brush stroke.
//! The only key of `Fltr` is `LqMe`, raw bytes (`tdta`), decoded from Photoshop's files:
//!
//! ```text
//! 00 00 00 04  "yfqL" "hseM"          (a big-endian 4, then LqFy and Mesh as little-endian codes)
//! then u32 little-endian:
//!   2                                  layout version
//!   cols, rows                         nodes per row, rows of nodes: width / 4, height / 4
//!   0, 1                               (always these)
//!   top, left, bottom, right           the content's bounds inside the frame
//!   0, 0, height, width                the frame: the image the mesh was made on
//! then for every row, runs until it holds `cols` nodes, starting with an undisplaced run:
//!   u32 n undisplaced nodes (may be 0) [, u32 m displaced nodes, m × (dx, dy) f32 LE] …
//! ```
//!
//! A row ends as soon as a run fills it. Undisplaced nodes are (0, 0). The mesh is an inverse map
//! in cells: the frame is divided into `cols × rows` cells (`width / cols` by `height / rows`
//! px, a little over 4), node (i, j) sits at the centre of cell (i, j), and the output there
//! shows the filter's input at the node plus `(dx, dy)` cells; between nodes the displacement is
//! bilinear, past the outermost ones it stays at theirs, and the input is sampled bilinearly. On
//! four meshes of a Photoshop mockup this reproduces the pixels Photoshop stored from its own
//! filter input within 0.01 levels on average (0–255).
//!
//! The frame is the pixel buffer Photoshop filters the smart object in (the rect of its `FEid`
//! cache item: the placed content's control bounds plus 3 px, grown to keep a mesh made on an
//! earlier placement): the mesh sits at its bottom-left corner. Import stores that rect in
//! `params.frame` (`[x0, y0, x1, y1]`, document pixels), so the Liquify stays where Photoshop drew
//! it when the contents are replaced; without one, or with one that can't be that buffer (it comes
//! from the file: see [`trusted_frame`]), the frame is the filter input's bounds plus 3 px.
//!
//! [`COMMAND`] keeps the whole `filterFXList` item verbatim (hex in `params.psd`, like an
//! unsupported filter) and decodes the mesh from it each time it renders, so PSD export writes
//! the item back byte for byte; only `blendOptions` and `enab` are rewritten, and only when the
//! smart filter's blending or visibility changed. Anything that doesn't decode (another header,
//! version or layout, runs that overflow, bytes left over, non-finite or absurd displacements,
//! more nodes than [`MAX_NODES`]) stays an [`super::UNSUPPORTED_FILTER`].

use photocraft_algo::displacement_mesh::DisplacementMesh;
use photocraft_doc::SmartFilter;
use photocraft_geom::Rect;
use photocraft_psd::descriptor::{Descriptor, Value};
use serde_json::{Value as J, json};

use super::{blend_options, from_hex, get_desc, int, item_blending, set, text_of, to_hex};

pub(super) const CLASS: &str = "LqFy";
/// `LqFy` as a big-endian code.
pub(super) const FILTER_ID: i32 = 0x4C71_4679;
/// Smart filter command of a Photoshop Liquify (rendered from its stored mesh, not editable).
pub const COMMAND: &str = "psd.liquify";
/// What an `LqMe` blob starts with: a big-endian 4, then `LqFy` and `Mesh` as little-endian codes.
const MAGIC: &[u8; 12] = b"\0\0\0\x04yfqLhseM";
/// The mesh layout this decoder knows.
const VERSION: u32 = 2;
/// Photoshop puts a node every 4 px of the frame (rounded down), so its cells are a little over 4 px.
const NODE_SPACING: u32 = 4;
/// What Photoshop adds around a smart object's content for its filter buffer (px).
const BUFFER_MARGIN: i32 = 3;
/// Farthest a stored frame may reach from the origin (px): beyond any PSB canvas.
const MAX_FRAME: i64 = 1 << 24;
/// How much larger than the mesh's frame a stored filter buffer may be, each way (px). Photoshop
/// grows the buffer to keep the input when the smart object was re-placed after the Liquify was
/// made: by up to 48 px in the files measured.
const MAX_GROWTH: i64 = 1024;
/// Largest filter buffer trusted from a file (px): 16k × 16k, a mesh at [`MAX_NODES`].
const MAX_FRAME_PIXELS: i64 = 1 << 28;
/// Mesh nodes a decoded mesh may have (a frame of 16k × 16k px).
const MAX_NODES: usize = 1 << 24;
/// Farthest a node may be displaced (cells): beyond any PSB canvas.
const MAX_DISPLACEMENT: f32 = (1 << 18) as f32;
/// The item with a mesh at the cap, plus room for the rest of the descriptor.
const MAX_TEMPLATE_BYTES: usize = MAX_NODES * 8 + (1 << 24);

/// A decoded Liquify mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredLiquify {
    /// Nodes per row.
    pub cols: usize,
    /// Rows of nodes.
    pub rows: usize,
    /// Width and height (px) of the image the mesh was made on (its frame).
    pub size: [u32; 2],
    /// Bounds `[x0, y0, x1, y1]` of that image's content inside the frame.
    pub content: [u32; 4],
    /// Displacement of every node, row-major, in cells.
    pub d: Vec<[f32; 2]>,
}

/// Little-endian u32 words from `b`, advancing it; `None` past the end.
fn word(b: &mut &[u8]) -> Option<u32> {
    let (w, rest) = b.split_first_chunk::<4>()?;
    *b = rest;
    Some(u32::from_le_bytes(*w))
}

/// The mesh of an `LqMe` blob, `None` when it doesn't decode: another header or version, a
/// node grid that doesn't fill the frame or is over [`MAX_NODES`], content outside the frame,
/// runs that overflow a row or the data, bytes left over, non-finite or absurd displacements.
pub(super) fn parse_mesh(blob: &[u8]) -> Option<StoredLiquify> {
    let mut b = blob.strip_prefix(MAGIC)?;
    let mut h = [0u32; 13];
    for v in &mut h {
        *v = word(&mut b)?;
    }
    let [version, cols, rows, a, one, top, left, bottom, right, ftop, fleft, height, width] = h;
    if version != VERSION || a != 0 || one != 1 || ftop != 0 || fleft != 0 {
        return None;
    }
    if cols == 0
        || rows == 0
        || cols != width / NODE_SPACING
        || rows != height / NODE_SPACING
        || left > right
        || top > bottom
        || right > width
        || bottom > height
    {
        return None;
    }
    let (cols, rows) = (usize::try_from(cols).ok()?, usize::try_from(rows).ok()?);
    let n = cols.checked_mul(rows).filter(|&n| n <= MAX_NODES)?;
    let mut d = vec![[0.0f32; 2]; n];
    for row in d.chunks_exact_mut(cols) {
        let mut at = 0usize;
        loop {
            at = at.checked_add(usize::try_from(word(&mut b)?).ok()?)?;
            if at >= cols {
                break;
            }
            let run = usize::try_from(word(&mut b)?).ok()?;
            let end = at.checked_add(run).filter(|&e| e <= cols)?;
            for node in &mut row[at..end] {
                let (x, y) = (f32::from_bits(word(&mut b)?), f32::from_bits(word(&mut b)?));
                if !(x.abs() <= MAX_DISPLACEMENT && y.abs() <= MAX_DISPLACEMENT) {
                    return None;
                }
                *node = [x, y];
            }
            at = end;
            if at == cols {
                break;
            }
        }
        if at != cols {
            return None;
        }
    }
    b.is_empty().then_some(StoredLiquify { cols, rows, size: [width, height], content: [left, top, right, bottom], d })
}

/// The mesh of a `filterFXList` item, `None` unless it is a Liquify whose mesh decodes.
pub(super) fn decode(item: &Descriptor) -> Option<StoredLiquify> {
    let f = get_desc(item, "Fltr")?;
    if int(item, "filterID") != Some(FILTER_ID) || !f.class_id.is(CLASS) {
        return None;
    }
    match f.get("LqMe")? {
        Value::RawData(b) => parse_mesh(b),
        _ => None,
    }
}

/// The filter buffer stored in `psd.liquify` params (`frame`), `None` when there is none or it is
/// not an ordered rect of whole pixels within [`MAX_FRAME`].
pub fn stored_frame(params: &J) -> Option<Rect> {
    let v: Vec<i64> = params.get("frame")?.as_array()?.iter().map(J::as_i64).collect::<Option<_>>()?;
    let &[x0, y0, x1, y1] = v.as_slice() else { return None };
    if x0 >= x1 || y0 >= y1 || v.iter().any(|c| c.abs() > MAX_FRAME) {
        return None;
    }
    Some(Rect::new(i32::try_from(x0).ok()?, i32::try_from(y0).ok()?, i32::try_from(x1).ok()?, i32::try_from(y1).ok()?))
}

/// The stored frame (see [`stored_frame`]) when it can be Photoshop's filter buffer for a smart
/// object whose filter input has the bounds `input`: it holds the mesh's frame and at most
/// [`MAX_GROWTH`] px more each way (the buffer is the mesh's frame grown to keep the input inside),
/// it reaches the input plus [`BUFFER_MARGIN`], and it is at most [`MAX_FRAME_PIXELS`]. `None`
/// otherwise: the frame comes from the file, so it must not size an allocation by itself.
pub fn trusted_frame(params: &J, input: Rect) -> Option<Rect> {
    let f = stored_frame(params)?;
    let m = stored_mesh(params)?;
    let (fw, fh, [mw, mh]) = (i64::from(f.width()), i64::from(f.height()), m.size.map(i64::from));
    let fits = fw >= mw && fh >= mh && fw - mw <= MAX_GROWTH && fh - mh <= MAX_GROWTH && fw * fh <= MAX_FRAME_PIXELS;
    (fits && !input.is_empty() && !f.intersect(&input.inflate(BUFFER_MARGIN)).is_empty()).then_some(f)
}

/// Stores Photoshop's filter buffer for the smart object (its `FEid` rect) in `psd.liquify` params.
pub fn set_frame(params: &mut J, buffer: Rect) {
    if let Some(p) = params.as_object_mut() {
        p.insert("frame".into(), json!([buffer.x0, buffer.y0, buffer.x1, buffer.y1]));
    }
}

/// The mesh of `psd.liquify` params placed in the document, and the area it renders over (the
/// filter buffer): the stored frame when it is trusted ([`trusted_frame`]), else `input` (the
/// filter input's bounds) plus 3 px. `None` when the stored item doesn't decode or there is
/// nothing to render over.
pub fn placed_mesh(params: &J, input: Rect) -> Option<(DisplacementMesh, Rect)> {
    let m = stored_mesh(params)?;
    let buffer = match trusted_frame(params, input) {
        Some(f) => f,
        None if input.is_empty() => return None,
        None => input.inflate(BUFFER_MARGIN),
    };
    let [w, h] = m.size.map(f64::from);
    let cell = [w / m.cols as f64, h / m.rows as f64];
    // The frame at the buffer's bottom-left corner; node (i, j) at the centre of cell (i, j).
    let origin = [f64::from(buffer.x0) + cell[0] / 2.0, f64::from(buffer.y1) - h + cell[1] / 2.0];
    let d = m.d.iter().map(|v| [(f64::from(v[0]) * cell[0]) as f32, (f64::from(v[1]) * cell[1]) as f32]).collect();
    Some((DisplacementMesh { origin, cell, cols: m.cols, rows: m.rows, d }, buffer))
}

/// `psd.liquify` params for a Liquify item whose mesh decodes: its name, filter id and the item
/// itself (`psd`, hex).
pub(super) fn import_params(item: &Descriptor) -> Option<J> {
    decode(item)?;
    let name = text_of(item, "Nm  ").unwrap_or_default().replace('&', "");
    Some(json!({"name": name, "filterID": FILTER_ID, "psd": to_hex(&item.to_bytes())}))
}

/// The item stored in `psd.liquify` params.
fn template(params: &J) -> Result<Descriptor, String> {
    let raw = params.get("psd").and_then(J::as_str).ok_or("its Liquify data is missing")?;
    if raw.len() > MAX_TEMPLATE_BYTES * 2 {
        return Err("its Liquify data is too large".into());
    }
    let bytes = from_hex(raw).ok_or("its Liquify data is not valid hex")?;
    Descriptor::from_bytes(&bytes).map_err(|e| format!("its Liquify data is unreadable ({e})"))
}

/// The mesh of `psd.liquify` params, `None` when the stored item is missing or doesn't decode.
pub fn stored_mesh(params: &J) -> Option<StoredLiquify> {
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

    /// An `LqMe` blob laid out as Photoshop writes one: the header, then per row of nodes runs
    /// alternating between undisplaced nodes (a count) and displaced ones (a count and an (x, y)
    /// f32 pair per node), starting with an undisplaced run (possibly empty) and ending as soon as
    /// the row is full.
    fn blob(cols: u32, rows: u32, size: [u32; 2], content: [u32; 4], d: &[[f32; 2]]) -> Vec<u8> {
        let mut b = vec![0, 0, 0, 4];
        b.extend(b"yfqLhseM");
        let [x0, y0, x1, y1] = content;
        for v in [2, cols, rows, 0, 1, y0, x0, y1, x1, 0, 0, size[1], size[0]] {
            b.extend(v.to_le_bytes());
        }
        for row in d.chunks(cols.max(1) as usize) {
            let mut i = 0;
            while i < row.len() {
                let zeros = row[i..].iter().take_while(|v| **v == [0.0, 0.0]).count();
                b.extend((zeros as u32).to_le_bytes());
                i += zeros;
                if i == row.len() {
                    break;
                }
                let moved = row[i..].iter().take_while(|v| **v != [0.0, 0.0]).count();
                b.extend((moved as u32).to_le_bytes());
                for v in &row[i..i + moved] {
                    b.extend(v[0].to_le_bytes());
                    b.extend(v[1].to_le_bytes());
                }
                i += moved;
            }
        }
        b
    }

    /// A 5 × 3 mesh over a 20 × 13 px frame: a row starting and ending with displaced nodes, an
    /// undisplaced row and one ending with undisplaced nodes.
    fn nodes() -> Vec<[f32; 2]> {
        let z = [0.0, 0.0];
        vec![[0.5, -0.25], z, z, [1.0, 0.0], [0.0, 2.0], z, z, z, z, z, z, [-1.5, 0.75], [3.0, 1.0], z, z]
    }

    fn mesh() -> Vec<u8> {
        blob(5, 3, [20, 13], [1, 2, 19, 12], &nodes())
    }

    /// A Liquify `filterFXList` item laid out as Photoshop writes it.
    fn item(lqme: Value) -> Descriptor {
        let enumv = |t: &str, v: &str| Value::Enumerated { type_id: Id::new(t), value: Id::new(v) };
        let rgb =
            |v: f64| Value::Descriptor(Descriptor::new("RGBC").with("Rd  ", Value::Double(v)).with("Grn ", Value::Double(v)).with("Bl  ", Value::Double(v)));
        Descriptor::new("filterFX")
            .with("Nm  ", Value::Text(UnicodeString::new_nul("Liquify...")))
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
            .with("Fltr", Value::Descriptor(Descriptor::new(CLASS).with("LqMe", lqme)))
            .with("filterID", Value::Integer(FILTER_ID))
    }

    fn liquify() -> Descriptor {
        item(Value::RawData(mesh()))
    }

    #[test]
    fn decodes_the_run_encoded_node_grid() {
        let m = parse_mesh(&mesh()).unwrap();
        assert_eq!((m.cols, m.rows, m.size, m.content), (5, 3, [20, 13], [1, 2, 19, 12]));
        assert_eq!(m.d, nodes());
        // Through the item.
        assert_eq!(decode(&liquify()), Some(m));
    }

    #[test]
    fn liquify_items_import_and_write_back_byte_for_byte() {
        let src = liquify();
        let f = filter_from_item(&src);
        assert_eq!(f.command, COMMAND);
        assert_eq!((f.params["name"].as_str(), f.params["filterID"].as_i64()), (Some("Liquify..."), Some(i64::from(FILTER_ID))));
        assert_eq!((f.blend, f.opacity, f.visible), (BlendMode::Normal, 1.0, true));
        let back = item_for_filter(&f).unwrap();
        assert_eq!(back.to_bytes(), src.to_bytes(), "Photoshop's own `Nrml` blend code is kept");
        assert_eq!(stored_mesh(&f.params), decode(&src));
        // Through the descriptor's own bytes, as a PSD round trip reads it.
        assert_eq!(filter_from_item(&Descriptor::from_bytes(&back.to_bytes()).unwrap()), f);
    }

    #[test]
    fn re_blended_or_hidden_liquify_keeps_its_mesh() {
        let f = filter_from_item(&liquify());
        let mut g = f.clone();
        g.blend = BlendMode::Screen;
        g.opacity = 0.25;
        g.visible = false;
        let d = item_for_filter(&g).unwrap();
        let back = filter_from_item(&Descriptor::from_bytes(&d.to_bytes()).unwrap());
        assert_eq!((back.command.as_str(), back.blend, back.opacity, back.visible), (COMMAND, BlendMode::Screen, 0.25, false));
        assert_eq!(stored_mesh(&back.params), stored_mesh(&f.params));
        // Only the blending and visibility keys changed.
        let a = liquify();
        for (k, v) in &a.items {
            if !k.is("blendOptions") && !k.is("enab") {
                assert_eq!(d.get(&String::from_utf8_lossy(k.as_bytes())), Some(v));
            }
        }
        let restored = SmartFilter { visible: true, blend: BlendMode::Normal, opacity: 1.0, ..g };
        assert_eq!(item_for_filter(&restored).unwrap().to_bytes(), liquify().to_bytes());
    }

    /// Anything that doesn't decode stays an unsupported filter, written back with the same
    /// filter descriptor.
    #[test]
    fn malformed_liquify_stays_unsupported() {
        let words = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = mesh();
            f(&mut b);
            item(Value::RawData(b))
        };
        let put = |at: usize, v: u32| move |b: &mut Vec<u8>| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let mut moved = nodes();
        moved[0] = [f32::NAN, 0.0];
        let nan = item(Value::RawData(blob(5, 3, [20, 13], [1, 2, 19, 12], &moved)));
        moved[0] = [0.0, f32::INFINITY];
        let inf = item(Value::RawData(blob(5, 3, [20, 13], [1, 2, 19, 12], &moved)));
        moved[0] = [3e6, 0.0];
        let far = item(Value::RawData(blob(5, 3, [20, 13], [1, 2, 19, 12], &moved)));
        let cases = vec![
            ("another container", words(&|b| b[3] = 5)),
            ("another tag", words(&|b| b[4] = b'x')),
            ("another version", words(&put(12, 3))),
            ("another first flag", words(&put(24, 1))),
            ("another second flag", words(&put(28, 0))),
            ("no columns", item(Value::RawData(blob(0, 3, [20, 13], [1, 2, 19, 12], &[])))),
            ("no rows", item(Value::RawData(blob(5, 0, [20, 13], [1, 2, 19, 12], &[])))),
            ("columns that don't fill the frame", item(Value::RawData(blob(4, 3, [20, 13], [1, 2, 19, 12], &[[0.0; 2]; 12])))),
            ("rows that don't fill the frame", item(Value::RawData(blob(5, 2, [20, 13], [1, 2, 19, 12], &[[0.0; 2]; 10])))),
            ("content outside the frame", item(Value::RawData(blob(5, 3, [20, 13], [1, 2, 21, 12], &nodes())))),
            ("content inside out", item(Value::RawData(blob(5, 3, [20, 13], [9, 2, 3, 12], &nodes())))),
            ("a frame not at the origin", words(&put(52, 1))),
            ("a run past the row", words(&|b| b[64] = 9)),
            ("a truncated run", words(&|b| b.truncate(b.len() - 4))),
            ("a missing row", words(&|b| b.truncate(b.len() - 4 * 4))),
            ("trailing bytes", words(&|b| b.extend([0, 0, 0, 0]))),
            ("a short header", item(Value::RawData(mesh()[..40].to_vec()))),
            ("NaN displacement", nan),
            ("infinite displacement", inf),
            ("absurd displacement", far),
            ("mesh as a list", item(Value::List(vec![Value::Integer(1)]))),
            ("no mesh", {
                let mut i = liquify();
                set(&mut i, "Fltr", Value::Descriptor(Descriptor::new(CLASS)));
                i
            }),
            ("another filter id", {
                let mut i = liquify();
                set(&mut i, "filterID", Value::Integer(990));
                i
            }),
            ("another class", {
                let mut i = liquify();
                set(&mut i, "Fltr", Value::Descriptor(Descriptor::new("Twrl").with("LqMe", Value::RawData(mesh()))));
                i
            }),
        ];
        for (what, src) in cases {
            let f = filter_from_item(&src);
            assert_eq!(f.command, UNSUPPORTED_FILTER, "{what}");
            let out = item_for_filter(&f).unwrap();
            let fltr = |d: &Descriptor| get_desc(d, "Fltr").map(Descriptor::to_bytes);
            assert_eq!(fltr(&out), fltr(&src), "{what}");
            assert_eq!(filter_from_item(&out).params, f.params, "{what}");
        }
    }

    /// A mesh over the cap is refused from its header, before anything is allocated.
    #[test]
    fn huge_liquify_meshes_are_refused() {
        let side = 1u32 << 13;
        let mut b = blob(side, side + 1, [side * 4, (side + 1) * 4], [0, 0, side * 4, (side + 1) * 4], &[]);
        // Every row a single undisplaced run: well-formed, but over the cap.
        for _ in 0..=side {
            b.extend(side.to_le_bytes());
        }
        assert!(parse_mesh(&b).is_none());
        let small = blob(1 << 6, 1 << 6, [1 << 8, 1 << 8], [0, 0, 1 << 8, 1 << 8], &[[0.0; 2]; 1 << 12]);
        assert!(parse_mesh(&small).is_some());
    }

    #[test]
    fn bad_params_never_render_or_export() {
        let ok = filter_from_item(&liquify());
        for p in [J::Null, json!({}), json!({"psd": 12}), json!({"psd": "zz"}), json!({"psd": "abc"}), json!({"psd": to_hex(b"not a descriptor")})] {
            assert!(stored_mesh(&p).is_none(), "{p}");
            assert!(item_for_filter(&SmartFilter { params: p.clone(), ..ok.clone() }).is_err(), "{p}");
        }
        assert!(stored_mesh(&json!({"psd": "0".repeat(MAX_TEMPLATE_BYTES * 2 + 2)})).is_none());
        assert!(stored_mesh(&json!({"psd": to_hex(&Descriptor::new("filterFX").to_bytes())})).is_none());
    }

    /// The mesh sits at the bottom-left corner of the filter buffer Photoshop stored (here taller
    /// than the mesh's frame, as when the smart object was moved up after the Liquify was made),
    /// a node at the centre of each cell, its displacement scaled from cells to pixels.
    #[test]
    fn the_mesh_sits_at_the_bottom_left_of_the_filter_buffer() {
        let mut f = filter_from_item(&liquify());
        set_frame(&mut f.params, Rect::new(10, 5, 30, 26));
        assert_eq!(stored_frame(&f.params), Some(Rect::new(10, 5, 30, 26)));
        let input = Rect::new(13, 8, 27, 22);
        assert_eq!(trusted_frame(&f.params, input), Some(Rect::new(10, 5, 30, 26)), "a normal frame is kept unchanged");
        let (m, area) = placed_mesh(&f.params, input).unwrap();
        assert_eq!(area, Rect::new(10, 5, 30, 26));
        // 20 × 13 px in 5 × 3 cells.
        let cell = [4.0, 13.0 / 3.0];
        assert_eq!(m.cell, cell);
        assert_eq!(m.origin, [10.0 + 2.0, 26.0 - 13.0 + cell[1] / 2.0]);
        assert_eq!((m.cols, m.rows), (5, 3));
        assert!(m.is_valid());
        let px: Vec<[f32; 2]> = nodes().iter().map(|v| [v[0] * 4.0, (f64::from(v[1]) * cell[1]) as f32]).collect();
        assert_eq!(m.d, px);
        // The output at node (3, 0) reads the input 4 px to its right.
        assert_eq!(m.at(m.origin[0] + 3.0 * cell[0], m.origin[1]), [4.0, 0.0]);
    }

    /// Without a stored frame the buffer is the filter input's bounds plus 3 px.
    #[test]
    fn without_a_frame_the_buffer_is_the_input_plus_a_margin() {
        let f = filter_from_item(&liquify());
        assert_eq!(stored_frame(&f.params), None);
        let (m, area) = placed_mesh(&f.params, Rect::new(40, 50, 54, 60)).unwrap();
        assert_eq!(area, Rect::new(37, 47, 57, 63));
        assert_eq!(m.origin, [39.0, 63.0 - 13.0 + 13.0 / 6.0]);
        assert!(placed_mesh(&f.params, Rect::default()).is_none(), "nothing to render over");
        assert!(placed_mesh(&json!({"frame": [0, 0, 8, 8]}), Rect::new(0, 0, 8, 8)).is_none(), "no mesh");
    }

    #[test]
    fn odd_frames_are_ignored() {
        let f = filter_from_item(&liquify());
        for frame in
            [json!([1, 2, 3]), json!([0, 0, 0, 5]), json!([4, 0, 2, 5]), json!([0, 0, 1.5, 4]), json!("0,0,4,4"), json!([0, 0, 1i64 << 40, 4]), json!(null)]
        {
            let mut p = f.params.clone();
            p["frame"] = frame.clone();
            assert_eq!(stored_frame(&p), None, "{frame}");
            assert_eq!(placed_mesh(&p, Rect::new(0, 0, 8, 8)).map(|m| m.1), Some(Rect::new(-3, -3, 11, 11)), "{frame}");
        }
        // A frame is an addition: the item still goes back byte for byte.
        let mut g = f.clone();
        set_frame(&mut g.params, Rect::new(-5, -5, 50, 50));
        assert_eq!(item_for_filter(&g).unwrap().to_bytes(), liquify().to_bytes());
    }

    /// The stored frame comes from the file: one that can't be Photoshop's buffer for this smart
    /// object (far from its content, smaller than the mesh, much larger than it, or huge) is not
    /// trusted, and the buffer is the input's bounds plus 3 px, so nothing is allocated past them.
    #[test]
    fn implausible_frames_fall_back_to_the_input() {
        let f = filter_from_item(&liquify());
        let input = Rect::new(40, 50, 54, 60);
        let big = 1 << 24;
        for frame in [
            Rect::new(-big, -big, big, big),
            Rect::new(-big, -big, -big + 20, -big + 13),
            Rect::new(1000, 1000, 1020, 1013),
            Rect::new(40, 50, 59, 63),
            Rect::new(40, 50, 60, 62),
            Rect::new(-2000, 50, 60, 63),
            Rect::new(40, -2000, 60, 63),
        ] {
            let mut p = f.params.clone();
            set_frame(&mut p, frame);
            assert_eq!(stored_frame(&p), Some(frame), "{frame:?}");
            assert_eq!(trusted_frame(&p, input), None, "{frame:?}");
            assert_eq!(placed_mesh(&p, input).map(|m| m.1), Some(input.inflate(3)), "{frame:?}");
        }
        // Mesh-sized, or grown to keep the input inside: trusted.
        for frame in [Rect::new(40, 50, 60, 63), Rect::new(37, 47, 60, 70), Rect::new(0, 0, 60, 63)] {
            let mut p = f.params.clone();
            set_frame(&mut p, frame);
            assert_eq!(trusted_frame(&p, input), Some(frame), "{frame:?}");
        }
        // Nothing to compare with: not trusted.
        let mut p = f.params.clone();
        set_frame(&mut p, Rect::new(40, 50, 60, 63));
        assert_eq!(trusted_frame(&p, Rect::default()), None);
    }

    /// Corrupted bytes of a Liquify item never panic, whatever they decode to.
    #[test]
    fn mutated_liquify_never_panics() {
        let bytes = liquify().to_bytes();
        let mut state = 0x2545_F491_4F6C_DD1Du64;
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
                if let Some(m) = stored_mesh(&f.params) {
                    assert_eq!(m.d.len(), m.cols * m.rows);
                    decoded += 1;
                }
            }
        }
        assert!(decoded > 0, "some mutations only touch values that still decode");
    }
}
