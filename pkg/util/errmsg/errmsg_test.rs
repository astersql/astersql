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

// errmsg 单元测试：正则扩展、空配置、非法正则、最长模式与并发安全。
//
// 通过 `ConfigScope` 串行化全局配置写入，覆盖文档后缀追加、句点规范化，
// 以及与 `store_global_config` 并发时的消息扩展正确性。

use super::ERRMSG_CONFIG_TEST_LOCK;
use astersql_errors::ErrorArg;
use std::sync::{Arc, Barrier, MutexGuard};
use util_errmsg::{
    Config, ErrorMessageExtension, Extend, config,
    parser::mysql::error::{NewErrf, SQLError},
};

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

/// 构造 Code=1105、消息为给定字符串的测试用 `SQLError`。
fn sql_error(message: &str) -> SQLError {
    NewErrf(1105, "%s", &[], vec![ErrorArg::String(message.to_owned())])
}

/// 表驱动：多种云端/权限类错误消息应按正则追加对应文档后缀。
#[test]
fn test_extend_by_regex() {
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
            "user prefix",
            "Access denied for user 'root.foo'@'127.0.0.1' (using password: YES)",
            "Access denied for user 'root.foo'@'127.0.0.1' (using password: YES), see https://docs.pingcap.com/tidbcloud/select-cluster-tier#user-name-prefix for more details.",
        ),
        (
            "max sleep seconds",
            "sleep() argument is greater than 31536000",
            "sleep() argument is greater than 31536000, see https://docs.pingcap.com/tidbcloud/serverless-tier-limitations#sql for more details.",
        ),
        (
            "require secure transport",
            "require_secure_transport can not be set to ON with SEM(security enhanced mode) enabled",
            "require_secure_transport can not be set to ON with SEM(security enhanced mode) enabled, see https://docs.pingcap.com/tidbcloud/secure-connections-to-serverless-tier-clusters for more details.",
        ),
        (
            "resource unit",
            "Exceeded resource group quota limitation",
            "Exceeded resource group quota limitation",
        ),
        (
            "serverless not support",
            "Feature 'SELECT INTO' is not supported when security enhanced mode is enabled",
            "Feature 'SELECT INTO' is not supported when security enhanced mode is enabled, see https://docs.pingcap.com/tidbcloud/limited-sql-features#statements for more details.",
        ),
        (
            "invisible table",
            "SELECT command denied to user 'u'@'%' for table 'tidb'",
            "SELECT command denied to user 'u'@'%' for table 'tidb', see https://docs.pingcap.com/tidbcloud/limited-sql-features#system-tables for more details.",
        ),
        (
            "invisible sysvar",
            "Access denied; you need (at least one of) the RESTRICTED_VARIABLES_ADMIN privilege(s) for this operation",
            "Access denied; you need (at least one of) the RESTRICTED_VARIABLES_ADMIN privilege(s) for this operation, see https://docs.pingcap.com/tidbcloud/limited-sql-features#system-variables for more details.",
        ),
        (
            "unmatched",
            "Table 'test.t' doesn't exist",
            "Table 'test.t' doesn't exist",
        ),
        (
            "message ending with period",
            "Error message.",
            "Error message, suffix.",
        ),
        (
            "message and suffix without period",
            "Error message without period",
            "Error message without period, suffix.",
        ),
        (
            "message and suffix ending with multiple periods",
            "Error message with multiple periods...",
            "Error message with multiple periods, suffix.",
        ),
        (
            "empty suffix",
            "Error message with empty suffix",
            "Error message with empty suffix",
        ),
    ];

    for (name, message, expected) in cases {
        let mut error = sql_error(message);
        Extend(Some(&mut error));
        assert_eq!(error.Message, expected, "case: {name}");
    }
}

/// 无扩展配置时，`Extend` 不得改动原消息。
#[test]
fn test_extend_without_config() {
    let _scope = ConfigScope::install(&[]);
    let mut error = sql_error("Exceeded resource group quota limitation");
    Extend(Some(&mut error));
    assert_eq!(error.Message, "Exceeded resource group quota limitation");
}

/// 非法正则应被跳过，后续合法规则仍可匹配并追加后缀。
#[test]
fn test_extend_skips_invalid_regex() {
    let _scope = ConfigScope::install(&[
        ("[", "invalid regex"),
        (
            r"^sleep\(\) argument is greater than [0-9]+$",
            "see https://docs.pingcap.com/tidbcloud/serverless-tier-limitations#sql for more details",
        ),
    ]);
    let mut error = sql_error("sleep() argument is greater than 31536000");
    Extend(Some(&mut error));
    assert_eq!(
        error.Message,
        "sleep() argument is greater than 31536000, see https://docs.pingcap.com/tidbcloud/serverless-tier-limitations#sql for more details."
    );
}

/// 多条规则均可匹配时，优先应用更长/更具体的模式（与配置排序/匹配语义一致）。
#[test]
fn test_extend_prefers_longest_pattern() {
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

/// 并发：一边反复 `store_global_config`，一边 `Extend`，一边改原子配置项，结果仍正确。
#[test]
fn test_extend_concurrent_with_store_global_config() {
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
            assert_eq!(
                error.Message,
                "Access denied for user 'root.foo'@'127.0.0.1' (using password: YES), specific user prefix message."
            );
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
