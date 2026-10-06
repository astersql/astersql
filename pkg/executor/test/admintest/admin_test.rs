// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// ADMIN CHECK / RECOVER / CLEANUP INDEX 测试。
//
// 可执行测试用内存 [`AdminTable`] 覆盖核心一致性修复语义。

use crate::{AdminSessionVars, AdminTable, Inconsistency};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows, TestKit};

struct RestoreOnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> RestoreOnDrop<F> {
    fn new(restore: F) -> Self {
        Self(Some(restore))
    }
}

impl<F: FnOnce()> Drop for RestoreOnDrop<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            restore();
        }
    }
}

fn admin_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk
}

/// Go 多值索引场景的真实成功路径：JSON 数组索引和普通 ADMIN CHECK
/// 必须共享同一会话配置与元数据。
#[test]
fn admin_check_accepts_consistent_multi_valued_index() {
    let _restore = RestoreOnDrop::new(astersql_config::restore_func());
    astersql_config::update_global(|conf| {
        conf.experimental.allows_expression_index = true;
    });
    let mut tk = admin_testkit();
    tk.MustExec("drop table if exists admin_multi", Vec::new());
    tk.MustExec(
        "create table admin_multi (id int primary key, payload json, index idx_payload((cast(payload as signed array))))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into admin_multi values (0, '[0,1,2]'), (1, '[1,2,3]'), (2, '[2,3,4]'), (3, '[3,4,5]'), (4, '[4,5,6]')",
        Vec::new(),
    );
    tk.MustExec("admin check table admin_multi", Vec::new());
    tk.MustExec("admin check index admin_multi idx_payload", Vec::new());
}

/// ADMIN RECOVER/CLEANUP INDEX must execute through ConcreteSession and return
/// the same no-corruption counts as the Go testkit path.
#[test]
fn admin_index_maintenance_executes_through_real_sql() {
    let mut tk = admin_testkit();
    tk.MustExec("drop table if exists admin_maintenance", Vec::new());
    tk.MustExec(
        "create table admin_maintenance (id int primary key, value int, index idx_value(value))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into admin_maintenance values (1, 10), (2, 20), (3, 30)",
        Vec::new(),
    );
    tk.MustQuery(
        "admin recover index admin_maintenance idx_value",
        Vec::new(),
    )
    .Check(Rows(&["0 3"]));
    tk.MustQuery(
        "admin cleanup index admin_maintenance idx_value",
        Vec::new(),
    )
    .Check(Rows(&["0"]));
    tk.MustQuery(
        "admin recover index admin_maintenance `primary`",
        Vec::new(),
    )
    .Check(Rows(&["0 0"]));
    tk.MustExec("admin check table admin_maintenance", Vec::new());
}

/// 构造单分区、单值索引样例表（handle 1/2/3/10/20）。
fn scalar_table(global: bool) -> AdminTable {
    let mut table = AdminTable::new(false, global);
    for handle in [1, 2, 3, 10, 20] {
        table.insert(0, handle, vec![handle]);
    }
    table
}

/// recover 应补回缺失索引项并返回 (新增数, 扫描行数)。
#[test]
fn admin_recover_index_restores_missing_entries_and_reports_scan_count() {
    let mut table = scalar_table(false);
    table.corrupt_remove(0, 1, 1);
    assert_eq!(
        table.check(),
        Err(Inconsistency::MissingIndex {
            partition: 0,
            handle: 1,
            value: 1
        })
    );
    assert_eq!(table.indexed_row_count(), 4);
    assert_eq!(table.recover_index(), (1, 5));
    assert_eq!(table.indexed_row_count(), 5);
    assert_eq!(table.check(), Ok(()));
}

/// cleanup 应删除悬空与错误值索引项。
#[test]
fn admin_cleanup_index_removes_dangling_and_wrong_value_entries() {
    let mut table = scalar_table(false);
    table.corrupt_insert(0, 42, 42);
    table.corrupt_insert(0, 1, 99);
    assert!(matches!(
        table.check(),
        Err(Inconsistency::DanglingIndex { .. })
    ));
    assert_eq!(table.cleanup_index(), 2);
    assert_eq!(table.check(), Ok(()));
}

/// 多值索引 recover 应对 JSON 数组元素去重后补齐。
#[test]
fn multi_valued_recover_deduplicates_json_array_elements() {
    let mut table = AdminTable::new(true, false);
    for handle in 0..5 {
        table.insert(0, handle, vec![handle, handle + 1, handle + 2]);
    }
    table.corrupt_remove(0, 1, 2);
    assert_eq!(
        table.check(),
        Err(Inconsistency::MissingIndex {
            partition: 0,
            handle: 1,
            value: 2,
        })
    );
    assert_eq!(table.recover_index(), (1, 5));
    assert_eq!(table.check(), Ok(()));
}

/// Go 的多值索引 cleanup 场景：额外的 (value=9, handle=9) 应只删除一项。
#[test]
fn multi_valued_cleanup_removes_only_the_dangling_entry() {
    let mut table = AdminTable::new(true, false);
    for handle in 0..5 {
        table.insert(0, handle, vec![handle, handle + 1, handle + 2]);
    }
    table.corrupt_insert(0, 9, 9);
    assert_eq!(
        table.check(),
        Err(Inconsistency::DanglingIndex {
            partition: 0,
            handle: 9,
            value: 9,
        })
    );
    assert_eq!(table.cleanup_index(), 1);
    assert_eq!(table.check(), Ok(()));
}

/// 分区索引修复时相同 handle 按 partition 隔离。
#[test]
fn partition_index_repair_keeps_equal_handles_isolated_by_partition() {
    let mut table = AdminTable::new(false, false);
    table.insert(11, 1, vec![7]);
    table.insert(12, 1, vec![7]);
    table.corrupt_remove(12, 1, 7);
    assert_eq!(table.recover_index(), (1, 2));
    assert_eq!(table.check(), Ok(()));
}

/// Go 对 hash/range 两种三分区表逐分区破坏并恢复，每次都扫描三行。
#[test]
fn partition_index_recovery_covers_hash_and_range_layouts() {
    let layouts = [
        [(101, 0, 0), (102, 1, 1), (103, 2, 2)],
        [(201, 0, 0), (202, 6, 6), (203, 12, 12)],
    ];

    for layout in layouts {
        let mut table = AdminTable::new(false, false);
        for (partition, handle, value) in layout {
            table.insert(partition, handle, vec![value]);
        }
        assert_eq!(table.recover_index(), (0, 3));

        for (partition, handle, value) in layout {
            table.corrupt_remove(partition, handle, value);
            assert_eq!(
                table.check(),
                Err(Inconsistency::MissingIndex {
                    partition,
                    handle,
                    value,
                })
            );
            assert_eq!(table.indexed_row_count(), 2);
            assert_eq!(table.recover_index(), (1, 3));
            assert_eq!(table.indexed_row_count(), 3);
            assert_eq!(table.check(), Ok(()));
        }
    }
}

/// 全局索引 cleanup/recover 保留分区身份。
#[test]
fn global_index_cleanup_and_recovery_preserve_partition_identity() {
    let mut table = AdminTable::new(false, true);
    table.insert(101, 1, vec![11]);
    table.insert(102, 2, vec![22]);
    table.corrupt_remove(101, 1, 11);
    table.corrupt_insert(999, 9, 99);
    assert!(table.is_global());
    assert_eq!(table.cleanup_index(), 1);
    assert_eq!(table.recover_index(), (1, 2));
    assert_eq!(table.check(), Ok(()));
}

/// Go 生成列场景把正确索引值 10 替换为错误值 5；普通/快速检查都必须报错。
#[test]
fn generated_index_corruption_is_detected_by_regular_and_fast_checks() {
    let mut table = AdminTable::new(false, false);
    table.insert(0, 2, vec![10]);
    table.corrupt_remove(0, 2, 10);
    table.corrupt_insert(0, 2, 5);
    assert_eq!(table.indexed_row_count(), 1);
    let expected = Err(Inconsistency::MissingIndex {
        partition: 0,
        handle: 2,
        value: 10,
    });
    assert_eq!(table.check(), expected);

    let vars = AdminSessionVars {
        index_lookup_size: 3,
        max_chunk_size: 3,
        concurrency: 4,
    };
    let mut propagated = Vec::new();
    assert_eq!(table.fast_check(vars, &mut propagated), expected);
    assert_eq!(propagated, vec![vars]);
}

/// 快照检查看到旧一致态，当前表则报缺失索引。
#[test]
fn snapshot_check_observes_old_consistent_state_and_current_corruption() {
    let mut table = scalar_table(false);
    let snapshot = table.clone();
    table.corrupt_remove(0, 10, 10);
    assert_eq!(snapshot.check(), Ok(()));
    assert!(matches!(
        table.check(),
        Err(Inconsistency::MissingIndex { handle: 10, .. })
    ));
}

/// fast_check 应把会话变量传播到内部 worker（此处记入 propagated）。
#[test]
fn fast_check_propagates_session_variables_to_internal_worker() {
    let table = scalar_table(false);
    let vars = AdminSessionVars {
        index_lookup_size: 3,
        max_chunk_size: 3,
        concurrency: 9,
    };
    let mut propagated = Vec::new();
    assert_eq!(table.fast_check(vars, &mut propagated), Ok(()));
    assert_eq!(propagated, vec![vars]);
}

/// Go 并发场景启动五个独立会话；内存契约用五个 worker 并发读取同一一致态。
#[test]
fn fast_check_supports_five_concurrent_workers() {
    let table = std::sync::Arc::new(scalar_table(false));
    let workers: Vec<_> = (0..5)
        .map(|_| {
            let table = std::sync::Arc::clone(&table);
            std::thread::spawn(move || {
                let mut propagated = Vec::new();
                assert_eq!(
                    table.fast_check(AdminSessionVars::default(), &mut propagated),
                    Ok(())
                );
                assert_eq!(propagated, vec![AdminSessionVars::default()]);
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("fast-check worker panicked");
    }
}

#[test]
fn partial_multi_valued_indexes_reject_admin_checks() {
    let _restore = RestoreOnDrop::new(astersql_config::restore_func());
    astersql_config::update_global(|conf| {
        conf.experimental.allows_expression_index = true;
    });
    let mut tk = admin_testkit();
    tk.MustExec("create table t_partial(a json, flag set('a','b'), index idx((cast(a as signed array))) where flag = 1)", Vec::new());
    for with_rows in [false, true] {
        if with_rows {
            tk.MustExec(
                "insert into t_partial values ('[1,2]', 'a'), ('[3,4]', 'b'), ('[5]', null)",
                Vec::new(),
            );
        }
        for fast_check in ["off", "on"] {
            tk.MustExec(
                &format!("set tidb_enable_fast_table_check = {fast_check}"),
                Vec::new(),
            );
            for sql in [
                "admin check table t_partial",
                "admin check index t_partial idx",
            ] {
                tk.MustGetErrMsg(sql, "[executor:8273]Validation of partial indexes requires tidb_enable_fast_table_check=ON");
            }
        }
    }
}
