// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sample boards, for the examples, tests and benchmarks.

use imaginarium_model::{
    Actor, Document, DocumentKind, Geometry, NodeId, NodeKind, Parent, Result, Transaction, Value,
};

/// The board of the mockups ("Launch plan · Q4"): a frame with three
/// shapes and lines, a diamond, a triangle, text boxes. Coordinates are the
/// mockup's, so a window of 1440 × 866 shows it as the mockup does.
pub fn launch_plan() -> Result<Document> {
    let mut document = Document::new(DocumentKind::Board)?;
    document.transact(&Actor::User, |txn| {
        let title = text(
            txn,
            Parent::Root,
            (240.0, 84.0, 600.0, 40.0),
            "Launch plan · Q4",
        )?;
        txn.set_style_override(title, "size", Value::Number(30.0))?;
        txn.mark_text(title, 0..16, "b", Value::Bool(true))?;
        let subtitle = text(
            txn,
            Parent::Root,
            (240.0, 128.0, 400.0, 24.0),
            "Draft for Monday's review",
        )?;
        txn.set_style_override(subtitle, "size", Value::Number(15.0))?;
        txn.set_style_override(subtitle, "color", color("#6B6B76"))?;

        let frame = txn.create_node(Parent::Root, None, &NodeKind::Frame)?;
        txn.set_geometry(frame, Geometry::new(240.0, 200.0, 540.0, 450.0))?;
        txn.set_prop(frame, "name", Value::String("Research".into()))?;
        let inside = Parent::Node(frame);
        let interview = shape(
            txn,
            inside,
            "rect",
            (36.0, 90.0, 220.0, 88.0),
            "#E0E7FF",
            "Interview 5 users",
        )?;
        txn.set_style_override(interview, "radius", Value::Number(12.0))?;
        let scan = shape(
            txn,
            inside,
            "rect",
            (288.0, 90.0, 216.0, 88.0),
            "#FEF3C7",
            "Competitor scan",
        )?;
        txn.set_style_override(scan, "radius", Value::Number(12.0))?;
        line(txn, inside, (146.0, 178.0, 92.0, 103.0), None)?;
        line(txn, inside, (302.0, 178.0, 94.0, 103.0), Some("h"))?;
        shape(
            txn,
            inside,
            "ellipse",
            (170.0, 280.0, 200.0, 112.0),
            "#DCFCE7",
            "Synthesis",
        )?;

        line(txn, Parent::Root, (610.0, 518.0, 270.0, 18.0), Some("v"))?;
        line(txn, Parent::Root, (1059.0, 518.0, 71.0, 0.0), None)?;
        shape(
            txn,
            Parent::Root,
            "diamond",
            (880.0, 450.0, 180.0, 136.0),
            "#FCE7F3",
            "Go / no-go",
        )?;
        let yes = text(txn, Parent::Root, (1082.0, 494.0, 40.0, 18.0), "yes")?;
        txn.set_style_override(yes, "size", Value::Number(13.0))?;
        txn.set_style_override(yes, "color", color("#6B6B76"))?;
        let beta = shape(
            txn,
            Parent::Root,
            "rect",
            (1130.0, 474.0, 200.0, 88.0),
            "#E0F2FE",
            "Beta for 50 teams",
        )?;
        txn.set_style_override(beta, "radius", Value::Number(12.0))?;
        shape(
            txn,
            Parent::Root,
            "triangle",
            (920.0, 240.0, 140.0, 120.0),
            "#FFE4E6",
            "Risks",
        )?;
        let note = text(
            txn,
            Parent::Root,
            (1110.0, 264.0, 250.0, 72.0),
            "Ship the beta to 50 teams by December. Two interviews left this week.",
        )?;
        txn.set_style_override(note, "color", color("#334155"))?;
        Ok(())
    })?;
    Ok(document)
}

/// A board of `count` shapes in a grid, of every preset, for benchmarks.
pub fn grid(count: usize) -> Result<Document> {
    let mut document = Document::new(DocumentKind::Board)?;
    let presets = ["rect", "ellipse", "triangle", "diamond", "rect"];
    let fills = [
        "#E0E7FF", "#FEF3C7", "#DCFCE7", "#E0F2FE", "#FCE7F3", "#FFE4E6",
    ];
    let columns = (count as f64).sqrt().ceil().max(1.0) as usize;
    document.transact(&Actor::User, |txn| {
        for index in 0..count {
            let (column, row) = (index % columns, index / columns);
            let id = shape(
                txn,
                Parent::Root,
                presets[index % presets.len()],
                (column as f64 * 64.0, row as f64 * 64.0, 48.0, 48.0),
                fills[index % fills.len()],
                "",
            )?;
            if index % presets.len() == 4 {
                txn.set_style_override(id, "radius", Value::Number(8.0))?;
            }
        }
        Ok(())
    })?;
    Ok(document)
}

fn color(hex: &str) -> Value {
    Value::String(hex.into())
}

fn text(
    txn: &mut Transaction<'_>,
    parent: Parent,
    (x, y, w, h): (f64, f64, f64, f64),
    content: &str,
) -> Result<NodeId> {
    let id = txn.create_node(parent, None, &NodeKind::Text)?;
    txn.set_geometry(id, Geometry::new(x, y, w, h))?;
    txn.insert_text(id, 0, content)?;
    Ok(id)
}

fn shape(
    txn: &mut Transaction<'_>,
    parent: Parent,
    preset: &str,
    (x, y, w, h): (f64, f64, f64, f64),
    fill: &str,
    label: &str,
) -> Result<NodeId> {
    let id = txn.create_node(parent, None, &NodeKind::Shape)?;
    txn.set_geometry(id, Geometry::new(x, y, w, h))?;
    txn.set_prop(id, "shape", Value::String(preset.into()))?;
    txn.set_style_override(id, "fill", color(fill))?;
    if !label.is_empty() {
        txn.insert_text(id, 0, label)?;
    }
    Ok(id)
}

fn line(
    txn: &mut Transaction<'_>,
    parent: Parent,
    (x, y, w, h): (f64, f64, f64, f64),
    flip: Option<&str>,
) -> Result<NodeId> {
    let id = txn.create_node(parent, None, &NodeKind::Shape)?;
    txn.set_geometry(id, Geometry::new(x, y, w, h))?;
    txn.set_prop(id, "shape", Value::String("line".into()))?;
    if let Some(flip) = flip {
        txn.set_prop(id, "flip", Value::String(flip.into()))?;
    }
    txn.set_style_override(id, "stroke", color("#475569"))?;
    Ok(id)
}
