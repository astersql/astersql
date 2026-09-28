// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Ingest 错误工具：识别重复键（duplicate key）类 terror，并转换为带表名/索引名的 KeyExists 错误。
//
// 在 DDL 加索引的 ingest 路径中，底层可能以固定错误码上报发现重复键；
// 本模块将其提升为更易读的 `KeyExists` 形态，便于上层向前端返回。

use std::sync::Arc;

/// 发现重复键时的 terror 错误码 ID（与 Go 侧 ERR_FOUND_DUPLICATE_KEYS 对应）。
pub const ERR_FOUND_DUPLICATE_KEYS_ID: u32 = 1001;

/// terror 错误参数：字节、文本或整数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorArgument {
    /// 原始字节参数（通常为 key/value）。
    Bytes(Vec<u8>),
    /// 文本参数。
    Text(String),
    /// 整型参数。
    Integer(i64),
}

/// Ingest 路径上的错误类型集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IngestError {
    /// 带错误码与参数列表的 terror 错误。
    Terror {
        /// 错误码 ID。
        id: u32,
        /// 附加参数。
        arguments: Vec<ErrorArgument>,
    },
    /// 包装错误：保留外层消息并链接到源错误。
    Wrapped {
        /// 外层说明消息。
        message: String,
        /// 被包装的源错误。
        source: Arc<IngestError>,
    },
    /// 键已存在（唯一约束冲突）：携带具体 key/value 与对象名。
    KeyExists {
        /// 冲突的键。
        key: Vec<u8>,
        /// 对应的值。
        value: Vec<u8>,
        /// 索引名。
        index_name: String,
        /// 表名（可含 schema 前缀）。
        table_name: String,
    },
    /// 其它未分类错误消息。
    Other(String),
}

/// 索引元信息（用于填充 KeyExists）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引名称。
    pub name: String,
}

/// 表元信息（用于填充 KeyExists 的表名）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    /// schema（库）名；为空时仅使用表名。
    pub schema: String,
    /// 表名。
    pub name: String,
}

impl IngestError {
    /// 沿 Wrapped 链追溯到根因错误。
    fn root_cause(&self) -> &Self {
        let mut error = self;
        while let Self::Wrapped { source, .. } = error {
            error = source.as_ref();
        }
        error
    }
}

/// 若根因是「发现重复键」terror，则转换为带索引/表名的 `KeyExists`；否则原样返回。
pub fn try_convert_to_key_exists_err(
    origin_error: IngestError,
    index_info: &IndexInfo,
    table_info: &TableInfo,
) -> IngestError {
    // 仅当根因错误码匹配且恰有两个 Bytes 参数时抽取 key/value。
    let (key, value) = match origin_error.root_cause() {
        IngestError::Terror { id, arguments }
            if *id == ERR_FOUND_DUPLICATE_KEYS_ID && arguments.len() == 2 =>
        {
            match (&arguments[0], &arguments[1]) {
                (ErrorArgument::Bytes(key), ErrorArgument::Bytes(value)) => {
                    (key.clone(), value.clone())
                }
                _ => return origin_error,
            }
        }
        _ => return origin_error,
    };
    IngestError::KeyExists {
        key,
        value,
        index_name: index_info.name.clone(),
        // Go ddlutil.GenKeyExistsErr uses tblInfo.Name when forming table.index.
        table_name: table_info.name.clone(),
    }
}
