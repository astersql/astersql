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
// Copyright 2026 AsterSQL.

// 管理工具：表/索引行数比对与记录-索引一致性检查。
//
// 对应 Go `util/admin`。通过受限 SQL 统计 COUNT，并扫描记录键校验索引存在性；
// 全局索引使用分区 Handle。TblCntGreater/IdxCntGreater 标明哪侧计数更大。

use std::collections::BTreeMap;

use thiserror::Error;

/// 简化版 Datum，仅覆盖 admin 校验所需的空/整型/字节/字符串。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Datum {
    Null,
    Int(i64),
    Bytes(Vec<u8>),
    String(String),
}

/// 行句柄：普通整型，或全局索引下的「分区 ID + 句柄」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Handle {
    Int(i64),
    Partition { partition_id: i64, handle: i64 },
}

impl Handle {
    /// 取出可比较的整型句柄值（分区变体取内层 handle）。
    pub fn int_value(self) -> i64 {
        match self {
            Self::Int(value) | Self::Partition { handle: value, .. } => value,
        }
    }
}

/// 一条记录的句柄与列值，用于不一致报错。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordData {
    pub Handle: Handle,
    pub Values: Vec<Datum>,
}

/// 列元信息：id、名称、非空约束与建表时默认值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub id: i64,
    pub name: String,
    pub not_null: bool,
    pub origin_default: Option<Datum>,
}

/// 索引元信息；`global` 表示分区表上的全局索引。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexMeta {
    pub name: String,
    pub column_offsets: Vec<usize>,
    pub global: bool,
}

/// KV 扫描得到的键值对。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KvPair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// admin 检查过程中的错误类型。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminError {
    #[error("can not get count, rows count = {0}")]
    InvalidCountRows(usize),
    #[error("table count {table_count} != index({index}) count {index_count}")]
    CountMismatch {
        table_count: i64,
        index: String,
        index_count: i64,
    },
    #[error(
        "Column {column} define as not null, but can't find the value where handle is {handle:?}"
    )]
    MissingNotNullValue { column: String, handle: Handle },
    #[error("record {record:?} is inconsistent with index {index_record:?}")]
    Inconsistent {
        record: RecordData,
        index_record: Option<RecordData>,
    },
    #[error("decode row failed: {0}")]
    Decode(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("SQL execution failed: {0}")]
    Sql(String),
}

/// COUNT(*) 查询返回的单行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountRow(pub i64);

/// 受限 SQL 执行器：在指定快照上跑管理查询。
pub trait RestrictedSqlExecutor {
    fn exec_restricted_sql(
        &self,
        snapshot: u64,
        sql: &str,
        args: &[String],
    ) -> Result<Vec<CountRow>, AdminError>;
}

/// 会话上下文：不可见索引开关、事务/快照时间戳与 SQL 执行器。
pub trait SessionContext {
    fn optimizer_use_invisible_indexes(&self) -> bool;
    fn set_optimizer_use_invisible_indexes(&mut self, enabled: bool);
    fn transaction_start_ts(&self) -> Result<Option<u64>, AdminError>;
    fn snapshot_ts(&self) -> u64;
    fn restricted_sql_executor(&self) -> &dyn RestrictedSqlExecutor;
}

/// KV 检索器：按键范围迭代记录。
pub trait Retriever {
    fn iter(&self, start_key: &[u8], upper_bound: &[u8]) -> Result<Vec<KvPair>, AdminError>;
}

/// 表抽象：列、记录前缀与行解码。
pub trait Table {
    fn name(&self) -> &str;
    fn columns(&self) -> &[Column];
    fn record_prefix(&self) -> Vec<u8>;
    fn decode_row(&self, handle: Handle, value: &[u8]) -> Result<BTreeMap<i64, Datum>, AdminError>;

    /// 从记录键解析分区 ID；非分区表默认不可用。
    fn partition_id_from_key(&self, _key: &[u8]) -> Result<i64, AdminError> {
        Err(AdminError::Decode(
            "partition id is unavailable for this table".into(),
        ))
    }
}

/// 索引查找结果：命中、缺失或发现重复句柄。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexLookup {
    Found,
    Missing,
    Duplicate(Handle),
}

/// 索引抽象：元信息与按列值/句柄探测存在性。
pub trait Index {
    fn meta(&self) -> &IndexMeta;
    fn exists(&self, values: &[Datum], handle: Handle) -> Result<IndexLookup, AdminError>;
}

/// 执行 COUNT SQL，要求恰好一行结果。
fn getCount(
    exec: &dyn RestrictedSqlExecutor,
    snapshot: u64,
    sql: &str,
    args: &[String],
) -> Result<i64, AdminError> {
    let rows = exec.exec_restricted_sql(snapshot, sql, args)?;
    if rows.len() != 1 {
        return Err(AdminError::InvalidCountRows(rows.len()));
    }
    Ok(rows[0].0)
}

/// 表行数大于索引行数。
pub const TblCntGreater: u8 = 1;
/// 索引行数大于表行数。
pub const IdxCntGreater: u8 = 2;

/// 比对表与各索引的 COUNT；临时开启不可见索引，结束后恢复。
/// 返回 (哪侧更大, 出错索引下标, 错误)。
pub fn CheckIndicesCount(
    ctx: &mut dyn SessionContext,
    db_name: &str,
    table_name: &str,
    indices: &[String],
) -> (u8, usize, Result<(), AdminError>) {
    let original_invisible = ctx.optimizer_use_invisible_indexes();
    // 管理检查需看见不可见索引
    ctx.set_optimizer_use_invisible_indexes(true);

    let result = (|| {
        // 优先使用会话显式 snapshot_ts，否则用事务 start_ts
        let mut snapshot = ctx.transaction_start_ts()?.unwrap_or(0);
        if ctx.snapshot_ts() != 0 {
            snapshot = ctx.snapshot_ts();
        }
        let exec = ctx.restricted_sql_executor();
        let table_count = getCount(
            exec,
            snapshot,
            "SELECT COUNT(*) FROM %n.%n USE INDEX()",
            &[db_name.to_owned(), table_name.to_owned()],
        )?;
        for (offset, index) in indices.iter().enumerate() {
            let index_count = match getCount(
                exec,
                snapshot,
                "SELECT COUNT(*) FROM %n.%n USE INDEX(%n)",
                &[db_name.to_owned(), table_name.to_owned(), index.clone()],
            ) {
                Ok(count) => count,
                Err(err) => return Ok((0, offset, Err(err))),
            };
            if table_count == index_count {
                continue;
            }
            let greater = if table_count > index_count {
                TblCntGreater
            } else {
                IdxCntGreater
            };
            return Ok((
                greater,
                offset,
                Err(AdminError::CountMismatch {
                    table_count,
                    index: index.clone(),
                    index_count,
                }),
            ));
        }
        Ok((0, 0, Ok(())))
    })();

    ctx.set_optimizer_use_invisible_indexes(original_invisible);
    match result {
        Ok(result) => result,
        Err(err) => (0, 0, Err(err)),
    }
}

/// 扫描表记录，校验索引存在且与记录一致；处理 NOT NULL 默认值回填。
pub fn CheckRecordAndIndex(
    sess_ctx: &dyn SessionContext,
    txn: &dyn Retriever,
    table: &dyn Table,
    index: &dyn Index,
) -> Result<(), AdminError> {
    let columns: Vec<Column> = index
        .meta()
        .column_offsets
        .iter()
        .map(|offset| table.columns()[*offset].clone())
        .collect();
    let start_key = encode_record_key(&table.record_prefix(), Handle::Int(i64::MIN));
    let is_global = index.meta().global;

    iterRecords(
        sess_ctx,
        txn,
        table,
        &start_key,
        &columns,
        is_global,
        &mut |handle, mut values, columns| {
            // NULL 且有 origin_default 时用默认值参与索引查找
            for (value, column) in values.iter_mut().zip(columns) {
                if *value == Datum::Null {
                    if column.not_null && column.origin_default.is_none() {
                        return Err(AdminError::MissingNotNullValue {
                            column: column.name.clone(),
                            handle,
                        });
                    }
                    if let Some(default) = &column.origin_default {
                        *value = default.clone();
                    }
                }
            }

            match index.exists(&values, handle)? {
                IndexLookup::Found => Ok(true),
                IndexLookup::Missing => Err(AdminError::Inconsistent {
                    record: RecordData {
                        Handle: handle,
                        Values: values,
                    },
                    index_record: None,
                }),
                IndexLookup::Duplicate(other_handle) => {
                    let record = RecordData {
                        Handle: handle,
                        Values: values,
                    };
                    Err(AdminError::Inconsistent {
                        index_record: Some(RecordData {
                            Handle: other_handle,
                            Values: record.Values.clone(),
                        }),
                        record,
                    })
                }
            }
        },
    )
}

/// 行解码器：委托给 Table::decode_row。
pub struct RowDecoder<'a> {
    table: &'a dyn Table,
}

impl RowDecoder<'_> {
    /// 按句柄与 value 字节解码列映射。
    pub fn decode(&self, handle: Handle, value: &[u8]) -> Result<BTreeMap<i64, Datum>, AdminError> {
        self.table.decode_row(handle, value)
    }
}

/// 构造行解码器（Go 侧在此构建 schema 表达式表）。
pub fn makeRowDecoder<'a>(
    table: &'a dyn Table,
    _session_ctx: &dyn SessionContext,
) -> Result<RowDecoder<'a>, AdminError> {
    // In TiDB the schema expression map is built here.  The Table boundary owns
    // the equivalent typed decoder in Rust, avoiding a second schema model.
    Ok(RowDecoder { table })
}

/// 记录迭代回调：返回 false 可提前停止。
pub type RecordIterFunc<'a> =
    dyn FnMut(Handle, Vec<Datum>, &[Column]) -> Result<bool, AdminError> + 'a;

/// 从 start_key 起扫描表记录前缀，对每行调用 callback。
pub fn iterRecords(
    session_ctx: &dyn SessionContext,
    retriever: &dyn Retriever,
    table: &dyn Table,
    start_key: &[u8],
    columns: &[Column],
    is_global_index: bool,
    callback: &mut RecordIterFunc<'_>,
) -> Result<(), AdminError> {
    let prefix = table.record_prefix();
    let upper_bound = prefix_next(&prefix);
    let pairs = retriever.iter(start_key, &upper_bound)?;
    if pairs.is_empty() {
        return Ok(());
    }
    let decoder = makeRowDecoder(table, session_ctx)?;
    let mut position = 0;
    while position < pairs.len() && pairs[position].key.starts_with(&prefix) {
        let pair = &pairs[position];
        let row_prefix = row_key_prefix(&pair.key)?;
        let raw_handle = decode_row_key(&pair.key)?;
        // 全局索引需要分区 ID 组成复合 Handle
        let handle = if is_global_index {
            Handle::Partition {
                partition_id: table.partition_id_from_key(&pair.key)?,
                handle: raw_handle,
            }
        } else {
            Handle::Int(raw_handle)
        };
        let row_map = decoder.decode(handle, &pair.value)?;
        let data = columns
            .iter()
            .map(|column| row_map.get(&column.id).cloned().unwrap_or(Datum::Null))
            .collect();
        if !callback(handle, data, columns)? {
            return Ok(());
        }
        // NextUntil(RowKeyPrefixFilter(rk)): skip the row and any lock/auxiliary
        // KV pairs sharing its record-key prefix.
        position += 1;
        while position < pairs.len() && pairs[position].key.starts_with(&row_prefix) {
            position += 1;
        }
    }
    Ok(())
}

/// 构造记录与索引不一致错误（对应 Go ErrAdminCheckTable）。
pub fn ErrAdminCheckTable(record: RecordData, index_record: Option<RecordData>) -> AdminError {
    AdminError::Inconsistent {
        record,
        index_record,
    }
}

/// 编码记录键：前缀 + 'r' + 可比较有序的句柄（符号位翻转）。
pub fn encode_record_key(prefix: &[u8], handle: Handle) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(b"r");
    // 异或最高位使有符号句柄按无符号字典序排列
    let comparable = (handle.int_value() as u64) ^ (1_u64 << 63);
    key.extend_from_slice(&comparable.to_be_bytes());
    key
}

/// 从记录键末 8 字节解码句柄。
pub fn decode_row_key(key: &[u8]) -> Result<i64, AdminError> {
    if key.len() < 8 {
        return Err(AdminError::Decode(
            "record key is shorter than a handle".into(),
        ));
    }
    let bytes: [u8; 8] = key[key.len() - 8..]
        .try_into()
        .map_err(|_| AdminError::Decode("invalid handle bytes".into()))?;
    Ok((u64::from_be_bytes(bytes) ^ (1_u64 << 63)) as i64)
}

/// 取出整条记录键作为行前缀（用于跳过同前缀辅助 KV）。
fn row_key_prefix(key: &[u8]) -> Result<Vec<u8>, AdminError> {
    if key.len() < 8 {
        return Err(AdminError::Decode("invalid record key".into()));
    }
    Ok(key[..key.len()].to_vec())
}

/// 计算前缀的下一边界，作为扫描上界（不含）。
fn prefix_next(prefix: &[u8]) -> Vec<u8> {
    let mut next = prefix.to_vec();
    for byte in next.iter_mut().rev() {
        if *byte != u8::MAX {
            *byte += 1;
            return next;
        }
        *byte = 0;
    }
    next.push(0);
    next
}
