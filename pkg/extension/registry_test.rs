// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 扩展注册表行为单元测试。
//
// 验证 Manifest 按名称排序、Setup 后拒绝迟到注册，以及 Reset 清空状态。
// 使用 `serial` 避免并发修改全局 registry。

use serial_test::serial;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// 注册乱序名称后 Setup，断言 Manifest 按字典序；迟到注册失败；Reset 后为空。
#[test]
#[serial]
fn canonical_registry_sorts_manifests_rejects_late_registration_and_resets() {
    crate::Reset();
    // 故意按 zeta、alpha 乱序注册，Setup 后应按名称排序。
    crate::Register("zeta".into(), vec![]).unwrap();
    crate::Register("alpha".into(), vec![]).unwrap();
    crate::Setup().unwrap();
    let extensions = crate::GetExtensions().unwrap().unwrap();
    let names = extensions
        .Manifests()
        .iter()
        .map(|manifest| manifest.Name().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["alpha", "zeta"]);
    // Setup 完成后禁止再注册。
    assert!(crate::Register("late".into(), vec![]).is_err());
    crate::Reset();
    assert!(crate::GetExtensions().unwrap().is_none());
}

/// 与 Go TestExtensionRegisterName 对齐：名称校验错误文案属于公开契约。
#[test]
#[serial]
fn registry_rejects_empty_and_duplicate_names_with_go_errors() {
    crate::Reset();

    let error = crate::Register(String::new(), vec![]).unwrap_err();
    assert_eq!(error.to_string(), "extension name should not be empty");

    crate::Register("test".into(), vec![]).unwrap();
    let error = crate::Register("test".into(), vec![]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "extension with name 'test' already registered"
    );

    crate::Reset();
}

/// 与 Go TestRegisterExtensionWithClose 对齐：Reset 关闭一次，重复 Reset 不重复关闭。
#[test]
#[serial]
fn registry_reset_runs_close_once() {
    crate::Reset();
    let closes = Arc::new(AtomicUsize::new(0));
    let closes_for_callback = Arc::clone(&closes);
    crate::Register(
        "test".into(),
        vec![crate::WithClose(move || {
            closes_for_callback.fetch_add(1, Ordering::SeqCst);
        })],
    )
    .unwrap();

    crate::Setup().unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    crate::Reset();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    crate::Reset();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

/// 与 Go TestRegisterExtensionWithClose 的错误路径对齐：后续工厂失败会回滚先前扩展。
#[test]
#[serial]
fn registry_setup_failure_rolls_back_initialized_extensions() {
    crate::Reset();
    let closes = Arc::new(AtomicUsize::new(0));
    let closes_for_callback = Arc::clone(&closes);
    crate::Register(
        "test1".into(),
        vec![crate::WithClose(move || {
            closes_for_callback.fetch_add(1, Ordering::SeqCst);
        })],
    )
    .unwrap();
    crate::RegisterFactory(
        "test2".into(),
        Arc::new(|| Err(crate::ExtensionError::new("error abc"))),
    )
    .unwrap();

    let error = crate::Setup().unwrap_err();
    assert_eq!(error.to_string(), "error abc");
    assert_eq!(closes.load(Ordering::SeqCst), 1);

    crate::Reset();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}
