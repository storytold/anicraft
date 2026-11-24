use super::*;
use crate::{ColorEffect, Keyframe, Layer, LayerId, Paint, Rgba, Shape, Symbol, SymbolId, SymbolKind, Timeline, Tween, evaluate};
use anicraft_geom::{Affine, Path, Vec2};

fn col(name: &str, v: &[Option<f64>]) -> DataColumn {
    DataColumn { name: name.into(), values: v.to_vec() }
}

fn nums(name: &str, v: &[f64]) -> DataColumn {
    DataColumn { name: name.into(), values: v.iter().copied().map(Some).collect() }
}

/// A document with a graphic symbol (three frames: a 10, 20 and 30 pixel square) and one
/// instance of it (element 50) at (100, 200) on frames 0..40.
fn doc() -> Document {
    let mut d = Document::default();
    let sid = SymbolId(40);
    let mut inner = Layer::new(LayerId(41), "art");
    inner.keyframes = (0..3u32)
        .map(|f| {
            let mut k = Keyframe::new(f, 1);
            let side = 10.0 * (f + 1) as f32;
            k.elements.push(Arc::new(Element { id: ElementId(42 + u64::from(f)), name: String::new(), transform: Affine::IDENTITY, color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Shape(Shape::filled(Path::rect(0.0, 0.0, side, side), Paint::Solid(Rgba::BLACK))) }));
            Arc::new(k)
        })
        .collect();
    d.library.push(Arc::new(Symbol { id: sid, name: "Bar".into(), kind: SymbolKind::Graphic, linkage: None, scale_grid_kept: None, scale_grid: None, timeline: Timeline { layers: vec![Arc::new(inner)] } }));
    let mut k = Keyframe::new(0, 40);
    k.elements.push(Arc::new(instance(50, sid, 100.0, 200.0)));
    Arc::make_mut(&mut d.timeline_mut(0).layers[0]).keyframes = vec![Arc::new(k)];
    d.next_id = 100;
    d
}

fn instance(id: u64, sid: SymbolId, x: f32, y: f32) -> Element {
    Element {
        id: ElementId(id),
        name: String::new(),
        transform: Affine::translate(x, y),
        color: ColorEffect::default(),
        transform_3d: None,
        kind: ElementKind::Symbol { symbol: sid, loop_mode: LoopMode::SingleFrame, first_frame: 0, blend: Default::default(), filters: Vec::new(), events: Vec::new(), pivot: None },
    }
}

fn add_table(d: &mut Document, t: DataTable) {
    Arc::make_mut(&mut d.data).tables.push(Arc::new(t));
}

fn binding(property: BoundProperty, column: &str, frames: u32) -> Binding {
    Binding { element: ElementId(50), property, table: DataId(90), column: column.into(), start: 0, frames, from: None, to: None, interp: Interp::Linear, scale: 1.0, offset: 0.0 }
}

fn bind(d: &mut Document, b: Binding) {
    assert!(Arc::make_mut(&mut d.data).bind(b));
}

#[test]
fn a_table_is_repaired_to_the_rules() {
    let t = DataTable::new(DataId(1), "t", vec![col("  a ", &[Some(1.0), Some(f64::NAN), Some(f64::INFINITY)]), col("A", &[Some(2.0)]), col("", &[]), col("\u{7}", &[])], Some("nope".into()));
    assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "A 2", "Column 3", "Column 4"]);
    assert_eq!(t.rows(), 3);
    assert_eq!(t.columns[0].values, [Some(1.0), None, None], "cells that aren't finite are gaps");
    assert_eq!(t.columns[1].values, [Some(2.0), None, None], "a short column is padded with gaps");
    assert_eq!(t.key, None, "a key that names no column is dropped");
    assert!(t.is_normal());
    let mut again = t.clone();
    again.normalize();
    assert_eq!(again, t, "normalize is idempotent");
}

#[test]
fn a_table_is_cut_to_its_limits() {
    let wide = DataTable::new(DataId(1), "t", (0..MAX_COLUMNS + 20).map(|i| nums(&format!("c{i}"), &[1.0])).collect(), None);
    assert_eq!(wide.columns.len(), MAX_COLUMNS);
    let long = DataTable::new(DataId(1), "t", vec![DataColumn { name: "a".into(), values: vec![Some(1.0); MAX_ROWS + 5] }], None);
    assert_eq!(long.rows(), MAX_ROWS);
    let big = DataTable::new(DataId(1), "t", (0..100).map(|i| DataColumn { name: format!("c{i}"), values: vec![Some(1.0); 20_000] }).collect(), None);
    assert!(big.rows() * big.columns.len() <= MAX_CELLS);
    assert_eq!(big.rows(), MAX_CELLS / 100);
}

#[test]
fn a_keyed_table_keeps_its_rows_in_key_order() {
    let t = DataTable::new(DataId(1), "t", vec![col("age", &[Some(30.0), None, Some(10.0), Some(20.0)]), nums("v", &[3.0, 9.0, 1.0, 2.0])], Some("AGE".into()));
    assert_eq!(t.key.as_deref(), Some("age"), "the key takes the column's own spelling");
    assert_eq!(t.columns[0].values, [Some(10.0), Some(20.0), Some(30.0), None]);
    assert_eq!(t.columns[1].values, [Some(1.0), Some(2.0), Some(3.0), Some(9.0)]);
    assert_eq!(t.key_range(), Some((10.0, 30.0)));
    // The row without a key plays no part.
    assert_eq!(t.sample(1, 1_000.0, Interp::Linear), Some(3.0));
}

#[test]
fn sampling_by_row_number() {
    let t = DataTable::new(DataId(1), "t", vec![nums("v", &[10.0, 20.0, 40.0])], None);
    let s = |k, i| t.sample(0, k, i);
    assert_eq!(s(0.0, Interp::Linear), Some(10.0));
    assert_eq!(s(0.5, Interp::Linear), Some(15.0));
    assert_eq!(s(1.0, Interp::Linear), Some(20.0));
    assert_eq!(s(1.75, Interp::Linear), Some(35.0));
    assert_eq!(s(2.0, Interp::Linear), Some(40.0));
    assert_eq!(s(0.99, Interp::Step), Some(10.0));
    assert_eq!(s(1.0, Interp::Step), Some(20.0));
    assert_eq!(s(1.99, Interp::Step), Some(20.0));
    // Outside the rows: the nearest end's value.
    assert_eq!(s(-5.0, Interp::Linear), Some(10.0));
    assert_eq!(s(1e12, Interp::Step), Some(40.0));
    assert_eq!(s(f64::NAN, Interp::Linear), None);
    assert_eq!(t.sample(7, 0.0, Interp::Linear), None, "no such column");
}

#[test]
fn sampling_skips_gaps() {
    let t = DataTable::new(DataId(1), "t", vec![col("v", &[None, Some(10.0), None, None, Some(40.0), None]), col("none", &[None; 6])], None);
    assert_eq!(t.sample(0, 0.0, Interp::Linear), Some(10.0), "before the first number: it");
    assert_eq!(t.sample(0, 2.0, Interp::Linear), Some(20.0), "across a gap, by row number");
    assert_eq!(t.sample(0, 3.0, Interp::Step), Some(10.0));
    assert_eq!(t.sample(0, 5.0, Interp::Linear), Some(40.0));
    assert_eq!(t.sample(1, 2.0, Interp::Linear), None, "a column of gaps has no value");
    let empty = DataTable::new(DataId(2), "e", vec![col("v", &[])], None);
    assert_eq!(empty.sample(0, 0.0, Interp::Linear), None);
    assert_eq!(empty.key_range(), None);
}

#[test]
fn sampling_by_key() {
    let t = DataTable::new(DataId(1), "t", vec![nums("depth", &[0.0, 10.0, 10.0, 30.0]), nums("v", &[1.0, 2.0, 5.0, 9.0])], Some("depth".into()));
    assert_eq!(t.sample(1, 5.0, Interp::Linear), Some(1.5));
    assert_eq!(t.sample(1, 10.0, Interp::Linear), Some(5.0), "the last row of a repeated key wins");
    assert_eq!(t.sample(1, 20.0, Interp::Linear), Some(7.0));
    assert_eq!(t.sample(1, 29.0, Interp::Step), Some(5.0));
    assert_eq!(t.sample(1, -1.0, Interp::Step), Some(1.0));
    assert_eq!(t.sample(1, 31.0, Interp::Linear), Some(9.0));
}

#[test]
fn one_frame_per_row_is_exact() {
    // 997 rows, a prime, so row/(rows - 1) is rarely exact in binary.
    let rows: Vec<f64> = (0..997).map(|i| f64::from(i) * 3.0).collect();
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("v", &rows)], None));
    let b = Binding { interp: Interp::Step, ..binding(BoundProperty::X, "v", 997) };
    for f in 0..997u32 {
        assert_eq!(b.key_at(d.data_table(DataId(90)).unwrap(), f), Some(f64::from(f)), "frame {f}");
        assert_eq!(b.value_at(&d.data, f), Some(f as f32 * 3.0), "frame {f}");
    }
}

#[test]
fn a_span_maps_onto_keys_forwards_and_backwards_and_holds_outside() {
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("age", &[100.0, 200.0, 300.0]), nums("v", &[1.0, 2.0, 3.0])], Some("age".into())));
    let fwd = Binding { start: 10, ..binding(BoundProperty::X, "v", 21) };
    let v = |b: &Binding, f| b.value_at(&d.data, f).unwrap();
    assert_eq!(v(&fwd, 0), 1.0, "before the span: its first frame's value");
    assert_eq!(v(&fwd, 10), 1.0);
    assert_eq!(v(&fwd, 20), 2.0);
    assert_eq!(v(&fwd, 25), 2.5);
    assert_eq!(v(&fwd, 30), 3.0);
    assert_eq!(v(&fwd, 5_000), 3.0, "after the span: its last frame's value");
    let back = Binding { from: Some(300.0), to: Some(100.0), ..fwd.clone() };
    assert_eq!(v(&back, 10), 3.0);
    assert_eq!(v(&back, 20), 2.0);
    assert_eq!(v(&back, 30), 1.0);
    let part = Binding { from: Some(150.0), to: Some(250.0), scale: 10.0, offset: -1.0, ..fwd.clone() };
    assert_eq!(v(&part, 10), 14.0);
    assert_eq!(v(&part, 30), 24.0);
    let one = Binding { frames: 1, ..fwd };
    assert_eq!(v(&one, 0), 1.0);
    assert_eq!(v(&one, 99), 1.0, "a one-frame span shows its first key");
}

fn placed(d: &Document, frame: u32) -> Affine {
    evaluate(d, frame).items[0].transform
}

#[test]
fn bindings_place_the_instance_in_evaluate() {
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("x", &[0.0, 300.0]), nums("s", &[100.0, 300.0]), nums("r", &[0.0, 90.0])], None));
    assert_eq!(placed(&d, 5).translation(), Vec2::new(100.0, 200.0), "a table alone changes nothing");
    bind(&mut d, binding(BoundProperty::X, "x", 11));
    assert_eq!(placed(&d, 0).translation(), Vec2::new(0.0, 200.0));
    assert!((placed(&d, 5).translation().x - 150.0).abs() < 1e-3);
    assert_eq!(placed(&d, 10).translation(), Vec2::new(300.0, 200.0));
    assert_eq!(placed(&d, 30).translation().x, 300.0, "held after the span");
    bind(&mut d, binding(BoundProperty::ScaleY, "s", 11));
    let m = placed(&d, 10);
    assert!((m.apply(Vec2::new(10.0, 10.0)) - Vec2::new(310.0, 230.0)).length() < 1e-3, "{m:?}");
    bind(&mut d, binding(BoundProperty::Rotation, "r", 11));
    // Turned a quarter clockwise at the end: the symbol's x axis points down the stage.
    let m = placed(&d, 10);
    assert!((m.apply(Vec2::new(10.0, 0.0)) - Vec2::new(300.0, 210.0)).length() < 1e-3, "{m:?}");
    // The document itself is untouched: time is a function.
    let own = d.timeline(0).layers[0].keyframes[0].elements[0].transform;
    assert_eq!(own, Affine::translate(100.0, 200.0));
}

#[test]
fn scale_and_rotation_turn_about_the_transformation_point() {
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("s", &[200.0])], None));
    let l = Arc::make_mut(&mut d.timeline_mut(0).layers[0]);
    Arc::make_mut(&mut l.keyframe_mut(0).unwrap().elements[0]).set_pivot(Some(Vec2::new(0.0, 10.0)));
    bind(&mut d, binding(BoundProperty::ScaleY, "s", 1));
    let m = placed(&d, 0);
    // The point (the bar's foot) stays where it was; the top goes up.
    assert!((m.apply(Vec2::new(0.0, 10.0)) - Vec2::new(100.0, 210.0)).length() < 1e-3, "{m:?}");
    assert!((m.apply(Vec2::new(0.0, 0.0)) - Vec2::new(100.0, 190.0)).length() < 1e-3, "{m:?}");
}

#[test]
fn alpha_and_the_frame_shown_follow_a_column() {
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("a", &[100.0, 0.0]), nums("level", &[0.0, 1.0, 2.0, 7.0])], None));
    // (The table has four rows; the alpha column has numbers on the first two.)
    bind(&mut d, Binding { to: Some(1.0), ..binding(BoundProperty::Alpha, "a", 11) });
    let alpha = |d: &Document, f| {
        let dl = evaluate(d, f);
        dl.effective_alpha(&dl.items[0])
    };
    assert!((alpha(&d, 0) - 1.0).abs() < 1e-5);
    assert!((alpha(&d, 5) - 0.5).abs() < 1e-5);
    assert!(alpha(&d, 10).abs() < 1e-5);
    // The frame a graphic shows: the symbol's squares are 10, 20 and 30 wide.
    bind(&mut d, Binding { interp: Interp::Step, ..binding(BoundProperty::Frame, "level", 4) });
    let width = |d: &Document, f| evaluate(d, f).items[0].path.bounds().width();
    assert_eq!(width(&d, 0), 10.0);
    assert_eq!(width(&d, 1), 20.0);
    assert_eq!(width(&d, 2), 30.0);
    assert_eq!(width(&d, 3), 30.0, "a frame past the symbol's end shows its last");
    // A looping instance is held on the frame the column picks.
    let l = Arc::make_mut(&mut d.timeline_mut(0).layers[0]);
    if let ElementKind::Symbol { loop_mode, .. } = &mut Arc::make_mut(&mut l.keyframe_mut(0).unwrap().elements[0]).kind {
        *loop_mode = LoopMode::Loop;
    }
    assert_eq!(width(&d, 1), 20.0);
}

#[test]
fn a_binding_follows_its_instance_through_a_tween() {
    let mut d = doc();
    let l = Arc::make_mut(&mut d.timeline_mut(0).layers[0]);
    let mut k0 = Keyframe::new(0, 10);
    k0.elements.push(Arc::new(instance(50, SymbolId(40), 0.0, 0.0)));
    k0.tween = Some(Tween::default());
    let mut k1 = Keyframe::new(10, 1);
    k1.elements.push(Arc::new(instance(50, SymbolId(40), 100.0, 50.0)));
    l.keyframes = vec![Arc::new(k0), Arc::new(k1)];
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("y", &[500.0, 600.0])], None));
    bind(&mut d, binding(BoundProperty::Y, "y", 11));
    // x still tweens; y is the column's.
    let p = placed(&d, 5).translation();
    assert!((p - Vec2::new(50.0, 550.0)).length() < 1e-3, "{p:?}");
    assert_eq!(placed(&d, 10).translation(), Vec2::new(100.0, 600.0));
}

#[test]
fn a_binding_with_nothing_to_read_leaves_the_instance_as_placed() {
    let mut d = doc();
    let at = Vec2::new(100.0, 200.0);
    bind(&mut d, binding(BoundProperty::X, "x", 10));
    assert_eq!(placed(&d, 3).translation(), at, "no such table");
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![col("gaps", &[None, None]), nums("y", &[7.0])], None));
    assert_eq!(placed(&d, 3).translation(), at, "no such column");
    bind(&mut d, binding(BoundProperty::X, "gaps", 10));
    assert_eq!(placed(&d, 3).translation(), at, "a column of gaps");
    // Another property with a value still applies.
    bind(&mut d, binding(BoundProperty::Y, "y", 10));
    assert_eq!(placed(&d, 3).translation(), Vec2::new(100.0, 7.0));
    // A shape is never bound, whatever the bindings say.
    let mut b = binding(BoundProperty::X, "y", 10);
    b.element = ElementId(42);
    bind(&mut d, b);
    assert_eq!(evaluate(&d, 0).items[0].path.bounds().width(), 10.0);
}

#[test]
fn hostile_bindings_and_values_stay_finite() {
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "t", vec![nums("v", &[1e300, -1e300, 1e-300])], None));
    for p in BoundProperty::ALL {
        bind(&mut d, Binding { scale: 1e9, offset: -1e9, start: u32::MAX, frames: 0, from: Some(f64::NAN), to: Some(f64::INFINITY), ..binding(p, "v", 3) });
    }
    assert!(d.data.bindings.iter().all(|b| b.frames >= 1 && b.start <= MAX_SPAN && b.from.is_none() && b.to.is_none()));
    for f in [0, 1, 2, 50, u32::MAX] {
        let dl = evaluate(&d, f.min(39));
        assert!(dl.items.iter().all(|i| i.transform.is_finite()), "frame {f}");
        for b in &d.data.bindings {
            assert!(b.value_at(&d.data, f).is_none_or(f32::is_finite));
        }
    }
}

#[test]
fn bindings_stay_sorted_and_one_per_property() {
    let mut set = DataSet::default();
    for (el, p) in [(9, BoundProperty::Y), (3, BoundProperty::Alpha), (9, BoundProperty::X), (3, BoundProperty::X), (9, BoundProperty::Y)] {
        let mut b = binding(p, "v", 2);
        b.element = ElementId(el);
        assert!(set.bind(b));
    }
    let order: Vec<(u64, BoundProperty)> = set.bindings.iter().map(|b| (b.element.0, b.property)).collect();
    assert_eq!(order, [(3, BoundProperty::X), (3, BoundProperty::Alpha), (9, BoundProperty::X), (9, BoundProperty::Y)]);
    assert_eq!(set.bindings_of(ElementId(9)).len(), 2);
    assert_eq!(set.bindings_of(ElementId(4)).len(), 0);
    assert_eq!(set.unbind(ElementId(9), Some(BoundProperty::X)), 1);
    assert_eq!(set.unbind(ElementId(3), None), 2);
    assert_eq!(set.bindings.len(), 1);
}

#[test]
fn data_round_trips_through_the_file_and_older_files_open() {
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "pollen", vec![nums("age", &[1.0, 2.0]), col("pinus", &[Some(0.5), None])], Some("age".into())));
    bind(&mut d, Binding { interp: Interp::Step, scale: 2.0, offset: 3.0, from: Some(2.0), ..binding(BoundProperty::ScaleY, "pinus", 24) });
    let json = serde_json::to_string(&d).unwrap();
    let mut back: Document = serde_json::from_str(&json).unwrap();
    back.sanitize();
    assert_eq!(back.data, d.data);
    assert_eq!(back, d);
    // A document without data writes no `data` field, and one without the field opens.
    let plain = serde_json::to_value(doc()).unwrap();
    assert!(plain.get("data").is_none());
    let old: Document = serde_json::from_value(plain).unwrap();
    assert!(old.data.is_empty());
}

#[test]
fn loading_repairs_hostile_data() {
    let mut d = doc();
    let folder = FolderId(77);
    let mut bad = DataTable { id: DataId(90), name: " a/b ".into(), columns: vec![col("x", &[Some(f64::NAN)]), col("x", &[Some(1.0), Some(2.0)])], key: Some("zz".into()), source: "C:\\secret\\data.csv".into(), folder: Some(folder) };
    bad.columns[0].values[0] = Some(f64::INFINITY);
    let twin = DataTable { id: DataId(90), ..bad.clone() };
    let same_name = DataTable { id: DataId(91), name: "T".into(), columns: Vec::new(), key: None, source: String::new(), folder: None };
    let same_name_2 = DataTable { id: DataId(92), name: "t".into(), ..same_name.clone() };
    let data = Arc::make_mut(&mut d.data);
    data.tables = vec![Arc::new(bad), Arc::new(twin), Arc::new(same_name), Arc::new(same_name_2)];
    // Unsorted, repeated, and one for an instance that is nowhere.
    data.bindings = vec![binding(BoundProperty::Y, "x", 0), binding(BoundProperty::X, "x", 5), binding(BoundProperty::Y, "x", 9), Binding { element: ElementId(9_999), ..binding(BoundProperty::X, "x", 5) }];
    d.next_id = 1;
    d.sanitize();
    let names: Vec<&str> = d.data.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Table 1", "T", "t 2"]);
    let t = &d.data.tables[0];
    assert_eq!((t.folder, t.key.as_deref(), t.source.as_str()), (None, None, "data.csv"));
    assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["x", "x 2"]);
    assert_eq!(t.columns[0].values, [None, None]);
    assert_eq!(d.data.bindings.iter().map(|b| (b.element.0, b.property, b.frames)).collect::<Vec<_>>(), [(50, BoundProperty::X, 5), (50, BoundProperty::Y, 1)]);
    assert!(d.next_id > 92, "table ids count towards the id counter");
    // Sound data is left alone (the same allocation).
    let before = d.data.clone();
    d.sanitize();
    assert!(Arc::ptr_eq(&before, &d.data));
}

#[test]
fn a_table_is_a_library_item() {
    use crate::library::{LibraryEntry, LibraryItem};
    let mut d = doc();
    add_table(&mut d, DataTable::new(DataId(90), "pollen", vec![nums("v", &[1.0])], None));
    let item = LibraryItem::Data(DataId(90));
    assert!(d.library_items().any(|i| i == item));
    assert_eq!(d.item_name(item), Some("pollen"));
    assert_eq!(d.unused_items(), vec![item], "a table nothing reads is unused");
    bind(&mut d, binding(BoundProperty::X, "v", 1));
    assert_eq!(d.library_uses().get(&item), Some(&1));
    assert!(d.unused_items().is_empty());
    let f = d.ensure_folder_path("Data").unwrap();
    d.place_entry(LibraryEntry::Item(item), Some(f));
    assert_eq!(d.parent_of(LibraryEntry::Item(item)), Some(f));
    assert_eq!(d.entry_path(LibraryEntry::Item(item)), "Data/pollen");
    assert!(d.library_children(Some(f)).contains(&LibraryEntry::Item(item)));
    assert!(d.library_tree.placed.is_empty(), "a table's folder is kept on the table");
    assert!(d.name_taken(Some(f), "POLLEN", None));
}
