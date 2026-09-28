// Copyright 2026 AsterSQL.
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

// 状态变量注册表的扩展单元测试（相对 `statusvar_test.rs` 更完整）。
//
// 覆盖：注册/作用域聚合/注销、重复注册逐次移除、默认状态的实时计数与 TLS、
// 空会话保留 Go 默认值，以及提供者错误不被静默吞掉。

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use astersql_sessionctx_variable::statusvar::{
    DefaultStatusVarScopeFlag, GetStatusVars, RegisterStatistics, SessionVars, Statistics,
    StatisticsError, StatisticsHandle, StatusValue, TLS_SUPPORTED_CIPHERS, TLSConnectionState,
    UnregisterStatistics,
};
use astersql_sessionctx_variable::vardef::{
    ConnectAttrsLongestSeen, ConnectAttrsLost, ScopeGlobal, ScopeSession,
};
use serial_test::serial;

/// 全局作用域测试状态名。
const TEST_STATUS: &str = "test_status";
/// 会话作用域测试状态名。
const TEST_SESSION_STATUS: &str = "test_session_status";

/// 返回两个固定状态项的模拟提供者。
struct MockStatistics;

impl Statistics for MockStatistics {
    fn GetScope(&self, status: &str) -> astersql_sessionctx_variable::vardef::ScopeFlag {
        if status == TEST_SESSION_STATUS {
            ScopeSession
        } else {
            *DefaultStatusVarScopeFlag
        }
    }

    fn Stats(
        &self,
        _vars: Option<&SessionVars>,
    ) -> Result<HashMap<String, StatusValue>, StatisticsError> {
        Ok(HashMap::from([
            (TEST_STATUS.to_owned(), StatusValue::new("test_status_val")),
            (TEST_SESSION_STATUS.to_owned(), StatusValue::new(7_u64)),
        ]))
    }
}

/// 始终返回错误的统计提供者，用于验证错误传播。
struct ErrorStatistics;

impl Statistics for ErrorStatistics {
    fn GetScope(&self, _status: &str) -> astersql_sessionctx_variable::vardef::ScopeFlag {
        ScopeGlobal
    }

    fn Stats(
        &self,
        _vars: Option<&SessionVars>,
    ) -> Result<HashMap<String, StatusValue>, StatisticsError> {
        Err(Box::new(io::Error::other("stats failed")))
    }
}

/// 注册提供者并返回句柄，便于测试结束时注销。
fn registered<S: Statistics + 'static>(statistics: S) -> StatisticsHandle {
    let statistics: StatisticsHandle = Arc::new(statistics);
    RegisterStatistics(statistics.clone());
    statistics
}

/// 注册后作用域与聚合值应正确，注销后键应消失。
#[test]
#[serial]
fn registration_scope_aggregation_and_unregister_match_go() {
    let statistics = registered(MockStatistics);

    assert_eq!(statistics.GetScope(TEST_STATUS), *DefaultStatusVarScopeFlag);
    assert_eq!(statistics.GetScope(TEST_SESSION_STATUS), ScopeSession);

    let values = GetStatusVars(None).expect("registered statistics must succeed");
    assert_eq!(values[TEST_STATUS].Scope, *DefaultStatusVarScopeFlag);
    assert_eq!(
        values[TEST_STATUS].Value.downcast_ref::<&str>(),
        Some(&"test_status_val")
    );
    assert_eq!(values[TEST_SESSION_STATUS].Scope, ScopeSession);
    assert_eq!(
        values[TEST_SESSION_STATUS].Value.downcast_ref::<u64>(),
        Some(&7)
    );

    UnregisterStatistics(&statistics);
    assert!(!GetStatusVars(None).unwrap().contains_key(TEST_STATUS));
}

/// 同一句柄重复注册时，`Unregister` 每次只移除最后一次匹配。
#[test]
#[serial]
fn duplicate_registration_is_removed_one_last_match_at_a_time() {
    let statistics: StatisticsHandle = Arc::new(MockStatistics);
    RegisterStatistics(statistics.clone());
    RegisterStatistics(statistics.clone());

    UnregisterStatistics(&statistics);
    assert!(GetStatusVars(None).unwrap().contains_key(TEST_STATUS));

    UnregisterStatistics(&statistics);
    assert!(!GetStatusVars(None).unwrap().contains_key(TEST_STATUS));
}

/// 默认状态应反映实时连接属性计数、会话 KeysExamined 与 TLS 字段。
#[test]
#[serial]
fn default_status_uses_live_counters_session_keys_and_tls_state() {
    let old_longest = ConnectAttrsLongestSeen.Load();
    let old_lost = ConnectAttrsLost.Load();
    ConnectAttrsLongestSeen.Store(321);
    ConnectAttrsLost.Store(9);

    let vars = SessionVars {
        KeysExamined: 88,
        TLSConnectionState: Some(TLSConnectionState {
            CipherSuite: 0x1301,
            Version: 0x0304,
        }),
    };
    let values = GetStatusVars(Some(&vars)).unwrap();

    assert_eq!(values["tidb_keys_examined"].Scope, ScopeSession);
    assert_eq!(
        values["tidb_keys_examined"].Value.downcast_ref::<u64>(),
        Some(&88)
    );
    assert_eq!(
        values["Performance_schema_session_connect_attrs_longest_seen"]
            .Value
            .downcast_ref::<i64>(),
        Some(&321)
    );
    assert_eq!(
        values["Performance_schema_session_connect_attrs_lost"]
            .Value
            .downcast_ref::<i64>(),
        Some(&9)
    );
    assert_eq!(
        values["Ssl_cipher"].Value.to_string(),
        "TLS_AES_128_GCM_SHA256"
    );
    assert_eq!(
        values["Ssl_cipher_list"].Value.to_string(),
        *TLS_SUPPORTED_CIPHERS
    );
    assert!(TLS_SUPPORTED_CIPHERS.ends_with(':'));
    assert_eq!(TLS_SUPPORTED_CIPHERS.matches(':').count(), 25);
    assert!(!TLS_SUPPORTED_CIPHERS.contains("::"));
    assert_eq!(
        values["Ssl_verify_mode"].Value.downcast_ref::<i32>(),
        Some(&5)
    );
    assert_eq!(values["Ssl_version"].Value.to_string(), "TLSv1.3");

    // 恢复全局计数器，避免污染其他串行测试。
    ConnectAttrsLongestSeen.Store(old_longest);
    ConnectAttrsLost.Store(old_lost);
}

/// 无会话上下文时 Ssl_* 与 keys_examined 应保持 Go 侧默认空/零值。
#[test]
#[serial]
fn nil_session_keeps_go_default_values() {
    let values = GetStatusVars(None).unwrap();

    assert_eq!(values["Ssl_cipher"].Value.to_string(), "");
    assert_eq!(values["Ssl_cipher_list"].Value.to_string(), "");
    assert_eq!(
        values["Ssl_verify_mode"].Value.downcast_ref::<i32>(),
        Some(&0)
    );
    assert_eq!(values["Ssl_version"].Value.to_string(), "");
    assert_eq!(
        values["tidb_keys_examined"].Value.downcast_ref::<u64>(),
        Some(&0)
    );
}

/// 提供者返回的错误应向上传播，不被聚合逻辑吞掉。
#[test]
#[serial]
fn provider_errors_are_returned_without_being_silenced() {
    let statistics = registered(ErrorStatistics);
    let error = GetStatusVars(None).expect_err("provider error must stop aggregation");
    assert_eq!(error.to_string(), "stats failed");
    UnregisterStatistics(&statistics);
}
