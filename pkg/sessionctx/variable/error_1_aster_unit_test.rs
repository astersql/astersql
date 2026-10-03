// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// 会话系统变量子系统的聚合单测：错误码、Mock 全局访问器、noop/已移除变量、
// 序列状态、会话重试/用户变量、慢日志规则解析等与 Go 语义对齐的回归检查。

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;

use crate::{
    error, mock_globalaccessor, noop, removed, sequence_state, session, setvar_affect, slow_log,
};

/// 校验错误描述符保留 MySQL 错误码、错误分类与消息模板格式化行为。
#[test]
fn error_descriptors_keep_mysql_class_code_and_templates() {
    assert_eq!(error::ErrUnknownSystemVar.code, 1193);
    assert_eq!(
        error::ErrUnknownSystemVar.class,
        error::ErrorClass::Variable
    );
    assert_eq!(
        error::ErrFunctionsNoopImpl.format(&["READ ONLY"]),
        "function READ ONLY has only noop implementation in tidb now, use tidb_enable_noop_functions to enable these functions"
    );
    assert_eq!(
        error::ErrNotValidPassword.class,
        error::ErrorClass::Executor
    );
    assert_eq!(error::ALL_ERRORS.len(), 24);
}

/// 校验普通模式与测试套件模式下全局变量查找/设置的顺序与副作用。
#[test]
fn mock_accessor_matches_normal_and_testsuite_lookup_and_set_order() {
    let mut registered = HashMap::new();
    registered.insert("autocommit".to_owned(), "ON".to_owned());
    // 普通模式：从外部 registered 表读取，缺失时返回空串而非报错。
    let normal = mock_globalaccessor::NewMockGlobalAccessor();
    assert_eq!(
        normal
            .GetGlobalSysVar("autocommit", Some(&registered))
            .unwrap(),
        "ON"
    );
    assert_eq!(
        normal
            .GetGlobalSysVar("missing", Some(&registered))
            .unwrap(),
        ""
    );

    // 测试套件模式：内部表驱动，未知变量报错；Set 先 validate 再 hook 再写入。
    let mut test = mock_globalaccessor::NewMockGlobalAccessor4Tests(registered);
    assert!(test.GetGlobalSysVar("missing", None).is_err());
    let mut calls = Vec::new();
    test.SetGlobalSysVar(
        "autocommit",
        "on",
        |value| Ok(value.to_ascii_uppercase()),
        |value| {
            calls.push(value.to_owned());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(calls, ["ON"]);
    assert_eq!(test.GetGlobalSysVar("autocommit", None).unwrap(), "ON");
}

/// 校验 noop 变量注册表规模、只读保护校验、已移除变量检测与 hint 可更新标记。
#[test]
fn noop_removed_and_hint_metadata_match_go_behavior() {
    let vars = noop::register_noop_sysvars();
    assert!(
        vars.len() > 200,
        "the Go compatibility table must not be reduced"
    );
    let tx = vars.iter().find(|v| v.name == "tx_read_only").unwrap();
    assert_eq!(tx.aliases, &["transaction_read_only"]);
    // NoopMode::Off：受保护只读变量在会话作用域拒绝开启，回落值为 OFF。
    assert_eq!(
        tx.validate("ON", "ON", noop::Scope::Session, noop::NoopMode::Off, false)
            .unwrap_err()
            .value,
        "OFF"
    );
    // NoopMode::Warn：允许设置但附带警告。
    let warning = tx
        .validate(
            "ON",
            "ON",
            noop::Scope::Session,
            noop::NoopMode::Warn,
            false,
        )
        .unwrap();
    assert_eq!(warning.value, "ON");
    assert!(warning.warning.is_some());

    // 已移除变量名大小写敏感；Check 返回含移除原因的错误。
    assert!(removed::IsRemovedSysVar("tidb_enable_streaming"));
    assert!(!removed::IsRemovedSysVar("TIDB_ENABLE_STREAMING"));
    assert!(
        removed::CheckSysVarIsRemoved("tidb_enable_streaming")
            .unwrap_err()
            .contains("streaming is no longer supported")
    );

    let mut sysvars = vec![
        setvar_affect::SysVar::new("tidb_allow_mpp"),
        setvar_affect::SysVar::new("not_hint_updatable"),
        setvar_affect::SysVar::new("tidb_paging_size_bytes"),
    ];
    setvar_affect::setHintUpdatable(&mut sysvars);
    assert!(sysvars[0].IsHintUpdatableVerified);
    assert!(!sysvars[1].IsHintUpdatableVerified);
    assert!(!sysvars[2].IsHintUpdatableVerified);
}

/// 校验序列（SEQUENCE）状态的并发更新、深拷贝隔离与合并写入语义。
#[test]
fn sequence_state_is_concurrent_and_copies_state() {
    let state = Arc::new(sequence_state::NewSequenceState());
    // 多线程并发 UpdateState，验证内部同步正确。
    let handles: Vec<_> = (0..8)
        .map(|id| {
            let state = Arc::clone(&state);
            thread::spawn(move || state.UpdateState(id, id * 10))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(state.GetLastValue(7), (70, false, None));
    // GetAllStates 返回副本；修改副本不影响原状态。
    let mut copied = state.GetAllStates();
    copied.insert(99, 99);
    assert_eq!(state.GetLastValue(99), (0, true, None));
    state.SetAllStates(&HashMap::from([(10, 100)]));
    assert_eq!(state.GetLastValue(10), (100, false, None));
    // Go maps.Copy 不会清除输入中缺失的旧键。
    assert_eq!(state.GetLastValue(7), (70, false, None));
}

/// 校验事务重试自增 ID、用户变量克隆/大小写、运行时过滤器解析与自适应副本读开关。
#[test]
fn session_retry_user_vars_and_runtime_filters_keep_go_semantics() {
    let mut retry = session::RetryInfo::default();
    retry.AddAutoIncrementID(11);
    retry.AddAutoIncrementID(12);
    assert_eq!(retry.GetCurrAutoIncrementID(), (11, true));
    // ResetOffset 回到队列起点；Clean 清空后读不到有效 ID。
    retry.ResetOffset();
    assert_eq!(retry.GetCurrAutoIncrementID(), (11, true));
    retry.Clean();
    assert_eq!(retry.GetCurrAutoIncrementID(), (0, false));

    let mut vars = session::UserVars::new();
    vars.SetUserVarVal("answer", "42".into());
    let mut answer_type = parser_ast::ast::FieldType::default();
    answer_type.SetFlen(42);
    vars.SetUserVarType("answer", answer_type.clone());
    // CloneVars 后 Unset 原表不影响克隆副本；用户变量名按大小写不敏感处理。
    let cloned = vars.CloneVars();
    vars.UnsetUserVar("ANSWER");
    assert_eq!(cloned.GetUserVarVal("answer").as_deref(), Some("42"));
    assert_eq!(cloned.GetUserVarType("answer"), Some(answer_type));
    assert_eq!(vars.GetUserVarVal("answer"), None);

    assert_eq!(
        session::RuntimeFilterTypeStringToType("MIN_MAX"),
        Some(session::RuntimeFilterType::MinMax)
    );
    assert_eq!(session::ToRuntimeFilterType("IN,MIN_MAX").0.len(), 2);
    assert!(!session::ToRuntimeFilterType("IN,unknown").1);
    // 自适应副本读（adaptive replica read）全局开关：返回值表示是否发生切换。
    session::SetEnableAdaptiveReplicaRead(false);
    assert!(session::SetEnableAdaptiveReplicaRead(true));
    assert!(session::IsAdaptiveReplicaReadEnabled());
    assert!(session::SetEnableAdaptiveReplicaRead(false));
}

/// 校验会话/全局慢日志规则解析的字段校验、拒绝项与同名字段覆盖规则。
#[test]
fn slow_log_rule_parser_matches_go_validation_and_overwrite_rules() {
    let rules = slow_log::ParseSessionSlowLogRules(
        "Exec_retry_count: 10, DB: db1, Succ: true, Query_time: 0.5276, Resource_group: rg1",
    )
    .unwrap()
    .unwrap();
    assert_eq!(rules.rules.len(), 1);
    assert!(rules.fields.contains("exec_retry_count"));
    assert!(rules.raw_rules.contains("exec_retry_count:10"));
    assert!(rules.raw_rules.contains("resource_group:rg1"));

    // 会话规则禁止 ConnID；Query_time 须为非负有限值；规则条数有上限。
    assert!(
        slow_log::ParseSessionSlowLogRules("Conn_ID:123")
            .unwrap_err()
            .contains("do not allow ConnID")
    );
    assert!(
        slow_log::ParseSessionSlowLogRules("Query_time:-1.5")
            .unwrap_err()
            .contains("non-negative")
    );
    assert!(
        slow_log::ParseSessionSlowLogRules("Query_time:NaN")
            .unwrap_err()
            .contains("finite")
    );
    assert!(
        slow_log::ParseSessionSlowLogRules(&"DB:x;".repeat(11))
            .unwrap_err()
            .contains("rules count")
    );

    // 同名字段后写覆盖前写。
    let reset = slow_log::ParseSessionSlowLogRules("Mem_max:100,Succ:true,Succ:false,Mem_max:200")
        .unwrap()
        .unwrap();
    assert_eq!(
        reset.rules[0].threshold("mem_max"),
        Some(&slow_log::Threshold::Int(200))
    );
    assert_eq!(
        reset.rules[0].threshold("succ"),
        Some(&slow_log::Threshold::Bool(false))
    );

    // 全局规则可按 ConnID 分桶，并生成非零 raw hash。
    let global = slow_log::ParseGlobalSlowLogRules("Conn_ID:123,DB:db1;DB:db2").unwrap();
    assert_eq!(global.rules_map.len(), 2);
    assert_ne!(global.raw_rules_hash, 0);
}
