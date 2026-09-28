// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software is distributed on an "AS IS" BASIS.

// 会话级 LRU 计划缓存。
//
// 用双端队列维护最近最少使用（LRU）顺序：队头为最近使用，队尾最旧。
// Get 命中会将条目移到队头；Put 超容量或超过 `quota * (1 - guard)` 内存阈值时
// 从队尾驱逐。同一键可按参数类型兼容性区分多条桶（bucket）。

use crate::{CheckTypesCompatibility4PC, PlanCacheValue};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// LRU 队列中的单条缓存条目。
struct LRUPlanCacheEntry {
    key: String,
    value: Arc<PlanCacheValue>,
}

/// 受互斥锁保护的 LRU 缓存内部状态。
#[derive(Default)]
struct LRUPlanCacheState {
    /// 最大条目容量。
    capacity: usize,
    /// 队头最近、队尾最旧的条目队列。
    entries: VecDeque<LRUPlanCacheEntry>,
    /// 键长与计划值内存占用之和。
    memory: i64,
}

/// 线程安全的 LRU 计划缓存，支持容量与内存 guard 双重控制。
pub struct LRUPlanCache {
    /// 内存保护比例：实际阈值为 `quota * (1 - guard)`。
    guard: f64,
    /// 内存配额（字节）；为 0 时不做内存控制。
    quota: u64,
    state: Mutex<LRUPlanCacheState>,
}

/// 创建 LRU 计划缓存；`capacity == 0` 时默认容量为 100。
pub fn NewLRUPlanCache(capacity: usize, guard: f64, quota: u64) -> LRUPlanCache {
    LRUPlanCache {
        guard,
        quota,
        state: Mutex::new(LRUPlanCacheState {
            capacity: if capacity == 0 { 100 } else { capacity },
            ..LRUPlanCacheState::default()
        }),
    }
}

impl LRUPlanCache {
    /// 获取内部状态锁；若锁已毒化则恢复内层状态继续使用。
    fn state(&self) -> MutexGuard<'_, LRUPlanCacheState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 按键与参数类型查找；命中则移到队头（提升为最近使用）。
    pub fn Get(
        &self,
        key: &str,
        parameter_types: &[types_dependency::metadata::FieldType],
    ) -> Option<Arc<PlanCacheValue>> {
        let mut state = self.state();
        let index = state.entries.iter().position(|entry| {
            entry.key == key && CheckTypesCompatibility4PC(&entry.value.ParamTypes, parameter_types)
        })?;
        let entry = state.entries.remove(index)?;
        let value = Arc::clone(&entry.value);
        state.entries.push_front(entry);
        Some(value)
    }

    /// 写入或替换兼容参数类型的条目，并执行容量与内存控制。
    pub fn Put<V>(&self, key: String, value: V)
    where
        V: Into<Arc<PlanCacheValue>>,
    {
        let value = value.into();
        let mut state = self.state();
        // 同键且参数类型兼容则先移除旧条目，避免重复桶。
        if let Some(index) = state.entries.iter().position(|entry| {
            entry.key == key
                && CheckTypesCompatibility4PC(&entry.value.ParamTypes, &value.ParamTypes)
        }) {
            if let Some(old) = state.entries.remove(index) {
                state.memory -= Self::entryMemoryUsage(&old);
            }
            let entry = LRUPlanCacheEntry { key, value };
            state.memory += Self::entryMemoryUsage(&entry);
            state.entries.push_front(entry);
            return;
        }

        let entry = LRUPlanCacheEntry { key, value };
        state.memory += Self::entryMemoryUsage(&entry);
        state.entries.push_front(entry);
        while state.entries.len() > state.capacity {
            Self::removeOldest(&mut state);
        }
        self.memoryControl(&mut state);
    }

    /// 估算单条条目占用：键字节数 + 计划值 MemoryUsage。
    fn entryMemoryUsage(entry: &LRUPlanCacheEntry) -> i64 {
        entry.key.len() as i64 + entry.value.MemoryUsage()
    }

    /// 弹出队尾最旧条目并扣减内存计数。
    fn removeOldest(state: &mut LRUPlanCacheState) {
        if let Some(entry) = state.entries.pop_back() {
            state.memory -= Self::entryMemoryUsage(&entry);
        }
    }

    /// 当内存超过 `quota * (1 - guard)` 时循环驱逐最旧条目。
    fn memoryControl(&self, state: &mut LRUPlanCacheState) {
        if self.quota == 0 || self.guard == 0.0 {
            return;
        }
        let threshold = (self.quota as f64 * (1.0 - self.guard).max(0.0)) as u64;
        while state.memory.max(0) as u64 > threshold && !state.entries.is_empty() {
            Self::removeOldest(state);
        }
    }

    /// 删除指定键的全部参数类型桶。
    pub fn Delete(&self, key: &str) {
        let mut state = self.state();
        let mut removed = 0_i64;
        state.entries.retain(|entry| {
            if entry.key == key {
                removed += Self::entryMemoryUsage(entry);
                false
            } else {
                true
            }
        });
        state.memory -= removed;
    }

    /// 清空全部条目并重置内存计数。
    pub fn DeleteAll(&self) {
        let mut state = self.state();
        state.entries.clear();
        state.memory = 0;
    }

    /// 返回当前条目数。
    pub fn Size(&self) -> usize {
        self.state().entries.len()
    }

    /// 调整容量；为 0 报错，缩小容量时立即驱逐超出部分。
    pub fn SetCapacity(&self, capacity: usize) -> Result<(), String> {
        if capacity == 0 {
            return Err("capacity of LRU cache should be at least 1".into());
        }
        let mut state = self.state();
        state.capacity = capacity;
        while state.entries.len() > capacity {
            Self::removeOldest(&mut state);
        }
        Ok(())
    }

    /// 返回当前估计内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.state().memory
    }

    /// 关闭缓存：等价于 DeleteAll。
    pub fn Close(&self) {
        self.DeleteAll();
    }
}
