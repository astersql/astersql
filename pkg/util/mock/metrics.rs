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

// 测试用内存指标计数器。
//
// 对应 Go 嵌入的可选 prometheus.Counter：本实现用原子 f64 位模式保证并发正确。

use std::sync::atomic::{AtomicU64, Ordering};

/// 内存计数器；`Counter` 可选以对齐 Go 中可为 nil 的 prometheus.Counter。
/// MetricsCounter is an in-memory counter used by tests. Counter remains
/// optional because the Go embedded prometheus.Counter may be nil.
pub struct MetricsCounter {
    pub Counter: Option<prometheus::Counter>,
    val: AtomicU64,
}

impl Default for MetricsCounter {
    fn default() -> Self {
        Self {
            Counter: None,
            val: AtomicU64::new(0_f64.to_bits()),
        }
    }
}

impl MetricsCounter {
    /// 原子累加浮点值（CAS 循环）。
    pub fn Add(&self, value: f64) {
        let mut current = self.val.load(Ordering::SeqCst);
        loop {
            let next = (f64::from_bits(current) + value).to_bits();
            match self
                .val
                .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// 计数加一。
    pub fn Inc(&self) {
        self.Add(1.0);
    }

    /// 读取当前浮点计数值。
    pub fn Val(&self) -> f64 {
        f64::from_bits(self.val.load(Ordering::SeqCst))
    }
}
