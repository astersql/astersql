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

// 系统 session 测试的通用初始化。

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_testkit_testsetup::SetupForCommonTest;

static COMMON_TEST_SETUP: Once = Once::new();
static COMMON_TEST_SETUP_CALLS: AtomicUsize = AtomicUsize::new(0);
/// 执行公共初始化，且进程内只执行一次。
pub fn test_main() {
    COMMON_TEST_SETUP.call_once(|| {
        SetupForCommonTest();
        COMMON_TEST_SETUP_CALLS.fetch_add(1, Ordering::SeqCst);
    });
}

fn common_setup_calls() -> usize {
    COMMON_TEST_SETUP_CALLS.load(Ordering::SeqCst)
}
/// 验证 CancellationToken 取消后保持单调为真（重复 cancel 无副作用）。
#[test]
fn cancellation_token_records_cancellation_monotonically() {
    // 同一真实测试入口执行通用初始化。
    test_main();
    let token = crate::CancellationToken::default();
    assert!(!token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
}

/// Go `TestMain` 的公共初始化必须真实执行，且进程内只执行一次。
#[test]
fn test_main_runs_common_setup_once() {
    test_main();
    test_main();
    assert_eq!(common_setup_calls(), 1);
}
