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

// InfoSchema 无 testkit 依赖的单元测试（对应 `infoschema_nokit_test.go`）。
//
// 覆盖：schema 增删、脱敏策略（Masking Policy）惰性加载的重试/不重试语义、
// 从旧 InfoSchema 拷贝脱敏缓存时的 loaded 标志与通道继承规则、按名歧义解析、
// 以及表 ID 列表规范化。

// 对应 pkg/infoschema/infoschema_nokit_test.go。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::builder::apply_masking_copy;
use crate::error::ErrTableNotExists;
use crate::infoschema::{
    CiString, InfoSchema, InfoSchemaError, MaskingPolicyInfo, MaskingPolicyLoader, infoSchema,
    normalizeMaskingPolicyTableIDs,
};

/// 可统计调用次数的脱敏策略加载器桩，用于验证重试行为。
struct CountingLoader {
    calls: AtomicUsize,
    mode: LoaderMode,
}

/// 加载失败模式：通用错误可重试，表未就绪则标记已加载且不再重试。
#[derive(Clone, Copy)]
enum LoaderMode {
    GenericError,
    TableNotReady,
}

impl MaskingPolicyLoader for CountingLoader {
    fn load(
        &self,
        _table_ids: &[i64],
        _snapshot_ts: u64,
    ) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            LoaderMode::GenericError => Err(InfoSchemaError {
                code: "ErrTemporary",
                message: "temporary load failure".into(),
            }),
            LoaderMode::TableNotReady => Err(InfoSchemaError {
                code: ErrTableNotExists.mysql_name,
                message: "mysql.tidb_masking_policy".into(),
            }),
        }
    }
}

/// 验证 infoSchema 按名/按 ID 增删 schema。
#[test]
fn test_info_schema_add_del() {
    let mut is = infoSchema::new(0);
    is.add_schema(
        crate::infoschema::DBInfo {
            id: 1,
            name: CiString::new("test"),
            tables: Vec::new(),
            table_name_2_id: Default::default(),
        },
        Vec::new(),
    );
    assert!(is.SchemaExists(&CiString::new("test")));
    assert!(is.SchemaByID(1).is_some());
    assert!(is.del_schema(&CiString::new("test")));
    assert!(!is.SchemaExists(&CiString::new("test")));
    assert!(is.SchemaByID(1).is_none());
}

/// 通用加载错误时不应置 loaded，下次查询会再次调用 loader。
#[test]
fn test_load_masking_policies_retry_on_generic_error() {
    let loader = Arc::new(CountingLoader {
        calls: AtomicUsize::new(0),
        mode: LoaderMode::GenericError,
    });
    let is = infoSchema::new(0).with_masking_loader(loader.clone(), 0);

    // Trigger load via MaskingPolicyByName.
    let _ = is.MaskingPolicyByName(&CiString::new("p"));
    assert!(!is.masking_policies_loaded());
    assert_eq!(1, loader.calls.load(Ordering::SeqCst));

    let _ = is.MaskingPolicyByName(&CiString::new("p"));
    assert!(!is.masking_policies_loaded());
    assert_eq!(2, loader.calls.load(Ordering::SeqCst));
}

/// 表未就绪错误视为“已尝试加载”，置 loaded 且不再重试。
#[test]
fn test_load_masking_policies_no_retry_when_table_not_ready() {
    let loader = Arc::new(CountingLoader {
        calls: AtomicUsize::new(0),
        mode: LoaderMode::TableNotReady,
    });
    let is = infoSchema::new(0).with_masking_loader(loader.clone(), 0);

    let _ = is.MaskingPolicyByName(&CiString::new("p"));
    assert!(is.masking_policies_loaded());
    assert_eq!(1, loader.calls.load(Ordering::SeqCst));

    let _ = is.MaskingPolicyByName(&CiString::new("p"));
    assert_eq!(1, loader.calls.load(Ordering::SeqCst));
}

/// 从旧 InfoSchema 拷贝脱敏策略时，应继承 loaded=true 状态与策略内容。
#[test]
fn test_init_with_old_info_schema_copies_masking_loaded_state() {
    let old = infoSchema::new(0);
    old.set_masking_policies_loaded(true);
    old.put_masking_policy(MaskingPolicyInfo {
        id: 1,
        name: CiString::new("p"),
        table_id: 42,
        column_id: 7,
        ..Default::default()
    });

    let new_is = infoSchema::new(0);
    apply_masking_copy(&new_is, &old, false);

    assert!(new_is.masking_policies_loaded());
    let policy = new_is
        .MaskingPolicyByTableColumn(42, 7)
        .expect("policy should be copied");
    assert_eq!(1, policy.id);
}

/// 旧实例仍有加载通道时：策略仍拷贝，但强制 loaded=false（对应 Go 通道非空分支）。
#[test]
fn test_init_with_old_info_schema_does_not_inherit_masking_load_channel() {
    let old = infoSchema::new(0);
    old.set_masking_policies_loaded(false);
    old.put_masking_policy(MaskingPolicyInfo {
        id: 1,
        name: CiString::new("p"),
        table_id: 42,
        column_id: 7,
        ..Default::default()
    });

    let new_is = infoSchema::new(0);
    // Go: when old.maskingPoliciesLoadCh != nil, force loaded=false and clear channel.
    apply_masking_copy(&new_is, &old, true);

    assert!(!new_is.masking_policies_loaded());
    // Policies are still copied; loaded flag is forced false.
    let map = new_is.clone_masking_policies();
    assert_eq!(1, map.get(&42).unwrap().get(&7).unwrap().id);
}

/// 同名脱敏策略多于一条时按名查询返回 None（歧义）。
#[test]
fn test_masking_policy_by_name_ambiguous() {
    let is = infoSchema::new(0);
    is.set_masking_policies_loaded(true);
    is.put_masking_policy(MaskingPolicyInfo {
        id: 1,
        name: CiString::new("dup"),
        table_id: 101,
        column_id: 11,
        ..Default::default()
    });
    is.put_masking_policy(MaskingPolicyInfo {
        id: 2,
        name: CiString::new("dup"),
        table_id: 202,
        column_id: 22,
        ..Default::default()
    });

    assert!(is.MaskingPolicyByName(&CiString::new("dup")).is_none());
}

/// 规范化表 ID：过滤非正数、去重并排序；空输入表示无过滤条件。
#[test]
fn test_normalize_masking_policy_table_ids() {
    let (ids, has_filter) = normalizeMaskingPolicyTableIDs(&[]);
    assert!(!has_filter);
    assert!(ids.is_empty());

    let (ids, has_filter) = normalizeMaskingPolicyTableIDs(&[0, -1, 42, 42, 7]);
    assert!(has_filter);
    assert_eq!(vec![7, 42], ids);

    let (ids, has_filter) = normalizeMaskingPolicyTableIDs(&[0, -1]);
    assert!(has_filter);
    assert!(ids.is_empty());
}
