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

// `sysvar` 模块的 Aster 单元测试。
//
// 覆盖执行器并发选项、细粒度 shuffle 系统变量、按运行时分支的安装默认值、
// TiFlash 派发策略与流水线 DML 资源策略，以及优化器选择性 / 部分有序 TopN 变量。

use std::sync::Arc;

use astersql_sessionctx_variable::{
    Context as FormalContext, GetSysVar, GlobalVarAccessor, SessionVars as FormalSessionVars,
    VariableError, register_builtin_sysvars,
};
use astersql_sessionctx_variable::{sysvar::*, tiflashcompute, vardef};

/// 规划器测试用空全局 accessor：读未知变量报错，写操作视为成功空操作。
#[derive(Default)]
struct PlannerGlobalVariables;

impl GlobalVarAccessor for PlannerGlobalVariables {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _context: &FormalContext,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}

/// `max_connections` 必须按 Go 定义注册为 INSTANCE 级无符号变量。
#[test]
fn max_connections_metadata_matches_go() {
    register_builtin_sysvars();
    let sysvar = GetSysVar(vardef::MaxConnections).expect("max_connections must be registered");
    assert_eq!(sysvar.Scope, vardef::ScopeInstance);
    assert_eq!(sysvar.Type, vardef::TypeUnsigned);
    assert_eq!(sysvar.Value, "0");
    assert_eq!(sysvar.MinValue, 0);
    assert_eq!(sysvar.MaxValue, 100_000);
}

/// 全局 `tidb_persist_analyze_options` 的钩子必须与 Go 一样读写进程级开关。
#[test]
fn persist_analyze_options_sysvar_matches_go_global_hooks() {
    register_builtin_sysvars();
    let sysvar = GetSysVar(vardef::TiDBPersistAnalyzeOptions)
        .expect("tidb_persist_analyze_options must be registered");
    assert_eq!(sysvar.Scope, vardef::ScopeGlobal);
    assert_eq!(sysvar.Value, vardef::On);
    assert_eq!(sysvar.Type, vardef::TypeBool);

    let original = vardef::PersistAnalyzeOptions.Load();
    struct RestorePersistAnalyzeOptions(bool);
    impl Drop for RestorePersistAnalyzeOptions {
        fn drop(&mut self) {
            vardef::PersistAnalyzeOptions.Store(self.0);
        }
    }
    let _restore = RestorePersistAnalyzeOptions(original);

    let mut vars = FormalSessionVars::new(Box::<PlannerGlobalVariables>::default());
    let disabled = sysvar
        .Validate(&mut vars, vardef::Off, vardef::ScopeGlobal)
        .expect("OFF is valid");
    sysvar
        .SetGlobalFromHook(&FormalContext, &mut vars, &disabled, false)
        .expect("set OFF");
    assert!(!vardef::PersistAnalyzeOptions.Load());
    assert_eq!(
        sysvar.GetGlobalFromHook(&FormalContext, &mut vars).unwrap(),
        vardef::Off
    );

    let enabled = sysvar
        .Validate(&mut vars, vardef::On, vardef::ScopeGlobal)
        .expect("ON is valid");
    sysvar
        .SetGlobalFromHook(&FormalContext, &mut vars, &enabled, false)
        .expect("set ON");
    assert!(vardef::PersistAnalyzeOptions.Load());
    assert_eq!(
        sysvar.GetGlobalFromHook(&FormalContext, &mut vars).unwrap(),
        vardef::On
    );
}

/// 校验执行器并发系统变量选项与 setter 行为与 Go 一致。
#[test]
fn executor_concurrency_options_and_setter_match_go() {
    let setter: ConcurrencySetter = Arc::new(|vars, value| {
        vars.ExecutorConcurrency = value;
    });
    let sys_var = newExecConcurrencySysVar(
        "tidb_executor_concurrency",
        5,
        setter,
        [withAllowAutoValue(false), withMinValue(2)],
    );

    assert_eq!(sys_var.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(sys_var.Name, "tidb_executor_concurrency");
    assert_eq!(sys_var.Value, "5");
    assert_eq!(sys_var.Type, vardef::TypeInt);
    assert_eq!(sys_var.MinValue, 2);
    assert_eq!(sys_var.MaxValue, vardef::MaxConfigurableConcurrency as u64);
    assert!(!sys_var.AllowAutoValue);

    let mut vars = SessionVars::default();
    sys_var.SetSession(&mut vars, "17").unwrap();
    assert_eq!(vars.ExecutorConcurrency, 17);
    sys_var.SetSession(&mut vars, "not-an-int").unwrap();
    assert_eq!(vars.ExecutorConcurrency, vardef::ConcurrencyUnset as i32);
}

/// 细粒度 shuffle / TiFlash 线程上限系统变量应写入规划器会话状态。
#[test]
fn fine_grained_shuffle_sysvars_update_planner_session_state() {
    register_builtin_sysvars();
    let stream_count = GetSysVar(vardef::TiFlashFineGrainedShuffleStreamCount)
        .expect("fine-grained shuffle stream count must be registered");
    assert_eq!(stream_count.MinValue, -1);
    assert_eq!(stream_count.MaxValue, 1024);
    let max_threads = GetSysVar(vardef::TiDBMaxTiFlashThreads)
        .expect("maximum TiFlash threads must be registered");
    assert_eq!(max_threads.MinValue, -1);
    assert_eq!(
        max_threads.MaxValue,
        vardef::MaxConfigurableConcurrency as u64
    );

    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    variables
        .SetSystemVar(vardef::TiFlashFineGrainedShuffleStreamCount, "16")
        .unwrap();
    variables
        .SetSystemVar(vardef::TiDBMaxTiFlashThreads, "10")
        .unwrap();
    assert_eq!(variables.TiFlashFineGrainedShuffleStreamCount, 16);
    assert_eq!(variables.TiFlashMaxThreads, 10);
}

/// 高级 Join Reorder 开关必须与 Go 一样有默认值，并在 SET 后写入规划器会话状态。
#[test]
fn advanced_join_reorder_sysvar_updates_planner_session_state() {
    register_builtin_sysvars();
    let sys_var = GetSysVar(vardef::TiDBOptEnableAdvancedJoinReorder)
        .expect("advanced join reorder sysvar must be registered");
    assert_eq!(sys_var.Value, "ON");

    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    assert!(variables.TiDBOptEnableAdvancedJoinReorder);
    variables
        .SetSystemVar(vardef::TiDBOptEnableAdvancedJoinReorder, "OFF")
        .unwrap();
    assert!(!variables.TiDBOptEnableAdvancedJoinReorder);
}

/// 安装默认值应按经典测试环境与 next-gen 运行时分支与 Go 对齐。
#[test]
fn global_initial_values_preserve_runtime_dependent_go_branches() {
    let classic_test = RuntimeEnvironment {
        store_is_tikv: true,
        in_test: true,
        next_gen: false,
        default_txn_assertion_level: vardef::AssertionStrictStr.to_owned(),
    };
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBEnableAsyncCommit,
            vardef::Off,
            &classic_test,
        ),
        vardef::On
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBEnable1PC,
            vardef::Off,
            &classic_test
        ),
        vardef::On
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBMemOOMAction,
            "CANCEL",
            &classic_test
        ),
        vardef::OOMActionLog
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBEnableAutoAnalyze,
            vardef::On,
            &classic_test
        ),
        vardef::Off
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBRowFormatVersion,
            "1",
            &classic_test
        ),
        vardef::DefTiDBRowFormatV2.to_string()
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBTxnAssertionLevel,
            "OFF",
            &classic_test
        ),
        vardef::AssertionFastStr
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBEnableMutationChecker,
            vardef::Off,
            &classic_test
        ),
        vardef::On
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBPessimisticTransactionFairLocking,
            vardef::Off,
            &classic_test,
        ),
        vardef::On
    );

    let next_gen = RuntimeEnvironment {
        store_is_tikv: false,
        in_test: false,
        next_gen: true,
        default_txn_assertion_level: vardef::AssertionStrictStr.to_owned(),
    };
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBTxnAssertionLevel,
            "FAST",
            &next_gen
        ),
        vardef::AssertionStrictStr
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBPessimisticTransactionFairLocking,
            vardef::On,
            &next_gen,
        ),
        vardef::Off
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(vardef::TiDBTxnMode, "pessimistic", &next_gen),
        "pessimistic"
    );
}

/// TiFlash 派发策略先解析再写会话；非法值不得部分更新。
#[test]
fn tiflash_dispatch_policy_is_parsed_before_session_update() {
    register_builtin_sysvars();
    let sys_var = GetSysVar(vardef::TiFlashComputeDispatchPolicy)
        .expect("TiFlash dispatch policy must be registered");
    assert_eq!(sys_var.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(sys_var.Type, vardef::TypeStr);
    assert_eq!(sys_var.Value, vardef::DefTiFlashComputeDispatchPolicy);

    let mut vars = SessionVars::default();
    setTiFlashComputeDispatchPolicy(&mut vars, vardef::DispatchPolicyRRStr).unwrap();
    assert_eq!(
        vars.TiFlashComputeDispatchPolicy,
        tiflashcompute::DispatchPolicyRR
    );

    let error = setTiFlashComputeDispatchPolicy(&mut vars, "random").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unexpected tiflash_compute dispatch policy")
    );
    assert_eq!(
        vars.TiFlashComputeDispatchPolicy,
        tiflashcompute::DispatchPolicyRR,
        "an invalid value must not partially update session state"
    );
}

/// 流水线资源策略支持预设与大小写/分隔符灵活的自定义语法。
#[test]
fn pipelined_resource_policy_supports_presets_and_flexible_custom_syntax() {
    let mut vars = SessionVars::default();
    setPipelinedDmlResourcePolicy(&mut vars, "  STANDARD ").unwrap();
    assert_eq!(
        vars.PipelinedDMLConfig,
        PipelinedDMLConfig {
            PipelinedFlushConcurrency: vardef::DefaultFlushConcurrency as i32,
            PipelinedResolveLockConcurrency: vardef::DefaultResolveConcurrency as i32,
            PipelinedWriteThrottleRatio: 0.0,
        }
    );

    setPipelinedDmlResourcePolicy(&mut vars, "conservative").unwrap();
    assert_eq!(
        vars.PipelinedDMLConfig.PipelinedFlushConcurrency,
        vardef::ConservativeFlushConcurrency as i32
    );
    assert_eq!(
        vars.PipelinedDMLConfig.PipelinedResolveLockConcurrency,
        vardef::ConservativeResolveConcurrency as i32
    );

    setPipelinedDmlResourcePolicy(
        &mut vars,
        " CuStOm { concurrency = 17, resolve_concurrency: 23, write_throttle_ratio = 0.25 } ",
    )
    .unwrap();
    assert_eq!(
        vars.PipelinedDMLConfig,
        PipelinedDMLConfig {
            PipelinedFlushConcurrency: 17,
            PipelinedResolveLockConcurrency: 23,
            PipelinedWriteThrottleRatio: 0.25,
        }
    );
}

/// 非法自定义策略失败时必须保持原会话配置不变。
#[test]
fn invalid_custom_policy_never_partially_applies_values() {
    let original = PipelinedDMLConfig {
        PipelinedFlushConcurrency: 7,
        PipelinedResolveLockConcurrency: 9,
        PipelinedWriteThrottleRatio: 0.4,
    };

    for value in [
        "custom",
        "custom {}",
        "custom {concurrency=0}",
        "custom {concurrency=8193}",
        "custom {resolve_concurrency=x}",
        "custom {write_throttle_ratio=-0.1}",
        "custom {write_throttle_ratio=1}",
        "custom {concurrency=10,unknown=1}",
        "custom {concurrency=10,}",
        "custom {concurrency=10=11}",
    ] {
        let mut vars = SessionVars {
            PipelinedDMLConfig: original.clone(),
            ..SessionVars::default()
        };
        let error = setPipelinedDmlResourcePolicy(&mut vars, value).unwrap_err();
        assert_eq!(error.variable(), vardef::TiDBPipelinedDmlResourcePolicy);
        assert_eq!(error.value(), value.trim());
        assert_eq!(vars.PipelinedDMLConfig, original, "case: {value}");
    }
}

/// 字符串匹配选择性与部分有序索引 TopN 系统变量校验/读写与 Go 一致。
#[test]
fn planner_selectivity_and_partial_ordered_topn_sysvars_match_go() {
    register_builtin_sysvars();
    let mut vars = FormalSessionVars::new(Box::<PlannerGlobalVariables>::default());

    let selectivity = GetSysVar(vardef::TiDBDefaultStrMatchSelectivity)
        .expect("default string match selectivity must be registered");
    assert_eq!(
        selectivity.Scope,
        vardef::ScopeGlobal | vardef::ScopeSession
    );
    assert_eq!(selectivity.Type, vardef::TypeFloat);
    assert_eq!(selectivity.MinValue, 0);
    assert_eq!(selectivity.MaxValue, 1);
    assert_eq!(
        selectivity.Value,
        vardef::DefTiDBDefaultStrMatchSelectivity.to_string()
    );
    for (input, expected) in [("0", "0"), ("0.3", "0.3"), ("1", "1")] {
        let normalized = selectivity
            .Validate(&mut vars, input, vardef::ScopeSession)
            .unwrap_or_else(|error| panic!("validate {input}: {error}"));
        assert_eq!(normalized, expected);
        selectivity
            .SetSessionFromHook(&mut vars, &normalized)
            .unwrap_or_else(|error| panic!("set {normalized}: {error}"));
        assert_eq!(
            vars.DefaultStrMatchSelectivity,
            expected.parse::<f64>().unwrap()
        );
        assert_eq!(selectivity.GetSessionFromHook(&mut vars).unwrap(), expected);
    }
    assert_eq!(
        selectivity
            .Validate(&mut vars, "-0.1", vardef::ScopeSession)
            .unwrap(),
        "0"
    );
    assert_eq!(
        selectivity
            .Validate(&mut vars, "1.1", vardef::ScopeGlobal)
            .unwrap(),
        "1"
    );
    assert!(
        selectivity
            .Validate(&mut vars, "not-a-float", vardef::ScopeSession)
            .is_err()
    );

    let partial = GetSysVar(vardef::TiDBOptPartialOrderedIndexForTopN)
        .expect("partial ordered TopN sysvar must be registered");
    assert_eq!(partial.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(partial.Type, vardef::TypeEnum);
    assert_eq!(partial.PossibleValues, ["DISABLE", "COST"]);
    assert_eq!(partial.Value, vardef::DefTiDBOptPartialOrderedIndexForTopN);
    assert!(partial.IsHintUpdatableVerified);
    for (input, expected) in [
        ("DISABLE", "DISABLE"),
        ("disable", "DISABLE"),
        ("Cost", "COST"),
        ("COST", "COST"),
    ] {
        let normalized = partial
            .Validate(&mut vars, input, vardef::ScopeSession)
            .unwrap_or_else(|error| panic!("validate {input}: {error}"));
        assert_eq!(normalized, expected);
        partial
            .SetSessionFromHook(&mut vars, &normalized)
            .unwrap_or_else(|error| panic!("set {normalized}: {error}"));
        assert_eq!(vars.OptPartialOrderedIndexForTopN, expected);
        assert_eq!(partial.GetSessionFromHook(&mut vars).unwrap(), expected);
    }
    for invalid in ["ON", "OFF", "0", "1", "true", "false", "yes", "no"] {
        assert!(
            partial
                .Validate(&mut vars, invalid, vardef::ScopeSession)
                .is_err(),
            "{invalid} must be rejected instead of treated as an enum index"
        );
    }
}
