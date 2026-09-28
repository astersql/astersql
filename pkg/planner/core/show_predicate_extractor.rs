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

// SHOW LIKE 谓词提取器，与 Go `show_predicate_extractor.go` 保持同一数据流。

use crate::ast::{ExprKind, ShowStmt, ShowStmtType};
use expression_dependency::collate::{self, WildcardPattern};

const fieldKey: &str = "field";
const tableKey: &str = "table";
const databaseKey: &str = "database";
const collationKey: &str = "collation";
const databaseNameKey: &str = "db_name";

/// SHOW LIKE 谓词提取状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShowBaseExtractor {
    pub ShowStmt: ShowStmt,
    field: String,
    fieldPattern: String,
}

pub fn newShowBaseExtractor(showStatement: ShowStmt) -> ShowBaseExtractor {
    ShowBaseExtractor {
        ShowStmt: showStatement,
        field: String::new(),
        fieldPattern: String::new(),
    }
}

impl ShowBaseExtractor {
    /// 从 SHOW LIKE 或 DESCRIBE COLUMN 中提取可下推字段。
    pub fn Extract(&mut self) -> bool {
        if let Some(pattern) = &self.ShowStmt.Pattern {
            if let ExprKind::Like {
                Pattern, Escape, ..
            } = &pattern.Kind
            {
                match &Pattern.Kind {
                    ExprKind::Value(value) => {
                        let pattern_text = value.text();
                        let escape = Escape.as_bytes().first().copied().unwrap_or(b'\\');
                        let (pattern_value, pattern_types) =
                            stringutil_dependency::string_util::CompilePattern(
                                &pattern_text,
                                escape,
                            );
                        if stringutil_dependency::string_util::IsExactMatch(&pattern_types) {
                            self.field =
                                pattern_value.into_iter().collect::<String>().to_lowercase();
                            return true;
                        }
                        self.fieldPattern = pattern_text.to_lowercase();
                        return true;
                    }
                    // MySQL rejects `SHOW COLUMNS FROM t LIKE abc`.
                    ExprKind::Column(_) => return false,
                    _ => {}
                }
            }
        } else if let Some(column) = &self.ShowStmt.Column
            && !column.Name.L.is_empty()
        {
            self.field = column.Name.L.clone();
            return true;
        }
        false
    }

    /// 生成与 Go 完全一致的 EXPLAIN 文本。
    pub fn ExplainInfo(&self) -> String {
        let key = match self.ShowStmt.Tp {
            ShowStmtType::Variables | ShowStmtType::Columns => fieldKey,
            ShowStmtType::Tables | ShowStmtType::TableStatus => tableKey,
            ShowStmtType::Databases => databaseKey,
            ShowStmtType::Collation => collationKey,
            ShowStmtType::StatsHealthy => databaseNameKey,
            _ => "",
        };
        let mut parts = Vec::with_capacity(2);
        if !self.field.is_empty() {
            parts.push(format!("{key}:[{}]", self.field));
        }
        if !self.fieldPattern.is_empty() {
            parts.push(format!("{key}_pattern:[{}]", self.fieldPattern));
        }
        parts.join(", ")
    }

    /// 返回已提取的精确字段值。
    pub fn Field(&self) -> String {
        self.field.clone()
    }

    /// 按 utf8mb4 默认 collation 编译通配模式；无模式时对应 Go nil。
    pub fn FieldPatternLike(&self) -> Option<Box<dyn WildcardPattern>> {
        if self.fieldPattern.is_empty() {
            return None;
        }
        let collator = collate::GetCollatorByID(collate::CollationName2ID(
            collate::mysql::UTF8MB4DefaultCollation,
        ));
        let mut pattern = collator.Pattern();
        pattern.Compile(&self.fieldPattern, b'\\');
        Some(pattern)
    }
}
