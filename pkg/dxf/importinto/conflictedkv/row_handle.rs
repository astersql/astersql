// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 冲突行句柄（Handle）的有界集合与过滤。
//
// Handle 是行在表内的唯一标识（整数主键或编码后的行 ID）。
// 处理索引冲突时需跳过已处理过的行，以免重复解码；同时用共享
// 原子计数限制内存中缓存的句柄总量，防止冲突规模过大时 OOM。
use std::collections::HashSet;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_kv::Handle;

/// 冲突行句柄 HashSet 的初始容量（条目数，非字节）。
const initMapSizeForConflictedRows: usize = 128;
/// 单条 HashSet 条目的浅层字节估算：Go string header + bool 占位。
///
/// Go 实现使用 `unsafe.Sizeof("")`，对应两字长的字符串视图；
/// Rust `String` 多一个 capacity 字段，不能用它的布局代替 Go 预算公式。
const handleMapEntryShallowSize: i64 = size_of::<&str>() as i64 + size_of::<bool>() as i64;

/// 基于有界句柄集合的过滤器：已收录的 Handle 在后续处理中应跳过。
pub struct HandleFilter {
    /// 共享的有界句柄集合（可跨 collector/handler 引用）。
    set: Arc<BoundedHandleSet>,
}

/// 用给定的有界集合构造句柄过滤器。
pub fn NewHandleFilter(set: Arc<BoundedHandleSet>) -> HandleFilter {
    HandleFilter { set }
}

impl HandleFilter {
    /// 若句柄已在集合中则返回 true，调用方应跳过该行。
    pub fn needSkip(&self, handle: &dyn Handle) -> bool {
        self.set.Contains(handle)
    }
}

/// 有界 Handle 集合：本地 HashSet 存句柄字符串，并通过共享原子计数约束总内存。
pub struct BoundedHandleSet {
    /// 跨多个集合共享的已用字节估算。
    shared_size: Arc<AtomicI64>,
    /// 允许的最大累计字节数；达到后 Add 变为空操作。
    size_limit: i64,
    /// 本实例持有的句柄字符串集合。
    handles: HashSet<String>,
}

/// 创建有界集合；若共享计数已达上限则容量预置为 0，避免无谓分配。
pub fn NewBoundedHandleSet(shared_size: Arc<AtomicI64>, limit: i64) -> BoundedHandleSet {
    // 共享预算已耗尽时不再预留 HashSet 容量。
    let capacity = if shared_size.load(Ordering::Acquire) >= limit {
        0
    } else {
        initMapSizeForConflictedRows
    };
    BoundedHandleSet {
        shared_size,
        size_limit: limit,
        handles: HashSet::with_capacity(capacity),
    }
}

impl BoundedHandleSet {
    /// 加入句柄并累加共享字节；已超限时静默忽略。
    pub fn Add(&mut self, handle: &dyn Handle) {
        // 超限后不再写入，防止内存持续膨胀。
        if self.BoundExceeded() {
            return;
        }
        // 以句柄字符串为键；delta 含字符串内容长度与浅层结构开销。
        let handle = handle.String();
        let delta = handle.len() as i64 + handleMapEntryShallowSize;
        self.shared_size.fetch_add(delta, Ordering::AcqRel);
        self.handles.insert(handle);
    }

    /// 判断句柄是否已在本集合中。
    pub fn Contains(&self, handle: &dyn Handle) -> bool {
        // 空集快速路径，避免无意义的 String 分配。
        if self.handles.is_empty() {
            return false;
        }
        self.handles.contains(&handle.String())
    }

    /// 合并另一集合的句柄（不回写 shared_size；Go 侧同语义）。
    pub fn Merge(&mut self, other: Option<&BoundedHandleSet>) {
        if let Some(other) = other {
            self.handles.extend(other.handles.iter().cloned());
        }
    }

    /// 共享累计字节是否已达到或超过限制。
    pub fn BoundExceeded(&self) -> bool {
        self.shared_size.load(Ordering::Acquire) >= self.size_limit
    }

    /// 本实例本地集合中的句柄个数。
    pub fn Len(&self) -> usize {
        self.handles.len()
    }

    /// 当前共享累计字节估算值。
    pub fn SharedSize(&self) -> i64 {
        self.shared_size.load(Ordering::Acquire)
    }
}
