// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 表/库元数据的 JSON 编解码入口。
//
// 将 [`TableInfo`] / [`DBInfo`] 序列化为字节以便写入元存储，或从字节反序列化回结构体。
// 实际编解码委托给 `ast::metadata_json`，本文件仅提供与 Go 同名的薄封装。

use super::{DBInfo, TableInfo};

/// 将表元数据编码为 JSON 字节；失败时返回错误描述。
pub fn EncodeTableInfo(table: &TableInfo) -> Result<Vec<u8>, String> {
    super::ast::metadata_json::encode(table)
}

/// 从 JSON 字节解码表元数据。
pub fn DecodeTableInfo(encoded: &[u8]) -> Result<TableInfo, String> {
    super::ast::metadata_json::decode(encoded)
}

/// 将库（database）元数据编码为 JSON 字节。
pub fn EncodeDBInfo(database: &DBInfo) -> Result<Vec<u8>, String> {
    super::ast::metadata_json::encode(database)
}

/// 从 JSON 字节解码库元数据。
pub fn DecodeDBInfo(encoded: &[u8]) -> Result<DBInfo, String> {
    super::ast::metadata_json::decode(encoded)
}
