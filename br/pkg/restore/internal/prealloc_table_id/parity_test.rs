// Copyright 2026 AsterSQL.

//! 与 Go `alloc.go` / `TestAllocator` 对齐的 prealloc_table_id 契约测试。
//! 覆盖：正常预分配、边界（空表/超大 ID/哈希）、错误路径与 checkpoint 复用。
//! 不改断言语义；仅验证 Rust 侧与 Go 的 ID 区间、重写规则和错误文案一致。
//! 测试夹具本地实现 Allocator，避免依赖真实 PD / meta 全局 ID 分配器。
//! 场景划分：正常用例矩阵、边界常量/哈希、错误包装与 checkpoint 复用。

use std::collections::HashMap;

use crate::alloc::{
    Allocator, InsaneTableIDThreshold, New, NewAndPrealloc, PreallocIDs, ReuseCheckpoint,
    compute_sorted_ids_hash_for_test, metautil, model, prealloc_ids_already_allocated_for_test,
};

// 可预测的全局 ID 桩：GetGlobalID 返回当前水位，Advance 按 n 推进。
// 对应 Go 测试里用固定 hasAllocatedTo 驱动 PreallocIDs 的写法。
// Advance 返回推进前旧值，与 TiDB meta.AdvanceGlobalIDs 契约一致。
struct TestAllocator(i64);

impl Allocator for TestAllocator {
    fn GetGlobalID(&mut self) -> Result<i64, crate::alloc::Error> {
        Ok(self.0)
    }

    fn AdvanceGlobalIDs(&mut self, n: usize) -> Result<i64, crate::alloc::Error> {
        let old = self.0;
        self.0 += n as i64;
        Ok(old)
    }
}

// 错误注入桩：分别模拟 GetGlobalID / AdvanceGlobalIDs 失败，校验错误包装链路。
// get_err / advance_err 互斥使用，对应 Go 测试中的失败分支。
struct FailAllocator {
    get_err: bool,
    advance_err: bool,
}

impl Allocator for FailAllocator {
    fn GetGlobalID(&mut self) -> Result<i64, crate::alloc::Error> {
        if self.get_err {
            return Err(crate::alloc::errors::Errorf("get global id failed"));
        }
        Ok(10)
    }

    fn AdvanceGlobalIDs(&mut self, _n: usize) -> Result<i64, crate::alloc::Error> {
        if self.advance_err {
            return Err(crate::alloc::errors::Errorf("advance global ids failed"));
        }
        Ok(10)
    }
}

// 按表 ID 与可选分区定义构造 metautil::Table，供 New/PreallocIDs 收集 ID。
// 分区 ID 会并入 rewrite 计数，与 Go collectIDs 语义对齐。
// 未给出分区时仍挂空 PartitionInfo，避免 Rewrite 路径对 None 的特殊分支。
fn make_tables(table_ids: &[i64], partitions: &HashMap<i64, Vec<i64>>) -> Vec<metautil::Table> {
    let mut tables = Vec::with_capacity(table_ids.len());
    for &id in table_ids {
        let mut info = model::TableInfo {
            ID: id,
            Partition: Some(model::PartitionInfo {
                Definitions: Vec::new(),
            }),
        };
        if let Some(parts) = partitions.get(&id) {
            info.Partition = Some(model::PartitionInfo {
                Definitions: parts
                    .iter()
                    .map(|&pid| model::PartitionDefinition { ID: pid })
                    .collect(),
            });
        }
        tables.push(metautil::Table { Info: info });
    }
    tables
}

// 复现 Go String() 期望：区间为 [hasAllocatedTo+1, reusableBorder+rewriteCnt)。
// rewriteCnt 统计需重写的表/分区 ID（<= 已分配水位或 >= Insane 阈值）。
// 空表集固定输出 ID:empty(end=0)，与 Go 空 PreallocIDs 一致。
fn expected_msg(
    table_ids: &[i64],
    partitions: &HashMap<i64, Vec<i64>>,
    has_allocated_to: i64,
    reusable_border: i64,
) -> String {
    if table_ids.is_empty() {
        return "ID:empty(end=0)".to_string();
    }
    let mut rewrite_cnt = 0_i64;
    for &id in table_ids {
        // 冲突或异常大 ID：必须重写到可复用边界之后。
        if id <= has_allocated_to || id >= InsaneTableIDThreshold {
            rewrite_cnt += 1;
        }
    }
    for parts in partitions.values() {
        for &id in parts {
            if id <= has_allocated_to || id >= InsaneTableIDThreshold {
                rewrite_cnt += 1;
            }
        }
    }
    format!(
        "ID:[{},{})",
        has_allocated_to + 1,
        reusable_border + rewrite_cnt
    )
}

// 校验 RewriteTableInfo 批量结果：可复用 ID 保持不变，冲突 ID 落到 reusable 之后。
// current 即 hasAllocatedTo；与 Go TestAllocator 中的检查逻辑同构。
fn check_batch_alloc(
    ret: &HashMap<i64, model::TableInfo>,
    tables: &[metautil::Table],
    current: i64,
    reusable: i64,
) -> Result<(), String> {
    if ret.len() != tables.len() {
        return Err(format!(
            "expect {} tables, but got {}",
            tables.len(),
            ret.len()
        ));
    }
    for t in tables {
        let Some(ret_info) = ret.get(&t.Info.ID) else {
            return Err(format!("table id {} not found in the result", t.Info.ID));
        };
        // 落在 (current, Insane) 内的 ID 应原样保留，不得被错误重写。
        if t.Info.ID > current && t.Info.ID < InsaneTableIDThreshold && ret_info.ID != t.Info.ID {
            return Err(format!(
                "expect table ID to be {}, but got {}",
                t.Info.ID, ret_info.ID
            ));
        }
        // 冲突/超大 ID 重写后必须 >= reusable，否则说明预分配区间不足。
        if (t.Info.ID <= current || t.Info.ID >= InsaneTableIDThreshold) && ret_info.ID < reusable {
            return Err(format!(
                "expect table ID to be greater than {}, but got {}",
                current, ret_info.ID
            ));
        }
    }
    Ok(())
}

// 对每张表调用 RewriteTableInfo，键仍用备份侧原始 ID，便于与输入对照。
// 返回 map 长度必须等于 tables，供 check_batch_alloc 做一一校验。
fn batch_alloc(
    tables: &[metautil::Table],
    p: &PreallocIDs,
) -> Result<HashMap<i64, model::TableInfo>, crate::alloc::Error> {
    let mut cloned = HashMap::with_capacity(tables.len());
    for t in tables {
        let info_clone = p.RewriteTableInfo(Some(&t.Info))?;
        cloned.insert(t.Info.ID, info_clone);
    }
    Ok(cloned)
}

#[test]
fn go_rust_public_contract_matches() {
    // --- 正常路径：对齐 Go TestAllocator 的表/分区用例矩阵 ---
    // 每案字段：输入表 ID、分区映射、全局已分配水位、期望可复用边界。
    struct Case {
        table_ids: Vec<i64>,
        partitions: HashMap<i64, Vec<i64>>,
        has_allocated_to: i64,
        reusable_border: i64,
    }

    let cases = vec![
        // 空表：不预占区间，checkpoint 应为 None。
        Case {
            table_ids: vec![],
            partitions: HashMap::new(),
            has_allocated_to: 20,
            reusable_border: 0,
        },
        // 部分 ID <= 水位：需重写；15 可复用，reusable_border=16。
        Case {
            table_ids: vec![1, 2, 15, 6, 7],
            partitions: HashMap::new(),
            has_allocated_to: 6,
            reusable_border: 16,
        },
        // 水位很低：多数 ID 可复用，区间上界取 maxID+1。
        Case {
            table_ids: vec![4, 6, 9, 2],
            partitions: HashMap::new(),
            has_allocated_to: 1,
            reusable_border: 10,
        },
        // 全部 ID 均 <= 水位：全部重写，border 紧贴 max+1。
        Case {
            table_ids: vec![1, 2, 3, 4],
            partitions: HashMap::new(),
            has_allocated_to: 5,
            reusable_border: 6,
        },
        // 边界相等：ID==水位也视为冲突，需重写。
        Case {
            table_ids: vec![2, 3, 4, 5],
            partitions: HashMap::new(),
            has_allocated_to: 5,
            reusable_border: 6,
        },
        // 含分区：分区 ID 一并计入收集与重写。
        Case {
            table_ids: vec![10, 7, 8, 9],
            partitions: HashMap::from([(7, vec![2, 3, 4, 11, 12])]),
            has_allocated_to: 5,
            reusable_border: 13,
        },
        // 超大 ID (>= 1<<50) 视为 insane，强制重写，不抬高 reusable 边界。
        Case {
            table_ids: vec![1, 2, 5, 6, 1 << 50, (1 << 50) + 2479],
            partitions: HashMap::new(),
            has_allocated_to: 3,
            reusable_border: 7,
        },
        // 分区与表 ID 交错：reusable 取全局 max+1。
        Case {
            table_ids: vec![11, 22, 5, 6, 7],
            partitions: HashMap::from([(7, vec![8, 9, 10, 11, 12])]),
            has_allocated_to: 6,
            reusable_border: 23,
        },
        // 极大但仍 < Insane 的表 ID 会抬高 reusable_border。
        Case {
            table_ids: vec![1, 2, 9000005, 7, 17, 130],
            partitions: HashMap::from([(7, vec![8, 9, 10, 11, 12])]),
            has_allocated_to: 9,
            reusable_border: 9000006,
        },
    ];

    for (i, c) in cases.into_iter().enumerate() {
        // New → PreallocIDs → Rewrite → String/Checkpoint，逐步对齐 Go 用例。
        let tables = make_tables(&c.table_ids, &c.partitions);
        let mut ids = New(&tables).unwrap_or_else(|e| panic!("case #{i} New: {e}"));
        let mut allocator = TestAllocator(c.has_allocated_to);
        ids.PreallocIDs(&mut allocator)
            .unwrap_or_else(|e| panic!("case #{i} PreallocIDs: {e}"));
        let alloc = batch_alloc(&tables, &ids).unwrap_or_else(|e| panic!("case #{i} batch: {e}"));
        check_batch_alloc(&alloc, &tables, c.has_allocated_to, c.reusable_border)
            .unwrap_or_else(|e| panic!("case #{i} check: {e}"));
        let expect = expected_msg(
            &c.table_ids,
            &c.partitions,
            c.has_allocated_to,
            c.reusable_border,
        );
        // String() 文案必须与手工推算的区间一致。
        assert_eq!(ids.to_string(), expect, "case #{i} String");

        // 资源收尾：空集无 checkpoint；非空则 Start/End/Hash 与区间一致。
        if c.table_ids.is_empty() {
            assert!(ids.CreateCheckpoint().is_none());
            let (start, end) = ids.GetIDRange();
            // 空区间哨兵：start=MAX、end=0，避免误用为合法全局 ID。
            assert_eq!(start, i64::MAX);
            assert_eq!(end, 0);
        } else {
            let cp = ids.CreateCheckpoint().expect("checkpoint");
            let (start, end) = ids.GetIDRange();
            assert_eq!(cp.Start, start);
            assert_eq!(cp.End, end);
            assert!(cp.ReusableBorder > 0);
            // Hash 用于 ReuseCheckpoint 防篡改，不可为全零。
            assert_ne!(cp.Hash, [0u8; 32]);
        }
    }

    // NewAndPrealloc：构造与预分配一步完成，水位=1 时期望 ID:[2,10)。
    let tables = make_tables(&[4, 6, 9, 2], &HashMap::new());
    let mut allocator = TestAllocator(1);
    let p = NewAndPrealloc(&tables, &mut allocator).expect("NewAndPrealloc");
    assert_eq!(p.to_string(), "ID:[2,10)");

    // --- 边界：空输入、阈值常量、排序哈希、ID 空间溢出 ---
    let empty = New(&[]).expect("empty New");
    assert_eq!(empty.to_string(), "ID:empty(end=0)");
    assert!(empty.CreateCheckpoint().is_none());

    // 空表 NewAndPrealloc 同样走 empty 分支，不调用 Advance。
    let empty2 = NewAndPrealloc(&[], &mut TestAllocator(0)).expect("empty NewAndPrealloc");
    assert_eq!(empty2.to_string(), "ID:empty(end=0)");

    // 与 Go InsaneTableIDThreshold 常量对齐。
    assert_eq!(InsaneTableIDThreshold, u32::MAX as i64);

    // 哈希稳定性：同一输入序列两次调用结果相同。
    let hash_a = compute_sorted_ids_hash_for_test(&[1, 2, 3]);
    let hash_b = compute_sorted_ids_hash_for_test(&[1, 2, 3]);
    let hash_c = compute_sorted_ids_hash_for_test(&[3, 2, 1]);
    assert_eq!(hash_a, hash_b);
    // 不同顺序入参产生不同摘要（测试辅助按字节序列哈希，非集合语义）。
    assert_ne!(hash_a, hash_c);

    // maxID + len(ids) + 1 越过 Insane 阈值时 New 必须失败（too large）。
    let huge = make_tables(&[InsaneTableIDThreshold - 1], &HashMap::new());
    let err = New(&huge).expect_err("too large");
    assert!(err.msg.contains("too large"), "unexpected err: {}", err.msg);

    // --- 错误路径：未预分配、重复分配、PD 失败、非法参数 ---
    let tables = make_tables(&[1, 2, 3], &HashMap::new());
    let pending = New(&tables).expect("New");
    // 尚未 PreallocIDs 时 AllocID 应拒绝。
    let err = pending.AllocID(1).expect_err("not allocated");
    assert!(err.msg.contains("not allocated yet"));

    // 成功后 unallocedIDs 清空：第二次 PreallocIDs 应为 no-op（Go len==0）。
    let mut ids = New(&tables).expect("New");
    ids.PreallocIDs(&mut TestAllocator(0)).expect("first");
    ids.PreallocIDs(&mut TestAllocator(0))
        .expect("second no-op");

    // start < end 且仍挂着 unalloced：视为已分配过，禁止再次分配。
    let mut once = prealloc_ids_already_allocated_for_test(vec![1, 2, 3]);
    let err = once
        .PreallocIDs(&mut TestAllocator(0))
        .expect_err("allocated once");
    assert!(err.msg.contains("only be allocated once"));

    // NewAndPrealloc 在 Get 失败时包装为 failed to allocate prealloc IDs。
    let err = NewAndPrealloc(
        &tables,
        &mut FailAllocator {
            get_err: true,
            advance_err: false,
        },
    )
    .expect_err("wrap get");
    assert!(err.msg.contains("failed to allocate prealloc IDs"));

    // PreallocIDs 直接暴露底层 get 错误文案。
    let mut ids = New(&tables).expect("New");
    let err = ids
        .PreallocIDs(&mut FailAllocator {
            get_err: true,
            advance_err: false,
        })
        .expect_err("get fail");
    assert!(err.msg.contains("get global id failed"));

    // Advance 失败路径独立覆盖。
    let mut ids = New(&tables).expect("New");
    let err = ids
        .PreallocIDs(&mut FailAllocator {
            get_err: false,
            advance_err: true,
        })
        .expect_err("advance fail");
    assert!(err.msg.contains("advance global ids failed"));

    let mut ids = New(&tables).expect("New");
    ids.PreallocIDs(&mut TestAllocator(0)).expect("alloc");
    // nil TableInfo 与 Go 一致报 table info is nil。
    let err = ids.RewriteTableInfo(None).expect_err("nil info");
    assert!(err.msg.contains("table info is nil"));

    // 不在预分配映射中的 ID：not in range。
    let err = ids.AllocID(99999).expect_err("missing rule");
    assert!(err.msg.contains("not in range"));

    // ReuseCheckpoint：nil / 哈希篡改 / border 非法 / 成功复用。
    let err = ReuseCheckpoint(None, &tables).expect_err("nil legacy");
    assert!(err.msg.contains("no prealloc IDs to be reused"));

    let mut ids = New(&tables).expect("New");
    ids.PreallocIDs(&mut TestAllocator(0)).expect("alloc");
    let cp = ids.CreateCheckpoint().expect("cp");

    // 成功复用：区间与 AllocID 映射应与原对象一致。
    let reused = ReuseCheckpoint(Some(&cp), &tables).expect("reuse");
    assert_eq!(reused.GetIDRange(), ids.GetIDRange());
    assert_eq!(reused.AllocID(1).unwrap(), ids.AllocID(1).unwrap());
    // 复用后无 pending unalloced；再 CreateCheckpoint 哈希应 round-trip。
    let cp2 = reused.CreateCheckpoint().expect("cp2");
    assert_eq!(cp2.Hash, cp.Hash);

    // 哈希被改 → ErrInvalidRange。
    let mut bad_hash = cp.clone();
    bad_hash.Hash[0] ^= 0xff;
    let err = ReuseCheckpoint(Some(&bad_hash), &tables).expect_err("hash");
    assert!(err.msg.contains("hash mismatch"));
    assert_eq!(err.code, Some("BR:Common:ErrInvalidRange"));

    // ReusableBorder=0 非法。
    let mut bad_border = cp.clone();
    bad_border.ReusableBorder = 0;
    let err = ReuseCheckpoint(Some(&bad_border), &tables).expect_err("border");
    assert!(err.msg.contains("reusable border"));
    assert_eq!(err.code, Some("BR:Common:ErrInvalidRange"));

    // 同表集难以构造 id>=ReusableBorder 的 else 分支；改为覆盖分区重写。
    // 断言：RewriteTableInfo 会改写表 ID 并保留分区定义条数。
    let parts = HashMap::from([(7, vec![2, 3])]);
    let tables = make_tables(&[7, 10], &parts);
    let mut ids = New(&tables).expect("New");
    ids.PreallocIDs(&mut TestAllocator(5)).expect("alloc");
    let rewritten = ids
        .RewriteTableInfo(Some(&tables[0].Info))
        .expect("rewrite");
    assert_ne!(rewritten.ID, 0);
    assert_eq!(rewritten.Partition.as_ref().unwrap().Definitions.len(), 2);

    // NewAndPrealloc 在 New 失败时包装 failed to create preallocIDs。
    let huge = make_tables(&[InsaneTableIDThreshold - 1], &HashMap::new());
    let err = NewAndPrealloc(&huge, &mut TestAllocator(0)).expect_err("create wrap");
    assert!(err.msg.contains("failed to create preallocIDs"));
}
