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

// helper 模块单元测试：清单声明往返与插件 ID 解码。
//
// 验证 `export_manifest` / `declare_*_manifest` 保留名称与版本，以及非法
// `Id` 无法 `decode`。

use crate::{
    AuditManifest, AuthenticationManifest, DaemonManifest, Id, Manifest, SchemaManifest,
    declare_audit_manifest, declare_authentication_manifest, declare_daemon_manifest,
    declare_schema_manifest, export_manifest,
};
use std::sync::Arc;

/// Corresponds to Go `TestPluginDeclare`.
/// 对 Audit/Authentication/Schema/Daemon 做导出再声明，核对基础字段一致。
#[test]
fn test_plugin_declare() {
    let audit_raw = AuditManifest {
        manifest: Manifest::default(),
        ..AuditManifest::default()
    };
    let audit_export = export_manifest(&audit_raw);
    let audit2 = declare_audit_manifest(audit_export);
    assert_eq!(audit_raw.manifest.name, audit2.manifest.name);
    assert_eq!(audit_raw.manifest.version, audit2.manifest.version);
    assert!(audit2.on_general_event.is_none());
    assert!(audit2.on_connection_event.is_none());

    let auth_raw = AuthenticationManifest {
        authenticate_user: Some(Arc::new(|| {})),
        generate_authentication_string: Some(Arc::new(|| {})),
        validate_authentication_string: Some(Arc::new(|| {})),
        set_salt: Some(Arc::new(|| {})),
        ..AuthenticationManifest::default()
    };
    let auth_export = export_manifest(&auth_raw);
    let auth2 = declare_authentication_manifest(auth_export);
    assert_eq!(auth_raw.manifest.name, auth2.manifest.name);
    assert!(Arc::ptr_eq(
        auth_raw.authenticate_user.as_ref().unwrap(),
        auth2.authenticate_user.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        auth_raw.generate_authentication_string.as_ref().unwrap(),
        auth2.generate_authentication_string.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        auth_raw.validate_authentication_string.as_ref().unwrap(),
        auth2.validate_authentication_string.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        auth_raw.set_salt.as_ref().unwrap(),
        auth2.set_salt.as_ref().unwrap()
    ));

    let schema_raw = SchemaManifest::default();
    let schema_export = export_manifest(&schema_raw);
    let schema2 = declare_schema_manifest(schema_export);
    assert_eq!(schema_raw.manifest.name, schema2.manifest.name);

    let daemon_raw = DaemonManifest::default();
    let daemon_export = export_manifest(&daemon_raw);
    let daemon2 = declare_daemon_manifest(daemon_export);
    assert_eq!(daemon_raw.manifest.name, daemon2.manifest.name);
}

/// Corresponds to Go `TestDecode`.
/// 无版本分隔符的 ID 应解码失败。
#[test]
fn test_decode() {
    let fail_id = Id("fail".into());
    assert!(fail_id.decode().is_err());
}
