// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Real unit coverage for the typed plan builder.
//
// These cases preserve the Go suite's schema, access-path, analyze-option,
// privilege, import-assignment and DDL-option boundaries.  SQL cases also run
// through the parser-backed planner fixture.
//
// 以下用例对齐 Go 套件中的 Schema、访问路径、ANALYZE 选项、
// 权限、IMPORT 赋值与 DDL 选项边界；部分 SQL 经解析器驱动的计划构建夹具执行。

use logicalop_dependency::LogicalPlan as _;
use std::collections::{HashMap, HashSet};

use crate::main_test::{exercise_statement_for_test, logical_optimize_default_for_test};
use crate::planbuilder::{
    AlterDDLJobOpt, AnalyzeOptionType, AnalyzeStatement, BuiltPlan, ColumnChoice, ColumnInfo,
    GetMaxWriteSpeedFromExpression, GetThreadOrBatchSizeFromExpression, IndexMeta, NewPlanBuilder,
    PartitionInfo, Privilege, ShowKind, Statement, TableInfo, Value,
    appendVisitInfoIsRestrictedUser, buildShowSchema, buildShowSlowSchema,
    checkAlterDDLJobOptValue, checkImportIntoColAssignments, checkNextGenS3PathWithSem,
    collectVisitInfoFromGrantStmt, fillDefaultDBForStatsObjects, getPathByIndexName,
    getPossibleAccessPaths, handleAnalyzeOptions, removeIgnoredPaths,
};
use crate::task::{Expression, FieldType, TypeCode};

/// 构造指定 TypeCode 的 FieldType。
fn field_type(code: TypeCode) -> FieldType {
    FieldType {
        code,
        flen: 32,
        decimal: 0,
        unsigned: false,
    }
}

/// 构造测试用列元数据。
fn column(id: i64, name: &str, offset: usize, code: TypeCode) -> ColumnInfo {
    ColumnInfo {
        id,
        name: name.to_owned(),
        offset,
        field_type: field_type(code),
        generated: false,
        stored: false,
        hidden: false,
        primary_key: id == 1,
    }
}

/// 构造含主键、二级索引与分区的测试表。
fn table() -> TableInfo {
    TableInfo {
        id: 42,
        db: "test".to_owned(),
        name: "t".to_owned(),
        columns: vec![
            column(1, "id", 0, TypeCode::Int),
            column(2, "name", 1, TypeCode::String),
            column(3, "payload", 2, TypeCode::Bytes),
        ],
        indices: vec![
            IndexMeta {
                id: 10,
                name: "idx_name".to_owned(),
                columns: vec![1],
                prefix_lengths: vec![None],
                unique: false,
                global: false,
                invisible: false,
                multi_valued: false,
                vector: false,
            },
            IndexMeta {
                id: 11,
                name: "idx_id".to_owned(),
                columns: vec![0],
                prefix_lengths: vec![None],
                unique: true,
                global: false,
                invisible: false,
                multi_valued: false,
                vector: false,
            },
        ],
        partitions: vec![
            PartitionInfo {
                id: 101,
                name: "p0".to_owned(),
            },
            PartitionInfo {
                id: 102,
                name: "p1".to_owned(),
            },
        ],
        common_handle: false,
        pk_is_handle: true,
        temporary: false,
    }
}

#[test]
/// 各类 SHOW 的 Schema 列应有非空名与正宽度。
fn show_schemas_have_named_nonzero_width_columns() {
    let kinds = [
        ShowKind::Tables,
        ShowKind::Warnings,
        ShowKind::Slow,
        ShowKind::Regions,
        ShowKind::Distribution,
        ShowKind::TrafficJobs,
        ShowKind::Triggers,
        ShowKind::Events,
        ShowKind::ProcedureStatus,
        ShowKind::NextRowId,
    ];
    // 遍历多种 SHOW 类型分别构建 Schema。
    for kind in kinds {
        let schema = buildShowSchema(&kind);
        assert!(!schema.is_empty());
        assert!(
            schema
                .iter()
                .all(|column| !column.name.is_empty() && column.field_type.flen > 0)
        );
    }
}

#[test]
fn show_slow_schema_appends_ia_remote_read_columns() {
    let (schema, names) = buildShowSlowSchema();
    assert_eq!(schema.len(), 17);
    assert_eq!(names.len(), 17);
    assert_eq!(
        &names[14..],
        [
            "IA_REMOTE_READ_SEGMENT_COUNT",
            "IA_REMOTE_READ_SEGMENT_SIZE",
            "IA_REMOTE_READ_SEGMENT_WAIT_TIME",
        ]
    );
}

#[test]
/// 按名解析访问路径大小写不敏感，并可移除忽略路径。
fn index_name_resolution_and_removal_are_exact_and_case_insensitive() {
    let table = table();
    let paths = getPossibleAccessPaths(&table, &[], &[], &[], true).expect("access paths");
    let name_path = getPathByIndexName(&paths, "IDX_NAME", &table).expect("named index");
    let primary_path = getPathByIndexName(&paths, "primary", &table).expect("primary handle");
    assert!(!std::ptr::eq(name_path, primary_path));
    assert!(getPathByIndexName(&paths, "idx", &table).is_none());
    assert!(getPathByIndexName(&paths, "missing", &table).is_none());

    let remained = removeIgnoredPaths(paths.clone(), &[name_path.clone()]);
    assert_eq!(remained.len() + 1, paths.len());
    assert!(getPathByIndexName(&remained, "idx_name", &table).is_none());
    assert!(getPathByIndexName(&remained, "idx_id", &table).is_some());
}

#[test]
/// ANALYZE 选项填默认值并拒绝非法 Buckets 等边界。
fn analyze_options_fill_defaults_and_reject_invalid_limits() {
    let options = handleAnalyzeOptions(&[
        (AnalyzeOptionType::Buckets, 512),
        (AnalyzeOptionType::TopN, 20),
    ])
    .expect("valid analyze options");
    assert_eq!(options[&AnalyzeOptionType::Buckets], 512);
    assert_eq!(options[&AnalyzeOptionType::TopN], 20);
    assert_eq!(
        crate::planbuilder::fillAnalyzeOptions(options)[&AnalyzeOptionType::CmsketchDepth],
        5
    );
    assert!(handleAnalyzeOptions(&[(AnalyzeOptionType::Buckets, 0)]).is_err());
    assert!(handleAnalyzeOptions(&[(AnalyzeOptionType::Buckets, 100001)]).is_err());
}

#[test]
/// 构建 SHOW、ANALYZE、ALTER DDL JOB 等管理计划。
fn builder_constructs_show_analyze_and_admin_plans() {
    let mut builder = NewPlanBuilder(&[]);
    let show = builder
        .Build(&Statement::Show(ShowKind::Warnings))
        .expect("show plan");
    let BuiltPlan::Show { schema, .. } = show else {
        panic!("expected show plan");
    };
    assert_eq!(schema.len(), 3);

    let analyze = AnalyzeStatement {
        table: table(),
        partition_names: vec!["p1".to_owned()],
        index_names: vec!["idx_name".to_owned()],
        columns: Vec::new(),
        column_choice: ColumnChoice::All,
        options: vec![(AnalyzeOptionType::Buckets, 128)],
        reset_options: Vec::new(),
        version: 2,
        incremental: false,
    };
    let BuiltPlan::Analyze {
        index_tasks,
        column_tasks,
    } = builder
        .Build(&Statement::Analyze(analyze))
        .expect("analyze plan")
    else {
        panic!("expected analyze plan");
    };
    assert_eq!(index_tasks.len(), 1);
    assert_eq!(index_tasks[0].physical_id, 102);
    assert!(column_tasks.is_empty());

    let ddl = builder
        .Build(&Statement::AlterDdlJob {
            job_ids: vec![4],
            options: vec![
                AlterDDLJobOpt::Thread(16),
                AlterDDLJobOpt::MaxWriteSpeed("64MB".to_owned()),
            ],
        })
        .expect("alter DDL job plan");
    let BuiltPlan::Admin { kind, payload, .. } = ddl else {
        panic!("expected admin plan");
    };
    assert_eq!(kind, "alter-ddl-job");
    assert_eq!(payload, vec!["4"]);
}

#[test]
/// 权限 visitInfo 与 IMPORT INTO 列赋值校验对齐 Go。
fn privilege_and_import_checks_preserve_go_boundaries() {
    let visits = collectVisitInfoFromGrantStmt(
        Vec::new(),
        "test",
        "t",
        &[Privilege::Insert, Privilege::Select],
    );
    assert_eq!(visits.len(), 2);
    assert_eq!(visits[0].db, "test");
    assert_eq!(visits[0].table, "t");
    let visits = appendVisitInfoIsRestrictedUser(visits, "root", "RESTRICTED_TABLES_ADMIN");
    assert_eq!(visits.len(), 3);
    assert_eq!(visits[2].dynamicPrivs, vec!["RESTRICTED_USER_ADMIN"]);

    let assignments = vec![
        ("A".to_owned(), Expression::default()),
        ("b".to_owned(), Expression::default()),
    ];
    let positions = checkImportIntoColAssignments(&assignments).expect("unique assignments");
    assert_eq!(positions["a"], 0);
    assert_eq!(positions["b"], 1);
    let duplicate = vec![
        ("A".to_owned(), Expression::default()),
        ("a".to_owned(), Expression::default()),
    ];
    assert!(checkImportIntoColAssignments(&duplicate).is_err());

    let objects = vec![
        (String::new(), "t".to_owned()),
        ("other".to_owned(), "u".to_owned()),
    ];
    assert_eq!(
        fillDefaultDBForStatsObjects(&objects, "test").expect("fill default DB"),
        vec![
            ("test".to_owned(), "t".to_owned()),
            ("other".to_owned(), "u".to_owned())
        ]
    );
}

#[test]
/// DDL 作业选项与 next-gen S3 路径 SEM 校验。
fn ddl_option_and_sem_path_validation_is_real() {
    assert_eq!(
        GetThreadOrBatchSizeFromExpression(&AlterDDLJobOpt::Thread(8)).expect("thread count"),
        8
    );
    assert_eq!(
        GetMaxWriteSpeedFromExpression(&AlterDDLJobOpt::MaxWriteSpeed("2GB".to_owned()))
            .expect("write speed"),
        2_i64 << 30
    );
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::BatchSize(0)).is_err());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::Thread(256)).is_ok());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::Thread(257)).is_err());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::BatchSize(31)).is_err());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::BatchSize(32)).is_ok());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::BatchSize(10_240)).is_ok());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::BatchSize(10_241)).is_err());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::MaxWriteSpeed("1PiB".to_owned())).is_ok());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::MaxWriteSpeed("2PiB".to_owned())).is_err());
    assert!(checkAlterDDLJobOptValue(&AlterDDLJobOpt::MaxWriteSpeed("MiB".to_owned())).is_err());

    assert!(checkNextGenS3PathWithSem("s3://bucket/path?region=us-east-1").is_err());
    assert!(checkNextGenS3PathWithSem("S3://bucket?access-key=ak&secret-access-key=sk").is_ok());
    assert!(checkNextGenS3PathWithSem("s3://bucket?access_key=ak&secret_access_key=sk").is_ok());
    assert!(checkNextGenS3PathWithSem("s3://bucket?role-arn=arn:aws:iam::123:role/import").is_ok());
    assert!(checkNextGenS3PathWithSem("s3://bucket?access-key=ak").is_err());
    assert!(
        checkNextGenS3PathWithSem("s3://bucket?external-id=another-keyspace&role-arn=role")
            .is_err()
    );
    assert!(checkNextGenS3PathWithSem("oss://bucket/path").is_err());
}

#[test]
/// 经解析器的核心 SELECT/CTE 形状可完成构建与逻辑优化。
fn parser_backed_builder_handles_core_statement_shapes() {
    // 覆盖投影、聚合与 IN 子查询等核心形状。
    for sql in [
        "select a from t",
        "select a, count(*) from t group by a",
        "select * from t where a in (select a from t2)",
    ] {
        exercise_statement_for_test(sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let cte = "with cte as (select a from t) select * from cte";
    let (_, logical) =
        logical_optimize_default_for_test(cte).unwrap_or_else(|error| panic!("{cte}: {error}"));
    // CTE 优化后输出列数为 1。
    assert_eq!(logical.Schema().Len(), 1);

    let _values = [
        Value::Int(1),
        Value::String("real builder value".to_owned()),
    ];
}

#[test]
fn dynamic_defaults_and_raw_options() {
    use crate::planbuilder::*;
    use vardef_dependency as vardef;
    let old = (
        vardef::AnalyzeDefaultNumBuckets.Load(),
        vardef::AnalyzeDefaultNumTopN.Load(),
    );
    struct Restore(u64, u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            vardef::AnalyzeDefaultNumBuckets.Store(self.0);
            vardef::AnalyzeDefaultNumTopN.Store(self.1);
        }
    }
    let _restore = Restore(old.0, old.1);
    vardef::AnalyzeDefaultNumBuckets.Store(512);
    vardef::AnalyzeDefaultNumTopN.Store(150);
    let raw = handleAnalyzeOptions(&[]).unwrap();
    assert!(
        raw.is_empty(),
        "validation must retain only explicitly specified options"
    );
    let defaults = fillAnalyzeOptionsV2(raw);
    assert_eq!(defaults[&AnalyzeOptionType::Buckets], 512);
    assert_eq!(defaults[&AnalyzeOptionType::TopN], 150);
    assert_eq!(
        defaults[&AnalyzeOptionType::SampleRate],
        (-1.0_f64).to_bits()
    );
    assert_eq!(defaults[&AnalyzeOptionType::NumSamples], 0);
    let explicit = handleAnalyzeOptions(&[(AnalyzeOptionType::Buckets, 1024)]).unwrap();
    let filled = fillAnalyzeOptionsV2(explicit);
    assert_eq!(filled[&AnalyzeOptionType::Buckets], 1024);
    assert_eq!(filled[&AnalyzeOptionType::TopN], 150);
    let builder = NewPlanBuilder(&[]);
    let saved = [(AnalyzeOptionType::Buckets, 128)].into_iter().collect();
    let merged = builder
        .genV2AnalyzeOptions(&[], &HashSet::new(), &saved)
        .unwrap();
    assert_eq!(merged[&AnalyzeOptionType::Buckets], 128);
    assert_eq!(merged[&AnalyzeOptionType::TopN], 150);
}

#[test]
fn explicit_option_boundaries() {
    use crate::planbuilder::*;
    for (key, value) in [
        (AnalyzeOptionType::TopN, 0),
        (AnalyzeOptionType::TopN, 100000),
        (AnalyzeOptionType::Buckets, 100000),
        (AnalyzeOptionType::CmsketchDepth, CMSketchSizeLimit),
        (AnalyzeOptionType::NumSamples, 5000000),
        (AnalyzeOptionType::SampleRate, 1.0_f64.to_bits()),
    ] {
        assert_eq!(
            handleAnalyzeOptions(&[(key, value)]).unwrap().get(&key),
            Some(&value)
        );
    }
    for (key, value) in [
        (AnalyzeOptionType::Buckets, 0),
        (AnalyzeOptionType::Buckets, 100001),
        (AnalyzeOptionType::TopN, 100001),
        (AnalyzeOptionType::CmsketchDepth, CMSketchSizeLimit + 1),
        (AnalyzeOptionType::NumSamples, 5000001),
        (AnalyzeOptionType::SampleRate, 0.0_f64.to_bits()),
        (AnalyzeOptionType::SampleRate, 1.1_f64.to_bits()),
    ] {
        assert!(handleAnalyzeOptions(&[(key, value)]).is_err());
    }
    assert!(
        handleAnalyzeOptions(&[
            (AnalyzeOptionType::NumSamples, 1),
            (AnalyzeOptionType::SampleRate, 0.5_f64.to_bits())
        ])
        .unwrap_err()
        .0
        .contains("Don't set both")
    );
}

#[test]
fn go_error_messages_and_table_plan() {
    use crate::planbuilder::*;
    for (key, value, message) in [
        (
            AnalyzeOptionType::TopN,
            100001,
            "Value of analyze option TOPN should not be larger than 100000",
        ),
        (
            AnalyzeOptionType::Buckets,
            100001,
            "Value of analyze option BUCKETS should be positive and not larger than 100000",
        ),
        (
            AnalyzeOptionType::SampleRate,
            2.0_f64.to_bits(),
            "Value of analyze option SAMPLERATE should not larger than 1.000000, and should be greater than 0",
        ),
    ] {
        assert_eq!(
            handleAnalyzeOptions(&[(key, value)]).unwrap_err().0,
            message
        );
    }
    let mut builder = NewPlanBuilder(&[]);
    let stmt = AnalyzeStatement {
        table: table(),
        partition_names: vec!["p1".into()],
        index_names: vec![],
        columns: vec![],
        column_choice: ColumnChoice::All,
        options: vec![(AnalyzeOptionType::Buckets, 1024)],
        reset_options: Vec::new(),
        version: 2,
        incremental: false,
    };
    let BuiltPlan::Analyze { column_tasks, .. } = builder.Build(&Statement::Analyze(stmt)).unwrap()
    else {
        panic!("expected analyze");
    };
    assert_eq!(column_tasks.len(), 1);
    assert_eq!(column_tasks[0].options[&AnalyzeOptionType::Buckets], 1024);
    assert_eq!(
        column_tasks[0].options[&AnalyzeOptionType::TopN],
        vardef_dependency::AnalyzeDefaultNumTopN.Load()
    );
    let table_saved = [
        (AnalyzeOptionType::Buckets, 256),
        (AnalyzeOptionType::TopN, 100),
    ]
    .into_iter()
    .collect();
    let partition_saved = [(AnalyzeOptionType::Buckets, 512)].into_iter().collect();
    let saved = mergeAnalyzeOptions(partition_saved, &table_saved);
    let opts = builder
        .genV2AnalyzeOptions(&[(AnalyzeOptionType::TopN, 0)], &HashSet::new(), &saved)
        .unwrap();
    assert_eq!(opts[&AnalyzeOptionType::Buckets], 512);
    assert_eq!(opts[&AnalyzeOptionType::TopN], 0);
}

#[test]
fn analyze_default_options_reset_saved_values() {
    use crate::planbuilder::*;

    let saved = HashMap::from([
        (AnalyzeOptionType::Buckets, 100),
        (AnalyzeOptionType::TopN, 20),
    ]);
    let resets = HashSet::from([AnalyzeOptionType::Buckets]);
    let merged = mergeAnalyzeOptionsWithResets(
        HashMap::from([(AnalyzeOptionType::NumSamples, 1000)]),
        &resets,
        &saved,
    );
    assert_eq!(merged.get(&AnalyzeOptionType::Buckets), None);
    assert_eq!(merged[&AnalyzeOptionType::TopN], 20);
    assert_eq!(merged[&AnalyzeOptionType::NumSamples], 1000);

    let pinned_topn = mergeAnalyzeOptionsWithResets(
        HashMap::from([(AnalyzeOptionType::TopN, 0)]),
        &HashSet::new(),
        &saved,
    );
    assert_eq!(pinned_topn[&AnalyzeOptionType::TopN], 0);

    let table_saved = HashMap::from([(AnalyzeOptionType::Buckets, 100)]);
    let partition_saved = HashMap::from([(AnalyzeOptionType::TopN, 10)]);
    let layered = overrideAnalyzeOptions(&partition_saved, &table_saved);
    assert_eq!(layered[&AnalyzeOptionType::Buckets], 100);
    assert_eq!(layered[&AnalyzeOptionType::TopN], 10);
}
