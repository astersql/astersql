// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::rollingback::{
    ColumnNullability, JobAction, JobState, RollbackError, RollbackJob, SchemaState,
    convert_job_to_rollback, record_rollback_conversion_error, update_columns_null_to_not_null,
};

fn job(action: JobAction, schema_state: SchemaState) -> RollbackJob {
    RollbackJob {
        id: 42,
        action,
        schema_state,
        state: JobState::Cancelling,
        error: None,
        error_count: 0,
        args_rewritten: false,
    }
}

#[test]
fn conversion_errors_cancel_on_the_fourth_failure_like_go() {
    let mut add_index = job(JobAction::AddIndex, SchemaState::DeleteOnly);
    for _ in 0..3 {
        record_rollback_conversion_error(&mut add_index, 3);
        assert_eq!(add_index.state, JobState::Cancelling);
    }
    record_rollback_conversion_error(&mut add_index, 3);

    assert_eq!(add_index.error_count, 4);
    assert_eq!(add_index.state, JobState::Cancelled);
    assert_eq!(
        add_index.error.as_deref(),
        Some("[ddl:-1]rollback DDL job error count exceed the limit 3, cancelled it now")
    );
}

#[test]
fn add_operations_fill_rollback_args_without_changing_the_job_action() {
    let mut add_index = job(JobAction::AddIndex, SchemaState::WriteReorganization);
    assert_eq!(convert_job_to_rollback(&mut add_index, "cancel"), Ok(()));
    assert_eq!(add_index.action, JobAction::AddIndex);
    assert_eq!(add_index.schema_state, SchemaState::DeleteOnly);
    assert_eq!(add_index.state, JobState::RollingBack);
    assert!(add_index.args_rewritten);

    let mut add_column = job(JobAction::AddColumn, SchemaState::WriteOnly);
    assert_eq!(convert_job_to_rollback(&mut add_column, "cancel"), Ok(()));
    assert_eq!(add_column.action, JobAction::AddColumn);
    assert_eq!(add_column.schema_state, SchemaState::DeleteOnly);
    assert_eq!(add_column.state, JobState::RollingBack);
    assert!(add_column.args_rewritten);
}

#[test]
fn drop_operations_cancel_only_before_the_public_object_starts_dropping() {
    for action in [JobAction::DropColumn, JobAction::DropIndex] {
        let mut public = job(action, SchemaState::Public);
        assert_eq!(
            convert_job_to_rollback(&mut public, "cancel"),
            Err(RollbackError::Cancelled)
        );
        assert_eq!(public.state, JobState::Cancelled);

        let mut started = job(action, SchemaState::DeleteOnly);
        assert_eq!(convert_job_to_rollback(&mut started, "cancel"), Ok(()));
        assert_eq!(started.action, action);
        assert_eq!(started.state, JobState::Running);
        assert!(!started.args_rewritten);
    }
}

#[test]
fn exchange_and_partition_paths_match_go_terminal_states() {
    let mut exchange = job(JobAction::ExchangePartition, SchemaState::WriteOnly);
    assert_eq!(convert_job_to_rollback(&mut exchange, "cancel"), Ok(()));
    assert_eq!(exchange.state, JobState::RollbackDone);
    assert_eq!(exchange.schema_state, SchemaState::Public);

    let mut add_partition = job(JobAction::AddPartition, SchemaState::ReplicaOnly);
    assert_eq!(
        convert_job_to_rollback(&mut add_partition, "cancel"),
        Ok(())
    );
    assert_eq!(add_partition.action, JobAction::AddPartition);
    assert_eq!(add_partition.state, JobState::RollingBack);
    assert!(add_partition.args_rewritten);

    let mut reorg_none = job(JobAction::ReorganizePartition, SchemaState::None);
    assert_eq!(
        convert_job_to_rollback(&mut reorg_none, "cancel"),
        Err(RollbackError::Cancelled)
    );
    let mut reorg_public = job(JobAction::ReorganizePartition, SchemaState::Public);
    assert_eq!(convert_job_to_rollback(&mut reorg_public, "cancel"), Ok(()));
    assert_eq!(reorg_public.state, JobState::Running);
}

#[test]
fn drop_and_alter_constraints_continue_after_their_public_state() {
    for action in [JobAction::DropConstraint, JobAction::AlterConstraint] {
        let mut public = job(action, SchemaState::Public);
        assert_eq!(
            convert_job_to_rollback(&mut public, "cancel"),
            Err(RollbackError::Cancelled)
        );

        let mut started = job(action, SchemaState::WriteOnly);
        assert_eq!(convert_job_to_rollback(&mut started, "cancel"), Ok(()));
        assert_eq!(started.state, JobState::Running);
        assert!(!started.args_rewritten);
    }
}

#[test]
fn update_null_columns_sets_not_null_and_clears_the_prevent_flag() {
    let mut columns = [
        ColumnNullability {
            column_id: 1,
            not_null: false,
            prevent_null_insert: true,
        },
        ColumnNullability {
            column_id: 2,
            not_null: false,
            prevent_null_insert: false,
        },
    ];

    assert_eq!(update_columns_null_to_not_null(&mut columns, &[1]), Ok(()));
    assert!(columns[0].not_null);
    assert!(!columns[0].prevent_null_insert);
    assert!(!columns[1].not_null);
    assert_eq!(
        update_columns_null_to_not_null(&mut columns, &[99]),
        Err(RollbackError::InvalidState)
    );
}
