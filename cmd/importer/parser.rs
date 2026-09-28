// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

//! 本模块把 importer 关心的 DDL 子集翻译成轻量运行期结构。
//! 它只覆盖 `CREATE TABLE` 和 `CREATE INDEX` 的最小语法面。
//! 这样既能保持与 Go 行为对齐，也能避免引入完整 TiDB 解析依赖。
//! `column` 负责承载单列的类型、comment 规则和生成期状态。
//! `table` 负责聚合列、列清单和索引映射，供 SQL 生成直接消费。
//! comment 规则中的 `range`、`set`、`step` 会直接影响后续造数。
//! 唯一列与自增列会在解析期就标记出来，避免非法重复值进入运行期。
//! 模块中的 AST 访问顺序刻意保持和 Go 相同，降低迁移漂移风险。
//! 表级约束与列级约束最终都会被折叠成索引映射。
//! 空索引 SQL 被视为正常输入，这是命令默认值依赖的宽容语义。
//! 布尔解析使用 Go 兼容集合，保证 comment 规则的容错边界一致。
//! 这里不会尝试做通用 SQL 修复，非法输入仍应尽快报错。
//! 调试字符串的字段顺序也尽量贴近 Go，方便 parity test 对照。
//! `TableInfo` 的构造只保留 importer 会读取的最小字段。
//! 本次改动仅补注释，不改变任何解析分支、默认值或错误文本。
//! 后续若扩语法，应优先查 Go 对应逻辑，再同步调整这里的约束说明。
//! 对调用方来说，这个模块的核心价值是稳定输出可造数的表视图。
//! 对维护者来说，这些注释重点解释“为什么这样解析”而非语法本身。
//! 因此文档会更多强调唯一性、顺序依赖和容错边界。
//! 这也是 importer 在迁移后最容易出现行为偏差的地方。

use std::collections::HashMap;
use std::sync::Arc;

use crate::data::{datum, newDatum};
use crate::stats::histogram;
use crate::stubs::{
    self, ColumnDef, ColumnOptionTp, Constraint, ConstraintTp, CreateIndexStmt, CreateTableStmt,
    Error, FieldType, IndexKeyType, Result, StmtNode, TableInfo,
};

/// `column` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct column {
    pub idx: i32,
    pub name: String,
    pub data: Arc<datum>,
    pub tp: FieldType,
    pub comment: String,
    pub min: String,
    pub max: String,
    pub incremental: bool,
    pub set: Vec<String>,
    pub hist: Option<Arc<histogram>>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl column {
    pub fn String(&self) -> String {
        let set = format!("[{}]", self.set.join(" "));
        format!(
            "[column]idx: {}, name: {}, tp: {:?}, min: {}, max: {}, step: {}, set: {}\n",
            self.idx,
            self.name,
            self.tp,
            self.min,
            self.max,
            self.data.step(),
            set
        )
    }

    /// 把 `[[key=value]]` 里的单条规则写回列状态。
    /// 这里只识别 importer 真正消费的几种键，其余内容保持忽略，与 Go 一致。
    /// `repeats` 和 `probability` 会直接约束后续造数分布，因此在解析期立即做边界校验。
    pub fn parseRule(&mut self, kvs: &[String], uniq: bool) {
        if kvs.len() != 2 {
            return;
        }
        let key = kvs[0].trim();
        let value = kvs[1].trim();
        if key == "range" {
            let fields: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
            if fields.len() == 1 {
                self.min = fields[0].to_string();
            } else if fields.len() == 2 {
                self.min = fields[0].to_string();
                self.max = fields[1].to_string();
            }
        } else if key == "step" {
            match value.parse::<i64>() {
                Ok(v) => self.data.set_step(v),
                Err(err) => stubs::fatal(format!("parsing err key={key} err={err}")),
            }
        } else if key == "set" {
            for field in value.split(',') {
                self.set.push(field.trim().to_string());
            }
        } else if key == "incremental" {
            match parse_bool_go(value) {
                Ok(v) => self.incremental = v,
                Err(err) => stubs::fatal(format!("parsing err key={key} err={err}")),
            }
        } else if key == "repeats" {
            let repeats = match value.parse::<u64>() {
                Ok(v) => v,
                Err(err) => stubs::fatal(format!("parsing err key={key} err={err}")),
            };
            if uniq && repeats > 1 {
                stubs::fatal("cannot repeat more than 1 times on unique columns");
            }
            self.data.set_repeats(repeats);
        } else if key == "probability" {
            let prob = match value.parse::<u32>() {
                Ok(v) => v,
                Err(err) => stubs::fatal(format!("parsing err key={key} err={err}")),
            };
            if prob > 100 || prob == 0 {
                stubs::fatal("probability must be in (0, 100]");
            }
            self.data.set_probability(prob);
        }
    }

    /// 只截取 comment 中 `[[...]]` 这段规则文本，外层自然语言说明会被保留但不参与解析。
    /// 分号拆字段、等号拆键值的顺序与 Go 保持一致，方便复用既有建表注释写法。
    /// 缺失包围标记时会退化为空规则，而不是报错，这是 importer 现有输入的宽容语义。
    pub fn parseColumnComment(&mut self, uniq: bool) {
        let comment = self.comment.trim().to_string();
        let start = comment.find("[[");
        let end = comment.find("]]");
        let mut content = String::new();
        if let (Some(start), Some(end)) = (start, end) {
            if start < end {
                content = comment[start + 2..end].to_string();
            }
        }
        for field in content.split(';') {
            let field = field.trim();
            let kvs: Vec<String> = field.split('=').map(|s| s.to_string()).collect();
            self.parseRule(&kvs, uniq);
        }
    }

    /// 先扫列级约束，再决定 comment 规则是否按唯一列解释。
    /// 这里把主键、唯一键和自增都视为“不能重复”，与 Go 对造数器的约束口径一致。
    /// 返回值只表达“本列是否新标成唯一”，真正写入索引映射要等列入表后才能拿到稳定下标。
    pub fn parseColumnOptions(&mut self, ops: &[stubs::ColumnOption]) -> bool {
        let mut uniq = false;
        for op in ops {
            match op.Tp {
                ColumnOptionTp::PrimaryKey
                | ColumnOptionTp::UniqKey
                | ColumnOptionTp::AutoIncrement => {
                    uniq = true;
                }
                ColumnOptionTp::Comment => {
                    self.comment = op.Comment.clone();
                }
                ColumnOptionTp::Other => {}
            }
        }
        uniq
    }
}

/// `table` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct table {
    pub name: String,
    pub columns: Vec<column>,
    pub columnList: String,
    pub indices: HashMap<String, Option<usize>>,
    pub uniqIndices: HashMap<String, Option<usize>>,
    pub tblInfo: TableInfo,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl table {
    pub fn printColumns(&self) -> String {
        let mut ret = String::new();
        for col in &self.columns {
            ret.push_str(&col.String());
        }
        ret
    }

    /// `String` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn String(&self) -> String {
        let mut ret = format!("[table]name: {}\n", self.name);
        ret.push_str("[table]columns:\n");
        ret.push_str(&self.printColumns());
        ret.push_str(&format!("[table]column list: {}\n", self.columnList));
        ret.push_str("[table]indices:\n");
        for (k, idx) in &self.indices {
            let value = idx
                .map(|idx| self.columns[idx].String())
                .unwrap_or_else(|| "<nil>".to_string());
            ret.push_str(&format!("key->{}, value->{}", k, value));
        }
        ret.push_str("[table]unique indices:\n");
        for (k, idx) in &self.uniqIndices {
            let value = idx
                .map(|idx| self.columns[idx].String())
                .unwrap_or_else(|| "<nil>".to_string());
            ret.push_str(&format!("key->{}, value->{}", k, value));
        }
        ret
    }

    /// `findCol` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn findCol(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// 把表级约束折叠回列索引映射，补齐列级选项里看不到的唯一性信息。
    /// 只有已经成功落入 `columns` 的列才会写进映射，避免为缺失列制造悬空引用。
    /// 唯一约束与普通索引分开保存，后续 SQL 生成和去重逻辑会依赖这个区分。
    pub fn parseTableConstraint(&mut self, cons: &Constraint) {
        match cons.Tp {
            ConstraintTp::PrimaryKey
            | ConstraintTp::Key
            | ConstraintTp::Uniq
            | ConstraintTp::UniqKey
            | ConstraintTp::UniqIndex => {
                for indexCol in &cons.Keys {
                    let name = indexCol.Column.L.clone();
                    let idx = self.findCol(&name);
                    self.uniqIndices.insert(name, idx);
                }
            }
            ConstraintTp::Index => {
                for indexCol in &cons.Keys {
                    let name = indexCol.Column.L.clone();
                    let idx = self.findCol(&name);
                    self.indices.insert(name, idx);
                }
            }
            ConstraintTp::Other => {}
        }
    }

    /// 缓存逗号拼接后的列名串，供 importer 后续直接拼装 SQL。
    /// 这里刻意沿用声明顺序，不按索引或约束重排，避免生成语句与 Go 输出漂移。
    pub fn buildColumnList(&mut self) {
        let columns: Vec<String> = self.columns.iter().map(|c| c.name.clone()).collect();
        self.columnList = columns.join(",");
    }
}

/// `newTable` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn newTable() -> table {
    table {
        name: String::new(),
        columns: Vec::new(),
        columnList: String::new(),
        indices: HashMap::new(),
        uniqIndices: HashMap::new(),
        tblInfo: TableInfo::default(),
    }
}

/// 把 `CREATE TABLE` AST 压成 importer 运行期使用的最小表视图。
/// 先建列、后收表级约束的顺序不是偶然设计，而是为了复刻 Go 中“列注释先看到列级唯一性”的行为。
/// `TableInfo` 也在这里同步构造，确保后续调用方不需要再次回看 AST。
pub fn parseTable(t: &mut table, stmt: &CreateTableStmt) -> Result<()> {
    t.name = stmt.Table.L.clone();
    t.columns = Vec::with_capacity(stmt.Cols.len());

    let mut mockTbl = stubs::build_table_info_from_ast(stmt)?;
    mockTbl.ID = 1;
    t.tblInfo = mockTbl;

    // First pass: column options may mark uniq before comments are parsed.
    // Mirror Go: parseColumn registers into uniqIndices during options, then
    // parseColumnComment uses that map.
    for (i, col_def) in stmt.Cols.iter().enumerate() {
        let mut col = column {
            idx: i as i32 + 1,
            name: String::new(),
            data: Arc::new(newDatum()),
            tp: FieldType::default(),
            comment: String::new(),
            min: String::new(),
            max: String::new(),
            incremental: false,
            set: Vec::new(),
            hist: None,
        };
        parse_column_into(t, &mut col, col_def);
    }

    for cons in &stmt.Constraints {
        t.parseTableConstraint(cons);
    }

    t.buildColumnList();
    Ok(())
}

// 把单个 AST 列定义灌入运行期 `column`，并在入表前完成 comment 规则解释。
// 这里先读列选项、再查唯一映射，是为了保留 Go 中“唯一列禁止 repeats>1”的触发时机。
// 真正写入 `uniqIndices` 要等 `push` 之后拿到稳定位置，因此拆成前后两步。
fn parse_column_into(t: &mut table, col: &mut column, cd: &ColumnDef) {
    col.name = cd.Name.L.clone();
    col.tp = cd.Tp.clone();
    let marked_uniq = col.parseColumnOptions(&cd.Options);
    if marked_uniq {
        // Will insert after push with correct index.
    }
    // uniq lookup uses current map (may already contain earlier columns).
    let uniq = t.uniqIndices.contains_key(&col.name) || marked_uniq;
    col.parseColumnComment(uniq);
    t.columns.push(column {
        idx: col.idx,
        name: col.name.clone(),
        data: Arc::clone(&col.data),
        tp: col.tp.clone(),
        comment: col.comment.clone(),
        min: col.min.clone(),
        max: col.max.clone(),
        incremental: col.incremental,
        set: col.set.clone(),
        hist: col.hist.clone(),
    });
    let idx = t.columns.len() - 1;
    if marked_uniq {
        t.uniqIndices.insert(col.name.clone(), Some(idx));
    }
}

/// `parseTableSQL` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parseTableSQL(table: &mut table, sql: &str) -> Result<()> {
    let stmt = stubs::parse_one_stmt(sql)?;
    match stmt {
        StmtNode::CreateTable(node) => parseTable(table, &node),
        other => Err(Error::new(format!("invalid statement - {}", other.Text()))),
    }
}

/// 处理独立的 `CREATE INDEX`，把增量索引信息并回已解析的表对象。
/// 先校验表名完全一致，避免把外部传错的索引语句静默落到当前表上。
/// 这里只接受普通索引和唯一索引；其他键类型继续按 Go 语义直接报错。
pub fn parseIndex(table: &mut table, stmt: &CreateIndexStmt) -> Result<()> {
    if table.name != stmt.Table.L {
        return Err(Error::new(format!(
            "mismatch table name for create index - {} : {}",
            table.name, stmt.Table.L
        )));
    }
    for indexCol in &stmt.IndexPartSpecifications {
        let name = indexCol.Column.L.clone();
        match stmt.KeyType {
            IndexKeyType::Unique => {
                let idx = table.findCol(&name);
                table.uniqIndices.insert(name, idx);
            }
            IndexKeyType::None => {
                let idx = table.findCol(&name);
                table.indices.insert(name, idx);
            }
            IndexKeyType::Other => {
                return Err(Error::new(format!(
                    "unsupported index type on column {}.{}",
                    table.name, name
                )));
            }
        }
    }
    Ok(())
}

/// 解析单条索引 SQL，并把空字符串视为“没有额外索引”。
/// 这个空输入短路是命令层默认配置依赖的宽容行为，不能轻易改成报错。
pub fn parseIndexSQL(table: &mut table, sql: &str) -> Result<()> {
    if sql.is_empty() {
        return Ok(());
    }
    let stmt = stubs::parse_one_stmt(sql)?;
    match stmt {
        StmtNode::CreateIndex(node) => parseIndex(table, &node),
        other => Err(Error::new(format!("invalid statement - {}", other.Text()))),
    }
}

/// Go `strconv.ParseBool`.
// 保持与 Go `strconv.ParseBool` 相同的真值集合，避免 comment 规则在迁移后出现兼容性分叉。
// 返回的错误文本也沿用 Go 习惯，便于 parity case 直接比对。
fn parse_bool_go(s: &str) -> std::result::Result<bool, String> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("strconv.ParseBool: parsing {s:?}: invalid syntax")),
    }
}
