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

// DDL 索引回填的 Coprocessor（协处理器）扫描辅助模块。
//
// 在分布式数据库中，"Coprocessor" 指下推到存储节点（TiKV）执行的计算逻辑，
// 用于就近扫描数据、减少网络传输。本模块模拟了添加索引（ADD INDEX）时
// 通过 Coprocessor 读取表数据的核心流程：按 Key 范围做表扫描、分批消费
// 扫描结果、从行数据中按列偏移提取 Datum、构造行句柄（Handle），以及
// 在事务（begin/rollback）包装下执行只读操作。

use crate::backfilling::Key;

/// 数据库中的基本数据单元（Datum），表示一列的一个具体取值。
///
/// 与 TiDB 中的 `types.Datum` 对应，这里仅保留索引回填所需的几种类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Datum {
    /// SQL NULL 值。
    Null,
    /// 有符号 64 位整数。
    Int(i64),
    /// 无符号 64 位整数。
    UInt(u64),
    /// 原始字节串（如 BLOB / 编码后的键值）。
    Bytes(Vec<u8>),
    /// UTF-8 文本字符串。
    Text(String),
}
/// 表扫描返回的一行数据：行键加上各列的取值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanRow {
    /// 该行在存储引擎中的编码键（row key）。
    pub key: Key,
    /// 该行各列的值，顺序与表列定义一致。
    pub columns: Vec<Datum>,
}
/// 行句柄（Handle）：唯一标识表中一行的逻辑主键。
///
/// 整数句柄对应整型主键（或隐藏的 `_tidb_rowid`）；
/// Common 句柄对应聚簇索引表的多列主键，以编码字节序列表示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Handle {
    /// 整数句柄。
    Int(i64),
    /// 多列（common handle）编码后的字节句柄。
    Common(Vec<u8>),
}
/// Coprocessor 扫描过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CopError {
    /// 扫描范围非法（起始键不小于结束键，或批大小为 0）。
    InvalidRange,
    /// 列偏移越界，携带出错的偏移值。
    ColumnOffset(usize),
    /// 无法从给定 Datum 构造行句柄。
    InvalidHandle,
    /// 事务操作（begin/rollback）失败，携带底层错误信息。
    Transaction(String),
    /// 唯一索引冲突（重复键），携带索引名。
    DuplicateKey(String),
}

/// 在 begin/rollback 事务包装中执行只读操作。
///
/// 索引回填读取快照数据时只需只读事务：先以 `start_ts`（事务开始时间戳，
/// 用于 MVCC 多版本读取）开启事务，执行 `operation` 后无论成败都回滚，
/// 保证不留下任何写入痕迹。与 Go 的 `defer se.Rollback()` 一致，回滚错误
/// 不覆盖操作结果。
pub fn wrap_in_begin_rollback<T>(
    start_ts: u64,
    begin: impl FnOnce() -> Result<(), String>,
    operation: impl FnOnce(u64) -> Result<T, CopError>,
    rollback: impl FnOnce() -> Result<(), String>,
) -> Result<T, CopError> {
    begin().map_err(CopError::Transaction)?;
    let result = operation(start_ts);
    // Go 在 defer 中忽略 Rollback 的返回值；这里也只保证清理被调用。
    let _ = rollback();
    result
}
/// 构建表扫描结果：返回落在 `[start, end)` 键范围内且满足可选过滤
/// 条件（selection，对应下推的 WHERE 谓词）的行，以及是否使用了过滤。
pub fn build_table_scan(
    rows: &[ScanRow],
    start: &[u8],
    end: &[u8],
    selection: Option<&dyn Fn(&ScanRow) -> bool>,
) -> Result<(Vec<ScanRow>, bool), CopError> {
    // 扫描范围必须是左闭右开的有效区间。
    if start >= end {
        return Err(CopError::InvalidRange);
    }
    Ok((
        rows.iter()
            // 只保留键在范围内且通过谓词过滤的行。
            .filter(|row| {
                row.key.as_slice() >= start
                    && row.key.as_slice() < end
                    && selection.is_none_or(|select| select(row))
            })
            .cloned()
            .collect(),
        selection.is_some(),
    ))
}
/// 按 `batch_size` 分批消费表扫描结果。
///
/// 索引回填按批处理数据以控制内存与事务大小；批大小为 0 视为非法范围。
pub fn fetch_table_scan_result(
    rows: &[ScanRow],
    batch_size: usize,
    mut consume: impl FnMut(&[ScanRow]) -> Result<(), CopError>,
) -> Result<(), CopError> {
    if batch_size == 0 {
        return Err(CopError::InvalidRange);
    }
    for batch in rows.chunks(batch_size) {
        consume(batch)?;
    }
    Ok(())
}
/// 补全错误信息：将重复键错误中的占位内容替换为具体的索引名，
/// 便于向用户报告是哪个唯一索引发生冲突；其他错误原样返回。
pub fn complete_error(error: CopError, index_name: &str) -> CopError {
    match error {
        CopError::DuplicateKey(_) => CopError::DuplicateKey(index_name.to_owned()),
        other => other,
    }
}
/// 按列偏移从扫描行中提取 Datum 列表。
///
/// `offsets` 是索引列在表列中的下标；`buffer` 为可复用的临时缓冲区，
/// 用于减少分配。偏移越界时返回 [`CopError::ColumnOffset`]。
pub fn extract_datum_by_offsets(
    row: &ScanRow,
    offsets: &[usize],
    buffer: &mut Vec<Datum>,
) -> Result<Vec<Datum>, CopError> {
    buffer.clear();
    for offset in offsets {
        buffer.push(
            row.columns
                .get(*offset)
                .cloned()
                .ok_or(CopError::ColumnOffset(*offset))?,
        );
    }
    Ok(buffer.clone())
}
/// 由主键列的 Datum 构造行句柄。
///
/// `common` 为 false 时要求首个 Datum 是整数，构造整数句柄；
/// 为 true 时把所有 Datum 编码成"长度前缀 + 内容"的字节序列，
/// 构造多列（common handle）句柄。
pub fn build_handle(datums: &[Datum], common: bool) -> Result<Handle, CopError> {
    if !common {
        return match datums.first() {
            Some(Datum::Int(value)) => Ok(Handle::Int(*value)),
            _ => Err(CopError::InvalidHandle),
        };
    }
    let mut output = Vec::new();
    // 逐个编码：先写入 8 字节大端长度，再写入调试格式的内容字节。
    for datum in datums {
        let text = format!("{datum:?}");
        output.extend_from_slice(&(text.len() as u64).to_be_bytes());
        output.extend_from_slice(text.as_bytes());
    }
    Ok(Handle::Common(output))
}
/// 获取索引项的还原数据（restore data）。
///
/// 还原数据用于从索引记录中恢复原始列值（如保留新排序规则下的原文）。
/// 聚簇索引表（`common` 为 true）还需附加主键列的值。
pub fn get_restore_data(_target: &[Datum], primary: &[Datum], common: bool) -> Vec<Datum> {
    if !common {
        return Vec::new();
    }
    // Go 用 Null 标记无需恢复的主键列，并原地压缩出其余列。
    primary
        .iter()
        .filter(|datum| !matches!(datum, Datum::Null))
        .cloned()
        .collect()
}
