// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 内置系统变量（builtin sysvar）注册表。
//
// 将 TiDB/MySQL 兼容的系统变量定义、会话/全局钩子与默认值注册到全局表；
// 含资源控制、服务器内存上限、schema 缓存、追踪事件等进程级原子状态。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::vardef;
use crate::variable::{format_go_duration, parse_go_duration};
use crate::{
    BoolToOnOff, Context, RegisterSysVar, SessionVars, SysVar, TiDBOptOn, VariableError,
    VariableErrorKind, checkCollation, checkIsolationLevel, parseByteSize, parseMemoryLimit,
    parseSchemaCacheSize,
};
use traceevent::flightrecorder::{
    FlightRecorderConfig, close_flight_recorder, get_flight_recorder, start_log_flight_recorder,
};

// 保证内置变量只注册一次。
static REGISTER: Once = Once::new();
/// 资源控制功能是否启用。
pub static RESOURCE_CONTROL_ENABLED: AtomicBool = AtomicBool::new(false);
/// 资源控制是否严格模式。
pub static RESOURCE_CONTROL_STRICT: AtomicBool = AtomicBool::new(true);
/// 服务器内存上限（字节）；0 表示未限制。
pub static SERVER_MEMORY_LIMIT: AtomicU64 = AtomicU64::new(0);
/// 会话触发服务器内存限制的最小用量。
pub static SERVER_MEMORY_LIMIT_SESS_MIN_SIZE: AtomicU64 = AtomicU64::new(128 << 20);
/// 触发 GC 的内存占比（f64 bit pattern）。
pub static SERVER_MEMORY_LIMIT_GC_TRIGGER_BITS: AtomicU64 = AtomicU64::new(0.7_f64.to_bits());
/// schema 缓存大小（字节）。
pub static SCHEMA_CACHE_SIZE: AtomicU64 = AtomicU64::new(512 << 20);
/// client timestamp validation 开关（由 `tidb_enable_ts_validation` 控制）。
pub static TS_VALIDATION_ENABLED: AtomicBool = AtomicBool::new(vardef::DefTiDBEnableTSValidation);
/// 追踪事件配置字符串（可选）。
pub static TRACE_EVENT_CONFIG: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

/// Apply the process-wide `tidb_trace_event` recorder configuration.
pub fn SetTraceEventConfig(value: &str) -> Result<(), String> {
    if crate::kerneltype::IsClassic() {
        return Err("can only be set for TiDB X kernel".to_owned());
    }
    if value.is_empty() {
        close_flight_recorder();
        traceevent::traceevent::set_mode("off")?;
        *TRACE_EVENT_CONFIG.lock().unwrap() = None;
        return Ok(());
    }
    let recorder_config: FlightRecorderConfig =
        serde_json::from_str(value).map_err(|error| error.to_string())?;
    start_log_flight_recorder(recorder_config)?;
    traceevent::traceevent::set_mode("full")?;
    *TRACE_EVENT_CONFIG.lock().unwrap() = Some(value.to_owned());
    Ok(())
}
// Go GC 触发阈值（占比）的 bit pattern。
static GOGC_THRESHOLD_BITS: AtomicU64 = AtomicU64::new(0.6_f64.to_bits());
static GOGC_MIN: AtomicI64 = AtomicI64::new(vardef::DefTiDBGOGCMinValue);
static GOGC_MAX: AtomicI64 = AtomicI64::new(vardef::DefTiDBGOGCMaxValue);

/// 返回同时具备全局与会话作用域的标志。
fn scope_both() -> vardef::ScopeFlag {
    vardef::ScopeGlobal | vardef::ScopeSession
}

/// 构造字符串类型系统变量骨架。
fn string_var(name: &str, value: &str, scope: vardef::ScopeFlag) -> SysVar {
    SysVar {
        Name: name.to_owned(),
        Value: value.to_owned(),
        Scope: scope,
        Type: vardef::TypeStr,
        ..SysVar::default()
    }
}

/// 构造布尔类型系统变量骨架（值为 ON/OFF）。
fn bool_var(name: &str, value: bool, scope: vardef::ScopeFlag) -> SysVar {
    SysVar {
        Name: name.to_owned(),
        Value: BoolToOnOff(value),
        Scope: scope,
        Type: vardef::TypeBool,
        ..SysVar::default()
    }
}

/// 构造有符号整数类型系统变量骨架。
fn int_var(name: &str, value: i64, scope: vardef::ScopeFlag, min: i64, max: u64) -> SysVar {
    SysVar {
        Name: name.to_owned(),
        Value: value.to_string(),
        Scope: scope,
        Type: vardef::TypeInt,
        MinValue: min,
        MaxValue: max,
        ..SysVar::default()
    }
}

/// 构造无符号整数类型系统变量骨架。
fn unsigned_var(name: &str, value: u64, scope: vardef::ScopeFlag, min: i64, max: u64) -> SysVar {
    SysVar {
        Name: name.to_owned(),
        Value: value.to_string(),
        Scope: scope,
        Type: vardef::TypeUnsigned,
        MinValue: min,
        MaxValue: max,
        ..SysVar::default()
    }
}

/// 构造浮点类型系统变量骨架。
fn float_var(name: &str, value: f64, scope: vardef::ScopeFlag, min: i64, max: u64) -> SysVar {
    SysVar {
        Name: name.to_owned(),
        Value: value.to_string(),
        Scope: scope,
        Type: vardef::TypeFloat,
        MinValue: min,
        MaxValue: max,
        ..SysVar::default()
    }
}

/// 构造枚举类型系统变量骨架。
fn enum_var(name: &str, value: &str, scope: vardef::ScopeFlag, values: &[&str]) -> SysVar {
    SysVar {
        Name: name.to_owned(),
        Value: value.to_owned(),
        Scope: scope,
        Type: vardef::TypeEnum,
        PossibleValues: values.iter().map(|value| (*value).to_owned()).collect(),
        ..SysVar::default()
    }
}

/// 将 Go `noopSysVars` 的兼容元数据转换成运行时系统变量定义。
fn noop_sys_var(variable: &crate::noop::NoopSysVar) -> SysVar {
    let scope = match variable.scope {
        crate::noop::Scope::None => vardef::ScopeNone,
        crate::noop::Scope::Global => vardef::ScopeGlobal,
        crate::noop::Scope::Session => vardef::ScopeSession,
        crate::noop::Scope::GlobalAndSession => vardef::ScopeGlobal | vardef::ScopeSession,
    };
    let variable_type = match variable.var_type {
        crate::noop::SysVarType::String => vardef::TypeStr,
        crate::noop::SysVarType::Bool => vardef::TypeBool,
        crate::noop::SysVarType::Unsigned => vardef::TypeUnsigned,
        crate::noop::SysVarType::Int => vardef::TypeInt,
        crate::noop::SysVarType::Enum => vardef::TypeEnum,
    };
    let min_value = variable
        .min_value
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("invalid noop sysvar minimum {value}"))
        })
        .unwrap_or_else(|| {
            if variable.var_type == crate::noop::SysVarType::Int {
                i64::MIN
            } else {
                0
            }
        });
    let max_value = variable
        .max_value
        .map(|value| match value {
            "math.MaxInt64" => i64::MAX as u64,
            "math.MaxUint64" => u64::MAX,
            "secondsPerYear" => crate::noop::SECONDS_PER_YEAR,
            value => value
                .parse()
                .unwrap_or_else(|_| panic!("invalid noop sysvar maximum {value}")),
        })
        .unwrap_or_else(|| {
            if variable.var_type == crate::noop::SysVarType::Int {
                i64::MAX as u64
            } else {
                u64::MAX
            }
        });

    SysVar {
        Scope: scope,
        Name: variable.name.to_owned(),
        Value: variable.value.to_owned(),
        Type: variable_type,
        MinValue: min_value,
        MaxValue: max_value,
        AutoConvertNegativeBool: variable.auto_convert_negative_bool,
        ReadOnly: variable.read_only,
        PossibleValues: variable
            .possible_values
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        IsHintUpdatableVerified: variable.is_hint_updatable_verified,
        Aliases: variable
            .aliases
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        IsNoop: true,
        ..SysVar::default()
    }
}

/// 注册 Go `noopSysVars` 中尚未由完整 Rust 实现覆盖的兼容变量。
fn register_noop_compatibility_vars() {
    for variable in crate::noop::NOOP_SYS_VARS {
        if crate::GetSysVar(variable.name).is_none() {
            RegisterSysVar(noop_sys_var(variable));
        }
    }
}

/// 注册 Go `sysvar.go` 中仍由会话变量测试直接依赖的基础兼容变量。
fn register_compatibility_vars() {
    RegisterSysVar(unsigned_var(
        vardef::Port,
        4000,
        vardef::ScopeNone,
        0,
        u16::MAX as u64,
    ));
    RegisterSysVar(string_var(
        "version_compile_os",
        std::env::consts::OS,
        vardef::ScopeNone,
    ));
    RegisterSysVar(string_var(
        "version_compile_machine",
        std::env::consts::ARCH,
        vardef::ScopeNone,
    ));

    RegisterSysVar(bool_var(vardef::TiDBGCEnable, true, vardef::ScopeGlobal));
    RegisterSysVar(string_var(
        vardef::TiDBGCRunInterval,
        "10m0s",
        vardef::ScopeGlobal,
    ));
    RegisterSysVar(string_var(
        vardef::TiDBGCLifetime,
        "10m0s",
        vardef::ScopeGlobal,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBGCConcurrency,
        -1,
        vardef::ScopeGlobal,
        -1,
        i64::MAX as u64,
    ));
    RegisterSysVar(enum_var(
        vardef::TiDBGCScanLockMode,
        "LEGACY",
        vardef::ScopeGlobal,
        &["PHYSICAL", "LEGACY"],
    ));
    RegisterSysVar(bool_var(
        vardef::RequireSecureTransport,
        vardef::DefRequireSecureTransport,
        vardef::ScopeGlobal,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBEnableAsyncCommit,
        vardef::DefTiDBEnableAsyncCommit,
        scope_both(),
    ));
    let mut historical_stats = bool_var(
        vardef::TiDBEnableHistoricalStats,
        false,
        vardef::ScopeGlobal,
    );
    historical_stats.Depended = true;
    RegisterSysVar(historical_stats);
    let build_stats = int_var(
        vardef::TiDBBuildStatsConcurrency,
        vardef::DefBuildStatsConcurrency,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    );
    RegisterSysVar(build_stats.clone());
    RegisterSysVar(int_var(
        vardef::TiDBAutoBuildStatsConcurrency,
        vardef::DefTiDBAutoBuildStatsConcurrency,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBAnalyzeDistSQLScanConcurrency,
        vardef::DefAnalyzeDistSQLScanConcurrency,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBSysProcScanConcurrency,
        vardef::DefTiDBSysProcScanConcurrency,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    ));
}

/// 构造取值非法类变量错误。
fn error(message: impl Into<String>) -> VariableError {
    VariableError::new(VariableErrorKind::InvalidValue, message)
}

/// 注册带上下限钳制的基础系统变量（TiFlash/执行时限等）。
fn register_basic_clamped_vars() {
    let mut allow_mpp = bool_var(
        vardef::TiDBAllowMPPExecution,
        vardef::DefTiDBAllowMPPExecution,
        scope_both(),
    );
    allow_mpp.Depended = true;
    RegisterSysVar(allow_mpp);
    RegisterSysVar(bool_var(
        vardef::TiDBAllowTiFlashCop,
        vardef::DefTiDBAllowTiFlashCop,
        scope_both(),
    ));
    RegisterSysVar(bool_var(
        vardef::TiFlashFastScan,
        vardef::DefTiFlashFastScan,
        scope_both(),
    ));
    let enforce_mpp = bool_var(
        vardef::TiDBEnforceMPPExecution,
        vardef::DefTiDBEnforceMPPExecution,
        scope_both(),
    );
    RegisterSysVar(enforce_mpp);
    RegisterSysVar(int_var(
        vardef::CTEMaxRecursionDepth,
        vardef::DefCTEMaxRecursionDepth,
        scope_both(),
        0,
        u32::MAX as u64,
    ));
    // 注册 TiFlash 线程/shuffle、执行时限、外部落盘阈值等带钳制变量。
    RegisterSysVar(int_var(
        vardef::TiDBMaxTiFlashThreads,
        vardef::DefTiFlashMaxThreads,
        scope_both(),
        -1,
        vardef::MaxConfigurableConcurrency as u64,
    ));
    RegisterSysVar(int_var(
        vardef::TiFlashFineGrainedShuffleStreamCount,
        vardef::DefTiFlashFineGrainedShuffleStreamCount,
        scope_both(),
        -1,
        1024,
    ));
    let mut select_limit = unsigned_var("sql_select_limit", u64::MAX, scope_both(), 0, u64::MAX);
    select_limit.SetSession = Some(Arc::new(|vars, value| {
        vars.SelectLimit = value
            .parse()
            .map_err(|_| VariableError::wrong_type("sql_select_limit"))?;
        Ok(())
    }));
    RegisterSysVar(select_limit);

    let mut max_execution = unsigned_var("max_execution_time", 0, scope_both(), 0, u64::MAX);
    max_execution.SetSession = Some(Arc::new(|vars, value| {
        vars.MaxExecutionTime = value
            .parse()
            .map_err(|_| VariableError::wrong_type("max_execution_time"))?;
        Ok(())
    }));
    RegisterSysVar(max_execution);

    let mut max_keys = int_var("tidb_max_keys_read", 0, scope_both(), 0, i64::MAX as u64);
    max_keys.IsHintUpdatableVerified = true;
    max_keys.SetSession = Some(Arc::new(|vars, value| {
        vars.MaxKeysRead = value
            .parse()
            .map_err(|_| VariableError::wrong_type("tidb_max_keys_read"))?;
        Ok(())
    }));
    RegisterSysVar(max_keys);

    let mut backoff_lock_fast = unsigned_var(
        vardef::TiDBBackoffLockFast,
        kv::DefBackoffLockFast as u64,
        scope_both(),
        1,
        i32::MAX as u64,
    );
    backoff_lock_fast.SetSession = Some(Arc::new(|vars, value| {
        vars.KVVars.BackoffLockFast = crate::tidbOptPositiveInt32(value, kv::DefBackoffLockFast);
        Ok(())
    }));
    RegisterSysVar(backoff_lock_fast);

    let mut backoff_weight = unsigned_var(
        vardef::TiDBBackOffWeight,
        kv::DefBackOffWeight as u64,
        scope_both(),
        0,
        i32::MAX as u64,
    );
    backoff_weight.SetSession = Some(Arc::new(|vars, value| {
        vars.KVVars.BackOffWeight = crate::tidbOptPositiveInt32(value, kv::DefBackOffWeight);
        Ok(())
    }));
    RegisterSysVar(backoff_weight);

    for (name, setter) in [
        ("tidb_max_bytes_before_tiflash_external_join", 0_u8),
        ("tidb_max_bytes_before_tiflash_external_group_by", 1_u8),
        ("tidb_max_bytes_before_tiflash_external_sort", 2_u8),
    ] {
        let mut variable = int_var(name, -1, scope_both(), -1, i64::MAX as u64);
        variable.SetSession = Some(Arc::new(move |vars, value| {
            let value = value.parse().map_err(|_| VariableError::wrong_type(name))?;
            match setter {
                0 => vars.TiFlashMaxBytesBeforeExternalJoin = value,
                1 => vars.TiFlashMaxBytesBeforeExternalGroupBy = value,
                _ => vars.TiFlashMaxBytesBeforeExternalSort = value,
            }
            Ok(())
        }));
        RegisterSysVar(variable);
    }

    let mut quota = int_var(
        "tiflash_mem_quota_query_per_node",
        -1,
        scope_both(),
        -1,
        i64::MAX as u64,
    );
    quota.SetSession = Some(Arc::new(|vars, value| {
        vars.TiFlashMaxQueryMemoryPerNode = value
            .parse()
            .map_err(|_| VariableError::wrong_type("tiflash_mem_quota_query_per_node"))?;
        Ok(())
    }));
    RegisterSysVar(quota);

    let mut spill = float_var("tiflash_query_spill_ratio", 0.0, scope_both(), 0, 1);
    spill.Validation = Some(Arc::new(|_, normalized, original, _| {
        let value = normalized
            .parse::<f64>()
            .map_err(|_| VariableError::wrong_type("tiflash_query_spill_ratio"))?;
        if value > 0.85 {
            return Err(VariableError::wrong_value(
                "tiflash_query_spill_ratio",
                original,
            ));
        }
        Ok(normalized.to_owned())
    }));
    spill.SetSession = Some(Arc::new(|vars, value| {
        vars.TiFlashQuerySpillRatio = value
            .parse()
            .map_err(|_| VariableError::wrong_type("tiflash_query_spill_ratio"))?;
        Ok(())
    }));
    RegisterSysVar(spill);

    RegisterSysVar(unsigned_var(
        "tidb_batch_pending_tiflash_count",
        0,
        scope_both(),
        0,
        u32::MAX as u64,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBMemQuotaQuery,
        vardef::DefTiDBMemQuotaQuery,
        scope_both(),
        -1,
        i64::MAX as u64,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBMemQuotaApplyCache,
        vardef::DefTiDBMemQuotaApplyCache,
        scope_both(),
        -1,
        i64::MAX as u64,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBBatchInsert,
        vardef::DefBatchInsert,
        vardef::ScopeSession,
    ));
    let dml_batch_size = unsigned_var(
        vardef::TiDBDMLBatchSize,
        vardef::DefDMLBatchSize as u64,
        scope_both(),
        0,
        i32::MAX as u64,
    );
    RegisterSysVar(dml_batch_size);
    RegisterSysVar(bool_var(
        vardef::TiDBEnableRateLimitAction,
        vardef::DefTiDBEnableRateLimitAction,
        scope_both(),
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBEnableCascadesPlanner,
        false,
        vardef::ScopeSession,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBEnableFullOuterJoin,
        vardef::DefTiDBEnableFullOuterJoin,
        scope_both(),
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBEnablePseudoForOutdatedStats,
        vardef::DefTiDBEnablePseudoForOutdatedStats,
        scope_both(),
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBEnableAnalyzeSnapshot,
        vardef::DefTiDBEnableAnalyzeSnapshot,
        scope_both(),
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBOptEnableHashJoin,
        vardef::DefTiDBOptEnableHashJoin,
        vardef::ScopeSession,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBOptEnableSemiJoinRewrite,
        vardef::DefOptEnableSemiJoinRewrite,
        vardef::ScopeSession,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBOptEnableAlternativeLogicalPlans,
        vardef::DefOptEnableAlternativeLogicalPlans,
        vardef::ScopeSession,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBOptExplainNoEvaledSubQuery,
        false,
        vardef::ScopeSession,
    ));
    RegisterSysVar(int_var(
        "tidb_query_log_max_len",
        4096,
        vardef::ScopeGlobal,
        0,
        1_073_741_824,
    ));
    RegisterSysVar(int_var(
        "tidb_committer_concurrency",
        128,
        vardef::ScopeGlobal,
        1,
        10_000,
    ));
    RegisterSysVar(unsigned_var(
        "tidb_ddl_flashback_concurrency",
        8,
        vardef::ScopeGlobal,
        1,
        vardef::MaxConfigurableConcurrency as u64,
    ));
    RegisterSysVar(unsigned_var(
        "net_buffer_length",
        16_384,
        vardef::ScopeGlobal,
        1_024,
        1_048_576,
    ));
    RegisterSysVar(int_var(
        "tidb_ddl_reorg_worker_cnt",
        4,
        vardef::ScopeGlobal,
        1,
        256,
    ));
    RegisterSysVar(int_var(
        "tidb_ddl_reorg_batch_size",
        256,
        vardef::ScopeGlobal,
        32,
        10_240,
    ));
    RegisterSysVar(int_var(
        "tidb_low_resolution_tso_update_interval",
        2_000,
        vardef::ScopeGlobal,
        10,
        60_000,
    ));
    RegisterSysVar(float_var(
        "tidb_cb_pd_metadata_error_rate_threshold_ratio",
        0.1,
        vardef::ScopeGlobal,
        0,
        1,
    ));
    RegisterSysVar(float_var(
        "tidb_opt_selectivity_factor",
        0.8,
        scope_both(),
        0,
        1,
    ));
}

/// 注册优化器调参相关系统变量。
fn register_planner_tuning_vars() {
    let mut executor_concurrency = unsigned_var(
        vardef::TiDBExecutorConcurrency,
        vardef::DefExecutorConcurrency as u64,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    );
    executor_concurrency.IsHintUpdatableVerified = true;
    RegisterSysVar(executor_concurrency);

    // Deprecated but still observable: Go keeps this variable so existing
    // sessions and regression tests can override the index-join worker count
    // independently before falling back to tidb_executor_concurrency.
    let mut index_lookup_join_concurrency = int_var(
        vardef::TiDBIndexLookupJoinConcurrency,
        vardef::DefIndexLookupJoinConcurrency as i64,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    );
    index_lookup_join_concurrency.AllowAutoValue = true;
    RegisterSysVar(index_lookup_join_concurrency);

    let mut hash_join_concurrency = int_var(
        vardef::TiDBHashJoinConcurrency,
        vardef::DefTiDBHashJoinConcurrency,
        scope_both(),
        1,
        vardef::MaxConfigurableConcurrency as u64,
    );
    hash_join_concurrency.AllowAutoValue = true;
    RegisterSysVar(hash_join_concurrency);

    let mut parallel_apply = bool_var(
        vardef::TiDBEnableParallelApply,
        vardef::DefTiDBEnableParallelApply,
        scope_both(),
    );
    parallel_apply.IsHintUpdatableVerified = true;
    RegisterSysVar(parallel_apply);

    // 注册优化器代价模型与选择性相关调参变量。
    // Keep these definitions aligned with the optimizer-facing entries in
    // Go's `defaultSysVars`. Both are legal in SET_VAR hints, so their session
    // hooks must update the typed SessionVars fields in addition to the common
    // string-valued system map maintained by SetSessionFromHook.
    let mut default_string_match_selectivity = float_var(
        vardef::TiDBDefaultStrMatchSelectivity,
        vardef::DefTiDBDefaultStrMatchSelectivity as f64,
        scope_both(),
        0,
        1,
    );
    default_string_match_selectivity.SetSession = Some(Arc::new(|vars, value| {
        vars.DefaultStrMatchSelectivity = value
            .parse()
            .map_err(|_| VariableError::wrong_type(vardef::TiDBDefaultStrMatchSelectivity))?;
        Ok(())
    }));
    RegisterSysVar(default_string_match_selectivity);

    // Keep the MPP broadcast-join thresholds configurable at session scope.
    // The physical join enumerator reads the normalized values from the common
    // session system-variable map, matching Go's two typed SessionVars fields.
    RegisterSysVar(int_var(
        vardef::TiDBBCJThresholdCount,
        vardef::DefBroadcastJoinThresholdCount,
        scope_both(),
        0,
        i64::MAX as u64,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBBCJThresholdSize,
        vardef::DefBroadcastJoinThresholdSize,
        scope_both(),
        0,
        i64::MAX as u64,
    ));

    let mut partial_ordered_index = enum_var(
        vardef::TiDBOptPartialOrderedIndexForTopN,
        vardef::DefTiDBOptPartialOrderedIndexForTopN,
        scope_both(),
        &["DISABLE", "COST"],
    );
    partial_ordered_index.IsHintUpdatableVerified = true;
    partial_ordered_index.Validation = Some(Arc::new(|_, _normalized, original, _| {
        let upper = original.trim().to_ascii_uppercase();
        if matches!(upper.as_str(), "DISABLE" | "COST") {
            Ok(upper)
        } else {
            Err(VariableError::wrong_value(
                vardef::TiDBOptPartialOrderedIndexForTopN,
                original,
            ))
        }
    }));
    partial_ordered_index.SetSession = Some(Arc::new(|vars, value| {
        vars.OptPartialOrderedIndexForTopN = value.to_ascii_uppercase();
        Ok(())
    }));
    RegisterSysVar(partial_ordered_index);

    let mut fuzzy_binding = bool_var(vardef::TiDBOptEnableFuzzyBinding, false, scope_both());
    fuzzy_binding.IsHintUpdatableVerified = true;
    fuzzy_binding.SetSession = Some(Arc::new(|vars, value| {
        vars.EnableFuzzyBinding = TiDBOptOn(value);
        Ok(())
    }));
    RegisterSysVar(fuzzy_binding);

    RegisterSysVar(int_var(
        vardef::TiDBOptJoinReorderThreshold,
        vardef::DefTiDBOptJoinReorderThreshold,
        scope_both(),
        0,
        63,
    ));

    let advanced_join_reorder = bool_var(
        vardef::TiDBOptEnableAdvancedJoinReorder,
        vardef::DefTiDBOptEnableAdvancedJoinReorder,
        scope_both(),
    );
    RegisterSysVar(advanced_join_reorder);
    RegisterSysVar(bool_var(
        vardef::TiDBOptEnableLateMaterialization,
        vardef::DefTiDBOptEnableLateMaterialization,
        scope_both(),
    ));

    let mut range_max_size = int_var(
        vardef::TiDBOptRangeMaxSize,
        vardef::DefTiDBOptRangeMaxSize,
        scope_both(),
        0,
        i64::MAX as u64,
    );
    range_max_size.IsHintUpdatableVerified = true;
    RegisterSysVar(range_max_size);

    let mut isolation_read_engines = string_var(
        vardef::TiDBIsolationReadEngines,
        "tikv,tiflash,tidb",
        scope_both(),
    );
    isolation_read_engines.Validation = Some(Arc::new(|_, _, original, _| {
        let mut engines = Vec::new();
        for engine in original.split(',').map(str::trim) {
            let engine = engine.to_ascii_lowercase();
            if !matches!(engine.as_str(), "tikv" | "tiflash" | "tidb") {
                return Err(VariableError::wrong_value(
                    vardef::TiDBIsolationReadEngines,
                    original,
                ));
            }
            if !engines.contains(&engine) {
                engines.push(engine);
            }
        }
        if engines.is_empty() {
            return Err(VariableError::wrong_value(
                vardef::TiDBIsolationReadEngines,
                original,
            ));
        }
        Ok(engines.join(","))
    }));
    RegisterSysVar(isolation_read_engines);

    let mut tiflash_preagg = string_var(
        vardef::TiFlashHashAggPreAggMode,
        vardef::DefTiFlashPreAggMode,
        scope_both(),
    );
    tiflash_preagg.Validation = Some(Arc::new(|_, normalized, original, _| {
        if matches!(
            normalized,
            vardef::ForcePreAggStr | vardef::AutoStr | vardef::ForceStreamingStr
        ) {
            Ok(normalized.to_owned())
        } else {
            Err(error(format!(
                "incorrect value: `{original}`. {} options: {}",
                vardef::TiFlashHashAggPreAggMode,
                crate::session::ValidTiFlashPreAggMode()
            )))
        }
    }));
    RegisterSysVar(tiflash_preagg);
}

/// 注册 SQL 模式、时区、事务隔离等会话/SQL 系统变量。
fn register_sql_and_session_vars() {
    // 注册 sql_mode、时区、事务隔离、字符集等经典 MySQL/TiDB 会话变量。
    // Connector/J（DataGrip 使用）会在建连时一次读取这些 MySQL 兼容变量。
    // 保持它们位于规范 SysVar 注册表中，让所有会话按作用域读取默认值。
    RegisterSysVar(string_var(vardef::SystemTimeZone, "CST", vardef::ScopeNone));
    RegisterSysVar(string_var(
        "license",
        "Apache License 2.0",
        vardef::ScopeNone,
    ));
    RegisterSysVar(unsigned_var(
        vardef::LowerCaseTableNames,
        2,
        vardef::ScopeNone,
        0,
        2,
    ));
    RegisterSysVar(bool_var(
        vardef::PerformanceSchema,
        false,
        vardef::ScopeNone,
    ));
    RegisterSysVar(unsigned_var(
        vardef::AutoIncrementIncrement,
        vardef::DefAutoIncrementIncrement as u64,
        scope_both(),
        1,
        u16::MAX as u64,
    ));
    RegisterSysVar(unsigned_var(
        vardef::AutoIncrementOffset,
        vardef::DefAutoIncrementOffset as u64,
        scope_both(),
        1,
        u16::MAX as u64,
    ));
    RegisterSysVar(string_var(
        vardef::CharacterSetClient,
        "utf8mb4",
        scope_both(),
    ));
    RegisterSysVar(string_var(
        vardef::CharacterSetResults,
        "utf8mb4",
        scope_both(),
    ));
    RegisterSysVar(string_var(vardef::InitConnect, "", vardef::ScopeGlobal));
    RegisterSysVar(unsigned_var(
        vardef::InteractiveTimeout,
        vardef::DefWaitTimeout as u64,
        scope_both(),
        1,
        31_536_000,
    ));
    RegisterSysVar(unsigned_var(
        vardef::NetWriteTimeout,
        60,
        scope_both(),
        1,
        31_536_000,
    ));
    RegisterSysVar(unsigned_var(
        vardef::WaitTimeout,
        vardef::DefWaitTimeout as u64,
        scope_both(),
        0,
        31_536_000,
    ));
    RegisterSysVar(bool_var(vardef::AutoCommit, true, scope_both()));
    RegisterSysVar(unsigned_var(
        vardef::MaxAllowedPacket,
        vardef::DefMaxAllowedPacket,
        scope_both(),
        1024,
        vardef::MaxOfMaxAllowedPacket.Load(),
    ));
    RegisterSysVar(unsigned_var(
        vardef::MaxConnections,
        0,
        vardef::ScopeInstance,
        0,
        100_000,
    ));

    let mut trace_event = string_var("tidb_trace_event", "", vardef::ScopeGlobal);
    trace_event.Validation = Some(Arc::new(|_, _, original, _| {
        if crate::kerneltype::IsClassic() {
            return Err(error("tidb_trace_event is unavailable in classic kernel"));
        }
        if original.is_empty()
            || (original.starts_with('{')
                && original.ends_with('}')
                && original.contains("enabled_categories")
                && original.contains("dump_trigger"))
        {
            Ok(original.to_owned())
        } else {
            Err(VariableError::wrong_value("tidb_trace_event", original))
        }
    }));
    trace_event.SetGlobal = Some(Arc::new(|_, _, value| {
        SetTraceEventConfig(value)
            .map_err(|error| VariableError::wrong_value("tidb_trace_event", &error))
    }));
    trace_event.GetGlobal = Some(Arc::new(|_, _| match get_flight_recorder() {
        Some(recorder) => serde_json::to_string(&recorder.config)
            .map_err(|error| VariableError::wrong_value("tidb_trace_event", &error.to_string())),
        None => Ok(String::new()),
    }));
    RegisterSysVar(trace_event);

    let mut sql_mode = string_var(
        "sql_mode",
        "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION",
        scope_both(),
    );
    sql_mode.Validation = Some(Arc::new(|_, _, original, _| {
        const MODES: &[&str] = &[
            "ONLY_FULL_GROUP_BY",
            "STRICT_TRANS_TABLES",
            "NO_ZERO_IN_DATE",
            "NO_ZERO_DATE",
            "ERROR_FOR_DIVISION_BY_ZERO",
            "NO_AUTO_CREATE_USER",
            "NO_ENGINE_SUBSTITUTION",
            "REAL_AS_FLOAT",
            "ANSI_QUOTES",
        ];
        let mut normalized = Vec::new();
        for mode in original
            .split(',')
            .map(str::trim)
            .filter(|mode| !mode.is_empty())
        {
            let upper = mode.to_ascii_uppercase();
            if !MODES.contains(&upper.as_str()) {
                return Err(VariableError::wrong_value("sql_mode", &upper));
            }
            normalized.push(upper);
        }
        Ok(normalized.join(","))
    }));
    RegisterSysVar(sql_mode);

    let mut collation = string_var("collation_server", "utf8mb4_bin", scope_both());
    collation.Validation = Some(Arc::new(checkCollation));
    collation.SetSession = Some(Arc::new(|vars, value| {
        let charset = value.split('_').next().unwrap_or(value);
        vars.set_system("character_set_server", charset);
        Ok(())
    }));
    RegisterSysVar(collation);

    let mut default_collation =
        string_var("default_collation_for_utf8mb4", "utf8mb4_bin", scope_both());
    default_collation.Validation = Some(Arc::new(|vars, normalized, original, scope| {
        let value = checkCollation(vars, normalized, original, scope)?;
        if matches!(
            value.as_str(),
            "utf8mb4_bin" | "utf8mb4_general_ci" | "utf8mb4_0900_ai_ci"
        ) {
            vars.StmtCtx.append_warning(error(
                "Updating 'default_collation_for_utf8mb4' is deprecated. It will be made read-only in a future release.",
            ));
            Ok(value)
        } else {
            Err(error(format!(
                "Invalid default utf8mb4 collation '{value}'"
            )))
        }
    }));
    RegisterSysVar(default_collation);

    let mut timezone = string_var("time_zone", "SYSTEM", scope_both());
    timezone.Validation = Some(Arc::new(|_, _, original, _| {
        let valid_named = matches!(original, "UTC" | "SYSTEM" | "America/Edmonton");
        let valid_offset = original
            .strip_prefix(['+', '-'])
            .and_then(|value| value.split_once(':'))
            .is_some_and(|(hours, minutes)| {
                hours.parse::<u8>().is_ok_and(|hours| hours <= 14)
                    && minutes.parse::<u8>().is_ok_and(|minutes| minutes < 60)
            });
        if valid_named || valid_offset {
            Ok(original.to_owned())
        } else {
            Err(VariableError::wrong_value("time_zone", original))
        }
    }));
    timezone.SetSession = Some(Arc::new(|vars, value| {
        if value == "UTC" || value == "+00:00" {
            vars.set_location(chrono::FixedOffset::east_opt(0).unwrap());
        } else if let Some((sign, rest)) = value.split_at_checked(1)
            && matches!(sign, "+" | "-")
            && let Some((hours, minutes)) = rest.split_once(':')
        {
            let seconds = (hours.parse::<i32>().unwrap_or(0) * 60
                + minutes.parse::<i32>().unwrap_or(0))
                * 60
                * if sign == "-" { -1 } else { 1 };
            vars.set_location(chrono::FixedOffset::east_opt(seconds).unwrap());
        }
        Ok(())
    }));
    RegisterSysVar(timezone);

    let mut isolation = enum_var(
        "tx_isolation",
        "REPEATABLE-READ",
        scope_both(),
        &[
            "READ-UNCOMMITTED",
            "READ-COMMITTED",
            "REPEATABLE-READ",
            "SERIALIZABLE",
        ],
    );
    isolation.Validation = Some(Arc::new(checkIsolationLevel));
    isolation.Aliases = vec!["transaction_isolation".to_owned()];
    RegisterSysVar(isolation.clone());
    isolation.Name = "transaction_isolation".to_owned();
    isolation.Aliases.clear();
    RegisterSysVar(isolation);
    RegisterSysVar(bool_var(
        "tidb_skip_isolation_level_check",
        false,
        scope_both(),
    ));

    let mut multi = enum_var(
        "tidb_multi_statement_mode",
        "OFF",
        scope_both(),
        &["OFF", "ON", "WARN"],
    );
    multi.SetSession = Some(Arc::new(|vars, value| {
        vars.MultiStatementMode = match value {
            "ON" => 1,
            "WARN" => 2,
            _ => 0,
        };
        Ok(())
    }));
    RegisterSysVar(multi);

    for (name, offline) in [
        ("tx_read_only", false),
        ("transaction_read_only", false),
        ("offline_mode", true),
        ("super_read_only", false),
        ("read_only", false),
    ] {
        let mut readonly = bool_var(name, false, scope_both());
        readonly.Validation = Some(Arc::new(move |vars, normalized, original, scope| {
            crate::checkReadOnly(vars, normalized, original, scope, offline)
        }));
        RegisterSysVar(readonly);
    }

    let mut noop = enum_var(
        "tidb_enable_noop_functions",
        "OFF",
        scope_both(),
        &["OFF", "ON", "WARN"],
    );
    noop.Depended = true;
    noop.SetSession = Some(Arc::new(|vars, value| {
        if value == "OFF" && vars.system("sql_auto_is_null").is_some_and(TiDBOptOn) {
            return Err(error(
                "tidb_enable_noop_functions cannot be disabled while sql_auto_is_null is enabled",
            ));
        }
        vars.NoopFuncsMode = match value {
            "ON" => 1,
            "WARN" => 2,
            _ => 0,
        };
        Ok(())
    }));
    RegisterSysVar(noop);

    let mut auto_is_null = bool_var("sql_auto_is_null", false, scope_both());
    auto_is_null.Validation = Some(Arc::new(|vars, normalized, _, scope| {
        if TiDBOptOn(normalized) {
            let enabled = if scope == vardef::ScopeSession {
                vars.NoopFuncsMode != 0
            } else {
                vars.global("tidb_enable_noop_functions")
                    .is_some_and(|value| value != "OFF")
            };
            if !enabled {
                return Err(error(
                    "function SQL_AUTO_IS_NULL has only noop implementation",
                ));
            }
        }
        Ok(normalized.to_owned())
    }));
    RegisterSysVar(auto_is_null);

    let mut secure_auth = bool_var("secure_auth", true, vardef::ScopeGlobal);
    secure_auth.Validation = Some(Arc::new(|_, normalized, original, _| {
        if normalized == "ON" {
            Ok(normalized.to_owned())
        } else {
            Err(VariableError::wrong_value("secure_auth", original))
        }
    }));
    RegisterSysVar(secure_auth);

    let mut replica = enum_var(
        "tidb_replica_read",
        "leader",
        scope_both(),
        &[
            "leader",
            "follower",
            "leader-and-follower",
            "closest-replicas",
        ],
    );
    replica.Validation = Some(Arc::new(|_, normalized, _, _| {
        if crate::kerneltype::IsNextGen() && normalized != "leader" {
            return Err(error("replica read mode is not supported in next-gen"));
        }
        Ok(normalized.to_owned())
    }));
    RegisterSysVar(replica);

    for name in ["tiflash_hash_join_version", "tidb_hash_join_version"] {
        RegisterSysVar(enum_var(
            name,
            "optimized",
            scope_both(),
            &["legacy", "optimized"],
        ));
    }

    let mut dispatch_policy = string_var(
        vardef::TiFlashComputeDispatchPolicy,
        vardef::DefTiFlashComputeDispatchPolicy,
        scope_both(),
    );
    dispatch_policy.Validation = Some(Arc::new(|_, _, original, _| {
        crate::tiflashcompute::GetDispatchPolicyByStr(original)
            .map(crate::tiflashcompute::GetDispatchPolicy)
            .map(str::to_owned)
            .map_err(|error_value| error(error_value.to_string()))
    }));
    RegisterSysVar(dispatch_policy);
}

/// 注册只读 getter 与动态默认值变量。
fn register_getters_and_defaults() {
    // 注册只读状态 getter 与依赖运行时的默认值钩子。
    let session_getters: [(&str, crate::GetSessionHook); 5] = [
        (
            "tidb_current_ts",
            Arc::new(|vars| Ok(vars.TxnStartTS.to_string())),
        ),
        (
            "tidb_last_txn_info",
            Arc::new(|vars| Ok(vars.LastTxnInfo.clone())),
        ),
        (
            "tidb_last_query_info",
            Arc::new(|vars| Ok(vars.LastQueryInfo.clone())),
        ),
        (
            "tidb_found_in_plan_cache",
            Arc::new(|vars| Ok(BoolToOnOff(vars.PrevFoundInPlanCache))),
        ),
        (
            vardef::TiDBFoundInBinding,
            Arc::new(|vars| Ok(BoolToOnOff(vars.PrevFoundInBinding))),
        ),
    ];
    for (name, getter) in session_getters {
        let mut variable = string_var(name, "", vardef::ScopeSession);
        variable.GetSession = Some(getter);
        RegisterSysVar(variable);
    }

    let mut warning_count = string_var(vardef::WarningCount, "0", vardef::ScopeSession);
    warning_count.ReadOnly = true;
    warning_count.GetSession = Some(Arc::new(|vars| Ok(vars.StmtCtx.WarningCount().to_string())));
    RegisterSysVar(warning_count);

    for name in ["last_insert_id", "identity"] {
        let mut variable = unsigned_var(name, 0, vardef::ScopeSession, 0, u64::MAX);
        variable.GetSession = Some(Arc::new(|vars| {
            Ok(vars.StmtCtx.PrevLastInsertID.to_string())
        }));
        RegisterSysVar(variable);
    }

    let mut timestamp = float_var("timestamp", 0.0, vardef::ScopeSession, 0, i32::MAX as u64);
    timestamp.GetSession = Some(Arc::new(|vars| {
        if let Some(value) = vars.system("timestamp")
            && value != "0"
        {
            return Ok(value.to_owned());
        }
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string())
    }));
    timestamp.Validation = Some(Arc::new(|_vars, normalized, original, _| {
        let value = original
            .parse::<f64>()
            .map_err(|_| VariableError::wrong_type("timestamp"))?;
        if value > i32::MAX as f64 {
            return Err(VariableError::wrong_value("timestamp", original));
        }
        // TypeFloat already clamps negative values to zero and records the
        // single truncation warning produced by the Go implementation.
        Ok(normalized.to_owned())
    }));
    RegisterSysVar(timestamp);

    let mut window = bool_var(
        "tidb_enable_window_function",
        vardef::DefEnableWindowFunction,
        scope_both(),
    );
    window.SetSession = Some(Arc::new(|vars, value| {
        vars.EnableWindowFunction = TiDBOptOn(value);
        Ok(())
    }));
    RegisterSysVar(window);
    RegisterSysVar(bool_var(
        vardef::TiDBEnablePipelinedWindowFunction,
        vardef::DefEnablePipelinedWindowFunction,
        scope_both(),
    ));

    let mut hash_join = GetSysVarDefinition("tidb_hash_join_version").unwrap();
    hash_join.SetSession = Some(Arc::new(|vars, value| {
        vars.UseHashJoinV2 = value.eq_ignore_ascii_case("optimized");
        Ok(())
    }));
    RegisterSysVar(hash_join);

    let mut lc_time = string_var("lc_time_names", "en_US", vardef::ScopeGlobal);
    lc_time.ReadOnly = true;
    RegisterSysVar(lc_time);
    RegisterSysVar(string_var("lc_messages", "en_US", scope_both()));
    RegisterSysVar(string_var(
        "character_set_connection",
        "utf8mb4",
        scope_both(),
    ));
    RegisterSysVar(string_var(
        "collation_connection",
        "utf8mb4_bin",
        scope_both(),
    ));
    RegisterSysVar(bool_var("tidb_enable_index_merge", true, scope_both()));
    RegisterSysVar(enum_var(
        vardef::TiDBEnableClusteredIndex,
        vardef::On,
        scope_both(),
        &[vardef::Off, vardef::On, vardef::IntOnly],
    ));
    RegisterSysVar(int_var(
        vardef::TiDBAnalyzeVersion,
        vardef::DefTiDBAnalyzeVersion,
        scope_both(),
        1,
        2,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBCostModelVersion,
        vardef::DefTiDBCostModelVer,
        scope_both(),
        1,
        2,
    ));
    RegisterSysVar(int_var(
        vardef::TiDBOptRangeMaxSize,
        vardef::DefTiDBOptRangeMaxSize,
        scope_both(),
        0,
        i64::MAX as u64,
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBOptAdvancedJoinHint,
        vardef::DefTiDBOptAdvancedJoinHint,
        scope_both(),
    ));
    RegisterSysVar(bool_var(
        vardef::TiDBEnableINLJoinInnerMultiPattern,
        vardef::DefTiDBEnableINLJoinMultiPattern,
        scope_both(),
    ));
    let mut enable_paging = bool_var(
        vardef::TiDBEnablePaging,
        vardef::DefTiDBEnablePaging,
        scope_both(),
    );
    enable_paging.Hidden = true;
    RegisterSysVar(enable_paging);
    RegisterSysVar(string_var(
        "tidb_memory_debug_mode_min_heap_inuse",
        "0",
        scope_both(),
    ));
    RegisterSysVar(string_var(
        "tidb_memory_debug_mode_alarm_ratio",
        "0",
        scope_both(),
    ));

    let mut general_log = bool_var(
        vardef::TiDBGeneralLog,
        vardef::DefTiDBGeneralLog,
        vardef::ScopeInstance,
    );
    general_log.SetGlobal = Some(Arc::new(|_, _, value| {
        vardef::ProcessGeneralLog.Store(TiDBOptOn(value));
        Ok(())
    }));
    general_log.GetGlobal = Some(Arc::new(|_, _| {
        Ok(BoolToOnOff(vardef::ProcessGeneralLog.Load()))
    }));
    RegisterSysVar(general_log);

    for (name, value) in [
        ("tidb_pprof_sql_cpu", "0"),
        ("tidb_expensive_query_time_threshold", "60"),
        ("tidb_expensive_txn_time_threshold", "600"),
        ("tidb_memory_usage_alarm_ratio", "0.8"),
        ("tidb_memory_usage_alarm_keep_record_num", "5"),
        ("tidb_force_priority", "NO_PRIORITY"),
        (vardef::TiDBDDLSlowOprThreshold, "300"),
        ("plugin_dir", ""),
        ("plugin_load", ""),
        ("tidb_slow_log_threshold", "300"),
        ("tidb_record_plan_in_slow_log", "OFF"),
        ("tidb_enable_slow_log", "ON"),
        ("tidb_check_mb4_value_in_utf8", "ON"),
        ("tidb_enable_collect_execution_info", "ON"),
        ("tidb_config", "{}"),
        ("tidb_log_file_max_days", "0"),
        ("tidb_rc_read_check_ts", "OFF"),
    ] {
        let mut variable = string_var(name, value, vardef::ScopeInstance);
        let returned = value.to_owned();
        variable.GetGlobal = Some(Arc::new(move |_, _| Ok(returned.clone())));
        RegisterSysVar(variable);
    }
}

/// 注册仅全局作用域的系统变量。
fn register_global_vars() {
    // 注册仅 GLOBAL 作用域变量（DDL、内存限制、资源控制等）。
    for (name, default, min, max, state) in [
        (
            vardef::TiDBAnalyzeDefaultNumBuckets,
            vardef::DefTiDBAnalyzeDefaultNumBuckets,
            vardef::MinTiDBAnalyzeDefaultNumBuckets,
            vardef::MaxTiDBAnalyzeDefaultNumBuckets,
            &vardef::AnalyzeDefaultNumBuckets,
        ),
        (
            vardef::TiDBAnalyzeDefaultNumTopN,
            vardef::DefTiDBAnalyzeDefaultNumTopN,
            vardef::MinTiDBAnalyzeDefaultNumTopN,
            vardef::MaxTiDBAnalyzeDefaultNumTopN,
            &vardef::AnalyzeDefaultNumTopN,
        ),
    ] {
        let mut variable = unsigned_var(name, default, vardef::ScopeGlobal, min, max);
        variable.GetGlobal = Some(Arc::new(move |_, _| Ok(state.Load().to_string())));
        variable.SetGlobal = Some(Arc::new(move |_, _, value| {
            let parsed = value
                .parse::<u64>()
                .map_err(|_| VariableError::wrong_value(name, value))?;
            state.Store(parsed);
            Ok(())
        }));
        RegisterSysVar(variable);
    }

    let mut enable_stmt_summary = bool_var(
        vardef::TiDBEnableStmtSummary,
        vardef::DefTiDBEnableStmtSummary,
        vardef::ScopeGlobal,
    );
    enable_stmt_summary.AllowEmpty = true;
    RegisterSysVar(enable_stmt_summary);

    let mut mem_oom_action = enum_var(
        vardef::TiDBMemOOMAction,
        vardef::DefTiDBMemOOMAction,
        vardef::ScopeGlobal,
        &[vardef::OOMActionCancel, vardef::OOMActionLog],
    );
    mem_oom_action.GetGlobal = Some(Arc::new(|_, _| Ok(vardef::OOMAction.Load())));
    mem_oom_action.SetGlobal = Some(Arc::new(|_, _, value| {
        vardef::OOMAction.Store(value);
        Ok(())
    }));
    RegisterSysVar(mem_oom_action);

    let mut ts_validation = bool_var(
        vardef::TiDBEnableTSValidation,
        vardef::DefTiDBEnableTSValidation,
        vardef::ScopeGlobal,
    );
    ts_validation.SetGlobal = Some(Arc::new(|_, _, value| {
        TS_VALIDATION_ENABLED.store(TiDBOptOn(value), Ordering::SeqCst);
        Ok(())
    }));
    RegisterSysVar(ts_validation);

    // 保留旧持久化值兼容变量；planner 始终使用 v2 路径，因此 OFF 必须拒绝。
    let mut index_join_build_v2 = bool_var(
        vardef::TiDBOptIndexJoinBuild,
        vardef::DefTiDBOptIndexJoinBuild,
        scope_both(),
    );
    index_join_build_v2.Validation = Some(Arc::new(|_, normalized, _, _| {
        if !TiDBOptOn(normalized) {
            return Err(VariableError::new(
                VariableErrorKind::InvalidValue,
                "tidb_opt_index_join_build_v2 is now always enabled and cannot be turned off",
            ));
        }
        Ok(vardef::On.to_owned())
    }));
    index_join_build_v2.GetSession = Some(Arc::new(|_| Ok(vardef::On.to_owned())));
    index_join_build_v2.GetGlobal = Some(Arc::new(|_, _| Ok(vardef::On.to_owned())));
    RegisterSysVar(index_join_build_v2);

    let mut advancer_check_point_lag_limit = SysVar {
        Scope: vardef::ScopeGlobal,
        Name: vardef::TiDBAdvancerCheckPointLagLimit.to_owned(),
        Value: format_go_duration(vardef::DefTiDBAdvancerCheckPointLagLimit as i128),
        Type: vardef::TypeDuration,
        MinValue: 1_000_000_000,
        MaxValue: (365_i64 * 24 * 3_600_000_000_000) as u64,
        ..SysVar::default()
    };
    advancer_check_point_lag_limit.SetGlobal = Some(Arc::new(|_, _, value| {
        let duration = parse_go_duration(value)
            .and_then(|duration| i64::try_from(duration).ok())
            .ok_or_else(|| VariableError::wrong_type(vardef::TiDBAdvancerCheckPointLagLimit))?;
        vardef::AdvancerCheckPointLagLimit.Store(duration);
        Ok(())
    }));
    advancer_check_point_lag_limit.GetGlobal = Some(Arc::new(|_, _| {
        Ok(format_go_duration(
            vardef::AdvancerCheckPointLagLimit.Load() as i128,
        ))
    }));
    RegisterSysVar(advancer_check_point_lag_limit);

    let mut enable_instance_plan_cache = bool_var(
        vardef::TiDBEnableInstancePlanCache,
        false,
        vardef::ScopeGlobal,
    );
    enable_instance_plan_cache.GetGlobal = Some(Arc::new(|_, _| {
        Ok(BoolToOnOff(vardef::EnableInstancePlanCache.Load()))
    }));
    enable_instance_plan_cache.SetGlobal = Some(Arc::new(|_, _, value| {
        vardef::EnableInstancePlanCache.Store(TiDBOptOn(value));
        Ok(())
    }));
    RegisterSysVar(enable_instance_plan_cache);

    let mut instance_plan_cache_reserved_percentage = float_var(
        vardef::TiDBInstancePlanCacheReservedPercentage,
        vardef::DefTiDBInstancePlanCacheReservedPercentage,
        vardef::ScopeGlobal,
        0,
        1,
    );
    instance_plan_cache_reserved_percentage.GetGlobal = Some(Arc::new(|_, _| {
        Ok(vardef::InstancePlanCacheReservedPercentage
            .Load()
            .to_string())
    }));
    instance_plan_cache_reserved_percentage.SetGlobal = Some(Arc::new(|_, _, value| {
        let percentage = value.parse::<f64>().map_err(|_| {
            VariableError::wrong_type(vardef::TiDBInstancePlanCacheReservedPercentage)
        })?;
        if !(0.0..=1.0).contains(&percentage) {
            return Err(VariableError::wrong_value(
                vardef::TiDBInstancePlanCacheReservedPercentage,
                value,
            ));
        }
        vardef::InstancePlanCacheReservedPercentage.Store(percentage);
        Ok(())
    }));
    RegisterSysVar(instance_plan_cache_reserved_percentage);

    let mut instance_plan_cache_max_mem_size = string_var(
        vardef::TiDBInstancePlanCacheMaxMemSize,
        &vardef::DefTiDBInstancePlanCacheMaxMemSize.to_string(),
        vardef::ScopeGlobal,
    );
    instance_plan_cache_max_mem_size.GetGlobal = Some(Arc::new(|_, _| {
        Ok(vardef::InstancePlanCacheMaxMemSize.Load().to_string())
    }));
    instance_plan_cache_max_mem_size.SetGlobal = Some(Arc::new(|_, _, value| {
        let (bytes, normalized) = parseByteSize(value);
        if normalized.is_empty() || bytes < vardef::MinTiDBInstancePlanCacheMemSize as u64 {
            return Err(VariableError::wrong_value(
                vardef::TiDBInstancePlanCacheMaxMemSize,
                value,
            ));
        }
        vardef::InstancePlanCacheMaxMemSize.Store(bytes as i64);
        Ok(())
    }));
    RegisterSysVar(instance_plan_cache_max_mem_size);

    let mut redact_log = enum_var(
        vardef::TiDBRedactLog,
        vardef::DefTiDBRedactLog,
        scope_both(),
        &[vardef::Off, vardef::On, vardef::Marker],
    );
    redact_log.InternalSessionVariable = true;
    RegisterSysVar(redact_log);

    let mut restricted_read_only = bool_var(
        vardef::TiDBRestrictedReadOnly,
        vardef::DefTiDBRestrictedReadOnly,
        vardef::ScopeGlobal,
    );
    restricted_read_only.GetGlobal = Some(Arc::new(|_, _| {
        Ok(BoolToOnOff(vardef::RestrictedReadOnly.Load()))
    }));
    restricted_read_only.SetGlobal = Some(Arc::new(|_, _, value| {
        let enabled = TiDBOptOn(value);
        vardef::RestrictedReadOnly.Store(enabled);
        if enabled {
            vardef::VarTiDBSuperReadOnly.Store(true);
        }
        Ok(())
    }));
    RegisterSysVar(restricted_read_only);

    let mut super_read_only = bool_var(
        vardef::TiDBSuperReadOnly,
        vardef::DefTiDBSuperReadOnly,
        vardef::ScopeGlobal,
    );
    super_read_only.GetGlobal = Some(Arc::new(|_, _| {
        Ok(BoolToOnOff(vardef::VarTiDBSuperReadOnly.Load()))
    }));
    super_read_only.SetGlobal = Some(Arc::new(|_, _, value| {
        let enabled = TiDBOptOn(value);
        if !enabled && vardef::RestrictedReadOnly.Load() {
            return Err(error(
                "can't turn off tidb_super_read_only when tidb_restricted_read_only is on",
            ));
        }
        vardef::VarTiDBSuperReadOnly.Store(enabled);
        Ok(())
    }));
    RegisterSysVar(super_read_only);

    RegisterSysVar(int_var(
        vardef::TiDBDDLErrorCountLimit,
        vardef::DefTiDBDDLErrorCountLimit,
        vardef::ScopeGlobal,
        0,
        i64::MAX as u64,
    ));
    RegisterSysVar(string_var(
        vardef::TiDBDDLReorgMaxWriteSpeed,
        &vardef::DefTiDBDDLReorgMaxWriteSpeed.to_string(),
        vardef::ScopeGlobal,
    ));

    for (name, default) in [
        ("tidb_enable_dist_task", "OFF"),
        ("tidb_partition_prune_mode", "dynamic"),
        ("tidb_ddl_enable_fast_reorg", "ON"),
        ("tidb_opt_agg_push_down", "OFF"),
        (vardef::TiDBOptDeriveTopN, "OFF"),
        ("tidb_opt_distinct_agg_push_down", "OFF"),
        ("tidb_ignore_inlist_plan_digest", "ON"),
        ("tidb_enable_row_level_checksum", "OFF"),
    ] {
        let scope = match name {
            "tidb_opt_agg_push_down"
            | vardef::TiDBOptDeriveTopN
            | "tidb_opt_distinct_agg_push_down" => scope_both(),
            _ => vardef::ScopeGlobal,
        };
        let variable = if matches!(default, "ON" | "OFF") {
            bool_var(name, default == "ON", scope)
        } else {
            string_var(name, default, scope)
        };
        RegisterSysVar(variable);
    }

    let mut fast_ddl = bool_var("tidb_ddl_enable_fast_reorg", true, vardef::ScopeGlobal);
    fast_ddl.Value = "ON".to_owned();
    RegisterSysVar(fast_ddl);

    RegisterSysVar(unsigned_var(
        "tidb_ddl_disk_quota",
        100_u64 << 30,
        vardef::ScopeGlobal,
        (100_u64 << 30) as i64,
        1_u64 << 50,
    ));

    let mut memory_limit = string_var("tidb_server_memory_limit", "80%", vardef::ScopeGlobal);
    memory_limit.Validation = Some(Arc::new(|vars, normalized, original, _| {
        let (bytes, value) = parseMemoryLimit(vars, normalized, original)?;
        SERVER_MEMORY_LIMIT.store(bytes, Ordering::SeqCst);
        Ok(value)
    }));
    RegisterSysVar(memory_limit);

    let mut sess_min = string_var(
        "tidb_server_memory_limit_sess_min_size",
        &(128_u64 << 20).to_string(),
        vardef::ScopeGlobal,
    );
    sess_min.Validation = Some(Arc::new(|vars, normalized, original, _| {
        let (mut bytes, value) = parseByteSize(normalized);
        if value.is_empty() {
            return Err(VariableError::wrong_value(
                "tidb_server_memory_limit_sess_min_size",
                original,
            ));
        }
        if bytes > 0 && bytes < 128 {
            vars.StmtCtx.append_warning(VariableError::truncated(
                "tidb_server_memory_limit_sess_min_size",
                original,
            ));
            bytes = 128;
        }
        SERVER_MEMORY_LIMIT_SESS_MIN_SIZE.store(bytes, Ordering::SeqCst);
        Ok(bytes.to_string())
    }));
    RegisterSysVar(sess_min);

    let mut gc_trigger = string_var(
        "tidb_server_memory_limit_gc_trigger",
        "0.7",
        vardef::ScopeGlobal,
    );
    gc_trigger.Validation = Some(Arc::new(|_, _, original, _| {
        let value = if let Some(percent) = original.strip_suffix('%') {
            percent
                .parse::<f64>()
                .map_err(|_| VariableError::wrong_type("tidb_server_memory_limit_gc_trigger"))?
                / 100.0
        } else {
            original
                .parse::<f64>()
                .map_err(|_| VariableError::wrong_type("tidb_server_memory_limit_gc_trigger"))?
        };
        let threshold = f64::from_bits(GOGC_THRESHOLD_BITS.load(Ordering::SeqCst));
        if !(0.0..1.0).contains(&value) || value <= threshold + 0.1 {
            return Err(VariableError::wrong_value(
                "tidb_server_memory_limit_gc_trigger",
                original,
            ));
        }
        Ok(value.to_string())
    }));
    gc_trigger.SetGlobal = Some(Arc::new(|_, _, value| {
        let value = value
            .parse::<f64>()
            .map_err(|_| VariableError::wrong_type("tidb_server_memory_limit_gc_trigger"))?;
        SERVER_MEMORY_LIMIT_GC_TRIGGER_BITS.store(value.to_bits(), Ordering::SeqCst);
        Ok(())
    }));
    RegisterSysVar(gc_trigger);
    let mut threshold = float_var("tidb_gogc_tuner_threshold", 0.6, vardef::ScopeGlobal, 0, 1);
    threshold.SetGlobal = Some(Arc::new(|_, _, value| {
        let value = value
            .parse::<f64>()
            .map_err(|_| VariableError::wrong_type("tidb_gogc_tuner_threshold"))?;
        GOGC_THRESHOLD_BITS.store(value.to_bits(), Ordering::SeqCst);
        Ok(())
    }));
    RegisterSysVar(threshold);
    let mut min_value = int_var(
        vardef::TiDBGOGCTunerMinValue,
        vardef::DefTiDBGOGCMinValue,
        vardef::ScopeGlobal,
        10,
        i32::MAX as u64,
    );
    min_value.Validation = Some(Arc::new(|_, normalized, original, _| {
        let value = normalized
            .parse::<i64>()
            .map_err(|_| VariableError::wrong_type("tidb_gogc_tuner_min_value"))?;
        if value >= GOGC_MAX.load(Ordering::SeqCst) {
            Err(VariableError::wrong_value(
                "tidb_gogc_tuner_min_value",
                original,
            ))
        } else {
            Ok(normalized.to_owned())
        }
    }));
    min_value.SetGlobal = Some(Arc::new(|_, _, value| {
        GOGC_MIN.store(
            value.parse().unwrap_or(vardef::DefTiDBGOGCMinValue),
            Ordering::SeqCst,
        );
        Ok(())
    }));
    RegisterSysVar(min_value);
    let mut max_value = int_var(
        vardef::TiDBGOGCTunerMaxValue,
        vardef::DefTiDBGOGCMaxValue,
        vardef::ScopeGlobal,
        10,
        i32::MAX as u64,
    );
    max_value.Validation = Some(Arc::new(|_, normalized, original, _| {
        let value = normalized
            .parse::<i64>()
            .map_err(|_| VariableError::wrong_type("tidb_gogc_tuner_max_value"))?;
        if value <= GOGC_MIN.load(Ordering::SeqCst) {
            Err(VariableError::wrong_value(
                "tidb_gogc_tuner_max_value",
                original,
            ))
        } else {
            Ok(normalized.to_owned())
        }
    }));
    max_value.SetGlobal = Some(Arc::new(|_, _, value| {
        GOGC_MAX.store(
            value.parse().unwrap_or(vardef::DefTiDBGOGCMaxValue),
            Ordering::SeqCst,
        );
        Ok(())
    }));
    RegisterSysVar(max_value);

    let mut resource = bool_var("tidb_enable_resource_control", true, vardef::ScopeGlobal);
    resource.SetGlobal = Some(Arc::new(|_, _, value| {
        RESOURCE_CONTROL_ENABLED.store(TiDBOptOn(value), Ordering::SeqCst);
        Ok(())
    }));
    RegisterSysVar(resource);

    let mut strict = bool_var(
        "tidb_resource_control_strict_mode",
        true,
        vardef::ScopeGlobal,
    );
    strict.SetGlobal = Some(Arc::new(|_, _, value| {
        RESOURCE_CONTROL_STRICT.store(TiDBOptOn(value), Ordering::SeqCst);
        Ok(())
    }));
    RegisterSysVar(strict);

    let mut auto_ratio = string_var("tidb_auto_analyze_ratio", "0.5", vardef::ScopeGlobal);
    auto_ratio.Validation = Some(Arc::new(|_, _, original, _| {
        let value = original
            .parse::<f64>()
            .map_err(|_| VariableError::wrong_type("tidb_auto_analyze_ratio"))?;
        if value < 0.00001 {
            return Err(VariableError::wrong_value(
                "tidb_auto_analyze_ratio",
                original,
            ));
        }
        Ok(original.to_owned())
    }));
    RegisterSysVar(auto_ratio);

    let mut persist_analyze_options = bool_var(
        vardef::TiDBPersistAnalyzeOptions,
        vardef::DefTiDBPersistAnalyzeOptions,
        vardef::ScopeGlobal,
    );
    persist_analyze_options.GetGlobal = Some(Arc::new(|_, _| {
        Ok(BoolToOnOff(vardef::PersistAnalyzeOptions.Load()))
    }));
    persist_analyze_options.SetGlobal = Some(Arc::new(|_, _, value| {
        vardef::PersistAnalyzeOptions.Store(TiDBOptOn(value));
        Ok(())
    }));
    RegisterSysVar(persist_analyze_options);

    RegisterSysVar(enum_var(
        "tiflash_replica_read",
        "all_replicas",
        vardef::ScopeGlobal,
        &["all_replicas", "closest_adaptive", "closest_replicas"],
    ));

    let mut schema = string_var(
        "tidb_schema_cache_size",
        &(512_u64 << 20).to_string(),
        vardef::ScopeGlobal,
    );
    schema.Validation = Some(Arc::new(|vars, normalized, original, _| {
        let (bytes, value) = parseSchemaCacheSize(vars, normalized, original)?;
        SCHEMA_CACHE_SIZE.store(bytes, Ordering::SeqCst);
        Ok(value)
    }));
    RegisterSysVar(schema);

    let mut retry = bool_var("tidb_disable_txn_auto_retry", true, scope_both());
    retry.Validation = Some(Arc::new(|vars, normalized, original, _| {
        if normalized == "OFF" {
            vars.StmtCtx.append_warning(error(format!(
                "'{original}' is deprecated and will be removed in a future release. Please use ON instead"
            )));
        }
        Ok("ON".to_owned())
    }));
    RegisterSysVar(retry);

    let mut schedule = SysVar {
        Name: "tidb_ttl_job_schedule_window_start_time".to_owned(),
        Value: "00:00 +0000".to_owned(),
        Scope: vardef::ScopeGlobal,
        Type: vardef::TypeTime,
        ..SysVar::default()
    };
    schedule.GetGlobal = Some(Arc::new(|_, vars| {
        vars.GlobalVarsAccessor
            .get_global_sys_var("tidb_ttl_job_schedule_window_start_time")
    }));
    RegisterSysVar(schedule);

    let mut auto_concurrency = int_var(
        "tidb_auto_analyze_concurrency",
        3,
        vardef::ScopeGlobal,
        0,
        i32::MAX as u64,
    );
    auto_concurrency.Validation = Some(Arc::new(|_, normalized, original, _| {
        if !vardef::RunAutoAnalyze.Load() || !vardef::EnableAutoAnalyzePriorityQueue.Load() {
            Err(error(format!(
                "cannot set tidb_auto_analyze_concurrency to {original}: auto analyze and priority queue must both be enabled"
            )))
        } else {
            Ok(normalized.to_owned())
        }
    }));
    RegisterSysVar(auto_concurrency);

    for (name, default) in [
        ("tidb_snapshot", ""),
        ("tidb_enable_chunk_rpc", "ON"),
        ("tx_isolation_one_shot", "REPEATABLE-READ"),
        ("tidb_ddl_reorg_priority", "PRIORITY_LOW"),
        ("tidb_slow_query_file", "tidb-slow.log"),
        ("tidb_wait_split_region_finish", "ON"),
        ("tidb_wait_split_region_timeout", "300"),
        ("tidb_metric_query_step", "60"),
        ("tidb_metric_query_range_duration", "60m0s"),
        ("rand_seed1", "0"),
        ("rand_seed2", "0"),
        ("collation_database", "utf8mb4_bin"),
        ("collation_connection", "utf8mb4_bin"),
        ("character_set_database", "utf8mb4"),
        ("character_set_connection", "utf8mb4"),
        ("character_set_server", "utf8mb4"),
        ("tidb_opt_tiflash_concurrency_factor", "24"),
        ("tidb_opt_seek_factor", "20"),
    ] {
        let mut variable = GetSysVarDefinition(name)
            .unwrap_or_else(|| string_var(name, default, vardef::ScopeSession));
        variable.Scope = variable.Scope | vardef::ScopeSession;
        variable.Value = default.to_owned();
        variable.skipInit = true;
        if variable.SetSession.is_none() {
            variable.SetSession = Some(Arc::new(|_, _| Ok(())));
        }
        RegisterSysVar(variable);
    }
}

/// 注册全局内存仲裁器的全局/会话变量。
fn register_mem_arbitrator_vars() {
    RegisterSysVar(enum_var(
        vardef::TiDBMemArbitratorMode,
        vardef::DefTiDBMemArbitratorModeText,
        vardef::ScopeGlobal,
        &["disable", "standard", "priority"],
    ));

    RegisterSysVar(string_var(
        vardef::TiDBMemArbitratorSoftLimit,
        vardef::DefTiDBMemArbitratorSoftLimitText,
        vardef::ScopeGlobal,
    ));

    RegisterSysVar(enum_var(
        vardef::TiDBMemArbitratorWaitAverse,
        vardef::DefTiDBMemArbitratorWaitAverse,
        vardef::ScopeSession,
        &["0", "1", "nolimit"],
    ));

    let mut query_reserved = unsigned_var(
        vardef::TiDBMemArbitratorQueryReserved,
        0,
        vardef::ScopeSession,
        0,
        i64::MAX as u64,
    );
    query_reserved.IsHintUpdatableVerified = true;
    RegisterSysVar(query_reserved);
}

/// 按名称取已注册的系统变量定义副本。
fn GetSysVarDefinition(name: &str) -> Option<SysVar> {
    crate::GetSysVar(name).map(|value| (*value).clone())
}

/// 一次性注册全部内置系统变量（幂等）。
pub fn register_builtin_sysvars() {
    // 幂等入口：按序调用各 register_* 完成内置表填充。
    REGISTER.call_once(|| {
        register_basic_clamped_vars();
        register_planner_tuning_vars();
        register_sql_and_session_vars();
        register_getters_and_defaults();
        register_global_vars();
        register_mem_arbitrator_vars();
        register_compatibility_vars();
        register_noop_compatibility_vars();
    });
}

/// 校验并设置全局系统变量，写回 accessor。
pub fn set_global_system_var(
    vars: &mut SessionVars,
    name: &str,
    value: &str,
) -> Result<String, VariableError> {
    // 先 Validate 再 SetGlobal，并同步本地会话视图。
    let sys_var = crate::GetSysVar(name).ok_or_else(|| VariableError::unknown(name))?;
    let normalized = sys_var.Validate(vars, value, vardef::ScopeGlobal)?;
    sys_var.SetGlobalFromHook(&Context, vars, &normalized, false)?;
    vars.GlobalVarsAccessor
        .set_global_sys_var_only(&Context, name, &normalized, true)?;
    Ok(normalized)
}
