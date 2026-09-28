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

// Executable FTS session slice. SQL is parsed into the canonical AST, DDL
// produces canonical model metadata, planning runs the FTS resolver pipeline,
// and dirty checks inspect the live KV transaction mem-buffer.
//
// 可执行的 FTS（Full-Text Search，全文检索）会话切片。
//
// SQL 解析为规范 AST；DDL 产出规范模型元数据；规划走 FTS resolver 管线；
// dirty 检查查看活跃 KV 事务的 mem-buffer（内存写缓冲，未提交变更）。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::convert::Infallible;

use astersql_config_deploymode as deploymode;
use astersql_ddl::BuildTableInfoWithStmt;
use astersql_ddl::index::BuildCanonicalFullTextIndex;
use astersql_expression::builtin_fts::{self, FtsAgainst, MatchArgument};
use astersql_kv as kv;
use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser as parser;
use astersql_parser_ast as ast;
use astersql_planner_core::{
    BuildFullTextPlan, FtsTableMetadata, FullTextIndex, FullTextPreparedCacheability, PlanNode,
    ResolveFullTextPlan,
};
use astersql_tablecodec as tablecodec;

use crate::runtime::transaction_has_table_prefix;
use crate::{SessionError, SessionResult};

/// PREPARE 后的 FTS 语句缓存项：保留原文与是否允许命中 plan cache。
#[derive(Clone, Debug)]
struct PreparedFtsStatement {
    /// 原始 SQL 文本，EXECUTE 时重新规划。
    sql: String,
    /// 是否满足全文 prepared 的 cacheability（常量 match 文本等）。
    cacheable: bool,
}

/// 一次 FTS 语句执行结果：可选物理计划及是否声称来自 plan cache。
#[derive(Clone, Debug, Default)]
pub struct FtsExecution {
    /// 解析/解析器产出的执行计划节点（SELECT/EXPLAIN 时有值）。
    plan: Option<PlanNode>,
    /// 是否标记为可缓存路径（本切片仍会重建计划，不真正复用缓存）。
    from_plan_cache: bool,
}

impl FtsExecution {
    /// 返回本次执行的计划节点（若有）。
    pub fn plan(&self) -> Option<&PlanNode> {
        self.plan.as_ref()
    }

    /// 是否标记为来自 plan cache。
    pub fn from_plan_cache(&self) -> bool {
        self.from_plan_cache
    }
}

/// 面向 FTS 的轻量会话运行时：内存表元数据 + 可选 KV 事务 + prepared 映射。
pub struct FtsSessionRuntime<S: kv::Storage> {
    /// 底层 KV 存储，用于开启事务与写入行键。
    storage: S,
    /// 逻辑表名（小写）到 `TableInfo` 的映射。
    tables: HashMap<String, model::TableInfo>,
    /// 当前显式事务；Drop 时未提交会回滚。
    transaction: Option<Box<dyn kv::Transaction>>,
    /// PREPARE 名称到语句缓存。
    prepared: HashMap<String, PreparedFtsStatement>,
    /// 下一个可分配的表 ID。
    next_table_id: i64,
}

impl<S: kv::Storage> FtsSessionRuntime<S> {
    /// 用给定存储构造空运行时（无表、无事务）。
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            tables: HashMap::new(),
            transaction: None,
            prepared: HashMap::new(),
            next_table_id: 1,
        }
    }

    /// 解析单条语句为 AST 节点。
    fn parse_one(sql: &str) -> SessionResult<Box<dyn ast::Node>> {
        parser::New()
            .ParseOneStmt(sql, "", "")
            .map_err(|error| SessionError::new(error.to_string()))
    }

    /// 从单表 FROM/INSERT 子句取出表名；多表或子查询不支持。
    fn table_name(table: &ast::TableRefsClause) -> SessionResult<&str> {
        if table.TableRefs.Right.is_some() {
            return Err(SessionError::new("FTS runtime requires one table"));
        }
        match table.TableRefs.Left.as_deref() {
            Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none() => {
                Ok(source.Source.Name.L.as_str())
            }
            _ => Err(SessionError::new("FTS runtime requires one table")),
        }
    }

    /// 递归校验表达式中的 `FTS_MATCH_WORD`：部署模式、常量查询串与列参数。
    fn validate_fts_expression(expression: &ast::ExprNode) -> SessionResult<bool> {
        match &expression.Kind {
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("fts_match_word") =>
            {
                if Args.len() != 2 {
                    return Err(SessionError::new("FTS_MATCH_WORD() requires two arguments"));
                }
                // 将第一参归类为常量字符串 / 非字符串常量 / 非常量，供表达式门禁使用。
                let against = match &Args[0].Kind {
                    ast::ExprKind::Value(value) => match &value.Datum {
                        ast::ValueDatum::String(value) | ast::ValueDatum::Decimal(value) => {
                            FtsAgainst::String(value.clone())
                        }
                        ast::ValueDatum::Bytes(value) => {
                            FtsAgainst::String(String::from_utf8_lossy(value).into_owned())
                        }
                        _ => FtsAgainst::NonStringConstant,
                    },
                    _ => FtsAgainst::NonConstant,
                };
                let column = if matches!(Args[1].Kind, ast::ExprKind::Column(_)) {
                    MatchArgument::StringColumn
                } else {
                    MatchArgument::NonColumn
                };
                builtin_fts::build_match_word(deploymode::IsStarter(), against, &[column])
                    .map_err(|error| SessionError::new(error.to_string()))?;
                Ok(true)
            }
            ast::ExprKind::Function { Args, .. } => {
                let mut found = false;
                for argument in Args {
                    found |= Self::validate_fts_expression(argument)?;
                }
                Ok(found)
            }
            ast::ExprKind::Binary { L, R, .. } => {
                Ok(Self::validate_fts_expression(L)? | Self::validate_fts_expression(R)?)
            }
            ast::ExprKind::Unary { V, .. } | ast::ExprKind::Parentheses(V) => {
                Self::validate_fts_expression(V)
            }
            _ => Ok(false),
        }
    }

    /// 扫描 SELECT 的 WHERE/投影/ORDER BY，确认是否含合法 FTS 表达式。
    fn validate_select_fts(select: &ast::SelectStmt) -> SessionResult<bool> {
        let mut found = false;
        if let Some(condition) = &select.Where {
            found |= Self::validate_fts_expression(condition)?;
        }
        for field in &select.Fields.Fields {
            if let Some(expression) = &field.Expr {
                found |= Self::validate_fts_expression(expression)?;
            }
        }
        for item in &select.OrderBy {
            found |= Self::validate_fts_expression(&item.Expr)?;
        }
        Ok(found)
    }

    /// 从 `TableInfo` 抽取规划器用的 FTS 元数据（列、全文索引、TiFlash 可用性）。
    fn planner_metadata(table: &model::TableInfo) -> FtsTableMetadata {
        // 仅纳入 Public 且带 FullTextInfo 的索引，列名按 Offset 映射回表列。
        let full_text_indexes = table
            .Indices
            .iter()
            .filter(|index| index.IsPublic() && index.FullTextInfo.is_some())
            .map(|index| FullTextIndex {
                name: index.Name.O.clone(),
                columns: index
                    .Columns
                    .iter()
                    .filter_map(|column| {
                        usize::try_from(column.Offset)
                            .ok()
                            .and_then(|offset| table.Columns.get(offset))
                            .map(|column| column.Name.O.clone())
                    })
                    .collect(),
            })
            .collect();
        FtsTableMetadata {
            id: table.ID,
            name: table.Name.O.clone(),
            columns: table
                .Columns
                .iter()
                .map(|column| column.Name.O.clone())
                .collect(),
            full_text_indexes,
            tiflash_available: table
                .TiFlashReplica
                .as_ref()
                .is_some_and(|replica| replica.Count > 0 && replica.Available),
        }
    }

    /// 当前事务 mem-buffer 是否含该表前缀的未提交写（dirty 则拒绝 FTS）。
    fn transaction_has_table_content(&self, table_id: i64) -> SessionResult<bool> {
        let Some(transaction) = self.transaction.as_ref() else {
            return Ok(false);
        };
        transaction_has_table_prefix(transaction.as_ref(), table_id)
    }

    /// 校验并规划全文 SELECT：BuildFullTextPlan + ResolveFullTextPlan。
    fn plan_select(&self, select: &ast::SelectStmt) -> SessionResult<PlanNode> {
        if !Self::validate_select_fts(select)? {
            return Err(SessionError::new("statement is not a full-text query"));
        }
        let table_name = Self::table_name(
            select
                .From
                .as_ref()
                .ok_or_else(|| SessionError::new("FTS query requires FROM"))?,
        )?;
        let table = self
            .tables
            .get(table_name)
            .ok_or_else(|| SessionError::new(format!("table {table_name} was not found")))?;
        let metadata = Self::planner_metadata(table);
        let dirty = self.transaction_has_table_content(table.ID)?;
        let plan = BuildFullTextPlan(select, &metadata).map_err(SessionError::new)?;
        ResolveFullTextPlan(plan, &metadata.full_text_indexes, dirty).map_err(SessionError::new)
    }

    /// CREATE TABLE：FULLTEXT 仅允许 starter 部署模式；分配表 ID 并登记。
    fn create_table(&mut self, statement: &ast::CreateTableStmt) -> SessionResult<()> {
        let has_full_text = statement
            .Constraints
            .iter()
            .any(|constraint| constraint.Tp == ast::ConstraintType::Fulltext);
        if has_full_text && !deploymode::IsStarter() {
            return Err(SessionError::new(
                "FULLTEXT index is only supported in starter deployment mode",
            ));
        }
        let context = metabuild::NewContext::<(), Infallible>(Vec::new());
        let mut table = BuildTableInfoWithStmt(&context, statement, "utf8mb4", "", None)
            .map_err(|error| SessionError::new(error.to_string()))?;
        table.ID = self.next_table_id;
        self.next_table_id += 1;
        let name = table.Name.L.clone();
        if self.tables.insert(name.clone(), table).is_some() {
            return Err(SessionError::new(format!("table {name} already exists")));
        }
        Ok(())
    }

    /// ALTER TABLE：追加规范 FULLTEXT 索引或设置 TiFlash 副本元数据。
    fn alter_table(&mut self, statement: &ast::AlterTableStmt) -> SessionResult<()> {
        let table = self
            .tables
            .get_mut(&statement.Table.Name.L)
            .ok_or_else(|| SessionError::new("table was not found"))?;
        for specification in &statement.Specs {
            match specification.Tp {
                // 通过 DDL 规范路径构建全文索引元数据。
                ast::AlterTableType::AddConstraint
                    if specification.Constraint.as_ref().is_some_and(|constraint| {
                        constraint.Tp == ast::ConstraintType::Fulltext
                    }) =>
                {
                    let constraint = specification
                        .Constraint
                        .as_ref()
                        .expect("FULLTEXT branch requires a constraint");
                    BuildCanonicalFullTextIndex(table, constraint)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                }
                ast::AlterTableType::SetTiFlashReplica => {
                    let replica = specification
                        .TiFlashReplica
                        .as_ref()
                        .ok_or_else(|| SessionError::new("missing TiFlash replica spec"))?;
                    table.TiFlashReplica = Some(model::TiFlashReplicaInfo {
                        Count: replica.Count,
                        LocationLabels: replica.Labels.clone(),
                        Available: replica.Count > 0,
                        AvailablePartitionIDs: Vec::new(),
                    });
                }
                _ => return Err(SessionError::new("unsupported ALTER TABLE operation")),
            }
        }
        Ok(())
    }

    /// 在活跃事务中写入行键（桩值），用于模拟未提交 mem-buffer 脏数据。
    fn insert(&mut self, statement: &ast::InsertStmt) -> SessionResult<()> {
        let table_name = Self::table_name(
            statement
                .Table
                .as_ref()
                .ok_or_else(|| SessionError::new("INSERT has no table"))?,
        )?;
        let table_id = self
            .tables
            .get(table_name)
            .map(|table| table.ID)
            .ok_or_else(|| SessionError::new("table was not found"))?;
        let transaction = self.transaction.as_mut().ok_or_else(|| {
            SessionError::new("FTS runtime INSERT requires an active transaction")
        })?;
        for row in &statement.Lists {
            // 首列作为 handle，编码为 tablecodec 行键写入事务。
            let handle = match row.first().map(|expression| &expression.Kind) {
                Some(ast::ExprKind::Value(value)) => match value.Datum {
                    ast::ValueDatum::Int64(value) => value,
                    ast::ValueDatum::Uint64(value) => i64::try_from(value)
                        .map_err(|_| SessionError::new("row handle overflows int64"))?,
                    _ => return Err(SessionError::new("row handle must be an integer")),
                },
                _ => return Err(SessionError::new("INSERT row requires a handle")),
            };
            transaction
                .Set(
                    kv::Key(
                        tablecodec::EncodeRowKeyWithHandle(
                            table_id,
                            Box::new(tablecodec::kv::IntHandle(handle)),
                        )
                        .0,
                    ),
                    b"fts-row".to_vec(),
                )
                .map_err(|error| SessionError::new(error.to_string()))?;
        }
        Ok(())
    }

    /// 执行一条 SQL：DDL/事务/INSERT 或规划全文 SELECT（可选 EXPLAIN 前缀）。
    pub fn execute(&mut self, sql: &str) -> SessionResult<FtsExecution> {
        let trimmed = sql.trim().trim_end_matches(';').trim();
        // 剥离可选 EXPLAIN 前缀，真正语句再交给解析器。
        let (explain, statement_sql) = if trimmed
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("explain"))
        {
            (true, trimmed[7..].trim())
        } else {
            (false, trimmed)
        };
        let statement = Self::parse_one(statement_sql)?;
        // 按 AST 类型分发：DDL / 事务边界 / INSERT / SELECT 规划。
        if let Some(create) = statement.as_any().downcast_ref::<ast::CreateTableStmt>() {
            self.create_table(create)?;
        } else if let Some(alter) = statement.as_any().downcast_ref::<ast::AlterTableStmt>() {
            self.alter_table(alter)?;
        } else if statement.as_any().is::<ast::BeginStmt>() {
            if self.transaction.is_some() {
                return Err(SessionError::new("transaction already active"));
            }
            self.transaction = Some(
                self.storage
                    .Begin(&[])
                    .map_err(|error| SessionError::new(error.to_string()))?,
            );
        } else if statement.as_any().is::<ast::RollbackStmt>() {
            let mut transaction = self
                .transaction
                .take()
                .ok_or_else(|| SessionError::new("no active transaction"))?;
            transaction
                .Rollback()
                .map_err(|error| SessionError::new(error.to_string()))?;
        } else if statement.as_any().is::<ast::CommitStmt>() {
            let mut transaction = self
                .transaction
                .take()
                .ok_or_else(|| SessionError::new("no active transaction"))?;
            transaction
                .Commit(&kv::Context::default())
                .map_err(|error| SessionError::new(error.to_string()))?;
        } else if let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() {
            self.insert(insert)?;
        } else if let Some(select) = statement.as_any().downcast_ref::<ast::SelectStmt>() {
            let plan = self.plan_select(select)?;
            return Ok(FtsExecution {
                plan: Some(plan),
                from_plan_cache: false,
            });
        } else {
            return Err(SessionError::new("unsupported FTS runtime statement"));
        }
        let _ = explain;
        Ok(FtsExecution::default())
    }

    /// PREPARE：校验可规划后登记名称；记录 cacheability 标志。
    pub fn prepare(&mut self, name: &str, sql: &str) -> SessionResult<()> {
        let statement = Self::parse_one(sql)?;
        let select = statement
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .ok_or_else(|| SessionError::new("FTS prepare requires SELECT"))?;
        self.plan_select(select)?;
        let (cacheable, _) = FullTextPreparedCacheability(select);
        self.prepared.insert(
            name.to_owned(),
            PreparedFtsStatement {
                sql: sql.to_owned(),
                cacheable,
            },
        );
        Ok(())
    }

    /// EXECUTE 已 PREPARE 的语句：始终重新 `execute`，再贴上 cacheability 标记。
    pub fn execute_prepared(&mut self, name: &str) -> SessionResult<FtsExecution> {
        let prepared =
            self.prepared.get(name).cloned().ok_or_else(|| {
                SessionError::new(format!("prepared statement {name} was not found"))
            })?;
        let mut execution = self.execute(&prepared.sql)?;
        // FTS 路径不真正命中 plan cache，仅回传 prepared 时的 cacheable 判定。
        execution.from_plan_cache = prepared.cacheable;
        Ok(execution)
    }

    /// DEALLOCATE 移除已登记的 prepared 名称。
    pub fn deallocate(&mut self, name: &str) -> SessionResult<()> {
        self.prepared
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| SessionError::new(format!("prepared statement {name} was not found")))
    }
}

impl<S: kv::Storage> Drop for FtsSessionRuntime<S> {
    fn drop(&mut self) {
        // 运行时销毁时回滚残留事务，避免泄漏未提交写。
        if let Some(mut transaction) = self.transaction.take() {
            let _ = transaction.Rollback();
        }
    }
}
