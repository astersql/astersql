// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// DataSink 注册器单测：TopSQL/TopRU 开关引用计数与并发注销。
//
// 通过 `StateGuard` 在用例前后复位全局 topsql_state，并用 `#[serial]` 串行化，
// 避免多测例共享进程级开关互相干扰。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::datasink::*;
use crate::pubsub::{PubSubDataSink, PubSubResponse, PubSubStream};
use serial_test::serial;

/// 强制关闭 TopRU/TopSQL 并复位 TopRU 采样间隔。
fn reset_top_sql_state() {
    while crate::topsqlstate::TopRUEnabled() {
        crate::topsqlstate::DisableTopRU();
    }
    crate::topsqlstate::DisableTopSQL();
    crate::topsqlstate::ResetTopRUItemInterval();
}

/// RAII：构造与析构时均复位全局 TopSQL/TopRU 状态。
struct StateGuard;

impl StateGuard {
    fn new() -> Self {
        reset_top_sql_state();
        Self
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        reset_top_sql_state();
    }
}

/// 内存 Mock：记录收到的批次，并标记 closing 回调。
#[derive(Default)]
struct MockDataSink {
    data: Mutex<Vec<Arc<ReportData>>>,
    closed: AtomicBool,
}

impl DataSink for MockDataSink {
    fn try_send(&self, data: Arc<ReportData>, _deadline: Instant) -> Result<(), DataSinkError> {
        self.data.lock().unwrap().push(data);
        Ok(())
    }

    fn on_reporter_closing(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// 空实现的 pubsub 流，仅用于构造 `PubSubDataSink`。
#[derive(Default)]
struct SinkStream;

impl PubSubStream for SinkStream {
    fn send(&self, _response: PubSubResponse) -> Result<(), DataSinkError> {
        Ok(())
    }
}

/// 构造带订阅配置的 PubSub DataSink。
fn pubsub_sink(enable_top_sql: bool, enable_top_ru: bool, item_interval: i32) -> Arc<dyn DataSink> {
    Arc::new(PubSubDataSink::new(
        Arc::new(SinkStream),
        enable_top_sql,
        enable_top_ru,
        item_interval,
    ))
}

/// 基本注册/注销：启用 TopSQL，不启用 TopRU。
#[test]
#[serial]
fn test_default_data_sink_registerer() {
    let _guard = StateGuard::new();
    let registerer = DefaultDataSinkRegisterer::new();
    let first: Arc<dyn DataSink> = Arc::new(MockDataSink::default());
    let second: Arc<dyn DataSink> = Arc::new(MockDataSink::default());

    registerer.register(first.clone()).unwrap();
    registerer.register(second.clone()).unwrap();
    assert_eq!(registerer.sink_count(), 2);
    assert!(crate::topsqlstate::TopSQLEnabled());
    assert!(!crate::topsqlstate::TopRUEnabled());

    registerer.deregister(&first);
    registerer.deregister(&second);
    assert_eq!(registerer.sink_count(), 0);
    assert!(!crate::topsqlstate::TopSQLEnabled());
    assert!(!crate::topsqlstate::TopRUEnabled());
}

/// 两个 TopRU sink：后注册者覆盖间隔；全部注销后复位默认间隔。
#[test]
#[serial]
fn test_default_data_sink_registerer_top_ru_two_sinks_ref_count_and_reset() {
    let _guard = StateGuard::new();
    let registerer = DefaultDataSinkRegisterer::new();
    let first = pubsub_sink(true, true, 30);
    let second = pubsub_sink(true, true, 15);

    registerer.register(first.clone()).unwrap();
    registerer.register(second.clone()).unwrap();
    assert!(crate::topsqlstate::TopRUEnabled());
    // 后注册的 interval=15 覆盖前者。
    assert_eq!(crate::topsqlstate::GetTopRUItemInterval(), 15);

    registerer.deregister(&second);
    assert!(crate::topsqlstate::TopRUEnabled());
    assert_eq!(crate::topsqlstate::GetTopRUItemInterval(), 15);

    registerer.deregister(&first);
    assert!(!crate::topsqlstate::TopRUEnabled());
    assert_eq!(
        crate::topsqlstate::GetTopRUItemInterval(),
        crate::topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
}

/// 同一 sink 重复 register 幂等，只计一次引用。
#[test]
#[serial]
fn test_default_data_sink_registerer_top_ru_duplicate_register_is_idempotent() {
    let _guard = StateGuard::new();
    let registerer = DefaultDataSinkRegisterer::new();
    let sink = pubsub_sink(true, true, 15);

    registerer.register(sink.clone()).unwrap();
    registerer.register(sink.clone()).unwrap();
    assert_eq!(registerer.sink_count(), 1);
    assert!(crate::topsqlstate::TopRUEnabled());

    registerer.deregister(&sink);
    assert!(!crate::topsqlstate::TopRUEnabled());
    assert_eq!(
        crate::topsqlstate::GetTopRUItemInterval(),
        crate::topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
}

/// 多线程交错 register/deregister 后引用计数归零、开关关闭。
#[test]
#[serial]
fn test_default_data_sink_registerer_top_ru_ref_count_concurrent_register_deregister() {
    let _guard = StateGuard::new();
    let registerer = Arc::new(DefaultDataSinkRegisterer::new());
    let sinks = [
        pubsub_sink(true, true, 15),
        pubsub_sink(true, true, 30),
        pubsub_sink(true, true, 60),
    ];

    // 四线程轮询三个 sink，验证并发下引用计数不泄漏。
    let workers: Vec<_> = (0..4)
        .map(|index| {
            let registerer = registerer.clone();
            let sink = sinks[index % sinks.len()].clone();
            std::thread::spawn(move || {
                for _ in 0..8 {
                    registerer.register(sink.clone()).unwrap();
                    registerer.deregister(&sink);
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    for sink in &sinks {
        registerer.deregister(sink);
    }

    assert!(!crate::topsqlstate::TopRUEnabled());
    assert_eq!(
        crate::topsqlstate::GetTopRUItemInterval(),
        crate::topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
    assert!(!crate::topsqlstate::TopSQLEnabled());
    assert_eq!(registerer.sink_count(), 0);
}

/// TopSQL-only 与 TopRU-only sink 独立控制各自全局开关。
#[test]
#[serial]
fn test_default_data_sink_registerer_top_sql_respects_top_ru_only() {
    let _guard = StateGuard::new();
    let registerer = DefaultDataSinkRegisterer::new();
    let top_sql = pubsub_sink(true, false, 0);
    let top_ru = pubsub_sink(false, true, 15);

    registerer.register(top_sql.clone()).unwrap();
    assert!(crate::topsqlstate::TopSQLEnabled());
    assert!(!crate::topsqlstate::TopRUEnabled());

    registerer.register(top_ru.clone()).unwrap();
    assert!(crate::topsqlstate::TopSQLEnabled());
    assert!(crate::topsqlstate::TopRUEnabled());

    registerer.deregister(&top_sql);
    assert!(!crate::topsqlstate::TopSQLEnabled());
    assert!(crate::topsqlstate::TopRUEnabled());

    registerer.deregister(&top_ru);
    assert!(!crate::topsqlstate::TopSQLEnabled());
    assert!(!crate::topsqlstate::TopRUEnabled());
}

/// SingleTarget（无订阅配置）只开 TopSQL，且不参与 TopRU 引用计数。
#[test]
#[serial]
fn test_default_data_sink_registerer_single_target_top_ru_behavior() {
    let _guard = StateGuard::new();

    // A sink without subscription configuration has SingleTarget semantics:
    // it enables TopSQL and never participates in the TopRU reference count.
    // 无 subscription_config 的 sink：启用 TopSQL，永不计入 TopRU。
    let registerer = DefaultDataSinkRegisterer::new();
    let single_target: Arc<dyn DataSink> = Arc::new(MockDataSink::default());
    registerer.register(single_target.clone()).unwrap();
    assert!(crate::topsqlstate::TopSQLEnabled());
    assert!(!crate::topsqlstate::TopRUEnabled());
    assert_eq!(
        crate::topsqlstate::GetTopRUItemInterval(),
        crate::topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
    registerer.deregister(&single_target);
    assert!(!crate::topsqlstate::TopSQLEnabled());

    let top_ru = pubsub_sink(false, true, 15);
    let single_target: Arc<dyn DataSink> = Arc::new(MockDataSink::default());
    registerer.register(top_ru.clone()).unwrap();
    assert!(!crate::topsqlstate::TopSQLEnabled());
    assert!(crate::topsqlstate::TopRUEnabled());
    registerer.register(single_target.clone()).unwrap();
    assert!(crate::topsqlstate::TopSQLEnabled());
    assert!(crate::topsqlstate::TopRUEnabled());

    registerer.deregister(&top_ru);
    assert!(crate::topsqlstate::TopSQLEnabled());
    assert!(!crate::topsqlstate::TopRUEnabled());
    assert_eq!(
        crate::topsqlstate::GetTopRUItemInterval(),
        crate::topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
    registerer.deregister(&single_target);
    assert!(!crate::topsqlstate::TopSQLEnabled());
}
