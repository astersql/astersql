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

// 插件清单声明、ID 解析与测试加载辅助函数。
//
// `declare_*_manifest` 从经 `export_manifest` 导出的通用 `Manifest` 还原各类型
// 专用清单；`Id` 解析「名称-版本」签名；`load_plugin_for_test` 注册内存审计插件
// 供集成测试使用。

use std::sync::Arc;

use crate::{
    AuditManifest, AuthenticationManifest, Config, Context, DaemonManifest, GeneralEventCallback,
    Kind, Manifest, PluginError, PluginErrorKind, SchemaManifest, audit_callbacks,
    authentication_callbacks, clear_static_plugins, export_manifest, init, load,
    register_static_plugin, set_test_hook, shutdown,
};

/// Corresponds to Go `DeclareAuditManifest`. Recovers audit callbacks stored in
/// `Manifest.extension` by `ExportManifest`.
/// 从导出后的 Manifest 还原审计回调（连接/通用/全局变量/解析事件）。
pub fn declare_audit_manifest(manifest: Manifest) -> AuditManifest {
    let callbacks = audit_callbacks(&manifest).cloned();
    AuditManifest {
        on_connection_event: callbacks
            .as_ref()
            .and_then(|c| c.on_connection_event.clone()),
        on_general_event: callbacks.as_ref().and_then(|c| c.on_general_event.clone()),
        on_global_variable_event: callbacks
            .as_ref()
            .and_then(|c| c.on_global_variable_event.clone()),
        on_parse_event: callbacks.as_ref().and_then(|c| c.on_parse_event.clone()),
        manifest,
    }
}

/// 声明认证类插件清单，并恢复导出时保存的四个认证回调。
pub fn declare_authentication_manifest(manifest: Manifest) -> AuthenticationManifest {
    let callbacks = authentication_callbacks(&manifest).cloned();
    AuthenticationManifest {
        authenticate_user: callbacks.as_ref().and_then(|c| c.authenticate_user.clone()),
        generate_authentication_string: callbacks
            .as_ref()
            .and_then(|c| c.generate_authentication_string.clone()),
        validate_authentication_string: callbacks
            .as_ref()
            .and_then(|c| c.validate_authentication_string.clone()),
        set_salt: callbacks.as_ref().and_then(|c| c.set_salt.clone()),
        manifest,
    }
}

/// 声明 Schema 类插件清单（仅包装基础 Manifest）。
pub fn declare_schema_manifest(manifest: Manifest) -> SchemaManifest {
    SchemaManifest { manifest }
}

/// 声明守护类插件清单（仅包装基础 Manifest）。
pub fn declare_daemon_manifest(manifest: Manifest) -> DaemonManifest {
    DaemonManifest { manifest }
}

/// 插件标识：形如 `name-version` 的字符串包装。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Id(pub String);

impl Id {
    /// 从右侧按最后一个 `-` 拆分名称与版本；格式错误则返回 InvalidPluginId。
    pub fn decode(&self) -> Result<(String, String), PluginError> {
        let (name, version) = self.0.rsplit_once('-').ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::InvalidPluginId,
                format!("invalid plugin ID: {}", self.0),
            )
        })?;
        Ok((name.to_owned(), version.to_owned()))
    }
}

/// Corresponds to Go `LoadPluginForTest`.
/// 为测试注册静态审计插件并完成 load/init，回调由参数注入。
pub fn load_plugin_for_test(callback: GeneralEventCallback) -> Result<(), PluginError> {
    // 先清空全局状态，保证测试可重复。
    clear_static_plugins();
    set_test_hook(None);
    shutdown(&Context::default());

    let name = "audit_test";
    register_static_plugin(
        name,
        Arc::new(move || {
            let mut manifest = Manifest::new(Kind::Audit, name, 1);
            manifest.validate = Some(Arc::new(|_, _| Ok(())));
            manifest.on_init = Some(Arc::new(|_, _| Ok(())));
            manifest.on_shutdown = Some(Arc::new(|_, _| Ok(())));
            export_manifest(&AuditManifest {
                manifest,
                on_connection_event: Some(Arc::new(|_, _, _| Ok(()))),
                on_general_event: Some(Arc::clone(&callback)),
                on_global_variable_event: None,
                on_parse_event: None,
            })
        }),
    )?;
    let config = Config {
        plugins: vec!["audit_test-1".into()],
        plugin_dir: String::new(),
        skip_when_fail: false,
        environment_versions: [("go".into(), 1112)].into(),
        etcd: None,
        loader: None,
    };
    let context = Context::default();
    load(&context, &config)?;
    init(&context, &config)
}
