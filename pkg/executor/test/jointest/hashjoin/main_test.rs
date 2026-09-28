// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Hash Join 测试包级 TestMain 语义。
//
// Rust libtest 没有 Go `TestMain`，因此以一次性 setup 复现 AutoID、全局
// 配置与 failpoint 开关的进程级副作用。

use std::sync::Once;

use astersql_config::{get_global_config, update_global};
use astersql_meta_autoid::{get_step, set_step};

static SETUP: Once = Once::new();

/// 应用 Go TestMain 中在 Rust 侧存在对应物的进程级设置。
pub(crate) fn setup() {
    SETUP.call_once(|| {
        set_step(5_000);
        update_global(|config| {
            config.instance.slow_threshold = 30_000;
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });
    });
}

#[test]
/// 验证 setup 幂等且真实应用 Go TestMain 的可迁移契约。
fn test_main_applies_hash_join_globals() {
    setup();
    setup();

    assert_eq!(get_step(), 5_000);
    let config = get_global_config();
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}
