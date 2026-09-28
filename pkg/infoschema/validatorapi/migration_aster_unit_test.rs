// Copyright 2026 AsterSQL.
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

// validatorapi 迁移期单元测试：锁定 `Result` 枚举取值与 `Validator` 接口边界。
//
// 使用纯记录桩验证 Go 侧参数与返回值形状，不在接口测试中虚构具体校验器的
// 租约、版本推进或生命周期语义。

use super::{Result, Validator};
use std::cell::{Cell, RefCell};

/// 事务相关的 schema 变更摘要桩：物理表 ID 列表与动作类型列表。
#[derive(Clone, Debug, Eq, PartialEq)]
struct RelatedSchemaChange {
    physical_table_ids: Vec<i64>,
    action_types: Vec<u64>,
}

/// 纯记录桩：只记录接口调用，并返回测试预先配置的结果。
struct RecordingValidator {
    update: RefCell<Option<(u64, i64, i64, Option<RelatedSchemaChange>)>>,
    check: RefCell<Option<(u64, i64, Option<Vec<i64>>, bool)>>,
    check_result: RefCell<(Option<RelatedSchemaChange>, Result)>,
    stopped: Cell<bool>,
    restarted_with: Cell<Option<i64>>,
    reset: Cell<bool>,
    started_result: Cell<bool>,
    lease_expired_result: Cell<bool>,
}

impl Default for RecordingValidator {
    fn default() -> Self {
        Self {
            update: RefCell::new(None),
            check: RefCell::new(None),
            check_result: RefCell::new((None, Result::ResultUnknown)),
            stopped: Cell::new(false),
            restarted_with: Cell::new(None),
            reset: Cell::new(false),
            started_result: Cell::new(false),
            lease_expired_result: Cell::new(false),
        }
    }
}

impl Validator for RecordingValidator {
    type RelatedSchemaChange = RelatedSchemaChange;

    fn Update(
        &self,
        lease_grant_time: u64,
        old_schema_ver: i64,
        new_schema_ver: i64,
        change: Option<&RelatedSchemaChange>,
    ) {
        self.update.replace(Some((
            lease_grant_time,
            old_schema_ver,
            new_schema_ver,
            change.cloned(),
        )));
    }

    fn Check(
        &self,
        txn_ts: u64,
        schema_ver: i64,
        related_physical_table_ids: Option<&[i64]>,
        need_check_schema: bool,
    ) -> (Option<RelatedSchemaChange>, Result) {
        self.check.replace(Some((
            txn_ts,
            schema_ver,
            related_physical_table_ids.map(<[i64]>::to_vec),
            need_check_schema,
        )));
        self.check_result.borrow().clone()
    }

    fn Stop(&self) {
        self.stopped.set(true);
    }
    fn Restart(&self, curr_schema_ver: i64) {
        self.restarted_with.set(Some(curr_schema_ver));
    }
    fn Reset(&self) {
        self.reset.set(true);
    }
    fn IsStarted(&self) -> bool {
        self.started_result.get()
    }
    fn IsLeaseExpired(&self) -> bool {
        self.lease_expired_result.get()
    }
}

/// 锁定 Result 与 Go iota 顺序一致：Succ=0, Fail=1, Unknown=2。
#[test]
fn result_values_match_go_iota_order() {
    assert_eq!(Result::ResultSucc as i32, 0);
    assert_eq!(Result::ResultFail as i32, 1);
    assert_eq!(Result::ResultUnknown as i32, 2);
}

/// 覆盖 Update/Check 与生命周期方法，确认参数、nil 和返回值形状。
#[test]
fn validator_boundary_preserves_go_arguments_and_return_shapes() {
    let validator = RecordingValidator::default();
    let change = RelatedSchemaChange {
        physical_table_ids: vec![1, 3],
        action_types: vec![2, 4],
    };
    validator.Update(42, 10, 11, Some(&change));
    assert_eq!(
        *validator.update.borrow(),
        Some((42, 10, 11, Some(change.clone())))
    );
    validator.Update(43, 11, 12, None);
    assert_eq!(*validator.update.borrow(), Some((43, 11, 12, None)));

    validator
        .check_result
        .replace((Some(change.clone()), Result::ResultFail));
    assert_eq!(
        validator.Check(100, 11, Some(&[1, 3]), true),
        (Some(change.clone()), Result::ResultFail)
    );
    assert_eq!(
        *validator.check.borrow(),
        Some((100, 11, Some(vec![1, 3]), true))
    );
    validator.Check(101, 12, None, false);
    assert_eq!(*validator.check.borrow(), Some((101, 12, None, false)));
    validator.Check(102, 12, Some(&[]), false);
    assert_eq!(
        *validator.check.borrow(),
        Some((102, 12, Some(vec![]), false))
    );

    validator.Restart(12);
    validator.Stop();
    validator.Reset();
    assert_eq!(validator.restarted_with.get(), Some(12));
    assert!(validator.stopped.get());
    assert!(validator.reset.get());

    validator.started_result.set(true);
    validator.lease_expired_result.set(true);
    assert!(validator.IsStarted());
    assert!(validator.IsLeaseExpired());
}
