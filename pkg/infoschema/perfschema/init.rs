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

// PERFORMANCE_SCHEMA 虚拟库初始化与轻量 DDL 解析。
//
// 在表达式求值器（EvalSimpleAst）就绪后，解析 `perfSchemaTables` 中的静态建表 SQL，
// 分配稳定表 ID 并注册到进程内虚拟库列表。DDL：数据定义语言；虚拟库无物理存储。

/*
// parser、ddl、model、infoschema 等 Go 依赖保留调用形状，后续跨文件接线时再替换为 Rust 实现。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::Once;

// once 对应 Go 包级 sync.Once，确保 Init 中的注册逻辑最多执行一次。
pub static once: Once = Once::new();

// Init register the PERFORMANCE_SCHEMA virtual tables.
// It should be init(), and the ideal usage should be:
// import _ "github.com/pingcap/tidb/perfschema"
// This function depends on plan/core.init(), which initialize the expression.EvalSimpleAst function.
// The initialize order is a problem if init() is used as the function name.
// Init 对应 Go 的同名入口：在 EvalSimpleAst 初始化后解析静态 DDL，构造 DBInfo 并注册虚拟表。
// Once 闭包中的 panic、表 ID 赋值和列 ID 重排均保留 Go 顺序；这里不会实际执行注册副作用。
pub fn Init() {
    let initOnce = || {
        let p = parser::New();
        let mut tbls: Vec<model::TableInfo> = Vec::new();
        let dbID = autoid::PerformanceSchemaDBID;
        let ctx = metabuild::NewNonStrictContext();

        for sql in perfSchemaTables.iter() {
            // Go 逐条解析 performance_schema 建表 SQL；解析失败属于启动期致命错误，原实现直接 panic。
            let stmt = match p.ParseOneStmt(sql, "", "") {
                Ok(stmt) => stmt,
                Err(err) => panic!("{:?}", err),
            };
            let mut meta = match ddl::BuildTableInfoFromAST(ctx, stmt.downcast::<ast::CreateTableStmt>()) {
                Ok(meta) => meta,
                Err(err) => panic!("{:?}", err),
            };

            let Some(table_id) = tableIDMap().get(meta.Name.O.as_str()).copied() else {
                // 未登记的系统表表示 const.rs 与 tables.rs 的静态清单不一致，沿用 Go 的 panic 语义。
                panic!("get performance_schema table id failed, unknown system table `{}`", meta.Name.O);
            };
            meta.ID = table_id;
            for (i, c) in meta.Columns.iter_mut().enumerate() {
                // Go 列 ID 从 1 开始，offset 顺序来自 DDL 解析结果。
                c.ID = i as i64 + 1;
            }
            meta.DBID = dbID;
            meta.State = model::StatePublic;
            // Go 的 tbls 保存 *TableInfo，后面对 meta 的修改会透传；在字段补齐后再推入 Vec。
            tbls.push(meta);
        }

        let mut dbInfo = model::DBInfo {
            ID: dbID,
            Name: metadef::PerformanceSchemaName,
            Charset: mysql::DefaultCharset.to_owned(),
            Collate: mysql::DefaultCollationName.to_owned(),
            ..Default::default()
        };
        dbInfo.Deprecated.Tables = tbls;
        infoschema::RegisterVirtualTable(dbInfo, tableFromMeta);
    };

    if expression::EvalSimpleAst.is_some() {
        // Go sync.Once 在表达式求值器可用后才触发，避免初始化顺序早于 plan/core。
        once.call_once(initOnce);
    }
}
*/

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, Once};

use crate::consts::perfSchemaTables;
use crate::tables::{ColumnInfo, IndexInfo, PERFORMANCE_SCHEMA_DB_ID, TABLE_ID_MAP, TableMeta};

/// 已注册的虚拟库元数据：库 ID、名、字符集与下属表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualDatabase {
    pub id: i64,
    pub name: String,
    pub charset: String,
    pub collation: String,
    pub tables: Vec<TableMeta>,
}

/// 初始化/解析阶段错误：非法 CREATE、未知表或重复注册。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitError {
    /// CREATE TABLE SQL 无法被轻量解析器识别。
    InvalidCreateSql(String),
    /// 表名不在 TABLE_ID_MAP 中，表示 const 与 tables 清单不一致。
    UnknownTable(String),
    /// 同名或同 ID 库已注册。
    DuplicateDatabase,
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InitError {}

/// 对应 Go sync.Once：注册逻辑最多执行一次。
static INIT_ONCE: Once = Once::new();
/// 标记 plan/core 是否已初始化 EvalSimpleAst；未就绪则跳过 Init。
static EVAL_SIMPLE_AST_READY: AtomicBool = AtomicBool::new(false);
/// 进程内已注册虚拟库列表（含 PERFORMANCE_SCHEMA）。
static VIRTUAL_DATABASES: LazyLock<Mutex<Vec<VirtualDatabase>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// 设置表达式求值器就绪标志；测试与启动路径在依赖就绪后调用。
pub fn set_eval_simple_ast_ready(ready: bool) {
    EVAL_SIMPLE_AST_READY.store(ready, Ordering::Release);
}

/// 返回当前已注册虚拟库快照。
pub fn registered_databases() -> Vec<VirtualDatabase> {
    VIRTUAL_DATABASES
        .lock()
        .expect("performance schema registry lock poisoned")
        .clone()
}

/// 在 EvalSimpleAst 就绪后一次性构建并注册 PERFORMANCE_SCHEMA。
pub fn init() {
    if !EVAL_SIMPLE_AST_READY.load(Ordering::Acquire) {
        return;
    }
    INIT_ONCE.call_once(|| {
        let database = build_performance_schema().unwrap_or_else(|error| panic!("{error}"));
        let mut registry = VIRTUAL_DATABASES
            .lock()
            .expect("performance schema registry lock poisoned");
        // 同 ID 或同名（忽略大小写）视为重复注册，沿用 Go panic 语义。
        if registry
            .iter()
            .any(|old| old.id == database.id || old.name.eq_ignore_ascii_case(&database.name))
        {
            panic!("performance_schema already registered");
        }
        registry.push(database);
    });
}

/// Go 风格别名，对应 `perfschema.Init()`。
#[allow(non_snake_case)]
pub fn Init() {
    init();
}

/// 解析全部静态建表 SQL，赋 ID/列偏移后组装 VirtualDatabase。
pub fn build_performance_schema() -> Result<VirtualDatabase, InitError> {
    let mut tables = Vec::with_capacity(perfSchemaTables.len());
    for sql in perfSchemaTables.iter() {
        let mut table = parse_create_table(sql)?;
        table.id = *TABLE_ID_MAP
            .get(table.name.to_ascii_lowercase().as_str())
            .ok_or_else(|| InitError::UnknownTable(table.name.clone()))?;
        table.database_id = PERFORMANCE_SCHEMA_DB_ID;
        table.public = true;
        // Go 列 ID 从 1 开始，offset 与 DDL 列顺序一致。
        for (offset, column) in table.columns.iter_mut().enumerate() {
            column.id = offset as i64 + 1;
            column.offset = offset;
        }
        tables.push(table);
    }
    Ok(VirtualDatabase {
        id: PERFORMANCE_SCHEMA_DB_ID,
        name: "PERFORMANCE_SCHEMA".to_string(),
        charset: "utf8mb4".to_string(),
        collation: "utf8mb4_bin".to_string(),
        tables,
    })
}

/// 轻量解析 `CREATE TABLE ... (cols, keys)`，不依赖完整 SQL parser。
pub fn parse_create_table(sql: &str) -> Result<TableMeta, InitError> {
    let normalized = sql.trim().trim_end_matches(';');
    let open = normalized
        .find('(')
        .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))?;
    let close = normalized
        .rfind(')')
        .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))?;
    if close <= open {
        return Err(InitError::InvalidCreateSql(sql.to_string()));
    }
    let header = normalized[..open].trim();
    // 取 CREATE 头最后一个 token 作为表名，去掉库前缀与反引号。
    let table_token = header
        .split_whitespace()
        .last()
        .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))?;
    let name = table_token
        .rsplit('.')
        .next()
        .unwrap_or(table_token)
        .trim_matches('`')
        .to_ascii_lowercase();
    if name.is_empty() {
        return Err(InitError::InvalidCreateSql(sql.to_string()));
    }
    let definitions = split_definitions(&normalized[open + 1..close]);
    let mut columns = Vec::new();
    let mut indices = Vec::new();
    for definition in definitions {
        let definition = definition.trim();
        if definition.is_empty() {
            continue;
        }
        let upper = definition.to_ascii_uppercase();
        // PRIMARY/KEY/UNIQUE KEY 行解析为 IndexInfo，列偏移回查已解析列。
        if upper.starts_with("PRIMARY KEY")
            || upper.starts_with("KEY ")
            || upper.starts_with("UNIQUE KEY")
        {
            let left = definition
                .find('(')
                .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))?;
            let right = definition
                .rfind(')')
                .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))?;
            let raw_names: Vec<String> = definition[left + 1..right]
                .split(',')
                .map(|name| name.trim().trim_matches('`').to_string())
                .collect();
            let offsets = raw_names
                .iter()
                .map(|name| {
                    columns
                        .iter()
                        .position(|column: &ColumnInfo| column.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let index_name = if upper.starts_with("PRIMARY") {
                "PRIMARY".to_string()
            } else {
                let prefix_tokens: Vec<&str> = definition[..left].split_whitespace().collect();
                let explicit_name = prefix_tokens
                    .iter()
                    .position(|token| token.eq_ignore_ascii_case("KEY"))
                    .and_then(|key| prefix_tokens.get(key + 1));
                explicit_name
                    .map(|name| name.trim_matches('`').to_string())
                    .or_else(|| raw_names.first().cloned())
                    .unwrap_or_else(|| format!("idx_{}", indices.len() + 1))
            };
            indices.push(IndexInfo {
                id: indices.len() as i64 + 1,
                name: index_name,
                columns: offsets,
                public: true,
            });
            continue;
        }
        let column_name = definition
            .split_whitespace()
            .next()
            .ok_or_else(|| InitError::InvalidCreateSql(sql.to_string()))?
            .trim_matches('`');
        columns.push(ColumnInfo {
            id: columns.len() as i64 + 1,
            name: column_name.to_ascii_lowercase(),
            offset: columns.len(),
            hidden: false,
        });
    }
    Ok(TableMeta {
        id: 0,
        database_id: 0,
        name,
        columns,
        indices,
        public: false,
        create_sql: sql.to_string(),
    })
}

/// 按顶层逗号切分列/索引定义，忽略括号与引号内的逗号。
fn split_definitions(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_u32;
    let mut quote = None;
    for (offset, character) in body.char_indices() {
        if let Some(active) = quote {
            if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&body[start..offset]);
                start = offset + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
}
