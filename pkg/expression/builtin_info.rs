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

// 会话与系统信息类内置函数内核。
//
// 对应 Go `builtin_info.go`：提供 DATABASE/USER/CURRENT_USER/CURRENT_ROLE、
// LAST_INSERT_ID、BENCHMARK、字符集/排序规则元数据、TiDB 版本与 DDL Owner、
// Key 编解码、MVCC 查询、SQL digest / Plan 解码、SEQUENCE（NEXTVAL/LASTVAL/SETVAL）
// 以及 FORMAT_BYTES / FORMAT_NANO_TIME 等标量实现。MVCC 指多版本并发控制，用于查看键上的历史版本。

use std::collections::HashMap;

use crate::{mysql, parser};
use printer_dependency as printer;
use serde::Serialize;

use crate::builtin_ilike_kernel::ExpressionError;

/// Login and authenticated identities are intentionally kept separately.
///
/// 登录身份与认证身份分开保存：`USER()` 用登录名，`CURRENT_USER()` 用认证名。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserIdentity {
    pub username: String,
    pub hostname: String,
    pub auth_username: String,
    pub auth_hostname: String,
}

impl UserIdentity {
    /// `USER()` 返回的登录身份字符串 `user@host`。
    pub fn login_string(&self) -> String {
        format!("{}@{}", self.username, self.hostname)
    }

    /// `CURRENT_USER()` 返回的认证身份；缺省字段回退到登录身份。
    pub fn authenticated_string(&self) -> String {
        let username = if self.auth_username.is_empty() {
            &self.username
        } else {
            &self.auth_username
        };
        let hostname = if self.auth_hostname.is_empty() {
            &self.hostname
        } else {
            &self.auth_hostname
        };
        format!("{username}@{hostname}")
    }
}

/// 角色身份，用于 `CURRENT_ROLE()` 的规范输出。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleIdentity {
    pub username: String,
    pub hostname: String,
}

impl RoleIdentity {
    /// 反引号引用后的 `user@host`，反引号自身按 MySQL 规则加倍转义。
    pub fn canonical_string(&self) -> String {
        fn quote(value: &str) -> String {
            format!("`{}`", value.replace('`', "``"))
        }
        format!("{}@{}", quote(&self.username), quote(&self.hostname))
    }
}

/// Session fields read or mutated by the information builtins in the Go file.
///
/// 信息类内置函数读写的会话快照字段集合。
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub current_db: String,
    pub last_found_rows: u64,
    pub user: Option<UserIdentity>,
    /// `None` represents a missing optional property; an empty vector is the
    /// valid MySQL `CURRENT_ROLE() = NONE` state.
    ///
    /// `None` 表示会话属性缺失会报错；空向量对应 `CURRENT_ROLE() = NONE`。
    pub active_roles: Option<Vec<RoleIdentity>>,
    pub connection_id: u64,
    pub previous_affected_rows: i64,
    pub previous_last_insert_id: u64,
    pub last_insert_id: u64,
    pub resource_group_name: String,
    /// Presence corresponds to StmtCtx.HasResourceGroup, including an empty hint.
    ///
    /// `Some` 对应 StmtCtx 已设置资源组 hint（可为空串）；`None` 用会话默认名。
    pub hinted_resource_group: Option<String>,
    /// 序列 id → 最近一次 NEXTVAL 结果，供 LASTVAL 查询。
    pub sequence_state: HashMap<i64, i64>,
}

impl Default for SessionInfo {
    fn default() -> Self {
        Self {
            current_db: String::new(),
            last_found_rows: 0,
            user: None,
            active_roles: Some(Vec::new()),
            connection_id: 0,
            previous_affected_rows: 0,
            previous_last_insert_id: 0,
            last_insert_id: 0,
            resource_group_name: String::new(),
            hinted_resource_group: None,
            sequence_state: HashMap::new(),
        }
    }
}

/// `DATABASE()`：当前库名；空串视为未选库返回 NULL。
pub fn database(session: &SessionInfo) -> Option<String> {
    (!session.current_db.is_empty()).then(|| session.current_db.clone())
}

/// `FOUND_ROWS()`：上一语句满足条件的行数（含 LIMIT 前）。
pub fn found_rows(session: &SessionInfo) -> i64 {
    session.last_found_rows as i64
}

/// `CURRENT_USER()`：认证身份；会话无用户则报错。
pub fn current_user(session: &SessionInfo) -> Result<String, ExpressionError> {
    session
        .user
        .as_ref()
        .map(UserIdentity::authenticated_string)
        .ok_or(ExpressionError::MissingSession("CURRENT_USER"))
}

/// `CURRENT_ROLE()`：活动角色列表排序后逗号拼接；空列表为 `NONE`。
pub fn current_role(session: &SessionInfo) -> Result<String, ExpressionError> {
    let roles = session
        .active_roles
        .as_ref()
        .ok_or(ExpressionError::MissingSession("CURRENT_ROLE"))?;
    if roles.is_empty() {
        return Ok("NONE".to_owned());
    }
    let mut result = roles
        .iter()
        .map(RoleIdentity::canonical_string)
        .collect::<Vec<_>>();
    // 与 Go 一致：角色名排序后再拼接，保证输出稳定。
    result.sort_unstable();
    Ok(result.join(","))
}

/// 当前资源组：优先语句 hint，否则会话默认名。
pub fn current_resource_group(session: &SessionInfo) -> String {
    session
        .hinted_resource_group
        .clone()
        .unwrap_or_else(|| session.resource_group_name.clone())
}

/// `USER()`：登录身份；会话无用户则报错。
pub fn user(session: &SessionInfo) -> Result<String, ExpressionError> {
    session
        .user
        .as_ref()
        .map(UserIdentity::login_string)
        .ok_or(ExpressionError::MissingSession("USER"))
}

/// `CONNECTION_ID()`：当前连接 id。
pub fn connection_id(session: &SessionInfo) -> i64 {
    session.connection_id as i64
}

/// 无参 `LAST_INSERT_ID()`：上一语句记录的自增 id。
pub fn last_insert_id(session: &SessionInfo) -> i64 {
    session.previous_last_insert_id as i64
}

/// 带参 `LAST_INSERT_ID(expr)`：非 NULL 时写入会话并原样返回。
pub fn last_insert_id_with_id(session: &mut SessionInfo, value: Option<i64>) -> Option<i64> {
    if let Some(value) = value {
        session.last_insert_id = value as u64;
    }
    value
}

/// `VERSION()`：MySQL 兼容服务器版本串。
pub fn version() -> String {
    mysql::r#const::ServerVersion()
}

/// `TIDB_VERSION()`：TiDB/AsterSQL 构建信息。
pub fn tidb_version() -> String {
    printer::GetTiDBInfo()
}

/// `TIDB_IS_DDL_OWNER()`：本节点是否为 DDL Owner（DDL 调度协调者）。
pub fn tidb_is_ddl_owner(is_owner: bool) -> i64 {
    i64::from(is_owner)
}

/// BENCHMARK evaluates the expression exactly loop_count times. Negative counts
/// return NULL; zero is a valid non-NULL result and performs no evaluations.
///
/// `BENCHMARK(count, expr)`：精确循环求值 count 次；负数返回 NULL，零次仍返回 0。
pub fn benchmark(
    loop_count: i64,
    mut expression: impl FnMut() -> Result<(), ExpressionError>,
) -> Result<Option<i64>, ExpressionError> {
    if loop_count < 0 {
        return Ok(None);
    }
    for _ in 0..loop_count {
        expression()?;
    }
    Ok(Some(0))
}

/// 表达式的字符集 / 排序规则 / 强制等级元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionMetadata {
    pub charset: String,
    pub collation: String,
    pub coercibility: i64,
}

/// `CHARSET(expr)`：表达式字符集名。
pub fn charset(metadata: &ExpressionMetadata) -> String {
    metadata.charset.clone()
}

/// `COERCIBILITY(expr)`：排序规则强制等级（越低优先级越高）。
pub fn coercibility(metadata: &ExpressionMetadata) -> i64 {
    metadata.coercibility
}

/// `COLLATION(expr)`：表达式排序规则名。
pub fn collation(metadata: &ExpressionMetadata) -> String {
    metadata.collation.clone()
}

/// `ROW_COUNT()`：上一语句影响行数。
pub fn row_count(session: &SessionInfo) -> i64 {
    session.previous_affected_rows
}

/// Values accepted by the injected record/index key encoders. The Go builtin
/// forwards each argument with its native evaluation type; this enum preserves
/// those types without coupling expression to a schema implementation.
///
/// Key 编解码入参的求值类型联合；保持原生类型，避免耦合 schema 实现。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum InfoValue {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Json(serde_json::Value),
}

/// 记录键 / 索引键编解码注入点，由上层 schema 实现。
pub trait KeyCodec: Send + Sync {
    fn encode_record_key(
        &self,
        arguments: &[InfoValue],
    ) -> Result<Option<Vec<u8>>, ExpressionError>;
    fn encode_index_key(&self, arguments: &[InfoValue])
    -> Result<Option<Vec<u8>>, ExpressionError>;
    fn decode_key(&self, source: &str) -> Result<String, ExpressionError>;
}

/// 将通用 AccessDenied 提升为带用户与表名的 TableAccessDenied。
fn map_table_access_error(
    error: ExpressionError,
    session: &SessionInfo,
    table_name: Option<&str>,
) -> ExpressionError {
    if matches!(error, ExpressionError::AccessDenied(_))
        && let (Some(user), Some(table)) = (session.user.as_ref(), table_name)
    {
        return ExpressionError::TableAccessDenied {
            user: user.auth_username.clone(),
            host: user.auth_hostname.clone(),
            table: table.to_owned(),
        };
    }
    error
}

/// `TIDB_ENCODE_RECORD_KEY`：编码行记录键并转十六进制；未注入 codec 则报错。
pub fn encode_record_key(
    codec: Option<&dyn KeyCodec>,
    session: &SessionInfo,
    table_name: Option<&str>,
    arguments: &[InfoValue],
) -> Result<Option<String>, ExpressionError> {
    let codec = codec.ok_or_else(|| {
        ExpressionError::External("EncodeRecordKeyFromRow is not initialized".into())
    })?;
    codec
        .encode_record_key(arguments)
        .map_err(|error| map_table_access_error(error, session, table_name))
        .map(|key| key.map(hex::encode))
}

/// `TIDB_ENCODE_INDEX_KEY`：编码索引键并转十六进制。
pub fn encode_index_key(
    codec: Option<&dyn KeyCodec>,
    session: &SessionInfo,
    table_name: Option<&str>,
    arguments: &[InfoValue],
) -> Result<Option<String>, ExpressionError> {
    let codec = codec.ok_or_else(|| {
        ExpressionError::External("EncodeIndexKeyFromRow is not initialized".into())
    })?;
    codec
        .encode_index_key(arguments)
        .map_err(|error| map_table_access_error(error, session, table_name))
        .map(|key| key.map(hex::encode))
}

/// `TIDB_DECODE_KEY`：解码十六进制键；无 codec 时原样返回。
pub fn decode_key(
    source: Option<&str>,
    codec: Option<&dyn KeyCodec>,
) -> Result<Option<String>, ExpressionError> {
    let Some(source) = source else {
        return Ok(None);
    };
    match codec {
        Some(codec) => codec.decode_key(source).map(Some),
        None => Ok(Some(source.to_owned())),
    }
}

/// MVCC 查询结果：JSON 主体 + 是否含版本条目标志。
#[derive(Clone, Debug, Serialize)]
pub struct MvccResponse {
    #[serde(flatten)]
    pub value: serde_json::Value,
    #[serde(skip)]
    pub has_entries: bool,
}

/// 按编码键查询 MVCC 与索引键转换的存储侧注入接口。
pub trait MvccProvider: Send + Sync {
    fn get_mvcc_by_encoded_key(&self, key: &[u8]) -> Result<MvccResponse, ExpressionError>;
    fn is_index_key(&self, key: &[u8]) -> bool;
    fn is_temp_index_key(&self, key: &[u8]) -> bool;
    fn to_temp_index_key(&self, key: &mut Vec<u8>);
}

/// 输出给 `TIDB_MVCC_INFO` 的单键条目。
#[derive(Serialize)]
struct MvccInfoResult {
    key: String,
    mvcc: MvccResponse,
}

/// `TIDB_MVCC_INFO`：需 SUPER 权限；索引键还会尝试附带临时索引键的 MVCC。
pub fn tidb_mvcc_info(
    source: Option<&str>,
    has_super_privilege: bool,
    provider: &dyn MvccProvider,
) -> Result<Option<String>, ExpressionError> {
    if !has_super_privilege {
        return Err(ExpressionError::AccessDenied("SUPER".into()));
    }
    let Some(source) = source else {
        return Ok(None);
    };
    let mut key =
        hex::decode(source).map_err(|error| ExpressionError::InvalidArgument(error.to_string()))?;
    let response = provider.get_mvcc_by_encoded_key(&key)?;
    let mut result = vec![MvccInfoResult {
        key: source.to_owned(),
        mvcc: response,
    }];
    // 普通索引键：若存在临时索引键且有条目，一并返回便于对照。
    if provider.is_index_key(&key) && !provider.is_temp_index_key(&key) {
        provider.to_temp_index_key(&mut key);
        let response = provider.get_mvcc_by_encoded_key(&key)?;
        if response.has_entries {
            result.push(MvccInfoResult {
                key: hex::encode(&key),
                mvcc: response,
            });
        }
    }
    serde_json::to_string(&result)
        .map(Some)
        .map_err(|error| ExpressionError::External(error.to_string()))
}

/// 按 digest 批量取回归一化 SQL 文本的全局检索接口。
pub trait SqlDigestRetriever: Send + Sync {
    fn retrieve_global(
        &self,
        digests: &[String],
    ) -> Result<HashMap<String, String>, ExpressionError>;
}

/// Decode a JSON array of SQL digests. Non-string elements retain their array
/// positions as NULL, retrieval failures become NULL plus a warning, and
/// cancellation remains an error as in Go.
///
/// 解码 JSON digest 数组：非字符串位保留为 NULL；检索失败变 NULL+警告；取消仍报错。
pub fn decode_sql_digests(
    source: Option<&str>,
    truncate_length: Option<i64>,
    has_process_privilege: bool,
    retriever: &dyn SqlDigestRetriever,
) -> Result<Option<String>, ExpressionError> {
    decode_sql_digests_with_warnings(
        source,
        truncate_length,
        has_process_privilege,
        retriever,
        &mut Vec::new(),
    )
}

/// 带警告输出通道的 digest 解码实现；需 PROCESS 权限。
pub fn decode_sql_digests_with_warnings(
    source: Option<&str>,
    truncate_length: Option<i64>,
    has_process_privilege: bool,
    retriever: &dyn SqlDigestRetriever,
    warnings: &mut Vec<String>,
) -> Result<Option<String>, ExpressionError> {
    if !has_process_privilege {
        return Err(ExpressionError::AccessDenied("PROCESS".into()));
    }
    let Some(source) = source else {
        return Ok(None);
    };
    let values: Vec<serde_json::Value> = match serde_json::from_str(source) {
        Ok(values) => values,
        Err(error) => {
            // JSON 解析失败：截断参数写入警告并返回 NULL（非错误）。
            let mut argument = source.chars().take(32).collect::<String>();
            if argument.len() < source.len() {
                argument.push_str("...");
            }
            warnings.push(format!(
                "The argument can't be unmarshalled as JSON array: '{argument}': {error}"
            ));
            return Ok(None);
        }
    };
    let requested = values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let statements = match retriever.retrieve_global(&requested) {
        Ok(statements) => statements,
        // 取消错误向上传播；其它检索失败变警告 + NULL。
        Err(error @ ExpressionError::Cancelled(_)) => return Err(error),
        Err(error) => {
            warnings.push(format!(
                "Retrieving statements information failed with error: {error}"
            ));
            return Ok(None);
        }
    };
    let truncate_length = truncate_length.unwrap_or(0);
    let result = values
        .iter()
        .map(|value| {
            let digest = value.as_str()?;
            let mut statement = statements.get(digest)?.clone();
            // Go uses a byte slice here. If it splits a UTF-8 code point,
            // encoding/json replaces the invalid fragment with U+FFFD.
            if truncate_length > 0 && statement.len() as i64 > truncate_length {
                let requested = truncate_length as usize;
                statement =
                    String::from_utf8_lossy(&statement.as_bytes()[..requested]).into_owned();
                statement.push_str("...");
            }
            (!statement.is_empty()).then_some(statement)
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&result)
        .map(Some)
        .map_err(|error| ExpressionError::External(error.to_string()))
}

/// `TIDB_ENCODE_SQL_DIGEST`：对 SQL 文本做 digest 哈希。
pub fn encode_sql_digest(source: Option<&str>) -> Result<Option<String>, ExpressionError> {
    Ok(source.map(|sql| parser::DigestHash(sql).String().to_owned()))
}

/// 文本 / 二进制执行计划解码注入接口。
pub trait PlanDecoder: Send + Sync {
    fn decode_plan(&self, source: &str) -> Result<String, ExpressionError>;
    fn decode_binary_plan(&self, source: &str) -> Result<String, ExpressionError>;
}

/// 未接线时的占位解码器，始终返回错误。
struct UnavailablePlanDecoder;

impl PlanDecoder for UnavailablePlanDecoder {
    fn decode_plan(&self, _source: &str) -> Result<String, ExpressionError> {
        Err(ExpressionError::External("plan decoder unavailable".into()))
    }

    fn decode_binary_plan(&self, _source: &str) -> Result<String, ExpressionError> {
        Err(ExpressionError::External(
            "binary plan decoder unavailable".into(),
        ))
    }
}

/// Text plan decode errors intentionally return the original input.
///
/// 文本计划解码失败时故意回退为原始输入。
pub fn decode_plan(source: Option<&str>) -> Option<String> {
    decode_plan_with(source, &UnavailablePlanDecoder)
}

/// 使用注入解码器解码文本计划；失败则返回原串。
pub fn decode_plan_with(source: Option<&str>, decoder: &dyn PlanDecoder) -> Option<String> {
    source.map(|source| {
        decoder
            .decode_plan(source)
            .unwrap_or_else(|_| source.to_owned())
    })
}

/// Binary plan decode errors append a warning and return an empty non-NULL value.
///
/// 二进制计划解码失败：追加警告并返回空串（非 NULL）。
pub fn decode_binary_plan(source: Option<&str>, warnings: &mut Vec<String>) -> Option<String> {
    decode_binary_plan_with(source, &UnavailablePlanDecoder, warnings)
}

/// 使用注入解码器解码二进制计划。
pub fn decode_binary_plan_with(
    source: Option<&str>,
    decoder: &dyn PlanDecoder,
    warnings: &mut Vec<String>,
) -> Option<String> {
    source.map(|source| match decoder.decode_binary_plan(source) {
        Ok(plan) => plan,
        Err(error) => {
            warnings.push(error.to_string());
            String::new()
        }
    })
}

/// 序列操作所需权限：NEXTVAL/SETVAL 需 INSERT，LASTVAL 需 SELECT。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequencePrivilege {
    Insert,
    Select,
}

/// 序列目录与取值服务注入接口。
pub trait SequenceService {
    fn sequence_id(&mut self, database: &str, sequence: &str) -> Result<i64, ExpressionError>;
    fn next_value(&mut self, database: &str, sequence: &str) -> Result<i64, ExpressionError>;
    fn set_value(
        &mut self,
        database: &str,
        sequence: &str,
        value: i64,
    ) -> Result<Option<i64>, ExpressionError>;
    fn verify(&self, database: &str, sequence: &str, privilege: SequencePrivilege) -> bool;
}

/// 解析 `db.seq` 或裸 `seq`；后者用当前库补全。
fn resolve_sequence_name(current_db: &str, source: &str) -> (String, String) {
    let (database, sequence) = get_schema_and_sequence(source);
    if database.is_empty() {
        (current_db.to_owned(), sequence)
    } else {
        (database, sequence)
    }
}

/// 构造带操作名与认证用户的序列权限错误。
fn sequence_access_error(
    session: &SessionInfo,
    operation: &'static str,
    sequence: String,
) -> ExpressionError {
    let user = session.user.as_ref();
    ExpressionError::SequenceAccessDenied {
        operation,
        sequence,
        user: user
            .map(|user| user.auth_username.clone())
            .unwrap_or_default(),
        host: user
            .map(|user| user.auth_hostname.clone())
            .unwrap_or_default(),
    }
}

/// `NEXTVAL`：校验 INSERT 权限，推进序列并写入会话状态。
pub fn next_val(
    source: Option<&str>,
    session: &mut SessionInfo,
    service: &mut dyn SequenceService,
) -> Result<Option<i64>, ExpressionError> {
    let Some(source) = source else {
        return Ok(None);
    };
    let (database, sequence) = resolve_sequence_name(&session.current_db, source);
    if !service.verify(&database, &sequence, SequencePrivilege::Insert) {
        return Err(sequence_access_error(session, "INSERT", sequence));
    }
    let id = service.sequence_id(&database, &sequence)?;
    let value = service.next_value(&database, &sequence)?;
    session.sequence_state.insert(id, value);
    Ok(Some(value))
}

/// `LASTVAL`：校验 SELECT 权限，读取会话中该序列最近一次 NEXTVAL。
pub fn last_val(
    source: Option<&str>,
    session: &SessionInfo,
    service: &mut dyn SequenceService,
) -> Result<Option<i64>, ExpressionError> {
    let Some(source) = source else {
        return Ok(None);
    };
    let (database, sequence) = resolve_sequence_name(&session.current_db, source);
    if !service.verify(&database, &sequence, SequencePrivilege::Select) {
        return Err(sequence_access_error(session, "SELECT", sequence));
    }
    let id = service.sequence_id(&database, &sequence)?;
    Ok(session.sequence_state.get(&id).copied())
}

/// `SETVAL`：校验 INSERT 权限后设置序列当前值。
pub fn set_val(
    source: Option<&str>,
    value: Option<i64>,
    session: &SessionInfo,
    service: &mut dyn SequenceService,
) -> Result<Option<i64>, ExpressionError> {
    let (Some(source), Some(value)) = (source, value) else {
        return Ok(None);
    };
    let (database, sequence) = resolve_sequence_name(&session.current_db, source);
    if !service.verify(&database, &sequence, SequencePrivilege::Insert) {
        return Err(sequence_access_error(session, "INSERT", sequence));
    }
    service.set_value(&database, &sequence, value)
}

/// Split on every dot and use the first two components, exactly like Go's
/// strings.Split implementation. The caller fills an empty database name.
///
/// 按 `.` 拆分，取前两段作为库名与序列名；无点时库名留空由调用方补全。
pub fn get_schema_and_sequence(source: &str) -> (String, String) {
    let mut parts = source.split('.');
    let first = parts.next().unwrap_or_default();
    match parts.next() {
        Some(second) => (first.to_owned(), second.to_owned()),
        None => (String::new(), first.to_owned()),
    }
}

const KIB: f64 = (1_u64 << 10) as f64;
const MIB: f64 = (1_u64 << 20) as f64;
const GIB: f64 = (1_u64 << 30) as f64;
const TIB: f64 = (1_u64 << 40) as f64;
const PIB: f64 = (1_u64 << 50) as f64;
const EIB: f64 = (1_u64 << 60) as f64;

/// `FORMAT_BYTES`：按 KiB..EiB 自动选单位格式化字节数。
pub fn format_bytes(value: Option<f64>) -> Option<String> {
    value.map(|bytes| {
        let (divisor, unit) = match bytes.abs() {
            value if value >= EIB => (EIB, "EiB"),
            value if value >= PIB => (PIB, "PiB"),
            value if value >= TIB => (TIB, "TiB"),
            value if value >= GIB => (GIB, "GiB"),
            value if value >= MIB => (MIB, "MiB"),
            value if value >= KIB => (KIB, "KiB"),
            _ => (1.0, "bytes"),
        };
        format_scaled(bytes, divisor, unit)
    })
}

const MICRO: f64 = 1_000.0;
const MILLI: f64 = 1_000.0 * MICRO;
const SECOND: f64 = 1_000.0 * MILLI;
const MINUTE: f64 = 60.0 * SECOND;
const HOUR: f64 = 60.0 * MINUTE;
const DAY: f64 = 24.0 * HOUR;

/// `FORMAT_NANO_TIME`：按 ns..d 自动选单位格式化纳秒时长。
pub fn format_nano_time(value: Option<f64>) -> Option<String> {
    value.map(|nanoseconds| {
        let (divisor, unit) = match nanoseconds.abs() {
            value if value >= DAY => (DAY, "d"),
            value if value >= HOUR => (HOUR, "h"),
            value if value >= MINUTE => (MINUTE, "min"),
            value if value >= SECOND => (SECOND, "s"),
            value if value >= MILLI => (MILLI, "ms"),
            value if value >= MICRO => (MICRO, "us"),
            _ => (1.0, "ns"),
        };
        format_scaled(nanoseconds, divisor, unit)
    })
}

/// 按除数缩放后格式化；极大值改用科学计数法并带符号指数。
fn format_scaled(value: f64, divisor: f64, unit: &str) -> String {
    if divisor == 1.0 {
        return format!("{value:.0} {unit}");
    }
    let scaled = value / divisor;
    if scaled.abs() >= 100_000.0 {
        let raw = format!("{scaled:.2e}");
        let (mantissa, exponent) = raw.split_once('e').expect("scientific format has exponent");
        let exponent = exponent.parse::<i32>().expect("valid exponent");
        format!("{mantissa}e{exponent:+03} {unit}")
    } else {
        format!("{scaled:.2} {unit}")
    }
}
