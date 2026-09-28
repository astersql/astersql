// Copyright 2026 AsterSQL.

use super::compiler::{CompilerPreparedStatement, CompilerStatementView, StatementDatabaseView};

fn prepared() -> CompilerPreparedStatement {
    CompilerPreparedStatement {
        cache: Box::default(),
        statementView: CompilerStatementView {
            statementType: "Execute".to_owned(),
            databases: StatementDatabaseView::Other,
        },
    }
}

#[test]
fn prepared_cache_is_attached_only_when_point_get_reuse_succeeds() {
    assert!(super::compiler::preparedCacheForExec(Some(prepared()), false).is_none());
    assert!(super::compiler::preparedCacheForExec(Some(prepared()), true).is_some());
    assert!(super::compiler::preparedCacheForExec(None, true).is_none());
}
