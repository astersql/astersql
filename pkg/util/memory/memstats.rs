// Copyright 2018 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 进程内存统计缓存。
//
// 对应 Go `memstats.go`：读取当前进程分配器的已用/保留堆字节，填充
// `heap_alloc`/`heap_inuse`，并缓存最近一次强制刷新结果。

#![allow(non_snake_case)]

#[cfg(target_os = "macos")]
use std::ffi::c_void;
use std::sync::{OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, SystemTime};
#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
use sysinfo::{System, get_current_pid};

/// 建议的最小读间隔（与 Go 侧 300ms 对齐；本文件缓存不强制节流）。
pub const ReadMemInterval: Duration = Duration::from_millis(300);

/// 一次内存快照：分配量与 in-use 量（字节）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemStats {
    pub heap_alloc: u64,
    pub heap_inuse: u64,
}

/// 全局缓存条目：时间戳 + 快照。
#[derive(Clone, Copy)]
struct GlobalMemStats {
    timestamp: SystemTime,
    stats: MemStats,
}

/// 进程级 MemStats 单例缓存。
fn cache() -> &'static RwLock<Option<GlobalMemStats>> {
    static STATS: OnceLock<RwLock<Option<GlobalMemStats>>> = OnceLock::new();
    STATS.get_or_init(|| RwLock::new(None))
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) const fn mem_stats_from_allocator(heap_alloc: u64, heap_inuse: u64) -> MemStats {
    MemStats {
        heap_alloc,
        heap_inuse,
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct MallocStatistics {
    blocks_in_use: u32,
    size_in_use: usize,
    max_size_in_use: usize,
    size_allocated: usize,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_statistics(zone: *mut c_void, stats: *mut MallocStatistics);
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[repr(C)]
struct MallInfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn mallinfo2() -> MallInfo2;
}

/// Read allocator-owned and allocator-reserved heap bytes, mirroring Go's
/// `HeapAlloc` and `HeapInuse` distinction as closely as the platform permits.
fn sample_heap() -> MemStats {
    #[cfg(target_os = "macos")]
    {
        let mut sample = MallocStatistics::default();
        // SAFETY: a null zone requests the sum of all process malloc zones;
        // `sample` is a correctly laid-out writable output value. Sampling only
        // the default zone can miss allocations owned by other zones.
        unsafe {
            malloc_zone_statistics(std::ptr::null_mut(), &mut sample);
        }
        return mem_stats_from_allocator(sample.size_in_use as u64, sample.size_allocated as u64);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `mallinfo2` takes no arguments and returns its snapshot by value.
        let sample = unsafe { mallinfo2() };
        return mem_stats_from_allocator(
            sample.uordblks as u64,
            sample.arena.saturating_add(sample.hblkhd) as u64,
        );
    }

    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    {
        let mut system = System::new_all();
        system.refresh_all();
        let resident = get_current_pid()
            .ok()
            .and_then(|pid| system.process(pid))
            .map(|process| process.memory())
            .unwrap_or(0);
        mem_stats_from_allocator(resident, resident)
    }
}

/// 读取缓存；若尚未强制刷新过则触发一次 `ForceReadMemStats`。
pub fn ReadMemStats() -> MemStats {
    if let Some(value) = *read_unpoisoned(cache()) {
        return value.stats;
    }
    ForceReadMemStats()
}

/// 强制刷新：查询当前进程内存并写入缓存。
pub fn ForceReadMemStats() -> MemStats {
    // Go records the timestamp immediately before runtime.ReadMemStats.
    let timestamp = SystemTime::now();
    let stats = sample_heap();
    *write_unpoisoned(cache()) = Some(GlobalMemStats { timestamp, stats });
    stats
}

/// 返回最近一次强制刷新的时间；尚未刷新则为 `None`。
pub fn LastReadTime() -> Option<SystemTime> {
    read_unpoisoned(cache()).map(|value| value.timestamp)
}
