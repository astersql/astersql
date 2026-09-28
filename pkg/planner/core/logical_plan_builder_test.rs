// Copyright 2026 AsterSQL.

use crate::logical_plan_builder::{
    BoundKind, ByItem, FrameBound, WindowFrame, WindowSpec, compareItems, getAllByItems,
    mergeWindowSpec, specEqual, windowFuncs,
};
use crate::task::Expression;

fn window(name: &str) -> WindowSpec {
    WindowSpec {
        name: name.into(),
        reference: None,
        partition_by: Vec::new(),
        order_by: Vec::new(),
        frame: None,
    }
}

fn expr(name: &str, column: usize) -> Expression {
    Expression {
        name: name.into(),
        column: Some(column),
        ..Expression::default()
    }
}

#[test]
fn inherited_window_rejects_parent_frame_and_child_partition_like_go() {
    let mut framed_parent = window("parent");
    framed_parent.frame = Some(WindowFrame {
        rows: true,
        start: FrameBound {
            kind: BoundKind::UnboundedPreceding,
            value: None,
        },
        end: FrameBound {
            kind: BoundKind::CurrentRow,
            value: None,
        },
    });
    let mut child = window("child");
    assert!(mergeWindowSpec(&mut child, &framed_parent).is_err());

    let mut partitioned_child = window("child");
    partitioned_child.partition_by.push(expr("a", 0));
    assert!(mergeWindowSpec(&mut partitioned_child, &window("parent")).is_err());
}

#[test]
fn window_item_comparison_and_frames_match_go_restore_semantics() {
    let mut spec = window("w");
    spec.partition_by = vec![expr("b", 1), expr("b", 1)];
    spec.order_by = vec![ByItem {
        expr: expr("a", 0),
        desc: true,
    }];
    let items = getAllByItems(
        vec![ByItem {
            expr: expr("stale", 9),
            desc: false,
        }],
        &spec,
    );
    assert_eq!(items.len(), 3, "Go preserves duplicate window keys");
    assert_eq!(
        items
            .iter()
            .map(|item| item.expr.name.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "b", "a"],
        "Go resets the scratch buffer and preserves item order",
    );
    assert!(compareItems(
        &[ByItem {
            expr: expr("a", 0),
            desc: false,
        }],
        &[ByItem {
            expr: expr("b", 1),
            desc: false,
        }],
    ));

    let mut other = spec.clone();
    other.frame = Some(WindowFrame {
        rows: true,
        start: FrameBound {
            kind: BoundKind::Preceding,
            value: Some(1),
        },
        end: FrameBound {
            kind: BoundKind::CurrentRow,
            value: None,
        },
    });
    assert!(!specEqual(&spec, &other));
    let mut different_frame = other.clone();
    different_frame.frame.as_mut().unwrap().start.value = Some(2);
    assert!(!specEqual(&other, &different_frame));

    let _public_shape = windowFuncs {
        spec,
        funcs: Vec::new(),
    };
}
