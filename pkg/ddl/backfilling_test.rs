// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// backfilling（回填）模块的单元测试。
//
// “回填”指在执行 DDL（数据定义语言，如 ADD INDEX）时，为表中已有的存量
// 数据补建索引记录的过程。回填通常将表的 key 范围切分成多个子任务，由
// 多个 worker 并发扫描并写入索引。
//
// 本文件覆盖以下内容：
// - `DoneTaskKeeper`：跟踪乱序完成的回填子任务，维护连续完成的最小前缀边界；
// - 分布式回填执行器（`BackfillDistExecutor`）的错误处理与可重试判定；
// - 回填方式选择（事务式 / 本地 ingest / 分布式）；
// - 重组（reorg）阶段的表达式上下文与表写入上下文构造；
// - key 范围的校验、裁剪与按分裂键切分逻辑；
// - 表扫描 worker 的批大小（batch size）动态调整。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backfilling::{
    BackfillTaskContext, DoneTaskKeeper, KvKey, KvKeyRange, merge_warnings_and_counts,
    split_ranges_by_keys, validate_and_fill_ranges,
};
use crate::backfilling_dist_executor::{
    BackfillDistExecutor, BackfillStep, BackfillTaskMeta, ExecutorError,
};
use crate::backfilling_operators::{IndexRecord, TableScanTask, TableScanWorker};
use crate::index::{ReorgType, pick_backfill_type};
use crate::reorg::{SqlMode, new_reorg_expression_context, new_reorg_table_mutate_context};

/// 测试辅助函数：由字节切片构造 KV 存储中的 key。
fn key(bytes: &[u8]) -> KvKey {
    KvKey(bytes.to_vec())
}

/// 测试辅助函数：构造左闭右开的 KV key 范围 [start, end)。
fn range(start: &[u8], end: &[u8]) -> KvKeyRange {
    KvKeyRange {
        StartKey: key(start),
        EndKey: key(end),
    }
}

/// 测试辅助函数：将范围列表转换为 (起始 key, 结束 key) 二元组列表，便于断言比较。
fn range_bounds(ranges: &[KvKeyRange]) -> Vec<(&[u8], &[u8])> {
    ranges
        .iter()
        .map(|range| (range.StartKey.0.as_slice(), range.EndKey.0.as_slice()))
        .collect()
}

#[test]
fn test_merge_warnings_and_counts_matches_go() {
    let mut warnings = [("existing".to_owned(), "first".to_owned())]
        .into_iter()
        .collect();
    let mut warning_counts = [("existing".to_owned(), 2)].into_iter().collect();
    let task = BackfillTaskContext {
        warnings: [
            ("existing".to_owned(), "replacement".to_owned()),
            ("new".to_owned(), "new warning".to_owned()),
        ]
        .into_iter()
        .collect(),
        warning_counts: [
            ("existing".to_owned(), 3),
            ("new".to_owned(), 4),
            ("orphan".to_owned(), 5),
        ]
        .into_iter()
        .collect(),
        ..BackfillTaskContext::default()
    };

    merge_warnings_and_counts(&mut warnings, &mut warning_counts, &task);

    assert_eq!(Some("first"), warnings.get("existing").map(String::as_str));
    assert_eq!(Some("new warning"), warnings.get("new").map(String::as_str));
    assert_eq!(Some(&5), warning_counts.get("existing"));
    assert_eq!(Some(&4), warning_counts.get("new"));
    assert!(!warning_counts.contains_key("orphan"));
}

/// 验证 DoneTaskKeeper 只在任务 ID 连续完成时才推进 next_key。
///
/// 回填子任务可能乱序完成；为了保证断点续传（checkpoint）的正确性，
/// next_key 只能推进到"编号连续的已完成任务"的最大结束 key。
#[test]
fn test_done_task_keeper() {
    let mut keeper = DoneTaskKeeper::new(key(b"a"));
    // 任务 0、1 按顺序完成，next_key 推进到任务 1 的结束 key "c"。
    keeper.update_next_key(0, key(b"b"));
    keeper.update_next_key(1, key(b"c"));
    assert_eq!(b"c", keeper.next_key());

    // 任务 3、4、5 乱序完成，但任务 2 尚未完成，next_key 不能越过缺口。
    keeper.update_next_key(4, key(b"f"));
    keeper.update_next_key(3, key(b"e"));
    keeper.update_next_key(5, key(b"g"));
    assert_eq!(b"c", keeper.next_key());

    // 补上任务 2 后，2..=5 连续完成，next_key 一次性推进到 "g"。
    keeper.update_next_key(2, key(b"d"));
    assert_eq!(b"g", keeper.next_key());
    keeper.update_next_key(6, key(b"h"));
    assert_eq!(b"h", keeper.next_key());
}

/// 验证任务元数据中的索引 ID 找不到对应索引信息时报错，且该错误不可重试。
///
/// 元数据缺失属于确定性错误，重试也不可能成功，因此必须判定为不可重试，
/// 让上层直接失败而不是无限重试。
#[test]
fn test_index_info_not_found_is_non_retryable() {
    let mut executor = BackfillDistExecutor::new(1, vec![1]);
    executor.init(BackfillTaskMeta {
        element_ids: vec![1, 2],
        ..BackfillTaskMeta::default()
    });

    let error = executor
        .get_step_executor(BackfillStep::ReadIndex)
        .expect_err("missing index metadata must fail");
    assert_eq!(ExecutorError::IndexInfoNotFound(2), error);
    assert!(!BackfillDistExecutor::is_retryable_error(&error));
}

/// 验证回填方式的选择逻辑。
///
/// 三种回填方式：
/// - Transactional：走普通事务写入，最慢但最通用；
/// - LocalIngest：本地生成 SST 文件后直接 ingest 到存储层，速度快；
/// - Distributed：借助分布式任务框架在多节点并行回填。
/// 三个入参依次为：是否启用分布式任务、是否启用快速 reorg（ingest）、
/// 是否为临时索引合并阶段。
#[test]
fn test_pick_backfill_type() {
    assert_eq!(
        ReorgType::Transactional,
        pick_backfill_type(false, false, false)
    );
    assert_eq!(
        ReorgType::LocalIngest,
        pick_backfill_type(false, true, false)
    );
    assert_eq!(
        ReorgType::Distributed,
        pick_backfill_type(true, true, false)
    );
    assert_eq!(
        ReorgType::LocalIngest,
        pick_backfill_type(true, true, true),
        "temporary-index merge must not use distributed backfill"
    );
}

/// 验证 reorg（重组）表达式上下文根据 SQL 模式设置告警降级开关。
///
/// 严格模式（Strict）下，截断、非法 NULL、除零等问题作为错误处理；
/// 非严格模式（NonStrict）下则降级为警告，以兼容宽松的数据写入。
/// 同时验证时区偏移（秒）被正确记录。
#[test]
fn test_reorg_expr_context() {
    let strict = new_reorg_expression_context(SqlMode::Strict, 9 * 60 * 60);
    assert!(!strict.truncate_as_warning);
    assert!(!strict.bad_null_as_warning);
    assert!(!strict.division_by_zero_as_warning);
    assert_eq!(9 * 60 * 60, strict.time_zone_offset_seconds);

    let non_strict = new_reorg_expression_context(SqlMode::NonStrict, 0);
    assert!(non_strict.truncate_as_warning);
    assert!(non_strict.bad_null_as_warning);
    assert!(non_strict.division_by_zero_as_warning);
    assert_eq!(0, non_strict.time_zone_offset_seconds);
}

/// 验证 reorg 表写入上下文的默认属性与行编码配置刷新逻辑。
///
/// 回填写入不属于用户会话，因此连接 ID 为 0、不开启事务断言
/// （txn assertion，一种写入前校验 key 存在性的机制）等。
#[test]
fn test_reorg_table_mutate_context() {
    let expression = new_reorg_expression_context(SqlMode::Strict, 8 * 60 * 60);
    let mut context = new_reorg_table_mutate_context(expression.clone());

    // 检查默认值：无连接、非受限 SQL、行 ID 预留耗尽等。
    assert_eq!(&expression, context.expression_context());
    assert_eq!(0, context.connection_id());
    assert!(!context.in_restricted_sql());
    assert!(!context.txn_assertion_enabled());
    assert_eq!(i64::MAX, context.shard_allocate_step());
    assert!(context.reserved_row_id_exhausted());
    assert!(context.mutate_buffers_mut().is_empty());

    // 按行格式版本刷新编码配置：版本 1 关闭新行编码与行级校验和，版本 2 开启。
    context.refresh_row_encoding_config(1);
    assert!(!context.row_encoding_config().row_encoder_enabled);
    assert!(!context.row_encoding_config().row_level_checksum_enabled);
    context.refresh_row_encoding_config(2);
    assert!(context.row_encoding_config().row_encoder_enabled);
    assert!(context.row_encoding_config().row_level_checksum_enabled);
}

/// 验证 validate_and_fill_ranges 对扫描范围列表的校验与裁剪。
///
/// 该函数要求各范围首尾相接（无缺口），并把整体裁剪到请求的
/// [start, end) 区间内；空 key 表示无界（Region 边界的常见表示，
/// Region 是存储层按 key 范围划分的数据分片单元）。
#[test]
fn test_validate_and_fill_ranges() {
    // 范围与请求区间完全一致：保持不变。
    let mut ranges = vec![range(b"b", b"c"), range(b"c", b"d"), range(b"d", b"e")];
    validate_and_fill_ranges(&mut ranges, b"b", b"e").unwrap();
    assert_eq!(
        range_bounds(&[range(b"b", b"c"), range(b"c", b"d"), range(b"d", b"e")]),
        range_bounds(&ranges)
    );

    // 范围超出请求区间：首尾被裁剪到 [b, f)。
    let mut ranges = vec![range(b"a", b"c"), range(b"c", b"e"), range(b"e", b"g")];
    validate_and_fill_ranges(&mut ranges, b"b", b"f").unwrap();
    assert_eq!(
        range_bounds(&[range(b"b", b"c"), range(b"c", b"e"), range(b"e", b"f")]),
        range_bounds(&ranges)
    );

    // 首尾为无界（空 key）：用请求区间的边界填充。
    let mut ranges = vec![range(b"", b"c"), range(b"c", b"e"), range(b"e", b"")];
    validate_and_fill_ranges(&mut ranges, b"b", b"f").unwrap();
    assert_eq!(
        range_bounds(&[range(b"b", b"c"), range(b"c", b"e"), range(b"e", b"f")]),
        range_bounds(&ranges)
    );

    // 仅两个范围且首尾无界：同样按请求区间填充边界。
    let mut ranges = vec![range(b"", b"c"), range(b"c", b"")];
    validate_and_fill_ranges(&mut ranges, b"b", b"f").unwrap();
    assert_eq!(
        range_bounds(&[range(b"b", b"c"), range(b"c", b"f")]),
        range_bounds(&ranges)
    );

    // 非法输入：中间出现无界结束 key，或范围间存在缺口（d 到 e 缺失）。
    let invalid = [
        vec![range(b"b", b"c"), range(b"c", b""), range(b"e", b"f")],
        vec![range(b"b", b"c"), range(b"c", b"d"), range(b"e", b"f")],
    ];
    for mut ranges in invalid {
        assert!(validate_and_fill_ranges(&mut ranges, b"b", b"f").is_err());
    }

    // 首个范围的起点晚于请求起点：请求区间 [a, e) 的开头未被覆盖，报错。
    let mut starts_after_requested = vec![range(b"b", b"c"), range(b"c", b"d"), range(b"d", b"e")];
    assert!(validate_and_fill_ranges(&mut starts_after_requested, b"a", b"e").is_err());

    // 范围整体短于请求区间的尾部：允许，结尾维持原有的 "e"。
    let mut shorter_tail = vec![range(b"b", b"c"), range(b"c", b"d"), range(b"d", b"e")];
    validate_and_fill_ranges(&mut shorter_tail, b"b", b"f").unwrap();
    assert_eq!(b"e", shorter_tail.last().unwrap().EndKey.0.as_slice());
}

/// 验证表扫描 worker 在扫描过程中能动态感知回填批大小的调整。
///
/// 回填批大小（reorg batch size）通过原子变量共享，运维可在线调大
/// 以加速回填；worker 在切分每个输出 chunk 前重新读取该值。
#[test]
fn test_tune_table_scan_worker_batch_size() {
    let batch_size = Arc::new(AtomicUsize::new(32));
    let worker = TableScanWorker {
        chunk_capacity: 32,
        condition_pushed: false,
        reorg_batch_size: Some(Arc::clone(&batch_size)),
    };
    let task = TableScanTask {
        id: 7,
        start: vec![0],
        end: vec![100],
    };
    let rows: Vec<_> = (0_u8..96)
        .map(|ordinal| IndexRecord {
            row_key: vec![ordinal],
            index_key: vec![ordinal],
            value: Vec::new(),
            matches_partial_index: true,
        })
        .collect();

    // 批大小为 32 时，96 行被切成 3 个大小为 32 的 chunk。
    let chunks = worker.scan_records(&task, &rows).unwrap();
    assert_eq!(
        vec![32, 32, 32],
        chunks.iter().map(|c| c.records.len()).collect::<Vec<_>>()
    );

    // 在线把批大小调到 64：首个 chunk 变为 64，剩余 32 行成为第二个 chunk。
    batch_size.store(64, Ordering::Release);
    let chunks = worker.scan_records(&task, &rows).unwrap();
    assert_eq!(
        vec![64, 32],
        chunks.iter().map(|c| c.records.len()).collect::<Vec<_>>()
    );
}

/// 验证 split_ranges_by_keys 按给定分裂键把范围进一步细分。
///
/// 分裂键通常来自存储层的 Region 边界，用于让回填子任务与 Region
/// 对齐，避免单个任务跨越多个 Region。落在范围边界上或范围之外的
/// 分裂键不产生新的切分。
#[test]
fn test_split_ranges_by_keys() {
    // 每个用例为 (名称, 输入范围, 分裂键, 期望输出)。
    let cases = [
        (
            "empty split keys",
            vec![range(&[0], &[10])],
            vec![],
            vec![range(&[0], &[10])],
        ),
        (
            "single split key in middle",
            vec![range(&[0], &[10])],
            vec![key(&[5])],
            vec![range(&[0], &[5]), range(&[5], &[10])],
        ),
        (
            "multiple split keys in one range",
            vec![range(&[0], &[20])],
            vec![key(&[5]), key(&[10]), key(&[15])],
            vec![
                range(&[0], &[5]),
                range(&[5], &[10]),
                range(&[10], &[15]),
                range(&[15], &[20]),
            ],
        ),
        (
            "split keys across ranges",
            vec![range(&[0], &[10]), range(&[10], &[20])],
            vec![key(&[5]), key(&[15])],
            vec![
                range(&[0], &[5]),
                range(&[5], &[10]),
                range(&[10], &[15]),
                range(&[15], &[20]),
            ],
        ),
        (
            "keys on and outside boundaries",
            vec![range(&[5], &[10])],
            vec![key(&[3]), key(&[5]), key(&[10]), key(&[15])],
            vec![range(&[5], &[10])],
        ),
        (
            "boundary keys with one inner key",
            vec![range(&[0], &[10])],
            vec![key(&[0]), key(&[5]), key(&[10])],
            vec![range(&[0], &[5]), range(&[5], &[10])],
        ),
    ];

    for (name, ranges, split_keys, expected) in cases {
        let actual = split_ranges_by_keys(&ranges, &split_keys);
        assert_eq!(range_bounds(&expected), range_bounds(&actual), "{name}");
    }
}
