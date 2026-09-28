// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 对应 Go `TestStatusVar`：状态变量（status variable）注册与聚合的基础测试。
//
// 状态变量是 `SHOW STATUS` 类接口暴露的运行时指标；本测试验证
// 自定义 `Statistics` 提供者的作用域查询与注册后聚合结果。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::Arc;

use serial_test::serial;
use task_variable::statusvar::{
    DefaultStatusVarScopeFlag, GetStatusVars, RegisterStatistics, SessionVars, Statistics,
    StatisticsError, StatisticsHandle, StatusValue, UnregisterStatistics,
};
use task_variable::vardef;

/// 全局作用域测试状态名。
const TEST_STATUS: &str = "test_status";
/// 会话作用域测试状态名。
const TEST_SESSION_STATUS: &str = "test_session_status";
/// 全局测试状态的字符串取值。
const TEST_STATUS_VAL: &str = "test_status_val";

/// 仅提供固定键值的模拟统计提供者。
struct MockStatistics;

impl Statistics for MockStatistics {
    fn GetScope(&self, status: &str) -> vardef::ScopeFlag {
        if status == TEST_SESSION_STATUS {
            vardef::ScopeSession
        } else {
            *DefaultStatusVarScopeFlag
        }
    }

    fn Stats(
        &self,
        _vars: Option<&SessionVars>,
    ) -> Result<HashMap<String, StatusValue>, StatisticsError> {
        Ok(HashMap::from([(
            TEST_STATUS.to_owned(),
            StatusValue::new(TEST_STATUS_VAL),
        )]))
    }
}

// Go: TestStatusVar.
/// 注册 Mock 后 `GetStatusVars` 应返回对应 Scope 与值，注销后可清理。
#[test]
#[serial]
fn test_status_var() {
    let statistics: StatisticsHandle = Arc::new(MockStatistics);
    assert_eq!(statistics.GetScope(TEST_STATUS), *DefaultStatusVarScopeFlag);
    assert_eq!(
        statistics.GetScope(TEST_SESSION_STATUS),
        vardef::ScopeSession
    );

    RegisterStatistics(statistics.clone());
    let vars = GetStatusVars(None).expect("registered status variables");
    UnregisterStatistics(&statistics);

    let value = vars.get(TEST_STATUS).expect("test status variable");
    assert_eq!(value.Scope, *DefaultStatusVarScopeFlag);
    assert_eq!(
        value.Value.downcast_ref::<&str>().copied(),
        Some(TEST_STATUS_VAL)
    );
}
