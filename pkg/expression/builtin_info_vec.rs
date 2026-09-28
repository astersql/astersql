// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 信息类内置函数的向量化求值包装。
//
// 对应 Go `builtin_info_vec.go`：声明哪些信息函数可向量化，并将标量内核结果
// 按批复制为列；BENCHMARK 与带参 LAST_INSERT_ID 有特殊按列语义。

use crate::builtin_ilike_kernel::ExpressionError;
use crate::builtin_info_kernel::{
    KeyCodec, SessionInfo, connection_id, current_resource_group, current_role, current_user,
    database, decode_key, found_rows, last_insert_id, row_count, tidb_is_ddl_owner, tidb_version,
    user, version,
};

/// The same vectorization matrix declared by builtin_info_vec.go.
///
/// 与 Go `builtin_info_vec.go` 一致的信息函数向量化能力枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoBuiltinKind {
    Database,
    ConnectionId,
    TiDBVersion,
    RowCount,
    CurrentUser,
    CurrentResourceGroup,
    CurrentRole,
    User,
    TiDBIsDdlOwner,
    FoundRows,
    Benchmark,
    LastInsertId,
    LastInsertIdWithId,
    Version,
    TiDBMvccInfo,
    TiDBEncodeRecordKey,
    TiDBEncodeIndexKey,
    TiDBDecodeKey,
}

impl InfoBuiltinKind {
    /// MVCC 与 Key 编码类函数不可向量化，其余可向量化。
    pub fn vectorized(self) -> bool {
        !matches!(
            self,
            Self::TiDBMvccInfo | Self::TiDBEncodeRecordKey | Self::TiDBEncodeIndexKey
        )
    }
}

/// 向量化 `DATABASE()`：每行复制同一会话库名。
pub fn vec_database(session: &SessionInfo, rows: usize) -> Vec<Option<String>> {
    vec![database(session); rows]
}

/// 向量化 `CONNECTION_ID()`。
pub fn vec_connection_id(session: &SessionInfo, rows: usize) -> Vec<i64> {
    vec![connection_id(session); rows]
}

/// 向量化 `TIDB_VERSION()`。
pub fn vec_tidb_version(rows: usize) -> Vec<String> {
    vec![tidb_version(); rows]
}

/// 向量化 `ROW_COUNT()`。
pub fn vec_row_count(session: &SessionInfo, rows: usize) -> Vec<i64> {
    vec![row_count(session); rows]
}

/// 向量化 `CURRENT_USER()`；会话缺失时整列失败。
pub fn vec_current_user(
    session: &SessionInfo,
    rows: usize,
) -> Result<Vec<String>, ExpressionError> {
    Ok(vec![current_user(session)?; rows])
}

/// 向量化当前资源组名。
pub fn vec_current_resource_group(session: &SessionInfo, rows: usize) -> Vec<String> {
    vec![current_resource_group(session); rows]
}

/// 向量化 `CURRENT_ROLE()`。
pub fn vec_current_role(
    session: &SessionInfo,
    rows: usize,
) -> Result<Vec<String>, ExpressionError> {
    Ok(vec![current_role(session)?; rows])
}

/// 向量化 `USER()`。
pub fn vec_user(session: &SessionInfo, rows: usize) -> Result<Vec<String>, ExpressionError> {
    Ok(vec![user(session)?; rows])
}

/// 向量化 `TIDB_IS_DDL_OWNER()`。
pub fn vec_tidb_is_ddl_owner(is_owner: bool, rows: usize) -> Vec<i64> {
    vec![tidb_is_ddl_owner(is_owner); rows]
}

/// 向量化 `FOUND_ROWS()`。
pub fn vec_found_rows(session: &SessionInfo, rows: usize) -> Vec<i64> {
    vec![found_rows(session); rows]
}

/// Vector BENCHMARK evaluates its child vector once per loop, then returns a
/// zero-filled non-NULL integer column, even when child rows are NULL.
///
/// 向量 BENCHMARK：每轮整列求值子表达式，返回全 0 非 NULL 列；要求正常量循环次数。
pub fn vec_benchmark(
    rows: usize,
    loop_count: i64,
    mut evaluate_child_vector: impl FnMut() -> Result<(), ExpressionError>,
) -> Result<Vec<i64>, ExpressionError> {
    if loop_count <= 0 {
        return Err(ExpressionError::InvalidArgument(
            "vector BENCHMARK requires a positive constant loop count".into(),
        ));
    }
    for _ in 0..loop_count {
        evaluate_child_vector()?;
    }
    Ok(vec![0; rows])
}

/// 向量化无参 `LAST_INSERT_ID()`。
pub fn vec_last_insert_id(session: &SessionInfo, rows: usize) -> Vec<i64> {
    vec![last_insert_id(session); rows]
}

/// Go scans the evaluated argument column backwards and records the last
/// non-NULL value while returning the argument column unchanged.
///
/// 带参向量 LAST_INSERT_ID：自后向前取最后一个非 NULL 写入会话，返回列本身不变。
pub fn vec_last_insert_id_with_id(
    session: &mut SessionInfo,
    values: Vec<Option<i64>>,
) -> Vec<Option<i64>> {
    if let Some(value) = values.iter().rev().flatten().next() {
        session.last_insert_id = *value as u64;
    }
    values
}

/// 向量化 `VERSION()`。
pub fn vec_version(rows: usize) -> Vec<String> {
    vec![version(); rows]
}

/// 向量化 `TIDB_DECODE_KEY`：逐行调用标量解码。
pub fn vec_decode_key(
    values: &[Option<String>],
    codec: Option<&dyn KeyCodec>,
) -> Result<Vec<Option<String>>, ExpressionError> {
    values
        .iter()
        .map(|value| decode_key(value.as_deref(), codec))
        .collect()
}
