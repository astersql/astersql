// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `util_for_test.go`：export 包测试共用的 mock 实现。
//! 提供可脚本化的 TableMeta/TableDataIR/MetaIR/ObjectWriter 替身，避免单测依赖真实 MySQL/PD。
//! 所有 mock 均纯内存实现，与 Go testify/sqlmock 用法类似。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::*;

// 返回桩 PdClient，供 GC safe point 相关测试注入行为。
// new_mock 构造的全字段 Default，测试通过 Arc<Mutex> 字段注入错误。
pub fn new_mock_pd_client_for_gc() -> PdClient {
    PdClient::new_mock()
}

// mockPoisonWriter 写入 "poison" 时返回错误，用于测试 write() 失败传播。
pub struct mockPoisonWriter {
    // buf 保存最后一次成功写入的内容（非 poison 时）。
    pub buf: String,
    // closed 标记 Close 是否已调用。
    pub closed: bool,
}
impl mockPoisonWriter {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            closed: false,
        }
    }
}
// mockPoisonWriter 实现 ObjectWriter，供 write() 错误路径单测。
impl ObjectWriter for mockPoisonWriter {
    // "poison"  magic 字符串触发 errors_new，模拟写入失败。
    fn Write(&mut self, data: &[u8]) -> Result<usize> {
        let s = String::from_utf8_lossy(data).to_string();
        if s == "poison" {
            return Err(errors_new("poison_error"));
        }
        self.buf = s;
        Ok(data.len())
    }
    fn Close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

// MetaIR 替身：按序输出 special comments，再返回固定 MetaSQL。
pub struct mockMetaIR {
    // tar_name 对应 MetaIR::TargetName，用于日志与路径。
    pub tar_name: String,
    // meta 为 CREATE TABLE/VIEW 等 DDL 正文。
    pub meta: String,
    // spec_cmt 按序输出在 DDL 前的 MySQL conditional comments。
    pub spec_cmt: Vec<String>,
    spec_idx: usize,
}
impl mockMetaIR {
    pub fn new(target_name: &str, meta: &str, special_comments: &[&str]) -> Self {
        Self {
            tar_name: target_name.into(),
            meta: meta.into(),
            spec_cmt: special_comments.iter().map(|s| (*s).to_string()).collect(),
            spec_idx: 0,
        }
    }
}
impl MetaIR for mockMetaIR {
    fn SpecialComments(&self) -> Box<dyn StringIter> {
        // 克隆 spec_cmt 交给 StringIter，避免借用生命周期问题。
        newStringIter(self.spec_cmt.clone())
    }
    fn TargetName(&self) -> &str {
        &self.tar_name
    }
    // MetaSQL 每次返回同一份 DDL，对应 Go mock 的固定字符串字段。
    fn MetaSQL(&mut self) -> String {
        self.meta.clone()
    }
}

// TableMeta + TableDataIR 双实现：用内存二维表驱动 Rows 迭代，列类型可简化为字符串。
pub struct mockTableIR {
    // db_name/tbl_name 供 TableMeta 与 Brief 使用。
    pub db_name: String,
    pub tbl_name: String,
    // chunk_index 预留分块标识，默认 0。
    pub chunk_index: i32,
    // data 为行优先的单元格矩阵，None 表示 NULL。
    pub data: Vec<Vec<Option<Vec<u8>>>>,
    // selected_field 写入 INSERT 列列表段，默认 "*"。
    pub selected_field: String,
    pub spec_cmt: Vec<String>,
    // col_types/col_names 驱动 MakeRowReceiver 与列元数据。
    pub col_types: Vec<String>,
    pub col_names: Vec<String>,
    // escape_backslash 控制 SQL 转义行为（Writer 读 conf 时覆盖）。
    pub escape_backslash: bool,
    // has_implicit_row_id 模拟 TiDB _tidb_rowid 隐式列。
    pub has_implicit_row_id: bool,
    // row_err 可注入行级迭代错误（扩展点）。
    pub row_err: Option<Error>,
    // column_infos 供 with_column_info 构造 Parquet/精确列名场景。
    pub column_infos: Vec<ColumnInfo>,
}

impl mockTableIR {
    // 基础构造：列名默认等于 col_types，selected_field 为 "*"。
    pub fn new(
        database_name: &str,
        table_name: &str,
        data: Vec<Vec<Option<Vec<u8>>>>,
        special_comments: &[&str],
        col_types: &[&str],
    ) -> Self {
        Self {
            db_name: database_name.into(),
            tbl_name: table_name.into(),
            chunk_index: 0,
            data,
            selected_field: "*".into(),
            spec_cmt: special_comments.iter().map(|s| (*s).to_string()).collect(),
            col_types: col_types.iter().map(|s| (*s).to_string()).collect(),
            col_names: col_types.iter().map(|s| (*s).to_string()).collect(),
            escape_backslash: true,
            has_implicit_row_id: false,
            row_err: None,
            column_infos: vec![],
        }
    }

    // with_column_info 从 ColumnInfo 推导 col_types/col_names。
    pub fn with_column_info(
        database_name: &str,
        table_name: &str,
        data: Vec<Vec<Option<Vec<u8>>>>,
        special_comments: &[&str],
        infos: Vec<ColumnInfo>,
    ) -> Self {
        let col_types: Vec<String> = infos.iter().map(|i| i.DatabaseTypeName.clone()).collect();
        let col_names: Vec<String> = infos.iter().map(|i| i.Name.clone()).collect();
        Self {
            db_name: database_name.into(),
            tbl_name: table_name.into(),
            chunk_index: 0,
            data,
            selected_field: "*".into(),
            spec_cmt: special_comments.iter().map(|s| (*s).to_string()).collect(),
            col_types,
            col_names,
            escape_backslash: true,
            has_implicit_row_id: false,
            row_err: None,
            column_infos: infos,
        }
    }
}

impl TableMeta for mockTableIR {
    // 以下方法均从 struct 字段直读，无 IO。
    fn DatabaseName(&self) -> &str {
        &self.db_name
    }
    fn TableName(&self) -> &str {
        &self.tbl_name
    }
    fn ColumnCount(&self) -> u32 {
        // 列数等于 col_types 长度。
        self.col_types.len() as u32
    }
    fn ColumnTypes(&self) -> Vec<String> {
        self.col_types.clone()
    }
    fn ColumnNames(&self) -> Vec<String> {
        // 默认与 col_types 同名，with_column_info 可覆盖。
        self.col_names.clone()
    }
    fn SelectedField(&self) -> &str {
        &self.selected_field
    }
    fn SelectedLen(&self) -> i32 {
        // CSV header 与 INSERT 列段均参考此长度。
        self.col_types.len() as i32
    }
    fn SpecialComments(&self) -> Box<dyn StringIter> {
        newStringIter(self.spec_cmt.clone())
    }
    fn ShowCreateTable(&self) -> &str {
        // mock 不写 SHOW CREATE，返回空串满足 trait。
        ""
    }
    fn ShowCreateView(&self) -> &str {
        ""
    }
    fn AvgRowLength(&self) -> u64 {
        // 分块估算用，mock 固定 0。
        0
    }
    fn HasImplicitRowID(&self) -> bool {
        self.has_implicit_row_id
    }
    fn ColumnInfos(&self) -> Vec<ColumnInfo> {
        // Parquet/精确列测试使用 with_column_info 填充。
        self.column_infos.clone()
    }
}

impl TableDataIR for mockTableIR {
    // mock 不访问真实连接，Start/Close 均为空操作。
    fn Start(&mut self, _tctx: &tcontext::Context, _conn: &Conn) -> Result<()> {
        Ok(())
    }
    fn Rows(&mut self) -> Box<dyn SQLRowIter> {
        // 用 stubs::Rows + newRowIter 包装内存 data。
        let mut rows = Rows::new(self.col_types.clone(), self.data.clone());
        // Go mock 通过 sqlmock.RowError 暴露注入的行级错误；Rust Rows 将等价错误
        // 保存在结果集上，供 SQLRowIter::Error 在消费结束后返回。
        rows.err = self.row_err.clone();
        Box::new(newRowIter(rows, self.col_types.len()))
    }
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
    fn RawRows(&mut self) -> Option<&mut Rows> {
        // 不提供底层 Rows 可变借用，走 Box<dyn SQLRowIter> 路径。
        None
    }
}

// 把字符串单元格编码为 Option<Vec<u8>>，与 sql.RawBytes 语义一致。
pub fn bytes_cell(s: &str) -> Option<Vec<u8>> {
    Some(s.as_bytes().to_vec())
}
// NULL 单元格占位，CSV/SQL NULL 编码由 RowReceiver 处理。
pub fn null_cell() -> Option<Vec<u8>> {
    None
}

// 引用 Atomic/Arc/Mutex/Duration 以免本 helper 模块在 --no-default-features 下触发 unused 警告。
#[allow(dead_code)]
fn _keep() {
    let _ = (
        AtomicBool::new(false),
        AtomicU64::new(0),
        Duration::from_secs(0),
        Arc::new(Mutex::new(())),
    );
}
