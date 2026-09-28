// Copyright 2026 AsterSQL.
// Copyright 2019 TiKV Project Authors.
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

//! Mock ID allocator matching `br/pkg/mock/mockid/mockid.go`.
//!
//! 测试专用 ID 分配器：用原子计数模拟元数据 ID 发放，不接真实 PD/TSO。
//! Alloc 语义对齐 Go `atomic.AddUint64`（返回自增后的值）；Rebase 为空操作。

use std::sync::atomic::{AtomicU64, Ordering};

/// IDAllocator mocks IDAllocator and it is only used for test.
///
/// 仅测试使用：`base` 从 0 起，Alloc 线程安全递增；不实现持久化或租约。
pub struct IDAllocator {
    base: AtomicU64,
}

/// NewIDAllocator creates a new IDAllocator.
///
/// 构造 base=0 的分配器；首次 Alloc 得到 1（与 Go 一致）。
pub fn NewIDAllocator() -> IDAllocator {
    IDAllocator {
        base: AtomicU64::new(0),
    }
}

#[cfg(test)]
pub(crate) fn new_id_allocator_with_base(base: u64) -> IDAllocator {
    IDAllocator {
        base: AtomicU64::new(base),
    }
}

impl IDAllocator {
    /// Alloc returns a new id (atomic add, returns post-increment value).
    ///
    /// SeqCst 自增后返回回绕的新值；错误类型为 Infallible，对应 Go 恒定 nil error。
    pub fn Alloc(&self) -> Result<u64, std::convert::Infallible> {
        Ok(self.base.fetch_add(1, Ordering::SeqCst).wrapping_add(1))
    }

    /// Rebase implements the IDAllocator interface (no-op in the mock).
    ///
    /// 接口占位：真实实现会向远端校正 base，mock 直接 Ok(())。
    pub fn Rebase(&self) -> Result<(), std::convert::Infallible> {
        Ok(())
    }
}
