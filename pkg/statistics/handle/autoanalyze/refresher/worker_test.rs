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

// 自动 ANALYZE worker 的并发与异常清理测试。
//
// 通过可计数、可阻塞以及主动 panic 的执行器，验证 worker 与 Go 实现一致的
// 槽位限制、运行中作业快照、动态并发度和作业结束后的状态回收语义。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// 记录实际执行次数，用于确认被拒绝的作业不会进入执行器。
struct CountingExecutor {
    calls: AtomicUsize,
}

impl crate::AnalysisExecutor for CountingExecutor {
    fn analyze(&self, _job: &crate::AnalysisJob) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

/// 将作业阻塞在执行器内，确保断言运行快照时槽位仍被占用。
struct BlockingExecutor {
    started: (Mutex<usize>, Condvar),
    release: AtomicBool,
}

impl BlockingExecutor {
    /// 等待指定数量的作业进入执行器，并设置超时避免测试永久挂起。
    fn wait_for_started(&self, expected: usize) {
        let (lock, ready) = &self.started;
        let mut started = lock.lock().expect("started mutex poisoned");
        let deadline = Instant::now() + Duration::from_secs(10);
        while *started < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            started = ready
                .wait_timeout_while(started, remaining, |started| *started < expected)
                .expect("started mutex poisoned")
                .0;
        }
        assert!(*started >= expected, "worker did not start all jobs");
    }
}

impl crate::AnalysisExecutor for BlockingExecutor {
    fn analyze(&self, _job: &crate::AnalysisJob) -> Result<(), String> {
        let (lock, ready) = &self.started;
        *lock.lock().expect("started mutex poisoned") += 1;
        ready.notify_all();
        while !self.release.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        Ok(())
    }
}

/// 模拟执行器 panic，验证 worker 的兜底清理路径。
struct PanicExecutor;

impl crate::AnalysisExecutor for PanicExecutor {
    fn analyze(&self, _job: &crate::AnalysisJob) -> Result<(), String> {
        panic!("simulated panic");
    }
}

fn job(table_id: i64) -> crate::AnalysisJob {
    crate::AnalysisJob {
        table_id,
        priority: 0,
        must_retry: false,
    }
}

#[test]
fn test_worker_concurrency_limit_and_running_snapshot() {
    let executor = Arc::new(BlockingExecutor {
        started: (Mutex::new(0), Condvar::new()),
        release: AtomicBool::new(false),
    });
    let worker = crate::Worker::new(executor.clone(), 2);

    assert!(worker.submit_job(job(1)));
    assert!(worker.submit_job(job(2)));
    executor.wait_for_started(2);
    // 两个作业都保持运行时，第三个作业必须因并发槽位耗尽而被拒绝。
    assert!(!worker.submit_job(job(3)));
    assert_eq!(worker.running_jobs().len(), 2);
    assert!(worker.running_jobs().contains(&1));
    assert!(worker.running_jobs().contains(&2));

    executor.release.store(true, Ordering::Release);
    worker.wait_finished();
    assert!(worker.running_jobs().is_empty());
    worker.stop();
}

#[test]
fn test_worker_updates_concurrency_without_clamping() {
    let executor = Arc::new(CountingExecutor {
        calls: AtomicUsize::new(0),
    });
    let worker = crate::Worker::new(executor, 5);
    assert_eq!(worker.max_concurrency(), 5);
    worker.update_concurrency(10);
    assert_eq!(worker.max_concurrency(), 10);
    // 与 Go 实现保持一致：0 是有效配置，不能被自动修正为 1。
    worker.update_concurrency(0);
    assert_eq!(worker.max_concurrency(), 0);
    worker.stop();
}

#[test]
fn test_worker_zero_concurrency_rejects_jobs_like_go() {
    let executor = Arc::new(CountingExecutor {
        calls: AtomicUsize::new(0),
    });
    let worker = crate::Worker::new(executor.clone(), 0);
    assert_eq!(worker.max_concurrency(), 0);
    assert!(!worker.submit_job(job(1)));
    assert_eq!(executor.calls.load(Ordering::Acquire), 0);
    assert!(worker.running_jobs().is_empty());
    worker.stop();
}

#[test]
fn test_worker_recovers_from_panic_and_cleans_running_job() {
    let worker = crate::Worker::new(Arc::new(PanicExecutor), 1);
    assert!(worker.submit_job(job(1)));

    // panic 发生在后台线程中，轮询等待清理完成后再检查运行集合。
    for _ in 0..100 {
        if worker.running_jobs().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(worker.running_jobs().is_empty());
    worker.wait_finished();
    worker.stop();
}

#[test]
fn test_worker_recovers_from_multiple_panics_and_cleans_running_jobs() {
    let worker = crate::Worker::new(Arc::new(PanicExecutor), 2);
    assert!(worker.submit_job(job(1)));
    assert!(worker.submit_job(job(2)));

    worker.wait_finished();
    assert!(worker.running_jobs().is_empty());
    worker.stop();
}

#[test]
fn test_worker_error_still_releases_slot() {
    struct ErrorExecutor;
    impl crate::AnalysisExecutor for ErrorExecutor {
        fn analyze(&self, _job: &crate::AnalysisJob) -> Result<(), String> {
            Err("analysis failed".into())
        }
    }

    let worker = crate::Worker::new(Arc::new(ErrorExecutor), 1);
    assert!(worker.submit_job(job(1)));
    worker.wait_finished();
    assert!(worker.running_jobs().is_empty());
    // 错误返回也必须释放并发槽位，使后续作业仍可提交。
    assert!(worker.submit_job(job(2)));
    worker.wait_finished();
    worker.stop();
}
