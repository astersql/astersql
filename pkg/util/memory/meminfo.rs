// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 主机与 cgroup 内存信息探针。
//
// 对应 Go `meminfo.go`：可替换的 `MemTotal`/`MemUsed` 函数指针、带 TTL 的缓存，
// 以及在检测到更紧的 cgroup 限制时切换到 cgroup 读数。`InstanceMemUsed` 读进程堆统计。

#![allow(non_snake_case)]

use crate::memstats::ReadMemStats;
use cgroup_crate::{GetMemoryLimit, GetMemoryUsage, InContainer};
use std::error::Error;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use sysinfo::System;

/// 内存探针返回值：字节数或可发送的错误。
pub type MemoryInfoResult = Result<u64, Box<dyn Error + Send + Sync>>;
/// 可热替换的内存探针函数类型（对应 Go 中可被赋值的函数变量）。
pub type MemoryInfoProbe = fn() -> MemoryInfoResult;

/// 当前选用的“内存总量”探针；默认 `MemTotalNormal`，可由 `InitMemoryHook` 切换。
pub static MemTotal: RwLock<MemoryInfoProbe> = RwLock::new(MemTotalNormal);
/// 当前选用的“已用内存”探针；默认 `MemUsedNormal`。
pub static MemUsed: RwLock<MemoryInfoProbe> = RwLock::new(MemUsedNormal);

/// 带时间戳的缓存条目，超时后重新探测。
#[derive(Clone, Copy, Default)]
struct CacheValue {
    value: u64,
    updated: Option<Instant>,
}

/// 总量缓存（默认 TTL 60s）。
fn total_cache() -> &'static RwLock<CacheValue> {
    static CACHE: OnceLock<RwLock<CacheValue>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(CacheValue::default()))
}

/// 已用量缓存（默认 TTL 500ms）。
fn used_cache() -> &'static RwLock<CacheValue> {
    static CACHE: OnceLock<RwLock<CacheValue>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(CacheValue::default()))
}

/// 本进程实例内存用量缓存。
fn process_cache() -> &'static RwLock<CacheValue> {
    static CACHE: OnceLock<RwLock<CacheValue>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(CacheValue::default()))
}

/// 若缓存未过期则返回缓存值。
fn cached(cache: &RwLock<CacheValue>, lifetime: Duration) -> Option<u64> {
    let value = *cache.read().expect("memory info cache poisoned");
    value
        .updated
        .filter(|updated| updated.elapsed() < lifetime)
        .map(|_| value.value)
}

/// 写入缓存值并刷新时间戳。
fn update(cache: &RwLock<CacheValue>, value: u64) {
    *cache.write().expect("memory info cache poisoned") = CacheValue {
        value,
        updated: Some(Instant::now()),
    };
}

/// 通过当前 `MemTotal` 函数指针调用总量探针。
fn call_total() -> MemoryInfoResult {
    let probe = *MemTotal.read().expect("MemTotal lock poisoned");
    probe()
}

/// 通过当前 `MemUsed` 函数指针调用已用探针。
fn call_used() -> MemoryInfoResult {
    let probe = *MemUsed.read().expect("MemUsed lock poisoned");
    probe()
}

/// 调用任意探针；失败时返回 0（忽略错误，供仲裁器等兜底路径使用）。
pub fn get_mem_total_ignore_err_with<F, E>(probe: F) -> u64
where
    F: FnOnce() -> Result<u64, E>,
{
    probe().unwrap_or(0)
}

/// 读取当前 MemTotal，错误时返回 0。
pub fn GetMemTotalIgnoreErr() -> u64 {
    get_mem_total_ignore_err_with(call_total)
}

/// 通过 sysinfo 刷新并返回 (总量, 已用)。
fn system_memory() -> (u64, u64) {
    let mut system = System::new();
    system.refresh_memory();
    (system.total_memory(), system.used_memory())
}

fn mem_total_normal_uncached() -> MemoryInfoResult {
    let total = system_memory().0;
    update(total_cache(), total);
    Ok(total)
}

/// 物理机内存总量探针（60s 缓存）。
pub fn MemTotalNormal() -> MemoryInfoResult {
    if let Some(value) = cached(total_cache(), Duration::from_secs(60)) {
        return Ok(value);
    }
    mem_total_normal_uncached()
}

/// 物理机已用内存探针（500ms 缓存）。
pub fn MemUsedNormal() -> MemoryInfoResult {
    if let Some(value) = cached(used_cache(), Duration::from_millis(500)) {
        return Ok(value);
    }
    let used = system_memory().1;
    update(used_cache(), used);
    Ok(used)
}

/// cgroup 感知的总量：取 cgroup limit 与物理总量的较小值。
pub fn MemTotalCGroup() -> MemoryInfoResult {
    if let Some(value) = cached(total_cache(), Duration::from_secs(60)) {
        return Ok(value);
    }
    // 与 Go 相同，先读取 cgroup；失败时不得用物理内存掩盖错误。
    let limit = GetMemoryLimit()?;
    let physical = system_memory().0;
    let value = limit.min(physical);
    update(total_cache(), value);
    Ok(value)
}

/// cgroup 感知的已用量：取 cgroup usage 与物理已用的较小值。
pub fn MemUsedCGroup() -> MemoryInfoResult {
    if let Some(value) = cached(used_cache(), Duration::from_millis(500)) {
        return Ok(value);
    }
    let usage = GetMemoryUsage()?;
    let physical = system_memory().1;
    let value = usage.min(physical);
    update(used_cache(), value);
    Ok(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemoryHookKind {
    Normal,
    CGroup,
}

/// 对应 Go `InitMemoryHook` 的分支判定；闭包保持探测的惰性与错误传播顺序。
pub(crate) fn select_memory_hook<L, P>(
    in_container: bool,
    cgroup_limit: L,
    physical_total: P,
) -> MemoryHookSelectionResult
where
    L: FnOnce() -> MemoryInfoResult,
    P: FnOnce() -> MemoryInfoResult,
{
    if in_container {
        return Ok(MemoryHookKind::CGroup);
    }
    let cgroup_value = cgroup_limit()?;
    let physical_value = physical_total()?;
    Ok(if cgroup_value != 0 && physical_value > cgroup_value {
        MemoryHookKind::CGroup
    } else {
        MemoryHookKind::Normal
    })
}

type MemoryHookSelectionResult = Result<MemoryHookKind, Box<dyn Error + Send + Sync>>;

/// 启动时根据容器状态或更紧的 cgroup 限制切换内存探针。
pub fn InitMemoryHook() -> MemoryInfoResult {
    let in_container = InContainer();
    let selected = select_memory_hook(
        in_container,
        || GetMemoryLimit().map_err(|error| error.into_boxed_dyn_error()),
        mem_total_normal_uncached,
    )?;
    if selected == MemoryHookKind::CGroup {
        *MemTotal.write().expect("MemTotal lock poisoned") = MemTotalCGroup;
        *MemUsed.write().expect("MemUsed lock poisoned") = MemUsedCGroup;
    }
    // Go 在已处于容器的分支中直接返回，不重复读取 cgroup/物理内存。
    if in_container {
        return Ok(0);
    }
    call_total()?;
    call_used()
}

/// 本 TiDB/AsterSQL 实例进程堆占用（来自 memstats，500ms 缓存）。
pub fn InstanceMemUsed() -> MemoryInfoResult {
    if let Some(value) = cached(process_cache(), Duration::from_millis(500)) {
        return Ok(value);
    }
    let value = ReadMemStats().heap_alloc;
    update(process_cache(), value);
    Ok(value)
}
