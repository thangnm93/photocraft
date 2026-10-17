//! A Puppet Warp smart filter from a Photoshop PSD renders from the deformed mesh Photoshop
//! stored (`psd.puppetWarp`): through the smart filter pipeline (blending, visibility, the filter
//! mask), following the smart object when it moves, and written back to PSD byte for byte.

use photocraft_doc::{Document, LayerContent, LayerId, LayerMask, SmartFilter, SmartObject};
use photocraft_engine::Session;
use photocraft_engine::distort_cmds::PSD_PUPPET;
use photocraft_engine::smart_cmds::{decode_source, refresh_layer, source_bytes};
use photocraft_geom::{Affine, Rect};
use photocraft_psd::descriptor::{Descriptor, Id, UnicodeString, Value};
use photocraft_raster::Surface;
use serde_json::json;

const W: i32 = 96;
const H: i32 = 64;
/// The painted block.
const BLOCK: Rect = Rect { x0: 8, y0: 6, x1: 56, y1: 46 };

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

/// Where the smart object's placement puts its corners, as PSD's `Trnf` stores them.
fn corners(doc: &Document, sm: &SmartObject) -> [[f64; 2]; 4] {
    let (name, bytes) = source_bytes(&doc.metadata, &sm.source).unwrap();
    let b = decode_source(&name, &bytes).unwrap().bounds();
    photocraft_io::smart_map::placement_quad(sm.transform, sm.perspective, (f64::from(b.width()), f64::from(b.height())), sm.warp.as_ref())
}

/// A Puppet Warp `filterFXList` item as Photoshop writes one: a grid mesh over `area` (`n` × `n`
/// cells), its vertices moved by `f`, made for the placement corners `quad`.
fn puppet_item(area: Rect, n: usize, f: impl Fn([f32; 2]) -> [f32; 2], quad: [[f64; 2]; 4]) -> Descriptor {
    let (mut orig, mut def, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=n {
        for i in 0..=n {
            let p = [area.x0 as f32 + area.width() as f32 * i as f32 / n as f32, area.y0 as f32 + area.height() as f32 * j as f32 / n as f32];
            orig.extend(p.iter().flat_map(|v| v.to_le_bytes()));
            def.extend(f(p).iter().flat_map(|v| v.to_le_bytes()));
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let m = n as u32;
            for v in [a, a + 1, a + m + 2, a, a + m + 2, a + m + 1] {
                tris.extend(v.to_le_bytes());
            }
        }
    }
    let shape = Descriptor::new("puppetShape")
        .with("rigidType", Value::Boolean(true))
        .with("VrsM", Value::Integer(1))
        .with("VrsN", Value::Integer(0))
        .with("originalVertexArray", Value::RawData(orig))
        .with("deformedVertexArray", Value::RawData(def))
        .with("indexArray", Value::RawData(tris))
        .with("meshQuality", Value::Integer(2))
        .with("meshExpansion", Value::Integer(2))
        .with("meshRigidity", Value::Integer(2))
        .with("imageResolution", Value::Double(72.0));
    let mut fltr =
        Descriptor::new("rigidTransform").with("rigidType", Value::Boolean(true)).with("puppetShapeList", Value::List(vec![Value::Descriptor(shape)]));
    for (i, [x, y]) in quad.into_iter().enumerate() {
        fltr = fltr.with(&format!("PuX{i}"), Value::Double(x)).with(&format!("PuY{i}"), Value::Double(y));
    }
    let blend = Descriptor::new("blendOptions")
        .with("Opct", Value::UnitFloat { unit: *b"#Prc", value: 100.0 })
        .with("Md  ", Value::Enumerated { type_id: Id::new("BlnM"), value: Id::new("Nrml") });
    Descriptor::new("filterFX")
        .with("Nm  ", Value::Text(UnicodeString::new_nul("Puppet Warp")))
        .with("blendOptions", Value::Descriptor(blend))
        .with("enab", Value::Boolean(true))
        .with("hasoptions", Value::Boolean(true))
        .with("Fltr", Value::Descriptor(fltr))
        .with("filterID", Value::Integer(990))
}

/// Adds `filter` to the smart object and re-renders it.
fn add(s: &mut Session, id: LayerId, filter: SmartFilter) {
    s.edit("puppet", |doc, _| {
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

/// The mesh moves the whole block 5 px right and 3 px down: the smart object shows the block
/// moved by exactly that, and nothing outside the mesh.
#[test]
fn puppet_warp_renders_from_its_stored_mesh() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let placed = cache(&s, id);
        let quad = corners(&s.active().unwrap().doc, &smart(&s.active().unwrap().doc, id));
        let filter = photocraft_io::smart_map::filter_from_item(&puppet_item(BLOCK, 4, |p| [p[0] + 5.0, p[1] + 3.0], quad));
        assert_eq!(filter.command, PSD_PUPPET);
        add(&mut s, id, filter);
        let out = cache(&s, id);
        let moved = photocraft_algo::resample::translate_surface(&placed, 5, 3);
        assert_eq!(out.content_bounds(), moved.content_bounds(), "depth {depth}");
        assert!(worst(&out, &moved, moved.content_bounds().inflate(1)) <= 1e-5, "depth {depth}: {}", worst(&out, &moved, moved.content_bounds()));

        // A mesh over the left half only: the right half isn't shown.
        let (mut s, id) = session(depth);
        let left = Rect::new(BLOCK.x0, BLOCK.y0, 32, BLOCK.y1);
        add(&mut s, id, photocraft_io::smart_map::filter_from_item(&puppet_item(left, 2, |p| p, quad)));
        assert_eq!(cache(&s, id).content_bounds(), left, "depth {depth}");
    }
}

/// Blending, opacity, visibility and the filter mask apply as to any smart filter.
#[test]
fn puppet_warp_goes_through_the_smart_filter_pipeline() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let placed = cache(&s, id);
        let quad = corners(&s.active().unwrap().doc, &smart(&s.active().unwrap().doc, id));
        // Moved 20 px right: (60, 20) is covered only by the warped block, (10, 20) only by the
        // original.
        add(&mut s, id, photocraft_io::smart_map::filter_from_item(&puppet_item(BLOCK, 4, |p| [p[0] + 20.0, p[1]], quad)));
        let full = cache(&s, id);
        assert_eq!((full.pixel(60, 20)[3], full.pixel(10, 20)[3]), (1.0, 0.0), "depth {depth}");
        let l = json!({"layer": id.0, "index": 0});
        s.execute("layer.smartFilter.blendingOptions", json!({"layer": id.0, "index": 0, "opacity": 50})).unwrap();
        // At half opacity the warped result lies over the unwarped pixels at half strength.
        let half = cache(&s, id);
        assert!((half.pixel(60, 20)[3] - 0.5).abs() < 0.01, "depth {depth}: {:?}", half.pixel(60, 20));
        assert_eq!(half.pixel(10, 20), placed.pixel(10, 20), "depth {depth}");
        // Multiply: where the warped and unwarped blocks overlap, the product of the two.
        s.execute("layer.smartFilter.blendingOptions", json!({"layer": id.0, "index": 0, "blend": "multiply", "opacity": 100})).unwrap();
        let rgba = |v: Vec<f32>| [v[0], v[1], v[2], v[3]];
        let expected = photocraft_color::blend::composite(photocraft_color::BlendMode::Multiply, rgba(placed.pixel(40, 20)), rgba(full.pixel(40, 20)), 1.0);
        let got = rgba(cache(&s, id).pixel(40, 20));
        assert!(got.iter().zip(expected).all(|(a, b)| (a - b).abs() < 2.5 / 255.0), "depth {depth}: {got:?} vs {expected:?}");
        assert_ne!(got, rgba(full.pixel(40, 20)), "depth {depth}: multiply changes the result");
        s.execute("layer.smartFilter.blendingOptions", json!({"layer": id.0, "index": 0, "blend": "normal", "opacity": 50})).unwrap();
        s.execute("layer.smartFilter.setVisible", json!({"layer": id.0, "index": 0, "visible": false})).unwrap();
        assert_eq!(cache(&s, id), placed, "depth {depth}: hidden");
        // Editing the stored mesh is refused; it can still be shown, re-blended and deleted.
        assert!(s.execute("layer.smartFilter.setParams", json!({"layer": id.0, "index": 0, "params": {"psd": ""}})).is_err());
        s.execute("layer.smartFilter.setVisible", json!({"layer": id.0, "index": 0, "visible": true})).unwrap();
        s.execute("layer.smartFilter.blendingOptions", json!({"layer": id.0, "index": 0, "opacity": 100})).unwrap();
        assert!(worst(&cache(&s, id), &full, Rect::new(0, 0, W, H)) <= 1e-6, "depth {depth}: shown again");
        // A filter mask over the left half of the canvas: there the unfiltered pixels show.
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
        s.execute("layer.smartFilter.delete", l).unwrap();
        assert!(smart(&s.active().unwrap().doc, id).smart_filters.is_empty());
    }
}

/// When the smart object moves after the warp was made, the mesh moves with it: the re-render is
/// the old result moved by the same amount, not the content falling out of a mesh left behind.
#[test]
fn puppet_warp_follows_the_smart_object() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let quad = corners(&s.active().unwrap().doc, &smart(&s.active().unwrap().doc, id));
        add(&mut s, id, photocraft_io::smart_map::filter_from_item(&puppet_item(BLOCK, 4, |p| [p[0] + 2.0 + 0.1 * (p[1] - 6.0), p[1]], quad)));
        let before = cache(&s, id);
        s.edit("move", |doc, _| {
            let mut l = doc.layer(id).unwrap().clone();
            let LayerContent::Smart(sm) = &mut l.content else { panic!("smart") };
            sm.transform = Affine::translate(30.0, 10.0).mul(&sm.transform);
            assert!(refresh_layer(doc, &mut l).unwrap());
            *doc.layer_mut(id).unwrap() = l;
            Ok(())
        })
        .unwrap();
        let after = cache(&s, id);
        let expected = photocraft_algo::resample::translate_surface(&before, 30, 10);
        assert_eq!(after.content_bounds(), expected.content_bounds(), "depth {depth}");
        assert!(worst(&after, &expected, expected.content_bounds()) < 1e-3, "depth {depth}: {}", worst(&after, &expected, expected.content_bounds()));
    }
}

/// Through a PSD save and open and a `.pcraft` save and load the filter comes back the same and
/// renders the same; the exported item is the one Photoshop wrote, byte for byte.
#[test]
fn puppet_warp_survives_psd_and_native_saves() {
    for depth in [8, 16, 32] {
        let (mut s, id) = session(depth);
        let quad = corners(&s.active().unwrap().doc, &smart(&s.active().unwrap().doc, id));
        let item = puppet_item(BLOCK, 3, |p| [p[0] + 0.05 * p[1], p[1] - 2.0], quad);
        let filter = photocraft_io::smart_map::filter_from_item(&item);
        add(&mut s, id, filter.clone());
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

        let back = photocraft_io::import("puppet.psd", &out.bytes).unwrap().document;
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
            assert_eq!(sm.smart_filters, vec![filter.clone()], "depth {depth}");
            let mut l = doc2.layer(bid).unwrap().clone();
            assert!(refresh_layer(doc2, &mut l).unwrap());
            let LayerContent::Smart(sm) = &l.content else { panic!("smart") };
            let (a, b) = (sm.cache.as_ref().unwrap(), smart(&doc, id).cache.unwrap());
            assert!(worst(a, &b, Rect::new(0, 0, W, H)) <= 1e-6, "depth {depth}: re-render");
        }
    }
}
