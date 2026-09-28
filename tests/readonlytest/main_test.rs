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

// 本文件对应 `tests/readonlytest/main_test.rs`，本次任务只补中文解释，不改行为。
// 本文件主要承接 Go TestMain 对应的前置配置和退出语义。
// 阅读重点是 setup 顺序与全局状态初始化。
// 中文注释会强调测试框架层面的职责。
// 这里保持原有 harness 行为不变。
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

// `ACTIVE_OWNED_THREADS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
static ACTIVE_OWNED_THREADS: AtomicUsize = AtomicUsize::new(0);

// `OwnedThreadGuard` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct OwnedThreadGuard;

// 这里实现 `OwnedThreadGuard` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl OwnedThreadGuard {
    // `enter` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn enter() -> Self {
        ACTIVE_OWNED_THREADS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl Drop for OwnedThreadGuard {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn drop(&mut self) {
        ACTIVE_OWNED_THREADS.fetch_sub(1, Ordering::SeqCst);
    }
}

// `verify_test_main` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn verify_test_main(run_suite: impl FnOnce() -> i32) -> i32 {
    let status = run_suite();
    assert_eq!(
        ACTIVE_OWNED_THREADS.load(Ordering::SeqCst),
        0,
        "readonlytest-owned worker leaked past suite completion"
    );
    status
}

// 测试 `test_main` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
#[test]
// `test_main` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_main() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let status = verify_test_main(|| {
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("readonlytest-main-worker".into())
            .spawn(move || {
                let _guard = OwnedThreadGuard::enter();
                done_tx.send("setup-complete").unwrap();
            })
            .unwrap();
        assert_eq!(done_rx.recv().unwrap(), "setup-complete");
        worker.join().expect("owned worker must terminate");
        23
    });
    assert_eq!(status, 23, "TestMain must propagate m.Run status");
}
