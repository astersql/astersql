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

use super::memstats::{MemStats, mem_stats_from_allocator};

/// Like Go's gctuner TestMem, a live large allocation must be included in the
/// process heap snapshot, even when macOS places it outside the default zone.
#[test]
fn large_live_allocation_is_included_in_heap_stats() {
    const SIZE: usize = 100 * 1024 * 1024;
    let heap = vec![0x5a_u8; SIZE + 1];
    let stats = super::memstats::ForceReadMemStats();
    assert!(stats.heap_inuse >= SIZE as u64, "{stats:?}");
    assert!(stats.heap_inuse >= stats.heap_alloc, "{stats:?}");
    std::hint::black_box(&heap);
}

#[cfg(target_os = "macos")]
#[test]
fn heap_stats_include_non_default_malloc_zones() {
    const TEST: &str = "memstats_test::heap_stats_include_non_default_malloc_zones";
    if std::env::var("ASTERSQL_ZONE_TEST").as_deref() != Ok(TEST) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("ASTERSQL_ZONE_TEST", TEST)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    use std::ffi::c_void;
    unsafe extern "C" {
        fn malloc_create_zone(start_size: usize, flags: u32) -> *mut c_void;
        fn malloc_zone_malloc(zone: *mut c_void, size: usize) -> *mut c_void;
        fn malloc_destroy_zone(zone: *mut c_void);
    }
    struct Zone(*mut c_void);
    impl Drop for Zone {
        fn drop(&mut self) {
            // SAFETY: this test owns the zone and no allocation escapes it.
            unsafe { malloc_destroy_zone(self.0) };
        }
    }
    const SIZE: usize = 100 * 1024 * 1024;
    // SAFETY: create a private allocator zone and keep it alive through sampling.
    let zone = Zone(unsafe { malloc_create_zone(0, 0) });
    assert!(!zone.0.is_null());
    let before = super::memstats::ForceReadMemStats();
    let allocation = unsafe { malloc_zone_malloc(zone.0, SIZE) };
    assert!(!allocation.is_null());
    // SAFETY: malloc returned SIZE writable bytes owned by zone.
    unsafe { allocation.cast::<u8>().write_bytes(0x5a, SIZE) };
    let after = super::memstats::ForceReadMemStats();
    assert!(
        after.heap_alloc >= before.heap_alloc + SIZE as u64,
        "non-default zone missing: before={before:?}, after={after:?}"
    );
    std::hint::black_box(allocation);
}

#[test]
fn allocator_fields_preserve_heap_alloc_and_heap_inuse_semantics() {
    assert_eq!(
        mem_stats_from_allocator(17, 29),
        MemStats {
            heap_alloc: 17,
            heap_inuse: 29,
        }
    );
}
