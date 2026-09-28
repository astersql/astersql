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

// `cteutil` 包级测试入口配置的回归检查。
//
// 并确认公共 setup 只执行一次。

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 保证公共测试 setup 只跑一次。
static COMMON_TEST_SETUP: Once = Once::new();
/// 记录 setup 实际执行次数，供断言。
static COMMON_TEST_SETUP_CALLS: AtomicUsize = AtomicUsize::new(0);

/// 等价 Go TestMain 中的 common setup：仅首次调用生效。
fn setup_for_common_test() {
    COMMON_TEST_SETUP.call_once(|| {
        COMMON_TEST_SETUP_CALLS.fetch_add(1, Ordering::SeqCst);
    });
}
