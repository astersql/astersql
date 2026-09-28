// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// KV 客户端会话侧变量：锁等待退避参数与 kill 标志。
//
// 对齐 client-go/kv.Variables 的公开数据契约，供事务重试与锁冲突退避使用。

use std::sync::atomic::{AtomicU32, Ordering};

/// 快速锁退避的默认基础延迟参数。
pub const DefBackoffLockFast: i32 = 10;
/// 锁退避权重默认值，用于放大退避时长。
pub const DefBackOffWeight: i32 = 2;

// Rust's TiKV client does not expose client-go's session Variables type. This
// compatibility value ports that public data contract directly: the defaults
// and the borrowed kill flag are the same as client-go/kv.NewVariables.
/// 事务/锁相关会话变量：退避参数与可共享的 kill（中止）标志。
pub struct Variables<'a> {
    /// 快速获取锁失败时的退避基数。
    pub BackoffLockFast: i32,
    /// 退避时长的权重乘数。
    pub BackOffWeight: i32,
    /// 非零表示会话已被 kill，事务应尽快退出。
    pub Killed: &'a AtomicU32,
}

impl Variables<'_> {
    /// 若 kill 标志非零则返回 true。
    pub fn IsKilled(&self) -> bool {
        // Go's atomic.LoadUint32 is sequentially consistent. Keep the same
        // ordering so observing a kill also observes writes sequenced before it.
        self.Killed.load(Ordering::SeqCst) != 0
    }
}

/// 以默认退避参数构造 Variables，并借用调用方提供的 kill 原子量。
pub fn NewVariables(killed: &AtomicU32) -> Variables<'_> {
    Variables {
        BackoffLockFast: DefBackoffLockFast,
        BackOffWeight: DefBackOffWeight,
        Killed: killed,
    }
}
