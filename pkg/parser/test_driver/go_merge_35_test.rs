// Copyright 2026 AsterSQL.

use crate::{InPlaceVisitor, ParamMarkerExpr, ValueExpr};
use std::any::Any;

#[derive(Default)]
struct Trace {
    entered: Vec<&'static str>,
    left: Vec<&'static str>,
    skip: bool,
}

impl InPlaceVisitor for Trace {
    fn Enter(&mut self, node: &mut dyn Any) -> bool {
        self.entered.push(if node.is::<ValueExpr>() {
            "value"
        } else if node.is::<ParamMarkerExpr>() {
            "marker"
        } else {
            "unknown"
        });
        self.skip
    }

    fn Leave(&mut self, node: &mut dyn Any) -> bool {
        self.left.push(if node.is::<ValueExpr>() {
            "value"
        } else if node.is::<ParamMarkerExpr>() {
            "marker"
        } else {
            "unknown"
        });
        true
    }
}

#[test]
fn go_merge_35_leaf_accept_in_place_calls_enter_and_leave() {
    for skip in [false, true] {
        let mut trace = Trace {
            skip,
            ..Trace::default()
        };
        assert!(ValueExpr::default().AcceptInPlace(&mut trace));
        assert!(ParamMarkerExpr::default().AcceptInPlace(&mut trace));
        assert_eq!(trace.entered, ["value", "marker"]);
        assert_eq!(trace.left, ["value", "marker"]);
    }
}
