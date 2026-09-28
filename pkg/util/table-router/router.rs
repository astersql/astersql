// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 表路由（table router）：按 schema/table 通配规则映射到目标库表。
//
// 对应 Go `pkg/util/table-router`。内部用 trie 选择器（`Selector`）匹配规则；
// 可选正则提取器从源库名/表名/数据源名抽出扩展列。大小写敏感由构造参数控制。

#![allow(dead_code, non_snake_case)]

use crate::selector;
use regex::Regex;
use std::sync::Arc;

/// 路由操作结果；错误消息为字符串，对齐 Go 侧错误包装风格。
pub type RouterResult<T> = Result<T, String>;

// TableRule is a rule to route schema/table to target schema/table.
/// 一条库表路由规则：源 pattern、目标库表，以及可选的扩展列提取器。
#[derive(Clone, Debug, Default)]
pub struct TableRule {
    /// 从表名正则提取扩展列。
    pub TableExtractor: Option<Box<TableExtractor>>,
    /// 从 schema 名正则提取扩展列。
    pub SchemaExtractor: Option<Box<SchemaExtractor>>,
    /// 从数据源名正则提取扩展列。
    pub SourceExtractor: Option<Box<SourceExtractor>>,
    /// 源 schema 通配 pattern（可含 `*`/`?` 等）。
    pub SchemaPattern: String,
    /// 源 table 通配；空串表示仅 schema 级规则。
    pub TablePattern: String,
    /// 路由后的目标 schema。
    pub TargetSchema: String,
    /// 路由后的目标 table；可为空表示保持原表名。
    pub TargetTable: String,
}

// TableExtractor extracts table name to column.
/// 用正则从 table 名捕获分组，写入目标列名。
#[derive(Clone, Debug, Default)]
pub struct TableExtractor {
    /// 编译后的正则；`Valid` 成功后填充。
    pub(crate) regexp: Option<Regex>,
    /// 扩展列列名。
    pub TargetColumn: String,
    /// 表名匹配正则原文。
    pub TableRegexp: String,
}

// SchemaExtractor extracts schema name to column.
/// 用正则从 schema 名捕获分组，写入目标列名。
#[derive(Clone, Debug, Default)]
pub struct SchemaExtractor {
    /// 编译后的正则；`Valid` 成功后填充。
    pub(crate) regexp: Option<Regex>,
    /// 扩展列列名。
    pub TargetColumn: String,
    /// schema 匹配正则原文。
    pub SchemaRegexp: String,
}

// SourceExtractor extracts source name to column.
/// 用正则从数据源名捕获分组，写入目标列名。
#[derive(Clone, Debug, Default)]
pub struct SourceExtractor {
    /// 编译后的正则；`Valid` 成功后填充。
    pub(crate) regexp: Option<Regex>,
    /// 扩展列列名。
    pub TargetColumn: String,
    /// 数据源匹配正则原文。
    pub SourceRegexp: String,
}

impl TableRule {
    // Valid checks validity of rule and compiles extractor regular expressions.
    /// 校验规则必填字段，并编译各提取器正则。
    pub fn Valid(&mut self) -> RouterResult<()> {
        if self.SchemaPattern.is_empty() {
            return Err("schema pattern of table route rule should not be empty".into());
        }
        if self.TargetSchema.is_empty() {
            return Err("target schema of table route rule should not be empty".into());
        }

        // 依次编译 table/schema/source 提取器；非法正则或空目标列则报错。
        if let Some(extractor) = self.TableExtractor.as_mut() {
            let regexp = compile_extractor(&extractor.TableRegexp).map_err(|_| {
                format!(
                    "table extractor table regexp illegal {}",
                    extractor.TableRegexp
                )
            })?;
            if extractor.TargetColumn.is_empty() {
                return Err("table extractor target column cannot be empty".into());
            }
            extractor.regexp = Some(regexp);
        }
        if let Some(extractor) = self.SchemaExtractor.as_mut() {
            let regexp = compile_extractor(&extractor.SchemaRegexp).map_err(|_| {
                format!(
                    "schema extractor schema regexp illegal {}",
                    extractor.SchemaRegexp
                )
            })?;
            if extractor.TargetColumn.is_empty() {
                return Err("schema extractor target column cannot be empty".into());
            }
            extractor.regexp = Some(regexp);
        }
        if let Some(extractor) = self.SourceExtractor.as_mut() {
            let regexp = compile_extractor(&extractor.SourceRegexp).map_err(|_| {
                format!(
                    "source extractor source regexp illegal {}",
                    extractor.SourceRegexp
                )
            })?;
            if extractor.TargetColumn.is_empty() {
                return Err("source extractor target column cannot be empty".into());
            }
            extractor.regexp = Some(regexp);
        }
        Ok(())
    }

    // ToLower converts schema/table patterns to lower case.
    /// 将 schema/table pattern 转为小写，供大小写不敏感匹配使用。
    pub fn ToLower(&mut self) {
        self.SchemaPattern = go_lowercase(&self.SchemaPattern);
        self.TablePattern = go_lowercase(&self.TablePattern);
    }

    /// 用提取器正则捕获分组并拼接为扩展列值；无匹配则返回空串。
    fn extractVal(&self, value: &str, extractor: ExtractorRef<'_>) -> String {
        let regexp = match extractor {
            ExtractorRef::Table(extractor) => extractor.regexp.as_ref(),
            ExtractorRef::Schema(extractor) => extractor.regexp.as_ref(),
            ExtractorRef::Source(extractor) => extractor.regexp.as_ref(),
        };
        // Like Go's nil regexp receiver, bypassing Valid through the public
        // selector is a programming error, not an unmatched expression.
        let regexp = regexp.expect("extractor regexp must be initialized by TableRule::Valid");
        let Some(captures) = regexp.captures(value) else {
            return String::new();
        };

        // 跳过完整匹配（组 0），拼接各捕获组，对齐 Go 多分组拼接语义。
        captures
            .iter()
            .skip(1)
            .filter_map(|capture| capture.map(|capture| capture.as_str()))
            .collect()
    }
}

// Table routes schema/table to target schema/table by given route rules.
/// 表路由器：持有 trie 选择器与大小写敏感标志。
pub struct Table {
    /// 底层规则选择器（通常为 trieSelector）。
    pub Selector: Box<dyn selector::Selector>,
    /// `true` 时按原文匹配；`false` 时 pattern 与输入均小写化。
    caseSensitive: bool,
}

/// `Table` 的别名，与 Go `TableRouter` 命名对齐。
pub type TableRouter = Table;

// NewTableRouter returns a table router.
/// 构造表路由器并依次 `AddRule` 初始规则集。
pub fn NewTableRouter(caseSensitive: bool, rules: Vec<TableRule>) -> RouterResult<Table> {
    let mut router = Table {
        Selector: selector::NewTrieSelector(),
        caseSensitive,
    };
    for mut rule in rules {
        router
            .insert_rule(&mut rule, selector::Insert)
            .map_err(|error| format!("initial rule {rule} in table router: {error}"))?;
    }
    Ok(router)
}

impl Table {
    // AddRule adds a rule into table router.
    /// 校验后插入规则（`Insert`）；不敏感模式下先 `ToLower`。
    pub fn AddRule(&mut self, mut rule: TableRule) -> RouterResult<()> {
        self.insert_rule(&mut rule, selector::Insert)
    }

    // UpdateRule updates rule.
    /// 校验后以 `Replace` 覆盖同 pattern 规则。
    pub fn UpdateRule(&mut self, mut rule: TableRule) -> RouterResult<()> {
        self.insert_rule(&mut rule, selector::Replace)
    }

    // Keep the prepared rule available to NewTableRouter's error annotation,
    // which Go constructs after Valid and ToLower have mutated the input rule.
    fn insert_rule(&mut self, rule: &mut TableRule, mode: i32) -> RouterResult<()> {
        rule.Valid()?;
        if !self.caseSensitive {
            rule.ToLower();
        }
        let action = if mode == selector::Insert {
            "add"
        } else {
            "update"
        };
        self.Selector
            .Insert(
                &rule.SchemaPattern,
                &rule.TablePattern,
                Some(Arc::new(rule.clone())),
                mode,
            )
            .map_err(|error| format!("{action} rule {rule} into table router: {error}"))
    }

    // RemoveRule removes a rule from table router.
    /// 按 pattern 删除规则；不敏感模式下先小写化 pattern。
    pub fn RemoveRule(&mut self, mut rule: TableRule) -> RouterResult<()> {
        if !self.caseSensitive {
            rule.ToLower();
        }
        self.Selector
            .Remove(&rule.SchemaPattern, &rule.TablePattern)
            .map_err(|error| format!("remove rule {rule} from table router: {error}"))
    }

    // Route routes schema/table to target schema/table.
    /// 匹配并返回目标 (schema, table)；多条同级规则命中时报冲突错误。
    pub fn Route(&self, schema: &str, table: &str) -> RouterResult<(String, String)> {
        // 匹配键按大小写策略规范化；返回的目标名仍基于原始入参做默认回退。
        let (schema_for_match, table_for_match) = if self.caseSensitive {
            (schema.to_owned(), table.to_owned())
        } else {
            (go_lowercase(schema), go_lowercase(table))
        };
        let rules = self.Selector.Match(&schema_for_match, &table_for_match);
        let (schema_rules, table_rules) = classify_rules(&rules)?;

        // 有表名且存在表级规则时优先表级；否则用 schema 级。同级 >1 条视为冲突。
        let selected = if table.is_empty() || table_rules.is_empty() {
            if schema_rules.len() > 1 {
                return Err(format!(
                    "`{schema}`.`{table}` matches {} schema route rules which is more than one.\nThe first two rules are {}, {}.\nIt's not supported",
                    schema_rules.len(),
                    schema_rules[0],
                    schema_rules[1]
                ));
            }
            schema_rules.first().copied()
        } else {
            if table_rules.len() > 1 {
                return Err(format!(
                    "`{schema}`.`{table}` matches {} table route rules which is more than one.\nThe first two rules are {}, {}.\nIt's not supported",
                    table_rules.len(),
                    table_rules[0],
                    table_rules[1]
                ));
            }
            table_rules.first().copied()
        };

        // 目标为空则回退到原始 schema/table。
        let target_schema = selected
            .map(|rule| rule.TargetSchema.clone())
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| schema.to_owned());
        let target_table = selected
            .map(|rule| rule.TargetTable.clone())
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| table.to_owned());
        Ok((target_schema, target_table))
    }

    // FetchExtendColumn returns extracted columns and values.
    /// 按命中规则提取扩展列名与值；表级优先于 schema 级。
    pub fn FetchExtendColumn(
        &self,
        schema: &str,
        table: &str,
        source: &str,
    ) -> (Vec<String>, Vec<String>) {
        let rules = self.Selector.Match(schema, table);
        let Ok((schema_rules, table_rules)) = classify_rules(&rules) else {
            return (Vec::new(), Vec::new());
        };
        let selected = table_rules.first().or_else(|| schema_rules.first());
        let Some(rule) = selected.copied() else {
            return (Vec::new(), Vec::new());
        };

        // 按 table → schema → source 顺序追加扩展列。
        let mut columns = Vec::new();
        let mut values = Vec::new();
        if let Some(extractor) = rule.TableExtractor.as_deref() {
            columns.push(extractor.TargetColumn.clone());
            values.push(rule.extractVal(table, ExtractorRef::Table(extractor)));
        }
        if let Some(extractor) = rule.SchemaExtractor.as_deref() {
            columns.push(extractor.TargetColumn.clone());
            values.push(rule.extractVal(schema, ExtractorRef::Schema(extractor)));
        }
        if let Some(extractor) = rule.SourceExtractor.as_deref() {
            columns.push(extractor.TargetColumn.clone());
            values.push(rule.extractVal(source, ExtractorRef::Source(extractor)));
        }
        (columns, values)
    }
}

/// 将选择器返回的动态规则按 TablePattern 是否为空分为 schema 级与 table 级。
fn classify_rules(rules: &selector::RuleSet) -> RouterResult<(Vec<&TableRule>, Vec<&TableRule>)> {
    let mut schema_rules = Vec::with_capacity(rules.0.len());
    let mut table_rules = Vec::with_capacity(rules.0.len());
    for raw_rule in &rules.0 {
        let rule = raw_rule
            .as_ref()
            .downcast_ref::<TableRule>()
            .ok_or_else(|| format!("table route rule {} not valid", go_rule_value(raw_rule)))?;
        if rule.TablePattern.is_empty() {
            schema_rules.push(rule);
        } else {
            table_rules.push(rule);
        }
    }
    Ok((schema_rules, table_rules))
}

/// 提取器借用枚举，统一 `extractVal` 取正则路径。
enum ExtractorRef<'a> {
    Table(&'a TableExtractor),
    Schema(&'a SchemaExtractor),
    Source(&'a SourceExtractor),
}

// Go strings.ToLower uses a single, context-independent mapping per rune.
fn go_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
        .collect()
}

#[path = "go_regex.rs"]
mod go_regex;

fn compile_extractor(pattern: &str) -> Result<Regex, regex::Error> {
    go_regex::compile(pattern)
}

// Go fmt %+v prints string fields without quoting and pointers as addresses.
impl std::fmt::Display for TableRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn pointer<T>(value: &Option<Box<T>>) -> String {
            value
                .as_deref()
                .map_or_else(|| "<nil>".into(), |value| format!("{value:p}"))
        }
        write!(
            f,
            "&{{TableExtractor:{} SchemaExtractor:{} SourceExtractor:{} SchemaPattern:{} TablePattern:{} TargetSchema:{} TargetTable:{}}}",
            pointer(&self.TableExtractor),
            pointer(&self.SchemaExtractor),
            pointer(&self.SourceExtractor),
            self.SchemaPattern,
            self.TablePattern,
            self.TargetSchema,
            self.TargetTable
        )
    }
}

fn go_rule_value(rule: &selector::Rule) -> String {
    macro_rules! display {
        ($($ty:ty),*) => { $(if let Some(value) = rule.downcast_ref::<$ty>() { return value.to_string(); })* };
    }
    display!(
        String, &str, bool, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64
    );
    // Rust Any does not provide Go's runtime reflection for arbitrary objects.
    // Preserve identity for opaque custom values in the diagnostic.
    format!("{:p}", Arc::as_ptr(rule))
}
