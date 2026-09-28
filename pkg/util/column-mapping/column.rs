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

// 列映射（column mapping）：按规则改写行值中的目标列。
//
// 对应 Go `pkg/util/column-mapping`。支持加前缀/后缀，以及把 instance/
// schema/table 位段拼入分区 ID（partition ID）。规则经 trie selector
// 匹配，结果缓存在 `Mapping.cache`。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use crate::table_rule_selector::{
    Insert as SelectorInsert, NewTrieSelector, Replace as SelectorReplace, Rule as SelectorRule,
    Selector,
};

type ColumnResult<T> = Result<T, String>;
type RuleRef = Arc<Rule>;

// for partition ID, ref definition of partitionID
// 这些全局位宽对应 Go package 变量；用 atomic 表达 SetPartitionRule 的全局可变语义。
static instanceIDBitSize: AtomicI32 = AtomicI32::new(4);
static schemaIDBitSize: AtomicI32 = AtomicI32::new(7);
static tableIDBitSize: AtomicI32 = AtomicI32::new(8);
static maxOriginID: AtomicI64 = AtomicI64::new(17_592_186_044_416);

// SetPartitionRule sets bit size of schema ID and table ID
/// 设置 partition ID 中 instance/schema/table 各段位宽，并重算 origin ID 上限。
// SetPartitionRule 对应 Go 的包级设置函数，更新 instance/schema/table 位宽并重算 origin ID 上限。
pub fn SetPartitionRule(instanceIDSize: i32, schemaIDSize: i32, tableIDSize: i32) {
    instanceIDBitSize.store(instanceIDSize, Ordering::SeqCst);
    schemaIDBitSize.store(schemaIDSize, Ordering::SeqCst);
    tableIDBitSize.store(tableIDSize, Ordering::SeqCst);

    let remain = 64 - instanceIDSize - schemaIDSize - tableIDSize - 1;
    // Go 使用 int64 左移；这里保留同样的“最高符号位不用”的容量计算。
    maxOriginID.store(1_i64 << remain, Ordering::SeqCst);
}

// Expr indicates how to handle column mapping
/// 列映射表达式：加前缀、加后缀、拼分区 ID，或未知扩展值。
// Expr 对应 Go 的 string alias；Other 保存未来扩展或配置中的未知表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    AddPrefix,
    AddSuffix,
    PartitionID,
    Other(String),
}

impl Expr {
    // as_str 保留 Go 常量的字符串取值，便于错误消息与配置内容对齐。
    fn as_str(&self) -> &str {
        match self {
            Expr::AddPrefix => "add prefix",
            Expr::AddSuffix => "add suffix",
            Expr::PartitionID => "partition id",
            Expr::Other(v) => v.as_str(),
        }
    }
}

// poor Expr
/// 构造「加前缀」表达式常量。
// 下面三个构造函数对应 Go 的 AddPrefix/AddSuffix/PartitionID 常量。
pub fn AddPrefix() -> Expr {
    Expr::AddPrefix
}

/// 构造「加后缀」表达式常量。
pub fn AddSuffix() -> Expr {
    Expr::AddSuffix
}

/// 构造「分区 ID」表达式常量。
pub fn PartitionID() -> Expr {
    Expr::PartitionID
}

/// 行值中可被映射处理的类型集合（对齐 Go `[]any` 中的常见形态）。
// Value 对应 Go []any 中会被本文件处理的值类型。
// 未知类型用 Other 记录，便于 partitionID 分支复刻 Go default 错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Int(i64),
    Int8(i8),
    Int32(i32),
    Uint(u64),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    String(String),
    Other(String),
}

// Rule is a rule to map column
// TODO: we will do it later, if we need to implement a real column mapping, we need table structure of source and target system
/// 单条列映射规则：匹配 schema/table pattern，并对目标列应用表达式。
// Rule 字段顺序和 Go struct 保持一致；yaml/json/toml tag 作为配置语义记录在字段名中。
#[derive(Clone, Debug)]
pub struct Rule {
    pub PatternSchema: String,
    pub PatternTable: String,
    pub SourceColumn: String,
    pub TargetColumn: String,
    pub Expression: Expr,
    pub Arguments: Vec<String>,
    pub CreateTableQuery: String,
}

impl Rule {
    // ToLower covert schema/table parttern to lower case
    // ToLower 对应 Go 方法，只归一化 schema/table pattern，不改列名和参数。
    pub fn ToLower(&mut self) {
        self.PatternSchema = self.PatternSchema.to_lowercase();
        self.PatternTable = self.PatternTable.to_lowercase();
    }

    // Valid checks validity of rule.
    // add prefix/suffix: it should have target column and one argument
    // partition id: it should have 3 to 4 arguments
    // Valid 对应 Go 的规则合法性检查，错误消息保留原 Go 分类。
    pub fn Valid(&self) -> ColumnResult<()> {
        if expr_handler(&self.Expression).is_none() {
            return Err(format!("expression {} not found", self.Expression.as_str()));
        }

        if self.TargetColumn.is_empty() {
            return Err("rule need to be applied a target column is not valid".to_string());
        }

        if self.Expression == Expr::AddPrefix || self.Expression == Expr::AddSuffix {
            if self.Arguments.len() != 1 {
                return Err(format!(
                    "arguments {:?} for add prefix/suffix is not valid",
                    self.Arguments
                ));
            }
        }

        if self.Expression == Expr::PartitionID {
            match self.Arguments.len() {
                3 | 4 => return Ok(()),
                _ => {
                    return Err(format!(
                        "arguments {:?} for patition id is not valid",
                        self.Arguments
                    ));
                }
            }
        }

        Ok(())
    }

    // Adjust normalizes the rule into an easier-to-process form, e.g. filling in
    // optional arguments with the default values.
    // Adjust 对应 Go 的可选参数补齐：partition id 只有 3 个参数时追加空分隔符。
    pub fn Adjust(&mut self) {
        if self.Expression == Expr::PartitionID && self.Arguments.len() == 3 {
            self.Arguments.push(String::new());
        }
    }

    // check source and target position
    // adjustColumnPosition 对应 Go 的位置检查；target 找不到时直接返回 NotFound 语义。
    fn adjustColumnPosition(&self, source: isize, target: isize) -> ColumnResult<(isize, isize)> {
        // if not found target, ignore it
        if target == -1 {
            return Err(format!("target column {} not found", self.TargetColumn));
        }

        Ok((source, target))
    }
}

// mappingInfo 对应 Go 的内部结构，缓存某个 schema/table 的匹配结果和 partition ID 预计算值。
#[derive(Clone, Debug)]
struct mappingInfo {
    ignore: bool,
    sourcePosition: isize,
    targetPosition: isize,
    rule: Option<RuleRef>,

    instanceID: i64,
    schemaID: i64,
    tableID: i64,
}

impl Default for mappingInfo {
    fn default() -> Self {
        mappingInfo {
            ignore: false,
            sourcePosition: -1,
            targetPosition: -1,
            rule: None,
            instanceID: 0,
            schemaID: 0,
            tableID: 0,
        }
    }
}

// Mapping maps column to something by rules
/// 列映射引擎：持有规则 selector、大小写策略与查询缓存。
// Mapping 对应 Go 的结构体。selector 保存规则，cache 用 RwLock 迁移 sync.RWMutex。
pub struct Mapping {
    selector: Box<dyn Selector>,
    caseSensitive: bool,
    cache: RwLock<HashMap<String, Arc<mappingInfo>>>,
}

// NewMapping returns a column mapping
/// 构造 Mapping：创建 selector、初始化缓存，并依次 `AddRule`。
// NewMapping 对应 Go 构造函数：创建 selector、初始化缓存，再依次 AddRule。
pub fn NewMapping(caseSensitive: bool, rules: Vec<Rule>) -> ColumnResult<Mapping> {
    let m = Mapping {
        selector: NewTrieSelector(),
        caseSensitive,
        cache: RwLock::new(HashMap::new()),
    };

    m.resetCache();
    for rule in rules {
        if let Err(err) = m.AddRule(Some(rule.clone())) {
            return Err(format!("initial rule {:?} in mapping: {}", rule, err));
        }
    }

    Ok(m)
}

impl Mapping {
    // addOrUpdateRule 对应 Go 的共享 AddRule/UpdateRule 逻辑：nil 直接忽略，校验、大小写归一化、清缓存，再插入或替换。
    fn addOrUpdateRule(&self, rule: Option<Rule>, isUpdate: bool) -> ColumnResult<()> {
        let mut rule = match rule {
            Some(rule) => rule,
            None => return Ok(()),
        };

        rule.Valid()?;
        if !self.caseSensitive {
            rule.ToLower();
        }
        rule.Adjust();

        self.resetCache();
        let schema = rule.PatternSchema.clone();
        let table = rule.PatternTable.clone();
        let selector_rule: SelectorRule = Arc::new(rule.clone());
        let insert_type = if isUpdate {
            SelectorReplace
        } else {
            SelectorInsert
        };
        self.selector
            .Insert(&schema, &table, Some(selector_rule), insert_type)
            .map_err(|err| {
                let method = if isUpdate { "update" } else { "add" };
                format!("{} rule {:?} into mapping: {}", method, rule, err)
            })
    }

    // AddRule adds a rule into mapping
    // AddRule 保留 Go 的插入语义；重复规则的精确冲突交由真实 selector 接线后恢复。
    pub fn AddRule(&self, rule: Option<Rule>) -> ColumnResult<()> {
        self.addOrUpdateRule(rule, false)
    }

    // UpdateRule updates mapping rule
    // UpdateRule 保留 Go 的 Replace 语义：按 schema/table pattern 找到旧规则并替换。
    pub fn UpdateRule(&self, rule: Option<Rule>) -> ColumnResult<()> {
        self.addOrUpdateRule(rule, true)
    }

    // RemoveRule removes a rule from mapping
    // RemoveRule 对应 Go 的删除逻辑，先按大小写策略调整 pattern，再清缓存并删除匹配规则。
    pub fn RemoveRule(&self, rule: Option<Rule>) -> ColumnResult<()> {
        let mut rule = match rule {
            Some(rule) => rule,
            None => return Ok(()),
        };
        if !self.caseSensitive {
            rule.ToLower();
        }

        self.resetCache();
        self.selector
            .Remove(&rule.PatternSchema, &rule.PatternTable)
            .map_err(|err| format!("remove rule {:?} from mapping: {}", rule, err))
    }

    // HandleRowValue handles row value
    // HandleRowValue 对应 Go 的行值处理：查规则、跳过 ignore、执行表达式并返回受影响的 source/target 位置。
    pub fn HandleRowValue(
        &self,
        schema: &str,
        table: &str,
        columns: &[String],
        vals: Vec<Value>,
    ) -> ColumnResult<(Vec<Value>, Option<Vec<isize>>)> {
        let (schemaL, tableL) = if self.caseSensitive {
            (schema.to_string(), table.to_string())
        } else {
            (schema.to_lowercase(), table.to_lowercase())
        };

        let info = self.queryColumnInfo(&schemaL, &tableL, columns)?;
        if info.ignore {
            return Ok((vals, None));
        }

        let rule = info
            .rule
            .as_ref()
            .expect("non-ignore mappingInfo must carry rule");
        let exp = expr_handler(&rule.Expression).ok_or_else(|| {
            format!(
                "column mapping expression {} not found",
                rule.Expression.as_str()
            )
        })?;

        let vals = exp(&info, vals)?;
        Ok((vals, Some(vec![info.sourcePosition, info.targetPosition])))
    }

    // HandleDDL handles ddl
    // HandleDDL 对应 Go 的 DDL 路径：目前只定位规则并返回“未实现”错误，待后续实现。
    pub fn HandleDDL(
        &self,
        schema: &str,
        table: &str,
        columns: &[String],
        statement: String,
    ) -> ColumnResult<(String, Option<Vec<isize>>)> {
        let (schemaL, tableL) = if self.caseSensitive {
            (schema.to_string(), table.to_string())
        } else {
            (schema.to_lowercase(), table.to_lowercase())
        };

        let info = self.queryColumnInfo(&schemaL, &tableL, columns)?;
        if info.ignore {
            return Ok((statement, None));
        }

        self.resetCache();
        // only output erro now, wait fix it manually
        let rule = info
            .rule
            .as_ref()
            .expect("non-ignore mappingInfo must carry rule");
        Err(format!(
            "ddl {} @ column mapping rule {}/{}:{:?} not implemented",
            statement, schema, table, rule
        ))
    }

    // queryColumnInfo 对应 Go 的核心查询缓存逻辑：先读缓存，未命中时匹配规则、分类优先级并计算列位置。
    fn queryColumnInfo(
        &self,
        schema: &str,
        table: &str,
        columns: &[String],
    ) -> ColumnResult<Arc<mappingInfo>> {
        let key = tableName(schema, table);
        if let Some(ci) = self
            .cache
            .read()
            .expect("Mapping.cache read lock poisoned")
            .get(&key)
        {
            return Ok(Arc::clone(ci));
        }

        let mut info = mappingInfo {
            ignore: true,
            ..mappingInfo::default()
        };

        let rules = self.selector.Match(schema, table).0;
        if rules.is_empty() {
            let cached = Arc::new(info);
            self.cache
                .write()
                .expect("Mapping.cache write lock poisoned")
                .insert(key, Arc::clone(&cached));
            return Ok(cached);
        }

        let mut schemaRules: Vec<RuleRef> = Vec::new();
        let mut tableRules: Vec<RuleRef> = Vec::with_capacity(1);
        // classify rules into schema level rules and table level
        // table level rules have highest priority
        for raw_rule in rules {
            let rule = Arc::downcast::<Rule>(raw_rule).map_err(|_| {
                "column mapping rule has an unexpected type is not valid".to_string()
            })?;
            if rule.PatternTable.is_empty() {
                schemaRules.push(rule);
            } else {
                tableRules.push(rule);
            }
        }

        // only support one expression for one table now, refine it later
        let rule = if table.is_empty() || tableRules.is_empty() {
            if schemaRules.len() != 1 {
                return Err(format!(
                    "`{}`.`{}` matches {} schema column mapping rules which should be one. It's not supported",
                    schema,
                    table,
                    schemaRules.len()
                ));
            }
            Arc::clone(&schemaRules[0])
        } else {
            if tableRules.len() != 1 {
                return Err(format!(
                    "`{}`.`{}` matches {} table column mapping rules which should be one. It's not supported",
                    schema,
                    table,
                    tableRules.len()
                ));
            }
            Arc::clone(&tableRules[0])
        };

        // compute source and target column position
        let sourcePosition = findColumnPosition(columns, &rule.SourceColumn);
        let targetPosition = findColumnPosition(columns, &rule.TargetColumn);
        let (sourcePosition, targetPosition) =
            rule.adjustColumnPosition(sourcePosition, targetPosition)?;

        info = mappingInfo {
            sourcePosition,
            targetPosition,
            rule: Some(Arc::clone(&rule)),
            ..mappingInfo::default()
        };

        // if expr is partition ID, compute schema and table ID
        if rule.Expression == Expr::PartitionID {
            let (instanceID, schemaID, tableID) = computePartitionID(schema, table, &rule)?;
            info.instanceID = instanceID;
            info.schemaID = schemaID;
            info.tableID = tableID;
        }

        let info = Arc::new(info);
        self.cache
            .write()
            .expect("Mapping.cache write lock poisoned")
            .insert(tableName(schema, table), Arc::clone(&info));

        Ok(info)
    }

    // resetCache 对应 Go 的缓存清理，写锁 guard 离开作用域时自动释放。
    fn resetCache(&self) {
        let mut cache = self
            .cache
            .write()
            .expect("Mapping.cache write lock poisoned");
        *cache = HashMap::new();
    }
}

// findColumnPosition 对应 Go 的线性扫描，找不到时返回 -1。
fn findColumnPosition(cols: &[String], col: &str) -> isize {
    for (i, c) in cols.iter().enumerate() {
        if c == col {
            return i as isize;
        }
    }

    -1
}

// tableName 对应 Go 的 fmt.Sprintf("`%s`.`%s`", schema, table)，用于 cache key。
fn tableName(schema: &str, table: &str) -> String {
    format!("`{}`.`{}`", schema, table)
}

// addPrefix 对应 Go 的同名表达式：要求目标值是 string，再把 prefix 拼到前面。
fn addPrefix(info: &mappingInfo, mut vals: Vec<Value>) -> ColumnResult<Vec<Value>> {
    let rule = info.rule.as_ref().expect("mappingInfo.rule is required");
    let prefix = &rule.Arguments[0];
    let idx = info.targetPosition as usize;
    let originStr = match vals.get(idx) {
        Some(Value::String(v)) => v.clone(),
        other => {
            return Err(format!(
                "column {} value is not string, but {:?}, which is not valid",
                info.targetPosition, other
            ));
        }
    };

    // fast to concatenated string
    let mut rawByte = String::with_capacity(prefix.len() + originStr.len());
    rawByte.push_str(prefix);
    rawByte.push_str(&originStr);

    vals[idx] = Value::String(rawByte);
    Ok(vals)
}

// addSuffix 对应 Go 的同名表达式：要求目标值是 string，再把 suffix 拼到后面。
fn addSuffix(info: &mappingInfo, mut vals: Vec<Value>) -> ColumnResult<Vec<Value>> {
    let rule = info.rule.as_ref().expect("mappingInfo.rule is required");
    let suffix = &rule.Arguments[0];
    let idx = info.targetPosition as usize;
    let originStr = match vals.get(idx) {
        Some(Value::String(v)) => v.clone(),
        other => {
            return Err(format!(
                "column {} value is not string, but {:?}, which is not valid",
                info.targetPosition, other
            ));
        }
    };

    let mut rawByte = String::with_capacity(suffix.len() + originStr.len());
    rawByte.push_str(&originStr);
    rawByte.push_str(suffix);

    vals[idx] = Value::String(rawByte);
    Ok(vals)
}

// partitionID 对应 Go 的同名表达式：解析目标列原始 ID，拼入 instance/schema/table 位段，再写回原类型形态。
fn partitionID(info: &mappingInfo, mut vals: Vec<Value>) -> ColumnResult<Vec<Value>> {
    // only int64 now
    let idx = info.targetPosition as usize;
    let (mut originID, isChars) = match vals.get(idx) {
        Some(Value::Int(v)) => (*v, false),
        Some(Value::Int8(v)) => (*v as i64, false),
        Some(Value::Int32(v)) => (*v as i64, false),
        Some(Value::Uint(v)) => (*v as i64, false),
        Some(Value::Uint16(v)) => (*v as i64, false),
        Some(Value::Uint32(v)) => (*v as i64, false),
        Some(Value::Uint64(v)) => (*v as i64, false),
        Some(Value::String(v)) => {
            let parsed = v.parse::<i64>().map_err(|_| {
                format!(
                    "column {} value is not int, but {:?}, which is not valid",
                    info.targetPosition, vals[idx]
                )
            })?;
            (parsed, true)
        }
        other => return Err(format!("type {:?} is not valid", other)),
    };

    let max_origin_id = maxOriginID.load(Ordering::SeqCst);
    if originID >= max_origin_id || originID < 0 {
        return Err(format!(
            "id must less than {}, greater than or equal to 0, but got {}, which is not valid",
            max_origin_id, originID
        ));
    }

    originID = info.instanceID | info.schemaID | info.tableID | originID;
    if isChars {
        vals[idx] = Value::String(originID.to_string());
    } else {
        vals[idx] = Value::Int(originID);
    }

    Ok(vals)
}

// computePartitionID 对应 Go 的同名函数，按参数中的 instance/schema/table 规则预计算三段高位 ID。
fn computePartitionID(schema: &str, table: &str, rule: &Rule) -> ColumnResult<(i64, i64, i64)> {
    let mut instanceID = 0_i64;
    let mut schemaID = 0_i64;
    let mut tableID = 0_i64;
    let mut shiftCnt: u32 = 63;

    let instance_bits = instanceIDBitSize.load(Ordering::SeqCst);
    if instance_bits > 0 && !rule.Arguments[0].is_empty() {
        shiftCnt -= instance_bits as u32;
        let instanceIDUnsign = parse_u64_with_bits(&rule.Arguments[0], instance_bits as u32)?;
        instanceID = (instanceIDUnsign << shiftCnt) as i64;
    }

    let sep = &rule.Arguments[3];

    let schema_bits = schemaIDBitSize.load(Ordering::SeqCst);
    if schema_bits > 0 && !rule.Arguments[1].is_empty() {
        shiftCnt -= schema_bits as u32;
        schemaID = computeID(
            schema,
            &rule.Arguments[1],
            sep,
            schema_bits as u32,
            shiftCnt,
        )?;
    }

    let table_bits = tableIDBitSize.load(Ordering::SeqCst);
    if table_bits > 0 && !rule.Arguments[2].is_empty() {
        shiftCnt -= table_bits as u32;
        tableID = computeID(table, &rule.Arguments[2], sep, table_bits as u32, shiftCnt)?;
    }

    Ok((instanceID, schemaID, tableID))
}

// computeID 对应 Go 的后缀 ID 解析：允许 name 刚好等于 prefix，否则要求 prefix+sep 后跟十进制数字。
fn computeID(
    name: &str,
    prefix: &str,
    sep: &str,
    bitSize: u32,
    shiftCount: u32,
) -> ColumnResult<i64> {
    if name == prefix {
        return Ok(0);
    }

    let prefix_with_sep = format!("{}{}", prefix, sep);
    if prefix_with_sep.len() >= name.len() || !name.starts_with(&prefix_with_sep) {
        return Err(format!(
            "{} is not the prefix of {} is not valid",
            prefix_with_sep, name
        ));
    }

    let idStr = &name[prefix_with_sep.len()..];
    let id = parse_u64_with_bits(idStr, bitSize).map_err(|_| {
        format!(
            "the suffix of {} can't be converted to int64 is not valid",
            idStr
        )
    })?;

    Ok((id << shiftCount) as i64)
}

type ExprFn = fn(&mappingInfo, Vec<Value>) -> ColumnResult<Vec<Value>>;

// expr_handler 对应 Go 的 Exprs map 查询，返回表达式处理函数。
fn expr_handler(expr: &Expr) -> Option<ExprFn> {
    match expr {
        Expr::AddPrefix => Some(addPrefix),
        Expr::AddSuffix => Some(addSuffix),
        Expr::PartitionID => Some(partitionID),
        Expr::Other(_) => None,
    }
}

// parse_u64_with_bits 对应 strconv.ParseUint(..., bitSize) 的范围检查。
fn parse_u64_with_bits(input: &str, bitSize: u32) -> ColumnResult<u64> {
    let val = input.parse::<u64>().map_err(|err| err.to_string())?;
    if bitSize < 64 && val >= (1_u64 << bitSize) {
        return Err(format!("value {} overflows {} bits", val, bitSize));
    }
    Ok(val)
}

#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;
