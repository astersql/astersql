// Copyright 2021 PingCAP, Inc.
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

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// 会话系统变量核心类型与注册表。
//
// 定义 `SessionVars`、`SysVar`、校验/读写钩子以及全局系统变量注册表。
// 系统变量（System Variable）控制会话与实例行为；作用域可为 SESSION / GLOBAL / INSTANCE。
// 校验路径负责类型转换、范围裁剪与错误/警告生成，对应 MySQL/TiDB 的 SET 语义。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};

use chrono::{FixedOffset, NaiveTime};

use crate::vardef;

/// MySQL 协议 LONGLONG 字段类型码。
pub const MYSQL_TYPE_LONGLONG: u8 = 8;
/// MySQL 协议 VAR_STRING 字段类型码。
pub const MYSQL_TYPE_VAR_STRING: u8 = 253;
/// 无符号标志位。
pub const UNSIGNED_FLAG: u32 = 32;
/// 二进制标志位。
pub const BINARY_FLAG: u32 = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 系统变量的原生取值载体，用于协议回包。
pub enum Datum {
    Int(i64),
    Uint(u64),
    String(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 系统变量错误分类，对应 TiDB/MySQL 错误语义。
pub enum VariableErrorKind {
    IncorrectScope,
    GlobalVariable,
    LocalVariable,
    UnknownSystemVariable,
    WrongType,
    WrongValue,
    TruncatedWrongValue,
    UnsupportedIsolationLevel,
    InvalidValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带分类与消息的系统变量错误。
pub struct VariableError {
    kind: VariableErrorKind,
    message: String,
}

impl VariableError {
    /// 构造错误。
    pub fn new(kind: VariableErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 返回错误分类。
    pub fn kind(&self) -> VariableErrorKind {
        self.kind
    }

    /// 未知系统变量错误。
    pub fn unknown(name: &str) -> Self {
        Self::new(
            VariableErrorKind::UnknownSystemVariable,
            format!("Unknown system variable '{name}'"),
        )
    }

    /// 类型不正确错误。
    pub fn wrong_type(name: &str) -> Self {
        Self::new(
            VariableErrorKind::WrongType,
            format!("Incorrect argument type to variable '{name}'"),
        )
    }

    /// 取值非法错误。
    pub fn wrong_value(name: &str, value: &str) -> Self {
        Self::new(
            VariableErrorKind::WrongValue,
            format!("Variable '{name}' can't be set to the value of '{value}'"),
        )
    }

    /// 取值被截断/裁剪时的警告型错误。
    pub fn truncated(name: &str, value: &str) -> Self {
        Self::new(
            VariableErrorKind::TruncatedWrongValue,
            format!("Truncated incorrect {name} value: '{value}'"),
        )
    }
}

impl fmt::Display for VariableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for VariableError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 轻量执行上下文占位（迁移基线）。
pub struct Context;

#[derive(Clone, Debug, Default)]
/// 语句级上下文：警告列表与语句相关标志。
pub struct StatementContext {
    warnings: Vec<VariableError>,
    pub InSelectStmt: bool,
    pub PrevLastInsertID: u64,
}

impl StatementContext {
    /// 追加一条语句警告。
    pub fn append_warning(&mut self, warning: VariableError) {
        self.warnings.push(warning);
    }

    /// 只读访问当前警告列表。
    pub fn warnings(&self) -> &[VariableError] {
        &self.warnings
    }

    /// 返回当前语句累计的警告数量，对应 Go `StatementContext.WarningCount`。
    pub fn WarningCount(&self) -> usize {
        self.warnings.len()
    }

    /// 取出并清空警告（用于宽松校验时暂存）。
    fn take_warnings(&mut self) -> Vec<VariableError> {
        std::mem::take(&mut self.warnings)
    }

    /// 恢复警告列表。
    fn set_warnings(&mut self, warnings: Vec<VariableError>) {
        self.warnings = warnings;
    }
}

/// 全局/实例系统变量与 mysql.tidb 表值的访问器。
pub trait GlobalVarAccessor: Send + Sync {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError>;

    fn set_global_sys_var(
        &mut self,
        ctx: &Context,
        name: &str,
        value: &str,
    ) -> Result<(), VariableError> {
        self.set_global_sys_var_only(ctx, name, value, true)
    }

    fn set_instance_sys_var(
        &mut self,
        ctx: &Context,
        name: &str,
        value: &str,
    ) -> Result<(), VariableError> {
        self.set_global_sys_var_only(ctx, name, value, true)
    }

    fn set_global_sys_var_only(
        &mut self,
        ctx: &Context,
        name: &str,
        value: &str,
        update_local: bool,
    ) -> Result<(), VariableError>;

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError>;

    fn set_tidb_table_value(
        &mut self,
        name: &str,
        value: &str,
        comment: &str,
    ) -> Result<(), VariableError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 连接端点信息（客户端/服务端 IP 与端口）。
pub struct ConnectionInfo {
    pub ClientIP: String,
    pub ClientPort: String,
    pub ServerIP: String,
    pub ServerPort: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 下发到 KV 事务的会话级退避参数。
pub struct SessionKVVars {
    pub BackoffLockFast: i32,
    pub BackOffWeight: i32,
}

impl Default for SessionKVVars {
    fn default() -> Self {
        Self {
            BackoffLockFast: kv::DefBackoffLockFast,
            BackOffWeight: kv::DefBackOffWeight,
        }
    }
}

/// 会话变量集合：系统变量映射、语句上下文及大量会话级调优字段。
pub struct SessionVars {
    systems: HashMap<String, String>,
    /// 当前会话累计的警告数，对应 Go `SessionVars.SysWarningCount`。
    pub SysWarningCount: i32,
    /// Nonzero values identify CDC writes and bypass BDR DDL restrictions.
    pub CDCWriteSource: u64,
    pub DMLBatchSize: i64,
    pub KVVars: SessionKVVars,
    pub StmtCtx: StatementContext,
    pub GlobalVarsAccessor: Box<dyn GlobalVarAccessor>,
    pub NoopFuncsMode: i32,
    pub SnapshotTS: u64,
    pub SnapshotInfoschema: Option<String>,
    pub TxnReadTS: u64,
    pub ReadStaleness: i64,
    pub memory_total: u64,
    pub memory_total_available: bool,
    pub SelectLimit: u64,
    pub MaxExecutionTime: u64,
    pub DefaultStrMatchSelectivity: f64,
    pub OptPartialOrderedIndexForTopN: String,
    /// 是否允许 schema `*` 的通用 SQL binding。
    pub EnableFuzzyBinding: bool,
    pub MaxKeysRead: u64,
    pub TiFlashMaxBytesBeforeExternalJoin: i64,
    pub TiFlashMaxBytesBeforeExternalGroupBy: i64,
    pub TiFlashMaxBytesBeforeExternalSort: i64,
    pub TiFlashMaxQueryMemoryPerNode: i64,
    pub TiFlashQuerySpillRatio: f64,
    pub AllowMPPExecution: bool,
    pub EnforceMPPExecution: bool,
    pub MultiStatementMode: i32,
    pub EnableWindowFunction: bool,
    pub UseHashJoinV2: bool,
    pub TxnStartTS: u64,
    pub LastTxnInfo: String,
    pub LastQueryInfo: String,
    pub PrevFoundInPlanCache: bool,
    pub FoundInBinding: bool,
    pub PrevFoundInBinding: bool,
    pub ExplainNonEvaledSubQuery: bool,
    pub ConnectionInfo: Option<ConnectionInfo>,
    pub User: Option<parser_auth::auth::UserIdentity>,
    pub ActiveRoles: Vec<parser_auth::auth::RoleIdentity>,
    location: FixedOffset,
}

impl SessionVars {
    /// 创建会话变量，并确保内置系统变量已注册。
    pub fn new(accessor: Box<dyn GlobalVarAccessor>) -> Self {
        crate::register_builtin_sysvars();
        Self {
            systems: HashMap::new(),
            SysWarningCount: 0,
            CDCWriteSource: 0,
            DMLBatchSize: vardef::DefDMLBatchSize,
            KVVars: SessionKVVars::default(),
            StmtCtx: StatementContext::default(),
            GlobalVarsAccessor: accessor,
            NoopFuncsMode: 0,
            SnapshotTS: 0,
            SnapshotInfoschema: None,
            TxnReadTS: 0,
            ReadStaleness: 0,
            memory_total: 0,
            memory_total_available: true,
            SelectLimit: u64::MAX,
            MaxExecutionTime: 0,
            DefaultStrMatchSelectivity: vardef::DefTiDBDefaultStrMatchSelectivity as f64,
            OptPartialOrderedIndexForTopN: vardef::DefTiDBOptPartialOrderedIndexForTopN.to_owned(),
            EnableFuzzyBinding: false,
            MaxKeysRead: 0,
            TiFlashMaxBytesBeforeExternalJoin: -1,
            TiFlashMaxBytesBeforeExternalGroupBy: -1,
            TiFlashMaxBytesBeforeExternalSort: -1,
            TiFlashMaxQueryMemoryPerNode: -1,
            TiFlashQuerySpillRatio: 0.0,
            AllowMPPExecution: vardef::DefTiDBAllowMPPExecution,
            EnforceMPPExecution: vardef::DefTiDBEnforceMPPExecution,
            MultiStatementMode: 0,
            EnableWindowFunction: true,
            UseHashJoinV2: true,
            TxnStartTS: 0,
            LastTxnInfo: String::new(),
            LastQueryInfo: "null".to_owned(),
            PrevFoundInPlanCache: false,
            FoundInBinding: false,
            PrevFoundInBinding: false,
            ExplainNonEvaledSubQuery: false,
            ConnectionInfo: None,
            User: None,
            ActiveRoles: Vec::new(),
            location: FixedOffset::east_opt(0).expect("UTC offset is valid"),
        }
    }

    /// 读取会话侧已加载的系统变量字符串值。
    pub fn system(&self, name: &str) -> Option<&str> {
        self.systems.get(name).map(String::as_str)
    }

    /// 写入会话侧系统变量映射（不经过完整 SET 钩子时的底层存储）。
    pub fn set_system(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.systems.insert(name.into(), value.into());
    }

    /// 通过访问器读取全局系统变量。
    pub fn global(&self, name: &str) -> Option<String> {
        self.GlobalVarsAccessor.get_global_sys_var(name).ok()
    }

    /// 会话时区（FixedOffset）。
    pub fn location(&self) -> FixedOffset {
        self.location
    }

    /// 设置会话时区。
    pub fn set_location(&mut self, location: FixedOffset) {
        self.location = location;
    }

    /// SELECT 语句下返回 MaxKeysRead；其它语句返回 0。
    pub fn GetMaxKeysRead(&self) -> u64 {
        if self.StmtCtx.InSelectStmt {
            self.MaxKeysRead
        } else {
            0
        }
    }

    /// 按作用域优先读取会话值，否则回落到全局/钩子。
    pub fn GetSessionOrGlobalSystemVar(
        &mut self,
        ctx: &Context,
        name: &str,
    ) -> Result<String, VariableError> {
        let sys_var = GetSysVar(name).ok_or_else(|| VariableError::unknown(name))?;
        if sys_var.HasNoneScope() || sys_var.HasInstanceScope() {
            return sys_var.GetGlobalFromHook(ctx, self);
        }
        if sys_var.HasSessionScope() {
            if let Ok(value) = sys_var.GetSessionFromHook(self) {
                return Ok(value);
            }
        }
        sys_var.GetGlobalFromHook(ctx, self)
    }

    /// 严格校验后设置会话系统变量。
    pub fn SetSystemVar(&mut self, name: &str, value: &str) -> Result<(), VariableError> {
        let sys_var = GetSysVar(name).ok_or_else(|| VariableError::unknown(name))?;
        let normalized = sys_var.Validate(self, value, vardef::ScopeSession)?;
        sys_var.SetSessionFromHook(self, &normalized)
    }

    /// 宽松校验后设置会话系统变量（失败时保留原串）。
    pub fn SetSystemVarWithRelaxedValidation(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), VariableError> {
        let sys_var = GetSysVar(name).ok_or_else(|| VariableError::unknown(name))?;
        let normalized = sys_var.ValidateWithRelaxedValidation(self, value, vardef::ScopeSession);
        sys_var.SetSessionFromHook(self, &normalized)
    }
}

/// 自定义校验钩子：归一化值并可能返回错误。
pub type ValidationHook = Arc<
    dyn Fn(&mut SessionVars, &str, &str, vardef::ScopeFlag) -> Result<String, VariableError>
        + Send
        + Sync,
>;
/// 设置会话值的副作用钩子。
pub type SetSessionHook =
    Arc<dyn Fn(&mut SessionVars, &str) -> Result<(), VariableError> + Send + Sync>;
/// 设置全局值的副作用钩子。
pub type SetGlobalHook =
    Arc<dyn Fn(&Context, &mut SessionVars, &str) -> Result<(), VariableError> + Send + Sync>;
/// 读取会话值的钩子。
pub type GetSessionHook =
    Arc<dyn Fn(&mut SessionVars) -> Result<String, VariableError> + Send + Sync>;
/// 读取全局值的钩子。
pub type GetGlobalHook =
    Arc<dyn Fn(&Context, &mut SessionVars) -> Result<String, VariableError> + Send + Sync>;
/// 读取会话状态值（含是否存在标志）的钩子。
pub type GetStateValueHook =
    Arc<dyn Fn(&mut SessionVars) -> Result<(String, bool), VariableError> + Send + Sync>;
/// 动态权限要求计算钩子。
pub type PrivilegeHook = Arc<dyn Fn(bool, bool) -> Vec<String> + Send + Sync>;

#[derive(Clone)]
/// 系统变量元数据：作用域、类型、范围、枚举值及各类钩子。
pub struct SysVar {
    pub Scope: vardef::ScopeFlag,
    pub Name: String,
    pub Value: String,
    pub Type: vardef::TypeFlag,
    pub MinValue: i64,
    pub MaxValue: u64,
    pub AutoConvertNegativeBool: bool,
    pub ReadOnly: bool,
    pub PossibleValues: Vec<String>,
    pub AllowEmpty: bool,
    pub AllowEmptyAll: bool,
    pub AllowAutoValue: bool,
    pub Validation: Option<ValidationHook>,
    pub SetSession: Option<SetSessionHook>,
    pub SetGlobal: Option<SetGlobalHook>,
    pub IsHintUpdatableVerified: bool,
    pub Hidden: bool,
    pub InternalSessionVariable: bool,
    pub Aliases: Vec<String>,
    pub GetSession: Option<GetSessionHook>,
    pub GetGlobal: Option<GetGlobalHook>,
    pub GetStateValue: Option<GetStateValueHook>,
    pub Depended: bool,
    pub skipInit: bool,
    pub IsNoop: bool,
    pub IsInitedFromConfig: bool,
    pub GlobalConfigName: String,
    pub RequireDynamicPrivileges: Option<PrivilegeHook>,
}

impl Default for SysVar {
    fn default() -> Self {
        Self {
            Scope: vardef::ScopeNone,
            Name: String::new(),
            Value: String::new(),
            Type: vardef::TypeStr,
            MinValue: 0,
            MaxValue: u64::MAX,
            AutoConvertNegativeBool: false,
            ReadOnly: false,
            PossibleValues: Vec::new(),
            AllowEmpty: false,
            AllowEmptyAll: false,
            AllowAutoValue: false,
            Validation: None,
            SetSession: None,
            SetGlobal: None,
            IsHintUpdatableVerified: false,
            Hidden: false,
            InternalSessionVariable: false,
            Aliases: Vec::new(),
            GetSession: None,
            GetGlobal: None,
            GetStateValue: None,
            Depended: false,
            skipInit: false,
            IsNoop: false,
            IsInitedFromConfig: false,
            GlobalConfigName: String::new(),
            RequireDynamicPrivileges: None,
        }
    }
}

/// 判断作用域标志字符串是否包含指定作用域名。
fn has_scope(scope: vardef::ScopeFlag, expected: &str) -> bool {
    scope.String().split(',').any(|value| value == expected)
}

impl SysVar {
    /// 经 GetGlobal 钩子或访问器读取全局值，并用宽松校验归一化。
    pub fn GetGlobalFromHook(
        &self,
        ctx: &Context,
        vars: &mut SessionVars,
    ) -> Result<String, VariableError> {
        if let Some(getter) = &self.GetGlobal {
            let value = getter(ctx, vars)?;
            return Ok(self.ValidateWithRelaxedValidation(vars, &value, vardef::ScopeGlobal));
        }
        if self.HasNoneScope() {
            return Ok(self.Value.clone());
        }
        vars.GlobalVarsAccessor.get_global_sys_var(&self.Name)
    }

    /// 经 GetSession 钩子或会话映射读取会话值。
    pub fn GetSessionFromHook(&self, vars: &mut SessionVars) -> Result<String, VariableError> {
        if self.HasNoneScope() {
            return Ok(self.Value.clone());
        }
        if let Some(getter) = &self.GetSession {
            let value = getter(vars)?;
            return Ok(self.ValidateWithRelaxedValidation(vars, &value, vardef::ScopeSession));
        }
        vars.systems.get(&self.Name).cloned().ok_or_else(|| {
            VariableError::new(VariableErrorKind::InvalidValue, "sysvar has not yet loaded")
        })
    }

    /// 调用 SetSession 钩子并写入会话映射，同时同步别名变量。
    pub fn SetSessionFromHook(
        &self,
        vars: &mut SessionVars,
        value: &str,
    ) -> Result<(), VariableError> {
        if let Some(setter) = &self.SetSession {
            setter(vars, value)?;
        }
        vars.set_system(self.Name.clone(), value);
        for alias_name in &self.Aliases {
            if let Some(alias) = GetSysVar(alias_name) {
                if let Some(setter) = &alias.SetSession {
                    setter(vars, value)?;
                }
                vars.set_system(alias.Name.clone(), value);
            }
        }
        Ok(())
    }

    /// 调用 SetGlobal 钩子，或将值写入别名的全局变量。
    pub fn SetGlobalFromHook(
        &self,
        ctx: &Context,
        vars: &mut SessionVars,
        value: &str,
        skip_aliases: bool,
    ) -> Result<(), VariableError> {
        if let Some(setter) = &self.SetGlobal {
            return setter(ctx, vars, value);
        }
        if !skip_aliases {
            for alias in &self.Aliases {
                vars.GlobalVarsAccessor
                    .set_global_sys_var_only(ctx, alias, value, true)?;
            }
        }
        Ok(())
    }

    /// 是否无作用域（只读常量类）。
    pub fn HasNoneScope(&self) -> bool {
        self.Scope == vardef::ScopeNone
    }

    /// 是否具有 SESSION 作用域。
    pub fn HasSessionScope(&self) -> bool {
        has_scope(self.Scope, "SESSION")
    }

    /// 是否具有 GLOBAL 作用域。
    pub fn HasGlobalScope(&self) -> bool {
        has_scope(self.Scope, "GLOBAL")
    }

    /// 是否具有 INSTANCE 作用域。
    pub fn HasInstanceScope(&self) -> bool {
        has_scope(self.Scope, "INSTANCE")
    }

    /// 严格校验：检查作用域、按类型归一化，再走自定义 Validation 钩子。
    pub fn Validate(
        &self,
        vars: &mut SessionVars,
        value: &str,
        scope: vardef::ScopeFlag,
    ) -> Result<String, VariableError> {
        self.validateScope(scope)?;
        let normalized = self.ValidateFromType(vars, value, scope)?;
        if let Some(validation) = &self.Validation {
            return validation(vars, &normalized, value, scope);
        }
        Ok(normalized)
    }

    /// 按 Type 字段做类型级校验与裁剪。
    pub fn ValidateFromType(
        &self,
        vars: &mut SessionVars,
        value: &str,
        scope: vardef::ScopeFlag,
    ) -> Result<String, VariableError> {
        if value.is_empty()
            && ((self.AllowEmpty && scope == vardef::ScopeSession) || self.AllowEmptyAll)
        {
            return Ok(value.to_owned());
        }
        match self.Type {
            vardef::TypeUnsigned => self.checkUInt64SystemVar(value, vars),
            vardef::TypeInt => self.checkInt64SystemVar(value, vars),
            vardef::TypeBool => self.checkBoolSystemVar(value),
            vardef::TypeFloat => self.checkFloatSystemVar(value, vars),
            vardef::TypeEnum => self.checkEnumSystemVar(value),
            vardef::TypeTime => self.checkTimeSystemVar(value, vars),
            vardef::TypeDuration => self.checkDurationSystemVar(value, vars),
            _ => Ok(value.to_owned()),
        }
    }

    /// 校验目标作用域是否允许写入该变量。
    fn validateScope(&self, scope: vardef::ScopeFlag) -> Result<(), VariableError> {
        if self.ReadOnly || self.HasNoneScope() {
            return Err(VariableError::new(
                VariableErrorKind::IncorrectScope,
                format!("Variable '{}' is read only", self.Name),
            ));
        }
        if scope == vardef::ScopeGlobal && !(self.HasGlobalScope() || self.HasInstanceScope()) {
            return Err(VariableError::new(
                VariableErrorKind::LocalVariable,
                format!("Variable '{}' is a SESSION variable", self.Name),
            ));
        }
        if scope == vardef::ScopeInstance && !self.HasInstanceScope() {
            return Err(VariableError::new(
                VariableErrorKind::LocalVariable,
                format!("Variable '{}' is not an INSTANCE variable", self.Name),
            ));
        }
        if scope == vardef::ScopeSession {
            if !self.HasSessionScope() {
                return Err(VariableError::new(
                    VariableErrorKind::GlobalVariable,
                    format!("Variable '{}' is a GLOBAL variable", self.Name),
                ));
            }
            if self.InternalSessionVariable {
                return Err(VariableError::unknown(&self.Name));
            }
        }
        Ok(())
    }

    /// 宽松校验：类型/自定义校验失败时回退为原值，并恢复原警告列表。
    pub fn ValidateWithRelaxedValidation(
        &self,
        vars: &mut SessionVars,
        value: &str,
        scope: vardef::ScopeFlag,
    ) -> String {
        let warnings = vars.StmtCtx.take_warnings();
        let normalized = match self.ValidateFromType(vars, value, scope) {
            Ok(normalized) => {
                if let Some(validation) = &self.Validation {
                    validation(vars, &normalized, value, scope).unwrap_or(normalized)
                } else {
                    normalized
                }
            }
            // Go's TIME validator returns an empty normalized value together with
            // its parse error; the other type validators return the input value.
            Err(_) if self.Type == vardef::TypeTime => String::new(),
            Err(_) => value.to_owned(),
        };
        vars.StmtCtx.set_warnings(warnings);
        normalized
    }

    /// 校验 TIME 类型：解析时刻并附带会话时区偏移。
    fn checkTimeSystemVar(&self, value: &str, vars: &SessionVars) -> Result<String, VariableError> {
        if value.len() <= vardef::LocalDayTimeFormat.len() {
            let time = NaiveTime::parse_from_str(value, "%H:%M").map_err(|error| {
                VariableError::new(VariableErrorKind::WrongType, error.to_string())
            })?;
            return Ok(format!(
                "{} {}",
                time.format("%H:%M"),
                chrono::Utc::now()
                    .with_timezone(&vars.location())
                    .format("%z")
            ));
        }
        let time = chrono::DateTime::parse_from_str(
            &format!("2000-01-01 {value}"),
            "%Y-%m-%d %H:%M %z",
        )
        .map_err(|error| VariableError::new(VariableErrorKind::WrongType, error.to_string()))?;
        Ok(time.format("%H:%M %z").to_string())
    }

    /// 校验 Duration（Go 风格时长串），越界则警告并裁剪到 Min/Max。
    fn checkDurationSystemVar(
        &self,
        value: &str,
        vars: &mut SessionVars,
    ) -> Result<String, VariableError> {
        let duration =
            parse_go_duration(value).ok_or_else(|| VariableError::wrong_type(&self.Name))?;
        if duration < self.MinValue as i128 {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(format_go_duration(self.MinValue as i128));
        }
        if duration > self.MaxValue as i128 {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(format_go_duration(self.MaxValue as i128));
        }
        Ok(format_go_duration(duration))
    }

    /// 校验无符号整数：负值警告裁剪到 Min；可选允许 -1 作为 AUTO。
    fn checkUInt64SystemVar(
        &self,
        value: &str,
        vars: &mut SessionVars,
    ) -> Result<String, VariableError> {
        if self.AllowAutoValue && value == "-1" {
            return Ok(value.to_owned());
        }
        if value.is_empty() {
            return Err(VariableError::wrong_type(&self.Name));
        }
        if value.starts_with('-') {
            value
                .parse::<i64>()
                .map_err(|_| VariableError::wrong_type(&self.Name))?;
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MinValue.to_string());
        }
        let parsed = value
            .parse::<u64>()
            .map_err(|_| VariableError::wrong_type(&self.Name))?;
        if parsed < self.MinValue as u64 {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MinValue.to_string());
        }
        if parsed > self.MaxValue {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MaxValue.to_string());
        }
        Ok(value.to_owned())
    }

    /// 校验有符号整数并按 Min/Max 裁剪。
    fn checkInt64SystemVar(
        &self,
        value: &str,
        vars: &mut SessionVars,
    ) -> Result<String, VariableError> {
        if self.AllowAutoValue && value == "-1" {
            return Ok(value.to_owned());
        }
        let parsed = value
            .parse::<i64>()
            .map_err(|_| VariableError::wrong_type(&self.Name))?;
        if parsed < self.MinValue {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MinValue.to_string());
        }
        if parsed > self.MaxValue as i64 {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MaxValue.to_string());
        }
        Ok(value.to_owned())
    }

    /// 校验枚举：匹配名（忽略大小写）或下标。
    fn checkEnumSystemVar(&self, value: &str) -> Result<String, VariableError> {
        for (index, possible) in self.PossibleValues.iter().enumerate() {
            if possible.eq_ignore_ascii_case(value) || index.to_string() == value {
                return Ok(possible.clone());
            }
        }
        Err(VariableError::wrong_value(&self.Name, value))
    }

    /// 校验浮点数并按 Min/Max 裁剪。
    fn checkFloatSystemVar(
        &self,
        value: &str,
        vars: &mut SessionVars,
    ) -> Result<String, VariableError> {
        if value.is_empty() {
            return Err(VariableError::wrong_type(&self.Name));
        }
        let parsed = value
            .parse::<f64>()
            .map_err(|_| VariableError::wrong_type(&self.Name))?;
        if parsed < self.MinValue as f64 {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MinValue.to_string());
        }
        if parsed > self.MaxValue as f64 {
            vars.StmtCtx
                .append_warning(VariableError::truncated(&self.Name, value));
            return Ok(self.MaxValue.to_string());
        }
        Ok(value.to_owned())
    }

    /// 校验布尔：ON/OFF 或 0/1；可选将负整数视为 ON。
    fn checkBoolSystemVar(&self, value: &str) -> Result<String, VariableError> {
        if value.eq_ignore_ascii_case(vardef::On) {
            return Ok(vardef::On.to_owned());
        }
        if value.eq_ignore_ascii_case(vardef::Off) {
            return Ok(vardef::Off.to_owned());
        }
        if let Ok(parsed) = value.parse::<i64>() {
            if parsed == 0 {
                return Ok(vardef::Off.to_owned());
            }
            if parsed == 1 || (self.AutoConvertNegativeBool && parsed < 0) {
                return Ok(vardef::On.to_owned());
            }
        }
        Err(VariableError::wrong_value(&self.Name, value))
    }

    /// 将字符串值转为协议原生 Datum 与类型/标志。
    pub fn GetNativeValType(&self, value: &str) -> (Datum, u8, u32) {
        match self.Type {
            vardef::TypeUnsigned => (
                Datum::Uint(value.parse().unwrap_or(0)),
                MYSQL_TYPE_LONGLONG,
                UNSIGNED_FLAG | BINARY_FLAG,
            ),
            vardef::TypeBool => (
                Datum::Int(i64::from(crate::TiDBOptOn(value))),
                MYSQL_TYPE_LONGLONG,
                BINARY_FLAG,
            ),
            _ => (Datum::String(value.to_owned()), MYSQL_TYPE_VAR_STRING, 0),
        }
    }

    /// 会话初始化时是否跳过该变量。
    pub fn SkipInit(&self) -> bool {
        self.skipInit || self.IsNoop || !self.HasSessionScope()
    }

    /// 是否跳过系统变量缓存（如 GC 相关需直读）。
    pub fn SkipSysvarCache(&self) -> bool {
        matches!(
            self.Name.as_str(),
            vardef::TiDBGCEnable
                | vardef::TiDBGCRunInterval
                | vardef::TiDBGCLifetime
                | vardef::TiDBGCConcurrency
                | vardef::TiDBGCScanLockMode
                | vardef::TiDBExternalTS
        )
    }
}

/// 解析 Go `time.Duration` 风格字符串，返回纳秒。
pub fn parse_go_duration(value: &str) -> Option<i128> {
    if value.is_empty() {
        return None;
    }
    let (sign, mut rest) = match value.as_bytes()[0] {
        b'-' => (-1_i128, &value[1..]),
        b'+' => (1_i128, &value[1..]),
        _ => (1_i128, value),
    };
    if rest.is_empty() || rest == "0" {
        return (rest == "0").then_some(0);
    }
    let mut total = 0_f64;
    while !rest.is_empty() {
        let number_end = rest
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
            .map(|(index, ch)| index + ch.len_utf8())
            .last()?;
        let number = rest[..number_end].parse::<f64>().ok()?;
        rest = &rest[number_end..];
        let (unit, multiplier) = [
            ("ns", 1_f64),
            ("us", 1_000_f64),
            ("µs", 1_000_f64),
            ("μs", 1_000_f64),
            ("ms", 1_000_000_f64),
            ("s", 1_000_000_000_f64),
            ("m", 60_000_000_000_f64),
            ("h", 3_600_000_000_000_f64),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        total += number * multiplier;
        rest = &rest[unit.len()..];
    }
    if !total.is_finite() || total > i64::MAX as f64 {
        return None;
    }
    Some(sign * total.trunc() as i128)
}

/// 将纳秒格式化为 Go Duration 字符串。
pub fn format_go_duration(value: i128) -> String {
    if value == 0 {
        return "0s".to_owned();
    }
    let sign = if value < 0 { "-" } else { "" };
    let mut nanos = value.abs();
    if nanos < 1_000 {
        return format!("{sign}{nanos}ns");
    }
    if nanos < 1_000_000 {
        return format_duration_part(sign, nanos, 1_000, 3, "µs");
    }
    if nanos < 1_000_000_000 {
        return format_duration_part(sign, nanos, 1_000_000, 6, "ms");
    }
    let hours = nanos / 3_600_000_000_000;
    nanos %= 3_600_000_000_000;
    let minutes = nanos / 60_000_000_000;
    nanos %= 60_000_000_000;
    let seconds = format_duration_part("", nanos, 1_000_000_000, 9, "s");
    if hours > 0 {
        format!("{sign}{hours}h{minutes}m{seconds}")
    } else if minutes > 0 {
        format!("{sign}{minutes}m{seconds}")
    } else {
        format!("{sign}{seconds}")
    }
}

fn format_duration_part(
    sign: &str,
    value: i128,
    unit_nanos: i128,
    fraction_width: usize,
    suffix: &str,
) -> String {
    let whole = value / unit_nanos;
    let remainder = value % unit_nanos;
    if remainder == 0 {
        return format!("{sign}{whole}{suffix}");
    }
    let fraction = format!("{remainder:0fraction_width$}");
    format!("{sign}{whole}.{}{suffix}", fraction.trim_end_matches('0'))
}

/// 全局系统变量注册表（键为小写名）。
static SYS_VARS: LazyLock<RwLock<HashMap<String, Arc<SysVar>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 注册/覆盖一个系统变量定义。
pub fn RegisterSysVar(sys_var: SysVar) {
    SYS_VARS
        .write()
        .expect("sysvar registry poisoned")
        .insert(sys_var.Name.to_ascii_lowercase(), Arc::new(sys_var));
}

/// 按名注销系统变量。
pub fn UnregisterSysVar(name: &str) {
    SYS_VARS
        .write()
        .expect("sysvar registry poisoned")
        .remove(&name.to_ascii_lowercase());
}

/// 按名（大小写不敏感）查找系统变量。
pub fn GetSysVar(name: &str) -> Option<Arc<SysVar>> {
    SYS_VARS
        .read()
        .expect("sysvar registry poisoned")
        .get(&name.to_ascii_lowercase())
        .cloned()
}

/// 更新已注册变量的默认/当前 Value 字段并写回注册表。
pub fn SetSysVar(name: &str, value: &str) -> Result<(), VariableError> {
    let mut updated = GetSysVar(name)
        .map(|value| (*value).clone())
        .ok_or_else(|| VariableError::unknown(name))?;
    updated.Value = value.to_owned();
    RegisterSysVar(updated);
    Ok(())
}

/// 返回注册表快照（深拷贝 SysVar）。
pub fn GetSysVars() -> HashMap<String, SysVar> {
    SYS_VARS
        .read()
        .expect("sysvar registry poisoned")
        .iter()
        .map(|(name, sys_var)| (name.clone(), (**sys_var).clone()))
        .collect()
}

/// 按依赖标记排序：Depended 的变量名排在前面。
pub fn OrderByDependency(names: &HashMap<String, String>) -> Vec<String> {
    let registry = SYS_VARS.read().expect("sysvar registry poisoned");
    let mut depended = Vec::with_capacity(names.len());
    let mut not_depended = Vec::with_capacity(names.len());
    for name in names.keys() {
        if registry.get(name).is_some_and(|sys_var| sys_var.Depended) {
            depended.push(name.clone());
        } else {
            not_depended.push(name.clone());
        }
    }
    depended.extend(not_depended);
    depended
}

/// 测试用：清空注册表。
pub fn clear_sys_vars_for_test() {
    SYS_VARS.write().expect("sysvar registry poisoned").clear();
}
