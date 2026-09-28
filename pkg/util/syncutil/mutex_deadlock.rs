// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 启用死锁检测的互斥锁 / 读写锁封装（对应 Go `deadlock` 构建标签）。
//
// 透明包装 `parking_lot` 的 Mutex/RWMutex；`EnableDeadlock` 恒为 true，
// `init` 镜像 Go 包级初始化，每 20 秒检查并报告死锁。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::ops::{Deref, DerefMut};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static START_DETECTOR: Once = Once::new();
static DETECTOR_STARTED: AtomicBool = AtomicBool::new(false);

/// A mutual exclusion lock which starts deadlock detection on first use.
#[derive(Debug)]
#[repr(transparent)]
pub struct Mutex<T: ?Sized>(parking_lot::Mutex<T>);

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        init();
        Self(parking_lot::Mutex::new(value))
    }

    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized> Deref for Mutex<T> {
    type Target = parking_lot::Mutex<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: ?Sized> DerefMut for Mutex<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// A reader/writer lock which starts deadlock detection on first use.
#[derive(Debug)]
#[repr(transparent)]
pub struct RWMutex<T: ?Sized>(parking_lot::RwLock<T>);

impl<T> RWMutex<T> {
    pub fn new(value: T) -> Self {
        init();
        Self(parking_lot::RwLock::new(value))
    }

    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }
}

impl<T: Default> Default for RWMutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized> Deref for RWMutex<T> {
    type Target = parking_lot::RwLock<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: ?Sized> DerefMut for RWMutex<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// EnableDeadlock is a flag to enable deadlock detection.
// EnableDeadlock 对应 Go 常量 `const EnableDeadlock = true`。
// 该文件只在 Go 的 deadlock 构建标签下生效，因此这里保持 true，供上层按原语义判断是否启用死锁检测。
/// 死锁检测开关：本变体固定为 true。
pub const EnableDeadlock: bool = true;

/// 报告锁等待超时的时长（对应 Go 包配置的 20 秒）。
/// The timeout configured by the Go package for reporting a lock wait.
pub const DEADLOCK_TIMEOUT: Duration = Duration::from_secs(20);

// init 对应 Go 包级 init 函数。Rust 没有包级初始化，因此锁构造器会惰性调用它。
/// Mirrors Go package initialization by starting one process-wide detector.
pub fn init() {
    START_DETECTOR.call_once(|| {
        DETECTOR_STARTED.store(true, Ordering::Release);
        std::thread::Builder::new()
            .name("syncutil-deadlock-detector".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(DEADLOCK_TIMEOUT);
                    let deadlocks = parking_lot::deadlock::check_deadlock();
                    if deadlocks.is_empty() {
                        continue;
                    }

                    eprintln!("{} deadlock(s) detected", deadlocks.len());
                    for (index, threads) in deadlocks.iter().enumerate() {
                        eprintln!(
                            "deadlock #{} involves {} thread(s)",
                            index + 1,
                            threads.len()
                        );
                        for thread in threads {
                            eprintln!(
                                "thread id: {:?}\n{:?}",
                                thread.thread_id(),
                                thread.backtrace()
                            );
                        }
                    }
                    std::process::abort();
                }
            })
            .expect("failed to start syncutil deadlock detector");
    });
}

#[cfg(test)]
pub(crate) fn detector_started() -> bool {
    DETECTOR_STARTED.load(Ordering::Acquire)
}
