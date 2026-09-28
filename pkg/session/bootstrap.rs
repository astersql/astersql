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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license in LICENSES/QL-LICENSE.

// Session bootstrap（系统初始化）与版本升级核心逻辑。
//
// 首次启动时由 DDL Owner 创建 mysql/sys 系统库表、写入 bootstrap 标志与全局变量；
// 已初始化集群则走 upgrade。对应 Go `bootstrap.go`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use astersql_meta_metadef::BootstrapSystemTableDefinitions;

/// 分布式 DDL Owner 锁在 etcd 上的键前缀。
pub const bootstrapOwnerKey: &str = "/tidb/distributeDDLOwnerLock/";
/// mysql.tidb 中布尔 True 字面量。
pub const varTrue: &str = "True";
/// mysql.tidb 中布尔 False 字面量。
pub const varFalse: &str = "False";
/// 标记集群是否已完成 bootstrap 的变量名。
pub const bootstrappedVar: &str = "bootstrapped";
/// 记录当前 bootstrap/升级版本号的变量名。
pub const tidbServerVersionVar: &str = "tidb_server_version";
/// 系统时区变量名。
pub const tidbSystemTZ: &str = "system_tz";
/// 新排序规则（new collation）是否启用的变量名。
pub const TidbNewCollationEnabled: &str = "new_collation_enabled";
/// 默认查询内存配额变量名。
pub const tidbDefMemoryQuotaQuery: &str = "default_memory_quota_query";
/// 默认 OOM（内存耗尽）动作变量名。
pub const tidbDefOOMAction: &str = "default_oom_action";
/// DDL 系统表版本变量名。
pub const tidbDDLTableVersion: &str = "ddl_table_version";
/// 集群 ID 变量名。
pub const tidbClusterID: &str = "cluster_id";
/// 内部 bootstrap SQL 执行超时。
pub const internalSQLTimeout: Duration = Duration::from_secs(75);
/// 测试中是否允许执行 bootstrap SQL 文件的开关。
pub static runBootstrapSQLFile: AtomicBool = AtomicBool::new(false);

/// Bootstrap 流程中可能出现的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError<E> {
    /// 下层运行时返回的外部错误。
    External(E),
    /// 期望的结果集缺失。
    MissingRecordSet,
    /// 缺少必要的系统变量。
    MissingVariable(String),
    /// bootstrap 版本号无法解析。
    InvalidBootstrapVersion(String),
    /// 旧密码十六进制解码失败。
    InvalidPasswordHex,
    /// 系统表不满足约束（如分区、AUTO_ID_CACHE=1）。
    InvalidSystemTable(&'static str),
}

impl<E> From<E> for BootstrapError<E> {
    fn from(error: E) -> Self {
        Self::External(error)
    }
}

/// 内部 SQL 绑定参数值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

/// 系统表基础元信息：保留 ID、表名与建表 SQL。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableBasicInfo {
    /// 系统表保留 ID，须单调且不复用。
    pub id: i64,
    /// 表名。
    pub name: &'static str,
    /// 首次 bootstrap 时执行的建表 SQL。
    pub create_sql: &'static str,
}

/// 系统库基础元信息及其包含的表列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseBasicInfo {
    /// 系统库保留 ID。
    pub id: i64,
    /// 库名（如 `mysql`、`sys`）。
    pub name: &'static str,
    /// 该库下的系统表列表。
    pub tables: &'static [TableBasicInfo],
}

/// 按 next-gen schema 版本增量引入的系统库表集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct versionedBootstrapSchema {
    /// 引入该批表的 schema 版本。
    pub version: i32,
    /// 该版本新增或补齐的数据库定义。
    pub databases: &'static [DatabaseBasicInfo],
}

/// 经典/基线版本下 mysql 库内的系统表清单。
const fn bootstrap_table(index: usize, id: i64) -> TableBasicInfo {
    TableBasicInfo {
        id,
        name: BootstrapSystemTableDefinitions[index].name,
        create_sql: BootstrapSystemTableDefinitions[index].create_sql,
    }
}

const BASE_TABLES: &[TableBasicInfo] = &[
    bootstrap_table(0, 3),
    bootstrap_table(1, 4),
    bootstrap_table(2, 5),
    bootstrap_table(3, 6),
    bootstrap_table(4, 7),
    bootstrap_table(5, 8),
    bootstrap_table(6, 9),
    bootstrap_table(7, 10),
    bootstrap_table(8, 11),
    bootstrap_table(9, 12),
    bootstrap_table(10, 13),
    bootstrap_table(11, 14),
    bootstrap_table(12, 15),
    bootstrap_table(13, 16),
    bootstrap_table(14, 17),
    bootstrap_table(15, 18),
    bootstrap_table(16, 19),
    bootstrap_table(17, 20),
    bootstrap_table(18, 21),
    bootstrap_table(19, 22),
    bootstrap_table(20, 23),
    bootstrap_table(21, 24),
    bootstrap_table(22, 25),
    bootstrap_table(23, 26),
    bootstrap_table(24, 27),
    bootstrap_table(25, 28),
    bootstrap_table(26, 29),
    bootstrap_table(27, 30),
    bootstrap_table(28, 31),
    bootstrap_table(29, 32),
    bootstrap_table(30, 33),
    bootstrap_table(31, 34),
    bootstrap_table(32, 35),
    bootstrap_table(33, 36),
    bootstrap_table(34, 37),
    bootstrap_table(35, 38),
    bootstrap_table(36, 39),
    bootstrap_table(37, 40),
    bootstrap_table(38, 41),
    bootstrap_table(39, 42),
    bootstrap_table(40, 43),
    bootstrap_table(41, 44),
    bootstrap_table(42, 45),
    bootstrap_table(43, 46),
    bootstrap_table(44, 47),
    bootstrap_table(45, 48),
    bootstrap_table(46, 49),
    bootstrap_table(47, 50),
    bootstrap_table(48, 51),
    bootstrap_table(49, 52),
    bootstrap_table(50, 53),
    bootstrap_table(51, 54),
];
/// 数据脱敏策略相关系统表。
const MASKING_TABLES: &[TableBasicInfo] = &[bootstrap_table(52, 55)];
/// 基线系统库：mysql（含系统表）与空的 sys。
const BASE_DATABASES: &[DatabaseBasicInfo] = &[
    DatabaseBasicInfo {
        id: 1,
        name: "mysql",
        tables: BASE_TABLES,
    },
    DatabaseBasicInfo {
        id: 2,
        name: "sys",
        tables: &[],
    },
];
/// 脱敏功能引入时的增量库表定义。
const MASKING_DATABASES: &[DatabaseBasicInfo] = &[DatabaseBasicInfo {
    id: 1,
    name: "mysql",
    tables: MASKING_TABLES,
}];
/// 对外暴露的经典系统库列表。
pub const systemDatabases: &[DatabaseBasicInfo] = BASE_DATABASES;
/// 按版本递增的 next-gen bootstrap schema 列表。
pub const versionedBootstrapSchemas: &[versionedBootstrapSchema] = &[
    versionedBootstrapSchema {
        version: 1,
        databases: BASE_DATABASES,
    },
    versionedBootstrapSchema {
        version: 2,
        databases: MASKING_DATABASES,
    },
];

/// 用于约束检查的系统表属性摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTableInfo {
    /// 是否为分区表（系统表禁止分区）。
    pub partitioned: bool,
    /// 是否使用分离自增（AUTO_ID_CACHE=1，系统表禁止）。
    pub separate_auto_increment: bool,
}

/// Bootstrap 所需的运行时依赖（DDL Owner、内部 SQL、系统表创建等）。
pub trait BootstrapRuntime {
    type Error;
    type LockGuard;

    /// 为 bootstrap 初始化元数据锁（MDL）。
    fn init_mdl_for_bootstrap(&mut self) -> Result<(), Self::Error>;
    /// 当前节点是否为 DDL Owner（负责集群级 DDL 的领导者）。
    fn is_ddl_owner(&mut self) -> Result<bool, Self::Error>;
    /// 对已 bootstrap 集群执行版本升级。
    fn upgrade(&mut self) -> Result<(), Self::Error>;
    /// 休眠指定时长（非 Owner 等待 Owner 完成初始化）。
    fn sleep(&mut self, duration: Duration);
    /// 切换到系统库；若不存在返回 false。
    fn use_system_database(&mut self) -> Result<bool, Self::Error>;
    /// 读取 mysql.tidb 中的变量值。
    fn read_tidb_variable(&mut self, name: &str) -> Result<Option<String>, Self::Error>;
    /// 提交当前事务。
    fn commit_transaction(&mut self) -> Result<(), Self::Error>;
    /// 在超时内执行内部 SQL。
    fn execute_internal(
        &mut self,
        sql: &str,
        args: &[SqlValue],
        timeout: Duration,
    ) -> Result<(), Self::Error>;
    /// 查询全局变量当前值。
    fn query_global_variable(&mut self, name: &str) -> Result<Option<String>, Self::Error>;
    /// 获取分布式 Owner 锁。
    fn acquire_distributed_lock(&mut self, key: &str) -> Result<Self::LockGuard, Self::Error>;
    /// 读取 next-gen schema 版本。
    fn nextgen_schema_version(&mut self) -> Result<i32, Self::Error>;
    /// 创建系统库。
    fn create_system_database(&mut self, database: DatabaseBasicInfo) -> Result<(), Self::Error>;
    /// 创建系统表并做 Region 预分裂。
    fn create_and_split_system_table(
        &mut self,
        database_id: i64,
        table: TableBasicInfo,
    ) -> Result<(), Self::Error>;
    /// 更新 next-gen schema 版本。
    fn set_nextgen_schema_version(&mut self, version: i32) -> Result<(), Self::Error>;
    /// 是否为经典内核（相对 next-gen）。
    fn classic_kernel(&mut self) -> bool;
    /// 插入内置 SQL Bind 信息。
    fn insert_builtin_bind_info(&mut self) -> Result<(), Self::Error>;
    /// 读取可选的初始化 SQL 文件内容。
    fn initialize_sql_file(&mut self) -> Result<Option<String>, Self::Error>;
    /// 解析 SQL 文本为语句列表。
    fn parse_sql(&mut self, sql: &str) -> Result<Vec<String>, Self::Error>;
    /// 执行单条语句。
    fn execute_statement(&mut self, statement: &str) -> Result<(), Self::Error>;
    /// 安全模式下 bootstrap root 用户名（auth_socket）。
    fn secure_bootstrap_user(&mut self) -> Result<Option<String>, Self::Error>;
    /// 枚举应写入的全局系统变量默认值。
    fn global_system_variables(&mut self) -> Result<Vec<(String, String)>, Self::Error>;
    /// 当前代码期望的 bootstrap 版本号。
    fn current_bootstrap_version(&mut self) -> i64;
    /// 首次 bootstrap 时是否启用新排序规则。
    fn new_collation_enabled_on_first_bootstrap(&mut self) -> bool;
    /// 写入系统时区。
    fn write_system_timezone(&mut self) -> Result<(), Self::Error>;
    /// 写入新排序规则参数。
    fn write_new_collation_parameter(&mut self, enabled: bool) -> Result<(), Self::Error>;
    /// 写入语句摘要相关变量。
    fn write_statement_summary_variables(&mut self) -> Result<(), Self::Error>;
    /// 写入 DDL 表版本。
    fn write_ddl_table_version(&mut self) -> Result<(), Self::Error>;
    /// 写入集群 ID。
    fn write_cluster_id(&mut self) -> Result<(), Self::Error>;
    /// 计算 SHA1（用于旧密码升级）。
    fn sha1(&mut self, bytes: &[u8]) -> [u8; 20];
    /// 重建所有分区值映射与有序结构。
    fn rebuild_partition_maps(&mut self) -> Result<(), Self::Error>;
}

/// 测试中关闭 bootstrap SQL 文件执行开关。
pub fn DisableRunBootstrapSQLFileInTest() {
    runBootstrapSQLFile.store(false, Ordering::SeqCst);
}

/// 集群 bootstrap 入口：已初始化则 upgrade，否则由 DDL Owner 执行 DDL/DML 初始化。
pub fn bootstrap<R: BootstrapRuntime>(runtime: &mut R) -> Result<(), BootstrapError<R::Error>> {
    runtime.init_mdl_for_bootstrap()?;
    loop {
        if checkBootstrapped(runtime)? {
            runtime.upgrade()?;
            return Ok(());
        }
        // 仅 DDL Owner 执行实际初始化；非 Owner 短暂等待后重试。
        if runtime.is_ddl_owner()? {
            doDDLWorks(runtime)?;
            doDMLWorks(runtime)?;
            runBootstrapSQLFile.store(true, Ordering::SeqCst);
            return Ok(());
        }
        runtime.sleep(Duration::from_millis(200));
    }
}

/// 检查 mysql.tidb 中 bootstrapped 标志是否为 True。
pub fn checkBootstrapped<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<bool, BootstrapError<R::Error>> {
    if !runtime.use_system_database()? {
        return Ok(false);
    }
    let (value, is_null) = getTiDBVar(runtime, bootstrappedVar)?;
    let bootstrapped = !is_null && value == varTrue;
    if bootstrapped {
        runtime.commit_transaction()?;
    }
    Ok(bootstrapped)
}

/// 读取 mysql.tidb 变量；缺失时返回空串与 is_null=true。
pub fn getTiDBVar<R: BootstrapRuntime>(
    runtime: &mut R,
    name: &str,
) -> Result<(String, bool), BootstrapError<R::Error>> {
    match runtime.read_tidb_variable(name)? {
        Some(value) => Ok((value, false)),
        None => Ok((String::new(), true)),
    }
}

/// 获取分布式 bootstrap/DDL Owner 锁。
pub fn acquireLock<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<R::LockGuard, BootstrapError<R::Error>> {
    runtime
        .acquire_distributed_lock(bootstrapOwnerKey)
        .map_err(Into::into)
}

/// 若全局变量不存在则插入默认值。
pub fn initGlobalVariableIfNotExists<R: BootstrapRuntime>(
    runtime: &mut R,
    name: &str,
    value: SqlValue,
) -> Result<(), BootstrapError<R::Error>> {
    if runtime.query_global_variable(name)?.is_none() {
        mustExecute(
            runtime,
            "INSERT HIGH_PRIORITY IGNORE INTO mysql.global_variables VALUES (?, ?)",
            &[SqlValue::String(name.to_owned()), value],
        )?;
    }
    Ok(())
}

/// 写入默认 OOM 动作为 log，保证兼容性。
pub fn writeOOMAction<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<(), BootstrapError<R::Error>> {
    mustExecute(
        runtime,
        "INSERT INTO mysql.tidb VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE VARIABLE_VALUE=?",
        &[
            SqlValue::String(tidbDefOOMAction.to_owned()),
            SqlValue::String("log".to_owned()),
            SqlValue::String("oom-action compatibility value".to_owned()),
            SqlValue::String("log".to_owned()),
        ],
    )
}

/// 将当前 bootstrap 版本号写入 mysql.tidb。
pub fn updateBootstrapVer<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<(), BootstrapError<R::Error>> {
    let version = runtime.current_bootstrap_version();
    mustExecute(
        runtime,
        "INSERT INTO mysql.tidb VALUES (?, ?, 'TiDB bootstrap version.') ON DUPLICATE KEY UPDATE VARIABLE_VALUE=?",
        &[
            SqlValue::String(tidbServerVersionVar.to_owned()),
            SqlValue::Integer(version),
            SqlValue::Integer(version),
        ],
    )
}

/// 读取已存储的 bootstrap 版本；缺失视为 0。
pub fn getBootstrapVersion<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<i64, BootstrapError<R::Error>> {
    let (value, is_null) = getTiDBVar(runtime, tidbServerVersionVar)?;
    if is_null {
        return Ok(0);
    }
    value
        .parse()
        .map_err(|_| BootstrapError::InvalidBootstrapVersion(value))
}

/// 按 versionedBootstrapSchemas 增量创建尚未应用的系统库表。
pub fn bootstrapSchemas<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<(), BootstrapError<R::Error>> {
    let current = runtime.nextgen_schema_version()?;
    let mut largest = current;
    for schema in versionedBootstrapSchemas {
        // 已应用过的版本跳过。
        if current >= schema.version {
            continue;
        }
        for database in schema.databases {
            runtime.create_system_database(*database)?;
            for table in database.tables {
                runtime.create_and_split_system_table(database.id, *table)?;
            }
        }
        largest = largest.max(schema.version);
    }
    if largest > current {
        runtime.set_nextgen_schema_version(largest)?;
    }
    Ok(())
}

/// 首次 bootstrap 的 DDL 部分：建库建表、内置 bind、视图与默认资源组。
pub fn doDDLWorks<R: BootstrapRuntime>(runtime: &mut R) -> Result<(), BootstrapError<R::Error>> {
    if runtime.classic_kernel() {
        for database in systemDatabases {
            mustExecute(
                runtime,
                "CREATE DATABASE IF NOT EXISTS ?",
                &[SqlValue::String(database.name.to_owned())],
            )?;
        }
        for schema in versionedBootstrapSchemas {
            for database in schema.databases {
                for table in database.tables {
                    mustExecute(runtime, table.create_sql, &[])?;
                }
            }
        }
    }
    runtime.insert_builtin_bind_info()?;
    mustExecute(runtime, "CREATE VIEW mysql.tidb_mdl_view", &[])?;
    mustExecute(runtime, "CREATE VIEW sys.schema_unused_indexes", &[])?;
    mustExecute(runtime, "CREATE DATABASE IF NOT EXISTS test", &[])?;
    mustExecute(
        runtime,
        "ALTER RESOURCE GROUP default BACKGROUND=(TASK_TYPES='stats')",
        &[],
    )
}

/// 校验系统表不得分区、不得使用 AUTO_ID_CACHE=1。
pub fn checkSystemTableConstraint<E>(table: &SystemTableInfo) -> Result<(), BootstrapError<E>> {
    if table.partitioned {
        return Err(BootstrapError::InvalidSystemTable(
            "system table should not be partitioned table",
        ));
    }
    if table.separate_auto_increment {
        return Err(BootstrapError::InvalidSystemTable(
            "system table should not use AUTO_ID_CACHE=1",
        ));
    }
    Ok(())
}

/// 若配置了初始化 SQL 文件则解析并逐条执行。
pub fn doBootstrapSQLFile<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<(), BootstrapError<R::Error>> {
    let Some(contents) = runtime.initialize_sql_file()? else {
        return Ok(());
    };
    for statement in runtime.parse_sql(&contents)? {
        // Go treats parse/read failures as bootstrap failures, but an error from
        // one parsed statement is logged and ignored so later statements still
        // run. The runtime boundary has no logger, so preserve the control flow
        // and leave statement-level diagnostics to the runtime implementation.
        let _ = runtime.execute_statement(&statement);
    }
    Ok(())
}

/// 首次 bootstrap 的 DML 部分：root 用户、全局变量、标志与元信息写入。
pub fn doDMLWorks<R: BootstrapRuntime>(runtime: &mut R) -> Result<(), BootstrapError<R::Error>> {
    mustExecute(runtime, "BEGIN", &[])?;
    // 安全 bootstrap 使用 auth_socket；否则创建任意主机可连的空密码 root。
    match runtime.secure_bootstrap_user()? {
        Some(username) => mustExecute(
            runtime,
            r#"INSERT HIGH_PRIORITY INTO mysql.user (Host,User,authentication_string,plugin,Select_priv,Insert_priv,Update_priv,Delete_priv,Create_priv,Drop_priv,Process_priv,Grant_priv,References_priv,Alter_priv,Show_db_priv,Super_priv,Create_tmp_table_priv,Lock_tables_priv,Execute_priv,Create_view_priv,Show_view_priv,Create_routine_priv,Alter_routine_priv,Index_priv,Create_user_priv,Event_priv,Repl_slave_priv,Repl_client_priv,Trigger_priv,Create_role_priv,Drop_role_priv,Account_locked,Shutdown_priv,Reload_priv,FILE_priv,Config_priv,Create_Tablespace_Priv,User_attributes,Token_issuer) VALUES ('localhost','root',?,'auth_socket','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','N','Y','Y','Y','Y','Y',NULL,'')"#,
            &[SqlValue::String(username)],
        )?,
        None => mustExecute(
            runtime,
            r#"INSERT HIGH_PRIORITY INTO mysql.user (Host,User,authentication_string,plugin,Select_priv,Insert_priv,Update_priv,Delete_priv,Create_priv,Drop_priv,Process_priv,Grant_priv,References_priv,Alter_priv,Show_db_priv,Super_priv,Create_tmp_table_priv,Lock_tables_priv,Execute_priv,Create_view_priv,Show_view_priv,Create_routine_priv,Alter_routine_priv,Index_priv,Create_user_priv,Event_priv,Repl_slave_priv,Repl_client_priv,Trigger_priv,Create_role_priv,Drop_role_priv,Account_locked,Shutdown_priv,Reload_priv,FILE_priv,Config_priv,Create_Tablespace_Priv,User_attributes,Token_issuer) VALUES ('%','root','','mysql_native_password','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','Y','N','Y','Y','Y','Y','Y',NULL,'')"#,
            &[],
        )?,
    }

    for (name, value) in runtime.global_system_variables()? {
        mustExecute(
            runtime,
            "INSERT INTO mysql.global_variables VALUES (?, ?)",
            &[SqlValue::String(name), SqlValue::String(value)],
        )?;
    }
    mustExecute(
        runtime,
        "UPSERT mysql.tidb bootstrap flag",
        &[SqlValue::String(varTrue.to_owned())],
    )?;
    let version = runtime.current_bootstrap_version();
    mustExecute(
        runtime,
        "INSERT mysql.tidb bootstrap version",
        &[SqlValue::Integer(version)],
    )?;
    runtime.write_system_timezone()?;
    let collation = runtime.new_collation_enabled_on_first_bootstrap();
    runtime.write_new_collation_parameter(collation)?;
    runtime.write_statement_summary_variables()?;
    runtime.write_ddl_table_version()?;
    runtime.write_cluster_id()?;

    // 提交失败时短暂等待并复查是否已被其他节点完成 bootstrap。
    if let Err(commit_error) = runtime.commit_transaction() {
        runtime.sleep(Duration::from_secs(1));
        if checkBootstrapped(runtime)? {
            return Ok(());
        }
        return Err(BootstrapError::External(commit_error));
    }
    Ok(())
}

/// 以内部超时执行 SQL，失败则包装为 BootstrapError。
pub fn mustExecute<R: BootstrapRuntime>(
    runtime: &mut R,
    sql: &str,
    args: &[SqlValue],
) -> Result<(), BootstrapError<R::Error>> {
    runtime
        .execute_internal(sql, args, internalSQLTimeout)
        .map_err(Into::into)
}

/// 将旧版十六进制密码升级为 MySQL native password（`*SHA1`）格式。
pub fn oldPasswordUpgrade<R: BootstrapRuntime>(
    runtime: &mut R,
    password: &str,
) -> Result<String, BootstrapError<R::Error>> {
    let decoded = decode_hex(password).ok_or(BootstrapError::InvalidPasswordHex)?;
    let hash = runtime.sha1(&decoded);
    let encoded = hash
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    Ok(format!("*{encoded}"))
}

/// 将偶长度十六进制字符串解码为字节；奇数长度返回 None。
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

/// 重建全部表的分区值映射与有序结构（升级后一致性修复）。
pub fn rebuildAllPartitionValueMapAndSorted<R: BootstrapRuntime>(
    runtime: &mut R,
) -> Result<(), BootstrapError<R::Error>> {
    runtime.rebuild_partition_maps().map_err(Into::into)
}
