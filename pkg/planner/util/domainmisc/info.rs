// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use std::collections::BTreeMap;

/// 规划器查询最新 schema 时使用的索引元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<String>,
    pub public: bool,
}

/// 提供最新 Schema 版本与按表查询索引的抽象，对应 Go InfoSchema 能力。
pub trait LatestSchema {
    /// 当前 Schema 元数据版本号。
    fn schema_version(&self) -> i64;
    /// 按表 ID 返回索引列表；表不存在时为 `None`。
    ///
    /// Go 的 `InfoSchema.TableByID` 只返回表与存在标志，不包含错误通道。
    fn table_indexes(&self, table_id: i64) -> Option<Vec<IndexInfo>>;
}

/// None means the schema version is unchanged; an empty map means the version
/// changed and the table no longer exists.
///
/// `None` 表示 Schema 版本未变；空 map 表示版本已变但表已不存在。
/// Domain/`schema` 缺失时返回错误字符串。
pub fn get_latest_index_info(
    schema: Option<&dyn LatestSchema>,
    table_id: i64,
    start_version: i64,
) -> Result<(Option<BTreeMap<i64, IndexInfo>>, bool), String> {
    let schema = schema.ok_or_else(|| "domain not found for ctx".to_string())?;
    // 版本未变则跳过刷新，与 Go 返回 (nil, false) 对齐。
    if schema.schema_version() == start_version {
        return Ok((None, false));
    }
    let indexes = schema
        .table_indexes(table_id)
        .unwrap_or_default()
        .into_iter()
        .map(|index| (index.id, index))
        .collect();
    Ok((Some(indexes), true))
}
