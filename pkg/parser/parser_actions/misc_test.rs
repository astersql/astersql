// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn matching_procedure_labels_leave_label_end_empty_like_go() {
    for sql in [
        "CREATE PROCEDURE p() outer_label: BEGIN SELECT 1; END outer_label",
        "CREATE PROCEDURE p() loop_label: WHILE 1 DO SELECT 1; END WHILE loop_label",
    ] {
        let mut parser = Parser::default();
        let statement = parser
            .ParseOneStmt(sql, "utf8mb4", "utf8mb4_bin")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        let procedure = statement
            .as_any()
            .downcast_ref::<parser_ast::ProcedureInfo>()
            .unwrap();
        let body = procedure.ProcedureBody.as_ref().unwrap();

        if let Some(label) = body
            .as_any()
            .downcast_ref::<parser_ast::ProcedureLabelBlock>()
        {
            assert!(!label.LabelError, "{sql}");
            assert_eq!(label.LabelEnd, "", "{sql}");
        } else if let Some(label) = body
            .as_any()
            .downcast_ref::<parser_ast::ProcedureLabelLoop>()
        {
            assert!(!label.LabelError, "{sql}");
            assert_eq!(label.LabelEnd, "", "{sql}");
        } else {
            panic!("unexpected procedure body for {sql}");
        }
    }
}

#[test]
fn go_merge_25_parsed_procedure_declaration_is_walked() {
    struct Trace(usize);
    impl parser_ast::InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn parser_ast::Node) -> bool {
            if node.as_any().is::<parser_ast::ExprNode>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn parser_ast::Node) -> bool {
            true
        }
    }
    let mut parser = Parser::default();
    let mut statement = parser
        .ParseOneStmt(
            "CREATE PROCEDURE p() BEGIN DECLARE x INT DEFAULT 1; END",
            "utf8mb4",
            "utf8mb4_bin",
        )
        .unwrap();
    let mut trace = Trace(0);
    assert!(parser_ast::Walk(statement.as_mut(), &mut trace));
    assert_eq!(trace.0, 1);
}

#[test]
fn go_merge_25_handler_keeps_condition_nodes() {
    struct Trace(usize);
    impl parser_ast::InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn parser_ast::Node) -> bool {
            if node.as_any().is::<parser_ast::ProcedureErrorCon>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn parser_ast::Node) -> bool {
            true
        }
    }
    let mut parser = Parser::default();
    let mut statement = parser
        .ParseOneStmt(
            "CREATE PROCEDURE p() BEGIN DECLARE EXIT HANDLER FOR SQLWARNING, NOT FOUND, SQLEXCEPTION SELECT 1; END",
            "utf8mb4",
            "utf8mb4_bin",
        )
        .unwrap();
    let procedure = statement
        .as_any()
        .downcast_ref::<parser_ast::ProcedureInfo>()
        .unwrap();
    let block = procedure
        .ProcedureBody
        .as_ref()
        .unwrap()
        .as_any()
        .downcast_ref::<parser_ast::ProcedureBlock>()
        .unwrap();
    let handler = block
        .ProcedureVars
        .iter()
        .find_map(|value| value.downcast_ref::<parser_ast::ProcedureErrorControl>())
        .unwrap();
    assert_eq!(handler.ErrorCon.len(), 3);
    let mut trace = Trace(0);
    assert!(parser_ast::Walk(statement.as_mut(), &mut trace));
    assert_eq!(trace.0, 3);
}
