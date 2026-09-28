// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// next-gen 受限系统变量的单元测试：公平锁、DML 类型与副本读模式。

#![allow(non_snake_case)]

use astersql_sessionctx_variable::BoolToOnOff;
use astersql_sessionctx_variable::nextgen::{
    GetSysVar, GlobalSystemVariableInitialValue, ReplicaRead, SessionVars,
};
use task_variable::error::{ErrNotSupportedInNextGen, ErrWrongValueForVar};
use task_variable::vardef;

/// 断言错误为 ErrNotSupportedInNextGen（码 1235）且消息提及 next generation。
fn assert_nextgen_error(error: &astersql_sessionctx_variable::nextgen::NextGenError) {
    assert_eq!(error.descriptor(), &ErrNotSupportedInNextGen);
    assert_eq!(error.descriptor().code, 1235);
    assert!(error.message().contains("next generation of TiDB"));
}

// Go: TestTiDBPessimisticTransactionFairLocking.
/// 悲观事务公平锁：OFF 通过；ON 被 next-gen 拒绝并回落 OFF；初始值强制 OFF。
#[test]
fn test_tidb_pessimistic_transaction_fair_locking() {
    let sys_var =
        GetSysVar(vardef::TiDBPessimisticTransactionFairLocking).expect("fair-locking sysvar");
    let mut vars = SessionVars::default();

    let (value, error) = sys_var.Validate("off");
    assert!(error.is_none());
    assert_eq!(value, vardef::Off);
    sys_var
        .SetSessionFromHook(&mut vars, &value)
        .expect("set fair locking off");
    assert!(!vars.PessimisticTransactionFairLocking);

    let (value, error) = sys_var.Validate("0");
    assert!(error.is_none());
    assert_eq!(value, vardef::Off);

    let (value, error) = sys_var.Validate("invalid");
    assert_eq!(value, "invalid");
    assert_eq!(
        error.expect("type validation error").descriptor(),
        &ErrWrongValueForVar
    );

    let (value, error) = sys_var.Validate("on");
    assert_nextgen_error(error.as_ref().expect("next-gen rejection"));
    assert_eq!(value, vardef::Off);
    sys_var
        .SetSessionFromHook(&mut vars, &value)
        .expect("apply next-gen fallback");
    assert!(!vars.PessimisticTransactionFairLocking);

    let value = GlobalSystemVariableInitialValue(
        vardef::TiDBPessimisticTransactionFairLocking,
        &BoolToOnOff(vardef::DefTiDBPessimisticTransactionFairLocking),
    );
    assert_eq!(value, vardef::Off);
}

// Go: TestTiDBDMLTypeInNextGen.
/// DML 类型：standard 通过；bulk 被拒绝并回落到默认类型，BulkDML 保持关闭。
#[test]
fn test_tidb_dml_type_in_nextgen() {
    let sys_var = GetSysVar(vardef::TiDBDMLType).expect("DML type sysvar");
    let mut vars = SessionVars::default();

    let (value, error) = sys_var.Validate("standard");
    assert!(error.is_none());
    assert_eq!(value, "standard");
    sys_var
        .SetSessionFromHook(&mut vars, &value)
        .expect("set standard DML");
    assert!(!vars.BulkDMLEnabled);

    let (value, error) = sys_var.Validate("unknown");
    assert!(error.is_none());
    assert_eq!(value, "unknown");
    assert_eq!(
        sys_var.SetSessionFromHook(&mut vars, &value),
        Err("unsupport DML type: unknown".to_owned())
    );

    let (value, error) = sys_var.Validate("bulk");
    assert_nextgen_error(error.as_ref().expect("next-gen rejection"));
    assert_eq!(value, vardef::DefTiDBDMLType);
    sys_var
        .SetSessionFromHook(&mut vars, &value)
        .expect("apply next-gen fallback");
    assert!(!vars.BulkDMLEnabled);
}

// Go: TestTiDBReplicaReadInNextGen.
/// 副本读：仅 leader 合法；其它模式一律拒绝并回落 leader。
#[test]
fn test_tidb_replica_read_in_nextgen() {
    let sys_var = GetSysVar(vardef::TiDBReplicaRead).expect("replica-read sysvar");
    let mut vars = SessionVars::default();

    let (value, error) = sys_var.Validate("leader");
    assert!(error.is_none());
    assert_eq!(value, "leader");
    sys_var
        .SetSessionFromHook(&mut vars, &value)
        .expect("set leader replica read");
    assert_eq!(vars.GetReplicaRead(), ReplicaRead::Leader);

    let (value, error) = sys_var.Validate("0");
    assert!(error.is_none());
    assert_eq!(value, "leader");

    let (value, error) = sys_var.Validate("invalid");
    assert_eq!(value, "invalid");
    assert_eq!(
        error.expect("type validation error").descriptor(),
        &ErrWrongValueForVar
    );

    for replica_read in [
        "follower",
        "prefer-leader",
        "leader-and-follower",
        "closest-replicas",
        "closest-adaptive",
        "learner",
    ] {
        let (value, error) = sys_var.Validate(replica_read);
        assert_nextgen_error(error.as_ref().expect("next-gen rejection"));
        assert_eq!(value, "leader", "{replica_read}");
        sys_var
            .SetSessionFromHook(&mut vars, &value)
            .expect("apply leader fallback");
        assert_eq!(vars.GetReplicaRead(), ReplicaRead::Leader, "{replica_read}");
    }
}
