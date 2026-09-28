// Copyright 2026 AsterSQL.

// Index LookUp Join 的单元测试。
//
// 通过内存中的固定内表数据，分别验证 outer worker 的动态分批与过滤、inner worker
// 的查找键去重和映射重建，以及完整执行器对外表顺序和左连接未命中补行的保证。

use super::index_lookup_join::{
    IndexJoinExecutorBuilder, IndexJoinLookupContent, IndexLookUpJoin, IndexLookUpJoinRuntimeStats,
    InnerCtx, InnerWorker, LookUpJoinTask, OuterCtx, OuterWorker, encode_key,
};
use super::joiner::{JoinType, Joiner, Row};
use super::row_table_builder::Value;
use std::sync::Arc;

/// 将整数切片转换为测试使用的行，避免样例被 Value 构造细节淹没。
fn int_row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

/// 仅返回首列命中查找键的固定行集，用来隔离真实索引读取依赖。
struct StaticBuilder {
    rows: Vec<Row>,
}

impl IndexJoinExecutorBuilder for StaticBuilder {
    /// 模拟内表索引扫描：一个查找键命中所有首列相等的候选行。
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        Ok(self
            .rows
            .iter()
            .filter(|row| {
                lookup_contents
                    .iter()
                    .any(|content| content.keys.first() == row.first())
            })
            .cloned()
            .collect())
    }
}

/// 构造以首列为连接键、使用普通等值语义的内表上下文。
fn inner_context(rows: Vec<Row>) -> InnerCtx {
    InnerCtx {
        builder: Box::new(StaticBuilder { rows }),
        key_columns: vec![0],
        key_column_ids: vec![1],
        null_safe: false,
    }
}

#[test]
/// Go 在读取前先扩大 batch：初始值 1 的首批应读取 2 行。
fn outer_worker_filters_rows_and_grows_batches() {
    let filter = Arc::new(|row: &[Value]| {
        Ok(Some(
            matches!(row.first(), Some(Value::Int(value)) if *value > 1),
        ))
    });
    let context = OuterCtx {
        rows: vec![int_row(&[1]), int_row(&[2]), int_row(&[3])],
        key_columns: vec![0],
        filters: vec![filter],
    };
    let mut worker = OuterWorker::new(1, 2).unwrap();

    let first = worker.build_task(&context).unwrap().unwrap();
    assert_eq!(first.outer_rows, [int_row(&[1]), int_row(&[2])]);
    assert_eq!(first.outer_match, [false, true]);
    let second = worker.build_task(&context).unwrap().unwrap();
    assert_eq!(second.outer_rows, [int_row(&[3])]);
    assert_eq!(second.outer_match, [true]);
    assert!(worker.build_task(&context).unwrap().is_none());
}

#[test]
/// Go VectorizedFilter 的错误必须返回主线程，不能被当成 false 吞掉。
fn outer_worker_propagates_filter_errors() {
    let context = OuterCtx {
        rows: vec![int_row(&[1])],
        key_columns: vec![0],
        filters: vec![Arc::new(|_| Err("filter failed".into()))],
    };
    let mut worker = OuterWorker::new(1, 1).unwrap();

    assert_eq!(worker.build_task(&context).unwrap_err(), "filter failed");
}

#[test]
/// 重复外键只触发一次查找；复用任务时必须清空旧结果并重建映射。
fn inner_worker_deduplicates_lookup_keys_and_rebuilds_map() {
    let context = inner_context(vec![int_row(&[1, 10]), int_row(&[2, 20])]);
    let worker = InnerWorker { context: &context };
    let mut task = LookUpJoinTask::new(vec![int_row(&[2]), int_row(&[1]), int_row(&[1])]);
    worker.handle_task(&mut task, &[0]).unwrap();

    assert!(task.done);
    assert_eq!(task.lookup_contents.len(), 2);
    assert_eq!(task.inner_rows.len(), 2);
    assert_eq!(
        task.lookup_map[&encode_key(&[Value::Int(1)])],
        [int_row(&[1, 10])]
    );

    // 改写同一任务再次执行，确认 lookup 内容和 map 不会残留上一批键 1 的数据。
    task.outer_rows = vec![int_row(&[2])];
    task.outer_match = vec![true];
    worker.handle_task(&mut task, &[0]).unwrap();
    assert_eq!(task.lookup_contents.len(), 1);
    assert_eq!(
        task.lookup_map[&encode_key(&[Value::Int(2)])],
        [int_row(&[2, 20])]
    );
}

#[test]
/// 普通等值连接的 inner NULL key 不应写入 lookup map。
fn inner_worker_skips_null_keys_for_normal_equality() {
    let context = inner_context(vec![vec![Value::Null, Value::Int(10)]]);
    let worker = InnerWorker { context: &context };
    let mut task = LookUpJoinTask::new(vec![int_row(&[1])]);
    task.inner_rows = vec![vec![Value::Null, Value::Int(10)]];

    worker.build_lookup_map(&mut task).unwrap();
    assert!(task.lookup_map.is_empty());
}

#[test]
/// Go Merge 只累加耗时/计数，不改变接收者的 concurrency。
fn runtime_stats_merge_preserves_receiver_concurrency() {
    let mut stats = IndexLookUpJoinRuntimeStats {
        concurrency: 2,
        ..Default::default()
    };
    let other = IndexLookUpJoinRuntimeStats {
        concurrency: 8,
        ..Default::default()
    };

    stats.merge(&other);
    assert_eq!(stats.concurrency, 2);
    assert_eq!(IndexLookUpJoinRuntimeStats::default().to_string(), "");
}

#[test]
/// 多批处理仍按外表顺序输出；无内表匹配的行由左连接默认行补齐。
fn index_lookup_join_preserves_outer_order_and_fills_left_misses() {
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        int_row(&[-1, -1]),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let mut executor = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![int_row(&[1, 100]), int_row(&[2, 200]), int_row(&[1, 101])],
            key_columns: vec![0],
            filters: Vec::new(),
        },
        inner_context(vec![
            int_row(&[1, 10]),
            int_row(&[1, 11]),
            int_row(&[3, 30]),
        ]),
        joiner,
        true,
        1,
        2,
    )
    .unwrap();

    assert_eq!(
        executor.next(8).unwrap(),
        [
            int_row(&[1, 100, 1, 10]),
            int_row(&[1, 100, 1, 11]),
            int_row(&[2, 200, -1, -1]),
            int_row(&[1, 101, 1, 10]),
            int_row(&[1, 101, 1, 11]),
        ]
    );
    assert!(executor.next(1).unwrap().is_empty());
    assert_eq!(executor.stats.inner_worker.tasks, 2);
}

#[test]
/// 外表过滤只跳过索引查找，不会让左连接丢弃该外表行。
fn index_lookup_join_filter_skips_lookup_but_keeps_left_row() {
    let filter = Arc::new(|row: &[Value]| {
        Ok(Some(
            matches!(row.first(), Some(Value::Int(value)) if *value >= 2),
        ))
    });
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        int_row(&[-1, -1]),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let mut executor = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![int_row(&[1, 100]), int_row(&[2, 200])],
            key_columns: vec![0],
            filters: vec![filter],
        },
        inner_context(vec![int_row(&[1, 10]), int_row(&[2, 20])]),
        joiner,
        true,
        8,
        8,
    )
    .unwrap();

    assert_eq!(
        executor.next(8).unwrap(),
        [int_row(&[1, 100, -1, -1]), int_row(&[2, 200, 2, 20])]
    );
}

#[test]
/// Go Close 只释放执行状态，不删除 outer 输入，之后可再次 Open 执行。
fn index_lookup_join_can_reopen_after_close() {
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        int_row(&[-1]),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let mut executor = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![int_row(&[1])],
            key_columns: vec![0],
            filters: Vec::new(),
        },
        inner_context(vec![int_row(&[1, 10])]),
        joiner,
        true,
        1,
        2,
    )
    .unwrap();

    assert_eq!(executor.next(8).unwrap(), [int_row(&[1, 1, 10])]);
    executor.close();
    executor.open().unwrap();
    assert_eq!(executor.next(8).unwrap(), [int_row(&[1, 1, 10])]);
}
