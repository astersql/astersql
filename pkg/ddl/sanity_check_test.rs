// Copyright 2026 AsterSQL.

use crate::delete_range::{DeleteRangeAction, DeleteRangeJob, IndexArgument};
use crate::sanity_check::{
    DeleteRangeCountContext, HistoryJobAction, HistorySanityError, HistoryStatementKind,
    check_history_job, expected_delete_range_count,
};

fn job(action: DeleteRangeAction) -> DeleteRangeJob {
    DeleteRangeJob {
        id: 7,
        table_id: 42,
        action,
        rollback_done: false,
        cancelled: false,
        old_physical_table_ids: Vec::new(),
        partition_ids: Vec::new(),
        index_arguments: Vec::new(),
        index_ids: Vec::new(),
        old_global_indexes: Vec::new(),
        subjobs: Vec::new(),
    }
}

#[test]
#[should_panic(expected = "finished drop-index job must contain an index argument")]
fn drop_index_requires_the_finished_index_argument_like_go() {
    let job = job(DeleteRangeAction::DropIndex);
    let _ = expected_delete_range_count(&mut DeleteRangeCountContext::default(), &job);
}

#[test]
fn delete_range_formulas_and_cross_subjob_deduplication_match_go() {
    let mut drop_table = job(DeleteRangeAction::DropTable);
    drop_table.old_physical_table_ids = vec![1, 2];
    assert_eq!(
        expected_delete_range_count(&mut DeleteRangeCountContext::default(), &drop_table),
        3
    );

    let mut add_index = job(DeleteRangeAction::AddIndex);
    add_index.partition_ids = vec![10, 11];
    add_index.rollback_done = true;
    add_index.index_arguments = vec![
        IndexArgument {
            global: false,
            ..IndexArgument::default()
        },
        IndexArgument {
            global: true,
            ..IndexArgument::default()
        },
    ];
    assert_eq!(
        expected_delete_range_count(&mut DeleteRangeCountContext::default(), &add_index),
        6
    );

    let mut first = job(DeleteRangeAction::ModifyColumn);
    first.partition_ids = vec![10, 11];
    first.index_ids = vec![1, 1, 2];
    let mut second = job(DeleteRangeAction::ModifyColumn);
    second.partition_ids = vec![10, 11];
    second.index_ids = vec![2, 3];
    let mut multi = job(DeleteRangeAction::MultiSchemaChange);
    multi.subjobs = vec![first, second];
    assert_eq!(
        expected_delete_range_count(&mut DeleteRangeCountContext::default(), &multi),
        6
    );
}

#[test]
fn history_query_exceptions_and_skip_match_go() {
    assert_eq!(
        check_history_job(1, HistoryJobAction::UnlockTable, "", &[]),
        Ok(())
    );
    assert_eq!(
        check_history_job(2, HistoryJobAction::UnlockTable, "unlock tables", &[]),
        Err(HistorySanityError::QueryMustBeEmpty(2))
    );
    assert_eq!(
        check_history_job(3, HistoryJobAction::Other, "skip", &[]),
        Ok(())
    );
}

#[test]
fn history_statement_count_and_action_specific_kinds_match_go() {
    assert_eq!(
        check_history_job(4, HistoryJobAction::Other, "", &[]),
        Err(HistorySanityError::EmptyQuery(4))
    );
    assert_eq!(
        check_history_job(
            7,
            HistoryJobAction::CreatePlacementPolicy,
            "create placement policy p followers=3",
            &[HistoryStatementKind::CreatePlacementPolicy],
        ),
        Ok(())
    );
    assert_eq!(
        check_history_job(
            7,
            HistoryJobAction::CreateSchema,
            "create database d",
            &[HistoryStatementKind::CreateSchema],
        ),
        Ok(())
    );
    assert_eq!(
        check_history_job(
            5,
            HistoryJobAction::Other,
            "create table t(a int); create table u(a int)",
            &[
                HistoryStatementKind::CreateTable,
                HistoryStatementKind::CreateTable
            ],
        ),
        Err(HistorySanityError::InvalidStatementCount(5))
    );
    assert_eq!(
        check_history_job(
            6,
            HistoryJobAction::CreateTables,
            "create table t(a int); create sequence s",
            &[
                HistoryStatementKind::CreateTable,
                HistoryStatementKind::CreateSequence
            ],
        ),
        Ok(())
    );
    assert_eq!(
        check_history_job(
            7,
            HistoryJobAction::CreateTable,
            "create view v as select 1",
            &[HistoryStatementKind::CreateView],
        ),
        Err(HistorySanityError::UnexpectedStatementKind(7))
    );
    assert_eq!(
        check_history_job(
            8,
            HistoryJobAction::Other,
            "select 1",
            &[HistoryStatementKind::NonDdl],
        ),
        Err(HistorySanityError::NonDdlStatement(8))
    );
}
