// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 死锁历史测试包的公共初始化入口（对应 Go `TestMain` / `SetupForCommonTest`）。
//
// Rust 无 TestMain；各测试调用幂等的 `setup_for_common_test`。

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 保证公共 setup 只执行一次。
static SETUP: Once = Once::new();
/// 记录 setup 实际执行次数，供测试断言幂等性。
static SETUP_COUNT: AtomicUsize = AtomicUsize::new(0);

// These are the Go-only background goroutines ignored by TestMain. Rust's test
// runner joins its own worker threads and none of these Go runtimes exist in the
// native harness, so preserving the list documents the exact boundary without
// inventing a replacement leak detector.

// Rust has no TestMain hook. Every test calls this idempotent setup entry,
// preserving SetupForCommonTest's once-before-tests lifecycle.

/// 幂等公共 setup，对应 Go SetupForCommonTest。
pub fn setup_for_common_test() {
    SETUP.call_once(|| {
        SETUP_COUNT.fetch_add(1, Ordering::SeqCst);
    });
}

/// 返回 `setup_for_common_test` 实际执行次数（期望恒为 1）。
pub fn setup_count() -> usize {
    SETUP_COUNT.load(Ordering::SeqCst)
}
