//! A Liquify smart filter from a Photoshop PSD renders from the displacement mesh Photoshop stored
//! (`psd.liquify`): through the smart filter pipeline (blending, visibility, the filter mask), in
//! the filter buffer Photoshop used (kept when the contents are replaced), and written back to
//! PSD byte for byte.

use photocraft_doc::{Document, LayerContent, LayerId, LayerMask, SmartFilter, SmartObject};
use photocraft_engine::Session;
use photocraft_engine::distort_cmds::PSD_LIQUIFY;
use photocraft_engine::smart_cmds::{Fit, refresh_layer, replace_contents_fit};
use photocraft_geom::Rect;
use photocraft_psd::descriptor::{Descriptor, Id, UnicodeString, Value};
use photocraft_raster::Surface;
use serde_json::json;

const W: i32 = 96;
const H: i32 = 64;
/// The painted block.
const BLOCK: Rect = Rect { x0: 8, y0: 6, x1: 56, y1: 46 };
/// Photoshop's filter buffer for the smart object: the canvas plus 4 px, 26 × 18 cells of 4 px.
const BUFFER: Rect = Rect { x0: -4, y0: -4, x1: W + 4, y1: H + 4 };

/// A smart object holding a patterned block, at `depth`.
fn session(depth: u64) -> (Session, LayerId) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": W, "height": H, "depth": depth})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.edit("paint", |doc, active| {
        let surf = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
        let fmt = surf.format();
        let mut data = Vec::new();
        for y in BLOCK.y0..BLOCK.y1 {
            for x in BLOCK.x0..BLOCK.x1 {
                let v = ((x / 4 + y / 4) % 2) as f32;
                data.extend(photocraft_raster::from_rgba(&fmt, [v, 1.0 - v, (x as f32) / W as f32, 1.0]));
            }
        }
        surf.write_region(BLOCK, &data);
        Ok(())
    })
    .unwrap();
    let id = s.execute("layer.smartObjects.convertToSmartObject", json!({})).unwrap()["layer"].as_u64().unwrap();
    (s, LayerId(id))
}

fn smart(doc: &Document, id: LayerId) -> SmartObject {
    match &doc.layer(id).unwrap().content {
        LayerContent::Smart(sm) => sm.clone(),
        _ => panic!("not a smart object"),
    }
}

fn cache(s: &Session, id: LayerId) -> Surface {
    smart(&s.active().unwrap().doc, id).cache.unwrap()
}

/// An `LqMe` blob over `BUFFER` (26 × 18 cells of 4 px) with every node displaced by `f(i, j)`
/// cells, run-encoded as Photoshop writes it.
fn mesh(f: impl Fn(usize, usize) -> [f32; 2]) -> Vec<u8> {
    let (cols, rows, w, h) = (26u32, 18u32, BUFFER.width(), BUFFER.height());
    let mut b = vec![0, 0, 0, 4];
    b.extend(b"yfqLhseM");
    for v in [2, cols, rows, 0, 1, 4, 4, h - 4, w - 4, 0, 0, h, w] {
        b.extend(v.to_le_bytes());
    }
    for j in 0..rows as usize {
        let row: Vec<[f32; 2]> = (0..cols as usize).map(|i| f(i, j)).collect();
        let mut i = 0;
        while i < row.len() {
            let still = row[i..].iter().take_while(|v| **v == [0.0, 0.0]).count();
            b.extend((still as u32).to_le_bytes());
            i += still;
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

/// A Liquify `filterFXList` item as Photoshop writes one.
fn liquify_item(lqme: Vec<u8>) -> Descriptor {
    let blend = Descriptor::new("blendOptions")
        .with("Opct", Value::UnitFloat { unit: *b"#Prc", value: 100.0 })
        .with("Md  ", Value::Enumerated { type_id: Id::new("BlnM"), value: Id::new("Nrml") });
    Descriptor::new("filterFX")
        .with("Nm  ", Value::Text(UnicodeString::new_nul("Liquify...")))
        .with("blendOptions", Value::Descriptor(blend))
        .with("enab", Value::Boolean(true))
        .with("hasoptions", Value::Boolean(true))
        .with("Fltr", Value::Descriptor(Descriptor::new("LqFy").with("LqMe", Value::RawData(lqme))))
        .with("filterID", Value::Integer(0x4C71_4679))
}

/// The smart filter for `item`, in Photoshop's filter buffer `BUFFER` (as a PSD import gives it).
fn filter(item: &Descriptor) -> SmartFilter {
    let mut f = photocraft_io::smart_map::filter_from_item(item);
    assert_eq!(f.command, PSD_LIQUIFY);
    photocraft_io::smart_map::liquify_set_frame(&mut f.params, BUFFER);
    f
}

/// Adds `filter` to the smart object and re-renders it.
fn add(s: &mut Session, id: LayerId, filter: SmartFilter) {
    s.edit("liquify", |doc, _| {
        let mut l = doc.layer(id).unwrap().clone();
        let LayerContent::Smart(sm) = &mut l.content else { panic!("smart") };
        sm.smart_filters.push(filter.clone());
        assert!(refresh_layer(doc, &mut l).unwrap());
        *doc.layer_mut(id).unwrap() = l;
        Ok(())
    })
    .unwrap();
}

fn worst(a: &Surface, b: &Surface, r: Rect) -> f32 {
    a.read_region(r).iter().zip(b.read_region(r)).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max)
}

/// Every node reads the input 5 px to its left and 3 px below: the content moves 5 px right and
/// 3 px up, exactly.
#[test]
fn liquify_renders_from_its_stored_mesh() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let placed = cache(&s, id);
        add(&mut s, id, filter(&liquify_item(mesh(|_, _| [-1.25, 0.75]))));
        let out = cache(&s, id);
        let moved = photocraft_algo::resample::translate_surface(&placed, 5, -3);
        assert_eq!(out.content_bounds(), moved.content_bounds(), "depth {depth}");
        assert!(worst(&out, &moved, BUFFER) <= 1e-5, "depth {depth}: {}", worst(&out, &moved, BUFFER));

        // Only the displaced nodes move things: nodes sit at the cells' centres (x = -2, 2, 6, …),
        // and from x = 38 on they read 4 px to the right.
        let (mut s, id) = session(depth);
        add(&mut s, id, filter(&liquify_item(mesh(|i, _| if i >= 10 { [1.0, 0.0] } else { [0.0, 0.0] }))));
        let out = cache(&s, id);
        assert!(worst(&out, &placed, Rect::new(0, 0, 34, H)) <= 1e-6, "depth {depth}: the left part stays");
        assert_eq!(out.pixel(40, 20), placed.pixel(44, 20), "depth {depth}");
        assert_ne!(out.pixel(54, 20), placed.pixel(54, 20), "depth {depth}: the right edge moves in");
    }
}

/// Blending, opacity, visibility and the filter mask apply as to any smart filter; the mesh
/// itself can't be edited.
#[test]
fn liquify_goes_through_the_smart_filter_pipeline() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let placed = cache(&s, id);
        // Moved 20 px right: (60, 20) is covered only by the liquified block, (10, 20) only by the
        // original.
        add(&mut s, id, filter(&liquify_item(mesh(|_, _| [-5.0, 0.0]))));
        let full = cache(&s, id);
        assert_eq!((full.pixel(60, 20)[3], full.pixel(10, 20)[3]), (1.0, 0.0), "depth {depth}");
        s.execute("layer.smartFilter.blendingOptions", json!({"layer": id.0, "index": 0, "opacity": 50})).unwrap();
        let half = cache(&s, id);
        assert!((half.pixel(60, 20)[3] - 0.5).abs() < 0.01, "depth {depth}: {:?}", half.pixel(60, 20));
        assert_eq!(half.pixel(10, 20), placed.pixel(10, 20), "depth {depth}");
        s.execute("layer.smartFilter.blendingOptions", json!({"layer": id.0, "index": 0, "opacity": 100})).unwrap();
        s.execute("layer.smartFilter.setVisible", json!({"layer": id.0, "index": 0, "visible": false})).unwrap();
        assert_eq!(cache(&s, id), placed, "depth {depth}: hidden");
        // Editing the stored mesh is refused; it can still be shown, re-blended and deleted.
        let refused = s.execute("layer.smartFilter.setParams", json!({"layer": id.0, "index": 0, "params": {"psd": ""}})).unwrap_err().to_string();
        assert!(refused.contains("Liquify"), "{refused}");
        s.execute("layer.smartFilter.setVisible", json!({"layer": id.0, "index": 0, "visible": true})).unwrap();
        assert!(worst(&cache(&s, id), &full, Rect::new(0, 0, W, H)) <= 1e-6, "depth {depth}: shown again");
        // A filter mask over the right part of the canvas: on the left the unfiltered pixels show.
        s.edit("mask", |doc, _| {
            let LayerContent::Smart(sm) = &mut doc.layer_mut(id).unwrap().content else { panic!("smart") };
            let mut m = LayerMask::hide_all();
            m.surface.fill_rect(Rect::new(40, 0, W, H), &[1.0]);
            sm.filter_mask = Some(m);
            let mut l = doc.layer(id).unwrap().clone();
            assert!(refresh_layer(doc, &mut l).unwrap());
            *doc.layer_mut(id).unwrap() = l;
            Ok(())
        })
        .unwrap();
        let masked = cache(&s, id);
        assert_eq!((masked.pixel(10, 20)[3], masked.pixel(60, 20)[3]), (1.0, 1.0), "depth {depth}: masked");
        s.execute("layer.smartFilter.delete", json!({"layer": id.0, "index": 0})).unwrap();
        assert!(smart(&s.active().unwrap().doc, id).smart_filters.is_empty());
    }
}

/// Replace Contents keeps the Liquify where Photoshop drew it: the new contents, stretched into
/// the old box, are liquified the same way.
#[test]
fn liquify_applies_to_replaced_contents_in_the_same_place() {
    let (mut s, id) = session(8);
    add(&mut s, id, filter(&liquify_item(mesh(|_, j| if j < 9 { [-1.25, 0.0] } else { [0.0, 0.0] }))));
    let mut design = Document::with_background(
        "design",
        photocraft_geom::Size::new(24, 10),
        photocraft_color::ColorMode::Rgb,
        photocraft_color::SampleType::U8,
        photocraft_color::Color::rgba(0.2, 0.6, 0.9, 1.0),
    );
    design.name = "design.png".into();
    let png = photocraft_io::export(&design, "png", &Default::default()).unwrap().bytes;
    replace_contents_fit(&mut s, id, "design.png", png, Fit::Stretch).unwrap();
    let doc = &s.active().unwrap().doc;
    let sm = smart(doc, id);
    assert_eq!(sm.smart_filters.len(), 1);
    let mut bare = sm.clone();
    bare.smart_filters.clear();
    let placed = photocraft_engine::smart_cmds::render(doc, &bare).unwrap().unwrap();
    // The new block fills the old one's box; its top part moves 5 px right, the bottom stays.
    assert_eq!(placed.content_bounds(), BLOCK);
    let out = sm.cache.unwrap();
    let top = Rect::new(0, 0, W, 10);
    assert!(worst(&out, &photocraft_algo::resample::translate_surface(&placed, 5, 0), top) <= 1e-5);
    assert!(worst(&out, &placed, Rect::new(0, 40, W, H)) <= 1e-6);
}

/// Through a PSD save and open and a `.pcraft` save and load the filter comes back the same
/// (its filter buffer included) and renders the same; the exported item is the one Photoshop
/// wrote, byte for byte.
#[test]
fn liquify_survives_psd_and_native_saves() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let item = liquify_item(mesh(|i, j| [0.05 * i as f32 - 0.6, 0.5 - 0.04 * j as f32]));
        let f = filter(&item);
        add(&mut s, id, f.clone());
        let doc = s.active().unwrap().doc.clone();
        let out = photocraft_io::export(&doc, "psd", &Default::default()).unwrap();
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let file = photocraft_psd::PsdFile::from_bytes(&out.bytes).unwrap();
        let sold = file.layers().iter().flat_map(|l| &l.blocks).find(|b| &b.key == b"SoLd").expect("a SoLd block");
        let placed = photocraft_io::smart_map::parse_sold(&sold.data).unwrap();
        let Some(Value::Descriptor(fx)) = placed.descriptor.get("filterFX") else { panic!("no filterFX") };
        let Some(Value::List(list)) = fx.get("filterFXList") else { panic!("no filterFXList") };
        let Some(Value::Descriptor(written)) = list.first() else { panic!("no filter") };
        assert_eq!(written.to_bytes(), item.to_bytes(), "depth {depth}");

        let back = photocraft_io::import("liquify.psd", &out.bytes).unwrap().document;
        let native = photocraft_format::load_from_bytes(&photocraft_format::save_to_bytes(&back, &Default::default()).unwrap()).unwrap();
        for doc2 in [&back, &native] {
            let (bid, sm) = doc2
                .walk()
                .into_iter()
                .find_map(|(_, _, l)| match &l.content {
                    LayerContent::Smart(sm) => Some((l.id, sm.clone())),
                    _ => None,
                })
                .unwrap();
            assert_eq!(sm.smart_filters, vec![f.clone()], "depth {depth}");
            let mut l = doc2.layer(bid).unwrap().clone();
            assert!(refresh_layer(doc2, &mut l).unwrap());
            let LayerContent::Smart(sm) = &l.content else { panic!("smart") };
            let (a, b) = (sm.cache.as_ref().unwrap(), smart(&doc, id).cache.unwrap());
            assert!(worst(a, &b, Rect::new(0, 0, W, H)) <= 1e-6, "depth {depth}: re-render");
        }
    }
}
