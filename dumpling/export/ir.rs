// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这个文件定义 dumpling 导出流程内部最核心的一组中间抽象接口：
// 表数据、表元信息、行迭代器、可写字符串、元信息对象等都会通过这些 trait 交互。
// 它们的职责是把“如何读取数据库结果”和“如何把结果写成导出文件”解耦，
// 让 writer、meta task 和 table data task 只依赖统一接口而不是具体实现。
// 因此这里更像一层协议定义，而不是直接承载具体业务逻辑。

// TableDataIR 表示“一个可启动、可逐行消费、可关闭”的表数据来源。
// 不同实现可以来自整表查询、分块查询或多 SQL 拼接，但 writer 看见的接口一致。
pub trait TableDataIR: Send {
    fn Start(&mut self, tctx: &tcontext::Context, conn: &Conn) -> Result<()>;
    fn Rows(&mut self) -> Box<dyn SQLRowIter>;
    fn Close(&mut self) -> Result<()>;
    fn RawRows(&mut self) -> Option<&mut Rows>;
}

// TableMeta 聚合 writer 生成 schema/data 文件时需要的结构化元信息。
// 它既服务 create 语句落盘，也服务 select 字段、列类型和特殊注释拼装。
pub trait TableMeta: Send + Sync {
    fn DatabaseName(&self) -> &str;
    fn TableName(&self) -> &str;
    fn ColumnCount(&self) -> u32;
    fn ColumnTypes(&self) -> Vec<String>;
    fn ColumnNames(&self) -> Vec<String>;
    fn SelectedField(&self) -> &str;
    fn SelectedLen(&self) -> i32;
    fn SpecialComments(&self) -> Box<dyn StringIter>;
    fn ShowCreateTable(&self) -> &str;
    fn ShowCreateView(&self) -> &str;
    fn AvgRowLength(&self) -> u64;
    fn HasImplicitRowID(&self) -> bool;
    fn ColumnInfos(&self) -> Vec<ColumnInfo>;
}

// SQLRowIter 抽象数据库结果集的逐行推进与解码接口。
// 这样上层不必直接依赖 `Rows` 的具体生命周期管理细节。
pub trait SQLRowIter: Send {
    fn Decode(&mut self, row: &mut dyn RowReceiver) -> Result<()>;
    fn Next(&mut self);
    fn Error(&self) -> Option<Error>;
    fn HasNext(&self) -> bool;
    fn Close(&mut self) -> Result<()>;
}

// Stringer 负责把单行或单值写入输出缓冲区，兼容 SQL/CSV 两条路径。
// 一个实现通常同时知道自己的原始字节和文本转义规则。
pub trait Stringer {
    fn WriteToBuffer(&self, bf: &mut Vec<u8>, escape_backslash: bool);
    fn WriteToBufferInCsv(&self, bf: &mut Vec<u8>, escape_backslash: bool, opt: &crate::csvOption);
    fn GetRawBytes(&self) -> Vec<RawBytes>;
}

// RowReceiver 接收 decode 后的列值绑定结果，类似 Go 中传入的扫描目标。
// 它与 Stringer 拆开定义，是为了允许“只接收、不序列化”的中间对象存在。
pub trait RowReceiver {
    fn BindAddress(&mut self, args: &mut [RawBytes]);
}

// 组合 trait，表示“既能接收绑定，也能序列化自己”。
pub trait RowReceiverStringer: RowReceiver + Stringer {}

// StringIter 用于顺序产出 special comments 等文本片段。
// 这里故意用迭代器协议，而不是直接暴露 `Vec<String>`，方便惰性生成。
pub trait StringIter: Send {
    fn Next(&mut self) -> String;
    fn HasNext(&self) -> bool;
}

// MetaIR 表示 database/table/view/sequence 这类元信息任务的统一接口。
// writer 只要拿到这个接口，就能以统一方式写出不同种类的元信息文件。
pub trait MetaIR: Send {
    fn SpecialComments(&self) -> Box<dyn StringIter>;
    fn TargetName(&self) -> &str;
    fn MetaSQL(&mut self) -> String;
}

pub fn decodeFromRows(
    rows: &mut Rows,
    args: &mut [RawBytes],
    row: &mut dyn RowReceiver,
) -> Result<()> {
    // Go first binds the scan destinations, so preserve that observable side effect even when
    // Scan fails. Rust's RawBytes adapter stores values rather than pointers, therefore a
    // successful scan needs a second bind to copy the filled values into the receiver.
    row.BindAddress(args);
    // `args` 由调用方复用，可以减少逐行扫描时的临时分配。
    if let Err(err) = rows.Scan(args) {
        // 扫描失败时顺手关闭 rows，避免调用方再持有一个半失效结果集。
        let _ = rows.Close();
        return Err(errors_trace(err));
    }
    row.BindAddress(args);
    Ok(())
}

pub fn setTableMetaFromRows(server_type: ServerType, rows: &Rows) -> Result<Box<dyn TableMeta>> {
    // 这个 helper 根据查询结果的列类型和列名，快速构造一个匿名 tableMeta。
    // 常见于 `--sql` 或尚未绑定真实表名时的结果集元信息推导。
    let tps = rows.ColumnTypes()?;
    let mut nms = rows.Columns()?;
    for n in &mut nms {
        // 列名统一补反引号，和真正的 SELECT 字段列表输出保持一致。
        *n = wrapBackTicks(n);
    }
    Ok(Box::new(tableMeta {
        database: String::new(),
        table: String::new(),
        col_types: tps,
        selected_field: nms.join(","),
        selected_len: nms.len() as i32,
        spec_cmts: getSpecialComments(server_type),
        show_create_table: String::new(),
        show_create_view: String::new(),
        avg_row_length: 0,
        has_implicit_row_id: false,
    }))
}
