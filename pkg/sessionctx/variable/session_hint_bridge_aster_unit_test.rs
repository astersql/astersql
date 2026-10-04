// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// 语句 Hint / SET_VAR 与系统变量桥接的单元测试。
//
// 验证：通过规范 sysvar 钩子读写 Hint 变量、语句结束时按栈还原，
// 以及 binding（执行计划绑定）标记仅在 `FinishHintStatement` 后推进。

use crate::{GetSysVar, register_builtin_sysvars, session};

/// 使用规范 sysvar 钩子设置 Hint，结束语句后应还原到首次写入前的值。
#[test]
fn planner_session_uses_canonical_sysvar_hooks_and_restores_the_first_value() {
    register_builtin_sysvars();
    let vars = session::SessionVars::new();
    let name = vardef::TiDBOptPartialOrderedIndexForTopN;
    let canonical = GetSysVar(name).expect("registered partial ordered TopN sysvar");

    assert_eq!(
        vars.GetHintSystemVar(name).expect("read default"),
        "DISABLE"
    );
    // 首次 Hint 写入并登记还原栈，随后 binding 覆盖再还原。
    let old = vars
        .SetHintSystemVarWithOldState(name, "cost")
        .expect("apply first statement hint");
    vars.AddHintSystemVarRestore(name, &old);
    assert_eq!(vars.GetHintSystemVar(name).expect("read COST"), "COST");
    assert_eq!(canonical.Name, name);

    let old = vars
        .SetHintSystemVarWithOldState(name, "DISABLE")
        .expect("binding hint overrides query hint");
    vars.AddHintSystemVarRestore(name, &old);
    assert_eq!(
        vars.GetHintSystemVar(name).expect("read override"),
        "DISABLE"
    );

    vars.FinishHintStatement()
        .expect("restore statement variables");
    assert_eq!(
        vars.GetHintSystemVar(name).expect("read restored"),
        "DISABLE"
    );
}

/// `FoundInBinding` 仅在语句 Finish 时推进到 `PrevFoundInBinding` 可读状态。
#[test]
fn binding_flag_advances_only_when_the_statement_finishes() {
    register_builtin_sysvars();
    let vars = session::SessionVars::new();

    vars.BeginHintStatement();
    vars.MarkHintStatementFromBinding();
    // Finish 前对外仍读到上一轮（或默认）的 OFF。
    assert_eq!(
        vars.GetHintSystemVar(vardef::TiDBFoundInBinding)
            .expect("read previous binding state"),
        "OFF"
    );
    vars.FinishHintStatement()
        .expect("finish binding statement");
    assert_eq!(
        vars.GetHintSystemVar(vardef::TiDBFoundInBinding)
            .expect("read completed binding state"),
        "ON"
    );

    // 下一轮无 Mark，Finish 后应回到 OFF。
    vars.BeginHintStatement();
    vars.FinishHintStatement()
        .expect("finish unbound statement");
    assert_eq!(
        vars.GetHintSystemVar(vardef::TiDBFoundInBinding)
            .expect("read next binding state"),
        "OFF"
    );
}

#[test]
#[serial_test::serial]
fn foreign_key_shared_lock_session_bridge_relaxes_persisted_initialization() {
    let restore = config::restore_func();
    config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = false
    });
    let mut vars = session::SessionVars::new();
    let name = vardef::TiDBForeignKeyCheckInSharedLock;
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(crate::Context, name)
            .unwrap(),
        "OFF"
    );
    vars.GlobalVarsAccessor
        .set_global_sys_var_only(&crate::Context, name, "ON", true)
        .unwrap();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(crate::Context, name)
            .unwrap(),
        "ON"
    );
    vars.SetSystemVarWithRelaxedValidation(name, "ON").unwrap();
    assert!(vars.ForeignKeyCheckInSharedLock);
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(crate::Context, name)
            .unwrap(),
        "ON"
    );
    vars.SetSystemVar(name, "OFF").unwrap();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(crate::Context, name)
            .unwrap(),
        "OFF"
    );
    if kerneltype::IsNextGen() {
        assert!(vars.SetSystemVar(name, "1").is_err());
    }
    restore();
}
