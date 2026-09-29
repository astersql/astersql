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
