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

// testsetup 迁移期单元测试。
//
// 串行化环境变量 `log_level`，验证未设置 noop、合法级别覆盖全局配置、非法值报错。

use std::sync::{Mutex, OnceLock};

use super::{ApplyLogLevel, apply_os_log_level, configured_log_level};

/// 进程级互斥：避免并行测试互相改写 `log_level` 环境变量。
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

/// 未设置 `log_level` 时应返回 Unchanged 且不改全局级别。
#[test]
fn unset_log_level_is_a_noop() {
    let _guard = env_lock();
    unsafe { std::env::remove_var("log_level") };

    assert_eq!(apply_os_log_level().unwrap(), ApplyLogLevel::Unchanged);
}

/// 合法级别（如 debug）应配置成功，且 `configured_log_level` 可读回。
#[test]
fn configured_log_level_replaces_the_global_level() {
    let _guard = env_lock();
    unsafe { std::env::set_var("log_level", "debug") };

    assert_eq!(
        apply_os_log_level().unwrap(),
        ApplyLogLevel::Configured(log::LevelFilter::Debug)
    );
    assert_eq!(configured_log_level(), log::LevelFilter::Debug);

    unsafe { std::env::remove_var("log_level") };
}

/// 非法级别字符串应返回包含原值的初始化错误。
#[test]
fn invalid_log_level_reports_initialization_failure() {
    let _guard = env_lock();
    unsafe { std::env::set_var("log_level", "definitely-not-a-level") };

    let error = apply_os_log_level().unwrap_err();
    assert!(error.to_string().contains("definitely-not-a-level"));

    unsafe { std::env::remove_var("log_level") };
}
