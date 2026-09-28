// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 表模式变更构建逻辑的单元测试。
//
// 覆盖：Normal→Import/Restore 成功构造 Job；同模式 no-op；Import→Restore 非法迁移报错。

use crate::{
    AlterTableModeTarget, ErrorKind, JobState, JobType, SessionVariables, TableMode,
    build_alter_table_mode_job,
};

/// 构造固定 schema/table 标识的测试目标。
fn target(current_mode: TableMode, target_mode: TableMode) -> AlterTableModeTarget {
    AlterTableModeTarget {
        current_mode,
        target_mode,
        schema_id: 101,
        table_id: 202,
        schema_name: "TestDB".into(),
        table_name: "TestTable".into(),
    }
}

/// 验证合法迁移、no-op 与非法迁移三种路径。
#[test]
fn canonical_alter_table_mode_builds_job() {
    let vars = SessionVariables {
        cdc_write_source: 7,
        sql_mode: 4,
    };
    // Normal → Import：应产出 Job 与 Args，noop=false。
    let (job, args, noop) =
        build_alter_table_mode_job(vars, target(TableMode::Normal, TableMode::Import)).unwrap();
    let job = job.unwrap();
    let args = args.unwrap();
    assert!(!noop);
    assert_eq!(job.version, 2);
    assert_eq!(job.job_type, JobType::AlterTableMode);
    assert_eq!((job.schema_id, job.table_id), (101, 202));
    assert_eq!((job.cdc_write_source, job.sql_mode), (7, 4));
    assert!(job.binlog_info_present);
    assert_eq!(job.query, "skip");
    assert_eq!(job.state, JobState::None);
    assert_eq!(
        (job.schema_name, job.table_name),
        ("testdb".to_owned(), "testtable".to_owned())
    );
    assert_eq!(args.table_mode, TableMode::Import);
    assert_eq!((args.schema_id, args.table_id), (101, 202));
    assert_eq!(
        job.involving_schemas,
        vec![("testdb".to_owned(), "testtable".to_owned())]
    );
}

#[test]
fn alter_table_mode_noop_and_invalid_transition_match_go() {
    let vars = SessionVariables {
        cdc_write_source: 7,
        sql_mode: 4,
    };

    // 同模式：noop，无 Job/Args。
    let (job, args, noop) =
        build_alter_table_mode_job(vars, target(TableMode::Normal, TableMode::Normal)).unwrap();
    assert!(noop);
    assert!(job.is_none() && args.is_none());

    // Normal → Restore is valid; only Import ↔ Restore is invalid.
    let (job, args, noop) =
        build_alter_table_mode_job(vars, target(TableMode::Normal, TableMode::Restore)).unwrap();
    assert!(!noop);
    assert_eq!(job.unwrap().job_type, JobType::AlterTableMode);
    assert_eq!(args.unwrap().table_mode, TableMode::Restore);
    let error = build_alter_table_mode_job(vars, target(TableMode::Import, TableMode::Restore))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Invalid);
    assert_eq!(
        error.message,
        "invalid table mode transition Import -> Restore for TestTable"
    );
}
