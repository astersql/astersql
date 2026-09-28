// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! 检查点 Runner 用的周期触发器，对齐 Go `br/pkg/checkpoint/ticker.go`。
//!
//! `dispatcherTicker(d)`：`d > 0` 时后台线程按间隔投递 `Instant`；
//! `d == 0` 时返回无 channel 的 `manualTicker`，供测试主动驱动或禁用定时。

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};

/// 统一 tick 抽象：可读 channel，并可 `Stop` 停止后台循环。
pub trait TimeTicker: Send {
    /// 定时事件通道；manual 实现返回 None
    fn Ch(&self) -> Option<&Receiver<Instant>>;
    /// 请求后台循环退出（幂等）
    fn Stop(&mut self);
}

/// 真实定时器：独立线程 sleep → send；`Stop` 置位后循环退出。
pub struct timeTicker {
    /// 消费者侧接收 Instant
    rx: Receiver<Instant>,
    /// 与后台线程共享的停止标志
    stop: ArcStop,
    /// 持有 join handle，防止线程被分离后失控
    _handle: Option<JoinHandle<()>>,
}

/// 包装 `Arc<AtomicBool>`，供 trait 对象与后台线程共享停止标志。
struct ArcStop(std::sync::Arc<AtomicBool>);

impl TimeTicker for timeTicker {
    fn Ch(&self) -> Option<&Receiver<Instant>> {
        Some(&self.rx)
    }

    fn Stop(&mut self) {
        self.stop.0.store(true, Ordering::SeqCst);
    }
}

/// 零周期占位：`Ch` 恒为 `None`，不会自发触发；`Stop` 为空操作。
pub struct manualTicker {}

impl TimeTicker for manualTicker {
    fn Ch(&self) -> Option<&Receiver<Instant>> {
        None
    }

    fn Stop(&mut self) {}
}

/// 按周期选择实现：正时长 → `timeTicker`；零时长 → `manualTicker`。
/// 对应 Go `dispatcherTicker`；后台线程在每次 sleep 后复查 stop，避免停后仍 send。
pub fn dispatcherTicker(d: Duration) -> Box<dyn TimeTicker> {
    if d > Duration::ZERO {
        // Go's time.Ticker keeps a single pending tick and drops later ticks
        // while the receiver is behind. A bounded channel preserves that
        // backpressure contract and prevents an idle consumer growing memory.
        let (tx, rx): (Sender<Instant>, Receiver<Instant>) = bounded(1);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                thread::sleep(d);
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tx.try_send(Instant::now());
            }
        });
        Box::new(timeTicker {
            rx,
            stop: ArcStop(stop),
            _handle: Some(handle),
        })
    } else {
        Box::new(manualTicker {})
    }
}
