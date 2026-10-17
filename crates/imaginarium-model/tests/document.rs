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
    let count = std::thread::spawn(move || view.node_count())
        .join()
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_rolled_back_delete_keeps_the_undo_history() {
    // Reverting a delete used to re-create the node's containers under new
    // ids, which turned every undo step that touched them into a no-op.
    let mut doc = board();
    let x = shape(&mut doc, Parent::Root, 1.0);
    doc.transact(&Actor::User, |t| {
        t.set_geometry(x, Geometry::new(2.0, 0.0, 10.0, 10.0))
    })
    .unwrap();
    let missing = shape(&mut board(), Parent::Root, 0.0); // from another document
    let failed = doc.transact(&Actor::User, |t| {
        t.set_generation(x, &[("model", "m".into())])?;
        t.delete_node(x)?;
        t.delete_node(missing)
    });
    assert!(failed.is_err());
    assert!(doc.view().node(x).is_some());
    assert_consistent(&doc);
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(doc.view().node(x).unwrap().geometry().unwrap().x, 1.0);
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(doc.view().node(x).is_none());
}

#[test]
fn deleted_nodes_are_gone_for_the_rest_of_the_transaction() {
    let mut doc = board();
    let [a, b, c] = [1.0, 2.0, 3.0].map(|x| shape(&mut doc, Parent::Root, x));
    let child = shape(&mut doc, Parent::Node(b), 4.0);
    let (created, _) = doc
        .transact(&Actor::User, |t| {
            t.delete_node(b)?;
            assert!(!t.contains(b));
            assert!(!t.contains(child), "descendants go with it");
            assert_eq!(t.children(Parent::Root), vec![a, c]);
            assert!(t.set_prop(b, "name", "x".into()).is_err());
            assert!(t.move_node(child, Parent::Root, None).is_err());
            assert!(
                t.create_node(Parent::Node(b), None, &NodeKind::Shape)
                    .is_err()
            );
            // Indices count the remaining siblings only.
            let created = t.create_node(Parent::Root, Some(2), &NodeKind::Shape)?;
            t.move_node(a, Parent::Root, Some(1))?;
            assert_eq!(t.children(Parent::Root), vec![c, a, created]);
            Ok(created)
        })
        .unwrap();
    let root: Vec<NodeId> = doc.view().children(Parent::Root).collect();
    assert_eq!(root, vec![c, a, created]);
    assert!(doc.view().node(child).is_none());
    assert_consistent(&doc);
}

#[test]
fn an_undo_that_skips_a_stale_step_stays_on_its_own_stack() {
    // The user edited a node the assistant created, then the assistant's
    // step was undone. The user's next undo skips the stale edit (a no-op
    // now). That skip used to drop the undo's actor and origin, so the
    // assistant's stack recorded the user's undo as its own step.
    let mut doc = board();
    let mine = shape(&mut doc, Parent::Root, 1.0);
    let ai = Actor::Ai("s1".into());
    let theirs = doc
        .transact(&ai, |t| t.create_node(Parent::Root, None, &NodeKind::Shape))
        .unwrap()
        .0;
    doc.transact(&Actor::User, |t| {
        t.set_geometry(theirs, Geometry::new(5.0, 5.0, 1.0, 1.0))
    })
    .unwrap();
    doc.undo(UndoScope::Assistant, &Actor::User)
        .unwrap()
        .unwrap();
    assert!(doc.can_redo(UndoScope::Assistant));
    let changes = doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(changes.origin, ChangeOrigin::Undo(UndoScope::User));
    assert_eq!(changes.deleted, vec![mine]);
    assert_eq!(doc.last_local_actor_for_tests(), Some(Actor::User));
    assert!(
        doc.can_redo(UndoScope::Assistant),
        "the assistant's redo stack was cleared"
    );
    assert!(
        !doc.can_undo(UndoScope::Assistant),
        "the user's undo landed on the assistant's stack"
    );
    // Both stacks still work.
    doc.redo(UndoScope::Assistant, &Actor::User)
        .unwrap()
        .unwrap();
    assert!(doc.view().node(theirs).is_some());
    doc.redo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(doc.view().node(mine).is_some());
    assert_consistent(&doc);
}

#[test]
fn undoing_a_move_of_a_node_that_is_gone_leaves_no_empty_node() {
    // Loro revives a deleted node when a move of it is undone. If another
    // actor's undo removed the node (and its properties), what comes back
    // is an empty node without kind or id. Nothing should come back.
    for into_other in [false, true] {
        let mut doc = board();
        let mine = shape(&mut doc, Parent::Root, 1.0);
        let ai = Actor::Ai("s1".into());
        let theirs = doc
            .transact(&ai, |t| t.create_node(Parent::Root, None, &NodeKind::Shape))
            .unwrap()
            .0;
        let target = if into_other {
            Parent::Node(mine)
        } else {
            Parent::Root
        };
        doc.transact(&Actor::User, |t| t.move_node(theirs, target, Some(0)))
            .unwrap();
        doc.undo(UndoScope::Assistant, &Actor::User)
            .unwrap()
            .unwrap();
        let count = doc.view().node_count();
        let changes = doc.undo(UndoScope::User, &Actor::User).unwrap();
        assert!(
            changes.as_ref().is_none_or(|c| c.created.is_empty()),
            "{changes:?}"
        );
        assert!(doc.view().node_count() <= count);
        let mut stack: Vec<NodeId> = doc.view().children(Parent::Root).collect();
        while let Some(id) = stack.pop() {
            let node = doc.view().node(id).unwrap();
            assert!(
                !matches!(node.kind(), NodeKind::Unknown(k) if k.is_empty()),
                "an empty node came back: {id}"
            );
            stack.extend(doc.view().children(Parent::Node(id)));
        }
        assert_consistent(&doc);
    }
}

#[test]
fn removing_a_duplicated_node_inside_a_subtree_keeps_the_view_consistent() {
    // Two peers delete the same node and both undo: two live copies with
    // one stored id, one of them shadowed. When the winner goes away as
    // part of a deleted subtree, the other copy takes over the id.
    let mut a = board();
    let frame = shape(&mut a, Parent::Root, 0.0);
    let dup = shape(&mut a, Parent::Root, 1.0);
    let mut b = open(&a.export_snapshot().unwrap());
    let (va, vb) = (a.version(), b.version());
    for doc in [&mut a, &mut b] {
        doc.transact(&Actor::User, |t| t.delete_node(dup)).unwrap();
        doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    }
    let from_b = b.export_updates_since(&vb).unwrap();
    a.apply_remote(&[&from_b]).unwrap();
    let from_a = a.export_updates_since(&va).unwrap();
    b.apply_remote(&[&from_a]).unwrap();
    assert_consistent(&a);
    a.transact(&Actor::User, |t| {
        t.move_node(dup, Parent::Node(frame), None)
    })
    .unwrap();
    assert_consistent(&a);
    a.transact(&Actor::User, |t| t.delete_node(frame)).unwrap();
    assert_consistent(&a);
    assert!(
        a.view().node(dup).is_some(),
        "the other copy has the id now"
    );
}

#[test]
fn remote_changes_that_need_a_newer_reader_make_the_document_read_only() {
    // A newer peer bumps the schema, or starts using a feature this version
    // doesn't know: this reader must stop writing (schema-v1.md §10).
    let mut newer = board();
    let mut reader = open(&newer.export_snapshot().unwrap());
    let before = newer.version();
    newer
        .set_schema_version_for_tests(imaginarium_model::SCHEMA_VERSION + 1)
        .unwrap();
    reader
        .apply_remote(&[&newer.export_updates_since(&before).unwrap()])
        .unwrap();
    assert_eq!(
        reader.mode(),
        &Mode::ReadOnly(ReadOnlyReason::NewerSchema(
            imaginarium_model::SCHEMA_VERSION + 1
        ))
    );
    assert!(reader.transact(&Actor::User, |t| t.set_title("x")).is_err());

    let mut newer = board();
    let mut reader = open(&newer.export_snapshot().unwrap());
    let before = newer.version();
    newer
        .transact(&Actor::User, |t| t.require_feature("linked-frames"))
        .unwrap();
    reader
        .apply_remote(&[&newer.export_updates_since(&before).unwrap()])
        .unwrap();
    assert_eq!(
        reader.mode(),
        &Mode::ReadOnly(ReadOnlyReason::MissingFeatures(vec![
            "linked-frames".into()
        ]))
    );
}

#[test]
fn documents_that_open_read_only_are_not_migrated() {
    // An old schema plus a feature this version doesn't know: read-only,
    // so the migration must not write to it either.
    let mut old = board();
    old.transact(&Actor::User, |t| t.require_feature("linked-frames"))
        .unwrap();
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
    assert!(matches!(
        doc.mode(),
        Mode::ReadOnly(ReadOnlyReason::MissingFeatures(_))
    ));
    assert_eq!(doc.view().meta().schema_version(), Some(0), "not migrated");
}

/// A migration that gives up.
fn failing_migration(_: &mut imaginarium_model::Transaction<'_>) -> imaginarium_model::Result<()> {
    Err(ModelError::Invalid("can't migrate this".into()))
}

#[test]
fn a_failing_migration_opens_the_document_read_only() {
    let mut old = board();
    let bytes = downgrade_to_version_0(&mut old);
    let migrations = [Migration {
        from: 0,
        apply: failing_migration,
    }];
    let doc = Document::open_with(
        &[&bytes],
        &OpenOptions {
            migrations: &migrations,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert!(
        matches!(
            doc.mode(),
            Mode::ReadOnly(ReadOnlyReason::MigrationFailed { from: 0, .. })
        ),
        "{:?}",
        doc.mode()
    );
    assert_eq!(doc.view().meta().schema_version(), Some(0));
}

fn plain(doc: &Document, id: NodeId) -> String {
    doc.view()
        .node(id)
        .unwrap()
        .text()
        .map_or_else(String::new, |t| t.to_plain_string())
}

#[test]
fn local_edits_keep_the_final_newline() {
    // schema-v1.md §6: a non-empty text ends with `\n`.
    let mut doc = board();
    let id = doc
        .transact(&Actor::User, |t| {
            let id = t.create_node(Parent::Root, None, &NodeKind::Text)?;
            t.insert_text(id, 0, "hello")?;
            Ok(id)
        })
        .unwrap()
        .0;
    assert_eq!(plain(&doc, id), "hello\n");
    // After the final newline: a new paragraph, terminated too.
    doc.transact(&Actor::User, |t| t.insert_text(id, 6, "x"))
        .unwrap();
    assert_eq!(plain(&doc, id), "hello\nx\n");
    // A range up to the end keeps the final newline...
    doc.transact(&Actor::User, |t| t.delete_text(id, 3, 5))
        .unwrap();
    assert_eq!(plain(&doc, id), "hel\n");
    doc.transact(&Actor::User, |t| t.delete_text(id, 3, 1))
        .unwrap();
    assert_eq!(plain(&doc, id), "hel\n");
    // ...unless the whole text goes.
    doc.transact(&Actor::User, |t| t.delete_text(id, 0, 4))
        .unwrap();
    assert_eq!(plain(&doc, id), "");
    assert_consistent(&doc);
}

#[test]
fn a_failed_text_edit_writes_nothing() {
    // insert_text used to create the `t` container before checking `pos`,
    // so a command that ignored the error still committed it.
    let mut doc = board();
    let id = shape(&mut doc, Parent::Root, 0.0);
    let (_, changes) = doc
        .transact(&Actor::User, |t| {
            assert!(t.insert_text(id, 5, "x").is_err());
            Ok(())
        })
        .unwrap();
    assert!(changes.is_empty(), "{changes:?}");
    assert!(doc.view().node(id).unwrap().text().is_none());
}

#[test]
fn a_failed_transaction_inside_an_undo_group_splits_it_cleanly() {
    let mut doc = board();
    let id = shape(&mut doc, Parent::Root, 0.0);
    let x = |doc: &Document| doc.view().node(id).unwrap().geometry().unwrap().x;
    let drag = |doc: &mut Document, x: f64| {
        doc.transact(&Actor::User, |t| {
            t.set_geometry(id, Geometry::new(x, 0.0, 10.0, 10.0))
        })
        .unwrap();
    };
    doc.begin_undo_group(UndoScope::User).unwrap();
    drag(&mut doc, 1.0);
    drag(&mut doc, 2.0);
    let failed = doc.transact(&Actor::User, |t| {
        t.set_geometry(id, Geometry::new(99.0, 0.0, 10.0, 10.0))?;
        Err::<(), _>(ModelError::Invalid("gave up".into()))
    });
    assert!(failed.is_err());
    drag(&mut doc, 3.0);
    doc.end_undo_group(UndoScope::User);
    assert_eq!(x(&doc), 3.0);
    assert_consistent(&doc);
    // Loro needs the group closed while the failed edit is reverted: the
    // drag becomes two steps, before and after the failure.
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(x(&doc), 2.0);
    doc.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert_eq!(x(&doc), 0.0);
    assert_consistent(&doc);
}

#[test]
fn a_document_keeps_its_own_settings_and_peers_merge_them() {
    let mut a = board();
    let changes = a
        .transact(&Actor::User, |t| {
            t.set_document_setting("board.background", Value::String("grid".into()))
        })
        .unwrap()
        .1;
    assert!(changes.meta);
    assert_eq!(
        a.view().meta().settings().get("board.background"),
        Some(&Value::String("grid".into()))
    );
    // Saved and opened again.
    let mut b = open(&a.export_snapshot().unwrap());
    assert_eq!(a.view(), b.view());
    // Two peers set two settings at once: both stay (a mergeable map).
    let (va, vb) = (a.version(), b.version());
    a.transact(&Actor::User, |t| {
        t.set_document_setting("board.snap", Value::Bool(true))
    })
    .unwrap();
    b.transact(&Actor::User, |t| {
        t.set_document_setting("board.background", Value::Null)
    })
    .unwrap();
    let from_a = a.export_updates_since(&va).unwrap();
    let from_b = b.export_updates_since(&vb).unwrap();
    a.apply_remote(&[&from_b]).unwrap();
    let remote = b.apply_remote(&[&from_a]).unwrap();
    assert!(remote.meta);
    assert_eq!(a.view(), b.view());
    let settings = a.view().meta().settings().clone();
    assert_eq!(settings.get("board.snap"), Some(&Value::Bool(true)));
    assert_eq!(settings.get("board.background"), None);
    assert_consistent(&a);
}

#[test]
fn document_settings_take_setting_keys_and_plain_values() {
    let mut a = board();
    for key in [
        "background",
        "Board.background",
        "board..x",
        "",
        "_a.1",
        "board.9x",
    ] {
        assert!(
            a.transact(&Actor::User, |t| t
                .set_document_setting(key, Value::Bool(true)))
                .is_err(),
            "{key:?}"
        );
    }
    assert!(
        a.transact(&Actor::User, |t| {
            t.set_document_setting("board.x", Value::Binary(std::sync::Arc::from(&[1u8][..])))
        })
        .is_err()
    );
    let list = Value::List(std::sync::Arc::from(vec![Value::Bool(true)]));
    assert!(
        a.transact(&Actor::User, |t| t.set_document_setting("board.x", list))
            .is_err()
    );
    // Removing from a document without settings adds nothing to it.
    let before = a.to_value();
    a.transact(&Actor::User, |t| {
        t.set_document_setting("board.x", Value::Null)
    })
    .unwrap();
    assert_eq!(a.to_value(), before);
}

#[test]
fn undo_takes_a_document_setting_back() {
    let mut a = board();
    a.transact(&Actor::User, |t| {
        t.set_document_setting("board.background", Value::String("grid".into()))
    })
    .unwrap();
    a.transact(&Actor::User, |t| {
        t.set_document_setting("board.background", Value::String("plain".into()))
    })
    .unwrap();
    let undone = a.undo(UndoScope::User, &Actor::User).unwrap().unwrap();
    assert!(undone.meta);
    assert_eq!(
        a.view().meta().settings().get("board.background"),
        Some(&Value::String("grid".into()))
    );
    a.undo(UndoScope::User, &Actor::User).unwrap();
    assert_eq!(a.view().meta().settings().get("board.background"), None);
    assert_consistent(&a);
}
