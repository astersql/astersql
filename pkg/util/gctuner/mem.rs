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

// 内存占用读取：为 GOGC / memory-limit 调谐提供当前堆使用量。
//
// 对应 Go `runtime.ReadMemStats` 的 `HeapInuse`，复用 memory 包的跨平台分配器统计。

#[cfg(target_os = "macos")]
use std::ffi::c_void;
use task_memory::memstats::ForceReadMemStats;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_default_zone() -> *mut c_void;
    fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// 读取当前进程“堆占用”字节数，供调谐器计算 GOGC 与判定是否触达 memory limit。
pub fn readMemoryInuse() -> u64 {
    ForceReadMemStats().heap_inuse
}

/// 请求系统分配器立即归还尽可能多的空闲页，作为 Rust 的真实回收边界。
pub fn releaseUnusedMemory() -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the default zone is owned by the process; pressure relief is
        // explicitly thread-safe in the malloc zone API.
        unsafe {
            malloc_zone_pressure_relief(malloc_default_zone(), 0);
        }
        return true;
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: malloc_trim is the glibc process allocator's thread-safe trim
        // operation and does not retain the supplied value.
        unsafe {
            malloc_trim(0);
        }
        return true;
    }
    #[allow(unreachable_code)]
    false
}
