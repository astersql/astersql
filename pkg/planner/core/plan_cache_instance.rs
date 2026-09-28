// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 实例级计划缓存（Instance Plan Cache）。
//
// 在进程/实例范围内按 SQL 键缓存多份 `PlanCacheValue`（同一键可因参数类型
// 不同并存），用软/硬内存上限控制容量。淘汰（Evict）按最近使用时间戳
// （单调 use_clock，避免 SystemTime 粗粒度导致 LRU 顺序不确定）优先驱逐最旧项。

use crate::{CheckTypesCompatibility4PC, PlanCacheValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// 缓存中的单条计划条目及其最近使用戳。
struct InstancePlanCacheEntry {
    value: Arc<PlanCacheValue>,
    /// Monotonic use stamp. SystemTime is too coarse on some hosts, which made
    /// LRU eviction order nondeterministic when many touches share one tick.
    /// 单调使用时间戳。部分主机上 SystemTime 粒度过粗，多笔访问落在同一
    /// 时钟滴答时会使 LRU 淘汰顺序不确定。
    last_used: u64,
}

/// 受互斥锁保护的实例缓存内部状态。
#[derive(Default)]
struct InstancePlanCacheState {
    /// 按缓存键分组的计划条目列表。
    items: HashMap<String, Vec<InstancePlanCacheEntry>>,
    /// 当前缓存占用的估计内存字节数。
    memory: i64,
    /// 当前缓存的计划条数。
    plans: usize,
    /// 单调递增的访问时钟，用于更新 last_used。
    use_clock: u64,
}

/// 线程安全的实例级计划缓存，带软/硬内存限制。
pub struct InstancePlanCache {
    state: Mutex<InstancePlanCacheState>,
    /// 软上限：超过后 Evict(false) 会尝试释放到该水位以下。
    soft: AtomicI64,
    /// 硬上限：Put 时若将超过则拒绝写入。
    hard: AtomicI64,
}

/// 以给定软/硬内存上限创建实例级计划缓存。
pub fn NewInstancePlanCache(soft: i64, hard: i64) -> InstancePlanCache {
    InstancePlanCache {
        state: Mutex::new(InstancePlanCacheState::default()),
        soft: AtomicI64::new(soft),
        hard: AtomicI64::new(hard),
    }
}

impl InstancePlanCache {
    /// 获取内部状态锁；若锁已毒化则恢复内层状态继续使用。
    fn state(&self) -> MutexGuard<'_, InstancePlanCacheState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 推进 use_clock 并返回新时间戳。
    fn touch(state: &mut InstancePlanCacheState) -> u64 {
        state.use_clock = state.use_clock.saturating_add(1);
        state.use_clock
    }

    /// 按键与参数类型兼容性查找缓存计划，命中则刷新 last_used。
    pub fn Get(
        &self,
        key: &str,
        parameter_types: &[types_dependency::metadata::FieldType],
    ) -> Option<Arc<PlanCacheValue>> {
        let mut state = self.state();
        let index = state.items.get(key)?.iter().position(|entry| {
            CheckTypesCompatibility4PC(&entry.value.ParamTypes, parameter_types)
        })?;
        state.use_clock = state.use_clock.saturating_add(1);
        let stamp = state.use_clock;
        let entry = &mut state.items.get_mut(key)?[index];
        entry.last_used = stamp;
        Some(Arc::clone(&entry.value))
    }

    /// 写入缓存；超过硬上限或已存在兼容参数类型的条目时返回 false。
    pub fn Put<V>(&self, key: String, value: V) -> bool
    where
        V: Into<Arc<PlanCacheValue>>,
    {
        let value = value.into();
        let mem = value.MemoryUsage();
        let mut state = self.state();
        // 硬上限或同键已有兼容参数类型的条目时拒绝写入。
        if state.memory + mem > self.hard.load(Ordering::Acquire)
            || state.items.get(&key).is_some_and(|entries| {
                entries.iter().any(|entry| {
                    CheckTypesCompatibility4PC(&entry.value.ParamTypes, &value.ParamTypes)
                })
            })
        {
            return false;
        }

        let last_used = Self::touch(&mut state);
        state.memory += mem;
        state.plans += 1;
        state
            .items
            .entry(key)
            .or_default()
            .push(InstancePlanCacheEntry { value, last_used });
        true
    }

    /// 返回缓存中全部计划值的克隆列表。
    pub fn All(&self) -> Vec<Arc<PlanCacheValue>> {
        self.state()
            .items
            .values()
            .flatten()
            .map(|entry| Arc::clone(&entry.value))
            .collect()
    }

    /// 淘汰计划：`all` 为真清空；否则仅在超过软上限时按 LRU 驱逐足够条数。
    pub fn Evict(&self, all: bool) -> usize {
        let mut state = self.state();
        let soft = self.soft.load(Ordering::Acquire);
        if !all && state.memory < soft {
            return 0;
        }

        // 按平均条目大小估算需释放的条数，或 all 时驱逐全部。
        let target = if all {
            state.plans
        } else if state.plans == 0 {
            0
        } else {
            let average = state.memory / state.plans as i64;
            if average <= 0 {
                0
            } else {
                let to_release = state.memory - soft;
                if to_release <= 0 {
                    0
                } else {
                    ((to_release + average - 1) / average) as usize
                }
            }
        };
        if target == 0 {
            return 0;
        }

        // 按 last_used 升序选出最久未用的受害者。
        let mut candidates = state
            .items
            .values()
            .flatten()
            .map(|entry| (entry.last_used, Arc::clone(&entry.value)))
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(last_used, _)| *last_used);
        let victims = candidates
            .into_iter()
            .take(target)
            .map(|(_, value)| value)
            .collect::<Vec<_>>();

        let mut released = 0_i64;
        let mut evicted = 0_usize;
        for entries in state.items.values_mut() {
            entries.retain(|entry| {
                if victims.iter().any(|value| Arc::ptr_eq(value, &entry.value)) {
                    released += entry.value.MemoryUsage();
                    evicted += 1;
                    false
                } else {
                    true
                }
            });
        }
        state.items.retain(|_, entries| !entries.is_empty());
        state.memory -= released;
        state.plans -= evicted;
        evicted
    }

    /// 返回当前估计内存占用。
    pub fn MemUsage(&self) -> i64 {
        self.state().memory
    }

    /// 返回当前缓存计划条数。
    pub fn Size(&self) -> usize {
        self.state().plans
    }

    /// 返回 (软上限, 硬上限)。
    pub fn GetLimits(&self) -> (i64, i64) {
        (
            self.soft.load(Ordering::Acquire),
            self.hard.load(Ordering::Acquire),
        )
    }

    /// 更新软/硬内存上限。
    pub fn SetLimits(&self, soft: i64, hard: i64) {
        self.soft.store(soft, Ordering::Release);
        self.hard.store(hard, Ordering::Release);
    }
}
