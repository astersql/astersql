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

// Schema/表名与表别名的查找键。
//
// 计划器在绑定、改写阶段用这些键在 HashMap 中索引表元数据。
// CIString（Case-Insensitive String）同时保留原始大小写与小写形式，
// 以兼容 MySQL 风格的大小写不敏感标识符比较。

/// 大小写不敏感字符串：`O` 为原始值，`L` 为小写形式。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct CIString {
    /// 原始大小写字符串。
    pub O: String,
    /// 小写字符串，用于相等比较与哈希。
    pub L: String,
}

impl CIString {
    /// 由任意可转成 String 的值构造，自动生成小写副本。
    pub fn New(value: impl Into<String>) -> Self {
        let original = value.into();
        Self {
            L: original.to_lowercase(),
            O: original,
        }
    }
}

/// 由 schema 名与 table 名组成的物理表查找键。
///
/// 与 Go 的 `schemaTableKey` 一致，键只保存 `CIString::L`，因此原始
/// 标识符大小写不会参与相等比较或哈希。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SchemaTableKey {
    pub schema: String,
    pub table: String,
}

/// 构造 `SchemaTableKey`。
pub fn newSchemaTableKey(schema: CIString, table: CIString) -> SchemaTableKey {
    SchemaTableKey {
        schema: schema.L,
        table: table.L,
    }
}

/// 表别名查找键；可带或不带 schema 限定。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TableAliasKey {
    pub schema: String,
    pub name: String,
    pub qualified: bool,
}

/// 构造仅含表别名、无 schema 的 `TableAliasKey`。
pub fn newTableAliasKey(name: CIString) -> TableAliasKey {
    TableAliasKey {
        schema: String::new(),
        name: name.L,
        qualified: false,
    }
}

/// 构造带 schema 限定的 `TableAliasKey`。
pub fn newQualifiedTableAliasKey(schema: CIString, name: CIString) -> TableAliasKey {
    TableAliasKey {
        schema: schema.L,
        name: name.L,
        qualified: true,
    }
}
