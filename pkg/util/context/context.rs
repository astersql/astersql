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

// 会话上下文键值存储与全局上下文 ID 生成。
//
// 对应 Go `pkg/util/context`：`ValueStoreContext` 以 Display key 关联任意值；
// `GenContextID` 用原子计数生成单调递增 ID（对齐 Go `atomic.Uint64.Add`）。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// 可按 key 存取任意值的上下文接口。
///
/// key 用 `fmt::Display` 近似 Go `fmt.Stringer`；value 用 `dyn Any` 近似 `any`。
// ValueStoreContext is a context that can store values.
// ValueStoreContext 对应 Go 接口；key 的 fmt.Stringer 用 fmt::Display 近似，value any 用 dyn Any 近似。
pub trait ValueStoreContext {
    /// 保存 key 关联的值。
    // SetValue saves a value associated with this context for key.
    // SetValue 保存 key 关联的值；Go 的 any 允许任意类型，用 Box<dyn Any> 表达所有权。
    fn SetValue(&mut self, key: &dyn fmt::Display, value: Box<dyn Any>);

    /// 返回 key 关联的值；不存在时为 `None`（对应 Go 的 nil）。
    // Value returns the value associated with this context for key.
    // Value 返回 key 关联的值；Go 不存在时返回 nil，这里用 Option 表达。
    fn Value(&self, key: &dyn fmt::Display) -> Option<&dyn Any>;

    /// 清理 key 关联的值。
    // ClearValue clears the value associated with this context for key.
    // ClearValue 清理 key 关联的值。
    fn ClearValue(&mut self, key: &dyn fmt::Display);

    /// 返回 domain 动态引用（对应 Go 的 `any`）。
    // GetDomain returns the domain.
    // GetDomain 返回 domain；Go 返回 any，保留为可选动态引用。
    fn GetDomain(&self) -> Option<&dyn Any>;
}

/// 包级上下文 ID 生成器；`fetch_add` 返回旧值，调用方需 +1 对齐 Go `Add(1)`。
// contextIDGenerator 对应 Go 的 atomic.Uint64 包级变量。
// AtomicU64::fetch_add 返回旧值，因此 GenContextID 需要 +1 才等价于 Go 的 Add(1) 返回新值。
pub static contextIDGenerator: AtomicU64 = AtomicU64::new(0);

/// 生成单调递增的唯一上下文 ID；`SeqCst` 保守对应 Go atomic 同步语义。
// GenContextID generates a unique context ID.
// GenContextID 生成单调递增 ID；Ordering::SeqCst 保守对应 Go atomic.Uint64.Add 的同步语义。
pub fn GenContextID() -> u64 {
    contextIDGenerator
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1)
}
