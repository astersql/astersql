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

// GC Worker 测试入口（对应 Go 包级 `TestMain`）。Rust libtest 没有 Go 的
// 进程级 runner，因此执行可移植的公共初始化并保留 MVCCLevelDB 收尾等待。

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use astersql_testkit_testsetup::SetupForCommonTest;

static COMMON_TEST_SETUP: Once = Once::new();
static COMMON_TEST_SETUP_CALLS: AtomicUsize = AtomicUsize::new(0);

const MVCC_LEVELDB_CLOSE_WAIT: Duration = Duration::from_secs(1);

fn common_setup_calls() -> usize {
    COMMON_TEST_SETUP_CALLS.load(Ordering::SeqCst)
}

#[test]
fn common_setup_and_cleanup_wait_are_preserved() {
    COMMON_TEST_SETUP.call_once(|| {
        SetupForCommonTest();
        COMMON_TEST_SETUP_CALLS.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(common_setup_calls(), 1);
    assert_eq!(MVCC_LEVELDB_CLOSE_WAIT, Duration::from_secs(1));
}
use crate::gc_worker::{
    gcDefaultAutoConcurrency, gcDefaultConcurrency, gcDefaultEnableValue, gcModeDefault,
    gcModeDistributed,
};

/// 断言 Go TestMain 依赖的 GC 默认常量在 Rust 侧保持一致。
#[test]
fn go_test_main_defaults_are_preserved() {
    assert_eq!(2, gcDefaultConcurrency);
    assert!(gcDefaultAutoConcurrency);
    assert!(gcDefaultEnableValue);
    assert_eq!(gcModeDistributed, gcModeDefault);
}
