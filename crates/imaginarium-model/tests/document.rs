// SPDX-License-Identifier: AGPL-3.0-or-later

//! Behaviour of documents: editing, syncing, undo, rollback, schema checks.

// Test helpers panic on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use imaginarium_model::{
    Actor, ChangeOrigin, Document, DocumentKind, Geometry, Migration, Mode, ModelError, NodeId,
    NodeKind, OpenOptions, Parent, ReadOnlyReason, UndoConfig, UndoScope, Value,
};

const NO_MERGE: UndoConfig = UndoConfig {
    merge_interval_ms: 0,
    max_steps: 100,
};

fn board() -> Document {
    Document::new_with(DocumentKind::Board, NO_MERGE).unwrap()
}

fn open(bytes: &[u8]) -> Document {
    Document::open_with(
        &[bytes],
        &OpenOptions {
            undo: NO_MERGE,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}

/// The incremental view must always equal a rebuild.
fn assert_consistent(doc: &Document) {
    assert_eq!(
        doc.view(),
        &doc.rebuild_view(),
        "incremental view diverged from the CRDT state"
    );
}

fn shape(doc: &mut Document, parent: Parent, x: f64) -> NodeId {
    doc.transact(&Actor::User, |t| {
        let id = t.create_node(parent, None, &NodeKind::Shape)?;
        t.set_geometry(id, Geometry::new(x, 0.0, 10.0, 10.0))?;
        t.set_prop(id, "shape", "rect".into())?;
        Ok(id)
    })
    .unwrap()
    .0
}

#[test]
fn new_documents_have_the_schema_skeleton() {
    let doc = Document::new(DocumentKind::Deck).unwrap();
    let meta = doc.view().meta();
    assert_eq!(meta.schema_version(), Some(1));
    assert_eq!(meta.kind(), DocumentKind::Deck);
    assert!(meta.created().is_some());
    assert!(
        !doc.can_undo(UndoScope::User),
        "the skeleton isn't an undo step"
    );
    assert_consistent(&doc);
}

#[test]
fn edits_update_the_view_and_report_changes() {
    let mut doc = board();
    let frame = doc
        .transact(&Actor::User, |t| {
            t.create_node(Parent::Root, None, &NodeKind::Frame)
        })
        .unwrap()
        .0;
    let a = shape(&mut doc, Parent::Node(frame), 1.0);
    let b = shape(&mut doc, Parent::Node(frame), 2.0);
    assert_eq!(
        doc.view().children(Parent::Node(frame)).collect::<Vec<_>>(),
        vec![a, b]
    );

    let (_, changes) = doc
        .transact(&Actor::User, |t| {
            t.set_geometry(a, Geometry::new(5.0, 6.0, 10.0, 10.0))?;
            t.move_node(b, Parent::Node(frame), Some(0))?;
            t.set_prop(a, "name", "Box A".into())
        })
        .unwrap();
    assert_eq!(changes.origin, ChangeOrigin::Local(Actor::User));
    assert_eq!(changes.moved, vec![b]);
    let a_changes = changes.node(a).unwrap();
    assert!(a_changes.geometry && a_changes.props && !a_changes.text);
    let node = doc.view().node(a).unwrap();
    assert_eq!(node.geometry(), Some(Geometry::new(5.0, 6.0, 10.0, 10.0)));
    assert_eq!(node.name(), Some("Box A"));
    assert_eq!(
        doc.view().children(Parent::Node(frame)).collect::<Vec<_>>(),
        vec![b, a]
    );

    let (_, changes) = doc
        .transact(&Actor::User, |t| t.delete_node(frame))
        .unwrap();
    let mut deleted = changes.deleted.clone();
    deleted.sort();
    let mut expected = vec![frame, a, b];
    expected.sort();
    assert_eq!(deleted, expected, "the whole subtree is reported");
    assert_eq!(doc.view().node_count(), 0);
    assert_consistent(&doc);
}

#[test]
fn text_has_paragraphs_and_marks() {
    let mut doc = Document::new_with(DocumentKind::Document, NO_MERGE).unwrap();
    let id = doc
        .transact(&Actor::User, |t| {
            let id = t.create_node(Parent::Root, None, &NodeKind::Text)?;
            t.insert_text(id, 0, "Title\nBody text")?;
            t.set_paragraph_mark(id, 0, "h", Value::Int(1))?;
            t.mark_text(id, 6..10, "b", Value::Bool(true))?;
            Ok(id)
        })
        .unwrap()
        .0;
    let text = doc.view().node(id).unwrap().text().unwrap().clone();
    assert_eq!(
        text.to_plain_string(),
        "Title\nBody text\n",
        "the final newline is added"
    );
    let paragraphs: Vec<_> = text.paragraphs().collect();
    assert_eq!(paragraphs[0].mark("h"), Some(&Value::Int(1)));
    assert_eq!(paragraphs[1].runs()[0].text(), "Body");
    assert_eq!(
        paragraphs[1].runs()[0].marks().get("b"),
        Some(&Value::Bool(true))
    );

    // Typing at the end of the heading keeps it a heading.
    doc.transact(&Actor::User, |t| t.insert_text(id, 5, "!"))
        .unwrap();
    let text = doc.view().node(id).unwrap().text().unwrap().clone();
    assert_eq!(text.paragraphs().next().unwrap().text(), "Title!");
    assert_eq!(
        text.paragraphs().next().unwrap().mark("h"),
        Some(&Value::Int(1))
    );

    let err = doc
        .transact(&Actor::User, |t| t.mark_text(id, 0..2, "h", Value::Int(2)))
        .unwrap_err();
    assert!(
        matches!(err, ModelError::Invalid(_)),
        "paragraph marks go through set_paragraph_mark"
    );
    assert!(
        doc.transact(&Actor::User, |t| t.mark_text(
            id,
            0..2,
            "zz",
            Value::Bool(true)
        ))
        .is_err()
    );
    doc.transact(&Actor::User, |t| {
        t.mark_text(id, 0..2, "org.example.notes:flag", Value::Bool(true))
    })
    .unwrap();
    assert_consistent(&doc);
}

#[test]
fn invalid_edits_change_nothing() {
    let mut doc = board();
    let a = shape(&mut doc, Parent::Root, 1.0);
    let child = shape(&mut doc, Parent::Node(a), 2.0);
    let before = doc.view().clone();
    let can_undo_before = doc.can_undo(UndoScope::User);

    assert_eq!(
        doc.transact(&Actor::User, |t| t.move_node(a, Parent::Node(child), None))
            .unwrap_err(),
        ModelError::Cycle(a)
    );
    assert!(
        doc.transact(&Actor::User, |t| t.create_node(
            Parent::Root,
            Some(99),
            &NodeKind::Shape
        ))
        .is_err()
    );
    assert!(
        doc.transact(&Actor::User, |t| t.set_prop(a, "g", 1.0.into()))
            .is_err()
    );
    // A command that edits and then fails: its edits are rolled back.
    let failed = doc.transact(&Actor::User, |t| {
        t.set_geometry(a, Geometry::new(99.0, 99.0, 1.0, 1.0))?;
        t.create_node(Parent::Root, None, &NodeKind::Text)?;
        Err::<(), _>(ModelError::Invalid("the command gave up".into()))
    });
    assert!(failed.is_err());
    assert_eq!(doc.view(), &before);
    assert_consistent(&doc);
    assert_eq!(doc.can_undo(UndoScope::User), can_undo_before);
    // The next undo undoes the last real edit, not the rolled-back one.
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(doc.view().node(child).is_none());
    assert!(doc.view().node(a).is_some());
}

#[test]
fn peers_converge_and_remote_changes_are_reported() {
    let mut a = board();
    let mut b = open(&a.export_snapshot().unwrap());
    let (va, vb) = (a.version(), b.version());
    let x = shape(&mut a, Parent::Root, 1.0);
    let y = shape(&mut b, Parent::Root, 2.0);
    let from_a = a.export_updates_since(&va).unwrap();
    let from_b = b.export_updates_since(&vb).unwrap();
    let changes = b.apply_remote(&[&from_a]).unwrap();
    assert_eq!(changes.origin, ChangeOrigin::Remote);
    assert_eq!(changes.created, vec![x]);
    a.apply_remote(&[&from_b]).unwrap();
    assert_eq!(a.view(), b.view());
    assert_eq!(a.view().node_count(), 2);
    assert!(a.view().node(y).is_some());
    assert_consistent(&a);
    assert_consistent(&b);
}

#[test]
fn concurrent_moves_keep_a_tree() {
    let mut a = board();
    let x = shape(&mut a, Parent::Root, 1.0);
    let y = shape(&mut a, Parent::Root, 2.0);
    let mut b = open(&a.export_snapshot().unwrap());
    let (va, vb) = (a.version(), b.version());
    a.transact(&Actor::User, |t| t.move_node(x, Parent::Node(y), None))
        .unwrap();
    b.transact(&Actor::User, |t| t.move_node(y, Parent::Node(x), None))
        .unwrap();
    let (ua, ub) = (
        a.export_updates_since(&va).unwrap(),
        b.export_updates_since(&vb).unwrap(),
    );
    a.apply_remote(&[&ub]).unwrap();
    b.apply_remote(&[&ua]).unwrap();
    assert_eq!(a.view(), b.view());
    assert_eq!(
        a.view().walk().len(),
        2,
        "both nodes are still reachable from the root"
    );
    assert_consistent(&a);
    assert_consistent(&b);
}

#[test]
fn concurrent_first_edits_of_a_part_merge() {
    let mut a = board();
    let id = a
        .transact(&Actor::User, |t| {
            t.create_node(Parent::Root, None, &NodeKind::Shape)
        })
        .unwrap()
        .0;
    let mut b = open(&a.export_snapshot().unwrap());
    let (va, vb) = (a.version(), b.version());
    // Neither peer has an `s` yet; both create it concurrently.
    a.transact(&Actor::User, |t| {
        t.set_style_override(id, "fill", "#ff0000".into())
    })
    .unwrap();
    b.transact(&Actor::User, |t| {
        t.set_style_override(id, "stroke", "#0000ff".into())
    })
    .unwrap();
    let (ua, ub) = (
        a.export_updates_since(&va).unwrap(),
        b.export_updates_since(&vb).unwrap(),
    );
    a.apply_remote(&[&ub]).unwrap();
    b.apply_remote(&[&ua]).unwrap();
    assert_eq!(a.view(), b.view());
    let overrides = a.view().node(id).unwrap().style_overrides().clone();
    assert_eq!(overrides.get("fill"), Some(&Value::from("#ff0000")));
    assert_eq!(
        overrides.get("stroke"),
        Some(&Value::from("#0000ff")),
        "both edits survive in the shared container"
    );
    assert_consistent(&a);
}

#[test]
fn undo_is_per_actor() {
    let mut doc = board();
    let mine = shape(&mut doc, Parent::Root, 1.0);
    let ai = doc
        .transact(&Actor::Ai("s1".into()), |t| {
            t.create_node(Parent::Root, None, &NodeKind::Text)
        })
        .unwrap()
        .0;
    let changes = doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(changes.origin, ChangeOrigin::Undo(UndoScope::User));
    assert_eq!(changes.deleted, vec![mine]);
    assert!(doc.view().node(ai).is_some(), "the assistant's edit stays");
    assert!(
        doc.undo(UndoScope::User, &Actor::User).unwrap().is_none(),
        "nothing left to undo"
    );
    doc.undo(UndoScope::Assistant, &Actor::User)
        .unwrap()
        .unwrap();
    assert_eq!(doc.view().node_count(), 0);
    doc.redo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(doc.view().node(mine).is_some());
    assert!(doc.view().node(ai).is_none());
    assert_consistent(&doc);
}

#[test]
fn remote_edits_are_not_undone() {
    let mut a = board();
    let mut b = open(&a.export_snapshot().unwrap());
    let mine = shape(&mut a, Parent::Root, 1.0);
    let vb = b.version();
    let theirs = shape(&mut b, Parent::Root, 2.0);
    a.apply_remote(&[&b.export_updates_since(&vb).unwrap()])
        .unwrap();
    a.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(a.view().node(mine).is_none());
    assert!(a.view().node(theirs).is_some());
}

#[test]
fn undo_groups_make_one_step() {
    let mut doc = board();
    let id = shape(&mut doc, Parent::Root, 0.0);
    doc.begin_undo_group(UndoScope::User).unwrap();
    for x in 1..=5 {
        doc.transact(&Actor::User, |t| {
            t.set_geometry(id, Geometry::new(f64::from(x), 0.0, 10.0, 10.0))
        })
        .unwrap();
    }
    doc.end_undo_group(UndoScope::User);
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(
        doc.view().node(id).unwrap().geometry().unwrap().x,
        0.0,
        "the whole drag undoes at once"
    );
}

#[test]
fn styles_resolve_through_parents_and_overrides() {
    let mut doc = board();
    let id = shape(&mut doc, Parent::Root, 0.0);
    doc.transact(&Actor::User, |t| {
        let base = t.create_style("Base", None)?;
        t.set_style_prop(&base, "fill", "#ff0000".into())?;
        t.set_style_prop(&base, "stroke", "#000000".into())?;
        let accent = t.create_style("Accent", Some(&base))?;
        t.set_style_prop(&accent, "fill", "#00ff00".into())?;
        t.set_style(id, Some(&accent))?;
        t.set_style_override(id, "stroke", "#0000ff".into())
    })
    .unwrap();
    let view = doc.view();
    assert_eq!(view.style_prop(id, "fill"), Some(&Value::from("#00ff00")));
    assert_eq!(view.style_prop(id, "stroke"), Some(&Value::from("#0000ff")));
    assert_eq!(view.style_prop(id, "opacity"), None);
    assert_consistent(&doc);
}

#[test]
fn newer_schemas_and_unknown_features_open_read_only() {
    let mut doc = board();
    doc.transact(&Actor::User, |t| t.require_feature("linked-frames"))
        .unwrap();
    let opened = open(&doc.export_snapshot().unwrap());
    assert_eq!(
        opened.mode(),
        &Mode::ReadOnly(ReadOnlyReason::MissingFeatures(vec![
            "linked-frames".into()
        ]))
    );
    let mut opened = opened;
    assert!(matches!(
        opened.transact(&Actor::User, |t| t.set_title("x")),
        Err(ModelError::ReadOnly(_))
    ));
    let known = Document::open_with(
        &[&doc.export_snapshot().unwrap()],
        &OpenOptions {
            known_features: &["linked-frames"],
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(known.mode(), &Mode::Editable);
}

/// A migration from a pretend version 0 that renamed `shape` to `preset`.
fn rename_preset(t: &mut imaginarium_model::Transaction<'_>) -> imaginarium_model::Result<()> {
    for id in t.children(Parent::Root) {
        t.set_prop(id, "shape", "rect".into())?;
        t.set_prop(id, "preset", Value::Null)?;
    }
    Ok(())
}

#[test]
fn old_schemas_are_migrated_on_open() {
    let mut old = board();
    let id = old
        .transact(&Actor::User, |t| {
            let id = t.create_node(Parent::Root, None, &NodeKind::Shape)?;
            t.set_prop(id, "preset", "rect".into())?;
            Ok(id)
        })
        .unwrap()
        .0;
    let bytes = downgrade_to_version_0(&mut old);
    let migrations = [Migration {
        from: 0,
        apply: rename_preset,
    }];
    let doc = Document::open_with(
        &[&bytes],
        &OpenOptions {
            migrations: &migrations,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(doc.mode(), &Mode::Editable);
    assert_eq!(doc.view().meta().schema_version(), Some(1));
    assert_eq!(
        doc.view().node(id).unwrap().prop("shape"),
        Some(&Value::from("rect"))
    );
    assert_eq!(doc.view().node(id).unwrap().prop("preset"), None);
    assert!(
        !doc.can_undo(UndoScope::User),
        "migrations aren't undo steps"
    );

    let without = open(&bytes);
    assert_eq!(
        without.mode(),
        &Mode::ReadOnly(ReadOnlyReason::NoMigration(0))
    );
}

fn downgrade_to_version_0(doc: &mut Document) -> Vec<u8> {
    doc.set_schema_version_for_tests(0).unwrap();
    doc.export_snapshot().unwrap()
}

#[test]
fn shallow_snapshots_drop_history_but_keep_the_state() {
    let mut doc = board();
    let id = shape(&mut doc, Parent::Root, 0.0);
    for x in 0..200 {
        doc.transact(&Actor::User, |t| {
            t.set_geometry(id, Geometry::new(f64::from(x), 0.0, 10.0, 10.0))
        })
        .unwrap();
    }
    let full = doc.export_snapshot().unwrap();
    let shallow = doc.export_shallow().unwrap();
    assert!(shallow.len() < full.len());
    assert_eq!(open(&shallow).view(), doc.view());
}

#[test]
fn errors_name_the_problem() {
    let mut doc = board();
    let ghost = NodeId::parse("999@1").unwrap();
    assert_eq!(
        doc.transact(&Actor::User, |t| t.set_geometry(ghost, Geometry::default()))
            .unwrap_err(),
        ModelError::NoSuchNode(ghost)
    );
    assert!(Document::open(&[b"not a document"]).is_err());
}

#[test]
fn undo_and_redo_keep_node_identity_and_references() {
    let mut doc = board();
    let (shape_id, connector) = doc
        .transact(&Actor::User, |t| {
            let shape_id = t.create_node(Parent::Root, None, &NodeKind::Text)?;
            t.set_geometry(shape_id, Geometry::new(1.0, 2.0, 30.0, 40.0))?;
            t.insert_text(shape_id, 0, "label")?;
            let connector = t.create_node(Parent::Root, None, &NodeKind::Connector)?;
            t.set_prop(connector, "from", shape_id.to_string().into())?;
            Ok((shape_id, connector))
        })
        .unwrap()
        .0;
    let original = doc.view().node(shape_id).unwrap().clone();

    doc.transact(&Actor::User, |t| t.delete_node(shape_id))
        .unwrap();
    assert!(doc.view().node(shape_id).is_none());
    let changes = doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(
        changes.created,
        vec![shape_id],
        "it comes back under the same id"
    );
    let restored = doc.view().node(shape_id).unwrap();
    assert_eq!(restored.geometry(), original.geometry());
    assert_eq!(restored.text().unwrap().to_plain_string(), "label\n");
    let from = doc
        .view()
        .node(connector)
        .unwrap()
        .prop("from")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();
    assert_eq!(
        NodeId::parse(&from),
        Some(shape_id),
        "the connector still points at it"
    );
    assert_consistent(&doc);

    // Undo the creation as well, then redo both: the ids stay.
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(doc.view().node_count(), 0);
    doc.redo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(doc.view().node(shape_id).is_some() && doc.view().node(connector).is_some());
    doc.redo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(doc.view().node(shape_id).is_none(), "the delete is redone");
    assert_consistent(&doc);

    // A peer that receives the re-created node sees the same identity.
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    let peer = open(&doc.export_snapshot().unwrap());
    assert_eq!(peer.view(), doc.view());
}

#[test]
fn views_can_go_to_other_threads_and_documents_can_move() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<imaginarium_model::DocumentView>();
    send_sync::<imaginarium_model::ChangeSet>();
    send::<Document>();
    let doc = board();
    let view = doc.view().clone();
    let count = std::thread::spawn(move || view.node_count()).join().unwrap();
    assert_eq!(count, 0);
}
