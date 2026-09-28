// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// Lightning KV 编码用的自增/自随机/行号分配器。
//
// 导入过程中需要跟踪 AUTO_RANDOM、AUTO_INCREMENT、隐式 RowID 已用到的最大值（base），
// 以便事后 rebase 表元数据中的自增游标，避免与在线写入冲突。本模块提供线程安全的
// `panickingAllocator` 与聚合容器 `Allocators`。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// 分配器种类：自随机、自增、隐式行号（RowID）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AllocatorType {
    AutoRandomType,
    AutoIncrementType,
    RowIDAllocType,
}

/// 仅维护 base（当前已见到的最大 ID）的轻量分配器；不实际发放新 ID。
///
/// 名称中的 panicking 对应 Go 侧“缺实现则 panic”的占位语义；此处用 CAS 单调抬升 base。
pub struct panickingAllocator {
    base: AtomicI64,
    ty: AllocatorType,
}

impl panickingAllocator {
    /// 将 base 单调 rebase 到 `newBase`（仅当 newBase 更大时生效）；`_allocIDs` 保留与 Go 签名对齐。
    pub fn Rebase(&self, newBase: i64, _allocIDs: bool) {
        let mut old = self.base.load(Ordering::SeqCst);
        // CAS 循环：并发 rebase 时取更大的值，保证 base 只增不减。
        while newBase > old {
            match self
                .base
                .compare_exchange(old, newBase, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(current) => old = current,
            }
        }
    }

    /// 返回当前 base。
    pub fn Base(&self) -> i64 {
        self.base.load(Ordering::SeqCst)
    }

    /// 返回分配器类型。
    pub fn GetType(&self) -> AllocatorType {
        self.ty
    }
}

/// 三类分配器的集合；`SepAutoInc` 表示是否将自增与 RowID 分配分离。
#[derive(Clone)]
pub struct Allocators {
    pub SepAutoInc: bool,
    values: [Arc<panickingAllocator>; 3],
}

impl Allocators {
    /// 按类型取出对应分配器。
    pub fn Get(&self, ty: AllocatorType) -> Arc<panickingAllocator> {
        self.values
            .iter()
            .find(|allocator| allocator.ty == ty)
            .expect("all allocator types are installed")
            .clone()
    }
}

/// 构造 base 均为 0 的分配器集合。
pub fn NewPanickingAllocators(sepAutoInc: bool) -> Allocators {
    NewPanickingAllocatorsWithBase(sepAutoInc, 0, 0, 0)
}

/// 按给定初始 base 构造三类分配器。
pub fn NewPanickingAllocatorsWithBase(
    sepAutoInc: bool,
    autoRandBase: i64,
    autoIncrBase: i64,
    autoRowIDBase: i64,
) -> Allocators {
    Allocators {
        SepAutoInc: sepAutoInc,
        values: [
            Arc::new(panickingAllocator {
                base: AtomicI64::new(autoRandBase),
                ty: AllocatorType::AutoRandomType,
            }),
            Arc::new(panickingAllocator {
                base: AtomicI64::new(autoIncrBase),
                ty: AllocatorType::AutoIncrementType,
            }),
            Arc::new(panickingAllocator {
                base: AtomicI64::new(autoRowIDBase),
                ty: AllocatorType::RowIDAllocType,
            }),
        ],
    }
}
