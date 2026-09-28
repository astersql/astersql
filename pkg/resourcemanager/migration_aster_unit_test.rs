// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// ResourceManager 迁移对照单测：注册、调度守卫、Exec 边界与生命周期。
//
// 用固定返回命令的 Scheduler 与可观测的 TestPool，核对与 Go 侧一致的
// 升降档（Overclock / Downclock / Hold）条件及 DistTask 跳过逻辑。

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::rm::{RandomName, ResourceManager};
use crate::scheduler::{Command, Scheduler};
use crate::util::{self, Component, GoroutinePool};

/// 测试用协程池：可配置容量/运行数，并记录 Tune 次数与时间戳。
struct TestPool {
    name: String,
    capacity: AtomicI32,
    running: AtomicI32,
    /// 原始并发度（GetOriginConcurrency），Overclock 上限参考。
    origin: i32,
    last_tune: Mutex<SystemTime>,
    tune_count: AtomicUsize,
}

impl TestPool {
    /// 构造指定名称、容量与运行数的池；初始 last_tune 设为 10 秒前以便可立即调谐。
    fn new(name: &str, capacity: i32, running: i32) -> Self {
        Self {
            name: name.to_owned(),
            capacity: AtomicI32::new(capacity),
            running: AtomicI32::new(running),
            origin: capacity,
            last_tune: Mutex::new(SystemTime::now() - Duration::from_secs(10)),
            tune_count: AtomicUsize::new(0),
        }
    }

    /// 当前容量快照。
    fn capacity(&self) -> i32 {
        self.capacity.load(Ordering::SeqCst)
    }

    /// 已调用 Tune 的次数。
    fn tune_count(&self) -> usize {
        self.tune_count.load(Ordering::SeqCst)
    }
}

impl GoroutinePool for TestPool {
    fn ReleaseAndWait(&self) {}

    fn Tune(&self, size: i32) {
        self.capacity.store(size, Ordering::SeqCst);
        self.tune_count.fetch_add(1, Ordering::SeqCst);
        *self.last_tune.lock().unwrap() = SystemTime::now();
    }

    fn LastTunerTs(&self) -> SystemTime {
        *self.last_tune.lock().unwrap()
    }

    fn Cap(&self) -> i32 {
        self.capacity()
    }

    fn Running(&self) -> i32 {
        self.running.load(Ordering::SeqCst)
    }

    fn Name(&self) -> &str {
        &self.name
    }

    fn GetOriginConcurrency(&self) -> i32 {
        self.origin
    }
}

/// 固定返回同一 Command 的调度器，用于隔离多调度器串联逻辑。
struct FixedScheduler(Command);

impl Scheduler for FixedScheduler {
    fn Tune(&self, _component: Component, _pool: &dyn GoroutinePool) -> Command {
        self.0
    }
}

/// 按给定命令序列构造 ResourceManager（每个命令一个 FixedScheduler）。
fn manager_with(commands: &[Command]) -> ResourceManager {
    ResourceManager::new_with_schedulers(
        commands
            .iter()
            .copied()
            .map(|command| Box::new(FixedScheduler(command)) as Box<dyn Scheduler + Send + Sync>)
            .collect(),
    )
}

/// RandomName 每次应返回不同且可解析的 UUID 字符串。
#[test]
fn random_name_is_a_fresh_uuid() {
    let first = RandomName();
    let second = RandomName();

    assert_ne!(first, second);
    assert!(uuid::Uuid::parse_str(&first).is_ok());
    assert!(uuid::Uuid::parse_str(&second).is_ok());
}

/// 同名注册应失败；Unregister / Reset 后可再次以同名注册。
#[test]
fn register_rejects_duplicates_and_unregister_and_reset_clear_entries() {
    let manager = manager_with(&[]);

    manager
        .Register(
            Arc::new(TestPool::new("first", 1, 1)),
            "pool".to_owned(),
            util::DDL,
        )
        .unwrap();
    assert!(
        manager
            .Register(
                Arc::new(TestPool::new("duplicate", 1, 1)),
                "pool".to_owned(),
                util::DDL,
            )
            .is_err()
    );

    manager.Unregister("pool");
    manager
        .Register(
            Arc::new(TestPool::new("second", 1, 1)),
            "pool".to_owned(),
            util::DDL,
        )
        .unwrap();
    manager.Reset();
    manager
        .Register(
            Arc::new(TestPool::new("third", 1, 1)),
            "pool".to_owned(),
            util::DDL,
        )
        .unwrap();
}

/// schedulePool：空闲 Hold；忙且非最小容量时取调度器 Downclock；
/// 容量已为 1 或 running > capacity（过载）时强制 Hold。
#[test]
fn schedule_pool_preserves_go_guard_and_scheduler_order() {
    let manager = manager_with(&[Command::Hold, Command::Downclock]);
    let idle = util::PoolContainer {
        Pool: Arc::new(TestPool::new("idle", 2, 0)),
        Component: util::DDL,
    };
    assert_eq!(manager.schedulePool(&idle), Command::Hold);

    let busy = util::PoolContainer {
        Pool: Arc::new(TestPool::new("busy", 2, 2)),
        Component: util::DDL,
    };
    assert_eq!(manager.schedulePool(&busy), Command::Downclock);

    let minimum = util::PoolContainer {
        Pool: Arc::new(TestPool::new("minimum", 1, 1)),
        Component: util::DDL,
    };
    assert_eq!(manager.schedulePool(&minimum), Command::Hold);

    let overloaded = util::PoolContainer {
        Pool: Arc::new(TestPool::new("overloaded", 2, 3)),
        Component: util::DDL,
    };
    assert_eq!(manager.schedulePool(&overloaded), Command::Hold);
}

/// Exec：Hold 不调谐；Downclock 减容；Overclock 受 origin 上限与调谐冷却间隔约束。
#[test]
fn exec_matches_hold_interval_downclock_and_overclock_limits() {
    let manager = manager_with(&[]);

    let pool = Arc::new(TestPool::new("pool", 2, 2));
    let container = util::PoolContainer {
        Pool: pool.clone(),
        Component: util::DDL,
    };
    manager.Exec(&container, Command::Hold);
    assert_eq!(pool.capacity(), 2);
    assert_eq!(pool.tune_count(), 0);

    manager.Exec(&container, Command::Downclock);
    assert_eq!(pool.capacity(), 1);
    assert_eq!(pool.tune_count(), 1);

    let overclocked = Arc::new(TestPool::new("overclocked", 1, 1));
    let overclocked_container = util::PoolContainer {
        Pool: overclocked.clone(),
        Component: util::DDL,
    };
    manager.Exec(&overclocked_container, Command::Overclock);
    assert_eq!(overclocked.capacity(), 2);
    // 冷却已过，但容量已达 origin，再次 Overclock 不再增大。
    *overclocked.last_tune.lock().unwrap() = SystemTime::now() - Duration::from_secs(10);
    manager.Exec(&overclocked_container, Command::Overclock);
    assert_eq!(overclocked.capacity(), 2);

    // 刚调谐过：冷却期内 Overclock 应被跳过。
    let recent = Arc::new(TestPool::new("recent", 1, 1));
    *recent.last_tune.lock().unwrap() = SystemTime::now();
    let recent_container = util::PoolContainer {
        Pool: recent.clone(),
        Component: util::DDL,
    };
    manager.Exec(&recent_container, Command::Overclock);
    assert_eq!(recent.capacity(), 1);
    assert_eq!(recent.tune_count(), 0);
}

/// Go 的 int32 运行时加减按二进制回绕；公开 Exec 在极值处不得仅在 debug 构建 panic。
#[test]
fn exec_wraps_i32_capacity_edges_like_go() {
    let manager = manager_with(&[]);

    let minimum = Arc::new(TestPool::new("minimum-i32", i32::MIN, 1));
    let minimum_container = util::PoolContainer {
        Pool: minimum.clone(),
        Component: util::DDL,
    };
    manager.Exec(&minimum_container, Command::Downclock);
    assert_eq!(minimum.capacity(), i32::MAX);

    let maximum = Arc::new(TestPool::new("maximum-i32", i32::MAX, 1));
    let maximum_container = util::PoolContainer {
        Pool: maximum.clone(),
        Component: util::DDL,
    };
    manager.Exec(&maximum_container, Command::Overclock);
    assert_eq!(maximum.capacity(), i32::MIN);
}

/// schedule 应对 DistTask 组件跳过调谐，仅调整 DDL 等可调度池。
#[test]
fn schedule_skips_distributed_task_pools() {
    let manager = manager_with(&[Command::Overclock]);
    let ddl = Arc::new(TestPool::new("ddl", 1, 1));
    let distributed = Arc::new(TestPool::new("distributed", 1, 1));

    manager
        .Register(ddl.clone(), "ddl".to_owned(), util::DDL)
        .unwrap();
    manager
        .Register(
            distributed.clone(),
            "distributed".to_owned(),
            util::DistTask,
        )
        .unwrap();
    manager.schedule();

    assert_eq!(ddl.capacity(), 2);
    assert_eq!(distributed.capacity(), 1);
}

/// Start / Stop 应能完成后台调度循环的启停，不阻塞或 panic。
#[test]
fn start_and_stop_complete_the_background_lifecycle() {
    let manager = manager_with(&[]);

    manager.Start();
    std::thread::sleep(Duration::from_millis(20));
    manager.Stop();
}

/// Go 的 exitCh 在构造时创建：先 Stop 会永久关闭它，之后 Start 不得恢复调度。
#[test]
fn stop_before_start_keeps_the_manager_stopped_like_go() {
    let manager = manager_with(&[Command::Overclock]);
    let pool = Arc::new(TestPool::new("stopped", 1, 1));
    manager
        .Register(pool.clone(), "stopped".to_owned(), util::DDL)
        .unwrap();

    manager.Stop();
    manager.Start();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(pool.capacity(), 1);
}
