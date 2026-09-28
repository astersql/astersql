// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 扩展鉴权、函数定义与注册表的 Aster 单元测试。
//
// 对应 Go 侧边界：鉴权插件校验错误文案与保留名、AuthConn 适配器契约、
// FunctionDef 可选参数长度、clear 回调收集顺序，以及注册表按名称排序、
// 会话事件分发与 Reset 幂等行为。

use astersql_extension::*;
use serial_test::serial;
use std::sync::{Arc, Mutex};

/// 模拟 privilege 层原始鉴权连接，记录写出数据与 Flush 上下文。
#[derive(Default)]
struct MockRawAuthConn {
    /// 累计写入的 AuthMoreData 字节。
    written: Vec<u8>,
    /// 每次 Flush 时收到的上下文值。
    flushed_with: Vec<u64>,
}

impl astersql_extension::privilege_conn::RawAuthConn for MockRawAuthConn {
    type Context = u64;
    type Error = std::io::Error;

    fn WriteAuthMoreData(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        self.written.extend_from_slice(data);
        Ok(())
    }

    fn ReadPacket(&mut self) -> Result<Vec<u8>, Self::Error> {
        Ok(b"client-response".to_vec())
    }

    fn Flush(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        self.flushed_with.push(*context);
        Ok(())
    }
}

/// 构造三个核心回调齐全的合法鉴权插件。
fn valid_plugin(name: &str) -> Arc<AuthPlugin> {
    Arc::new(AuthPlugin {
        Name: name.to_owned(),
        AuthenticateUser: Some(Arc::new(|_| Ok(()))),
        GenerateAuthString: Some(Arc::new(|password| (password, true))),
        ValidateAuthString: Some(Arc::new(|_| true)),
        ..AuthPlugin::default()
    })
}

/// 校验失败用例的错误文案与保留名拒绝，以及重复名与合法注册。
#[test]
fn auth_validation_matches_go_errors_and_reserved_names() {
    let cases = [
        (
            Arc::new(AuthPlugin::default()),
            "auth plugin name cannot be empty",
        ),
        (
            Arc::new(AuthPlugin {
                Name: "plugin1".into(),
                GenerateAuthString: Some(Arc::new(|password| (password, true))),
                ValidateAuthString: Some(Arc::new(|_| true)),
                ..AuthPlugin::default()
            }),
            "AuthenticateUser function cannot be nil for plugin1",
        ),
        (
            Arc::new(AuthPlugin {
                Name: "plugin1".into(),
                AuthenticateUser: Some(Arc::new(|_| Ok(()))),
                ValidateAuthString: Some(Arc::new(|_| true)),
                ..AuthPlugin::default()
            }),
            "GenerateAuthString function cannot be nil for plugin1",
        ),
        (
            Arc::new(AuthPlugin {
                Name: "plugin1".into(),
                AuthenticateUser: Some(Arc::new(|_| Ok(()))),
                GenerateAuthString: Some(Arc::new(|password| (password, true))),
                ..AuthPlugin::default()
            }),
            "ValidateAuthString function cannot be nil for plugin1",
        ),
        (
            valid_plugin("mysql_native_password"),
            "reserved name for default auth plugins",
        ),
    ];

    for (plugin, expected) in cases {
        let error = validate_auth_plugins(&[plugin]).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }

    // 同名插件重复注册应失败；单次合法注册应成功。
    let duplicate =
        validate_auth_plugins(&[valid_plugin("plugin1"), valid_plugin("plugin1")]).unwrap_err();
    assert!(
        duplicate
            .to_string()
            .contains("has already been registered")
    );
    validate_auth_plugins(&[valid_plugin("plugin1")]).unwrap();
}

/// AuthConnAdapter 应正确转发写包、读包与带上下文的 Flush。
#[test]
fn auth_conn_adapter_uses_the_migrated_privilege_connection_contract() {
    let mut connection = AuthConnAdapter::new(MockRawAuthConn::default(), 42);
    connection.WriteAuthMoreData(b"challenge").unwrap();
    assert_eq!(connection.ReadPacket().unwrap(), b"client-response");
    connection.Flush().unwrap();

    let connection = connection.into_inner();
    assert_eq!(connection.written, b"challenge");
    assert_eq!(connection.flushed_with, [42]);
}

/// FunctionDef.Validate 保持与 Go 相同的空名与 OptionalArgsLen 边界。
#[test]
fn function_validation_keeps_go_boundaries() {
    let mut definition = FunctionDef::default();
    assert_eq!(
        definition.Validate().unwrap_err().to_string(),
        "extension function name should not be empty"
    );

    definition.Name = "f".into();
    definition.ArgTps = vec![parser_types::types::ETInt];
    definition.OptionalArgsLen = -1;
    assert_eq!(
        definition.Validate().unwrap_err().to_string(),
        "invalid OptionalArgsLen: -1"
    );
    definition.OptionalArgsLen = 2;
    assert_eq!(
        definition.Validate().unwrap_err().to_string(),
        "invalid OptionalArgsLen: 2"
    );
    definition.OptionalArgsLen = 1;
    definition.Validate().unwrap();
}

/// clear 回调应按收集顺序执行（与 Go 收集顺序一致）。
#[test]
fn clear_builder_runs_in_go_collection_order() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut builder = clearFuncBuilder::default();
    for value in [1, 2, 3] {
        let calls = Arc::clone(&calls);
        builder
            .DoWithCollectClear(move || {
                Ok(Some(Box::new(move || calls.lock().unwrap().push(value))))
            })
            .unwrap();
    }
    builder.Build()();
    assert_eq!(*calls.lock().unwrap(), vec![1, 2, 3]);
}

/// 注册表按名称排序 Manifest，会话事件按序分发，Reset 只在首次真正清理。
#[test]
#[serial]
fn registry_sorts_manifests_dispatches_sessions_and_resets_once() {
    Reset();
    let calls = Arc::new(Mutex::new(Vec::new()));

    // 故意乱序注册，Setup 后应按字典序排列。
    for name in ["z-last", "a-first"] {
        let close_calls = Arc::clone(&calls);
        let event_calls = Arc::clone(&calls);
        Register(
            name.to_owned(),
            vec![
                WithClose(move || close_calls.lock().unwrap().push(name)),
                WithSessionHandlerFactory(move || {
                    let event_calls = Arc::clone(&event_calls);
                    Some(SessionHandler {
                        OnConnectionEvent: Some(Arc::new(move |_, _| {
                            event_calls.lock().unwrap().push(name)
                        })),
                        OnStmtEvent: None,
                    })
                }),
                WithCustomAuthPlugins(vec![valid_plugin(name)]),
            ],
        )
        .unwrap();
    }

    Setup().unwrap();
    let extensions = GetExtensions().unwrap().unwrap();
    let names: Vec<_> = extensions
        .Manifests()
        .iter()
        .map(|manifest| manifest.Name().to_owned())
        .collect();
    assert_eq!(names, ["a-first", "z-last"]);

    let session = extensions.NewSessionExtensions();
    session.OnConnectionEvent(ConnEventTp::ConnConnected, &ConnEventInfo::default());
    assert_eq!(*calls.lock().unwrap(), vec!["a-first", "z-last"]);
    assert!(session.GetAuthPlugin("z-last").1);
    assert!(!session.GetAuthPlugin("a-first").1);

    // 连续两次 Reset：close 回调只因第一次 Reset 再追加一轮。
    Reset();
    Reset();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["a-first", "z-last", "a-first", "z-last"]
    );
}

/// 全局鉴权插件表累加全部插件；会话侧取最后一个非空 Manifest 的映射语义。
#[test]
#[serial]
fn extension_auth_map_accumulates_but_session_map_uses_last_non_nil_manifest() {
    Reset();
    Register(
        "first".into(),
        vec![WithCustomAuthPlugins(vec![valid_plugin("one")])],
    )
    .unwrap();
    Register(
        "second".into(),
        vec![WithCustomAuthPlugins(vec![valid_plugin("two")])],
    )
    .unwrap();
    Setup().unwrap();
    let extensions = GetExtensions().unwrap().unwrap();

    let all = extensions.GetAuthPlugins();
    assert!(all.contains_key("one"));
    assert!(all.contains_key("two"));

    let session = extensions.NewSessionExtensions();
    assert!(!session.GetAuthPlugin("one").1);
    assert!(session.GetAuthPlugin("two").1);
    Reset();
}

/// Setup 失败时应执行已收集的 close 回调一次，随后 Reset 不再重复触发。
#[test]
#[serial]
fn setup_error_runs_collected_close_callbacks_once() {
    Reset();
    let closes = Arc::new(Mutex::new(0));
    let observed = Arc::clone(&closes);
    Register(
        "test".into(),
        vec![
            WithClose(move || *observed.lock().unwrap() += 1),
            WithCustomAuthPlugins(vec![Arc::new(AuthPlugin {
                Name: "broken".into(),
                ..AuthPlugin::default()
            })]),
        ],
    )
    .unwrap();

    assert_eq!(
        Setup().unwrap_err().to_string(),
        "auth plugin AuthenticateUser function cannot be nil for broken"
    );
    assert_eq!(*closes.lock().unwrap(), 1);
    Reset();
    assert_eq!(*closes.lock().unwrap(), 1);
}
