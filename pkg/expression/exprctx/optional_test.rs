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

// 可选属性键与位图集合的单元测试。
//
// 对应 Go `optional_test.go`：验证 Add/Remove/Contains、空/满判定，
// 以及未使用高位不参与有效集合运算的语义。

#![allow(non_snake_case)]

use exprctx_crate::OptPropSessionContext;
use std::ptr;

use exprctx_crate::{
    OPT_PROPS_CNT, OPTIONAL_PROPERTY_DESC_LIST, OptPropAdvisoryLock, OptPropCurrentUser,
    OptPropDDLOwnerInfo, OptPropInfoSchema, OptPropKVStore, OptPropPrivilegeChecker,
    OptPropSQLExecutor, OptPropSequenceOperator, OptPropSessionVars, OptionalEvalPropKey,
    OptionalEvalPropKeySet, validateOptionalProperties,
};

/// Provider 可携带由调用方管理的借用状态；Go 接口没有 `'static` 限制。
struct BorrowingOptionalPropProvider<'a> {
    state: &'a str,
}

impl exprctx_crate::OptionalEvalPropProvider for BorrowingOptionalPropProvider<'_> {
    fn Desc(&self) -> &'static exprctx_crate::OptionalEvalPropDesc {
        let _ = self.state;
        OptPropCurrentUser.Desc()
    }
}

/// 覆盖空集、单键加入、多键加入与移除后的 Contains / IsEmpty / IsFull。
#[test]
pub fn TestOptionalPropKeySet() {
    let key_set = OptionalEvalPropKeySet::default();
    assert!(key_set.IsEmpty());
    assert!(!key_set.IsFull());
    assert!(!key_set.Contains(OptPropCurrentUser));

    let key_set2 = key_set.Add(OptPropCurrentUser);
    assert!(key_set2.Contains(OptPropCurrentUser));
    assert!(!key_set2.IsEmpty());
    assert!(!key_set2.IsFull());
    assert!(key_set.IsEmpty());

    let key_set3 = key_set2.Add(OptPropDDLOwnerInfo);
    assert!(key_set3.Contains(OptPropCurrentUser));
    assert!(key_set3.Contains(OptPropDDLOwnerInfo));
    assert!(!key_set3.IsEmpty());
    assert!(!key_set3.IsFull());
    assert!(!key_set2.Contains(OptPropDDLOwnerInfo));

    let mut key_set4 = key_set3.Remove(OptPropCurrentUser);
    assert!(!key_set4.Contains(OptPropCurrentUser));
    assert!(key_set4.Contains(OptPropDDLOwnerInfo));
    assert!(!key_set4.IsFull());
    assert!(!key_set4.IsEmpty());

    // 凑齐全部有效键后应为满集。
    key_set4 = key_set3
        .Add(OptPropSessionVars)
        .Add(OptPropSessionContext)
        .Add(OptPropInfoSchema)
        .Add(OptPropKVStore)
        .Add(OptPropSQLExecutor)
        .Add(OptPropSequenceOperator)
        .Add(OptPropAdvisoryLock)
        .Add(OptPropPrivilegeChecker);
    assert!(key_set4.IsFull());
    assert!(!key_set4.IsEmpty());
}

/// 高位未使用比特不参与 IsEmpty/IsFull；左移/右移构造边界位图。
#[test]
pub fn TestOptionalPropKeySetWithUnusedBits() {
    assert!(OPT_PROPS_CNT < u64::BITS as usize);
    let full = OptionalEvalPropKeySet(u64::MAX);

    let mut bits = OptionalEvalPropKeySet(full.0 << OPT_PROPS_CNT);
    assert!(bits.IsEmpty());
    assert!(!bits.Contains(OptPropCurrentUser));
    assert!(!bits.Contains(OptPropDDLOwnerInfo));
    bits = bits.Add(OptPropCurrentUser);
    assert!(bits.Contains(OptPropCurrentUser));

    bits = OptionalEvalPropKeySet(full.0 >> (u64::BITS as usize - OPT_PROPS_CNT));
    assert!(bits.IsFull());
    assert!(bits.Contains(OptPropCurrentUser));
    assert!(bits.Contains(OptPropDDLOwnerInfo));
    bits = bits.Remove(OptPropCurrentUser);
    assert!(!bits.Contains(OptPropCurrentUser));
}

/// 校验描述表下标、单键位集互斥，以及 Remove 后为空。
#[test]
pub fn TestOptionalPropKey() {
    validateOptionalProperties();
    for i in 0..OPT_PROPS_CNT {
        let key = OptionalEvalPropKey(i);
        let mut key_set = key.AsPropKeySet();
        assert!(key_set.Contains(key));
        assert_eq!(key, OPTIONAL_PROPERTY_DESC_LIST[i].Key());
        assert!(ptr::eq(key.Desc(), &OPTIONAL_PROPERTY_DESC_LIST[i]));

        for j in 0..OPT_PROPS_CNT {
            if i != j {
                let key2 = OptionalEvalPropKey(j);
                assert!(!key_set.Contains(key2));
            }
        }

        key_set = key_set.Remove(key);
        assert!(key_set.IsEmpty());
    }
}

/// OptionalEvalPropProvider 仅约束 Desc，不应额外要求实现者拥有 `'static` 生命周期。
#[test]
pub fn TestOptionalPropProviderAllowsBorrowedState() {
    let state = String::from("session-owned");
    let provider = BorrowingOptionalPropProvider { state: &state };
    let provider: &dyn exprctx_crate::OptionalEvalPropProvider = &provider;

    assert!(ptr::eq(provider.Desc(), OptPropCurrentUser.Desc()));
    assert!(provider.as_any().is_none());
}
