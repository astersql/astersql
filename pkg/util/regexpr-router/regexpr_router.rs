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

// 基于正则/通配的库表路由表（RouteTable），对齐 Go regexpr-router。
//
// 将 TableRule 编译为 filter，支持大小写敏感配置、库级/表级规则优先级、
// 多规则冲突报错、AllRules 拆分，以及从 table/schema/source 提取扩展列。

// 保留与 Go 对应的模块名和字段名，便于持续做一比一语义对照。

use regex::Regex;

pub use router_crate::router;

/// 过滤规则类型别名，对应 Go `int32`，区分表级与库级规则。
// FilterType is the type of filter
// FilterType 对应 Go 里的 int32 类型别名，用来区分表级规则和库级规则。
pub type FilterType = i32;

/// 表级过滤：规则含 table pattern（Go iota 从 1 起）。
// TblFilter is table filter
// TblFilter 表示包含 table pattern 的表级路由规则，Go iota 从 1 开始。
pub const TblFilter: FilterType = 1;
/// 库级过滤：仅含 schema pattern。
// SchmFilter is schema filter
// SchmFilter 表示只包含 schema pattern 的库级路由规则。
pub const SchmFilter: FilterType = 2;

/// 内部规则包装：绑定原始 TableRule、目标表、匹配器与规则类型。
// filterWrapper 对应 Go 的私有结构体 filterWrapper。
// 它把原始 TableRule、目标表信息、匹配器和规则类型绑在一起，便于 Route/AllRules/FetchExtendColumn 复用。
struct filterWrapper {
    // Go 里是 *filter.Filter；Rust 用 Option<Box<_>> 表达 AddRule 构造期间的未初始化状态。
    filter: Option<Box<filter::Filter>>,
    // Go 里保留 *router.TableRule 指针；AddRule 会在大小写归一化后保存这份规则。
    rawRule: Option<Box<router::TableRule>>,
    target: filter::Table,
    typ: FilterType,
}

/// 路由表：按添加顺序保存过滤规则，并记录是否大小写敏感。
// RouteTable is route table
// RouteTable 保存按添加顺序排列的过滤规则，以及是否大小写敏感的匹配配置。
pub struct RouteTable {
    filters: Vec<filterWrapper>,
    caseSensitive: bool,
}

/// 创建 RouteTable 并按输入顺序逐条 AddRule；任一条失败则立即返回错误。
// NewRegExprRouter is to create RouteTable
// NewRegExprRouter 对应 Go 构造函数：创建空 RouteTable 后按输入顺序逐条 AddRule。
// 任一规则校验或 filter.New 失败时，保持 Go 语义立即返回错误。
pub fn NewRegExprRouter(
    caseSensitive: bool,
    rules: Vec<router::TableRule>,
) -> Result<RouteTable, String> {
    let mut r = RouteTable {
        filters: Vec::new(),
        caseSensitive,
    };
    for rule in rules {
        // Go 版本传入 *TableRule；这里按值接收后交给 AddRule，指针/所有权差异留给后续编译修复。
        if let Err(err) = r.AddRule(rule) {
            return Err(err);
        }
    }
    Ok(r)
}

impl RouteTable {
    // AddRule is to add rule
    // AddRule 校验并加入一条路由规则；这段保留 Go 的校验、大小写归一化、构建 filter、追加 filters 顺序。
    pub fn AddRule(&mut self, mut rule: router::TableRule) -> Result<(), String> {
        // 对应 rule.Valid()；失败时用 errors.Trace 保留 Go 的错误包装语义。
        if let Err(err) = rule.Valid() {
            return Err(err);
        }
        // case-insensitive 模式会直接修改传入规则内容；Go 里这是对 *TableRule 的原地修改。
        if !self.caseSensitive {
            rule.ToLower();
        }

        let mut newFilter = filterWrapper {
            filter: None,
            // 保存归一化后的原始规则，后续 AllRules 和 FetchExtendColumn 都依赖这份规则。
            rawRule: Some(Box::new(rule)),
            target: filter::Table {
                Schema: String::new(),
                Name: String::new(),
            },
            typ: 0,
        };

        // Go 字段名保持为 TargetSchema/TargetTable，方便和来源文件对照。
        let rule_ref = newFilter.rawRule.as_ref().expect("rule is set above");
        newFilter.target = filter::Table {
            Schema: rule_ref.TargetSchema.clone(),
            Name: rule_ref.TargetTable.clone(),
        };

        if rule_ref.TablePattern.is_empty() {
            // raw schema rule
            // TablePattern 为空时是库级规则，只配置 DoDBs。
            newFilter.typ = SchmFilter;
            let rawFilter = match filter::New(
                self.caseSensitive,
                Some(Box::new(filter::Rules {
                    DoDBs: vec![rule_ref.SchemaPattern.clone()],
                    ..Default::default()
                })),
            ) {
                Ok(rawFilter) => rawFilter,
                Err(err) => {
                    return Err(format!("add rule {:?} into table router: {err}", rule_ref));
                }
            };
            newFilter.filter = Some(rawFilter);
        } else {
            // TablePattern 非空时是表级规则，同时配置 DoTables 和 DoDBs，保持 Go filter.New 入参形状。
            newFilter.typ = TblFilter;
            let rawFilter = match filter::New(
                self.caseSensitive,
                Some(Box::new(filter::Rules {
                    DoTables: vec![Box::new(filter::Table {
                        Schema: rule_ref.SchemaPattern.clone(),
                        Name: rule_ref.TablePattern.clone(),
                    })],
                    DoDBs: vec![rule_ref.SchemaPattern.clone()],
                    ..Default::default()
                })),
            ) {
                Ok(rawFilter) => rawFilter,
                Err(err) => {
                    return Err(format!("add rule {:?} into table router: {err}", rule_ref));
                }
            };
            newFilter.filter = Some(rawFilter);
        }

        self.filters.push(newFilter);
        Ok(())
    }

    // Route is to route table
    // Route 根据 schema/table 查找目标 schema/table。返回值形状对应 Go 的命名返回值：
    // targetSchema、targetTable 和 err；用 Result<(String, String), Error> 表达。
    pub fn Route(&self, schema: &str, table: &str) -> Result<(String, String), String> {
        let curTable = filter::Table {
            Schema: schema.to_owned(),
            Name: table.to_owned(),
        };
        let mut tblRules: Vec<&filterWrapper> = Vec::new();
        let mut schmRules: Vec<&filterWrapper> = Vec::new();

        for filterWrapper in &self.filters {
            // Go 里 filter 必定由 AddRule 填充；Option 只是在 实现中标明指针语义。
            let matched = filterWrapper
                .filter
                .as_ref()
                .map(|raw_filter| raw_filter.Match(&curTable))
                .unwrap_or(false);
            if matched {
                if filterWrapper.typ == TblFilter {
                    tblRules.push(filterWrapper);
                } else {
                    schmRules.push(filterWrapper);
                }
            }
        }

        let mut targetSchema = String::new();
        let mut targetTable = String::new();
        if table.is_empty() || tblRules.is_empty() {
            // 1. no need to match table or
            // 2. match no table
            // 未传 table 或没有表级命中时只考虑库级规则；多个库级命中保持 Go 的冲突错误。
            if schmRules.len() > 1 {
                return Err(format!(
                    "table {}.{} matches more than one rule",
                    schema, table
                ));
            }
            if schmRules.len() == 1 {
                targetSchema = schmRules[0].target.Schema.clone();
                targetTable = schmRules[0].target.Name.clone();
            }
        } else {
            // 有表名且有表级命中时，表级规则优先；多个表级命中同样返回冲突错误。
            if tblRules.len() > 1 {
                return Err(format!(
                    "table {}.{} matches more than one rule",
                    schema, table
                ));
            }
            targetSchema = tblRules[0].target.Schema.clone();
            targetTable = tblRules[0].target.Name.clone();
        }

        // Go 命名返回值默认为空串；没有目标 schema/table 时回退到输入值。
        if targetSchema.is_empty() {
            targetSchema = schema.to_owned();
        }
        if targetTable.is_empty() {
            targetTable = table.to_owned();
        }
        Ok((targetSchema, targetTable))
    }

    // AllRules is to get all rules
    // AllRules 按 Go 逻辑把内部规则拆分为库级规则和表级规则；返回顺序保持 filters 添加顺序。
    pub fn AllRules(&self) -> (Vec<router::TableRule>, Vec<router::TableRule>) {
        let mut schmRouteRules: Vec<router::TableRule> = Vec::new();
        let mut tableRouteRules: Vec<router::TableRule> = Vec::new();
        for f in &self.filters {
            if let Some(raw_rule) = &f.rawRule {
                if f.typ == SchmFilter {
                    schmRouteRules.push((**raw_rule).clone());
                } else {
                    tableRouteRules.push((**raw_rule).clone());
                }
            }
        }
        (schmRouteRules, tableRouteRules)
    }

    // FetchExtendColumn is to fetch extend column
    // FetchExtendColumn 查找当前表命中的规则，并从 table/schema/source 三类 extractor 中提取扩展列。
    // Go 代码只取匹配到的第一条表级规则；若没有表级规则，则取第一条库级规则。
    pub fn FetchExtendColumn(
        &self,
        schema: &str,
        table: &str,
        source: &str,
    ) -> (Vec<String>, Vec<String>) {
        let mut cols: Vec<String> = Vec::new();
        let mut vals: Vec<String> = Vec::new();
        let mut rules: Vec<&filterWrapper> = Vec::new();
        let curTable = filter::Table {
            Schema: schema.to_owned(),
            Name: table.to_owned(),
        };

        for f in &self.filters {
            // 只收集 filter.Match 命中的规则；这里不访问外部资源，只做内存匹配。
            let matched = f
                .filter
                .as_ref()
                .map(|raw_filter| raw_filter.Match(&curTable))
                .unwrap_or(false);
            if matched {
                rules.push(f);
            }
        }

        let mut schemaRules: Vec<&router::TableRule> = Vec::with_capacity(rules.len());
        let mut tableRules: Vec<&router::TableRule> = Vec::with_capacity(rules.len());
        for f in rules {
            if let Some(rule) = &f.rawRule {
                if rule.TablePattern.is_empty() {
                    schemaRules.push(rule);
                } else {
                    tableRules.push(rule);
                }
            }
        }

        if tableRules.is_empty() && schemaRules.is_empty() {
            return (cols, vals);
        }

        // Go 中 rule 是 *router.TableRule；这里用引用保留“选择一个命中规则继续提取”的语义。
        let rule = if tableRules.is_empty() {
            schemaRules[0]
        } else {
            tableRules[0]
        };

        if let Some(table_extractor) = &rule.TableExtractor {
            cols.push(table_extractor.TargetColumn.clone());
            vals.push(extractVal(table, ExtractorRef::Table(table_extractor)));
        }

        if let Some(schema_extractor) = &rule.SchemaExtractor {
            cols.push(schema_extractor.TargetColumn.clone());
            vals.push(extractVal(schema, ExtractorRef::Schema(schema_extractor)));
        }

        if let Some(source_extractor) = &rule.SourceExtractor {
            cols.push(source_extractor.TargetColumn.clone());
            vals.push(extractVal(source, ExtractorRef::Source(source_extractor)));
        }
        (cols, vals)
    }
}

/// 辅助枚举：表达 Go `extractVal(s, ext any)` 的 type switch。
// ExtractorRef 是迁移辅助枚举，用来表达 Go extractVal(s string, ext any) 的 type switch。
// 使用强类型变体替代 Go `any` 与 type switch，避免运行时 downcast。
enum ExtractorRef<'a> {
    Table(&'a router::TableExtractor),
    Schema(&'a router::SchemaExtractor),
    Source(&'a router::SourceExtractor),
}

/// 按 extractor 类型取正则，拼接第 1 个之后的捕获组作为扩展列值。
// extractVal 对应 Go 的私有函数 extractVal。
// 它按 extractor 类型选择正则字段，忽略正则编译错误，并把第 1 个之后的 submatch 拼接成返回值。
fn extractVal(s: &str, ext: ExtractorRef<'_>) -> String {
    let mut params: Vec<String> = Vec::new();
    match ext {
        ExtractorRef::Table(e) => {
            // Go regexp.Compile 失败时不会返回错误，只留下空 params。
            if let Ok(regExpr) = Regex::new(&e.TableRegexp) {
                if let Some(captures) = regExpr.captures(s) {
                    params = captures
                        .iter()
                        .map(|m| m.map(|v| v.as_str().to_owned()).unwrap_or_default())
                        .collect();
                }
            }
        }
        ExtractorRef::Schema(e) => {
            if let Ok(regExpr) = Regex::new(&e.SchemaRegexp) {
                if let Some(captures) = regExpr.captures(s) {
                    params = captures
                        .iter()
                        .map(|m| m.map(|v| v.as_str().to_owned()).unwrap_or_default())
                        .collect();
                }
            }
        }
        ExtractorRef::Source(e) => {
            if let Ok(regExpr) = Regex::new(&e.SourceRegexp) {
                if let Some(captures) = regExpr.captures(s) {
                    params = captures
                        .iter()
                        .map(|m| m.map(|v| v.as_str().to_owned()).unwrap_or_default())
                        .collect();
                }
            }
        }
    }

    let mut val = String::new();
    for (idx, param) in params.iter().enumerate() {
        if idx > 0 {
            val.push_str(param);
        }
    }
    val
}
