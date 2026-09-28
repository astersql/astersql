// Copyright 2026 AsterSQL.

// executor 内部测试工具的 Rust 回归测试。
//
// 重点校验 Rust 端与 Go 版本一致的用例默认值和字符串格式，以及 Mock 物理计划、
// 数据源、随机 Chunk 与内存超限动作的关键测试契约。

use super::*;
use astersql_executor_internal_exec::executor::{Chunk, ExecContext, Executor};

// Sort 用例的展示文本会被基准测试结果使用，因此字段顺序和 Go 风格数组格式都需固定。
#[test]
fn sort_case_string_matches_go_format() {
    let case = DefaultSortTestCase(SessionContext::default());
    assert_eq!(
        case.to_string(),
        "(rows:300000, orderBy:[0 1], ndvs: [0 0])"
    );
}

// Window 用例沿用 Go/MySQL 的字段类型名称，避免 Rust 枚举调试文本泄漏到结果中。
#[test]
fn window_case_string_uses_go_field_type_name() {
    let case = DefaultWindowTestCase(SessionContext::default());
    assert_eq!(
        case.to_string(),
        "(func:row_number, aggColType:double, numFunc:1, ndv:1000, rows:10000000, sorted:true, concurrency:1, pipelined:0)"
    );
}

// 物理计划只包装执行器；重复查询必须仍指向同一个 Mock 数据源，而不是重建实例。
#[test]
fn mock_physical_plan_returns_the_same_executor_each_time() {
    let source = BuildMockDataSource(MockDataSourceParameters {
        DataSchema: vec![ColumnDef::new(0, FieldKind::LongLong)],
        Rows: 0,
        ..Default::default()
    });
    let plan = BuildMockDataPhysicalPlan(Box::new(source));
    assert_eq!(
        plan.GetExecutor().executorType(),
        "*testutil.MockDataSource"
    );
    assert_eq!(
        plan.GetExecutor().executorType(),
        "*testutil.MockDataSource"
    );
}

// 汇总各算子构造器的 Go 默认参数，并验证内存限制会同时下沉到会话与语句跟踪器。
#[test]
fn operator_cases_match_go_defaults() {
    let agg = DefaultAggTestCase(SessionContext::default(), "hash".into());
    assert_eq!(agg.Ctx.vars.init_chunk_size, DEF_INIT_CHUNK_SIZE);
    assert_eq!(agg.AggFunc, "sum");
    assert_eq!(agg.Rows, 10_000_000);
    assert_eq!(
        agg.to_string(),
        "(execType:hash, aggFunc:sum, ndv:1000, hasDistinct:false, rows:10000000, concurrency:4, sorted:true)"
    );

    let limit = DefaultLimitTestCase(SessionContext::default());
    assert_eq!(limit.Ctx.vars.statement_memory_tracker.limit, -1);
    assert!(!limit.Ctx.vars.statement_memory_tracker.attached);
    assert_eq!(
        limit.to_string(),
        "(rows:30000, offset:10000, count:10000, inline_projection:false)"
    );

    let sort = SortTestCaseWithMemoryLimit(SessionContext::default(), 64);
    assert_eq!(sort.Ctx.vars.memory_tracker.limit, 64);
    assert_eq!(sort.Ctx.vars.statement_memory_tracker.limit, 64);
    assert!(sort.Ctx.vars.statement_memory_tracker.attached);

    let window = DefaultWindowTestCase(SessionContext::default());
    assert_eq!(window.RawDataSmall, "xxxxxxxxxxxxxxxx");
    assert_eq!(window.Columns[0].kind, FieldKind::Double);
    assert_eq!(window.Columns[0].kind.to_string(), "double");
}

// 数据源既要保留 MySQL 类型码映射，也要遵守 Open/Next 的分批读取与耗尽语义。
#[test]
fn mock_data_source_preserves_go_type_codes_and_chunk_lifecycle() {
    let kinds = vec![
        FieldKind::Tiny,
        FieldKind::Short,
        FieldKind::Int24,
        FieldKind::Long,
        FieldKind::LongLong,
        FieldKind::Float,
        FieldKind::Double,
        FieldKind::Decimal,
        FieldKind::Varchar,
        FieldKind::VarString,
        FieldKind::Year,
        FieldKind::Date,
        FieldKind::DateTime,
        FieldKind::Timestamp,
        FieldKind::Duration,
        FieldKind::Enum,
        FieldKind::Set,
        FieldKind::Bit,
        FieldKind::Json,
    ];
    let all_types = BuildMockDataSource(MockDataSourceParameters {
        DataSchema: kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| ColumnDef::new(index, kind.clone()))
            .collect(),
        Rows: 0,
        ..Default::default()
    });
    assert_eq!(
        all_types
            .RetFieldTypes()
            .iter()
            .map(|field| field.type_code)
            .collect::<Vec<_>>(),
        vec![
            1, 2, 9, 3, 8, 4, 5, 246, 15, 253, 13, 10, 12, 7, 11, 247, 248, 16, 245
        ]
    );

    let mut source = BuildMockDataSource(MockDataSourceParameters {
        DataSchema: vec![ColumnDef::new(0, FieldKind::LongLong)],
        Rows: 2,
        ..Default::default()
    });
    let context = ExecContext::default();
    source.Open(&context).unwrap();
    let mut request = Chunk::new(
        &source.RetFieldTypes(),
        source.InitCap(),
        source.MaxChunkSize(),
    );
    source.Next(&context, &mut request).unwrap();
    assert_eq!(request.NumRows(), 2);
    // 第二次读取已耗尽的数据源时，复用的请求 Chunk 必须清空上一批行。
    source.Next(&context, &mut request).unwrap();
    assert_eq!(request.NumRows(), 0);
}

// 随机 Chunk 覆盖不同字段类别，并单独锁定无符号整数与 TypeNull 的确定性行为。
#[test]
fn random_chunks_cover_go_field_categories_and_unsigned_values() {
    let mut schema = vec![
        ColumnDef::new(0, FieldKind::Tiny),
        ColumnDef::new(1, FieldKind::Varchar),
        ColumnDef::new(2, FieldKind::Date),
        ColumnDef::new(3, FieldKind::Duration),
        ColumnDef::new(4, FieldKind::Json),
        ColumnDef::new(5, FieldKind::Null),
    ];
    schema[0].nullable = false;
    schema[0].unsigned = true;
    let chunk = GenRandomChunks(&schema, 4);
    assert_eq!(chunk.NumRows(), 4);
    assert_eq!(chunk.columns.len(), schema.len());
    assert!(
        chunk.columns[0]
            .iter()
            .all(|datum| matches!(datum, Datum::UInt(_)))
    );
    assert!(
        chunk.columns[5]
            .iter()
            .all(|datum| matches!(datum, Datum::Null))
    );
}

// Go 生成器会覆盖 Enum/Set 的全部候选，并按同一规则生成正负 Duration；
// 这些随机分支不能退化成固定占位值。
#[test]
fn random_field_generation_preserves_go_value_domains() {
    let enum_field = ColumnDef {
        nullable: false,
        ..ColumnDef::new(0, FieldKind::Enum)
    };
    let set_field = ColumnDef {
        nullable: false,
        ..ColumnDef::new(1, FieldKind::Set)
    };
    let duration_field = ColumnDef {
        nullable: false,
        ..ColumnDef::new(2, FieldKind::Duration)
    };

    let enums = GenRandomChunks(&[enum_field], 128).columns.remove(0);
    let sets = GenRandomChunks(&[set_field], 128).columns.remove(0);
    let durations = GenRandomChunks(&[duration_field], 256).columns.remove(0);

    assert!(enums.iter().any(|value| value != &enums[0]));
    assert!(sets.iter().any(|value| value != &sets[0]));
    assert!(durations.iter().any(|value| match value {
        Datum::Duration(value) => value.is_negative(),
        _ => false,
    }));
    assert!(durations.iter().any(|value| match value {
        Datum::Duration(value) => !value.is_negative(),
        _ => false,
    }));
}

// Mock 超限动作保持 Go 默认优先级，并为每次触发累计一次计数。
#[test]
fn mock_action_counts_triggers_and_keeps_go_priority() {
    let action = MockActionOnExceed::default();
    assert_eq!(action.GetPriority(), 1);
    action.Action();
    action.Action();
    assert_eq!(action.GetTriggeredNum(), 2);
}
