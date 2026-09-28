// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Aster 迁移补充单测：对齐 Go worker 池的 FIFO 归还、阻塞申请与指标更新。
//
// 覆盖：取尽后按 Recycle 顺序再 Apply、池空时 Apply 阻塞直到归还、
// 以及 idle_workers_gauge / apply_worker_seconds_histogram 与 Go 一致。

use crate::{NewPool, metric};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// 验证容量 3 的池：ID 从 1 递增、取尽后 HasWorker 为假、Recycle 后 FIFO 复用同一 Arc，
/// 且 Recycle(None) panic 文案为 "invalid restore worker"。
#[test]
fn apply_recycle_matches_go_fifo_and_nil_guard() {
    let pool = NewPool(&metric::MetricContext::background(), 3, "test".to_owned());

    let w1 = pool.Apply();
    let w2 = pool.Apply();
    let w3 = pool.Apply();
    assert_eq!((w1.ID, w2.ID, w3.ID), (1, 2, 3));
    assert!(!pool.HasWorker());

    // 按 w3→w2→w1 归还，再 Apply 应得到同一 Arc（有界通道 FIFO）。
    pool.Recycle(Some(Arc::clone(&w3)));
    assert!(pool.HasWorker());
    assert!(Arc::ptr_eq(&w3, &pool.Apply()));
    pool.Recycle(Some(Arc::clone(&w2)));
    assert!(Arc::ptr_eq(&w2, &pool.Apply()));
    pool.Recycle(Some(Arc::clone(&w1)));
    assert!(Arc::ptr_eq(&w1, &pool.Apply()));
    assert!(!pool.HasWorker());

    // None 对应 Go 的 nil worker，须 panic 且文案不变。
    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.Recycle(None))).unwrap_err();
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("invalid restore worker"));
}

/// 容量为 1 时，取走唯一 worker 后另一线程的 Apply 必须阻塞，直到 Recycle 归还。
#[test]
fn apply_waits_until_a_worker_is_recycled() {
    let pool = Arc::new(NewPool(
        &metric::MetricContext::background(),
        1,
        "blocking".to_owned(),
    ));
    let worker = pool.Apply();
    let waiting_pool = Arc::clone(&pool);
    let waiter = thread::spawn(move || waiting_pool.Apply());

    // 短暂等待后确认 waiter 仍未完成，证明 Apply 在阻塞。
    thread::sleep(Duration::from_millis(20));
    assert!(
        !waiter.is_finished(),
        "Apply must block while the pool is empty"
    );
    pool.Recycle(Some(Arc::clone(&worker)));
    assert!(Arc::ptr_eq(&worker, &waiter.join().unwrap()));
}

/// 空闲 worker 仪表与 Apply 耗时直方图的标签与采样行为对齐 Go。
#[test]
fn pool_updates_the_same_metrics_as_go() {
    let factory = metric::promutil::NewDefaultFactory();
    let metrics = Arc::new(metric::new_metrics(factory.as_ref()));
    let ctx = metric::with_metric(metric::MetricContext::background(), Arc::clone(&metrics));
    let pool = NewPool(&ctx, 2, "metric-test".to_owned());
    let idle = metrics
        .idle_workers_gauge
        .get_metric_with_label_values(&["metric-test"])
        .unwrap();
    assert_eq!(idle.get(), 2.0);

    // Apply 后空闲减一，Recycle 后恢复；histogram 记录一次申请等待。
    let worker = pool.Apply();
    assert_eq!(idle.get(), 1.0);
    pool.Recycle(Some(worker));
    assert_eq!(idle.get(), 2.0);

    let histogram = metrics
        .apply_worker_seconds_histogram
        .get_metric_with_label_values(&["metric-test"])
        .unwrap();
    let snapshot = metric::read_histogram(&histogram).unwrap();
    assert_eq!(snapshot.histogram.as_ref().unwrap().sample_count(), 1);
}
