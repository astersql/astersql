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

// SPI（Service Provider Interface，服务提供方接口）导出/声明路径的单元测试。
//
// 验证 `export_manifest` 与 `declare_audit_manifest` 往返后，
// 生命周期回调与审计事件回调仍可正确触发。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    AuditManifest, Context, GeneralEvent, Kind, Manifest, RejectReasonContextKey,
    declare_audit_manifest, export_manifest,
};

/// Go `context.WithValue` 的派生、遮蔽与取消传播语义。
#[test]
fn context_with_value_derives_without_mutating_parent() {
    let parent = Context::default();
    let child = parent.with_value(RejectReasonContextKey, "access denied".to_owned());

    assert!(parent.value(RejectReasonContextKey).is_none());
    assert_eq!(
        child
            .value(RejectReasonContextKey)
            .as_deref()
            .map(String::as_str),
        Some("access denied")
    );

    let grandchild = child.with_value(RejectReasonContextKey, "account locked".to_owned());
    assert_eq!(
        child
            .value(RejectReasonContextKey)
            .as_deref()
            .map(String::as_str),
        Some("access denied")
    );
    assert_eq!(
        grandchild
            .value(RejectReasonContextKey)
            .as_deref()
            .map(String::as_str),
        Some("account locked")
    );

    parent.cancel();
    assert!(child.is_cancelled());
    assert!(grandchild.is_cancelled());
}

/// Corresponds to Go `TestExportManifest` (`package plugin_test`).
/// 校验导出 Manifest 后 OnInit 可执行，再经 declare 恢复审计回调。
#[test]
fn test_export_manifest() {
    let on_init_called = Arc::new(AtomicBool::new(false));
    let notify_event_called = Arc::new(AtomicBool::new(false));

    let on_init_flag = Arc::clone(&on_init_called);
    let notify_flag = Arc::clone(&notify_event_called);

    // 构造带 OnInit 与 OnGeneralEvent 的审计清单。
    let mut manifest = Manifest::new(Kind::Authentication, "test audit", 1);
    manifest.on_init = Some(Arc::new(move |_, _| {
        on_init_flag.store(true, Ordering::SeqCst);
        Ok(())
    }));
    let audit = AuditManifest {
        manifest,
        on_general_event: Some(Arc::new(move |_, _, _, _| {
            notify_flag.store(true, Ordering::SeqCst);
        })),
        on_connection_event: None,
        on_global_variable_event: None,
        on_parse_event: None,
    };

    // 导出后直接调用 OnInit，再 declare 回 AuditManifest 并触发通用事件。
    let exported = export_manifest(&audit);
    exported.on_init.as_ref().expect("OnInit exported")(&Context::default(), &exported)
        .expect("OnInit ok");

    let recovered = declare_audit_manifest(exported);
    recovered
        .on_general_event
        .as_ref()
        .expect("OnGeneralEvent recovered")(
        &Context::default(),
        None,
        GeneralEvent::Completed,
        "QUERY",
    );

    assert!(notify_event_called.load(Ordering::SeqCst));
    assert!(on_init_called.load(Ordering::SeqCst));
}
