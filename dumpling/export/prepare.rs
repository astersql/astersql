// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这个文件负责把“准备导出什么”整理成后续 dump 流程可消费的数据结构。
// 内容主要分成三块：
// 1. 输出文件模板的默认值与解析入口；
// 2. 待导出数据库列表的发现与显式配置校验；
// 3. 库表集合、表类型以及若干便于后续查询/过滤的辅助结构。
// 它本身不执行真正的导出，而是为 dumper 准备稳定、可比较的输入清单。

pub const outputFileTemplateSchema: &str = "schema";
pub const outputFileTemplateTable: &str = "table";
pub const outputFileTemplateView: &str = "view";
pub const outputFileTemplateSequence: &str = "sequence";
pub const outputFileTemplateData: &str = "data";
pub const outputFileTemplatePolicy: &str = "placement-policy";
// 匿名结果集导出时默认落到 `result.<index>`，避免没有库表名时无法生成文件名。

pub const DefaultAnonymousOutputFileTemplateText: &str = "result.{{.Index}}";
// 默认模板只初始化一次，避免每次解析都重新构造同样的静态骨架。

static DEFAULT_OUTPUT_FILE_TEMPLATE: OnceLock<OutputTemplate> = OnceLock::new();

// 返回 clone，让调用者可以继续 Parse/改写而不污染全局默认模板。
// 这样测试和调用方都能安全地在本地模板上继续修改。
pub fn DefaultOutputFileTemplate() -> OutputTemplate {
    DEFAULT_OUTPUT_FILE_TEMPLATE
        .get_or_init(|| {
            let mut template = OutputTemplate::default_dumpling();
            // Go's defaultOutputFileTemplateBase also defines the post-schema
            // object templates. Keep them here, where the default template is
            // owned, instead of silently falling back to the definition name.
            template.defines.insert(
                "event".into(),
                "{{fn .DB}}.{{fn .Table}}-schema-post".into(),
            );
            template.defines.insert(
                "function".into(),
                "{{fn .DB}}.{{fn .Table}}-schema-post".into(),
            );
            template.defines.insert(
                "procedure".into(),
                "{{fn .DB}}.{{fn .Table}}-schema-post".into(),
            );
            template.defines.insert(
                "trigger".into(),
                "{{fn .DB}}.{{fn .Table}}-schema-triggers".into(),
            );
            template
        })
        .clone()
}

// 解析时总是从默认模板副本出发，保持 Go 侧“默认占位符永远可用”的语义。
// 即使用户只传入局部片段，也不会丢掉默认模板携带的上下文。
pub fn ParseOutputFileTemplate(text: &str) -> Result<OutputTemplate> {
    let mut t = DefaultOutputFileTemplate().Clone();
    t.Parse(text)?;
    Ok(t)
}

pub fn prepareDumpingDatabases(
    tctx: &tcontext::Context,
    conf: &Config,
    db: &Conn,
) -> Result<Vec<String>> {
    // 先取真实实例里的数据库列表，再套用 filter 和显式白名单。
    // 这里的顺序很重要：必须先知道真实存在的数据库，才能判断用户配置是否合法。
    let mut databases = ShowDatabases(db)?;
    databases = filterDatabases(tctx, conf, databases);
    if conf.Databases.is_empty() {
        // 未显式指定时，过滤后的真实结果就是最终导出目标。
        return Ok(databases);
    }
    // 显式指定了 databases 时，需要校验用户列出的每个名字都真实存在。
    let db_map: HashMap<&str, ()> = databases.iter().map(|d| (d.as_str(), ())).collect();
    let mut not_exists = Vec::new();
    for database in &conf.Databases {
        if !db_map.contains_key(database.as_str()) {
            not_exists.push(database.clone());
        }
    }
    if !not_exists.is_empty() {
        return Err(errors_errorf(format!(
            "Unknown databases [{}]",
            not_exists.join(",")
        )));
    }
    Ok(conf.Databases.clone())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i8)]
pub enum TableType {
    // 基础表、视图、序列分别对应 prepare/list 阶段需要区分的三类对象。
    #[default]
    TableTypeBase = 0,
    TableTypeView = 1,
    TableTypeSequence = 2,
}

// 文本字面直接对齐 information_schema / SHOW 语句返回值。
pub const TableTypeBaseStr: &str = "BASE TABLE";
pub const TableTypeViewStr: &str = "VIEW";
pub const TableTypeSequenceStr: &str = "SEQUENCE";

impl TableType {
    pub fn String(self) -> &'static str {
        // 导出时很多 SQL 构造和测试断言都依赖这组稳定字符串。
        match self {
            Self::TableTypeBase => TableTypeBaseStr,
            Self::TableTypeView => TableTypeViewStr,
            Self::TableTypeSequence => TableTypeSequenceStr,
        }
    }
}

pub fn ParseTableType(s: &str) -> Result<TableType> {
    // 未知类型必须报错，避免把新枚举值误归到 base table。
    match s {
        TableTypeBaseStr => Ok(TableType::TableTypeBase),
        TableTypeViewStr => Ok(TableType::TableTypeView),
        TableTypeSequenceStr => Ok(TableType::TableTypeSequence),
        _ => Err(errors_errorf(format!("unknown table type {s}"))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    // Name 是对象名；AvgRowLength 主要给分块/估算使用；Type 决定后续导出路径。
    pub Name: String,
    pub AvgRowLength: u64,
    pub Type: TableType,
}

impl TableInfo {
    pub fn Equals(&self, other: &TableInfo) -> bool {
        // 与 Go 一样只比较名字和类型，故意不把统计信息纳入“身份”等价。
        self.Name == other.Name && self.Type == other.Type
    }
}

// 库名 -> 该库下对象列表，是 prepare 阶段最常用的中间结果。
pub type DatabaseTables = HashMap<String, Vec<TableInfo>>;

pub fn NewDatabaseTables() -> DatabaseTables {
    // 单独提供构造函数，是为了让调用点写法保持和 Go 的 `make(...)` 辅助接近。
    HashMap::new()
}

pub trait DatabaseTablesExt {
    // 这些 helper 让调用点用接近 Go 链式 append 的写法组织库表清单。
    fn AppendTable(&mut self, db_name: impl Into<String>, table: TableInfo) -> &mut Self;
    fn AppendTables(
        &mut self,
        db_name: impl Into<String>,
        table_names: &[String],
        avg_row_lengths: &[u64],
    ) -> &mut Self;
    fn AppendViews(&mut self, db_name: impl Into<String>, view_names: &[&str]) -> &mut Self;
    fn Merge(&mut self, other: DatabaseTables);
    fn Literal(&self) -> String;
}

impl DatabaseTablesExt for DatabaseTables {
    fn AppendTable(&mut self, db_name: impl Into<String>, table: TableInfo) -> &mut Self {
        // 单表追加是最基础的原语，其他批量 helper 最终都等价于重复 push。
        self.entry(db_name.into()).or_default().push(table);
        self
    }
    fn AppendTables(
        &mut self,
        db_name: impl Into<String>,
        table_names: &[String],
        avg_row_lengths: &[u64],
    ) -> &mut Self {
        // 批量普通表默认都标成 base table，并保留调用方给出的平均行长。
        let db = db_name.into();
        for (i, t) in table_names.iter().enumerate() {
            self.entry(db.clone()).or_default().push(TableInfo {
                Name: t.clone(),
                AvgRowLength: avg_row_lengths[i],
                Type: TableType::TableTypeBase,
            });
        }
        self
    }
    fn AppendViews(&mut self, db_name: impl Into<String>, view_names: &[&str]) -> &mut Self {
        // 视图没有平均行长概念，这里统一填 0，只保留名字和类型。
        let db = db_name.into();
        for v in view_names {
            self.entry(db.clone()).or_default().push(TableInfo {
                Name: (*v).to_string(),
                AvgRowLength: 0,
                Type: TableType::TableTypeView,
            });
        }
        self
    }
    fn Merge(&mut self, other: DatabaseTables) {
        // Merge 只做拼接，不尝试去重，保持和 Go 版相同的“调用方负责消重”约束。
        for (name, infos) in other {
            self.entry(name).or_default().extend(infos);
        }
    }
    fn Literal(&self) -> String {
        // Literal 主要服务日志/调试输出，不追求稳定排序。
        let mut b = String::from("tables list\n\n");
        for (db_name, tables) in self {
            b.push_str("schema ");
            b.push_str(db_name);
            b.push_str(" :[");
            for tbl in tables {
                b.push_str(&tbl.Name);
                b.push_str(", ");
            }
            b.push(']');
        }
        b
    }
}

pub fn DatabaseTablesToMap(d: &DatabaseTables) -> HashMap<String, HashMap<String, ()>> {
    // 转成 set-like 结构时只保留 base table，视图/序列不会进入需要快速判断的路径。
    // 结果常用于生成 LOCK TABLES、过滤命中判断等只关心普通表名字的流程。
    let mut mp = HashMap::new();
    for (name, infos) in d {
        let mut inner = HashMap::new();
        for info in infos {
            if info.Type == TableType::TableTypeBase {
                inner.insert(info.Name.clone(), ());
            }
        }
        mp.insert(name.clone(), inner);
    }
    mp
}
