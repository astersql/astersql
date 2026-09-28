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

// TiKV 工具：提交并发度原子变量。
//
// 提供与 Go `go.uber.org/atomic.Int32` 一致的顺序一致读写封装，
// 以及系统变量 `tidb_committer_concurrency`（两阶段提交中 committer 并发度）的全局缓存。

use std::sync::atomic::{AtomicI32, Ordering};

/// 顺序一致语义的原子 `i32`，对齐 Go `go.uber.org/atomic.Int32`。
/// An atomic `i32` with the sequentially consistent load/store semantics used
/// by Go's `go.uber.org/atomic.Int32`.
pub struct GoAtomicI32(AtomicI32);

impl GoAtomicI32 {
    /// 以给定初值构造原子整数。
    pub const fn new(value: i32) -> Self {
        Self(AtomicI32::new(value))
    }

    /// 以 SeqCst 顺序读取当前值。
    pub fn load(&self) -> i32 {
        self.0.load(Ordering::SeqCst)
    }

    /// 以 SeqCst 顺序写入新值。
    pub fn store(&self, value: i32) {
        self.0.store(value, Ordering::SeqCst);
    }
}

// CommitterConcurrency 缓存系统变量 tidb_committer_concurrency 的当前值。
// CommitterConcurrency stores the current value of the sysvar tidb_committer_concurrency
#[allow(non_upper_case_globals)]
pub static CommitterConcurrency: GoAtomicI32 = GoAtomicI32::new(128);
