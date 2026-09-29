// Copyright 2026 AsterSQL.

use crate::{DoStmt, ExprKind, ExprNode, InPlaceVisitor, Node, ValueDatum, Walk};

#[test]
fn go_merge_23_in_place_traversal_preserves_order_skip_and_short_circuit() {
    struct Recorder {
        events: Vec<String>,
        skip_root: bool,
        stop_at: Option<&'static str>,
    }

    impl InPlaceVisitor for Recorder {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.events.push("enter root".into());
                return self.skip_root;
            }
            if let Some(expr) = node.as_any_mut().downcast_mut::<ExprNode>() {
                if let ExprKind::Value(value) = &mut expr.Kind {
                    self.events.push(format!("enter {value}"));
                    if let ValueDatum::String(text) = &mut value.Datum {
                        text.push('!');
                    }
                }
            }
            false
        }

        fn leave(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.events.push("leave root".into());
                return true;
            }
            if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
                if let ExprKind::Value(value) = &expr.Kind {
                    self.events.push(format!("leave {value}"));
                    return self.stop_at != Some(value.as_str().trim_end_matches('!'));
                }
            }
            true
        }
    }

    let make_stmt = || DoStmt {
        Exprs: vec![
            ExprNode::Value("first".into()),
            ExprNode::Value("second".into()),
        ],
        ..Default::default()
    };

    let mut skipped = make_stmt();
    let mut recorder = Recorder {
        events: vec![],
        skip_root: true,
        stop_at: None,
    };
    assert!(Walk(&mut skipped, &mut recorder));
    assert_eq!(recorder.events, ["enter root", "leave root"]);
    assert_eq!(skipped.Exprs, make_stmt().Exprs);

    let mut completed = make_stmt();
    let mut recorder = Recorder {
        events: vec![],
        skip_root: false,
        stop_at: None,
    };
    assert!(Walk(&mut completed, &mut recorder));
    assert_eq!(
        recorder.events,
        [
            "enter root",
            "enter first",
            "leave first!",
            "enter second",
            "leave second!",
            "leave root"
        ]
    );
    assert!(matches!(&completed.Exprs[0].Kind, ExprKind::Value(value) if value == "first!"));
    assert!(matches!(&completed.Exprs[1].Kind, ExprKind::Value(value) if value == "second!"));

    let mut stopped = make_stmt();
    let mut recorder = Recorder {
        events: vec![],
        skip_root: false,
        stop_at: Some("first"),
    };
    assert!(!Walk(&mut stopped, &mut recorder));
    assert_eq!(
        recorder.events,
        ["enter root", "enter first", "leave first!"]
    );
    assert!(matches!(&stopped.Exprs[1].Kind, ExprKind::Value(value) if value == "second"));
}
