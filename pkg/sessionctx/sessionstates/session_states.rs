// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 会话状态快照：会话在节点间迁移时需要跟随的变量、预处理语句与上下文信息。
//
// 对应 Go `session_states.go`。`SessionStates` 聚合用户变量、系统变量、
// prepared statement、SQL binding、DDL/查询元信息、假设索引等；
// 具体 TiDB 类型（`Datum`、`FieldType`、`SQLWarn`、`IndexInfo`）保持与依赖 crate 一致，
// 并在本 crate 组合各类型的 Go 兼容 JSON 编解码。

#![allow(non_snake_case, non_upper_case_globals)]

use base64::Engine;
use contextutil::SQLWarn;
use dbterror::{dbterror as error_class, errors as tidb_errors};
use model::group_4::IndexInfo;
use parser_types::types::FieldType;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::sync::LazyLock;
use types::datum::Datum;

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// SessionStateType is the type of session state handlers.
/// 会话状态处理器类型（区分预处理语句、SQL binding 等处理分支）。
pub type SessionStateType = i32;

/// StatePrepareStmt represents prepared statements.
/// 预处理语句状态类型常量。
pub const StatePrepareStmt: SessionStateType = 0;
/// StateBinding represents session SQL bindings.
/// 会话级 SQL Binding 状态类型常量。
pub const StateBinding: SessionStateType = 1;

/// ErrCannotMigrateSession indicates that the current session cannot migrate.
/// 当前会话不可迁移时的标准错误（如活跃事务、临时表、未关闭游标等）。
pub static ErrCannotMigrateSession: LazyLock<Box<dbterror::terror::Error>> =
    LazyLock::new(|| error_class::ClassSession.NewStd(errno::errcode::ErrCannotMigrateSession));

/// 会话状态编解码或迁移过程中的错误。
#[derive(Debug, thiserror::Error)]
pub enum SessionStateError {
    /// JSON 序列化/反序列化失败。
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// 会话不可迁移（附带 TiDB SharedError）。
    #[error("{0}")]
    CannotMigrate(tidb_errors::SharedError),
}

impl SessionStateError {
    /// 返回对应的 SQL errno；JSON 错误无固定码时返回 0。
    pub fn code(&self) -> u16 {
        match self {
            Self::CannotMigrate(_) => errno::errcode::ErrCannotMigrateSession,
            Self::Json(_) => 0,
        }
    }

    /// 以给定原因构造 `CannotMigrate` 错误（堆栈参数来自标准 ErrCannotMigrateSession）。
    pub(crate) fn cannot_migrate(reason: impl Into<String>) -> Self {
        Self::CannotMigrate(ErrCannotMigrateSession.GenWithStackByArgs(&[reason.into().into()]))
    }
}

/// 与 Go `encoding/json` 对 `[]byte` 的处理一致：序列化为 base64 字符串。
mod go_bytes {
    use super::*;

    /// 将字节切片编码为标准 base64 字符串再序列化。
    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(value))
    }

    /// 从 base64 字符串反序列化还原字节向量。
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(encoded) = Option::<String>::deserialize(deserializer)? else {
            return Ok(Vec::new());
        };
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)
    }
}

/// PreparedStmtInfo contains information about text and binary prepared statements.
/// 文本/二进制协议预处理语句的可迁移信息（名称、SQL 文本、库名、参数类型字节）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedStmtInfo {
    #[serde(
        rename = "name",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    /// 语句名称（文本协议 `PREPARE name FROM ...`）；空则 omitempty。
    pub Name: String,
    #[serde(rename = "text", default, deserialize_with = "null_default")]
    /// 预处理 SQL 文本。
    pub StmtText: String,
    #[serde(
        rename = "db",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    /// 预处理时所在数据库；空则 omitempty。
    pub StmtDB: String,
    #[serde(
        rename = "types",
        default,
        with = "go_bytes",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// 参数类型编码（Go `[]byte`，JSON 中为 base64）。
    pub ParamTypes: Vec<u8>,
}

/// QueryInfo is the information of the last executed query exposed for tests.
/// 最近一次查询的元信息（事务作用域、起止时间戳、RU 消耗与错误消息），供测试/诊断读取。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryInfo {
    #[serde(rename = "txn_scope", default, deserialize_with = "null_default")]
    /// 事务作用域（如 global）。
    pub TxnScope: String,
    #[serde(rename = "start_ts", default, deserialize_with = "null_default")]
    /// 查询起始时间戳（start_ts）。
    pub StartTS: u64,
    #[serde(rename = "for_update_ts", default, deserialize_with = "null_default")]
    /// 悲观锁/FOR UPDATE 相关时间戳。
    pub ForUpdateTS: u64,
    #[serde(rename = "ru_consumption", default, deserialize_with = "null_default")]
    /// 资源单元（RU）消耗量。
    pub RUConsumption: f64,
    #[serde(
        rename = "error",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    /// 错误消息；空则 omitempty。
    pub ErrMsg: String,
}

/// LastDDLInfo is the information of the last DDL exposed for tests.
/// 最近一次 DDL 的查询文本与序号，供测试/诊断读取。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastDDLInfo {
    #[serde(rename = "query", default, deserialize_with = "null_default")]
    /// DDL SQL 文本。
    pub Query: String,
    #[serde(rename = "seq_num", default, deserialize_with = "null_default")]
    /// DDL 序号（seq_num）。
    pub SeqNum: u64,
}

/// All state which follows a session when it migrates to another server.
///
/// The concrete TiDB types are intentionally retained: `Datum`, `FieldType`, `SQLWarn`,
/// and `IndexInfo` come from the already migrated dependency crates rather than local
/// stand-ins. This type composes their Go-compatible JSON implementations directly.
///
/// 会话迁移到另一节点时需跟随的全部状态快照。
/// 保留具体 TiDB 类型而非本地替身，并直接组合各依赖类型的 Go 兼容 JSON。
#[derive(Clone, Default)]
pub struct SessionStates {
    /// 用户变量名到 Datum 值的映射。
    pub UserVars: HashMap<String, Box<Datum>>,
    /// 用户变量名到类型（FieldType）的映射。
    pub UserVarTypes: HashMap<String, Box<FieldType>>,
    /// 需要随会话迁移的系统变量名到字符串值。
    pub SystemVars: HashMap<String, String>,
    /// 预处理语句 ID 到语句信息。
    pub PreparedStmts: HashMap<u32, Box<PreparedStmtInfo>>,
    /// 下一个预处理语句 ID（分配计数器）。
    pub PreparedStmtID: u32,
    /// 会话状态位（如 autocommit 等 Status 标志）。
    pub Status: u32,
    /// 当前数据库名。
    pub CurrentDB: String,
    /// 最近事务信息字符串（tidb_last_txn_info）。
    pub LastTxnInfo: String,
    /// 最近查询信息。
    pub LastQueryInfo: Option<Box<QueryInfo>>,
    /// 最近 DDL 信息。
    pub LastDDLInfo: Option<Box<LastDDLInfo>>,
    /// FOUND_ROWS() 相关行数。
    pub LastFoundRows: u64,
    /// 上一语句是否命中计划缓存（plan cache）。
    pub FoundInPlanCache: bool,
    /// 上一语句是否命中 SQL Binding。
    pub FoundInBinding: bool,
    /// Sequence 最新值映射（sequence id → last value）。
    pub SequenceLatestValues: HashMap<i64, i64>,
    /// 最近 affected rows。
    pub LastAffectedRows: i64,
    /// 最近 LAST_INSERT_ID。
    pub LastInsertID: u64,
    /// 警告/错误列表（SQLWarn）。
    pub Warnings: Vec<SQLWarn>,
    /// 会话级 SQL Binding 的序列化字符串。
    pub Bindings: String,
    /// 当前资源组名称。
    pub ResourceGroupName: String,
    /// 假设索引（hypo index）：库 → 表 → 索引名 → IndexInfo。
    pub HypoIndexes: HashMap<String, HashMap<String, HashMap<String, Box<IndexInfo>>>>,
    /// 假设 TiFlash 副本：库 → 表 → 占位。
    pub HypoTiFlashReplicas: HashMap<String, HashMap<String, ()>>,
}

fn json_value<T: Serialize>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn take_json_or_default<T>(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Default,
{
    object.remove(key).map_or_else(
        || Ok(T::default()),
        |value| {
            if value.is_null() {
                Ok(T::default())
            } else {
                serde_json::from_value(value).map_err(|error| error.to_string())
            }
        },
    )
}

fn datum_map_to_json(values: &HashMap<String, Box<Datum>>) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::with_capacity(values.len());
    for (name, datum) in values {
        let encoded = datum.MarshalJSON().map_err(|error| error.to_string())?;
        let value = serde_json::from_slice(&encoded).map_err(|error| error.to_string())?;
        object.insert(name.clone(), value);
    }
    Ok(serde_json::Value::Object(object))
}

fn datum_map_from_json(
    value: Option<serde_json::Value>,
) -> Result<HashMap<String, Box<Datum>>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(HashMap::new());
    };
    let serde_json::Value::Object(object) = value else {
        return Err("user-var-values must be a JSON object".to_owned());
    };
    let mut values = HashMap::with_capacity(object.len());
    for (name, value) in object {
        let mut datum = Datum::default();
        let encoded = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
        datum
            .UnmarshalJSON(&encoded)
            .map_err(|error| error.to_string())?;
        values.insert(name, Box::new(datum));
    }
    Ok(values)
}

fn warnings_to_json(warnings: &[SQLWarn]) -> Result<serde_json::Value, String> {
    let mut values = Vec::with_capacity(warnings.len());
    for warning in warnings {
        let encoded = warning.MarshalJSON().map_err(|error| error.to_string())?;
        values.push(serde_json::from_slice(&encoded).map_err(|error| error.to_string())?);
    }
    Ok(serde_json::Value::Array(values))
}

fn warnings_from_json(value: Option<serde_json::Value>) -> Result<Vec<SQLWarn>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let serde_json::Value::Array(values) = value else {
        return Err("warnings must be a JSON array".to_owned());
    };
    let mut warnings = Vec::with_capacity(values.len());
    for value in values {
        let mut warning = SQLWarn {
            Level: String::new(),
            Err: None,
        };
        let encoded = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
        warning
            .UnmarshalJSON(&encoded)
            .map_err(|error| error.to_string())?;
        warnings.push(warning);
    }
    Ok(warnings)
}

fn tiflash_replicas_to_json(replicas: &HashMap<String, HashMap<String, ()>>) -> serde_json::Value {
    let databases = replicas
        .iter()
        .map(|(database, tables)| {
            let tables = tables
                .keys()
                .map(|table| (table.clone(), serde_json::json!({})))
                .collect();
            (database.clone(), serde_json::Value::Object(tables))
        })
        .collect();
    serde_json::Value::Object(databases)
}

fn tiflash_replicas_from_json(
    value: Option<serde_json::Value>,
) -> Result<HashMap<String, HashMap<String, ()>>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(HashMap::new());
    };
    let serde_json::Value::Object(databases) = value else {
        return Err("hypo-tiflash-replicas must be a JSON object".to_owned());
    };
    databases
        .into_iter()
        .map(|(database, value)| {
            let serde_json::Value::Object(tables) = value else {
                return Err(format!(
                    "hypo-tiflash-replicas database {database:?} must contain an object"
                ));
            };
            Ok((
                database,
                tables.into_iter().map(|(table, _)| (table, ())).collect(),
            ))
        })
        .collect()
}

impl Serialize for SessionStates {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut object = serde_json::Map::new();
        macro_rules! insert_nonempty {
            ($key:literal, $value:expr) => {
                if !$value.is_empty() {
                    object.insert(
                        $key.to_owned(),
                        json_value(&$value).map_err(serde::ser::Error::custom)?,
                    );
                }
            };
        }
        macro_rules! insert_nonzero {
            ($key:literal, $value:expr) => {
                if $value != 0 {
                    object.insert($key.to_owned(), serde_json::Value::from($value));
                }
            };
        }

        if !self.UserVars.is_empty() {
            object.insert(
                "user-var-values".to_owned(),
                datum_map_to_json(&self.UserVars).map_err(serde::ser::Error::custom)?,
            );
        }
        insert_nonempty!("user-var-types", self.UserVarTypes);
        insert_nonempty!("sys-vars", self.SystemVars);
        insert_nonempty!("prepared-stmts", self.PreparedStmts);
        insert_nonzero!("prepared-stmt-id", self.PreparedStmtID);
        insert_nonzero!("status", self.Status);
        insert_nonempty!("current-db", self.CurrentDB);
        insert_nonempty!("txn-info", self.LastTxnInfo);
        if let Some(value) = &self.LastQueryInfo {
            object.insert(
                "query-info".to_owned(),
                json_value(value).map_err(serde::ser::Error::custom)?,
            );
        }
        if let Some(value) = &self.LastDDLInfo {
            object.insert(
                "ddl-info".to_owned(),
                json_value(value).map_err(serde::ser::Error::custom)?,
            );
        }
        insert_nonzero!("found-rows", self.LastFoundRows);
        if self.FoundInPlanCache {
            object.insert("in-plan-cache".to_owned(), serde_json::Value::Bool(true));
        }
        if self.FoundInBinding {
            object.insert("in-binding".to_owned(), serde_json::Value::Bool(true));
        }
        insert_nonempty!("seq-values", self.SequenceLatestValues);
        insert_nonzero!("affected-rows", self.LastAffectedRows);
        insert_nonzero!("last-insert-id", self.LastInsertID);
        if !self.Warnings.is_empty() {
            object.insert(
                "warnings".to_owned(),
                warnings_to_json(&self.Warnings).map_err(serde::ser::Error::custom)?,
            );
        }
        insert_nonempty!("bindings", self.Bindings);
        insert_nonempty!("rs-group", self.ResourceGroupName);
        insert_nonempty!("hypo-indexes", self.HypoIndexes);
        if !self.HypoTiFlashReplicas.is_empty() {
            object.insert(
                "hypo-tiflash-replicas".to_owned(),
                tiflash_replicas_to_json(&self.HypoTiFlashReplicas),
            );
        }
        serde_json::Value::Object(object).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SessionStates {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let serde_json::Value::Object(mut object) = serde_json::Value::deserialize(deserializer)?
        else {
            return Err(serde::de::Error::custom(
                "session states must be a JSON object",
            ));
        };

        Ok(Self {
            UserVars: datum_map_from_json(object.remove("user-var-values"))
                .map_err(serde::de::Error::custom)?,
            UserVarTypes: take_json_or_default(&mut object, "user-var-types")
                .map_err(serde::de::Error::custom)?,
            SystemVars: take_json_or_default(&mut object, "sys-vars")
                .map_err(serde::de::Error::custom)?,
            PreparedStmts: take_json_or_default(&mut object, "prepared-stmts")
                .map_err(serde::de::Error::custom)?,
            PreparedStmtID: take_json_or_default(&mut object, "prepared-stmt-id")
                .map_err(serde::de::Error::custom)?,
            Status: take_json_or_default(&mut object, "status")
                .map_err(serde::de::Error::custom)?,
            CurrentDB: take_json_or_default(&mut object, "current-db")
                .map_err(serde::de::Error::custom)?,
            LastTxnInfo: take_json_or_default(&mut object, "txn-info")
                .map_err(serde::de::Error::custom)?,
            LastQueryInfo: take_json_or_default(&mut object, "query-info")
                .map_err(serde::de::Error::custom)?,
            LastDDLInfo: take_json_or_default(&mut object, "ddl-info")
                .map_err(serde::de::Error::custom)?,
            LastFoundRows: take_json_or_default(&mut object, "found-rows")
                .map_err(serde::de::Error::custom)?,
            FoundInPlanCache: take_json_or_default(&mut object, "in-plan-cache")
                .map_err(serde::de::Error::custom)?,
            FoundInBinding: take_json_or_default(&mut object, "in-binding")
                .map_err(serde::de::Error::custom)?,
            SequenceLatestValues: take_json_or_default(&mut object, "seq-values")
                .map_err(serde::de::Error::custom)?,
            LastAffectedRows: take_json_or_default(&mut object, "affected-rows")
                .map_err(serde::de::Error::custom)?,
            LastInsertID: take_json_or_default(&mut object, "last-insert-id")
                .map_err(serde::de::Error::custom)?,
            Warnings: warnings_from_json(object.remove("warnings"))
                .map_err(serde::de::Error::custom)?,
            Bindings: take_json_or_default(&mut object, "bindings")
                .map_err(serde::de::Error::custom)?,
            ResourceGroupName: take_json_or_default(&mut object, "rs-group")
                .map_err(serde::de::Error::custom)?,
            HypoIndexes: take_json_or_default(&mut object, "hypo-indexes")
                .map_err(serde::de::Error::custom)?,
            HypoTiFlashReplicas: tiflash_replicas_from_json(object.remove("hypo-tiflash-replicas"))
                .map_err(serde::de::Error::custom)?,
        })
    }
}
