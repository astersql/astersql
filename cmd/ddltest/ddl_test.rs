// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! Go-equivalent tests for `cmd/ddltest/ddl_test.go`.
//!
//! External TiDB processes / TiKV / MySQL driver are replaced by the in-crate
//! stub harness (darwin arm64: no kv/domain/kvproto/grpcio). Control flow,
//! concurrency, assertions and cleanup match Go.
//!
//! 这个文件保留 Go 用例的测试结构，但把外部集群、网络连接和驱动依赖
//! 折叠进 crate 内的桩环境，方便在当前 Rust 工作区做稳定回归。
//! 注释重点说明每组场景想验证什么、为何这样计数，以及哪些地方是在
//! “对齐 Go 语义”而不是追求最小实现。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use astersql_cmd_ddltest::stubs::{
    Datum, ENABLE_RESTART, HandleMap, Suite, create_ddl_suite, match_rows, random_num,
};

struct TeardownGuard<F: FnOnce()> {
    teardown: Option<F>,
}

impl<F: FnOnce()> TeardownGuard<F> {
    fn new(teardown: F) -> Self {
        Self {
            teardown: Some(teardown),
        }
    }
}

impl<F: FnOnce()> Drop for TeardownGuard<F> {
    fn drop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
    }
}

#[test]
fn teardown_guard_runs_during_unwind() {
    let teardown_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&teardown_called);

    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = TeardownGuard::new(move || observed.store(true, Ordering::SeqCst));
        panic!("force the assertion-failure cleanup path");
    }));
    assert!(panic_result.is_err());
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "teardown callback was skipped during unwinding"
    );
}

/// 创建并回收 `Suite` 的薄包装。
///
/// Go 版本每个顶层测试都会构造一套独立的 `ddlSuite`，结束时统一清理。
/// Rust 这里用闭包包一层，保证每个 `#[test]` 都复用同样的生命周期约束：
/// 先执行测试主体，再无条件 teardown，避免后续场景共享残留状态。
fn with_suite<F: FnOnce(Suite)>(f: F) {
    let s = create_ddl_suite();
    let teardown_suite = Arc::clone(&s);
    let _teardown = TeardownGuard::new(move || teardown_suite.teardown());
    f(Arc::clone(&s));
}

/// TestSimple — Go `TestSimple` (Basic / Mixed / Inc).
#[test]
fn test_simple() {
    with_suite(|s| {
        // Basic 子场景只验证最短 DDL/DML 链路：
        // 先异步建表，再插入一行，再读回一列，最后删表。
        // 这里覆盖的是“DDL 完成后 schema 对后续 SQL 立刻可见”这一前提。
        // Basic
        {
            let done = s.run_ddl("create table if not exists test_simple (c1 int, c2 int, c3 int)");
            done.recv().unwrap().expect("create test_simple");
            s.exec("insert into test_simple values (1, 1, 1)")
                .expect("insert");
            let mut rows = s
                .query("select c1 from test_simple limit 1")
                .expect("query");
            match_rows(&mut rows, &[vec![Datum::Int(1)]]);
            let done = s.run_ddl("drop table if exists test_simple");
            done.recv().unwrap().expect("drop test_simple");
        }

        // Mixed 同时覆盖普通主键表与复合主键表，确保桩环境中的表遍历、
        // 更新与删除行为不会只在单主键路径上成立。
        // Mixed
        for tbl_name in ["test_mixed", "test_mixed_common"] {
            // Go 版本固定 10 个 worker 并把总数据量均分；这里保持同一并发形状，
            // 让插入、更新、删除三种动作的竞争关系具有可比性。
            let worker_num = 10;
            let row_count = 10000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                handles.push(thread::spawn(move || {
                    // 第一阶段先铺满一批互不冲突的基准数据，避免后续混合流量
                    // 因“表本来就是空的”而失去观测价值。
                    for j in 0..batch {
                        let k = batch * i + j;
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleMixed][Insert][Time Cost]{:?}", start.elapsed());

            let start = Instant::now();
            // `row_id` 从已有最大键值开始递增，模拟 Go 中 atomic.AddInt64
            // 产生的新主键。这样新插入记录与“历史记录被更新/删除”可区分。
            let row_id = Arc::new(AtomicI64::new(row_count as i64));
            let default_value = -1_i64;
            let mut handles = Vec::new();
            for _ in 0..worker_num {
                let suite = Arc::clone(&s);
                let row_id = Arc::clone(&row_id);
                handles.push(thread::spawn(move || {
                    for _ in 0..batch {
                        // 新增记录使用单调递增键，保证插入动作本身不与初始批次冲突。
                        let key = row_id.fetch_add(1, Ordering::SeqCst) + 1;
                        suite.exec_insert(&format!("insert into {tbl_name} values ({key}, {key})"));
                        // 更新只随机命中初始键空间，因此最终值为 `-1` 的记录
                        // 代表“旧记录被并发改写”，不会把新插入的数据混进来。
                        let key = random_num(&[row_count]) as i64;
                        suite.must_exec(&format!(
                            "update {tbl_name} set c2 = {default_value} where c1 = {key}"
                        ));
                        // 删除同样只作用在初始键空间，最终剩余行数因此可由
                        // “总生成键数 - 保留的插入类 - 保留的更新类”反推出来。
                        let key = random_num(&[row_count]) as i64;
                        suite.must_exec(&format!("delete from {tbl_name} where c1 = {key}"));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleMixed][Mixed][Time Cost]{:?}", start.elapsed());

            s.new_txn().unwrap();
            let mut update_count = 0i64;
            let mut insert_count = 0i64;
            s.iter_records(tbl_name, |_h, data, _cols| {
                // `c2 == c1` 表示记录只经历过插入，没有被后续更新命中。
                if data[1].deep_equal(&data[0]) {
                    insert_count += 1;
                // `c2 == -1` 且键仍落在初始范围内，表示这是一条旧记录，
                // 它 survived delete 并被至少一次更新改写。
                } else if data[1].deep_equal(&Datum::Int(default_value))
                    && data[0].get_int64() < row_count as i64
                {
                    update_count += 1;
                } else {
                    panic!("[TestSimpleMixed fail]invalid row {data:?}");
                }
                Ok(true)
            })
            .unwrap();
            // `row_id` 记录的是理论上发号到哪里；减去当前可见的插入类和更新类，
            // 剩下的就是被删除或被竞争覆盖掉的数量。这里只要求三类都出现，
            // 证明混合压力确实触发了所有分支，而不是某个分支完全没命中。
            let delete_count = row_id.load(Ordering::SeqCst) - insert_count - update_count;
            assert!(insert_count > 0);
            assert!(update_count > 0);
            assert!(delete_count > 0);
        }

        // Inc 聚焦“热点行自增”而不是多键分布：
        // 只有 `c1 = 0` 会被反复更新，其他行保持镜像值。
        // Inc
        for tbl_name in ["test_inc", "test_inc_common"] {
            let worker_num = 10;
            let row_count = 1000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                handles.push(thread::spawn(move || {
                    // 先灌入确定性基线，后面才能判断只有热点行发生了累计变化。
                    for j in 0..batch {
                        let k = batch * i + j;
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleInc][Insert][Time Cost]{:?}", start.elapsed());

            let start = Instant::now();
            let mut handles = Vec::new();
            for _ in 0..worker_num {
                let suite = Arc::clone(&s);
                handles.push(thread::spawn(move || {
                    for _ in 0..batch {
                        // 所有 worker 都命中同一主键，用来模拟 Go 中最容易出现
                        // 写冲突、重试和重启抖动的更新热点。
                        suite.must_exec(&format!("update {tbl_name} set c2 = c2 + 1 where c1 = 0"));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleInc][Update][Time Cost]{:?}", start.elapsed());

            s.new_txn().unwrap();
            // Go 原始实现这里把 `getTable` 写死成 `"test_inc"`，导致复合主键表
            // 只执行了压力而没有被最终遍历验证。Rust 桩测试改用 `tbl_name`，
            // 目的是让两套表都接受同一组断言，而不是弱化检查。
            s.iter_records(tbl_name, |_h, data, _cols| {
                if data[0].deep_equal(&Datum::Int(0)) {
                    // 开启随机重启后，热点行可能因为重试/重复执行而大于理论值；
                    // 关闭重启时则要求精确等于 worker 总更新次数。
                    if ENABLE_RESTART {
                        assert!(data[1].get_int64() >= row_count as i64);
                    } else {
                        assert_eq!(data[1].get_int64(), row_count as i64);
                    }
                } else {
                    // 非热点行不应被波及，仍保持插入时的镜像列值。
                    assert!(data[1].deep_equal(&data[0]));
                }
                Ok(true)
            })
            .unwrap();
        }
    });
}

/// TestSimpleInsert — Go `TestSimpleInsert` (Basic / Conflict).
#[test]
fn test_simple_insert() {
    with_suite(|s| {
        // Basic 关注“高并发无冲突插入后，所有行都可见且列值保持镜像”。
        // Basic
        for tbl_name in ["test_insert", "test_insert_common"] {
            let worker_num = 10;
            let row_count = 10000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                handles.push(thread::spawn(move || {
                    // 每个 worker 负责互不重叠的键区间，所以最终句柄数必须
                    // 精确等于 `row_count`，不能只检查“至少插进去了若干行”。
                    for j in 0..batch {
                        let k = batch * i + j;
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleInsert][Time Cost]{:?}", start.elapsed());

            s.new_txn().unwrap();
            let mut handles_map = HandleMap::new();
            s.iter_records(tbl_name, |h, data, _cols| {
                // 用 handle 集合计数，而不是靠 SQL count(*)，是为了贴近 Go 的
                // 表遍历语义，顺带验证底层遍历没有重复或漏读。
                handles_map.set(h);
                assert!(data[1].deep_equal(&data[0]));
                Ok(true)
            })
            .unwrap();
            assert_eq!(row_count as usize, handles_map.len());
        }

        // Conflict 允许多个线程反复命中同一键；预期不是“每次 insert 都成功”，
        // 而是“最终落盘的键集合是所有成功或重复尝试键的子集且无脏值”。
        // 这种写法刻意不把唯一键冲突当失败，因为它本来就是测试目标的一部分。
        // Conflict
        for tbl_name in ["test_conflict_insert", "test_conflict_insert_common"] {
            // 共享 map 记录“测试线程曾经尝试过哪些键”，它是最终结果的上界，
            // 不是成功次数统计，因此和真实表行数比较时只看 key set。
            let keys_map = Arc::new(Mutex::new(HashMap::<i64, i64>::new()));
            let worker_num = 10;
            let row_count = 10000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for _ in 0..worker_num {
                let suite = Arc::clone(&s);
                let keys_map = Arc::clone(&keys_map);
                handles.push(thread::spawn(move || {
                    for _ in 0..batch {
                        // 与 Go 一样，这里故意忽略冲突插入的返回值：
                        // 重点是并发唯一键冲突后的最终表状态，而不是单次 SQL 成败。
                        let k = random_num(&[row_count]);
                        let _ = suite.exec(&format!("insert into {tbl_name} values ({k}, {k})"));
                        keys_map.lock().unwrap().insert(k as i64, k as i64);
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleConflictInsert][Time Cost]{:?}", start.elapsed());

            s.new_txn().unwrap();
            let keys = keys_map.lock().unwrap();
            let mut handles_map = HandleMap::new();
            s.iter_records(tbl_name, |h, data, _cols| {
                handles_map.set(h);
                // 每条保留下来的记录都必须来自尝试过的键空间，避免出现桩环境
                // 自发生成或串场的数据。
                assert!(keys.contains_key(&data[0].get_int64()));
                assert!(data[1].deep_equal(&data[0]));
                Ok(true)
            })
            .unwrap();
            // 最终记录数应与尝试过的唯一键数相等，表示冲突只折叠重复键，
            // 没有额外丢行或制造重复 handle。
            assert_eq!(keys.len(), handles_map.len());
        }
    });
}

/// TestSimpleUpdate — Go `TestSimpleUpdate` (Basic / Conflict).
#[test]
fn test_simple_update() {
    with_suite(|s| {
        // Basic 先插入唯一键，再立刻按同一键更新，验证并发更新不会破坏
        // 主键集合，同时检查每一行都携带最后一次写入的值。
        // Basic
        for tbl_name in ["test_update", "test_update_common"] {
            let keys_map = Arc::new(Mutex::new(HashMap::<i64, i64>::new()));
            let worker_num = 10;
            let row_count = 10000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                let keys_map = Arc::clone(&keys_map);
                handles.push(thread::spawn(move || {
                    for j in 0..batch {
                        let k = batch * i + j;
                        // 插入与更新配对发生，模拟业务中“创建后立即写业务字段”的模式。
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                        let v = random_num(&[row_count]);
                        suite.must_exec(&format!("update {tbl_name} set c2 = {v} where c1 = {k}"));
                        keys_map.lock().unwrap().insert(k as i64, v as i64);
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleUpdate][Time Cost]{:?}", start.elapsed());

            s.new_txn().unwrap();
            let keys = keys_map.lock().unwrap();
            let mut handles_map = HandleMap::new();
            s.iter_records(tbl_name, |h, data, _cols| {
                handles_map.set(h);
                let key = data[0].get_int64();
                // `keys_map` 保存每个键最后一次写入的目标值，断言这里相等
                // 就是在验证并发更新没有把旧值回写回来。
                assert_eq!(keys[&key], data[1].get_int64());
                Ok(true)
            })
            .unwrap();
            assert_eq!(row_count as usize, handles_map.len());
        }

        // Conflict 把“插入唯一键”和“随机更新已有键”拆成两个阶段，
        // 用来观察同一批记录被重复更新后的最终可接受状态。
        // 判断标准采用允许集合，而不是精确逐键比对，以保持和 Go 一致的容错边界。
        // Conflict
        for tbl_name in ["test_conflict_update", "test_conflict_update_common"] {
            let keys_map = Arc::new(Mutex::new(HashMap::<i64, i64>::new()));
            let worker_num = 10;
            let row_count = 10000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                let keys_map = Arc::clone(&keys_map);
                handles.push(thread::spawn(move || {
                    for j in 0..batch {
                        let k = batch * i + j;
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                        keys_map.lock().unwrap().insert(k as i64, k as i64);
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!(
                "[TestSimpleConflictUpdate][Insert][Time Cost]{:?}",
                start.elapsed()
            );

            let start = Instant::now();
            // `-1` 是哨兵值：它不会与初始镜像值 `c2 == c1` 混淆，便于区分
            // “未命中的旧记录”和“被竞争更新覆盖过的记录”。
            let default_value = -1_i64;
            let mut handles = Vec::new();
            for _ in 0..worker_num {
                let suite = Arc::clone(&s);
                let keys_map = Arc::clone(&keys_map);
                handles.push(thread::spawn(move || {
                    for _ in 0..batch {
                        let k = random_num(&[row_count]);
                        suite.must_exec(&format!(
                            "update {tbl_name} set c2 = {default_value} where c1 = {k}"
                        ));
                        keys_map.lock().unwrap().insert(k as i64, default_value);
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!(
                "[TestSimpleConflictUpdate][Update][Time Cost]{:?}",
                start.elapsed()
            );

            s.new_txn().unwrap();
            let keys = keys_map.lock().unwrap();
            let mut handles_map = HandleMap::new();
            s.iter_records(tbl_name, |h, data, _cols| {
                handles_map.set(h);
                assert!(keys.contains_key(&data[0].get_int64()));
                // 这个场景不要求精确知道每个键最终是不是 `-1`，因为多个线程
                // 可能反复命中同一行；它只要求结果属于 Go 允许的两种状态之一。
                if !data[1].deep_equal(&data[0]) && !data[1].deep_equal(&Datum::Int(default_value))
                {
                    panic!("[TestSimpleConflictUpdate fail]Bad row {data:?}");
                }
                Ok(true)
            })
            .unwrap();
            assert_eq!(row_count as usize, handles_map.len());
        }
    });
}

/// TestSimpleDelete — Go `TestSimpleDelete` (Basic / Conflict).
#[test]
fn test_simple_delete() {
    with_suite(|s| {
        // Basic 验证“每次插入都紧跟删除”后表应完全为空，
        // 这是删除路径最直接的正确性信号。
        // Basic
        for tbl_name in ["test_delete", "test_delete_common"] {
            let worker_num = 10;
            let row_count = 1000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                handles.push(thread::spawn(move || {
                    for j in 0..batch {
                        let k = batch * i + j;
                        // 单个 worker 内串行执行 insert + delete，跨 worker 再并发，
                        // 可以同时覆盖语句时序正确性和整体竞争下的最终空表状态。
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                        suite.must_exec(&format!("delete from {tbl_name} where c1 = {k}"));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!("[TestSimpleDelete][Time Cost]{:?}", start.elapsed());

            s.new_txn().unwrap();
            let mut handles_map = HandleMap::new();
            s.iter_records(tbl_name, |h, _data, _cols| {
                handles_map.set(h);
                Ok(true)
            })
            .unwrap();
            // 这里必须是 0，而不是“没有异常即可”，否则无法证明删除真的落地。
            assert_eq!(0, handles_map.len());
        }

        // Conflict 先装满一批确定性记录，再随机删除已有键，
        // 验证剩余键集合与测试侧维护的 map 一致。
        // Conflict
        for tbl_name in ["test_conflict_delete", "test_conflict_delete_common"] {
            let keys_map = Arc::new(Mutex::new(HashMap::<i64, i64>::new()));
            let worker_num = 10;
            let row_count = 1000;
            let batch = row_count / worker_num;
            let start = Instant::now();
            let mut handles = Vec::new();
            for i in 0..worker_num {
                let suite = Arc::clone(&s);
                let keys_map = Arc::clone(&keys_map);
                handles.push(thread::spawn(move || {
                    for j in 0..batch {
                        let k = batch * i + j;
                        suite.exec_insert(&format!("insert into {tbl_name} values ({k}, {k})"));
                        keys_map.lock().unwrap().insert(k as i64, k as i64);
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!(
                "[TestSimpleConflictDelete][Insert][Time Cost]{:?}",
                start.elapsed()
            );

            let start = Instant::now();
            let mut handles = Vec::new();
            for _ in 0..worker_num {
                let suite = Arc::clone(&s);
                let keys_map = Arc::clone(&keys_map);
                handles.push(thread::spawn(move || {
                    for _ in 0..batch {
                        // 删除命中随机键，线程本地并不知道该键当前是否还存在；
                        // 共享 map 因此承担“预期剩余键集合”的角色。
                        let k = random_num(&[row_count]);
                        suite.must_exec(&format!("delete from {tbl_name} where c1 = {k}"));
                        keys_map.lock().unwrap().remove(&(k as i64));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            println!(
                "[TestSimpleConflictDelete][Delete][Time Cost]{:?}",
                start.elapsed()
            );

            s.new_txn().unwrap();
            let keys = keys_map.lock().unwrap();
            let mut handles_map = HandleMap::new();
            s.iter_records(tbl_name, |h, data, _cols| {
                handles_map.set(h);
                // 遍历出来的每一行都必须还能在预期 map 中找到，
                // 这能证明删除不会“假成功”或留下未知来源的记录。
                assert!(keys.contains_key(&data[0].get_int64()));
                Ok(true)
            })
            .unwrap();
            // 表中可见行数与 map 长度相等，表示删除结果与测试侧模型收敛一致。
            assert_eq!(keys.len(), handles_map.len());
        }
    });
}
