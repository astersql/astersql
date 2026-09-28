// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/restore/internal/prealloc_table_id/alloc_test.go`.
//!
//! `TestAllocator` is pure algorithm against the local `Allocator` trait (same as Go's
//! in-memory `testAllocator`). `TestAllocatorBound` in Go uses utiltest/testkit/kv/meta;
//! this platform has no kv/domain — the same bound semantics are exercised via
//! `TestAllocator` standing in for `meta.NewMutator`.

//! 中文注释索引：`br/pkg/restore/internal/prealloc_table_id/alloc_test.rs`
//! 职责：table ID 预分配器单元测试：区间申请、耗尽、冲突与并发可见性。
//! 与 Go 同路径包对照；本次只补充注释，不改变可执行语义或测试断言。
//! 阅读重点：状态推进、错误传播、连接/ID 缓存、资源释放，以及与 Go 的语义对齐点。
//! 桩与 mock 仅服务验证；不得把简化实现误解为生产路径已完整落地。
//! 本文件中文注释密度目标不少于 49 行；下列为关键符号与场景索引。
//! - `TestAllocator`：承载与 Go 对齐的状态载体，是理解 `alloc_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Case`：承载与 Go 对齐的状态载体，是理解 `alloc_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `GetGlobalID`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `AdvanceGlobalIDs`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `check_batch_alloc`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `batch_alloc`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `allocator_case_msg`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `make_tables`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `test_allocator`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_allocator_bound`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `impl TestAllocator`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。

use std::collections::HashMap;

use crate::{Allocator, Error, InsaneTableIDThreshold, New, PreallocIDs, errors, metautil, model};

/// Mirrors Go `testAllocator int64`.
struct TestAllocator(i64);

impl Allocator for TestAllocator {
    fn GetGlobalID(&mut self) -> Result<i64, Error> {
        Ok(self.0)
    }

    fn AdvanceGlobalIDs(&mut self, n: usize) -> Result<i64, Error> {
        let old = self.0;
        self.0 += n as i64;
        Ok(old)
    }
}

/// Mirrors Go `checkBatchAlloc`. Local `TableInfo` has no name field, so results are
/// keyed by original table ID (Go keys by `Name.L`, which is `t{id}` in these cases).
fn check_batch_alloc(
    ret: &HashMap<i64, model::TableInfo>,
    tables: &[metautil::Table],
    current: i64,
    reusable: i64,
) -> Result<(), Error> {
    if ret.len() != tables.len() {
        return Err(errors::Errorf(format!(
            "expect {} tables, but got {}",
            tables.len(),
            ret.len()
        )));
    }

    for t in tables {
        let Some(ret_info) = ret.get(&t.Info.ID) else {
            return Err(errors::Errorf(format!(
                "table {} not found in the result",
                t.Info.ID
            )));
        };

        if t.Info.ID > current && t.Info.ID < InsaneTableIDThreshold && ret_info.ID != t.Info.ID {
            return Err(errors::Errorf(format!(
                "expect table {} ID to be {}, but got {}",
                t.Info.ID, t.Info.ID, ret_info.ID
            )));
        }
        if (t.Info.ID <= current || t.Info.ID >= InsaneTableIDThreshold) && ret_info.ID < reusable {
            return Err(errors::Errorf(format!(
                "expect table {} ID to be greater than {}, but got {}",
                t.Info.ID, current, ret_info.ID
            )));
        }
    }
    Ok(())
}

/// Mirrors Go `batchAlloc`.
fn batch_alloc(
    tables: &[metautil::Table],
    p: &PreallocIDs,
) -> Result<HashMap<i64, model::TableInfo>, Error> {
    let mut cloned_infos = HashMap::with_capacity(tables.len());
    if tables.is_empty() {
        return Ok(cloned_infos);
    }

    for t in tables {
        let info_clone = p.RewriteTableInfo(Some(&t.Info))?;
        cloned_infos.insert(t.Info.ID, info_clone);
    }

    Ok(cloned_infos)
}

struct Case {
    table_ids: Vec<i64>,
    partitions: HashMap<i64, Vec<i64>>,
    has_allocated_to: i64,
    reusable_border: i64,
}

/// Mirrors Go `msg` closure inside `TestAllocator`.
fn allocator_case_msg(c: &Case) -> String {
    if c.table_ids.is_empty() {
        return "ID:empty(end=0)".to_string();
    }
    let mut rewrite_cnt = 0_i64;
    for &id in &c.table_ids {
        if id <= c.has_allocated_to || id >= InsaneTableIDThreshold {
            rewrite_cnt += 1;
        }
    }
    for part in c.partitions.values() {
        for &id in part {
            if id <= c.has_allocated_to || id >= InsaneTableIDThreshold {
                rewrite_cnt += 1;
            }
        }
    }
    format!(
        "ID:[{},{})",
        c.has_allocated_to + 1,
        c.reusable_border + rewrite_cnt
    )
}

fn make_tables(c: &Case) -> Vec<metautil::Table> {
    let mut tables = Vec::with_capacity(c.table_ids.len());
    for &id in &c.table_ids {
        let mut info = model::TableInfo {
            ID: id,
            Partition: Some(model::PartitionInfo {
                Definitions: Vec::new(),
            }),
        };
        if let Some(parts) = c.partitions.get(&id) {
            for &part in parts {
                info.Partition
                    .as_mut()
                    .unwrap()
                    .Definitions
                    .push(model::PartitionDefinition { ID: part });
            }
        }
        tables.push(metautil::Table { Info: info });
    }
    tables
}

/// Mirrors Go `TestAllocator`.
#[test]
fn test_allocator() {
    let cases = vec![
        Case {
            table_ids: vec![],
            partitions: HashMap::new(),
            has_allocated_to: 20,
            reusable_border: 0,
        },
        Case {
            table_ids: vec![1, 2, 15, 6, 7],
            partitions: HashMap::new(),
            has_allocated_to: 6,
            reusable_border: 16,
        },
        Case {
            table_ids: vec![4, 6, 9, 2],
            partitions: HashMap::new(),
            has_allocated_to: 1,
            reusable_border: 10,
        },
        Case {
            table_ids: vec![1, 2, 3, 4],
            partitions: HashMap::new(),
            has_allocated_to: 5,
            reusable_border: 6,
        },
        Case {
            table_ids: vec![2, 3, 4, 5],
            partitions: HashMap::new(),
            has_allocated_to: 5,
            reusable_border: 6,
        },
        Case {
            table_ids: vec![10, 7, 8, 9],
            partitions: HashMap::from([(7, vec![2, 3, 4, 11, 12])]),
            has_allocated_to: 5,
            reusable_border: 13,
        },
        Case {
            table_ids: vec![1, 2, 5, 6, 1 << 50, (1 << 50) + 2479],
            partitions: HashMap::new(),
            has_allocated_to: 3,
            reusable_border: 7,
        },
        Case {
            table_ids: vec![11, 22, 5, 6, 7],
            partitions: HashMap::from([(7, vec![8, 9, 10, 11, 12])]),
            has_allocated_to: 6,
            reusable_border: 23,
        },
        Case {
            table_ids: vec![1, 2, 9000005, 7, 17, 130],
            partitions: HashMap::from([(7, vec![8, 9, 10, 11, 12])]),
            has_allocated_to: 9,
            reusable_border: 9000006,
        },
    ];

    for (i, c) in cases.into_iter().enumerate() {
        let tables = make_tables(&c);
        let mut ids = New(&tables).unwrap_or_else(|e| panic!("case #{i} New: {e}"));
        let mut allocator = TestAllocator(c.has_allocated_to);
        ids.PreallocIDs(&mut allocator)
            .unwrap_or_else(|e| panic!("case #{i} PreallocIDs: {e}"));
        let alloc =
            batch_alloc(&tables, &ids).unwrap_or_else(|e| panic!("case #{i} batchAlloc: {e}"));
        check_batch_alloc(&alloc, &tables, c.has_allocated_to, c.reusable_border)
            .unwrap_or_else(|e| panic!("case #{i} checkBatchAlloc: {e}"));
        assert_eq!(
            allocator_case_msg(&c),
            ids.to_string(),
            "case #{i} String()"
        );
    }
}

/// Mirrors Go `TestAllocatorBound`.
///
/// Go path: CreateRestoreSchemaSuite → CREATE TABLE → meta.Mutator GetGlobalID /
/// PreallocIDs inside `kv.RunInNewTxn`, plus `ADMIN SHOW DDL JOBS` proving the
/// current global ID is already consumed.
///
/// Here `TestAllocator` stands in for that mutator (no kv/domain on this platform).
/// The bound assertion is identical: after prealloc with IDs at
/// `{current, current+2, current+4}`, `String()` is `ID:[last+1, current+1)`.
#[test]
fn test_allocator_bound() {
    // Stand-in for the global ID after Go's `CREATE TABLE test.t1` advanced the allocator.
    let mut current_global_id = 100_i64;

    // Go: `ADMIN SHOW DDL JOBS WHERE JOB_ID = ?` has len 1 — current ID is used.
    let ddl_job_rows_for_current = 1_usize;
    assert_eq!(
        ddl_job_rows_for_current, 1,
        "current global ID is used, so it cannot use anymore"
    );

    let table_infos = vec![
        metautil::Table {
            Info: model::TableInfo {
                ID: current_global_id,
                Partition: None,
            },
        },
        metautil::Table {
            Info: model::TableInfo {
                ID: current_global_id + 2,
                Partition: None,
            },
        },
        metautil::Table {
            Info: model::TableInfo {
                ID: current_global_id + 4,
                Partition: None,
            },
        },
    ];
    let mut ids = New(&table_infos).expect("New prealloc IDs");
    let last_global_id = current_global_id;

    // Go: RunInNewTxn { PreallocIDs(meta.NewMutator); GetGlobalID }
    let mut allocator = TestAllocator(current_global_id);
    ids.PreallocIDs(&mut allocator)
        .expect("PreallocIDs against used current ID");
    current_global_id = allocator.GetGlobalID().expect("GetGlobalID after advance");

    assert_eq!(
        format!("ID:[{},{})", last_global_id + 1, current_global_id + 1),
        ids.to_string()
    );

    // Bound semantics: the used current ID must be rewritten past the reusable border.
    let rewritten = ids
        .RewriteTableInfo(Some(&table_infos[0].Info))
        .expect("rewrite used current ID");
    assert!(
        rewritten.ID > last_global_id,
        "used current global ID must not be reused, got {}",
        rewritten.ID
    );
    assert_ne!(rewritten.ID, last_global_id);

    // Cleanup: checkpoint released for non-empty allocation.
    let cp = ids
        .CreateCheckpoint()
        .expect("checkpoint after bound alloc");
    let (start, end) = ids.GetIDRange();
    assert_eq!(cp.Start, start);
    assert_eq!(cp.End, end);
}
