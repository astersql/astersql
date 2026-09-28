// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// errmsg 迁移回归测试：`Extend` 与 Go 用例表对齐。
//
// 覆盖正则匹配/未匹配、nil 防御、空配置、非法正则跳过、更长模式优先，
// 以及配置发布并发下消息扩展的安全性。

use super::ERRMSG_CONFIG_TEST_LOCK;
use std::sync::{Arc, Barrier, MutexGuard};
use util_errmsg::{Config, ErrorMessageExtension, Extend, config, parser::mysql::error::SQLError};

/// RAII 配置作用域：持锁安装扩展规则，Drop 时恢复原全局配置。
struct ConfigScope {
    _lock: MutexGuard<'static, ()>,
    original: Config,
}

impl ConfigScope {
    /// 安装给定 `(pattern, suffix)` 扩展列表，返回作用域守卫。
    fn install(extensions: &[(&str, &str)]) -> Self {
        let lock = ERRMSG_CONFIG_TEST_LOCK
            .lock()
            .expect("config test lock poisoned");
        let original = config::get_global_config().as_ref().clone();
        let mut configured = original.clone();
        configured.error_msg_extension = extensions
            .iter()
            .map(|(pattern, suffix)| ErrorMessageExtension::new(*pattern, *suffix))
            .collect();
        config::store_global_config(configured);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for ConfigScope {
    fn drop(&mut self) {
        config::store_global_config(self.original.clone());
    }
}

/// 直接构造固定 Code/State 的 `SQLError`，便于与 Go 消息表比对。
fn sql_error(message: &str) -> SQLError {
    SQLError {
        Code: 1105,
        Message: message.to_owned(),
        State: "HY000".to_owned(),
    }
}

/// 与 Go 用例表对齐：匹配则追加文档后缀，未匹配则保持原消息。
#[test]
fn migration_extend_by_regex_matches_go_table() {
    let _scope = ConfigScope::install(&[
        (
            r"^Access denied for user '.+'@'.+' \(using password: (YES|NO)\)$",
            "see https://docs.pingcap.com/tidbcloud/select-cluster-tier#user-name-prefix for more details",
        ),
        (
            r"^require_secure_transport can not be set to ON with SEM\(security enhanced mode\) enabled$",
            "see https://docs.pingcap.com/tidbcloud/secure-connections-to-serverless-tier-clusters for more details",
        ),
        (
            r"^sleep\(\) argument is greater than [0-9]+$",
            "see https://docs.pingcap.com/tidbcloud/serverless-tier-limitations#sql for more details",
        ),
        (
            r"^[A-Z ]+ command denied to user '[^']+'@'[^']+' for table '[^']+'$",
            "see https://docs.pingcap.com/tidbcloud/limited-sql-features#system-tables for more details",
        ),
        (
            r"^Access denied; you need \(at least one of\) the RESTRICTED_VARIABLES_ADMIN privilege\(s\) for this operation$",
            "see https://docs.pingcap.com/tidbcloud/limited-sql-features#system-variables for more details",
        ),
        (
            r"^Feature '.+' is not supported when security enhanced mode is enabled$",
            "see https://docs.pingcap.com/tidbcloud/limited-sql-features#statements for more details",
        ),
        (r"^Error message\.$", "suffix."),
        (r"^Error message without period$", "suffix"),
        (r"^Error message with multiple periods\.\.\.$", "suffix..."),
        (r"^Error message with empty suffix$", ""),
    ]);
    let cases = [
        (
            "Access denied for user 'root.foo'@'127.0.0.1' (using password: YES)",
            "Access denied for user 'root.foo'@'127.0.0.1' (using password: YES), see https://docs.pingcap.com/tidbcloud/select-cluster-tier#user-name-prefix for more details.",
        ),
        (
            "sleep() argument is greater than 31536000",
            "sleep() argument is greater than 31536000, see https://docs.pingcap.com/tidbcloud/serverless-tier-limitations#sql for more details.",
        ),
        (
            "require_secure_transport can not be set to ON with SEM(security enhanced mode) enabled",
            "require_secure_transport can not be set to ON with SEM(security enhanced mode) enabled, see https://docs.pingcap.com/tidbcloud/secure-connections-to-serverless-tier-clusters for more details.",
        ),
        (
            "Exceeded resource group quota limitation",
            "Exceeded resource group quota limitation",
        ),
        (
            "Feature 'SELECT INTO' is not supported when security enhanced mode is enabled",
            "Feature 'SELECT INTO' is not supported when security enhanced mode is enabled, see https://docs.pingcap.com/tidbcloud/limited-sql-features#statements for more details.",
        ),
        (
            "SELECT command denied to user 'u'@'%' for table 'tidb'",
            "SELECT command denied to user 'u'@'%' for table 'tidb', see https://docs.pingcap.com/tidbcloud/limited-sql-features#system-tables for more details.",
        ),
        (
            "Access denied; you need (at least one of) the RESTRICTED_VARIABLES_ADMIN privilege(s) for this operation",
            "Access denied; you need (at least one of) the RESTRICTED_VARIABLES_ADMIN privilege(s) for this operation, see https://docs.pingcap.com/tidbcloud/limited-sql-features#system-variables for more details.",
        ),
        (
            "Table 'test.t' doesn't exist",
            "Table 'test.t' doesn't exist",
        ),
        ("Error message.", "Error message, suffix."),
        (
            "Error message without period",
            "Error message without period, suffix.",
        ),
        (
            "Error message with multiple periods...",
            "Error message with multiple periods, suffix.",
        ),
        (
            "Error message with empty suffix",
            "Error message with empty suffix",
        ),
    ];

    for (message, expected) in cases {
        let mut error = sql_error(message);
        Extend(Some(&mut error));
        assert_eq!(error.Message, expected, "message: {message}");
    }
}

/// `Extend(None)` 对应 Go nil，不得 panic。
#[test]
fn migration_extend_accepts_nil() {
    let _scope = ConfigScope::install(&[(".*", "unused")]);
    Extend(None);
}

/// 空扩展列表时消息保持不变。
#[test]
fn migration_extend_without_config_keeps_message() {
    let _scope = ConfigScope::install(&[]);
    let mut error = sql_error("Exceeded resource group quota limitation");
    Extend(Some(&mut error));
    assert_eq!(error.Message, "Exceeded resource group quota limitation");
}

/// 非法正则跳过，后续合法规则仍生效。
#[test]
fn migration_extend_skips_invalid_regex() {
    let _scope = ConfigScope::install(&[
        ("[", "invalid regex"),
        (r"^sleep\(\) argument is greater than [0-9]+$", "see docs"),
    ]);
    let mut error = sql_error("sleep() argument is greater than 31536000");
    Extend(Some(&mut error));
    assert_eq!(
        error.Message,
        "sleep() argument is greater than 31536000, see docs."
    );
}

/// 更具体的用户拒绝访问模式优先于泛化的 `Access denied`。
#[test]
fn migration_extend_prefers_longest_pattern() {
    let _scope = ConfigScope::install(&[
        (r"^Access denied", "generic access denied message"),
        (
            r"^Access denied for user '.+'@'.+' \(using password: (YES|NO)\)$",
            "specific user prefix message",
        ),
    ]);
    let mut error =
        sql_error("Access denied for user 'root.foo'@'127.0.0.1' (using password: YES)");
    Extend(Some(&mut error));
    assert_eq!(
        error.Message,
        "Access denied for user 'root.foo'@'127.0.0.1' (using password: YES), specific user prefix message."
    );
}

/// 配置写线程与 Extend/原子字段写线程同步并发时，扩展结果仍以具体后缀结尾。
#[test]
fn migration_extend_is_safe_during_config_publication() {
    let _scope = ConfigScope::install(&[
        (r"^Access denied", "generic access denied message"),
        (
            r"^Access denied for user '.+'@'.+' \(using password: (YES|NO)\)$",
            "specific user prefix message",
        ),
    ]);
    let published = Arc::new(config::get_global_config().as_ref().clone());
    let start = Arc::new(Barrier::new(4));

    let writer_config = Arc::clone(&published);
    let writer_start = Arc::clone(&start);
    let writer = std::thread::spawn(move || {
        writer_start.wait();
        for _ in 0..1000 {
            config::store_global_config(writer_config.as_ref().clone());
        }
    });

    let extender_start = Arc::clone(&start);
    let extender = std::thread::spawn(move || {
        extender_start.wait();
        for _ in 0..1000 {
            let mut error =
                sql_error("Access denied for user 'root.foo'@'127.0.0.1' (using password: YES)");
            Extend(Some(&mut error));
            assert!(error.Message.ends_with(", specific user prefix message."));
        }
    });

    let atomic_start = Arc::clone(&start);
    let atomic = std::thread::spawn(move || {
        atomic_start.wait();
        for i in 0..1000 {
            config::get_global_config()
                .instance
                .tidb_enable_ddl
                .store(i % 2 == 0);
        }
    });

    start.wait();
    writer.join().expect("config writer panicked");
    extender.join().expect("message extender panicked");
    atomic.join().expect("atomic config writer panicked");
}
