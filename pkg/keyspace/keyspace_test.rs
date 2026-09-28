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

// Keyspace 包单元测试：配置读取、空名称判定与用户名前缀策略。
//
// 对应 Go 的 `keyspace_test.go`。因 Rust 生产路径用 `OnceLock` 缓存名称字节，
// 而 Go 测试可重置 `sync.Once`，敏感用例通过子进程隔离执行，避免缓存污染。

use std::process::Command;

use crate::{
    GetKeyspaceNameBySettings, GetKeyspaceNameBytesBySettings, GetUsernamePolicy,
    IsKeyspaceNameEmpty, config, deploymode, kerneltype,
};
use exeerrors_dependency::{errno::ErrUsername, exeerrors::ErrUserNameNeedPrefix};

/// Go resets the package `sync.Once` in each test. Rust's production cache is a
/// `OnceLock`, so run cache-sensitive bodies in a fresh copy of this test binary.
///
/// 在子进程中运行对 `OnceLock` 敏感的测试体，等价于 Go 每次重置 `sync.Once`。
fn run_isolated(test_name: &str, body: impl FnOnce()) {
    /// 子进程识别标记：环境变量值等于当前测试名时直接执行 body。
    const CHILD_TEST: &str = "TIDB_KEYSPACE_CHILD_TEST";
    // 已在隔离子进程中：直接跑断言体并返回。
    if std::env::var(CHILD_TEST).as_deref() == Ok(test_name) {
        body();
        return;
    }

    // 父进程：以相同测试二进制再启动一次，注入 CHILD_TEST 环境变量。
    let status = Command::new(std::env::current_exe().expect("locate current test binary"))
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_TEST, test_name)
        .status()
        .expect("run isolated keyspace test");
    assert!(status.success(), "isolated test {test_name} failed");
}

// TestSetKeyspaceNameInConf verifies that the configured name is returned and
// that the lazily cached byte representation uses the same value.
/// 验证配置中的 keyspace 名称可读，且惰性缓存的字节表示与之一致（NextGen）。
#[test]
fn test_set_keyspace_name_in_conf() {
    run_isolated("keyspace_test::test_set_keyspace_name_in_conf", || {
        config::update_global(|conf| conf.keyspace_name.clear());

        let keyspace_name_in_cfg = "test_keyspace_cfg";
        config::update_global(|conf| conf.keyspace_name = keyspace_name_in_cfg.to_owned());

        let get_keyspace_name = GetKeyspaceNameBySettings();
        assert_eq!(keyspace_name_in_cfg, get_keyspace_name);
        assert!(!IsKeyspaceNameEmpty(&get_keyspace_name));

        let get_keyspace_name_bytes = GetKeyspaceNameBytesBySettings();
        // Classic 构建字节缓存始终为空；NextGen 应与配置字符串一致。
        if kerneltype::IsNextGen() {
            assert_eq!(keyspace_name_in_cfg.as_bytes(), get_keyspace_name_bytes);
        } else {
            assert!(get_keyspace_name_bytes.is_empty());
        }
    });
}

// TestNoKeyspaceNameSet covers the empty-name predicate and cached bytes.
/// 覆盖未设置 keyspace 名称时的空串判定与空字节缓存。
#[test]
fn test_no_keyspace_name_set() {
    run_isolated("keyspace_test::test_no_keyspace_name_set", || {
        config::update_global(|conf| conf.keyspace_name.clear());

        let get_keyspace_name = GetKeyspaceNameBySettings();
        assert_eq!("", get_keyspace_name);
        assert!(IsKeyspaceNameEmpty(&get_keyspace_name));

        let get_keyspace_name_bytes = GetKeyspaceNameBytesBySettings();
        assert!(get_keyspace_name_bytes.is_empty());
    });
}

// TestUsernamePolicy preserves the default-policy and NextGen Starter-policy
// branches from the Go test, including restoration of global state.
/// 保留 Go 测试中的默认策略与 NextGen Starter 前缀策略分支，并恢复全局状态。
#[test]
fn test_username_policy() {
    run_isolated("keyspace_test::test_username_policy", || {
        let restore_config = config::restore_func();
        let original_mode = deploymode::Get();
        config::update_global(|conf| conf.keyspace_name = "ks".to_owned());

        // 非 Starter：默认策略放行任意用户名，不生成变体。
        let mut policy = GetUsernamePolicy();
        assert!(policy.ValidateUsername("user").is_ok());
        assert!(policy.GetUsernameVariants("user").is_empty());
        assert!(policy.GetOriginalUsername("ks.user").is_empty());

        // NextGen + Starter：强制 `keyspace.` 前缀，并校验格式/变体/去前缀。
        if kerneltype::IsNextGen() {
            deploymode::Set(deploymode::Starter).expect("set Starter deploy mode");
            policy = GetUsernamePolicy();
            assert!(policy.ValidateUsername("ks.user").is_ok());
            assert!(policy.ValidateUsernameFormat("other.user"));
            assert!(!policy.ValidateUsernameFormat("other.user.extra"));

            let error = policy
                .ValidateUsername("user")
                .expect_err("unprefixed username must be rejected");
            assert!(ErrUserNameNeedPrefix.Equal(Some(&error)));
            let root = exeerrors_dependency::errors::Cause(Some(&error))
                .expect("username error must have a root cause");
            let normalized = root
                .downcast_ref::<exeerrors_dependency::errors::Error>()
                .expect("username error must be normalized");
            assert_eq!(normalized.Code(), ErrUsername as i32);
            assert_eq!(normalized.RFCCode(), format!("ddl:{ErrUsername}"));
            assert_eq!(vec!["ks.user"], policy.GetUsernameVariants("user"));
            assert!(policy.GetUsernameVariants("ks.user").is_empty());
            assert_eq!(
                vec!["ks.other.user"],
                policy.GetUsernameVariants("other.user")
            );
            assert_eq!(
                vec!["ks.other.user.extra"],
                policy.GetUsernameVariants("other.user.extra")
            );
            assert_eq!("user", policy.GetOriginalUsername("ks.user"));

            deploymode::Set(original_mode).expect("restore deploy mode");
        }
        restore_config();
    });
}

// BenchmarkGetKeyspaceNameBytesBySettings's hot loop. Cargo's stable test
// harness has no Go-style benchmark runner, so callers provide the iteration
// count while retaining the production call performed on every iteration.
/// 对应 Go 基准热循环：调用方提供迭代次数，每轮调用生产路径的字节缓存读取。
pub fn benchmark_get_keyspace_name_bytes_by_settings(iterations: usize) -> &'static [u8] {
    // Classic 无有效缓存，直接返回空切片。
    if !kerneltype::IsNextGen() {
        return &[];
    }

    config::update_global(|conf| conf.keyspace_name = "benchmark_keyspace".to_owned());
    let mut result = &[][..];
    for _ in 0..iterations {
        result = GetKeyspaceNameBytesBySettings();
    }
    result
}
