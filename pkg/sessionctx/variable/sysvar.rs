// Copyright 2015 PingCAP, Inc.
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

// 系统变量（sysvar）回调与会话侧状态片段。
//
// 提供执行器并发、TiFlash 计算派发策略、流水线 DML（pipelined DML，
// 将大批量写拆成可流水处理的阶段）资源配置等系统变量的构造与校验，
// 以及按运行时环境选择安装时默认值的逻辑。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::fmt;
use std::sync::Arc;

use crate::{config, kerneltype, tiflashcompute, vardef};

/// The session fields written by the callbacks implemented in this file.
///
/// The complete TiDB `SessionVars` is assembled by the package integration
/// task. Keeping the state here explicit makes the callback behavior usable in
/// the native task harness without replacing any external service.
///
/// 本文件回调写入的会话字段子集；完整 `SessionVars` 由包集成组装。
#[derive(Clone, Debug, PartialEq)]
pub struct SessionVars {
    /// 执行器并行度（未设置时用 `ConcurrencyUnset`）。
    pub ExecutorConcurrency: i32,
    /// TiFlash 计算层任务派发策略。
    pub TiFlashComputeDispatchPolicy: tiflashcompute::DispatchPolicy,
    /// 流水线 DML 的刷盘/解锁并发与写节流比例。
    pub PipelinedDMLConfig: PipelinedDMLConfig,
}

impl Default for SessionVars {
    fn default() -> Self {
        Self {
            ExecutorConcurrency: vardef::ConcurrencyUnset as i32,
            TiFlashComputeDispatchPolicy: tiflashcompute::DispatchPolicyInvalid,
            PipelinedDMLConfig: PipelinedDMLConfig::default(),
        }
    }
}

/// PipelinedDMLConfig mirrors the state-bearing Go struct from session.go.
///
/// 对应 Go `session.go` 中流水线 DML 资源配置结构。
#[derive(Clone, Debug, PartialEq)]
pub struct PipelinedDMLConfig {
    /// 流水线刷盘（flush）并发度。
    pub PipelinedFlushConcurrency: i32,
    /// 流水线解析锁（resolve lock）并发度。
    pub PipelinedResolveLockConcurrency: i32,
    /// 写节流比例，取值需满足 `0 <= ratio < 1`。
    pub PipelinedWriteThrottleRatio: f64,
}

impl Default for PipelinedDMLConfig {
    fn default() -> Self {
        Self {
            PipelinedFlushConcurrency: vardef::DefaultFlushConcurrency as i32,
            PipelinedResolveLockConcurrency: vardef::DefaultResolveConcurrency as i32,
            PipelinedWriteThrottleRatio: 0.0,
        }
    }
}

/// 系统变量设置失败时的错误（含变量名、尝试值与可读消息）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SysVarError {
    variable: String,
    value: String,
    message: String,
}

impl SysVarError {
    /// 构造“取值非法”错误。
    fn wrong_value(variable: &str, value: &str) -> Self {
        Self {
            variable: variable.to_owned(),
            value: value.to_owned(),
            message: format!("Variable '{variable}' can't be set to the value of '{value}'"),
        }
    }

    /// 构造依赖校验失败错误（保留下游错误文本）。
    fn dependency(variable: &str, value: &str, error: impl fmt::Display) -> Self {
        Self {
            variable: variable.to_owned(),
            value: value.to_owned(),
            message: error.to_string(),
        }
    }

    /// 返回出错的系统变量名。
    pub fn variable(&self) -> &str {
        &self.variable
    }

    /// 返回触发错误的原始取值。
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for SysVarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SysVarError {}

/// 并发类系统变量写入会话状态的回调类型。
pub type ConcurrencySetter = Arc<dyn Fn(&mut SessionVars, i32) + Send + Sync>;
/// 构造执行器并发系统变量时的可选配置闭包。
pub type ExecConcurrencySysVarOption = Box<dyn FnOnce(&mut SysVar) + Send>;

/// The fields of the executor-concurrency system variables constructed by
/// `newExecConcurrencySysVar` in the Go implementation.
///
/// 对应 Go `newExecConcurrencySysVar` 构造的执行器并发系统变量字段。
pub struct SysVar {
    /// 作用域标志（全局 / 会话）。
    pub Scope: vardef::ScopeFlag,
    /// 变量名。
    pub Name: String,
    /// 默认值字符串。
    pub Value: String,
    /// 值类型标志。
    pub Type: vardef::TypeFlag,
    /// 允许的最小值。
    pub MinValue: i64,
    /// 允许的最大值。
    pub MaxValue: u64,
    /// 是否允许自动取值（auto）。
    pub AllowAutoValue: bool,
    setter: ConcurrencySetter,
}

impl SysVar {
    /// 将会话侧并发值解析并写入；非法或非正整数回退为 `ConcurrencyUnset`。
    pub fn SetSession(&self, vars: &mut SessionVars, value: &str) -> Result<(), SysVarError> {
        let value = value
            .parse::<i32>()
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or(vardef::ConcurrencyUnset as i32);
        (self.setter)(vars, value);
        Ok(())
    }
}

/// 选项：是否允许自动取值。
pub fn withAllowAutoValue(allow: bool) -> ExecConcurrencySysVarOption {
    Box::new(move |sys_var| sys_var.AllowAutoValue = allow)
}

/// 选项：覆盖最小值。
pub fn withMinValue(min_value: i64) -> ExecConcurrencySysVarOption {
    Box::new(move |sys_var| sys_var.MinValue = min_value)
}

/// Creates a session/global variable for executor concurrency settings.
///
/// This preserves the Go defaults and applies options in call order.
///
/// 创建执行器并发相关的会话/全局系统变量，保留 Go 默认并按序应用选项。
pub fn newExecConcurrencySysVar(
    name: impl Into<String>,
    default_value: i32,
    setter: ConcurrencySetter,
    options: impl IntoIterator<Item = ExecConcurrencySysVarOption>,
) -> SysVar {
    let mut sys_var = SysVar {
        Scope: vardef::ScopeGlobal | vardef::ScopeSession,
        Name: name.into(),
        Value: default_value.to_string(),
        Type: vardef::TypeInt,
        MinValue: 1,
        MaxValue: vardef::MaxConfigurableConcurrency as u64,
        AllowAutoValue: true,
        setter,
    };
    for option in options {
        option(&mut sys_var);
    }
    sys_var
}

/// Runtime inputs read by `GlobalSystemVariableInitialValue` in Go.
/// The explicit form also lets callers validate both classic and next-gen
/// branches without mutating process-wide configuration.
///
/// Go `GlobalSystemVariableInitialValue` 读取的运行时环境；显式传入可避免改全局配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEnvironment {
    /// 存储引擎是否为 TiKV。
    pub store_is_tikv: bool,
    /// 是否处于测试构建/测试路径。
    pub in_test: bool,
    /// 是否 next-gen 内核部署。
    pub next_gen: bool,
    /// next-gen 下事务断言级别默认值。
    pub default_txn_assertion_level: String,
}

impl RuntimeEnvironment {
    /// 从全局配置与编译期开关采样当前运行时环境。
    pub fn current() -> Self {
        Self {
            store_is_tikv: config::get_global_config().store == config::StoreTypeTiKV.String(),
            in_test: cfg!(test),
            next_gen: kerneltype::IsNextGen(),
            default_txn_assertion_level: vardef::GetDefaultTxnAssertionLevel().to_owned(),
        }
    }
}

/// Gets the default value for a system variable, including the dynamically
/// selected defaults used for a new installation.
///
/// 取得系统变量安装时默认值（含按环境动态选择的分支）。
pub fn GlobalSystemVariableInitialValue(var_name: &str, var_value: &str) -> String {
    GlobalSystemVariableInitialValueWithRuntime(var_name, var_value, &RuntimeEnvironment::current())
}

/// 在给定运行时环境下解析系统变量初始默认值。
pub fn GlobalSystemVariableInitialValueWithRuntime(
    var_name: &str,
    var_value: &str,
    runtime: &RuntimeEnvironment,
) -> String {
    // 按变量名与运行时分支覆盖默认值；未命中则原样返回 var_value。
    match var_name {
        // TiKV 上默认开启异步提交（async commit）与一阶段提交（1PC）。
        vardef::TiDBEnableAsyncCommit | vardef::TiDBEnable1PC if runtime.store_is_tikv => {
            vardef::On.to_owned()
        }
        // 测试环境避免 OOM 直接取消，改为只记日志；并关闭自动 analyze。
        vardef::TiDBMemOOMAction if runtime.in_test => vardef::OOMActionLog.to_owned(),
        vardef::TiDBEnableAutoAnalyze if runtime.in_test => vardef::Off.to_owned(),
        vardef::TiDBRowFormatVersion => vardef::DefTiDBRowFormatV2.to_string(),
        vardef::TiDBTxnAssertionLevel if runtime.next_gen => {
            runtime.default_txn_assertion_level.clone()
        }
        vardef::TiDBTxnAssertionLevel => vardef::AssertionFastStr.to_owned(),
        vardef::TiDBEnableMutationChecker => vardef::On.to_owned(),
        // next-gen 关闭悲观事务公平锁；经典路径默认开启。
        vardef::TiDBPessimisticTransactionFairLocking if runtime.next_gen => vardef::Off.to_owned(),
        vardef::TiDBPessimisticTransactionFairLocking => vardef::On.to_owned(),
        _ => var_value.to_owned(),
    }
}

/// 解析并设置 TiFlash 计算派发策略；解析失败不改动会话状态。
pub fn setTiFlashComputeDispatchPolicy(
    vars: &mut SessionVars,
    value: &str,
) -> Result<(), SysVarError> {
    let policy = tiflashcompute::GetDispatchPolicyByStr(value).map_err(|error| {
        SysVarError::dependency(vardef::TiFlashComputeDispatchPolicy, value, error)
    })?;
    vars.TiFlashComputeDispatchPolicy = policy;
    Ok(())
}

/// Applies the standard, conservative, or custom pipelined-DML resource
/// policy. Custom values are validated into a temporary config so an error can
/// never leave a partially updated session, matching the Go implementation.
///
/// 应用标准/保守/自定义流水线 DML 资源策略；自定义先写入临时配置，避免部分更新。
pub fn setPipelinedDmlResourcePolicy(
    vars: &mut SessionVars,
    value: &str,
) -> Result<(), SysVarError> {
    let value = value.trim();
    let lower_value = value.to_ascii_lowercase();
    // 预设策略：standard 用默认；conservative 用更低并发。
    match lower_value.as_str() {
        vardef::StrategyStandard => {
            vars.PipelinedDMLConfig = PipelinedDMLConfig::default();
            return Ok(());
        }
        vardef::StrategyConservative => {
            vars.PipelinedDMLConfig = PipelinedDMLConfig {
                PipelinedFlushConcurrency: vardef::ConservativeFlushConcurrency as i32,
                PipelinedResolveLockConcurrency: vardef::ConservativeResolveConcurrency as i32,
                PipelinedWriteThrottleRatio: 0.0,
            };
            return Ok(());
        }
        _ => {}
    }

    // 自定义策略必须以 `custom{...}` 形式给出花括号参数列表。
    let Some(remaining) = lower_value.strip_prefix(vardef::StrategyCustom) else {
        return Err(SysVarError::wrong_value(
            vardef::TiDBPipelinedDmlResourcePolicy,
            value,
        ));
    };
    let remaining = remaining.trim();
    if remaining.len() < 2 || !remaining.starts_with('{') || !remaining.ends_with('}') {
        return Err(SysVarError::wrong_value(
            vardef::TiDBPipelinedDmlResourcePolicy,
            value,
        ));
    }
    let content = remaining[1..remaining.len() - 1].trim();
    if content.is_empty() {
        return Err(SysVarError::wrong_value(
            vardef::TiDBPipelinedDmlResourcePolicy,
            value,
        ));
    }

    // 先填临时配置，全部参数合法后再一次性写回会话。
    let mut new_config = PipelinedDMLConfig::default();
    for raw_parameter in content.split(',') {
        let parameter = raw_parameter.trim();
        if parameter.is_empty() {
            return Err(SysVarError::wrong_value(
                vardef::TiDBPipelinedDmlResourcePolicy,
                value,
            ));
        }
        let parts: Vec<_> = parameter
            .split(|character| character == '=' || character == ':')
            // Go uses strings.FieldsFunc, which omits empty fields produced by
            // repeated separators or separators at either edge.
            .filter(|part| !part.is_empty())
            .collect();
        if parts.len() != 2 {
            return Err(SysVarError::wrong_value(
                vardef::TiDBPipelinedDmlResourcePolicy,
                value,
            ));
        }
        let key = parts[0].trim();
        let raw_value = parts[1].trim();
        match key {
            "concurrency" => {
                new_config.PipelinedFlushConcurrency =
                    parse_pipelined_concurrency(raw_value, value)?;
            }
            "resolve_concurrency" => {
                new_config.PipelinedResolveLockConcurrency =
                    parse_pipelined_concurrency(raw_value, value)?;
            }
            "write_throttle_ratio" => {
                let ratio = raw_value.parse::<f64>().map_err(|_| {
                    SysVarError::wrong_value(vardef::TiDBPipelinedDmlResourcePolicy, value)
                })?;
                // 节流比例必须落在 [0, 1)。
                if ratio < 0.0 || ratio >= 1.0 {
                    return Err(SysVarError::wrong_value(
                        vardef::TiDBPipelinedDmlResourcePolicy,
                        value,
                    ));
                }
                new_config.PipelinedWriteThrottleRatio = ratio;
            }
            _ => {
                return Err(SysVarError::wrong_value(
                    vardef::TiDBPipelinedDmlResourcePolicy,
                    value,
                ));
            }
        }
    }

    vars.PipelinedDMLConfig = new_config;
    Ok(())
}

/// 解析流水线并发整数值，并校验落在允许区间内。
fn parse_pipelined_concurrency(raw_value: &str, original_value: &str) -> Result<i32, SysVarError> {
    let concurrency = raw_value.parse::<i64>().map_err(|_| {
        SysVarError::wrong_value(vardef::TiDBPipelinedDmlResourcePolicy, original_value)
    })?;
    if !(vardef::MinPipelinedDMLConcurrency..=vardef::MaxPipelinedDMLConcurrency)
        .contains(&concurrency)
    {
        return Err(SysVarError::wrong_value(
            vardef::TiDBPipelinedDmlResourcePolicy,
            original_value,
        ));
    }
    Ok(concurrency as i32)
}
