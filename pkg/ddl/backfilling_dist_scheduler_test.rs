// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 分布式回填（backfill）调度器的单元测试。
//
// 回填是 DDL（数据定义语言）中为已有数据补建索引等操作的过程：
// 例如 ADD INDEX 时，需要扫描表中的存量行并为其生成索引记录。
// 分布式回填框架会把整表任务切分为多个子任务（subtask），
// 分发到多个执行节点上并行处理。
//
// 本测试覆盖以下核心逻辑：
// - `LitBackfillScheduler` 的状态机步骤流转（本地模式 / 全局排序模式 /
//   临时索引合并模式下 `get_next_step` 的行为）；
// - 按物理表和 Region（TiKV 中按 key 范围划分的数据分片）生成回填计划；
// - 计算每个子任务应处理的 Region 批次大小；
// - 全局排序（global sort）模式下归并排序计划的生成；
// - 回填任务元数据（`BackfillTaskMeta`）的版本号约定。

use crate::backfilling_dist_executor::{
    BACKFILL_TASK_META_VERSION_0, BACKFILL_TASK_META_VERSION_1, BackfillStep, BackfillTaskMeta,
};
use crate::backfilling_dist_scheduler::{
    LitBackfillScheduler, Modification, MultipleFilesStat, PlanError, RegionMeta,
    calculate_region_batch, generate_merge_sort_plan, generate_plan_for_physical_table,
    generate_temporary_index_plan,
};
use crate::backfilling_read_index::SortedKvMeta;

/// 构造 `count` 个连续的 Region 元数据，用作测试输入。
///
/// 第 i 个 Region 的 key 范围为 `[i, i+1)`，各 Region 首尾相接、互不重叠，
/// 模拟一张表的数据在存储层被切分成多个连续分片的情形。
fn regions(count: u8) -> Vec<RegionMeta> {
    (0..count)
        .map(|index| RegionMeta {
            start_key: vec![index],
            end_key: vec![index + 1],
        })
        .collect()
}

/// 测试本地模式（不使用云端全局排序）下调度器的行为。
///
/// 本地模式指索引数据直接在各节点本地摄入（ingest），
/// 步骤流转为 Init -> ReadIndex -> Done，跳过 MergeSort 与 WriteAndIngest。
#[test]
fn test_backfilling_scheduler_local_mode() {
    // 默认元数据中 cloud_storage_uri 为空，即本地模式。
    let scheduler = LitBackfillScheduler::new(BackfillTaskMeta::default());
    assert_eq!(
        BackfillStep::ReadIndex,
        scheduler.get_next_step(BackfillStep::Init)
    );
    assert_eq!(
        BackfillStep::Done,
        scheduler.get_next_step(BackfillStep::ReadIndex)
    );

    // 模拟 TS（时间戳，事务系统中用于标识快照版本的逻辑时间）分配器：
    // 每次调用返回递增的时间戳。
    let mut next_ts = 100;
    let mut allocate = || {
        next_ts += 1;
        next_ts
    };
    // 为 4 张物理表（分区表的每个分区对应一个物理表 ID）分别生成回填计划，
    // 并把所有子任务汇总到一起。
    let mut subtasks = Vec::new();
    for physical_table_id in [11_i64, 12, 13, 14] {
        let mut plan = generate_plan_for_physical_table(
            physical_table_id,
            &[0],
            &[2],
            regions(2),
            1,
            false,
            &mut allocate,
        )
        .unwrap();
        subtasks.append(&mut plan);
    }
    // 每张表 2 个 Region、批次大小允许合并为 1 个子任务，共 4 个子任务。
    assert_eq!(4, subtasks.len());
    assert_eq!(
        vec![11, 12, 13, 14],
        subtasks
            .iter()
            .map(|meta| meta.physical_table_id)
            .collect::<Vec<_>>()
    );
    // 空 key 范围且没有任何 Region 时，应生成空计划而不是报错。
    assert!(
        generate_plan_for_physical_table(20, b"", b"", Vec::new(), 1, false, &mut allocate)
            .unwrap()
            .is_empty()
    );
}

/// 测试单个子任务应处理的 Region 批次大小计算。
///
/// 输入为 (Region 总数, 可用节点数, 是否本地摄入模式, 期望批次大小)。
/// 非本地模式下批次有上限以控制子任务粒度；本地模式下倾向于把
/// Region 平均分给各节点（向上取整）。节点数为 0 时应返回 NoNodes 错误。
#[test]
fn test_calculate_region_batch() {
    for (regions, nodes, local, expected) in [
        (100, 8, false, 13),
        (2, 8, false, 1),
        (8, 8, false, 1),
        (100, 8, true, 100),
        (2, 8, true, 2),
        (24, 8, true, 24),
        (1000, 8, true, 334),
        (1000, 2, true, 500),
        (200, 3, true, 100),
    ] {
        assert_eq!(
            expected,
            calculate_region_batch(regions, nodes, local).unwrap()
        );
    }
    // 没有可用执行节点时无法划分批次，返回 NoNodes 错误。
    assert_eq!(
        Err(PlanError::NoNodes),
        calculate_region_batch(10, 0, false)
    );
}

/// 测试全局排序（global sort）模式下调度器的行为。
///
/// 全局排序模式指各节点先把读取到的索引 KV 数据写入云存储并排序，
/// 再统一归并后写回存储层，步骤流转为
/// Init -> ReadIndex -> MergeSort -> WriteAndIngest -> Done。
#[test]
fn test_backfilling_scheduler_global_sort_mode() {
    // 设置了 cloud_storage_uri 即启用全局排序模式。
    let scheduler = LitBackfillScheduler::new(BackfillTaskMeta {
        cloud_storage_uri: "gs://sorted/addindex".to_owned(),
        ..BackfillTaskMeta::default()
    });
    assert_eq!(
        BackfillStep::ReadIndex,
        scheduler.get_next_step(BackfillStep::Init)
    );
    assert_eq!(
        BackfillStep::MergeSort,
        scheduler.get_next_step(BackfillStep::ReadIndex)
    );
    assert_eq!(
        BackfillStep::WriteAndIngest,
        scheduler.get_next_step(BackfillStep::MergeSort)
    );
    assert_eq!(
        BackfillStep::Done,
        scheduler.get_next_step(BackfillStep::WriteAndIngest)
    );

    // 构造 ReadIndex 阶段产出的已排序 KV 文件元信息：
    // key 范围 [ta, tc)，共 4 个文件、总大小 12 字节。
    let meta = SortedKvMeta {
        start_key: b"ta".to_vec(),
        end_key: b"tc".to_vec(),
        file_count: 4,
        total_kv_size: 12,
    };
    // 文件重叠度统计：max_overlap 表示同一 key 上最多有多少个文件重叠，
    // 重叠度高时需要归并排序来减少后续摄入的读放大。
    let stats = vec![vec![MultipleFilesStat {
        max_overlap: 9,
        data_files: vec!["1".into(), "2".into(), "3".into(), "4".into()],
    }]];
    // 生成归并排序计划：应产出非空子任务，且每个子任务都关联索引元素 ID 10
    //（element_id 标识回填的目标索引）。
    let plan = generate_merge_sort_plan(&[meta], &stats, &[10], 1, 4).unwrap();
    assert!(!plan.is_empty());
    assert!(plan.iter().all(|subtask| subtask.element_ids == vec![10]));
}

/// 汇总测试三种模式下 `get_next_step` 的状态机流转：
/// 本地模式、全局排序模式、临时索引合并模式。
#[test]
fn test_get_next_step() {
    // 本地模式：Init -> ReadIndex -> Done。
    let local = LitBackfillScheduler::new(BackfillTaskMeta::default());
    assert_eq!(
        BackfillStep::ReadIndex,
        local.get_next_step(BackfillStep::Init)
    );
    assert_eq!(
        BackfillStep::Done,
        local.get_next_step(BackfillStep::ReadIndex)
    );

    // 全局排序模式：ReadIndex 之后要经过 MergeSort 与 WriteAndIngest。
    let global = LitBackfillScheduler::new(BackfillTaskMeta {
        cloud_storage_uri: "s3://bucket".into(),
        ..BackfillTaskMeta::default()
    });
    assert_eq!(
        BackfillStep::MergeSort,
        global.get_next_step(BackfillStep::ReadIndex)
    );
    assert_eq!(
        BackfillStep::WriteAndIngest,
        global.get_next_step(BackfillStep::MergeSort)
    );

    // 临时索引合并模式：把 DDL 期间增量写入的临时索引（temporary index）
    // 合并回正式索引，流转为 Init -> MergeTemporaryIndex -> Done。
    let merge = LitBackfillScheduler::new(BackfillTaskMeta {
        merge_temporary_index: true,
        ..BackfillTaskMeta::default()
    });
    assert_eq!(
        BackfillStep::MergeTemporaryIndex,
        merge.get_next_step(BackfillStep::Init)
    );
    assert_eq!(
        BackfillStep::Done,
        merge.get_next_step(BackfillStep::MergeTemporaryIndex)
    );
}

/// 测试回填任务元数据的版本号约定：
/// 默认构造的元数据为版本 0（兼容旧格式），显式指定后为版本 1。
#[test]
fn test_backfill_task_meta_version() {
    let default_meta = BackfillTaskMeta::default();
    assert_eq!(BACKFILL_TASK_META_VERSION_0, default_meta.version);
    let current = BackfillTaskMeta {
        version: BACKFILL_TASK_META_VERSION_1,
        ..BackfillTaskMeta::default()
    };
    assert_eq!(BACKFILL_TASK_META_VERSION_1, current.version);
}

/// Go's merge-temporary-index plan identifies the target through the encoded
/// temporary-index key range and deliberately leaves `EleIDs` unset.
#[test]
fn test_merge_temporary_index_plan_does_not_set_element_ids() {
    let plan = generate_temporary_index_plan(
        42,
        7,
        vec![1],
        vec![3],
        vec![RegionMeta {
            start_key: vec![0],
            end_key: vec![4],
        }],
        1,
    )
    .unwrap();

    assert_eq!(1, plan.len());
    assert_eq!(42, plan[0].physical_table_id);
    assert!(plan[0].element_ids.is_empty());
    assert_eq!(vec![1], plan[0].legacy_sorted_kv_meta.start_key);
    assert_eq!(vec![3], plan[0].legacy_sorted_kv_meta.end_key);
}

/// `DDLReorgMeta.SetBatchSize` stores zero unchanged in Go; zero retains its
/// compatibility meaning of falling back to the configured batch size.
#[test]
fn test_modify_meta_preserves_zero_batch_size() {
    let mut scheduler = LitBackfillScheduler::new(BackfillTaskMeta {
        batch_size: 32,
        ..BackfillTaskMeta::default()
    });

    scheduler.modify_meta(&[Modification::BatchSize(0)]);

    assert_eq!(0, scheduler.task_meta.batch_size);
}

#[test]
fn test_timestamp_retry_discards_partial_plan() {
    use crate::backfilling_dist_scheduler::{
        retry_region_plan, try_generate_plan_for_physical_table,
    };
    let mut scans = 0;
    let mut calls = 0;
    let mut waits = Vec::new();
    let plan = retry_region_plan(
        || {
            scans += 1;
            Ok(regions(200))
        },
        |regions| {
            try_generate_plan_for_physical_table(42, &[0], &[200], regions, 2, false, || {
                calls += 1;
                if calls == 2 {
                    Err(PlanError::TimestampAllocation("transient TSO".into()))
                } else {
                    Ok(calls)
                }
            })
        },
        |delay| {
            waits.push(delay);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(scans, 2);
    assert_eq!(calls, 4);
    assert_eq!(plan.len(), 2);
    assert_eq!(
        (plan[0].physical_table_id, plan[1].physical_table_id),
        (42, 42)
    );
    assert_eq!(
        (&plan[0].row_start, &plan[0].row_end),
        (&vec![0], &vec![100])
    );
    assert_eq!(
        (&plan[1].row_start, &plan[1].row_end),
        (&vec![100], &vec![200])
    );
    assert_eq!((plan[0].ts, plan[1].ts), (3, 4));
    assert_eq!(waits, vec![std::time::Duration::from_millis(200)]);
}

#[test]
fn test_region_discontinuity_reloads_physical_and_temporary_plans() {
    use crate::backfilling_dist_scheduler::retry_region_plan;
    for merging in [false, true] {
        let mut scans = 0;
        let mut allocations = 0;
        let plan = retry_region_plan(
            || {
                scans += 1;
                let mut values = regions(2);
                if scans == 1 {
                    values[1].start_key = vec![9];
                }
                Ok(values)
            },
            |values| {
                if merging {
                    generate_temporary_index_plan(42, 7, vec![0], vec![2], values, 2)
                } else {
                    generate_plan_for_physical_table(42, &[0], &[2], values, 2, true, || {
                        allocations += 1;
                        allocations
                    })
                }
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(scans, 2);
        assert_eq!(plan.len(), 2);
        assert!(
            plan.iter()
                .all(|meta| meta.physical_table_id == 42 && meta.element_ids.is_empty())
        );
        if merging {
            assert_eq!(plan[0].legacy_sorted_kv_meta.start_key, vec![0]);
            assert_eq!(plan[1].legacy_sorted_kv_meta.end_key, vec![2]);
            assert_eq!(allocations, 0);
        } else {
            assert_eq!(plan[0].row_start, vec![0]);
            assert_eq!(plan[1].row_end, vec![2]);
            assert_eq!(allocations, 2);
        }
    }
}

#[test]
fn test_region_plan_retry_exhaustion_preserves_last_error() {
    use crate::backfilling_dist_scheduler::{
        retry_region_plan, try_generate_plan_for_physical_table,
    };
    for merging in [false, true] {
        let mut scans = 0;
        let mut waits = Vec::new();
        let mut broken = regions(2);
        broken[1].start_key = vec![9];
        let error = retry_region_plan(
            || {
                scans += 1;
                Ok(broken.clone())
            },
            |values| {
                if merging {
                    generate_temporary_index_plan(42, 7, vec![0], vec![2], values, 2)
                } else {
                    generate_plan_for_physical_table(42, &[0], &[2], values, 2, true, || {
                        panic!("TSO before continuous scan")
                    })
                }
            },
            |delay| {
                waits.push(delay.as_millis());
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            PlanError::RegionsNotContinuous {
                expected: vec![1],
                actual: vec![9]
            }
        );
        assert_eq!(scans, 8);
        assert_eq!(waits, vec![200, 400, 800, 1600, 2000, 2000, 2000, 2000]);
    }
    let mut calls = 0;
    let error = retry_region_plan(
        || Ok(regions(2)),
        |values| {
            try_generate_plan_for_physical_table(42, &[0], &[2], values, 2, true, || {
                calls += 1;
                Err(PlanError::TimestampAllocation(format!("TSO {calls}")))
            })
        },
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(calls, 8);
    assert_eq!(error, PlanError::TimestampAllocation("TSO 8".into()));
}

#[test]
fn test_region_plan_does_not_retry_scan_or_permanent_plan_errors() {
    use crate::backfilling_dist_scheduler::retry_region_plan;
    let error = retry_region_plan(
        || Err(PlanError::RegionScan("PD unavailable".into())),
        |_| panic!("must not build after scan failure"),
        |_| panic!("scan errors are not retryable"),
    )
    .unwrap_err();
    assert_eq!(error, PlanError::RegionScan("PD unavailable".into()));
    let error = retry_region_plan(
        || Ok(regions(2)),
        |values| {
            generate_plan_for_physical_table(42, &[0], &[2], values, 0, false, || {
                panic!("invalid nodes")
            })
        },
        |_| panic!("permanent plan error is not retryable"),
    )
    .unwrap_err();
    assert_eq!(error, PlanError::NoNodes);
}

#[test]
fn merge_plan_preserves_exact_target_limits() {
    let scheduler = LitBackfillScheduler::new(Default::default());
    let meta = SortedKvMeta::default();
    let stats = vec![vec![MultipleFilesStat {
        max_overlap: 251,
        data_files: (0..4580).map(|i| i.to_string()).collect(),
    }]];
    let plan = generate_merge_sort_plan(&[meta], &stats, &[10], 8, 1).unwrap();
    assert_eq!(plan.len(), 24);
    assert!(plan.iter().all(|p| p.element_ids == vec![10]));
    assert_eq!(
        plan.iter()
            .flat_map(|p| p.data_files.iter().cloned())
            .collect::<Vec<_>>(),
        stats[0][0].data_files
    );
    let stats = vec![vec![MultipleFilesStat {
        max_overlap: 251,
        data_files: vec![String::new(); 62501],
    }]];
    let error =
        generate_merge_sort_plan(&[SortedKvMeta::default()], &stats, &[10], 10, 1).unwrap_err();
    assert!(!scheduler.is_retryable_error(&error));
    assert_eq!(
        error.to_string(),
        "generate merge-sort plan failed: [GlobalSort:TooManyDataFiles]cannot merge 62501 data files with concurrency 1 into at most 250 target files"
    );
}

#[test]
fn merge_limit_error_is_not_retryable() {
    let scheduler = LitBackfillScheduler::new(Default::default());
    #[derive(Debug)]
    struct Wrapped(astersql_ingestor_errdef::NormalizedError);
    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "generate merge-sort plan failed: {}", self.0.Error())
        }
    }
    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let error = Wrapped(astersql_ingestor_errdef::TooManyDataFiles(1000, 1, 250));
    assert!(!scheduler.is_retryable_error(&error));
    assert!(scheduler.is_retryable_error(&std::io::Error::other("temporary scheduler error")));
    assert!(!LitBackfillScheduler::is_retryable_scheduler_message(
        &error.to_string()
    ));
    assert!(LitBackfillScheduler::is_retryable_scheduler_message(
        "temporary scheduler error"
    ));
}
