// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `conn_ip_example` 插件的加载与审计回调集成测试。
//
// 对应 Go `TestLoadPlugin`：通过测试钩子注入清单（manifest）、执行 load/init，
// 再经 `foreach_plugin` 触发通用 SQL 事件与连接事件，校验连接计数在 shutdown 后归零。

use std::sync::Arc;

use astersql_plugin::{
    Config, ConnectionEvent, ConnectionInfo, Context, GeneralEvent, Kind, RejectReasonContextKey,
    clear_static_plugins, declare_audit_manifest, export_manifest, foreach_plugin, init, load,
    set_test_hook, shutdown,
};
use serial_test::serial;

use crate::{connection_count, plugin_manifest};

/// 拒绝连接时读取 Go `RejectReasonCtxValue` 对应的上下文值。
#[test]
fn test_reject_reason_from_context() {
    let context =
        Context::default().with_value(RejectReasonContextKey, "invalid credentials".to_owned());

    assert_eq!(
        crate::conn_ip_example::rejection_reason(&context),
        "invalid credentials"
    );
    assert_eq!(
        crate::conn_ip_example::rejection_reason(&Context::default()),
        ""
    );
}

/// Corresponds to Go `TestLoadPlugin`.
/// 验证示例审计插件可加载，并能正确响应通用事件与连接事件。
#[test]
#[serial]
fn test_load_plugin() {
    // 清理全局插件状态，避免与其它串行测试互相干扰。
    clear_static_plugins();
    set_test_hook(None);
    shutdown(&Context::default());

    let ctx = Context::default();
    let plugin_name = "conn_ip_example";
    let plugin_version: u16 = 1;
    // 插件签名格式为「名称-版本」，与配置中的 plugins 列表一致。
    let plugin_sign = format!("{plugin_name}-{plugin_version}");

    let cfg = Config {
        plugins: vec![plugin_sign.clone()],
        plugin_dir: String::new(),
        environment_versions: [("go".into(), 1112)].into(),
        ..Config::default()
    };

    // 测试钩子绕过真实动态库加载，直接导出本示例的 AuditManifest。
    set_test_hook(Some(Arc::new(move |_, _| {
        Ok(export_manifest(&plugin_manifest()))
    })));

    load(&ctx, &cfg)
        .unwrap_or_else(|err| panic!("load plugin [{plugin_sign}] fail, error [{err}]"));
    init(&ctx, &cfg)
        .unwrap_or_else(|err| panic!("init plugin [{plugin_sign}] fail, error [{err}]"));

    // 触发一次 Completed 通用事件（审计插件在 SQL 语句完成后的回调）。
    foreach_plugin(Kind::Audit, |audit_plugin| {
        let audit = declare_audit_manifest(audit_plugin.manifest.clone());
        if let Some(on_general) = &audit.on_general_event {
            on_general(&Context::default(), None, GeneralEvent::Completed, "QUERY");
        }
        Ok(())
    })
    .unwrap_or_else(|err| panic!("query event fail, error [{err}]"));

    // 模拟多次客户端连接，期望连接计数器递增。
    let connection_num = 5;
    for _ in 0..connection_num {
        foreach_plugin(Kind::Audit, |audit_plugin| {
            let audit = declare_audit_manifest(audit_plugin.manifest.clone());
            if let Some(on_connection) = &audit.on_connection_event {
                on_connection(
                    &Context::default(),
                    ConnectionEvent::Connected,
                    &ConnectionInfo {
                        host: "localhost".into(),
                        ..ConnectionInfo::default()
                    },
                )?;
            }
            Ok(())
        })
        .unwrap_or_else(|err| panic!("OnConnectionEvent error [{err}]"));
    }

    assert_eq!(connection_num, connection_count());
    // shutdown 应释放连接跟踪并清零计数。
    shutdown(&Context::default());
    assert_eq!(0, connection_count());

    clear_static_plugins();
    set_test_hook(None);
}
