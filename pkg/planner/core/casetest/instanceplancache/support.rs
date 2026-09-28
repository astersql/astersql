// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Shared real TestKit plumbing for the instance-plan-cache case tests.
//!
//! 实例级计划缓存用例的共享测试支撑：创建共用存储上的独立会话，串行隔离会修改
//! 进程级缓存配置的测试，并提供确定性的并发随机数及常用 SQL 断言封装。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};

/// Small thread-safe deterministic equivalent of the Go test's rand.Intn.
/// 为并发用例提供可复现的伪随机序列，避免依赖线程局部随机源造成结果漂移。
pub mod rand {
    use super::{AtomicU64, Ordering};

    static STATE: AtomicU64 = AtomicU64::new(0x9e3779b97f4a7c15);

    /// 返回 `[0, upper)` 内的整数；非正上界与 Go 测试的非法调用一样直接失败。
    /// CAS 循环保证多个测试线程推进同一序列时不会丢失状态更新。
    pub fn intn(upper: i32) -> i32 {
        assert!(upper > 0);
        let mut old = STATE.load(Ordering::Relaxed);
        loop {
            let next = old.wrapping_mul(6364136223846793005).wrapping_add(1);
            match STATE.compare_exchange_weak(old, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return (next % upper as u64) as i32,
                Err(current) => old = current,
            }
        }
    }
}

/// A fresh domain plus the store from which independent sessions can be made.
/// 单个用例的测试环境：主会话负责初始化，共享存储用于派生相互独立的工作会话。
pub struct Harness {
    /// 所有会话共用的统计信息模拟存储。
    pub store: Arc<AnalyzeStatsStore>,
    /// 用例初始化与断言使用的主会话。
    pub tk: TestKit,
    // Go's testing package runs these package-level cases serially. Keep the
    // same isolation for Rust's parallel test runner because the cases mutate
    // process-wide instance-plan-cache variables.
    // 持有全局锁直到环境销毁，避免 Rust 并行测试互相污染实例级计划缓存变量。
    _test_guard: MutexGuard<'static, ()>,
}

static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

impl Harness {
    /// 获取跨用例互斥锁后创建全新的模拟存储、域和主会话。
    pub fn new() -> Self {
        let test_guard = TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (store, _domain) = CreateMockStoreAndDomain();
        let tk = TestKit::new(store.clone());
        Self {
            store,
            tk,
            _test_guard: test_guard,
        }
    }

    /// 在同一存储上创建独立会话，用于模拟并发连接而不共享会话状态。
    pub fn session(&self) -> TestKit {
        TestKit::new(self.store.clone())
    }
}

/// 执行一条无需返回结果的 SQL，并沿用 TestKit 的失败即终止语义。
pub fn exec(tk: &mut TestKit, sql: &str) {
    tk.MustExec(sql, Vec::new());
}

/// 执行查询并返回结果包装，供调用方继续排序或比较行集。
pub fn query(tk: &TestKit, sql: &str) -> astersql_testkit::result::Result {
    tk.MustQuery(sql, Vec::new())
}

/// 按给定顺序批量执行 SQL，确保建表、写入和会话设置的先后关系不变。
pub fn exec_many(tk: &mut TestKit, statements: &[&str]) {
    for statement in statements {
        exec(tk, statement);
    }
}

/// 将一维字符串序列转换为 TestKit 比较所需的单列行集。
pub fn rows(values: &[String]) -> Vec<Vec<String>> {
    values.iter().map(|value| vec![value.clone()]).collect()
}
