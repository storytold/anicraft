//! 3D Postcard rotation and Z-depth commands (`P-044`): `modify.set_3d_transform` and
//! `modify.reset_3d_transform`.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{
    ElementId, ElementKind, MAX_3D_ANGLE_DEG, MAX_3D_Z, MAX_VANISHING, Transform3D,
};
use anicraft_geom::Vec2;
use anicraft_geom::perspective3d::{DEFAULT_FOV_DEG, MAX_FOV_DEG, MIN_FOV_DEG};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "modify.set_3d_transform",
            label: "3D Rotation & Translation",
            menu: "",
            shortcut: None,
            params: "{rx?: degrees, ry?: degrees, rz?: degrees, z?: pixels, fov?: 10..150, vanishing?: [x, y] | null, element?}",
            mutates: true,
            run: set_3d_transform,
        },
        CommandSpec {
            id: "modify.reset_3d_transform",
            label: "Reset 3D Transform",
            menu: "Modify/Transform",
            shortcut: None,
            params: "{element?}",
            mutates: true,
            run: reset_3d_transform,
        },
    ]
}

fn parse_vanishing(p: &Value) -> Result<Option<Option<Vec2>>> {
    let Some(v) = p.get("vanishing") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(Some(None));
    }
    let arr = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| EngineError::Param("param 'vanishing' must be [x, y] or null".into()))?;
    let x = arr
        .first()
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| n as f32)
        .ok_or_else(|| EngineError::Param("param 'vanishing' needs two finite numbers [x, y]".into()))?;
    let y = arr
        .get(1)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| n as f32)
        .ok_or_else(|| EngineError::Param("param 'vanishing' needs two finite numbers [x, y]".into()))?;
    if x.abs() > MAX_VANISHING || y.abs() > MAX_VANISHING {
        return Err(EngineError::Param(format!("param 'vanishing' must be within ±{MAX_VANISHING} px")));
    }
    Ok(Some(Some(Vec2::new(x, y))))
}

fn checked_angle(p: &Value, key: &str) -> Result<Option<f32>> {
    match opt_f32(p, key)? {
        None => Ok(None),
        Some(v) if v.is_finite() && v.abs() <= MAX_3D_ANGLE_DEG => Ok(Some(v)),
        Some(_) => Err(EngineError::Param(format!("param '{key}' must be within ±{MAX_3D_ANGLE_DEG}°"))),
    }
}

fn checked_z(p: &Value, key: &str) -> Result<Option<f32>> {
    match opt_f32(p, key)? {
        None => Ok(None),
        Some(v) if v.is_finite() && v.abs() <= MAX_3D_Z => Ok(Some(v)),
        Some(_) => Err(EngineError::Param(format!("param '{key}' must be within ±{MAX_3D_Z} px"))),
    }
}

fn checked_fov(p: &Value) -> Result<Option<f32>> {
    match opt_f32(p, "fov")? {
        None => Ok(None),
        Some(v) if v.is_finite() && (MIN_FOV_DEG..=MAX_FOV_DEG).contains(&v) => Ok(Some(v)),
        Some(_) => Err(EngineError::Param(format!("param 'fov' must be between {MIN_FOV_DEG} and {MAX_FOV_DEG}°"))),
    }
}

fn set_3d_transform(s: &mut Session, p: &Value) -> Result<Value> {
    let rx = checked_angle(p, "rx")?;
    let ry = checked_angle(p, "ry")?;
    let rz = checked_angle(p, "rz")?;
    let tz = checked_z(p, "z")?;
    let fov = checked_fov(p)?;
    let vanishing = parse_vanishing(p)?;
    let target_el = opt_u64(p, "element")?.map(ElementId);

    if rx.is_none() && ry.is_none() && rz.is_none() && tz.is_none() && fov.is_none() && vanishing.is_none() {
        return Err(EngineError::Param(
            "pass at least one of 'rx', 'ry', 'rz', 'z', 'fov' or 'vanishing'".into(),
        ));
    }

    let apply_one = |el: &mut anicraft_doc::Element| -> Option<Transform3D> {
        if matches!(el.kind, ElementKind::Camera) {
            return None;
        }
        let mut cur = el.transform_3d.unwrap_or_default().sanitized();
        if let Some(v) = rx {
            cur.rx_deg = v;
        }
        if let Some(v) = ry {
            cur.ry_deg = v;
        }
        if let Some(v) = rz {
            cur.rz_deg = v;
        }
        if let Some(v) = tz {
            cur.tz = v;
        }
        if let Some(v) = fov {
            cur.fov_deg = v;
        }
        if let Some(v) = vanishing {
            cur.vanishing = v;
        }
        let clean = cur.sanitized();
        el.transform_3d = (!clean.is_identity()).then_some(clean);
        Some(clean)
    };

    let mut updated = 0usize;
    let mut last = Transform3D { fov_deg: DEFAULT_FOV_DEG, ..Transform3D::default() };

    if let Some(eid) = target_el {
        let kf = s.editable_keyframe()?;
        let mut found = false;
        for el in kf.elements_mut_where(|id| id == eid) {
            found = true;
            if let Some(applied) = apply_one(el) {
                updated += 1;
                last = applied;
            }
        }
        if !found {
            return Err(EngineError::State(format!("no element {} on the active keyframe", eid.0)));
        }
        if updated == 0 {
            return Err(EngineError::State("camera elements do not take 3D postcard transforms".into()));
        }
    } else {
        if s.selection.is_empty() {
            return Err(EngineError::State("nothing is selected".into()));
        }
        s.each_layer(false, |s| {
            let els = s.selected_elements_mut()?;
            for el in els {
                if let Some(applied) = apply_one(el) {
                    updated += 1;
                    last = applied;
                }
            }
            Ok(Value::Null)
        })?;
        if updated == 0 {
            return Err(EngineError::State("no selected artwork can take a 3D transform".into()));
        }
    }

    Ok(json!({
        "updated": updated,
        "rx": last.rx_deg,
        "ry": last.ry_deg,
        "rz": last.rz_deg,
        "z": last.tz,
        "fov": last.fov_deg,
        "vanishing": last.vanishing.map(|v| [v.x, v.y]),
    }))
}

fn reset_3d_transform(s: &mut Session, p: &Value) -> Result<Value> {
    let target_el = opt_u64(p, "element")?.map(ElementId);
    let mut reset = 0usize;

    if let Some(eid) = target_el {
        let kf = s.editable_keyframe()?;
        let mut found = false;
        for el in kf.elements_mut_where(|id| id == eid) {
            found = true;
            if el.transform_3d.take().is_some() {
                reset += 1;
            }
        }
        if !found {
            return Err(EngineError::State(format!("no element {} on the active keyframe", eid.0)));
        }
    } else {
        if s.selection.is_empty() {
            return Err(EngineError::State("nothing is selected".into()));
        }
        s.each_layer(false, |s| {
            let els = s.selected_elements_mut()?;
            for el in els {
                if el.transform_3d.take().is_some() {
                    reset += 1;
                }
            }
            Ok(Value::Null)
        })?;
    }

    Ok(json!({ "reset": reset }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_doc::{Bitmap, BitmapId, ElementKind, Paint, Shape};
    use anicraft_geom::{Affine, FillRule, Path};
    use std::sync::Arc;

    #[test]
    fn set_and_reset_3d_transform_projects_vectors_and_roundtrips_json() {
        let mut s = Session::default();
        let clean_json = serde_json::to_string(&s.doc).expect("serialize clean");
        assert!(!clean_json.contains("transform_3d"), "absent when None");

        let v = s
            .execute("draw.rect", &json!({"x": 200, "y": 150, "w": 240, "h": 160, "fill": "#e8553b", "object": true}))
            .expect("draw rect");
        let eid = v["element"].as_u64().expect("element id");

        let before_dl = anicraft_doc::evaluate(&s.doc, 0);
        let before_bounds = before_dl.items[0].path.bounds().transformed(&before_dl.items[0].transform);

        let r = s
            .execute("modify.set_3d_transform", &json!({"rx": 15.0, "ry": -45.0, "rz": 0.0, "z": 240.0, "fov": 60.0, "vanishing": [640.0, 360.0]}))
            .expect("set 3d transform");
        assert_eq!(r["updated"], 1);
        assert_eq!(r["fov"], 60.0);

        let with_3d_json = serde_json::to_string(&s.doc).expect("serialize 3d");
        assert!(with_3d_json.contains("transform_3d"));
        let loaded: anicraft_doc::Document = serde_json::from_str(&with_3d_json).expect("deserialize 3d");
        assert_eq!(loaded, s.doc);

        let after_dl = anicraft_doc::evaluate(&s.doc, 0);
        let after_bounds = after_dl.items[0].path.bounds().transformed(&after_dl.items[0].transform);
        // Positive Z depth pushes the postcard away from the camera, foreshortening its width & height.
        assert!(after_bounds.width() < before_bounds.width(), "{after_bounds:?} vs {before_bounds:?}");
        assert!(after_bounds.height() < before_bounds.height());

        // Reset clears transform_3d back to None (omitted in JSON).
        let res = s.execute("modify.reset_3d_transform", &json!({"element": eid})).expect("reset");
        assert_eq!(res["reset"], 1);
        let reset_json = serde_json::to_string(&s.doc).expect("serialize after reset");
        assert!(!reset_json.contains("transform_3d"));
    }

    #[test]
    fn bitmap_3d_perspective_generates_12x12_warp_mesh_and_renders() {
        let mut s = Session::default();
        let bid = BitmapId(s.doc.alloc_id());
        let mut rgba = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16u32 {
            for x in 0..16u32 {
                let c = if (x / 4 + y / 4) % 2 == 0 { [240, 80, 40, 255] } else { [40, 140, 240, 255] };
                rgba.extend_from_slice(&c);
            }
        }
        let bmp = Bitmap::from_rgba(bid, "checker", 16, 16, rgba).expect("bitmap");
        s.doc.bitmaps.push(Arc::new(bmp));

        let eid = ElementId(s.doc.alloc_id());
        let shape = Shape {
            path: Arc::new(Path::rect(0.0, 0.0, 200.0, 160.0)),
            fill: Some(Paint::Bitmap {
                bitmap: bid,
                repeat: false,
                matrix: Affine::scale(200.0 / 16.0, 160.0 / 16.0),
            }),
            stroke: None,
            fill_rule: FillRule::NonZero,
            merge: false,
            warp: None,
        fill_tag: None,
        stroke_tag: None,
        };
        s.editable_keyframe().expect("kf").elements.push(Arc::new(anicraft_doc::Element {
            id: eid,
            name: String::new(),
            transform: Affine::translate(300.0, 200.0),
            color: anicraft_doc::ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::Shape(shape),
        }));
        s.select_only([eid]);

        s.execute("modify.set_3d_transform", &json!({"ry": 48.0, "rx": -18.0, "z": 80.0}))
            .expect("apply 3d to bitmap");

        let dl = anicraft_doc::evaluate(&s.doc, 0);
        assert_eq!(dl.items.len(), 1);
        let mesh = dl.items[0].warp.as_ref().expect("12x12 perspective warp mesh generated");
        // 12x12 cells => 13x13 = 169 vertices and 12*12*2 = 288 triangles.
        assert_eq!(mesh.rest.len(), 169);
        assert_eq!(mesh.shown.len(), 169);
        assert_eq!(mesh.tris.len(), 288);

        let (pm, _) = anicraft_render::render(&dl, &anicraft_render::View::stage(&s.doc, 0.5)).expect("render");
        assert!(pm.data.iter().any(|&px| px != pm.data[0]), "foreshortened bitmap rendered non-blank pixels");
    }

    #[test]
    fn classic_and_motion_tweens_interpolate_3d_rotation_and_depth() {
        // Classic tween interpolation of Transform3D.
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 300, "y": 200, "w": 200, "h": 140, "fill": "#3d8bfd", "object": true}))
            .expect("rect");
        s.execute("modify.convert_to_symbol", &json!({"name": "Card"})).expect("symbol");
        s.execute("timeline.insert_keyframe", &json!({"frame": 10})).expect("kf 10");
        s.execute("timeline.goto", &json!({"frame": 10})).expect("goto 10");
        s.execute("modify.set_3d_transform", &json!({"ry": 60.0, "z": 200.0})).expect("set end 3d");
        s.execute("timeline.goto", &json!({"frame": 0})).expect("goto 0");
        s.execute("timeline.create_tween", &json!({"frame": 0})).expect("classic tween");

        let dl_mid = anicraft_doc::evaluate(&s.doc, 5);
        let dl_start = anicraft_doc::evaluate(&s.doc, 0);
        let dl_end = anicraft_doc::evaluate(&s.doc, 10);
        let w_start = dl_start.items[0].path.bounds().transformed(&dl_start.items[0].transform).width();
        let w_mid = dl_mid.items[0].path.bounds().transformed(&dl_mid.items[0].transform).width();
        let w_end = dl_end.items[0].path.bounds().transformed(&dl_end.items[0].transform).width();
        assert!(w_mid < w_start && w_end < w_mid, "classic tween foreshortens smoothly: {w_start} -> {w_mid} -> {w_end}");

        // Motion tween Property::RotationX, RotationY, Z tracks.
        let mut sm = Session::default();
        sm.execute("draw.rect", &json!({"x": 300, "y": 200, "w": 200, "h": 140, "fill": "#ff9f43", "object": true}))
            .expect("rect");
        sm.execute("modify.convert_to_symbol", &json!({"name": "SpinCard"})).expect("symbol");
        sm.execute("timeline.insert_frame", &json!({"frame": 10})).expect("span");
        sm.execute("timeline.create_motion_tween", &json!({"frame": 0})).expect("motion tween");
        sm.execute("motion.set_key", &json!({"property": "rotation_y", "frame": 10, "value": 50.0}))
            .expect("key ry");
        sm.execute("motion.set_key", &json!({"property": "z", "frame": 10, "value": 160.0}))
            .expect("key z");
        let m_start = anicraft_doc::evaluate(&sm.doc, 0).items[0].path.bounds().width();
        let m_mid = anicraft_doc::evaluate(&sm.doc, 5).items[0].path.bounds().width();
        assert!(m_mid < m_start, "motion tween 3D tracks foreshorten mid-span: {m_start} -> {m_mid}");
    }
}
