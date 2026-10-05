// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DDL 回填（backfill）事务执行器的单元测试。
//
// 背景：在数据库中执行 DDL（数据定义语言，如添加索引）时，需要把表中已有
// 的存量数据"回填"到新索引里。回填过程通常由一组读 worker（读取表数据）
// 与写 worker（写入/摄取索引数据，即 ingest）并行完成。
// 本测试验证根据并发度、平均行大小以及是否启用全局排序（global sort，
// 分布式导入时先对索引键值全局排序再写入的模式）计算读/写 worker 数量
// 的策略是否符合预期。

use crate::backfilling::BackfillerType;
use crate::backfilling_txn_executor::{
    ExecutorError, MAX_BACKFILL_WORKER_SIZE, ReorgMeta, SessionContext, TaskIdAllocator,
    TxnBackfillExecutor, expected_ingest_worker_count, new_default_reorg_dist_sql_context,
    new_reorg_dist_sql_context_with_reorg_meta,
};

#[test]
fn reorg_dist_sql_contexts_do_not_fill_tikv_block_cache() {
    use std::sync::Arc;

    use astersql_distsql_context::contextutil::NewStaticWarnHandler;
    use astersql_meta_model::group_3::DDLReorgMeta;

    let default = new_default_reorg_dist_sql_context(Arc::new(NewStaticWarnHandler(0)));
    assert!(default.NotFillCache);

    let metadata = DDLReorgMeta {
        ResourceGroupName: "ddl".to_owned(),
        ..DDLReorgMeta::default()
    };
    let with_metadata =
        new_reorg_dist_sql_context_with_reorg_meta(&metadata, Arc::new(NewStaticWarnHandler(0)));
    assert!(with_metadata.NotFillCache);
    assert_eq!(with_metadata.ResourceGroupName, "ddl");
}

/// 验证不同并发度、行大小、全局排序开关组合下的 worker 数量计算结果。
#[test]
fn test_expected_ingest_worker_count() {
    // 用例格式：(并发度, 平均行大小, 是否全局排序, 期望读 worker 数, 期望写 worker 数)。
    let cases = [
        (10, 100, true, 10, 10),
        (20, 500, true, 20, 20),
        (10, 0, false, 5, 7),
        (40, 0, false, 16, 16),
        (1, 0, false, 1, 2),
        (10, 100, false, 5, 10),
        (10, 300, false, 10, 10),
        (10, 600, false, 20, 10),
        (10, 2_000, false, 40, 10),
        (10, 5_000, false, 80, 10),
        // Go 保留全局排序的零并发配置，不在这个辅助函数中擅自修正。
        (0, 100, true, 0, 0),
        // 有统计信息时仅 txn worker 池受 16 的上限约束，ingest reader 不截断。
        (40, 5_000, false, 320, 40),
    ];

    // 逐个用例断言计算结果与期望一致，断言消息中带上输入参数便于定位失败用例。
    for (concurrency, row_size, global_sort, expected_reader, expected_writer) in cases {
        assert_eq!(
            expected_ingest_worker_count(concurrency, row_size, global_sort),
            (expected_reader, expected_writer),
            "concurrency={concurrency}, row_size={row_size}, global_sort={global_sort}"
        );
    }
}

#[test]
fn worker_resize_matches_go_bounds_and_preserves_slot_order() {
    let mut executor = TxnBackfillExecutor::new(BackfillerType::AddIndex);

    executor.setup_workers(3).unwrap();
    assert_eq!(executor.worker_count(), 3);

    executor.adjust_worker_size(MAX_BACKFILL_WORKER_SIZE + 10);
    assert_eq!(executor.worker_count(), MAX_BACKFILL_WORKER_SIZE);

    // Go's adjustWorkerSize permits the configured worker count to fall to zero.
    executor.adjust_worker_size(0);
    assert_eq!(executor.worker_count(), 0);
    assert_eq!(
        executor.setup_workers(0),
        Err(ExecutorError::InvalidConcurrency)
    );
}

#[test]
fn session_restore_and_task_ids_preserve_state() {
    let original = SessionContext {
        strict_sql_mode: false,
        time_zone: "Asia/Shanghai".to_owned(),
        resource_group_name: "interactive".to_owned(),
        batch_size: 32,
    };
    let mut session = original.clone();
    let snapshot = session.initialize_for_reorganization(&ReorgMeta {
        strict_sql_mode: true,
        time_zone: "UTC".to_owned(),
        resource_group_name: "ddl".to_owned(),
        batch_size: 0,
        ..ReorgMeta::default()
    });
    assert!(session.strict_sql_mode);
    assert_eq!(session.time_zone, "UTC");
    assert_eq!(session.resource_group_name, "ddl");
    assert_eq!(session.batch_size, 1);
    session.restore(snapshot);
    assert_eq!(session, original);

    let mut allocator = TaskIdAllocator::new();
    assert_eq!(
        (allocator.alloc(), allocator.alloc(), allocator.alloc()),
        (0, 1, 2)
    );
}
