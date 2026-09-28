// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// GC Tuner 终结器（finalizer）运行时适配。
//
// Go 依赖 tracing-GC 的 finalizer 在回收时回调并重新 arm；Rust 无对等钩子。
// 本模块用独立运行时线程周期性请求系统分配器归还空闲页，然后回调并重新 arm；
// `run` 仍保留为测试和显式回收边界入口。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::mem::releaseUnusedMemory;

const RUNTIME_COLLECTION_INTERVAL: Duration = Duration::from_millis(300);

/// 终结器回调：可在多次 `run` 间可变捕获状态。
pub type finalizerCallback = Box<dyn FnMut() + Send + 'static>;

/// Rust has no tracing-GC finalizer hook. This object preserves the Go
/// finalizer's re-arm and stop semantics while the runtime integration calls
/// `run` at each collection boundary.
/// 可重复触发的终结器：`run` 执行回调后仍保持 armed，`stop` 后拒绝再跑。
pub struct Finalizer {
    callback: Mutex<finalizerCallback>,
    stopped: AtomicBool,
    wake_lock: Mutex<()>,
    wake: Condvar,
}

/// Go 风格类型别名，指向 `Finalizer`。
pub type finalizer = Finalizer;

/// 创建带回调的终结器，初始未停止。
pub fn newFinalizer(callback: finalizerCallback) -> Arc<Finalizer> {
    let finalizer = Arc::new(Finalizer {
        callback: Mutex::new(callback),
        stopped: AtomicBool::new(false),
        wake_lock: Mutex::new(()),
        wake: Condvar::new(),
    });
    Finalizer::startRuntimeDriver(&finalizer);
    finalizer
}

impl Finalizer {
    fn startRuntimeDriver(finalizer: &Arc<Self>) {
        let weak = Arc::downgrade(finalizer);
        thread::spawn(move || {
            loop {
                let Some(finalizer) = weak.upgrade() else {
                    return;
                };
                let guard = finalizer
                    .wake_lock
                    .lock()
                    .expect("finalizer wake lock poisoned");
                let (_guard, timeout) = finalizer
                    .wake
                    .wait_timeout_while(guard, RUNTIME_COLLECTION_INTERVAL, |_| {
                        !finalizer.stopped.load(Ordering::SeqCst)
                    })
                    .expect("finalizer wake lock poisoned");
                if finalizer.stopped.load(Ordering::SeqCst) {
                    return;
                }
                if timeout.timed_out() {
                    releaseUnusedMemory();
                    finalizer.run();
                }
            }
        });
    }

    /// Executes one finalizer notification and remains armed for the next one.
    /// 执行一次回调通知；已 stop 则返回 false 且不调用回调。
    pub fn run(&self) -> bool {
        // 双重检查 stopped：加锁前后都读，避免与 stop 竞态下误执行。
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        let mut callback = self
            .callback
            .lock()
            .expect("finalizer callback lock poisoned");
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        callback();
        true
    }

    /// 标记停止；之后的 `run` 不再执行回调。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    /// 是否已调用 `stop`。
    pub fn isStopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}
