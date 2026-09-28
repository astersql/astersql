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

// 结果集列名（FieldName）及其切片工具。
//
// 记录列的库/表/列名（含原始名与别名）、隐藏列等元信息，
// 并提供字符串化、内存占用估算与 AST 列名查找。

#[derive(Debug, Default)]
/// 结果集中一列的命名与可见性元信息。
///
/// `Orig*` 为原始表/列名；`DBName`/`TblName`/`ColName` 为解析后（可含别名）的限定名；
/// `Hidden` 表示隐藏列；`NotExplicitUsable`/`Redundant` 标记不可显式引用或冗余列。
pub struct FieldName {
    pub OrigTblName: ast::CIStr,
    pub OrigColName: ast::CIStr,
    pub DBName: ast::CIStr,
    pub TblName: ast::CIStr,
    pub ColName: ast::CIStr,
    pub Hidden: bool,
    pub NotExplicitUsable: bool,
    pub Redundant: bool,
}

/// 隐藏列在 String() 中的占位显示名。
const emptyName: &str = "EMPTY_NAME";

impl FieldName {
    /// 格式化为 `db.tbl.col`；隐藏列返回 EMPTY_NAME。
    pub fn String(&self) -> String {
        if self.Hidden {
            return emptyName.to_owned();
        }
        let mut result = String::with_capacity(
            self.DBName.L.len() + self.TblName.L.len() + self.ColName.L.len() + 2,
        );
        if !self.DBName.L.is_empty() {
            result.push_str(&self.DBName.L);
            result.push('.');
        }
        if !self.TblName.L.is_empty() {
            result.push_str(&self.TblName.L);
            result.push('.');
        }
        result.push_str(&self.ColName.L);
        result
    }

    /// 估算本结构占用的近似内存字节数。
    pub fn MemoryUsage(&self) -> i64 {
        cistr_memory_usage(&self.OrigTblName)
            + cistr_memory_usage(&self.OrigColName)
            + cistr_memory_usage(&self.DBName)
            + cistr_memory_usage(&self.TblName)
            + cistr_memory_usage(&self.ColName)
            + size::SizeOfBool * 3
    }

    /// 深拷贝字段名结构。
    pub fn Clone(&self) -> FieldName {
        FieldName {
            OrigTblName: self.OrigTblName.clone(),
            OrigColName: self.OrigColName.clone(),
            DBName: self.DBName.clone(),
            TblName: self.TblName.clone(),
            ColName: self.ColName.clone(),
            Hidden: self.Hidden,
            NotExplicitUsable: self.NotExplicitUsable,
            Redundant: self.Redundant,
        }
    }
}

/// 估算 CIStr（大小写不敏感字符串）的内存占用。
fn cistr_memory_usage(value: &ast::CIStr) -> i64 {
    size::SizeOfString * 2 + (value.O.len() + value.L.len()) as i64
}

/// FieldName 的可选 Arc 切片，用于结果列名列表。
pub struct NameSlice(pub Vec<Option<std::sync::Arc<FieldName>>>);

impl NameSlice {
    /// 浅拷贝切片（Arc 引用计数 +1）。
    pub fn Shallow(&self) -> NameSlice {
        NameSlice(self.0.clone())
    }

    /// 在切片中查找是否存在匹配的 AST 列名（Schema/Table 可为空通配）。
    pub fn FindAstColName(&self, name: &ast::ColumnName) -> bool {
        self.0.iter().flatten().any(|field_name| {
            (name.Schema.L.is_empty() || name.Schema.L == field_name.DBName.L)
                && (name.Table.L.is_empty() || name.Table.L == field_name.TblName.L)
                && name.Name.L == field_name.ColName.L
        })
    }
}

/// 全局共享的隐藏空列名实例。
pub static EmptyName: std::sync::LazyLock<std::sync::Arc<FieldName>> =
    std::sync::LazyLock::new(|| {
        std::sync::Arc::new(FieldName {
            Hidden: true,
            ..FieldName::default()
        })
    });
