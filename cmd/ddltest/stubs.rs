// Copyright 2026 AsterSQL.
//! Local stand-ins for TiKV / session / domain / MySQL / process boundaries
//! (darwin arm64: no kv / domain / kvproto / grpcio).
//!
//! In-memory SQL+DDL harness preserving Go `ddltest` control flow:
//! multi-server routing, retryable exec, concurrent DDL windows, lease wait,
//! and table-iteration assertions.
//!
//! 中文概述：本模块不是 TiDB DDL 的通用实现，
//! 而是专门给 `cmd/ddltest` 迁移测试使用的本地桩。
//! 它把 Go 版本依赖的 TiKV、domain、MySQL 驱动和多进程 server
//! 折叠成单进程内存模型，只保留测试真正观察到的行为。
//! 这里的重点不是 SQL 兼容度，而是让列变更、索引变更、
//! server 重启窗口、租约等待和结果断言仍按 Go 用例的节奏运转。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Flags (Go package-level flag vars)
// ---------------------------------------------------------------------------
// 这些常量对应 Go 包级 flag 的默认值。
// Rust 侧不会真的去解析命令行，
// 但测试代码仍通过这些公开符号读取“环境设定”。
// 因此这里保留名字和大致语义，方便测试文件最小改动迁移。

/// 模拟 Go `-etcd` 默认值；本地桩不会真正连接 etcd，只保留展示用途。
pub static ETCD: &str = "127.0.0.1:2379";
/// 模拟 TiDB server 对外监听的主机名，供伪 server 地址拼接使用。
pub static TIDB_IP: &str = "127.0.0.1";
/// 模拟 Go `-tikv_path`；当前桩不访问外部 TiKV，可为空字符串。
pub static TIKV_PATH: &str = "";
/// Schema lease scaled to stub-milliseconds (Go uses seconds).
/// 为了让测试可快速完成，这里的租约只保留相对等待关系，
/// 不追求与真实秒级时间完全一致。
pub static LEASE: i32 = 5;
/// 伪 server 数量；用于模拟随机路由和重启，而不是启动真实进程。
pub static SERVER_NUM: i32 = 3;
/// 伪 server 的起始端口；仅用于形成稳定的地址字符串。
pub static START_PORT: i32 = 5000;
/// 保留 Go 中状态端口的概念，当前桩本身不暴露独立状态接口。
pub static STATUS_PORT: i32 = 8000;
/// 测试入口会检查日志级别已配置，因此这里保留一个非空默认值。
pub static LOG_LEVEL: &str = "error";
/// 对齐 Go 中 DDL server 的日志级别常量，方便测试引用。
pub static DDL_SERVER_LOG_LEVEL: &str = "fatal";
/// 生成测试数据时使用的基础规模，保持与 Go 用例常见取值接近。
pub static DATA_NUM: i32 = 100;
/// 是否开启随机重启窗口；许多容错分支都依赖这个开关来模拟 Go 场景。
pub static ENABLE_RESTART: bool = true;

// ---------------------------------------------------------------------------
// Random helpers (Go random_test.go)
// ---------------------------------------------------------------------------
// Go 原测试会频繁依赖随机数据制造并发扰动。
// 这里不要求与 Go 的随机序列逐项一致，
// 只要求分布合理、调用开销低，并且在当前进程内可持续产出随机值。

/// 进程内共享的伪随机状态。
/// 使用原子值是为了在多线程测试里避免额外锁竞争。
static RAND_STATE: AtomicU64 = AtomicU64::new(0x4d595df4d0f33173);

/// 生成下一个 `u64` 随机值。
/// 这里采用轻量的 xorshift 风格算法，
/// 目的是服务测试扰动，而非提供密码学安全性。
fn next_u64() -> u64 {
    let mut x = RAND_STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = 0x4d595df4d0f33173;
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RAND_STATE.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

/// 返回一个非负随机整数。
/// Go 侧很多 helper 只关心“值会变化”，因此这里维持同类契约即可。
pub fn random_int() -> i32 {
    (next_u64() >> 33) as i32
}

/// 返回 `[0, n)` 范围内的随机整数。
/// 与 Go `rand.Intn` 一样，`n <= 0` 属于调用错误并触发 panic。
pub fn random_intn(n: i32) -> i32 {
    assert!(n > 0, "invalid argument to random_intn: {n}");
    (next_u64() % n as u64) as i32
}

/// 生成 `[0, 1)` 分布的浮点随机数。
/// 主要用于索引并发测试里的 `double` 列写入。
pub fn random_float() -> f64 {
    // Use the high 53 bits, matching the precision and half-open range of
    // Go's rand.Float64: the result can be zero but never reaches one.
    ((next_u64() >> 11) as f64) * (1.0 / ((1_u64 << 53) as f64))
}

/// 构造定长字母数字串。
/// 这足以覆盖 Go 用例里对 `varchar` 列的随机写入需要。
pub fn random_string(n: i32) -> String {
    const ALPHANUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    assert!(n >= 0, "negative random string length: {n}");
    let n = n as usize;
    let mut bytes = vec![0u8; n];
    for b in &mut bytes {
        *b = ALPHANUM[random_intn(ALPHANUM.len() as i32) as usize];
    }
    String::from_utf8(bytes).unwrap()
}

/// 对齐 Go 的变参随机 helper。
/// 两个参数表示区间，一个参数表示上界，零参数表示任意整数。
pub fn random_num(args: &[i32]) -> i32 {
    if args.len() > 1 {
        args[0] + random_intn(args[1] - args[0])
    } else if args.len() == 1 {
        random_intn(args[0])
    } else {
        random_int()
    }
}

// ---------------------------------------------------------------------------
// Datum / schema
// ---------------------------------------------------------------------------
// 这部分只建模 `ddltest` 会观察到的最小 schema/row 形态。
// 它不尝试复刻 TiDB 全量类型系统，
// 而是只保留整数、浮点、字符串和空值这几种测试实际使用的值。

/// 测试桩中的最小单元值表示。
/// 只覆盖当前 DDL/查询测试真正会写入和比较的几类数据。
#[derive(Clone, Debug)]
pub enum Datum {
    Int(i64),
    Float(f64),
    Str(String),
    Null,
}

impl Datum {
    /// 按 Go 测试常见路径把当前值转成 `i64`。
    /// 非整数输入采用宽松转换，方便主键/where 解析复用。
    pub fn get_int64(&self) -> i64 {
        match self {
            Datum::Int(v) => *v,
            Datum::Float(v) => *v as i64,
            Datum::Str(s) => s.parse().unwrap_or(0),
            Datum::Null => 0,
        }
    }

    /// 提供比 Rust 默认 `Eq` 更接近 Go 断言风格的比较。
    /// 其中浮点比较允许极小误差，整数与浮点也能互相视为相等。
    pub fn deep_equal(&self, other: &Datum) -> bool {
        match (self, other) {
            (Datum::Int(a), Datum::Int(b)) => a == b,
            (Datum::Float(a), Datum::Float(b)) => (a - b).abs() < 1e-9,
            (Datum::Str(a), Datum::Str(b)) => a == b,
            (Datum::Null, Datum::Null) => true,
            (Datum::Int(a), Datum::Float(b)) => (*a as f64 - *b).abs() < 1e-9,
            (Datum::Float(a), Datum::Int(b)) => (*a - *b as f64).abs() < 1e-9,
            _ => false,
        }
    }

    /// 输出用于断言和错误信息的文本表示。
    /// 对整值浮点做压缩输出，减少 `1` 和 `1.0` 的噪声差异。
    pub fn to_string(&self) -> String {
        match self {
            Datum::Int(v) => v.to_string(),
            Datum::Float(v) => {
                if v.fract() == 0.0 && *v >= i64::MIN as f64 && *v <= i64::MAX as f64 {
                    (*v as i64).to_string()
                } else {
                    v.to_string()
                }
            }
            Datum::Str(s) => s.clone(),
            Datum::Null => "NULL".into(),
        }
    }
}

/// 让 `assert_eq!` 走上面的宽松比较规则，贴近 Go `DeepEqual` 语义。
impl PartialEq for Datum {
    fn eq(&self, other: &Self) -> bool {
        self.deep_equal(other)
    }
}

/// 简化后的列元信息。
/// 这里只保留列 ID、列名和默认值，足够支撑列变更校验。
#[derive(Clone, Debug)]
pub struct Column {
    pub id: i64,
    pub name: String,
    pub default_val: Option<Datum>,
}

/// 简化后的索引元信息。
/// 当前测试只需要知道索引名和索引列列表。
#[derive(Clone, Debug)]
pub struct IndexMeta {
    pub name: String,
    pub columns: Vec<String>,
}

/// 给测试返回的索引句柄壳体。
/// Go 用例只拿它确认“索引是否存在”，不依赖更多内部状态。
#[derive(Clone, Debug)]
pub struct IndexHandle {
    pub name: String,
}

/// 以最少字段描述一张测试表。
/// `rows` 使用主键句柄映射到一整行值，便于遍历和校验。
#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub indices: Vec<IndexMeta>,
    pub rows: BTreeMap<i64, Vec<Datum>>,
    pub next_col_id: i64,
    pub pk_cols: Vec<String>,
}

impl Table {
    /// 返回当前列快照；供检查函数按 Go 方式遍历 schema。
    pub fn cols(&self) -> &[Column] {
        &self.columns
    }
    /// 返回索引元信息列表；测试只做存在性和列名检查。
    pub fn indices(&self) -> &[IndexMeta] {
        &self.indices
    }
    /// 通过大小写不敏感的列名定位列下标。
    /// 这样可以容忍 SQL 文本中的大小写差异。
    pub fn col_index(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }
}

/// 在一组列里查找指定名字的列。
/// 单独暴露出来是为了让测试代码保持接近 Go helper 写法。
pub fn find_col<'a>(cols: &'a [Column], name: &str) -> Option<&'a Column> {
    cols.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------
// `Engine` 是单进程内存 SQL/DDL 执行器。
// 它只支持 `ddltest` 用例覆盖到的那一小部分语法，
// 并把“最终可观察行为”作为第一目标，而不是 SQL 完整性。

#[derive(Default)]
struct Database {
    tables: HashMap<String, Table>,
}

impl Database {
    /// 预置 Go 用例会访问的表集合。
    /// 这样测试初始化无需真正执行完整 bootstrap 流程，
    /// 也能保留各张表主键形态与列布局。
    fn bootstrap_tables(&mut self) {
        // 这一组表统一使用单列主键 `c1`，
        // 覆盖列增删、插入、更新、删除和混合写入场景。
        let names_pk_c1 = [
            "test_column",
            "test_insert",
            "test_conflict_insert",
            "test_update",
            "test_conflict_update",
            "test_delete",
            "test_conflict_delete",
            "test_mixed",
            "test_inc",
        ];
        for name in names_pk_c1 {
            self.tables.insert(
                name.to_string(),
                Table {
                    name: name.to_string(),
                    columns: vec![
                        Column {
                            id: 1,
                            name: "c1".into(),
                            default_val: None,
                        },
                        Column {
                            id: 2,
                            name: "c2".into(),
                            default_val: None,
                        },
                    ],
                    indices: vec![],
                    rows: BTreeMap::new(),
                    next_col_id: 3,
                    pk_cols: vec!["c1".into()],
                },
            );
        }
        // `test_index` 单独保留与索引测试一致的四列布局。
        self.tables.insert(
            "test_index".into(),
            Table {
                name: "test_index".into(),
                columns: vec![
                    Column {
                        id: 1,
                        name: "c".into(),
                        default_val: None,
                    },
                    Column {
                        id: 2,
                        name: "c1".into(),
                        default_val: None,
                    },
                    Column {
                        id: 3,
                        name: "c2".into(),
                        default_val: None,
                    },
                    Column {
                        id: 4,
                        name: "c3".into(),
                        default_val: None,
                    },
                ],
                indices: vec![],
                rows: BTreeMap::new(),
                next_col_id: 5,
                pk_cols: vec!["c".into()],
            },
        );
        // 这一组 `_common` 表使用联合主键，
        // 用于对齐 Go 中“公共 handle 行为”相关用例。
        let common = [
            "test_insert_common",
            "test_conflict_insert_common",
            "test_update_common",
            "test_conflict_update_common",
            "test_delete_common",
            "test_conflict_delete_common",
            "test_mixed_common",
            "test_inc_common",
        ];
        for name in common {
            self.tables.insert(
                name.to_string(),
                Table {
                    name: name.to_string(),
                    columns: vec![
                        Column {
                            id: 1,
                            name: "c1".into(),
                            default_val: None,
                        },
                        Column {
                            id: 2,
                            name: "c2".into(),
                            default_val: None,
                        },
                    ],
                    indices: vec![],
                    rows: BTreeMap::new(),
                    next_col_id: 3,
                    pk_cols: vec!["c1".into(), "c2".into()],
                },
            );
        }
    }
}

/// 执行语句后的最小返回值。
/// 当前只需要 `rows_affected` 来承接少量断言。
#[derive(Clone, Debug)]
pub struct ExecResult {
    pub rows_affected: u64,
}

/// 查询结果游标。
/// 这里一次性物化所有行，但仍保留 `next/current/close` 接口，
/// 让调用侧维持 Go `sql.Rows` 风格。
#[derive(Clone, Debug)]
pub struct QueryRows {
    pub cols: Vec<String>,
    pub rows: Vec<Vec<Datum>>,
    idx: usize,
}

impl QueryRows {
    /// 前进到下一行；语义与 Go `rows.Next()` 对齐。
    pub fn next(&mut self) -> bool {
        if self.idx < self.rows.len() {
            self.idx += 1;
            true
        } else {
            false
        }
    }
    /// 返回当前游标所在行。
    /// 约定调用方只在 `next()` 成功后访问。
    pub fn current(&self) -> &[Datum] {
        &self.rows[self.idx - 1]
    }
    /// 保留 `close()` 接口形态，便于测试逻辑直接迁移。
    /// 当前实现没有外部资源，因此关闭动作为空。
    pub fn close(&mut self) {}
}

/// 解析一个简单字面量。
/// 支持 `NULL`、整数、浮点和单引号字符串，已满足当前测试覆盖面。
fn parse_value(tok: &str) -> Datum {
    let t = tok.trim().trim_matches('\'');
    if t.eq_ignore_ascii_case("null") {
        return Datum::Null;
    }
    if let Ok(v) = t.parse::<i64>() {
        return Datum::Int(v);
    }
    if let Ok(v) = t.parse::<f64>() {
        return Datum::Float(v);
    }
    Datum::Str(t.to_string())
}

/// 在不进入字符串字面量内部的前提下按逗号切分。
/// 这足以支持当前 insert/set 子句里出现的简单 CSV 文本。
fn split_csv(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for ch in s.chars() {
        match ch {
            '\'' => {
                in_str = !in_str;
                cur.push(ch);
            }
            ',' if !in_str => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `SET` 子句中的赋值表达式。
/// 目前只支持直接赋值，以及 `col = col + n` 这类自增形式。
#[derive(Clone, Debug)]
enum SetExpr {
    Value(Datum),
    Inc(i64),
}

/// 把 `SET a=1, b=b+1` 解析成列名与表达式元组。
/// 这是更新测试最常见的写法，因此单独抽成 helper。
fn parse_assignments(set_clause: &str) -> Vec<(String, SetExpr)> {
    let mut out = Vec::new();
    for part in split_csv(set_clause) {
        let part = part.trim();
        let Some((lhs, rhs)) = part.split_once('=') else {
            continue;
        };
        let col = lhs.trim().to_string();
        let rhs = rhs.trim();
        if let Some((base, rest)) = rhs.split_once('+') {
            if base.trim().eq_ignore_ascii_case(&col) {
                let add: i64 = rest.trim().parse().unwrap_or(1);
                out.push((col, SetExpr::Inc(add)));
                continue;
            }
        }
        out.push((col, SetExpr::Value(parse_value(rhs))));
    }
    out
}

/// 从 where 子句里抽取主键句柄。
/// 当前只识别 `c1` 或 `c` 等测试固定主键名字，
/// 不支持一般性的布尔表达式解析。
fn parse_where_key(where_clause: &str) -> Option<i64> {
    let w = where_clause.trim();
    for prefix in ["c1 =", "c1=", "c =", "c="] {
        if let Some(rest) = w.strip_prefix(prefix) {
            return rest.trim().parse().ok();
        }
    }
    w.split('=').nth(1)?.trim().parse().ok()
}

/// 单进程内存执行器；所有表数据都保存在这里。
struct Engine {
    db: Database,
}

impl Engine {
    /// 创建新执行器并预置测试基础表。
    fn new() -> Self {
        let mut db = Database::default();
        db.bootstrap_tables();
        Self { db }
    }

    /// 根据 SQL 前缀把语句路由到对应的极简实现。
    /// 这里只覆盖 `ddltest` 用例真正会发出的语句。
    fn exec(&mut self, sql: &str) -> Result<ExecResult, String> {
        let sql = sql.trim().trim_end_matches(';');
        let lower = sql.to_ascii_lowercase();

        // 这些语句在当前测试里只承担环境准备作用，
        // 即使命中也无需产生真实副作用。
        if lower.starts_with("create database") || lower.starts_with("use ") {
            return Ok(ExecResult { rows_affected: 0 });
        }
        if lower.starts_with("create table") {
            return self.exec_create_table(sql);
        }
        if lower.starts_with("drop table") {
            let if_exists = lower.starts_with("drop table if exists");
            let names = if if_exists {
                sql["drop table if exists".len()..].trim()
            } else {
                sql["drop table".len()..].trim()
            };
            for name in names
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                if self.db.tables.remove(name).is_none() && !if_exists {
                    return Err(format!("table {name} not found"));
                }
            }
            return Ok(ExecResult { rows_affected: 0 });
        }
        if lower.starts_with("alter table") {
            return self.exec_alter(sql);
        }
        if lower.starts_with("create index") {
            return self.exec_create_index(sql);
        }
        if lower.starts_with("drop index") {
            return self.exec_drop_index(sql);
        }
        if lower.starts_with("insert into") {
            return self.exec_insert(sql);
        }
        if lower.starts_with("update ") {
            return self.exec_update(sql);
        }
        if lower.starts_with("delete from") {
            return self.exec_delete(sql);
        }
        if lower.starts_with("admin check table") {
            let name = sql.split_whitespace().last().unwrap_or("");
            if !self.db.tables.contains_key(name) {
                return Err(format!("table {name} not found"));
            }
            return Ok(ExecResult { rows_affected: 0 });
        }
        // 未支持的语句直接报错，避免把缺失能力伪装成成功。
        Err(format!("unsupported SQL: {sql}"))
    }

    /// 执行极简 `select` 查询。
    /// 只支持单表查询、列投影和可选 `limit`，
    /// 因为 `ddltest` 断言只依赖这些能力。
    fn query(&mut self, sql: &str) -> Result<QueryRows, String> {
        let sql = sql.trim().trim_end_matches(';');
        let lower = sql.to_ascii_lowercase();
        if !lower.starts_with("select") {
            return Err(format!("unsupported query: {sql}"));
        }
        let from_idx = lower.find(" from ").ok_or("missing from")?;
        let after_from = &sql[from_idx + 6..];
        let table_tok = after_from.split_whitespace().next().unwrap_or("");
        let tbl = self
            .db
            .tables
            .get(table_tok)
            .ok_or_else(|| format!("table {table_tok} not found"))?;
        let select_part = sql[6..from_idx].trim();
        // `*` 按当前 schema 顺序展开，便于列增删测试直接观测结果形态。
        let col_names: Vec<String> = if select_part == "*" {
            tbl.columns.iter().map(|c| c.name.clone()).collect()
        } else {
            select_part
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        };
        let mut limit = usize::MAX;
        if let Some(lpos) = lower.find(" limit ") {
            limit = sql[lpos + 7..]
                .split_whitespace()
                .next()
                .and_then(|x| x.parse().ok())
                .unwrap_or(usize::MAX);
        }
        let col_idxs: Vec<usize> = col_names
            .iter()
            .map(|n| {
                tbl.col_index(n)
                    .ok_or_else(|| format!("unknown column {n}"))
            })
            .collect::<Result<_, _>>()?;
        let mut rows = Vec::new();
        // `BTreeMap` 让遍历顺序按主键稳定，
        // 这样断言结果更接近 Go 里按 handle 扫描的表现。
        for (_h, data) in tbl.rows.iter() {
            rows.push(col_idxs.iter().map(|&i| data[i].clone()).collect());
            if rows.len() >= limit {
                break;
            }
        }
        Ok(QueryRows {
            cols: col_names,
            rows,
            idx: 0,
        })
    }

    /// 解析并创建一张最小测试表。
    /// 这里只处理列定义和主键列表，不关心类型细节或其他约束。
    fn exec_create_table(&mut self, sql: &str) -> Result<ExecResult, String> {
        let lower = sql.to_ascii_lowercase();
        let if_not_exists = lower.starts_with("create table if not exists");
        let rest = if if_not_exists {
            sql["create table if not exists".len()..].trim()
        } else {
            sql["create table".len()..].trim()
        };
        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        if self.db.tables.contains_key(&name) {
            if if_not_exists {
                return Ok(ExecResult { rows_affected: 0 });
            }
            return Err(format!("table {name} already exists"));
        }
        let cols_start = rest.find('(').ok_or("bad create")?;
        let cols_end = rest.rfind(')').ok_or("bad create")?;
        let body = &rest[cols_start + 1..cols_end];
        let mut columns = Vec::new();
        let mut next_id = 1i64;
        let mut pk_cols = Vec::new();
        // 这里按逗号粗切定义体已经足够，
        // 因为测试建表语句都比较规整，不包含复杂表达式。
        for part in body.split(',') {
            let part = part.trim();
            let pl = part.to_ascii_lowercase();
            if pl.starts_with("primary key") {
                let inside = part
                    .find('(')
                    .and_then(|i| part.rfind(')').map(|j| &part[i + 1..j]))
                    .unwrap_or("");
                pk_cols = inside.split(',').map(|s| s.trim().to_string()).collect();
                continue;
            }
            let cname = part.split_whitespace().next().unwrap_or("").to_string();
            columns.push(Column {
                id: next_id,
                name: cname,
                default_val: None,
            });
            next_id += 1;
        }
        // 若未显式声明主键，则退化为首列，
        // 与当前内存行存储“必须有一个句柄”的模型兼容。
        if pk_cols.is_empty() && !columns.is_empty() {
            pk_cols.push(columns[0].name.clone());
        }
        self.db.tables.insert(
            name.clone(),
            Table {
                name,
                columns,
                indices: vec![],
                rows: BTreeMap::new(),
                next_col_id: next_id,
                pk_cols,
            },
        );
        Ok(ExecResult { rows_affected: 0 })
    }

    /// 执行列增删相关的 `alter table`。
    /// 这是列 DDL 测试的核心：新增列要回填默认值，删列要同步裁剪所有行。
    fn exec_alter(&mut self, sql: &str) -> Result<ExecResult, String> {
        let parts: Vec<&str> = sql.split_whitespace().collect();
        let tname = parts.get(2).copied().unwrap_or("");
        let tbl = self
            .db
            .tables
            .get_mut(tname)
            .ok_or_else(|| format!("table {tname} not found"))?;
        let lower = sql.to_ascii_lowercase();
        if lower.contains("add column") {
            // 新增列时把现有行补成默认值或 NULL，
            // 用来模拟旧行在 schema 变化后的可见结果。
            let col_name = parts
                .iter()
                .position(|p| p.eq_ignore_ascii_case("column"))
                .and_then(|i| parts.get(i + 1))
                .copied()
                .unwrap_or("");
            let mut default_val = None;
            if let Some(di) = parts.iter().position(|p| p.eq_ignore_ascii_case("default")) {
                default_val = parts.get(di + 1).map(|v| parse_value(v));
            }
            let id = tbl.next_col_id;
            tbl.next_col_id += 1;
            tbl.columns.push(Column {
                id,
                name: col_name.to_string(),
                default_val: default_val.clone(),
            });
            let fill = default_val.unwrap_or(Datum::Null);
            for row in tbl.rows.values_mut() {
                row.push(fill.clone());
            }
            return Ok(ExecResult { rows_affected: 0 });
        }
        if lower.contains("drop column") {
            // 删列时不仅移除 schema，也要同步移除每一行对应位置的值，
            // 否则后续按列序读取会错位。
            let col_name = parts
                .iter()
                .position(|p| p.eq_ignore_ascii_case("column"))
                .and_then(|i| parts.get(i + 1))
                .copied()
                .unwrap_or("");
            let idx = tbl
                .col_index(col_name)
                .ok_or_else(|| format!("column {col_name} not found"))?;
            tbl.columns.remove(idx);
            for row in tbl.rows.values_mut() {
                if idx < row.len() {
                    row.remove(idx);
                }
            }
            return Ok(ExecResult { rows_affected: 0 });
        }
        Err(format!("unsupported alter: {sql}"))
    }

    /// 创建索引元数据。
    /// 当前不维护真实二级索引结构，只记录“索引存在且绑定哪些列”。
    fn exec_create_index(&mut self, sql: &str) -> Result<ExecResult, String> {
        let parts: Vec<&str> = sql.split_whitespace().collect();
        let iname = parts.get(2).copied().unwrap_or("");
        let on_pos = parts.iter().position(|p| p.eq_ignore_ascii_case("on"));
        let tname = on_pos.and_then(|i| parts.get(i + 1)).copied().unwrap_or("");
        let cols_part = sql
            .find('(')
            .and_then(|i| sql.rfind(')').map(|j| &sql[i + 1..j]))
            .unwrap_or("");
        let cols: Vec<String> = cols_part.split(',').map(|s| s.trim().to_string()).collect();
        let tbl = self
            .db
            .tables
            .get_mut(tname)
            .ok_or_else(|| format!("table {tname} not found"))?;
        // 重复创建仍报 Go/MySQL 风格错误文本，
        // 让测试能沿用原断言或错误处理路径。
        if tbl.indices.iter().any(|i| i.name == iname) {
            return Err(format!("Duplicate key name '{iname}'"));
        }
        tbl.indices.push(IndexMeta {
            name: iname.to_string(),
            columns: cols,
        });
        Ok(ExecResult { rows_affected: 0 })
    }

    /// 删除索引元数据。
    /// 和创建一样，只处理测试可观察到的存在性变化。
    fn exec_drop_index(&mut self, sql: &str) -> Result<ExecResult, String> {
        let parts: Vec<&str> = sql.split_whitespace().collect();
        let iname = parts.get(2).copied().unwrap_or("");
        let on_pos = parts.iter().position(|p| p.eq_ignore_ascii_case("on"));
        let tname = on_pos.and_then(|i| parts.get(i + 1)).copied().unwrap_or("");
        let tbl = self
            .db
            .tables
            .get_mut(tname)
            .ok_or_else(|| format!("table {tname} not found"))?;
        let before = tbl.indices.len();
        tbl.indices.retain(|i| i.name != iname);
        // 如果名字不存在就显式报错，
        // 防止测试在无效语句上悄悄通过。
        if tbl.indices.len() == before {
            return Err(format!("index {iname} not found"));
        }
        Ok(ExecResult { rows_affected: 0 })
    }

    /// 执行最小 `insert`。
    /// 支持带列名和不带列名两种形式，并在需要时回填默认值。
    fn exec_insert(&mut self, sql: &str) -> Result<ExecResult, String> {
        let after = sql["insert into".len()..].trim();
        let tname = after.split_whitespace().next().unwrap_or("").to_string();
        let rest = after[tname.len()..].trim();
        let rest_lower = rest.to_ascii_lowercase();
        let rv = rest_lower
            .find("values")
            .ok_or_else(|| format!("bad insert: {sql}"))?;
        let before = rest[..rv].trim();
        let values_part = rest[rv + "values".len()..].trim();
        let col_list = if before.starts_with('(') {
            let inner = &before[1..before.rfind(')').unwrap_or(1)];
            Some(
                inner
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        let vals_inner = values_part
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')');
        let vals: Vec<Datum> = split_csv(vals_inner)
            .iter()
            .map(|v| parse_value(v))
            .collect();

        let tbl = self
            .db
            .tables
            .get_mut(&tname)
            .ok_or_else(|| format!("table {tname} not found"))?;

        // 行内部始终按当前 schema 顺序存储，
        // 未显式写入的列先置空，再根据默认值补齐。
        let mut row = vec![Datum::Null; tbl.columns.len()];
        if let Some(cols) = col_list {
            if cols.len() != vals.len() {
                return Err("Column count doesn't match value count at row".into());
            }
            for (c, v) in cols.iter().zip(vals.iter()) {
                let idx = tbl
                    .col_index(c)
                    .ok_or_else(|| format!("unknown column {c}"))?;
                row[idx] = v.clone();
            }
            for (i, col) in tbl.columns.iter().enumerate() {
                if matches!(row[i], Datum::Null) {
                    if let Some(d) = &col.default_val {
                        row[i] = d.clone();
                    }
                }
            }
        } else {
            if vals.len() != tbl.columns.len() {
                return Err("Column count doesn't match value count at row".into());
            }
            row = vals;
        }

        // 当前存储模型只用主键第一列作为句柄，
        // 对 `ddltest` 场景已经足够。
        let pk_idx = tbl.col_index(&tbl.pk_cols[0]).ok_or("missing pk")?;
        let handle = row[pk_idx].get_int64();
        // 继续沿用 Go/MySQL 风格的重复主键错误文本，
        // 便于上层重启容错逻辑识别并吞掉预期冲突。
        if tbl.rows.contains_key(&handle) {
            return Err(format!("Duplicate entry '{handle}' for key 'PRIMARY'"));
        }
        tbl.rows.insert(handle, row);
        Ok(ExecResult { rows_affected: 1 })
    }

    /// 执行最小 `update`。
    /// 支持整表更新或按主键更新，并允许简单自增表达式。
    fn exec_update(&mut self, sql: &str) -> Result<ExecResult, String> {
        let lower = sql.to_ascii_lowercase();
        let after = sql["update".len()..].trim();
        let tname = after.split_whitespace().next().unwrap_or("").to_string();
        let set_pos = lower.find(" set ").ok_or("missing set")?;
        let where_pos = lower.find(" where ");
        let set_clause = if let Some(w) = where_pos {
            sql[set_pos + 5..w].trim()
        } else {
            sql[set_pos + 5..].trim()
        };
        let where_clause = where_pos.map(|w| sql[w + 7..].trim()).unwrap_or("");
        let assigns = parse_assignments(set_clause);
        let key = parse_where_key(where_clause);

        let tbl = self
            .db
            .tables
            .get_mut(&tname)
            .ok_or_else(|| format!("table {tname} not found"))?;

        // 先确定受影响句柄集合，再逐行应用赋值，
        // 这样逻辑更接近 Go 测试对“按键命中”结果的预期。
        let targets: Vec<i64> = if let Some(k) = key {
            if tbl.rows.contains_key(&k) {
                vec![k]
            } else {
                vec![]
            }
        } else {
            tbl.rows.keys().copied().collect()
        };
        let mut affected = 0u64;
        for h in targets {
            if let Some(row) = tbl.rows.get_mut(&h) {
                for (col, expr) in &assigns {
                    let Some(idx) = tbl
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(col))
                    else {
                        return Err(format!("unknown column {col}"));
                    };
                    match expr {
                        SetExpr::Value(v) => row[idx] = v.clone(),
                        // 这里只实现整数递增，足以覆盖测试中的计数场景。
                        SetExpr::Inc(n) => {
                            let cur = row[idx].get_int64();
                            row[idx] = Datum::Int(cur + *n);
                        }
                    }
                }
                affected += 1;
            }
        }
        Ok(ExecResult {
            rows_affected: affected,
        })
    }

    /// 执行最小 `delete`。
    /// 只支持整表删除或按主键删除，因为测试里不会出现更复杂条件。
    fn exec_delete(&mut self, sql: &str) -> Result<ExecResult, String> {
        let lower = sql.to_ascii_lowercase();
        let after = sql["delete from".len()..].trim();
        let tname = after.split_whitespace().next().unwrap_or("").to_string();
        let where_pos = lower.find(" where ");
        let where_clause = where_pos.map(|w| sql[w + 7..].trim()).unwrap_or("");
        let key = parse_where_key(where_clause);
        let tbl = self
            .db
            .tables
            .get_mut(&tname)
            .ok_or_else(|| format!("table {tname} not found"))?;
        if let Some(k) = key {
            if tbl.rows.remove(&k).is_some() {
                Ok(ExecResult { rows_affected: 1 })
            } else {
                Ok(ExecResult { rows_affected: 0 })
            }
        } else {
            // 无条件删除时返回清空前的行数，
            // 与常见 SQL `rows affected` 语义保持一致。
            let n = tbl.rows.len() as u64;
            tbl.rows.clear();
            Ok(ExecResult { rows_affected: n })
        }
    }

    /// 按名字返回当前表快照；供外层断言逻辑读取。
    fn get_table(&self, name: &str) -> Option<Table> {
        self.db.tables.get(name).cloned()
    }
}

// ---------------------------------------------------------------------------
// Suite
// ---------------------------------------------------------------------------
// `DdlSuite` 是测试可直接操作的门面。
// 它把内存引擎、伪 server 列表、随机重启线程和一批 Go 风格 helper
// 聚合在一起，让测试文件可以继续沿用“suite 驱动”的组织方式。

/// 伪 server 记录。
/// 这里只关心是否存活以及它暴露给调用侧的地址字符串。
struct MockServer {
    alive: AtomicBool,
    addr: String,
}

/// `ddltest` 的核心 suite。
/// 与 Go 中的 `ddlSuite` 相比，这里省掉真实存储、domain 和 session，
/// 但保留路由、重启、DDL 等待与结果校验入口。
pub struct DdlSuite {
    engine: Arc<Mutex<Engine>>,
    procs: Arc<Mutex<Vec<Option<Arc<MockServer>>>>>,
    quit: Arc<AtomicBool>,
    restart_handle: Mutex<Option<thread::JoinHandle<()>>>,
    pub retry_count: i32,
}

/// 公开类型别名，方便测试按 Go 风格传递共享 suite。
pub type Suite = Arc<DdlSuite>;

impl DdlSuite {
    /// 构造一个新的测试 suite。
    /// 创建时会初始化内存引擎、伪 server 列表，
    /// 并启动后台随机重启线程来制造 owner/server 抖动窗口。
    pub fn create() -> Suite {
        let engine = Arc::new(Mutex::new(Engine::new()));
        let mut procs = Vec::with_capacity(SERVER_NUM as usize);
        // 伪 server 启动即视为存活，
        // 地址只用于让日志/调试信息看起来像真实多实例环境。
        for i in 0..SERVER_NUM {
            procs.push(Some(Arc::new(MockServer {
                alive: AtomicBool::new(true),
                addr: format!("{}:{}", TIDB_IP, START_PORT + i),
            })));
        }
        let quit = Arc::new(AtomicBool::new(false));
        let procs_arc = Arc::new(Mutex::new(procs));
        let procs_bg = Arc::clone(&procs_arc);
        let quit_bg = Arc::clone(&quit);
        let handle = thread::spawn(move || {
            loop {
                // 等待一个租约相关窗口后再尝试重启，
                // 用来模拟 Go 测试里“DDL 期间 server 可能上下线”的节奏。
                let after = LEASE * (6 + random_intn(6));
                let deadline = Instant::now() + Duration::from_millis(after.max(1) as u64 * 20);
                while Instant::now() < deadline {
                    if quit_bg.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                if quit_bg.load(Ordering::Relaxed) {
                    return;
                }
                if ENABLE_RESTART {
                    let i = random_intn(SERVER_NUM) as usize;
                    let mut guard = procs_bg.lock().unwrap();
                    if let Some(Some(srv)) = guard.get(i) {
                        // 旧对象先标记为死亡，再原位放入一个新对象，
                        // 表达“同地址实例被重启”的效果。
                        srv.alive.store(false, Ordering::Relaxed);
                        let addr = srv.addr.clone();
                        guard[i] = Some(Arc::new(MockServer {
                            alive: AtomicBool::new(true),
                            addr,
                        }));
                    }
                }
            }
        });
        Arc::new(Self {
            engine,
            procs: procs_arc,
            quit,
            restart_handle: Mutex::new(Some(handle)),
            retry_count: 20,
        })
    }

    /// 停止后台线程并清空所有伪 server。
    /// 这样每个测试都能在独立、可回收的环境里结束。
    pub fn teardown(&self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(h) = self.restart_handle.lock().unwrap().take() {
            let _ = h.join();
        }
        let mut procs = self.procs.lock().unwrap();
        for p in procs.iter_mut() {
            *p = None;
        }
    }

    /// 随机挑选一个当前可用的 server。
    /// 这一步并不改变执行结果，
    /// 主要是保留 Go 测试里“请求会被路由到不同实例”的语义外观。
    fn get_server(&self) -> Arc<MockServer> {
        let procs = self.procs.lock().unwrap();
        for _ in 0..20 {
            let i = random_intn(SERVER_NUM) as usize;
            if let Some(Some(s)) = procs.get(i) {
                if s.alive.load(Ordering::Relaxed) {
                    return Arc::clone(s);
                }
            }
        }
        // 如果随机尝试没有命中存活实例，就退化到第一个仍存在的 server，
        // 让测试更稳，而不是因抽样时机崩溃。
        for p in procs.iter().flatten() {
            return Arc::clone(p);
        }
        panic!("try to get server too many times");
    }

    /// 通过 suite 执行一条 SQL。
    /// 先做一次伪 server 选择，再转发给底层内存引擎。
    pub fn exec(&self, query: &str) -> Result<ExecResult, String> {
        let _srv = self.get_server();
        self.engine.lock().unwrap().exec(query)
    }

    /// 要求语句必须成功，否则直接 panic。
    /// 这与 Go 测试里 `mustExec` 的失败风格一致。
    pub fn must_exec(&self, query: &str) -> ExecResult {
        match self.exec(query) {
            Ok(r) => r,
            Err(e) => panic!("[mustExec fail]query={query} err={e}"),
        }
    }

    /// 专门给插入语句使用的包装。
    /// 当开启重启模拟时，重复主键可能是预期竞态结果，
    /// 因此这里会把特定错误转成“0 行受影响”而不是失败。
    pub fn exec_insert(&self, query: &str) -> ExecResult {
        match self.exec(query) {
            Ok(r) => r,
            Err(e) => {
                if ENABLE_RESTART && e.contains("Duplicate entry") && e.contains("for key") {
                    return ExecResult { rows_affected: 0 };
                }
                panic!("[execInsert fail]query={query} err={e}");
            }
        }
    }

    /// 执行查询并返回游标包装。
    pub fn query(&self, query: &str) -> Result<QueryRows, String> {
        let _srv = self.get_server();
        self.engine.lock().unwrap().query(query)
    }

    /// 异步执行 DDL。
    /// Go 用例会在 DDL 挂起期间并发施加写入，
    /// 所以这里通过线程和 channel 暴露一个“稍后完成”的接口。
    pub fn run_ddl(&self, sql: &str) -> Receiver<Result<(), String>> {
        let (tx, rx) = mpsc::channel();
        let eng = Arc::clone(&self.engine);
        let sql = sql.to_string();
        thread::spawn(move || {
            let err = eng.lock().unwrap().exec(&sql).map(|_| ());
            if err.is_ok() {
                // 成功后额外等待两个租约窗口，
                // 近似表达 schema 变更传播和观察延迟。
                thread::sleep(Duration::from_millis((LEASE.max(1) as u64) * 2 * 8));
            }
            let _ = tx.send(err);
        });
        rx
    }

    /// 读取指定表的当前快照；不存在时直接报错。
    pub fn get_table(&self, name: &str) -> Table {
        self.engine
            .lock()
            .unwrap()
            .get_table(name)
            .unwrap_or_else(|| panic!("table {name} not found"))
    }

    /// 占位的新事务入口。
    /// 当前内存模型没有真实事务，但保留接口能让检查逻辑维持 Go 形态。
    pub fn new_txn(&self) -> Result<(), String> {
        Ok(())
    }

    /// 遍历某张表的所有记录。
    /// 回调返回 `false` 时提早终止，接口风格对齐 Go 的 record iterator。
    pub fn iter_records<F>(&self, tbl_name: &str, mut f: F) -> Result<(), String>
    where
        F: FnMut(i64, &[Datum], &[Column]) -> Result<bool, String>,
    {
        let live = self.get_table(tbl_name);
        for (h, row) in &live.rows {
            if !f(*h, row, &live.columns)? {
                break;
            }
        }
        Ok(())
    }

    /// 校验“加列期间并发写入”后的最终数据分布。
    /// 断言重点不是精确行数，
    /// 而是四类可观察结果都存在：旧插入、新插入、旧更新、新更新。
    pub fn check_add_column(&self, row_id: i64, default_val: Datum, updated_val: Datum) {
        self.new_txn().unwrap();
        let mut old_insert = 0i64;
        let mut new_insert = 0i64;
        let mut old_update = 0i64;
        let mut new_update = 0i64;
        self.iter_records("test_column", |_h, data, _cols| {
            let col1 = &data[0];
            let col2 = &data[1];
            let col3 = &data[2];
            // `c1 == c2` 代表该行来自插入路径；
            // 再看 `c3` 是默认值还是等于主值，以区分旧 schema 与新 schema 插入。
            if col1.deep_equal(col2) {
                if col3.deep_equal(&default_val) {
                    old_insert += 1;
                } else if col3.deep_equal(col1) {
                    new_insert += 1;
                } else {
                    panic!("[checkAddColumn fail]invalid row {data:?}");
                }
            }
            // `c2 == updated_val` 代表该行经过更新路径；
            // 若 `c3` 也被同步改掉，则说明更新发生在新列已可见之后。
            if col2.deep_equal(&updated_val) {
                if col3.deep_equal(&default_val) || col3.deep_equal(col1) {
                    old_update += 1;
                } else if col3.deep_equal(&updated_val) {
                    new_update += 1;
                } else {
                    panic!("[checkAddColumn fail]invalid row {data:?}");
                }
            }
            Ok(true)
        })
        .unwrap();
        let delete_count = row_id - old_insert - new_insert - old_update - new_update;
        // 这些断言只要求四类状态都被观察到，
        // 用来证明并发窗口确实覆盖到了 schema 变更前后。
        assert!(old_insert >= 0);
        assert!(new_insert >= 0);
        assert!(old_update > 0, "old_update={old_update}");
        assert!(new_update > 0, "new_update={new_update}");
        assert!(delete_count > 0, "delete_count={delete_count}");
    }

    /// 校验删列后的表状态。
    /// 既要确认目标列 ID 已从 schema 消失，
    /// 也要确认剩余数据仍能分成插入路径和更新路径两类。
    pub fn check_drop_column(&self, row_id: i64, alter_column: &Column, update_default: Datum) {
        self.new_txn().unwrap();
        let tbl = self.get_table("test_column");
        // 先从 schema 层确认被删列不再可见。
        for col in tbl.cols() {
            assert_ne!(alter_column.id, col.id);
        }
        let mut insert_count = 0i64;
        let mut update_count = 0i64;
        self.iter_records("test_column", |_h, data, _cols| {
            // 删除 `c3` 后，保留下来的第二列要么等于原主值，
            // 要么等于更新写入的默认值。
            if data[1].deep_equal(&data[0]) {
                insert_count += 1;
            } else if data[1].deep_equal(&update_default) {
                update_count += 1;
            } else {
                panic!("[checkDropColumn fail]invalid row {data:?}");
            }
            Ok(true)
        })
        .unwrap();
        let delete_count = row_id - insert_count - update_count;
        assert!(insert_count > 0);
        assert!(update_count > 0);
        assert!(delete_count > 0);
    }

    /// 校验删索引后的收尾路径。
    /// Go 中这里还会触发 GC 删除 range；Rust 桩只保留调用顺序语义。
    pub fn check_drop_index(&self, table_name: &str) {
        // Go: MockGCWorker.DeleteRanges(MaxInt32) then admin check.
        let _ = mock_gc_delete_ranges();
        self.must_exec(&format!("admin check table {table_name}"));
    }
}

/// 模拟 GC worker 删除过期索引范围。
/// 当前没有真实 KV range，因此只保留一个成功返回的占位函数。
fn mock_gc_delete_ranges() -> Result<(), String> {
    Ok(())
}

/// 公开构造入口。
/// 测试文件通过它创建 suite，避免直接依赖内部结构体名。
pub fn create_ddl_suite() -> Suite {
    DdlSuite::create()
}

/// 按名字查找索引。
/// 返回轻量句柄而不是完整元信息，贴近 Go 用例的使用方式。
pub fn get_index(t: &Table, name: &str) -> Option<IndexHandle> {
    for idx in t.indices() {
        if idx.name == name {
            return Some(IndexHandle {
                name: idx.name.clone(),
            });
        }
    }
    None
}

/// 把租约换算成测试里常用的等待时长。
/// 这里强调“相对节奏”而非真实时间单位。
pub fn lease_tick() -> Duration {
    Duration::from_millis((LEASE.max(1) as u64) * 4)
}

/// 把游标中的所有行提取为二维数组，便于后续断言。
pub fn dump_rows(rows: &mut QueryRows) -> Vec<Vec<Datum>> {
    let mut ay = Vec::new();
    while rows.next() {
        ay.push(rows.current().to_vec());
    }
    rows.close();
    ay
}

/// 比较整组结果集。
/// 先比行数，再逐行调用 `match_row` 做宽松值比较。
pub fn match_rows(rows: &mut QueryRows, expected: &[Vec<Datum>]) {
    let ay = dump_rows(rows);
    assert_eq!(expected.len(), ay.len(), "expected={expected:?}");
    for i in 0..ay.len() {
        match_row(&ay[i], &expected[i]);
    }
}

/// 比较单行结果。
/// `NULL` 需要按空值语义处理，其余值统一比较字符串表示，
/// 这样能减少整数/浮点表示差异带来的噪声。
pub fn match_row(row: &[Datum], expected: &[Datum]) {
    assert_eq!(expected.len(), row.len());
    for i in 0..row.len() {
        if matches!(row[i], Datum::Null) {
            assert!(matches!(expected[i], Datum::Null));
            continue;
        }
        assert_eq!(row[i].to_string(), expected[i].to_string());
    }
}

/// Go TestMain setup retained for the observable logger configuration.
pub fn setup_test_main() -> Result<(), String> {
    if LOG_LEVEL.is_empty() {
        return Err("empty log level".into());
    }
    Ok(())
}

/// 测试文件共享的一组并发操作 helper。
/// 这些方法把“持续写表直到 DDL 完成”的控制流从测试本体中抽出来。
pub trait SuiteOps {
    fn exec_column_operations(
        &self,
        worker_num: i32,
        count: i32,
        row_id: &Arc<AtomicI64>,
        update_default: i64,
    );
    fn exec_index_operations(&self, worker_num: i32, count: i32, insert_id: &Arc<AtomicI64>);
}

impl SuiteOps for Suite {
    /// 在列 DDL 期间持续制造插入、更新、删行混合流量。
    /// 线程之间共享 `row_id`，模拟 Go 用例里不断增长的写入句柄。
    fn exec_column_operations(
        &self,
        worker_num: i32,
        count: i32,
        row_id: &Arc<AtomicI64>,
        update_default: i64,
    ) {
        let mut handles = Vec::new();
        for _ in 0..worker_num {
            let suite = Arc::clone(self);
            let row_id = Arc::clone(row_id);
            handles.push(thread::spawn(move || {
                for _ in 0..count {
                    // 一次循环故意覆盖多类写入：
                    // 老 schema 插入、新 schema 插入、老路径更新、新路径更新、删除。
                    let key = row_id.fetch_add(2, Ordering::SeqCst) + 2;
                    suite.exec_insert(&format!(
                        "insert into test_column (c1, c2) values ({}, {})",
                        key - 1,
                        key - 1
                    ));
                    let _ = suite.exec(&format!(
                        "insert into test_column values ({}, {}, {})",
                        key, key, key
                    ));
                    suite.must_exec(&format!(
                        "update test_column set c2 = {} where c1 = {}",
                        update_default,
                        random_num(&[key as i32])
                    ));
                    let _ = suite.exec(&format!(
                        "update test_column set c2 = {}, c3 = {} where c1 = {}",
                        update_default,
                        update_default,
                        random_num(&[key as i32])
                    ));
                    suite.must_exec(&format!(
                        "delete from test_column where c1 = {}",
                        random_num(&[key as i32])
                    ));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    /// 在索引 DDL 期间持续制造索引相关负载。
    /// 这里的目标不是验证随机数本身，
    /// 而是让插入、删除、更新交错发生，从而逼近 Go 并发场景。
    fn exec_index_operations(&self, worker_num: i32, count: i32, insert_id: &Arc<AtomicI64>) {
        let mut handles = Vec::new();
        for _ in 0..worker_num {
            let suite = Arc::clone(self);
            let insert_id = Arc::clone(insert_id);
            handles.push(thread::spawn(move || {
                for _ in 0..count {
                    // 先插入，再随机删、随机改，
                    // 用于覆盖索引建立和删除期间的多种数据形态。
                    let id = insert_id.fetch_add(1, Ordering::SeqCst) + 1;
                    let sql = format!(
                        "insert into test_index values ({}, {}, {}, '{}')",
                        id,
                        random_int(),
                        random_float(),
                        random_string(10)
                    );
                    suite.exec_insert(&sql);
                    let sql = format!(
                        "delete from test_index where c = {}",
                        random_intn(id as i32)
                    );
                    suite.must_exec(&sql);
                    let sql = format!(
                        "update test_index set c1 = {}, c2 = {}, c3 = '{}' where c = {}",
                        random_int(),
                        random_float(),
                        random_string(10),
                        random_intn(id as i32)
                    );
                    suite.must_exec(&sql);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }
}

/// 极简 handle 集合。
/// Go 测试会把访问到的句柄放进 map/set，这里只保留去重和计数能力。
#[derive(Default)]
pub struct HandleMap {
    set: HashSet<i64>,
}

impl HandleMap {
    /// 创建一个空集合。
    pub fn new() -> Self {
        Self::default()
    }
    /// 记录一个句柄；重复值会被自动去重。
    pub fn set(&mut self, h: i64) {
        self.set.insert(h);
    }
    /// 返回当前去重后的句柄个数。
    pub fn len(&self) -> usize {
        self.set.len()
    }
}
