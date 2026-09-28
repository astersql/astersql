// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 查询结果行集与断言辅助。
//
// 将 SQL 查询结果规范化为 `Vec<Vec<String>>`，并提供与 Go TestKit 对齐的
// `Check` / `CheckAt` / `CheckContain` 等断言，以及按分隔符构造期望行的
// [`Rows`] / [`RowsWithSep`] 便捷函数。

use std::fmt::Display;

/// 一次查询的结果：行数据与可选注释（失败断言时附加到消息）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Result {
    rows: Vec<Vec<String>>,
    comment: String,
}

impl Result {
    /// 由行集构造结果，注释为空。
    pub fn new(rows: Vec<Vec<String>>) -> Self {
        Self {
            rows,
            comment: String::new(),
        }
    }

    /// 由行集与注释构造结果。
    pub fn with_comment(rows: Vec<Vec<String>>, comment: impl Into<String>) -> Self {
        Self {
            rows,
            comment: comment.into(),
        }
    }

    /// 断言全部行与期望完全相等（期望元素先 `Display` 成字符串）。
    pub fn Check<T: Display>(&self, expected: Vec<Vec<T>>) {
        assert_eq!(
            render_rows(&expected),
            render_rows(&self.rows),
            "{}",
            self.comment
        );
    }

    /// 比较是否与期望行集相等，不 panic。
    pub fn Equal<T: Display>(&self, expected: Vec<Vec<T>>) -> bool {
        render_rows(&expected) == render_rows(&self.rows)
    }

    /// 追加断言失败时展示的注释行。
    pub fn AddComment(&mut self, comment: &str) {
        // Go always appends a newline before every additional comment,
        // including the first one.
        self.comment.push('\n');
        self.comment.push_str(comment);
    }

    /// 按自定义比较函数逐行断言；行数须先一致。
    pub fn CheckWithFunc<T, F>(&self, expected: Vec<Vec<T>>, compare: F)
    where
        F: Fn(&[String], &[T]) -> bool,
    {
        assert_eq!(
            expected.len(),
            self.rows.len(),
            "{}: result length mismatch",
            self.comment
        );
        for (actual, expected) in self.rows.iter().zip(expected.iter()) {
            assert!(
                compare(actual, expected),
                "{}: actual={actual:?}",
                self.comment
            );
        }
    }

    /// 就地按行字典序排序，便于忽略返回顺序的断言。
    pub fn Sort(&mut self) -> &mut Self {
        self.rows.sort();
        self
    }

    /// 克隆返回内部行集。
    pub fn Rows(&self) -> Vec<Vec<String>> {
        self.rows.clone()
    }

    /// 仅投影指定列下标后与期望比较。
    pub fn CheckAt<T: Display>(&self, columns: &[usize], expected: Vec<Vec<T>>) {
        for row in &expected {
            assert_eq!(
                row.len(),
                columns.len(),
                "{}: expected row has {} columns, selected {}",
                self.comment,
                row.len(),
                columns.len()
            );
        }
        // 按列下标投影每一行，再与期望做全等比较。
        let projected = self
            .rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| {
                        row.get(*column)
                            .unwrap_or_else(|| panic!("column {column} out of range"))
                            .clone()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            render_rows(&expected),
            render_rows(&projected),
            "{}",
            self.comment
        );
    }

    /// 断言任意单元格包含给定子串。
    pub fn CheckContain(&self, expected: &str) {
        assert!(
            self.rows
                .iter()
                .flatten()
                .any(|value| value.contains(expected)),
            "{}: result does not contain {expected:?}\n{}",
            self.comment,
            self.String()
        );
    }

    /// 断言任意单元格都不包含给定子串。
    pub fn CheckNotContain(&self, unexpected: &str) {
        assert!(
            !self
                .rows
                .iter()
                .flatten()
                .any(|value| value.contains(unexpected)),
            "{}: result contains {unexpected:?}\n{}",
            self.comment,
            self.String()
        );
    }

    /// 对多个子串依次调用 [`CheckContain`]。
    pub fn MultiCheckContain(&self, expected: &[String]) {
        let result = self.String();
        for value in expected {
            assert!(
                result.contains(value),
                "{}: result does not contain {value:?}\n{result}",
                self.comment
            );
        }
    }

    /// 对多个子串依次调用 [`CheckNotContain`]。
    pub fn MultiCheckNotContain(&self, unexpected: &[String]) {
        let result = self.String();
        for value in unexpected {
            assert!(
                !result.contains(value),
                "{}: result contains {value:?}\n{result}",
                self.comment
            );
        }
    }

    /// 将结果格式化为「行内空格分隔、行间换行」的可读字符串。
    pub fn String(&self) -> String {
        self.rows
            .iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 返回行数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    /// 是否无行。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Match Go TestKit's `fmt.Fprintf(buffer, "%s\n", row)` comparison.
///
/// Go deliberately compares each row's rendered text, so a plan-tree row split
/// into four protocol columns is equivalent to the same text split by `Rows`
/// at every space. Preserve row boundaries while ignoring cell boundaries.
fn render_rows<T: Display>(rows: &[Vec<T>]) -> String {
    let mut rendered = String::new();
    for row in rows {
        rendered.push_str(
            &row.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        );
        rendered.push('\n');
    }
    rendered
}

/// 以空格分隔每行字符串，构造期望行集（Go `Rows` 风格）。
pub fn Rows(rows: &[&str]) -> Vec<Vec<String>> {
    RowsWithSep(" ", rows)
}

/// 以指定分隔符拆分每行字符串，构造期望行集。
pub fn RowsWithSep(separator: &str, rows: &[&str]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            if separator.is_empty() {
                row.chars().map(|value| value.to_string()).collect()
            } else {
                row.split(separator).map(str::to_owned).collect()
            }
        })
        .collect()
}
