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

// MockGlobalAccessor 的单元测试：非法变量、校验失败、合法写入与 TiDB 表值读取。

#![allow(non_snake_case)]

use task_variable::mock_globalaccessor::NewMockGlobalAccessor4Tests;
use task_variable::vardef;

// Go: TestMockAPI.
/// 对齐 Go `TestMockAPI`：未知变量报错、校验拒绝、合法 Set/SetOnly 与 GC life time 读取。
#[test]
fn test_mock_api() {
    let mut mock = NewMockGlobalAccessor4Tests([
        (
            vardef::DefaultAuthPlugin.to_owned(),
            "mysql_native_password".to_owned(),
        ),
        ("tikv_gc_life_time".to_owned(), "10m0s".to_owned()),
    ]);
    // 未知系统变量：Get/Set/SetOnly 均应失败。
    let result = mock.GetGlobalSysVar("illegalopt", None);
    assert_eq!(
        result.expect_err("illegal sysvar must fail"),
        "Unknown system variable 'illegalopt'"
    );

    let result = mock.SetGlobalSysVar(
        "illegalopt",
        "val",
        |value| Ok(value.to_owned()),
        |_| Ok(()),
    );
    assert!(result.is_err());
    assert!(mock.SetGlobalSysVarOnly("illegalopt", "val", true).is_err());

    // validate 回调拒绝非法认证插件。
    let result = mock.SetGlobalSysVar(
        vardef::DefaultAuthPlugin,
        "invalidvalue",
        |value| {
            if value == "mysql_native_password" {
                Ok(value.to_owned())
            } else {
                Err(format!("invalid authentication plugin: {value}"))
            }
        },
        |_| Ok(()),
    );
    assert!(result.is_err());

    mock.SetGlobalSysVar(
        vardef::DefaultAuthPlugin,
        "mysql_native_password",
        |value| Ok(value.to_owned()),
        |_| Ok(()),
    )
    .expect("valid global sysvar value");
    mock.SetGlobalSysVarOnly(vardef::DefaultAuthPlugin, "mysql_native_password", true)
        .expect("valid global sysvar-only value");

    assert_eq!(
        mock.GetTiDBTableValue("tikv_gc_life_time")
            .expect("mock TiDB table value"),
        "10m0s"
    );

    // Go 从系统变量注册表读取该值，而不是从可变的全局值缓存读取。
    mock.SetGlobalSysVarOnly("tikv_gc_life_time", "20m0s", true)
        .expect("known GC lifetime variable");
    assert_eq!(
        mock.GetTiDBTableValue("tikv_gc_life_time")
            .expect("registry default remains authoritative"),
        "10m0s"
    );
}

/// 确保 MockGlobalAccessor 满足 Send + Sync，可在多线程测试中共享。
#[test]
fn mock_global_accessor_remains_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<task_variable::mock_globalaccessor::MockGlobalAccessor>();
}

/// Go 在 GC 生命周期注册项意外缺失时会 panic，而不是返回普通错误。
#[test]
#[should_panic(expected = "Get SysVar Failed")]
fn missing_gc_lifetime_registry_entry_panics() {
    let mock = NewMockGlobalAccessor4Tests(std::iter::empty());
    let _ = mock.GetTiDBTableValue("tikv_gc_life_time");
}
