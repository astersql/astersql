// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 本文件为 `ir.rs` 中定义的导出中间接口提供具体实现，
// 负责把数据库结果集包装成可迭代、可序列化、可关闭的运行时对象。
// 这里的实现大多不直接关心导出编排，而是专注于“如何把 rows 变成 IR”：
// 单查询、跨多查询 chunk、表元信息、普通元信息和特殊注释都在这里落地。
// 大多数类型都对齐 Go 里的同名结构，因此注释会强调它们的生命周期和数据流边界。

// rowIter 是最基础的 `SQLRowIter`：围绕一个 `Rows` 做顺序推进。
// 对应 Go 的 `rowIter`，上层 writer 只依赖接口而不感知具体游标类型。
pub struct rowIter {
    // rows 为 None 说明迭代器已被消费或尚未初始化。
    // 这允许 `Close`/`Next` 在空状态下保持幂等。
    pub rows: Option<Rows>,
    // has_next 缓存上一轮 `Next()` 的结果，`HasNext()` 不再推进游标。
    pub has_next: bool,
    // args 是每行扫描复用的原始字节槽，避免重复分配。
    pub args: Vec<RawBytes>,
}

pub fn newRowIter(mut rows: Rows, arg_len: usize) -> rowIter {
    // 构造时先预取一行，保持与 Go 版 `newRowIter` 的使用习惯一致。
    let has_next = rows.Next();
    rowIter {
        rows: Some(rows),
        has_next,
        args: vec![RawBytes(None); arg_len],
    }
}

impl SQLRowIter for rowIter {
    fn Close(&mut self) -> Result<()> {
        // 允许多次关闭：没有 rows 时直接视为成功。
        if let Some(r) = &mut self.rows {
            return r.Close();
        }
        Ok(())
    }
    fn Decode(&mut self, row: &mut dyn RowReceiver) -> Result<()> {
        // 真正的扫描/绑定逻辑统一委托给 `decodeFromRows`。
        let rows = self.rows.as_mut().unwrap();
        if rows.closed {
            return Err(errors_new("sql: Rows are closed"));
        }
        decodeFromRows(rows, &mut self.args, row)
    }
    fn Error(&self) -> Option<Error> {
        // 错误延迟从底层 rows 提取，和 Go 的 `rows.Err()` 语义一致。
        self.rows.as_ref().and_then(|r| r.Err()).map(errors_trace)
    }
    fn Next(&mut self) {
        // Next 只推进一次底层游标，并把结果缓存到 has_next。
        if let Some(r) = &mut self.rows {
            self.has_next = r.Next();
        }
    }
    fn HasNext(&self) -> bool {
        self.has_next
    }
}

// multiQueriesChunkIter 把多条 SQL 查询结果串成一个逻辑上的连续迭代器。
pub struct multiQueriesChunkIter {
    // tctx 只用于记录调试日志，不直接参与扫描逻辑。
    pub tctx: tcontext::Context,
    // conn 在切换查询时被重复复用，不为每条 SQL 重新建连接。
    pub conn: Conn,
    pub rows: Option<Rows>,
    pub has_next: bool,
    // id 指向下一条尚未启动的查询。
    pub id: usize,
    pub queries: Vec<String>,
    // 所有查询共用一套扫描缓冲，保证跨 chunk 迭代器的内存形态一致。
    pub args: Vec<RawBytes>,
    // err 用来缓存跨查询切换过程中遇到的终态错误。
    pub err: Option<Error>,
}

pub fn newMultiQueryChunkIter(
    tctx: tcontext::Context,
    conn: Conn,
    queries: Vec<String>,
    arg_len: usize,
) -> multiQueriesChunkIter {
    let mut r = multiQueriesChunkIter {
        tctx,
        conn,
        rows: None,
        has_next: false,
        id: 0,
        queries,
        args: vec![RawBytes(None); arg_len],
        err: None,
    };
    // 构造后立刻切到第一段有效 rows，调用方拿到即可直接消费。
    r.nextRows();
    r
}

impl multiQueriesChunkIter {
    pub fn nextRows(&mut self) {
        // 所有查询都耗尽后，直接把 `has_next` 置成 false。
        if self.id >= self.queries.len() {
            self.has_next = false;
            return;
        }
        while self.id < self.queries.len() {
            // 切换到下一条查询前，必须先关闭上一段 rows 并读取其错误状态。
            if let Some(mut rows) = self.rows.take() {
                if let Err(err) = rows.Close() {
                    self.has_next = false;
                    self.err = Some(errors_trace(err));
                    return;
                }
                if let Some(err) = rows.Err() {
                    self.has_next = false;
                    self.err = Some(errors_trace(err));
                    return;
                }
            }
            self.tctx.L().Debug(
                "try to start nextRows",
                [Field::string("query", self.queries[self.id].clone())],
            );
            match self.conn.QueryContext(&self.queries[self.id]) {
                Ok(mut rows) => {
                    // 新 rows 自身如果已经带错，应立即终止而不是继续推进。
                    if let Some(err) = rows.Err() {
                        self.has_next = false;
                        self.err = Some(errors_trace(err));
                        return;
                    }
                    self.id += 1;
                    self.has_next = rows.Next();
                    self.rows = Some(rows);
                    if self.has_next {
                        // 找到第一段非空结果集后立即返回，后续再按需继续切换。
                        return;
                    }
                }
                Err(err) => {
                    // 查询启动失败直接成为终态错误，不再尝试后续 SQL。
                    self.has_next = false;
                    self.err = Some(errors_trace(err));
                    return;
                }
            }
        }
    }
}

impl SQLRowIter for multiQueriesChunkIter {
    fn Close(&mut self) -> Result<()> {
        // 已记录错误时优先返回该错误，保持调用方可见性。
        // 这能避免“关闭成功”掩盖掉更早发生的跨查询切换错误。
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        if let Some(r) = &mut self.rows {
            return r.Close();
        }
        Ok(())
    }
    fn Decode(&mut self, row: &mut dyn RowReceiver) -> Result<()> {
        // 没有有效 rows 时，说明调用方在错误的时机请求了解码。
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        let Some(rows) = self.rows.as_mut() else {
            return Err(errors_errorf(format!(
                "no valid rows found, id: {}",
                self.id
            )));
        };
        if rows.closed {
            return Err(errors_new("sql: Rows are closed"));
        }
        decodeFromRows(rows, &mut self.args, row)
    }
    fn Error(&self) -> Option<Error> {
        // 先看自身缓存的跨查询错误，再退回到底层 rows 错误。
        if self.err.is_some() {
            return self.err.clone();
        }
        self.rows.as_ref().and_then(|r| r.Err()).map(errors_trace)
    }
    fn Next(&mut self) {
        // 当前 rows 耗尽后，自动切到下一条查询结果，向上表现为连续流。
        if self.err.is_none() {
            if let Some(r) = &mut self.rows {
                self.has_next = r.Next();
                if !self.has_next {
                    self.nextRows();
                }
            }
        }
    }
    fn HasNext(&self) -> bool {
        self.has_next
    }
}

// stringIter 是 `Vec<String>` 的最轻量适配器，常用来输出 special comments。
pub struct stringIter {
    pub idx: usize,
    pub ss: Vec<String>,
}

pub fn newStringIter(ss: Vec<String>) -> Box<dyn StringIter> {
    // 返回 trait object，调用方无需关心底层到底是切片还是惰性生成器。
    Box::new(stringIter { idx: 0, ss })
}

impl StringIter for stringIter {
    fn Next(&mut self) -> String {
        // 越界时返回空串，和 Go 侧“调用方应先看 HasNext”这一约定兼容。
        if self.idx >= self.ss.len() {
            return String::new();
        }
        let ret = self.ss[self.idx].clone();
        self.idx += 1;
        ret
    }
    fn HasNext(&self) -> bool {
        self.idx < self.ss.len()
    }
}

// tableData 对应“单条查询生成的一段表数据”。
// 它在 Start 时真正打开 rows，在 Rows 时懒创建 rowIter，在 Close 时兜底回收资源。
pub struct tableData {
    // query 就是这段数据真正要执行的 SQL，可以是整表查询也可以是子查询。
    pub query: String,
    pub rows: Option<Rows>,
    // col_len 为扫描缓冲区大小；某些路径会在 Start 后按真实列数回填。
    pub col_len: usize,
    // `--sql` 等路径需要先读出列类型，再推导匿名 table meta。
    pub need_col_types: bool,
    pub col_types: Vec<String>,
    // iter 创建后会在第一次 `Rows()` 调用时被取走，贴近 Go 的“单次消费”语义。
    pub iter: Option<Box<dyn SQLRowIter>>,
}

pub fn newTableData(
    query: impl Into<String>,
    col_length: usize,
    need_col_types: bool,
) -> tableData {
    tableData {
        query: query.into(),
        rows: None,
        col_len: col_length,
        need_col_types,
        col_types: vec![],
        iter: None,
    }
}

impl TableDataIR for tableData {
    fn Start(&mut self, tctx: &tcontext::Context, conn: &Conn) -> Result<()> {
        // 每次 Start 都重新执行 SQL，并清掉上一轮遗留的 iter。
        tctx.L().Debug(
            "try to start tableData",
            [Field::string("query", self.query.clone())],
        );
        let mut rows = conn
            .QueryContext(&self.query)
            .map_err(|e| errors_annotatef(e, format!("sql: {}", self.query)))?;
        if let Some(err) = rows.Err() {
            return Err(errors_annotatef(err, format!("sql: {}", self.query)));
        }
        self.iter = None;
        if self.need_col_types {
            // 当调用方尚不知道结果列信息时，这里顺手把列数和数据库类型抓出来。
            let ns = rows.Columns()?;
            self.col_len = ns.len();
            self.col_types.clear();
            for c in rows.ColumnTypes()? {
                self.col_types.push(c.DatabaseTypeName().to_string());
            }
        }
        self.rows = Some(rows);
        Ok(())
    }
    fn Rows(&mut self) -> Box<dyn SQLRowIter> {
        // rowIter 延迟到真正消费前再创建，避免只做 meta 探测时也抢先推进 rows。
        if self.iter.is_none() {
            let rows = self.rows.take().unwrap_or_default();
            self.iter = Some(Box::new(newRowIter(rows, self.col_len)));
        }
        // trait 目前按值返回迭代器，因此这里遵循“只允许取走一次”的约束。
        if let Some(iter) = self.iter.take() {
            return iter;
        }
        // 再次取走时返回空迭代器哨兵，避免 panic，同时明确表示数据已被消费。
        Box::new(newRowIter(Rows::default(), 0))
    }
    fn Close(&mut self) -> Result<()> {
        // 优先关闭已经暴露给外部的 iter；否则再尝试直接关闭尚未包装的 rows。
        // 两边都为空时返回成功，保持与其他 IR 类型一致的幂等关闭行为。
        if let Some(iter) = &mut self.iter {
            return iter.Close();
        }
        if let Some(rows) = &mut self.rows {
            return rows.Close();
        }
        Ok(())
    }
    fn RawRows(&mut self) -> Option<&mut Rows> {
        self.rows.as_mut()
    }
}

// tableMeta 汇总 writer 生成表 schema/data 文件所需的全部静态元信息。
pub struct tableMeta {
    pub database: String,
    pub table: String,
    // col_types 同时承担列名、数据库类型和 nullable/precision 等信息来源。
    pub col_types: Vec<ColumnType>,
    pub selected_field: String,
    pub selected_len: i32,
    // special comments 会先于 schema/data SQL 输出，用于对齐不同数据库方言习惯。
    pub spec_cmts: Vec<String>,
    pub show_create_table: String,
    pub show_create_view: String,
    pub avg_row_length: u64,
    pub has_implicit_row_id: bool,
}

impl TableMeta for tableMeta {
    fn ColumnInfos(&self) -> Vec<ColumnInfo> {
        // 这里把底层驱动 `ColumnType` 投影成更稳定的导出层结构体。
        self.col_types
            .iter()
            .map(|ct| {
                let (nullable, _) = ct.Nullable();
                let (precision, scale, _) = ct.DecimalSize();
                ColumnInfo {
                    Name: ct.Name().to_string(),
                    DatabaseTypeName: ct.DatabaseTypeName().to_string(),
                    Nullable: nullable,
                    Precision: precision,
                    Scale: scale,
                }
            })
            .collect()
    }
    fn ColumnTypes(&self) -> Vec<String> {
        // 返回 String 而不是借用，避免导出任务跨线程/跨作用域时受限。
        self.col_types
            .iter()
            .map(|c| c.DatabaseTypeName().to_string())
            .collect()
    }
    fn ColumnNames(&self) -> Vec<String> {
        self.col_types
            .iter()
            .map(|c| c.Name().to_string())
            .collect()
    }
    // 下面这些 getter 基本都是零成本投影，确保 writer 不需要了解 `tableMeta` 内部布局。
    fn DatabaseName(&self) -> &str {
        &self.database
    }
    fn TableName(&self) -> &str {
        &self.table
    }
    fn ColumnCount(&self) -> u32 {
        self.col_types.len() as u32
    }
    fn SelectedField(&self) -> &str {
        &self.selected_field
    }
    fn SelectedLen(&self) -> i32 {
        self.selected_len
    }
    fn SpecialComments(&self) -> Box<dyn StringIter> {
        // 每次都新建迭代器，避免前一次消费位置污染后续 writer。
        newStringIter(self.spec_cmts.clone())
    }
    fn ShowCreateTable(&self) -> &str {
        &self.show_create_table
    }
    fn ShowCreateView(&self) -> &str {
        &self.show_create_view
    }
    fn AvgRowLength(&self) -> u64 {
        self.avg_row_length
    }
    fn HasImplicitRowID(&self) -> bool {
        self.has_implicit_row_id
    }
}

// metaData 对应 database/view/sequence 等“只有元 SQL、没有数据行”的任务体。
pub struct metaData {
    pub target: String,
    pub meta_sql: String,
    pub spec_cmts: Vec<String>,
}

impl crate::MetaIR for metaData {
    fn SpecialComments(&self) -> Box<dyn StringIter> {
        // 元信息任务与表数据任务共享同一套 special comments 输出协议。
        newStringIter(self.spec_cmts.clone())
    }
    fn TargetName(&self) -> &str {
        &self.target
    }
    fn MetaSQL(&mut self) -> String {
        // 输出前强制补上 `;\n`，减少上层 writer 为不同来源 SQL 做收尾修补。
        if !self.meta_sql.ends_with(";\n") {
            self.meta_sql.push_str(";\n");
        }
        self.meta_sql.clone()
    }
}

// multiQueriesChunk 对应“由多条 SQL 拼接成的一个逻辑数据块”。
// 常见于无法单 SQL 完成的数据读取场景，但对 writer 仍表现为普通 `TableDataIR`。
pub struct multiQueriesChunk {
    pub tctx: Option<tcontext::Context>,
    pub conn: Option<Conn>,
    pub queries: Vec<String>,
    pub col_len: usize,
    pub iter: Option<Box<dyn SQLRowIter>>,
}

pub fn newMultiQueriesChunk(queries: Vec<String>, col_length: usize) -> multiQueriesChunk {
    multiQueriesChunk {
        tctx: None,
        conn: None,
        queries,
        col_len: col_length,
        iter: None,
    }
}

impl TableDataIR for multiQueriesChunk {
    fn Start(&mut self, tctx: &tcontext::Context, conn: &Conn) -> Result<()> {
        // Start 只缓存运行环境，不立刻执行查询；真正打开 rows 留给 `Rows()`。
        // 这样可以让任务准备阶段保持轻量，直到 writer 真正开始消费数据。
        self.tctx = Some(tctx.clone());
        self.conn = Some(conn.clone());
        self.iter = None;
        Ok(())
    }
    fn Rows(&mut self) -> Box<dyn SQLRowIter> {
        // 第一次请求行迭代器时，才把所有 query 串成 multiQueriesChunkIter。
        if self.iter.is_none() {
            let tctx = self.tctx.clone().unwrap_or_else(tcontext::Background);
            let conn = self.conn.clone().unwrap_or_default();
            self.iter = Some(Box::new(newMultiQueryChunkIter(
                tctx,
                conn,
                self.queries.clone(),
                self.col_len,
            )));
        }
        // 与 tableData 一样，这里也遵守“Rows 只取走一次”的消费模型。
        self.iter.take().unwrap()
    }
    fn Close(&mut self) -> Result<()> {
        // 若迭代器尚未被取走，则由这里承担最后的资源释放责任。
        // 一旦迭代器已经交给外部，外部就应负责最终关闭。
        if let Some(iter) = &mut self.iter {
            return iter.Close();
        }
        Ok(())
    }
    fn RawRows(&mut self) -> Option<&mut Rows> {
        None
    }
}

pub fn getSpecialComments(server_type: ServerType) -> Vec<String> {
    // MySQL/TiDB 与 MariaDB 的注释 SQL 细节不同，因此分别返回两组模板。
    // 未知数据库类型则保持空集合，让 writer 不输出额外方言前置语句。
    // 这些语句通常会写在导出文件最前面，用来降低导入时的兼容性问题。
    match server_type {
        ServerType::ServerTypeMySQL | ServerType::ServerTypeTiDB => vec![
            // 先关闭外键检查，再强制 names binary，贴近上游默认导出习惯。
            "/*!40014 SET FOREIGN_KEY_CHECKS=0*/;".into(),
            "/*!40101 SET NAMES binary*/;".into(),
        ],
        ServerType::ServerTypeMariaDB => vec![
            // MariaDB 在外键检查语句上不再使用同样的版本注释包裹方式。
            "/*!40101 SET NAMES binary*/;".into(),
            "SET FOREIGN_KEY_CHECKS=0;".into(),
        ],
        _ => vec![],
    }
}
