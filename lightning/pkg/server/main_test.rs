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

//! Rust harness equivalent of Go's `TestMain`.
//!
//!
//! 这个文件只验证测试主入口的控制语义，而不重复覆盖业务逻辑。
//! Go 的 `TestMain` 会决定退出码与清理顺序；Rust 则需要把这层约束转成普通函数测试。
//! 核心关注点是：无论 suite 返回什么状态，清理逻辑都必须先执行。
//! 因此这里保护的是测试基建契约，而不是某个具体功能分支。
//! 阅读时可以把它看成 server 测试体系的保险丝。

use std::sync::atomic::{AtomicBool, Ordering};

/// Rust's test harness owns process exit, so return the suite status after
/// deterministic cleanup instead of calling `os.Exit` inside the test process.
fn run_test_main(run_suite: impl FnOnce() -> i32, cleanup: impl FnOnce()) -> i32 {
    let status = run_suite();
    cleanup();
    status
}

#[test]
fn test_main_propagates_suite_status_after_cleanup() {
    let ran = AtomicBool::new(false);
    let cleaned = AtomicBool::new(false);
    let status = run_test_main(
        || {
            ran.store(true, Ordering::SeqCst);
            23
        },
        || cleaned.store(true, Ordering::SeqCst),
    );
    assert!(ran.load(Ordering::SeqCst));
    assert!(cleaned.load(Ordering::SeqCst));
    assert_eq!(status, 23);
}
