// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `SET` 语句执行器。
//
// 对应 Go 的 `SetExecutor`：处理用户变量、系统变量（SESSION / GLOBAL / INSTANCE）、
// `SET NAMES` / `SET CHARACTER SET`，以及快照读（`tidb_snapshot` / `txn_read_ts`）相关校验。
// `SetBackend` 抽象会话变量、权限、审计与 InfoSchema 边界。
#![allow(non_snake_case)]

use std::fmt::Display;

#[derive(Clone, Debug, Eq, PartialEq)]
/// SET 表达式求值结果：NULL 或字符串。
pub enum SetDatum {
    /// SQL NULL。
    Null,
    /// 字符串值。
    String(String),
}

impl SetDatum {
    /// 取字符串视图；NULL 视为空串。
    fn string(&self) -> &str {
        match self {
            Self::String(value) => value,
            Self::Null => "",
        }
    }
}

/// 一条变量赋值：名称、表达式、可选扩展值（如 collation）与作用域标志。
pub struct VarAssignment<E> {
    /// 变量名（用户变量或系统变量）。
    pub name: String,
    /// 右侧表达式。
    pub expression: E,
    /// 扩展值（如 SET NAMES 的 collation）。
    pub extend_value: Option<SetDatum>,
    /// 是否为 DEFAULT。
    pub is_default: bool,
    /// 是否为系统变量（否则用户变量）。
    pub is_system: bool,
    /// 是否 GLOBAL 作用域。
    pub is_global: bool,
    /// 是否 INSTANCE 作用域。
    pub is_instance: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 系统变量元数据。
pub struct SystemVariable {
    /// 规范变量名。
    pub name: String,
    /// 默认值。
    pub default_value: String,
    /// 是否为 noop（仅兼容、无实际效果）。
    pub is_noop: bool,
    /// 是否支持 INSTANCE 作用域。
    pub has_instance_scope: bool,
}

/// 系统变量查找结果。
pub enum SystemVariableLookup {
    /// 找到定义。
    Found(SystemVariable),
    /// 已移除的变量（静默忽略）。
    Removed,
    /// 未知变量。
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 排序规则及其所属字符集。
pub struct Collation {
    /// 排序规则名。
    pub name: String,
    /// 所属字符集名。
    pub charset_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// SET 相关特殊变量名与字符集常量集合（由后端提供，避免硬编码漂移）。
pub struct SetVariableNames {
    /// `SET NAMES` 伪变量名。
    pub set_names: String,
    /// `SET CHARACTER SET` 伪变量名。
    pub set_charset: String,
    /// DEFAULT 字符集名。
    pub default_charset: String,
    /// `utf8mb4` 字符集名。
    pub utf8mb4_charset: String,
    /// SET NAMES 需同步写入的会话变量列表。
    pub set_names_variables: Vec<String>,
    /// SET CHARSET 需同步写入的会话变量列表。
    pub set_charset_variables: Vec<String>,
    /// `collation_connection` 变量名。
    pub collation_connection: String,
    /// `character_set_connection` 变量名。
    pub character_set_connection: String,
    /// `character_set_database` 变量名。
    pub charset_database: String,
    /// `collation_database` 变量名。
    pub collation_database: String,
    /// 云存储 URI（日志中需脱敏）。
    pub cloud_storage_uri: String,
    /// 服务作用域变量名。
    pub service_scope: String,
    /// `tidb_snapshot` 变量名。
    pub snapshot: String,
    /// `txn_read_ts` 变量名。
    pub txn_read_ts: String,
    /// 一次性事务隔离级别变量名。
    pub txn_isolation_one_shot: String,
}

/// SET 结果 chunk 接口（执行后重置为空结果）。
pub trait SetResultChunk {
    /// 清空结果。
    fn reset(&mut self);
}

/// Production boundary for expression evaluation, SessionVars, SysVar hooks,
/// privileges, plugins, task metadata, snapshot validation, and InfoSchema.
/// All externally observable operations are mandatory.
/// 生产边界：表达式求值、SessionVars、SysVar 钩子、权限、插件、
/// 任务元数据、快照校验与 InfoSchema；对外可见操作均为必选。
pub trait SetBackend {
    /// 执行上下文。
    type Context;
    /// 错误类型。
    type Error: Display;
    /// 表达式类型。
    type Expression;
    /// 字段类型。
    type FieldType;
    /// 快照 InfoSchema。
    type SnapshotInfoSchema;

    /// 特殊变量名常量集。
    fn variable_names(&self) -> SetVariableNames;
    /// 求值表达式。
    fn evaluate(&self, expression: &Self::Expression) -> Result<SetDatum, Self::Error>;
    /// 表达式结果类型。
    fn expression_type(&self, expression: &Self::Expression) -> Self::FieldType;
    /// Datum 转字符串。
    fn datum_to_string(&self, datum: &SetDatum) -> Result<String, Self::Error>;

    /// 删除用户变量。
    fn unset_user_variable(&mut self, name: &str);
    /// 设置用户变量值。
    fn set_user_variable(&mut self, name: &str, value: SetDatum);
    /// 设置用户变量类型。
    fn set_user_variable_type(&mut self, name: &str, field_type: Self::FieldType);

    /// 查找系统变量定义。
    fn system_variable(&self, name: &str) -> SystemVariableLookup;
    /// 构造未知系统变量错误。
    fn unknown_system_variable(&self, name: &str) -> Self::Error;
    /// 修改该变量所需的动态权限列表。
    fn required_dynamic_privileges(
        &self,
        variable: &SystemVariable,
        global: bool,
        sem_enabled: bool,
    ) -> Vec<String>;
    /// 是否启用 SEM（安全增强模式）。
    fn sem_enabled(&self) -> bool;
    /// 是否启用 SEM v2。
    fn sem_v2_enabled(&self) -> bool;
    /// 变量在 SEM v2 下是否只读。
    fn sem_v2_read_only_variable(&self, name: &str) -> bool;
    /// 校验当前用户是否持有动态权限。
    fn verify_dynamic_privilege(&self, privilege: &str) -> bool;
    /// 构造权限拒绝错误。
    fn access_denied(&self, privilege: &str) -> Self::Error;
    /// 是否允许 noop 变量生效提示。
    fn noop_variables_enabled(&self) -> bool;
    /// 是否启用遗留 INSTANCE 作用域行为。
    fn legacy_instance_scope_enabled(&self) -> bool;
    /// 追加 noop 变量警告。
    fn append_noop_warning(&mut self, variable: &str);
    /// 追加 INSTANCE 作用域警告。
    fn append_instance_scope_warning(&mut self, variable: &str);

    /// 写入 GLOBAL 系统变量。
    fn set_global_system_variable(
        &mut self,
        context: &Self::Context,
        name: &str,
        value: &str,
    ) -> Result<(), Self::Error>;
    /// 写入 INSTANCE 系统变量。
    fn set_instance_system_variable(
        &mut self,
        context: &Self::Context,
        name: &str,
        value: &str,
    ) -> Result<(), Self::Error>;
    /// 审计全局变量变更事件。
    fn audit_global_variable_event(&mut self, name: &str, value: &str) -> Result<(), Self::Error>;
    /// 对 URL/URI 脱敏。
    fn redact_url(&self, value: &str) -> String;
    /// 记录全局/实例变量变更日志。
    fn log_global_variable(&self, instance: bool, name: &str, value: &str);
    /// 当前服务作用域。
    fn current_service_scope(&self) -> String;
    /// 按服务作用域初始化任务管理器。
    fn initialize_task_manager_for_service_scope(
        &mut self,
        context: &Self::Context,
        service_scope: &str,
    ) -> Result<(), Self::Error>;

    /// 取全局变量初始/默认值。
    fn global_system_variable_initial_value(&self, name: &str, default: &str) -> String;
    /// 读取 GLOBAL 系统变量。
    fn get_global_system_variable(
        &self,
        context: &Self::Context,
        name: &str,
    ) -> Result<String, Self::Error>;
    /// 无作用域限制地读取全局变量。
    fn get_global_system_variable_unscoped(&self, name: &str) -> Result<String, Self::Error>;
    /// 写入 SESSION 系统变量。
    fn set_session_system_variable(&mut self, name: &str, value: &str) -> Result<(), Self::Error>;
    /// 当前是否在事务中。
    fn in_transaction(&self) -> bool;
    /// 当前事务是否为过期读（stale read）。
    fn transaction_is_staleness(&self) -> bool;
    /// 事务中禁止修改事务特性的错误。
    fn cannot_change_transaction_characteristics(&self) -> Self::Error;

    /// 当前 `tidb_snapshot` 时间戳。
    fn snapshot_ts(&self) -> u64;
    /// 当前 `txn_read_ts` 时间戳。
    fn txn_read_ts(&self) -> u64;
    /// 设置 `tidb_snapshot`。
    fn set_snapshot_ts(&mut self, timestamp: u64);
    /// 设置 `txn_read_ts`。
    fn set_txn_read_ts(&mut self, timestamp: u64);
    /// 校验快照读时间戳合法性。
    fn validate_snapshot_read_ts(
        &self,
        context: &Self::Context,
        timestamp: u64,
        stale_read: bool,
        validate_for_tidb_snapshot: bool,
    ) -> Result<(), Self::Error>;
    /// 校验快照未被 GC。
    fn validate_gc_snapshot(&self, timestamp: u64) -> Result<(), Self::Error>;
    /// 按时间戳加载快照 InfoSchema。
    fn snapshot_info_schema(&self, timestamp: u64)
    -> Result<Self::SnapshotInfoSchema, Self::Error>;
    /// 将本地临时表附着到快照 InfoSchema。
    fn attach_local_temporary_tables(
        &self,
        info_schema: Self::SnapshotInfoSchema,
    ) -> Self::SnapshotInfoSchema;
    /// 安装或清除会话快照 InfoSchema。
    fn set_snapshot_info_schema(&mut self, info_schema: Option<Self::SnapshotInfoSchema>);
    /// 记录快照 InfoSchema 加载日志。
    fn log_snapshot_info_schema(&self, timestamp: u64);
    /// 记录会话变量变更日志。
    fn log_session_variable(&self, name: &str, value: &str);

    /// utf8mb4 默认排序规则。
    fn default_collation_for_utf8mb4(&self) -> String;
    /// 字符集默认排序规则。
    fn default_collation(&self, charset: &str) -> Result<String, Self::Error>;
    /// 按名查找排序规则。
    fn collation(&self, name: &str) -> Result<Collation, Self::Error>;
    /// 排序规则与字符集不匹配错误。
    fn collation_charset_mismatch(&self, collation: &str, charset: &str) -> Self::Error;
}

/// `SET` 执行器：一次性语句，`done` 防止重复执行。
pub struct SetExecutor<B: SetBackend> {
    /// 后端 / 基类执行器。
    pub BaseExecutor: B,
    /// 待赋值变量列表。
    pub vars: Vec<VarAssignment<B::Expression>>,
    /// 是否已执行过。
    pub done: bool,
}

impl<B: SetBackend> SetExecutor<B> {
    /// 执行全部赋值：NAMES/CHARSET、用户变量或系统变量。
    pub fn Next<C: SetResultChunk>(
        &mut self,
        context: &B::Context,
        request: &mut C,
    ) -> Result<(), B::Error> {
        request.reset();
        if self.done {
            return Ok(());
        }
        self.done = true;
        let names = self.BaseExecutor.variable_names();
        for index in 0..self.vars.len() {
            let assignment_name = self.vars[index].name.clone();
            // SET NAMES / SET CHARACTER SET 走字符集专用路径。
            if assignment_name == names.set_names || assignment_name == names.set_charset {
                let set_names = assignment_name == names.set_names;
                if self.vars[index].is_default {
                    self.setCharset(&names.default_charset, "", set_names)?;
                } else {
                    let value = self.BaseExecutor.evaluate(&self.vars[index].expression)?;
                    let charset = value.string().to_owned();
                    let collation = self.vars[index]
                        .extend_value
                        .as_ref()
                        .map(SetDatum::string)
                        .unwrap_or("")
                        .to_owned();
                    self.setCharset(&charset, &collation, set_names)?;
                }
                continue;
            }

            let name = assignment_name.to_lowercase();
            // 用户变量：NULL 则删除，否则写入值与类型。
            if !self.vars[index].is_system {
                let value = self.BaseExecutor.evaluate(&self.vars[index].expression)?;
                if value == SetDatum::Null {
                    self.BaseExecutor.unset_user_variable(&name);
                } else {
                    let field_type = self
                        .BaseExecutor
                        .expression_type(&self.vars[index].expression);
                    self.BaseExecutor.set_user_variable(&name, value);
                    self.BaseExecutor.set_user_variable_type(&name, field_type);
                }
                continue;
            }
            self.setSysVariable(context, &name, index)?;
        }
        Ok(())
    }

    /// 设置系统变量：校验动态权限与 SEM，再按 GLOBAL/INSTANCE/SESSION 写入；
    /// 涉及快照变量时做时间戳与 InfoSchema 校验，失败则回滚时间戳。
    fn setSysVariable(
        &mut self,
        context: &B::Context,
        name: &str,
        assignment_index: usize,
    ) -> Result<(), B::Error> {
        let system_variable = match self.BaseExecutor.system_variable(name) {
            SystemVariableLookup::Found(variable) => variable,
            SystemVariableLookup::Removed => return Ok(()),
            SystemVariableLookup::Unknown => {
                return Err(self.BaseExecutor.unknown_system_variable(name));
            }
        };
        // 校验修改该变量所需的动态权限。
        let sem_enabled = self.BaseExecutor.sem_enabled();
        for privilege in self.BaseExecutor.required_dynamic_privileges(
            &system_variable,
            self.vars[assignment_index].is_global,
            sem_enabled,
        ) {
            if !self.BaseExecutor.verify_dynamic_privilege(&privilege) {
                let message = if sem_enabled {
                    privilege
                } else {
                    format!("SUPER or {privilege}")
                };
                return Err(self.BaseExecutor.access_denied(&message));
            }
        }
        if self.BaseExecutor.sem_v2_enabled()
            && self
                .BaseExecutor
                .sem_v2_read_only_variable(&self.vars[assignment_index].name)
            && !self
                .BaseExecutor
                .verify_dynamic_privilege("RESTRICTED_VARIABLES_ADMIN")
        {
            return Err(self
                .BaseExecutor
                .access_denied("RESTRICTED_VARIABLES_ADMIN"));
        }
        if system_variable.is_noop && !self.BaseExecutor.noop_variables_enabled() {
            self.BaseExecutor.append_noop_warning(&system_variable.name);
        }
        if system_variable.has_instance_scope
            && !self.vars[assignment_index].is_global
            && self.BaseExecutor.legacy_instance_scope_enabled()
        {
            self.vars[assignment_index].is_instance = true;
            self.BaseExecutor
                .append_instance_scope_warning(&system_variable.name);
        }

        // GLOBAL / INSTANCE：写持久化配置、审计并可能刷新服务作用域。
        if self.vars[assignment_index].is_global || self.vars[assignment_index].is_instance {
            let value = self.getVarValue(context, assignment_index, Some(&system_variable))?;
            if self.vars[assignment_index].is_global {
                self.BaseExecutor
                    .set_global_system_variable(context, name, &value)?;
            } else {
                self.BaseExecutor
                    .set_instance_system_variable(context, name, &value)?;
            }
            let names = self.BaseExecutor.variable_names();
            let shown_value = if name.eq_ignore_ascii_case(&names.cloud_storage_uri) {
                self.BaseExecutor.redact_url(&value)
            } else if astersql_sessionctx_variable::is_embedding_api_key(name) && !value.is_empty()
            {
                "******".into()
            } else {
                value.clone()
            };
            self.BaseExecutor
                .audit_global_variable_event(name, &shown_value)?;
            self.BaseExecutor.log_global_variable(
                self.vars[assignment_index].is_instance,
                name,
                &shown_value,
            );
            if name == names.service_scope {
                let service_scope = self.BaseExecutor.current_service_scope();
                return self
                    .BaseExecutor
                    .initialize_task_manager_for_service_scope(context, &service_scope);
            }
            return Ok(());
        }

        let value = self.getVarValue(context, assignment_index, None)?;
        let names = self.BaseExecutor.variable_names();
        let old_snapshot_ts = self.snapshotTimestampByName(name, &names);
        // 事务中禁止修改部分事务特性 / 过期读快照。
        if self.BaseExecutor.in_transaction() {
            if name == names.txn_isolation_one_shot || name == names.txn_read_ts {
                return Err(self
                    .BaseExecutor
                    .cannot_change_transaction_characteristics());
            }
            if name == names.snapshot && self.BaseExecutor.transaction_is_staleness() {
                return Err(self
                    .BaseExecutor
                    .cannot_change_transaction_characteristics());
            }
        }
        self.BaseExecutor
            .set_session_system_variable(name, &value)?;
        let new_snapshot_ts = self.snapshotTimestampByName(name, &names);
        // 快照时间戳变更：校验合法性；失败则恢复旧值。
        if new_snapshot_ts > 0 && new_snapshot_ts != old_snapshot_ts {
            let stale_read = name == names.txn_read_ts;
            let validation = self.BaseExecutor.validate_snapshot_read_ts(
                context,
                new_snapshot_ts,
                stale_read,
                !stale_read,
            );
            let validation = if validation.is_ok() && !stale_read {
                self.BaseExecutor.validate_gc_snapshot(new_snapshot_ts)
            } else {
                validation
            };
            if let Err(error) = validation {
                self.restoreSnapshotTimestamp(name, &names, old_snapshot_ts);
                return Err(error);
            }
        }
        if let Err(error) = self.loadSnapshotInfoSchemaIfNeeded(name, new_snapshot_ts, &names) {
            self.restoreSnapshotTimestamp(name, &names, old_snapshot_ts);
            return Err(error);
        }
        self.BaseExecutor.log_session_variable(name, &value);
        Ok(())
    }

    /// 设置字符集与排序规则：SET NAMES 写连接相关变量；
    /// SET CHARSET 将 connection 对齐到 database 字符集。
    fn setCharset(
        &mut self,
        charset: &str,
        collation: &str,
        set_names: bool,
    ) -> Result<(), B::Error> {
        let names = self.BaseExecutor.variable_names();
        // 未指定 collation 时按字符集取默认（utf8mb4 有专用默认）。
        let collation = if collation.is_empty() {
            if charset == names.utf8mb4_charset {
                self.BaseExecutor.default_collation_for_utf8mb4()
            } else {
                self.BaseExecutor.default_collation(charset)?
            }
        } else {
            let resolved = self.BaseExecutor.collation(collation)?;
            if resolved.charset_name != charset {
                return Err(self
                    .BaseExecutor
                    .collation_charset_mismatch(&resolved.name, charset));
            }
            collation.to_owned()
        };
        if set_names {
            for variable in &names.set_names_variables {
                self.BaseExecutor
                    .set_session_system_variable(variable, charset)?;
            }
            return self
                .BaseExecutor
                .set_session_system_variable(&names.collation_connection, &collation);
        }
        for variable in &names.set_charset_variables {
            self.BaseExecutor
                .set_session_system_variable(variable, charset)?;
        }
        let database_charset = self
            .BaseExecutor
            .get_global_system_variable_unscoped(&names.charset_database)?;
        let database_collation = self
            .BaseExecutor
            .get_global_system_variable_unscoped(&names.collation_database)?;
        self.BaseExecutor
            .set_session_system_variable(&names.character_set_connection, &database_charset)?;
        self.BaseExecutor
            .set_session_system_variable(&names.collation_connection, &database_collation)
    }

    /// 解析赋值右侧：DEFAULT 取全局初值，否则求值并转为字符串。
    fn getVarValue(
        &self,
        context: &B::Context,
        assignment_index: usize,
        system_variable: Option<&SystemVariable>,
    ) -> Result<String, B::Error> {
        let assignment = &self.vars[assignment_index];
        if assignment.is_default {
            if let Some(variable) = system_variable {
                return Ok(self.BaseExecutor.global_system_variable_initial_value(
                    &variable.name,
                    &variable.default_value,
                ));
            }
            return self
                .BaseExecutor
                .get_global_system_variable(context, &assignment.name);
        }
        let value = self.BaseExecutor.evaluate(&assignment.expression)?;
        if value == SetDatum::Null {
            return Ok(String::new());
        }
        // Producing an owned String preserves Go's explicit strings.Clone.
        // 生成自有 String，对应 Go 的 strings.Clone。
        self.BaseExecutor
            .datum_to_string(&value)
            .map(|value| value.clone())
    }

    /// 若变量为快照相关，则加载对应 InfoSchema。
    fn loadSnapshotInfoSchemaIfNeeded(
        &mut self,
        name: &str,
        snapshot_ts: u64,
        names: &SetVariableNames,
    ) -> Result<(), B::Error> {
        if name != names.snapshot && name != names.txn_read_ts {
            return Ok(());
        }
        loadSnapshotInfoSchemaIfNeeded(&mut self.BaseExecutor, snapshot_ts)
    }

    /// 按变量名读取当前快照时间戳；无关变量返回 0。
    fn snapshotTimestampByName(&self, name: &str, names: &SetVariableNames) -> u64 {
        if name == names.snapshot {
            self.BaseExecutor.snapshot_ts()
        } else if name == names.txn_read_ts {
            self.BaseExecutor.txn_read_ts()
        } else {
            0
        }
    }

    /// 将快照时间戳恢复为旧值（校验失败回滚）。
    fn restoreSnapshotTimestamp(&mut self, name: &str, names: &SetVariableNames, timestamp: u64) {
        if name == names.snapshot {
            self.BaseExecutor.set_snapshot_ts(timestamp);
        } else if name == names.txn_read_ts {
            self.BaseExecutor.set_txn_read_ts(timestamp);
        }
    }
}

/// 按时间戳加载快照 InfoSchema；时间为 0 则清除。
/// 会附着本地临时表后再安装到会话。
pub fn loadSnapshotInfoSchemaIfNeeded<B: SetBackend>(
    backend: &mut B,
    snapshot_ts: u64,
) -> Result<(), B::Error> {
    // 关闭快照读：清除会话上的快照 InfoSchema。
    if snapshot_ts == 0 {
        backend.set_snapshot_info_schema(None);
        return Ok(());
    }
    backend.log_snapshot_info_schema(snapshot_ts);
    let info_schema = backend.snapshot_info_schema(snapshot_ts)?;
    let info_schema = backend.attach_local_temporary_tables(info_schema);
    backend.set_snapshot_info_schema(Some(info_schema));
    Ok(())
}
