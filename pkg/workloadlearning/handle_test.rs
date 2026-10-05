// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载学习 Handle 的单元测试。
//
// 覆盖指标落盘、按表 ID 累加，以及从执行计划树提取并归一化表读代价。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::cache_test::MemoryStore;
use crate::*;

/// 验证 SaveTableReadCostMetrics 写入新版本且存储中有一行。
#[test]
fn TestSaveReadTableCostMetrics() {
    let store = Arc::new(MemoryStore::default());
    let store_trait: Arc<dyn WorkloadStore> = store.clone();
    let handle = NewWorkloadLearningHandle(store_trait);
    let metric = TableReadCostMetrics {
        DbName: CIStr::new("test"),
        TableName: CIStr::new("test"),
        TableScanTime: Duration::from_nanos(10),
        TableMemUsage: 10,
        ReadFrequency: 10,
        TableReadCost: 1.0,
    };
    assert_eq!(
        handle
            .SaveTableReadCostMetrics(
                &HashMap::from([(1, metric)]),
                SystemTime::now(),
                SystemTime::now()
            )
            .unwrap(),
        1
    );
    assert_eq!(store.metrics.lock().unwrap()[&1].len(), 1);
}

/// 验证按频率放大后与已有表指标累加，并插入新表条目。
#[test]
fn TestAccumulateMetricsGroupByTableID() {
    let store = MemoryStore::default();
    store.tables.lock().unwrap().extend([
        (("test".into(), "test1".into()), 1),
        (("test".into(), "test2".into()), 2),
    ]);
    let current = vec![
        TableReadCostMetrics {
            DbName: CIStr::new("test"),
            TableName: CIStr::new("test1"),
            TableScanTime: Duration::from_nanos(10),
            TableMemUsage: 10,
            ..Default::default()
        },
        TableReadCostMetrics {
            DbName: CIStr::new("test"),
            TableName: CIStr::new("test2"),
            TableScanTime: Duration::from_nanos(10),
            TableMemUsage: 10,
            ..Default::default()
        },
    ];
    let mut previous = HashMap::from([(
        1,
        TableReadCostMetrics {
            DbName: CIStr::new("test"),
            TableName: CIStr::new("test1"),
            TableScanTime: Duration::from_nanos(10),
            TableMemUsage: 10,
            ReadFrequency: 1,
            ..Default::default()
        },
    )]);
    AccumulateMetricsGroupByTableID(current, 2, &mut previous, &store);
    assert_eq!(previous[&1].ReadFrequency, 3);
    assert_eq!(previous[&1].TableScanTime, Duration::from_nanos(30));
    assert_eq!(previous[&1].TableMemUsage, 30);
    assert_eq!(previous[&2].ReadFrequency, 2);
}

/// 端到端：从 TableReader 计划提取指标并归一化代价，同时校验时长解析。
#[test]
fn plan_tree_extraction_and_cost_normalization_are_executable() {
    let store = Arc::new(MemoryStore::default());
    store
        .tables
        .lock()
        .unwrap()
        .insert(("db".into(), "t".into()), 9);
    let plan = ExplainOperator {
        name: "TableReader_1".into(),
        memory_bytes: 20,
        root_basic_exec_info: "time: 2ms, loops:1".into(),
        children: vec![ExplainOperator {
            name: "TableFullScan_2".into(),
            access_objects: vec![AccessObject {
                database: "db".into(),
                table: "t".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    store.statements.lock().unwrap().push(StatementRecord {
        digest: "d".into(),
        sql: "select * from t".into(),
        binary_plan: serde_json::to_string(&plan).unwrap(),
        frequency: 3,
    });
    let store_trait: Arc<dyn WorkloadStore> = store;
    let metrics = NewWorkloadLearningHandle(store_trait)
        .HandleTableReadCost()
        .unwrap();
    // frequency=3：扫描时间与内存均乘 3；单表归一化后代价为 1+1=2。
    assert_eq!(metrics[&9].TableScanTime, Duration::from_millis(6));
    assert_eq!(metrics[&9].TableMemUsage, 60);
    assert_eq!(metrics[&9].TableReadCost, 2.0);
    assert_eq!(
        extractScanTimeFromString("time:274.5µs, loops:1").unwrap(),
        Duration::from_nanos(274_500)
    );
}

/// Go 使用 `time.ParseDuration`，应接受复合的小时、分钟和秒片段。
#[test]
fn scan_time_accepts_go_duration_syntax() {
    assert_eq!(
        extractScanTimeFromString("time:1h2m3.5s, loops:1").unwrap(),
        Duration::from_secs(3_723) + Duration::from_millis(500)
    );
}

/// Go 只读取 RootGroupExecInfo 的首项；首项失败后应继续回退到 cop info。
#[test]
fn scan_time_uses_only_first_group_info_before_cop_fallback() {
    let op = ExplainOperator {
        root_group_exec_info: vec!["invalid".into(), "time:2ms, loops:1".into()],
        cop_exec_info: "time:3ms, loops:1".into(),
        ..Default::default()
    };
    assert_eq!(
        extractScanTimeFromExecutionInfo(&op).unwrap(),
        Duration::from_millis(3)
    );
}

/// 非替换型 AST 访问只返回遍历控制信号，同时按小写 schema 去重。
#[test]
fn db_name_extractor_uses_in_place_control_results() {
    let mut extractor = DBNameExtractor::default();
    let table = Node::TableName {
        schema: "Analytics".into(),
        table: "events".into(),
    };

    assert!(!extractor.Enter(&table));
    assert!(!extractor.Enter(&table));
    assert!(!extractor.Enter(&Node::Other));
    assert!(extractor.Leave(&table));
    assert_eq!(extractor.DBs, HashSet::from(["analytics".to_owned()]));
}
