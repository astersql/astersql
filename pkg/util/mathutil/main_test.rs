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

//
// 对应 Go `TestMain`：调用 `SetupForCommonTest`，并保留线程泄漏白名单。

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use testsetup::SetupForCommonTest;

/// 保证公共测试初始化只执行一次。
static COMMON_TEST_SETUP: Once = Once::new();
/// 记录 `SetupForCommonTest` 实际调用次数，供断言验证。
static COMMON_TEST_SETUP_CALLS: AtomicUsize = AtomicUsize::new(0);

/// 通过 `Once` 调用 `SetupForCommonTest`，重复调用不会再次执行。
fn setup_for_common_test() {
    COMMON_TEST_SETUP.call_once(|| {
        SetupForCommonTest();
        COMMON_TEST_SETUP_CALLS.fetch_add(1, Ordering::SeqCst);
    });
}
