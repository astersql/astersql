// Copyright 2026 AsterSQL.

use crate::{
    ExprKind, ExprNode, InPlaceVisitor, Node, SelectStmt, TableOptimizerHint, Visitor, Walk,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::any::Any;
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

#[global_allocator]
static TEST_ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS
            .try_with(|count| count.set(count.get() + 1))
            .ok();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS
            .try_with(|count| count.set(count.get() + 1))
            .ok();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS
            .try_with(|count| count.set(count.get() + 1))
            .ok();
        unsafe { System.alloc_zeroed(layout) }
    }
}

struct ReplaceOnLeave;

impl InPlaceVisitor for ReplaceOnLeave {
    fn enter(&mut self, _node: &mut dyn Node) -> bool {
        false
    }

    fn leave(&mut self, _node: &mut dyn Node) -> bool {
        true
    }

    fn leave_replacement(&mut self, node: &mut dyn Node) -> Option<Box<dyn Any>> {
        let expr = node.as_any().downcast_ref::<ExprNode>()?;
        matches!(&expr.Kind, ExprKind::Value(value) if value == "original")
            .then(|| Box::new(ExprNode::Value("replacement".into())) as Box<dyn Any>)
    }

    fn leave_embedded_replacement(&mut self, node: &mut dyn Any) -> Option<Box<dyn Any>> {
        let hint = node.downcast_ref::<TableOptimizerHint>()?;
        (hint.HintName.O == "original").then(|| {
            Box::new(TableOptimizerHint {
                HintName: crate::NewCIStr("replacement"),
                ..Default::default()
            }) as Box<dyn Any>
        })
    }
}

#[test]
fn go_merge_27_legacy_visitor_replaces_expression_child() {
    let mut root = ExprNode {
        Kind: ExprKind::Parentheses(Box::new(ExprNode::Value("original".into()))),
        ..Default::default()
    };
    assert!(Walk(&mut root, &mut ReplaceOnLeave));
    match root.Kind {
        ExprKind::Parentheses(child) => {
            assert_eq!(*child, ExprNode::Value("replacement".into()))
        }
        _ => panic!("parentheses changed kind"),
    }
}

#[test]
fn go_merge_27_legacy_visitor_replaces_table_hint_child() {
    let mut root = SelectStmt {
        TableHints: vec![TableOptimizerHint {
            HintName: crate::NewCIStr("original"),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(Walk(&mut root, &mut ReplaceOnLeave));
    assert_eq!(root.TableHints[0].HintName.O, "replacement");
}

#[test]
fn go_merge_27_depth_first_order_skip_and_stop() {
    struct Trace {
        events: Vec<String>,
        skip: Option<&'static str>,
        stop: Option<&'static str>,
    }
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            let Some(expr) = node.as_any().downcast_ref::<ExprNode>() else {
                return false;
            };
            let name = match &expr.Kind {
                ExprKind::Between { .. } => "root".to_owned(),
                ExprKind::Unary { .. } => "unary".to_owned(),
                ExprKind::Value(value) => value.to_string(),
                _ => panic!("unexpected expression kind"),
            };
            self.events.push(format!("enter {name}"));
            self.skip == Some(name.as_str())
        }
        fn leave(&mut self, node: &mut dyn Node) -> bool {
            let Some(expr) = node.as_any().downcast_ref::<ExprNode>() else {
                return true;
            };
            let name = match &expr.Kind {
                ExprKind::Between { .. } => "root".to_owned(),
                ExprKind::Unary { .. } => "unary".to_owned(),
                ExprKind::Value(value) => value.to_string(),
                _ => panic!("unexpected expression kind"),
            };
            self.events.push(format!("leave {name}"));
            self.stop != Some(name.as_str())
        }
    }
    fn fixture() -> ExprNode {
        ExprNode {
            Kind: ExprKind::Between {
                Expr: Box::new(ExprNode {
                    Kind: ExprKind::Unary {
                        Op: Default::default(),
                        V: Box::new(ExprNode::Value("A".into())),
                    },
                    ..Default::default()
                }),
                Left: Box::new(ExprNode::Value("B".into())),
                Right: Box::new(ExprNode::Value("C".into())),
                Not: false,
            },
            ..Default::default()
        }
    }
    let mut trace = Trace {
        events: Vec::new(),
        skip: None,
        stop: None,
    };
    assert!(Walk(&mut fixture(), &mut trace));
    assert_eq!(
        trace.events,
        [
            "enter root",
            "enter unary",
            "enter A",
            "leave A",
            "leave unary",
            "enter B",
            "leave B",
            "enter C",
            "leave C",
            "leave root"
        ]
    );

    trace.events.clear();
    trace.skip = Some("unary");
    assert!(Walk(&mut fixture(), &mut trace));
    assert_eq!(
        trace.events,
        [
            "enter root",
            "enter unary",
            "leave unary",
            "enter B",
            "leave B",
            "enter C",
            "leave C",
            "leave root"
        ]
    );

    trace.events.clear();
    trace.skip = None;
    trace.stop = Some("B");
    assert!(!Walk(&mut fixture(), &mut trace));
    assert_eq!(
        trace.events,
        [
            "enter root",
            "enter unary",
            "enter A",
            "leave A",
            "leave unary",
            "enter B",
            "leave B"
        ]
    );
}

#[test]
fn go_merge_27_query_watch_skip_children() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<crate::AddQueryWatchStmt>() {
                self.0.push("enter root");
                return true;
            }
            false
        }
        fn leave(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<crate::AddQueryWatchStmt>() {
                self.0.push("leave root");
            }
            true
        }
        fn enter_embedded(&mut self, _node: &mut dyn Any) -> bool {
            self.0.push("child");
            false
        }
    }
    let mut root = crate::AddQueryWatchStmt {
        QueryWatchOptionList: vec![Default::default()],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut root, &mut trace));
    assert_eq!(trace.0, ["enter root", "leave root"]);
}

#[test]
fn go_merge_27_value_stored_window_spec_mutates_original() {
    struct SetAlias;
    impl InPlaceVisitor for SetAlias {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_embedded(&mut self, node: &mut dyn Any) -> bool {
            if let Some(spec) = node.downcast_mut::<crate::WindowSpec>() {
                spec.OnlyAlias = true;
            }
            false
        }
    }
    let mut root = SelectStmt {
        WindowSpecs: vec![Default::default()],
        ..Default::default()
    };
    assert!(Walk(&mut root, &mut SetAlias));
    assert!(root.WindowSpecs[0].OnlyAlias);
}

#[test]
fn go_merge_27_as_of_child_stop_propagates_to_parent() {
    struct StopAtAsOf {
        left_root: bool,
    }
    impl Visitor for StopAtAsOf {
        fn enter(&mut self, _node: &dyn Node) -> bool {
            false
        }
        fn leave(&mut self, node: &dyn Node) -> bool {
            if node.as_any().is::<crate::RefreshMaterializedViewStmt>() {
                self.left_root = true;
            }
            true
        }
        fn leave_embedded(&mut self, node: &dyn Any) -> bool {
            !node.is::<crate::AsOfClause>()
        }
    }
    impl InPlaceVisitor for StopAtAsOf {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<crate::RefreshMaterializedViewStmt>() {
                self.left_root = true;
            }
            true
        }
        fn leave_embedded(&mut self, node: &mut dyn Any) -> bool {
            !node.is::<crate::AsOfClause>()
        }
    }
    let mut root = crate::RefreshMaterializedViewStmt {
        AsOf: Some(crate::AsOfClause {
            TsExpr: ExprNode::Value("1".into()),
        }),
        ..Default::default()
    };
    let mut visitor = StopAtAsOf { left_root: false };
    assert!(!root.accept(&mut visitor));
    assert!(!visitor.left_root);
    assert!(!Walk(&mut root, &mut visitor));
    assert!(!visitor.left_root);
}

#[test]
fn go_merge_27_enter_replacement_traverses_replacement_children() {
    struct ReplaceOnEnter {
        visited_child: bool,
    }
    impl InPlaceVisitor for ReplaceOnEnter {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
                if matches!(&expr.Kind, ExprKind::Value(value) if value == "child") {
                    self.visited_child = true;
                }
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_replacement(&mut self, node: &mut dyn Node) -> Option<Box<dyn Any>> {
            let expr = node.as_any().downcast_ref::<ExprNode>()?;
            matches!(&expr.Kind, ExprKind::Value(value) if value == "original").then(|| {
                Box::new(ExprNode {
                    Kind: ExprKind::Parentheses(Box::new(ExprNode::Value("child".into()))),
                    ..Default::default()
                }) as Box<dyn Any>
            })
        }
    }
    let mut root = ExprNode::Value("original".into());
    let mut visitor = ReplaceOnEnter {
        visited_child: false,
    };
    assert!(Walk(&mut root, &mut visitor));
    assert!(visitor.visited_child);
}

#[test]
fn go_merge_27_in_place_walk_has_no_framework_writes_or_allocations() {
    struct Count(usize);
    impl InPlaceVisitor for Count {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            self.0 += 1;
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let mut root = SelectStmt {
        Fields: crate::FieldList {
            Fields: (0..100)
                .map(|_| crate::SelectField {
                    Expr: Some(ExprNode::Value("value".into())),
                    ..Default::default()
                })
                .collect(),
        },
        ..Default::default()
    };
    let fields_ptr = root.Fields.Fields.as_ptr();
    let mut visitor = Count(0);
    // Initialize the thread-local counter before the measured traversal.
    ALLOCATIONS.with(|_| {});
    let before = ALLOCATIONS.with(Cell::get);
    for _ in 0..100 {
        assert!(Walk(&mut root, &mut visitor));
    }
    let after = ALLOCATIONS.with(Cell::get);
    assert_eq!(after - before, 0);
    assert_eq!(visitor.0, 10_100);
    assert_eq!(root.Fields.Fields.as_ptr(), fields_ptr);
    assert!(
        root.Fields
            .Fields
            .iter()
            .all(|field| field.Expr == Some(ExprNode::Value("value".into())))
    );
}

#[test]
fn go_merge_27_driver_leaf_nodes_enter_and_leave_once() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            self.0.push("enter");
            true
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            self.0.push("leave");
            true
        }
    }
    for mut node in [
        ExprNode::Value("value".into()),
        ExprNode {
            Kind: ExprKind::ParamMarker { Offset: 0 },
            ..Default::default()
        },
    ] {
        let mut visitor = Trace(Vec::new());
        assert!(Walk(&mut node, &mut visitor));
        assert_eq!(visitor.0, ["enter", "leave"]);
    }
}
