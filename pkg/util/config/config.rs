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

// Plan Replayer 配置加载：从 TOML 读取系统变量并写入会话。
//
// Plan Replayer 用于复现查询计划相关环境；本模块仅服务该场景与测试，
// 按变量名校验、忽略名单过滤后设置到 `SessionVars`（会话级系统变量集合）。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read;
use std::sync::LazyLock;

use logutil::log::{BgLogger, LogField, LogLevel};

/// Plan Replayer 加载时需忽略的系统变量名集合。
///
/// 含 `innodb_lock_wait_timeout` 等不宜在回放场景强行改写的项。
static ignoredSystemVariablesForPlanReplayerLoad: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from([variable::vardef::InnodbLockWaitTimeout]));

/// `sessionctx.Context` 的窄化子集，仅暴露可变 `SessionVars`。
///
/// 避免在本包复制完整会话上下文接口。
/// The subset of `sessionctx.Context` used by the Go implementation.
///
/// Keeping this as a narrow trait lets real session contexts expose their
/// mutable `SessionVars` without duplicating the much larger session context
/// interface in this package.
pub trait PlanReplayerSessionContext {
    /// 返回可变的会话系统变量容器。
    fn GetSessionVars(&mut self) -> &mut variable::SessionVars;
}

impl PlanReplayerSessionContext for variable::SessionVars {
    fn GetSessionVars(&mut self) -> &mut variable::SessionVars {
        self
    }
}

/// 加载 Plan Replayer 配置时的错误：读入失败或 TOML 解码失败。
#[derive(Debug)]
pub enum ConfigLoadError {
    /// 底层 `Read` 失败。
    Io(std::io::Error),
    /// TOML 反序列化失败。
    Toml(toml::de::Error),
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => err.fmt(f),
            Self::Toml(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Toml(err) => Some(err),
        }
    }
}

/// 通过后台日志器输出 Warn 级消息，可选附带 error 字段。
fn warn(message: String, error: Option<String>) {
    let fields = error
        .into_iter()
        .map(|err| LogField::String("error".to_owned(), err));
    BgLogger().log(LogLevel::Warn, message, fields);
}

/// 从 TOML 读取器加载系统变量到会话，仅用于 Plan Replayer 与测试。
///
/// 返回未能成功设置的变量名列表；忽略名单中的变量只打日志不写入。
// LoadConfigForPlanReplayerLoad loads system variables from a toml reader. it is only for plan replayer and test.
pub fn LoadConfigForPlanReplayerLoad(
    ctx: &mut impl PlanReplayerSessionContext,
    mut v: impl Read,
) -> Result<Vec<String>, ConfigLoadError> {
    // 先读入全部 TOML 文本再解码为 name→value 映射。
    let mut input = String::new();
    v.read_to_string(&mut input).map_err(ConfigLoadError::Io)?;
    let varMap: HashMap<String, String> = toml::from_str(&input).map_err(ConfigLoadError::Toml)?;
    let mut unLoadVars: Vec<String> = Vec::new();
    let vars = ctx.GetSessionVars();

    // 逐项处理：忽略名单 → 未知变量 → 校验失败 → 设置失败。
    for (name, value) in varMap {
        // 忽略名单内的变量不写入会话，仅记录 warn。
        if ignoredSystemVariablesForPlanReplayerLoad.contains(name.as_str()) {
            warn(format!("ignore set variable {name}:{value}"), None);
            continue;
        }

        // 未注册的系统变量记入未加载列表并跳过。
        let sysVar = variable::GetSysVar(&name);
        if sysVar.is_none() {
            unLoadVars.push(name.clone());
            warn(format!("skip set variable {name}:{value}"), None);
            continue;
        }

        let sysVar = sysVar.expect("checked above");
        // Validate 按会话作用域规范化取值；失败则跳过该变量。
        let sVal = match sysVar.Validate(vars, &value, variable::vardef::ScopeSession) {
            Ok(sVal) => sVal,
            Err(err) => {
                unLoadVars.push(name.clone());
                warn(
                    format!("skip variable {name}:{value}"),
                    Some(err.to_string()),
                );
                continue;
            }
        };

        // SetSystemVar 写入会话；钩子拒绝时同样记入未加载列表。
        if let Err(err) = vars.SetSystemVar(&name, &sVal) {
            unLoadVars.push(name.clone());
            warn(
                format!("skip set variable {name}:{value}"),
                Some(err.to_string()),
            );
            continue;
        }
    }

    Ok(unLoadVars)
}
