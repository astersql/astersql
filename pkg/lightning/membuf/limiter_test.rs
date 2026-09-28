// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `Limiter` 单元测试：并发 Acquire/Release 不超配额，以及一次释放唤醒多个等待者。

use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
    mpsc,
};
use std::thread;

struct CapturingLogger {
    messages: std::sync::Mutex<Vec<String>>,
}

impl log::Log for CapturingLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Error
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.messages
                .lock()
                .unwrap()
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static TEST_LOGGER: CapturingLogger = CapturingLogger {
    messages: std::sync::Mutex::new(Vec::new()),
};

/// 多线程各 Acquire(1)/Release(1)，期间并发持有量不超过 limit，结束后配额归满。
#[test]
fn test_limiter() {
    let limit = 20;
    let current = Arc::new(AtomicI64::new(0));
    let limiter = NewLimiter(limit);
    let mut workers = Vec::with_capacity(100);

    // 100 个 worker 争抢配额；持有期间用原子计数校验不突破上限
    for _ in 0..100 {
        let limiter = limiter.clone();
        let current = current.clone();
        workers.push(thread::spawn(move || {
            limiter.Acquire(1);
            let value = current.fetch_add(1, Ordering::SeqCst) + 1;
            assert!(value <= limit as i64);
            current.fetch_sub(1, Ordering::SeqCst);
            limiter.Release(1);
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(limit, limiter.state.lock().unwrap().limit);
}

/// 先占用大部分配额，再让多个线程阻塞在 Acquire；一次 Release 应唤醒全部等待者。
#[test]
fn test_wait_up_multiple_caller() {
    let limit = 20;
    let limiter = NewLimiter(limit);
    // 先拿走 18，剩余 2；后面每个等待者要 3，必须阻塞
    limiter.Acquire(18);

    let (start_tx, start_rx) = mpsc::sync_channel(3);
    let (finish_tx, finish_rx) = mpsc::sync_channel(3);
    let mut workers = Vec::with_capacity(3);
    for _ in 0..3 {
        let limiter = limiter.clone();
        let start_tx = start_tx.clone();
        let finish_tx = finish_tx.clone();
        workers.push(thread::spawn(move || {
            start_tx.send(()).unwrap();
            limiter.Acquire(3);
            finish_tx.send(()).unwrap();
        }));
    }
    // 确认三个线程都已进入 Acquire 路径
    for _ in 0..3 {
        start_rx.recv().unwrap();
    }
    // 释放前不应有人完成
    assert!(finish_rx.try_recv().is_err());
    // 归还 18 后余额足够连续唤醒 3 个各需 3 的等待者
    limiter.Release(18);
    for _ in 0..3 {
        finish_rx.recv().unwrap();
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(limit - 3 * 3, limiter.state.lock().unwrap().limit);
}

/// 与 Go 的 zap.Stack 一致，超量归还的错误日志应携带真实回溯而非迁移占位文本。
#[test]
fn release_overflow_logs_a_real_stack() {
    let _ = log::set_logger(&TEST_LOGGER);
    log::set_max_level(log::LevelFilter::Error);
    TEST_LOGGER.messages.lock().unwrap().clear();

    NewLimiter(1).Release(1);

    let messages = TEST_LOGGER.messages.lock().unwrap();
    let message = messages
        .iter()
        .find(|message| message.contains("limit overflow"))
        .expect("overflow must be logged");
    assert!(message.contains("stack="));
    assert!(!message.contains("placeholder"));
}
