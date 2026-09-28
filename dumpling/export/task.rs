// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 导出任务类型定义，对应 Go `export/task.go`。
// Dumpling 把一次导出拆成若干 Task：库/表/视图/序列/策略元数据，以及分块表数据。
// Writer 按 Task 类型分发到不同的 Write* 方法；Brief 仅用于日志与进度展示。

// Task 是导出工作单元的最小接口，对应 Go `export.Task`。
// 实现者负责在 Brief 中给出人类可读的摘要，便于并发 Writer 日志对齐。
pub trait Task: Send {
    fn Brief(&self) -> String;
}

// 导出单个数据库 CREATE DATABASE 语句的任务。
pub struct TaskDatabaseMeta {
    // 目标库名，Brief 与输出路径模板均引用此字段。
    pub DatabaseName: String,
    // SHOW CREATE DATABASE 得到的 DDL 正文。
    pub CreateDatabaseSQL: String,
}
// 导出单张表 CREATE TABLE 语句的任务。
pub struct TaskTableMeta {
    pub DatabaseName: String,
    pub TableName: String,
    // CREATE TABLE 原文，Writer 经 writeMetaToFile 落盘。
    pub CreateTableSQL: String,
}
// 导出视图元数据：底层表 DDL 与 CREATE VIEW 均需落盘，与 Go 字段一致。
pub struct TaskViewMeta {
    pub DatabaseName: String,
    pub ViewName: String,
    pub CreateTableSQL: String,
    pub CreateViewSQL: String,
}
// 导出 SEQUENCE 定义的任务（TiDB 扩展对象）。
pub struct TaskSequenceMeta {
    pub DatabaseName: String,
    pub SequenceName: String,
    pub CreateSequenceSQL: String,
}
// 导出 placement policy DDL 的任务。
pub struct TaskPolicyMeta {
    pub PolicyName: String,
    pub CreatePolicySQL: String,
}
// 导出表数据分块：Meta 描述表结构，Data 提供行迭代，ChunkIndex/TotalChunks 标识分片进度。
pub struct TaskTableData {
    // Meta 提供库表名、列类型等元信息。
    pub Meta: Box<dyn TableMeta>,
    // Data 封装行迭代 IR，Writer 写数据前会 Start/Close。
    pub Data: Box<dyn TableDataIR>,
    // ChunkIndex 从 0 起，与 outputFileNamer.Index 一致。
    pub ChunkIndex: i32,
    // TotalChunks 供 Brief 展示 (i/total) 进度。
    pub TotalChunks: i32,
}

// 构造数据库元数据任务，字段命名与 Go NewTaskDatabaseMeta 一致。
pub fn NewTaskDatabaseMeta(
    db_name: impl Into<String>,
    create_sql: impl Into<String>,
) -> TaskDatabaseMeta {
    TaskDatabaseMeta {
        DatabaseName: db_name.into(),
        CreateDatabaseSQL: create_sql.into(),
    }
}
pub fn NewTaskTableMeta(
    db: impl Into<String>,
    tbl: impl Into<String>,
    create_sql: impl Into<String>,
) -> TaskTableMeta {
    TaskTableMeta {
        DatabaseName: db.into(),
        TableName: tbl.into(),
        CreateTableSQL: create_sql.into(),
    }
}
pub fn NewTaskViewMeta(
    db: impl Into<String>,
    tbl: impl Into<String>,
    create_table: impl Into<String>,
    create_view: impl Into<String>,
) -> TaskViewMeta {
    TaskViewMeta {
        DatabaseName: db.into(),
        ViewName: tbl.into(),
        CreateTableSQL: create_table.into(),
        CreateViewSQL: create_view.into(),
    }
}
pub fn NewTaskSequenceMeta(
    db: impl Into<String>,
    tbl: impl Into<String>,
    create_sql: impl Into<String>,
) -> TaskSequenceMeta {
    TaskSequenceMeta {
        DatabaseName: db.into(),
        SequenceName: tbl.into(),
        CreateSequenceSQL: create_sql.into(),
    }
}
pub fn NewTaskPolicyMeta(
    policy: impl Into<String>,
    create_sql: impl Into<String>,
) -> TaskPolicyMeta {
    TaskPolicyMeta {
        PolicyName: policy.into(),
        CreatePolicySQL: create_sql.into(),
    }
}
// current_chunk 从 0 起计，与 Go NewTaskTableData 的分块语义相同。
pub fn NewTaskTableData(
    meta: Box<dyn TableMeta>,
    data: Box<dyn TableDataIR>,
    current_chunk: i32,
    total_chunks: i32,
) -> TaskTableData {
    TaskTableData {
        Meta: meta,
        Data: data,
        ChunkIndex: current_chunk,
        TotalChunks: total_chunks,
    }
}

// Brief 文案与 Go 保持一致（含 dababase 拼写），避免日志对照时产生差异。
impl Task for TaskDatabaseMeta {
    fn Brief(&self) -> String {
        format!("meta of dababase '{}'", self.DatabaseName)
    }
}
impl Task for TaskTableMeta {
    fn Brief(&self) -> String {
        format!("meta of table '{}'.'{}'", self.DatabaseName, self.TableName)
    }
}
impl Task for TaskViewMeta {
    fn Brief(&self) -> String {
        format!("meta of view '{}'.'{}'", self.DatabaseName, self.ViewName)
    }
}
impl Task for TaskSequenceMeta {
    fn Brief(&self) -> String {
        format!(
            "meta of sequence '{}'.'{}'",
            self.DatabaseName, self.SequenceName
        )
    }
}
impl Task for TaskPolicyMeta {
    fn Brief(&self) -> String {
        format!("meta of placement policy '{}'", self.PolicyName)
    }
}
impl Task for TaskTableData {
    fn Brief(&self) -> String {
        format!(
            "data of table '{}'.'{}'({}/{})",
            self.Meta.DatabaseName(),
            self.Meta.TableName(),
            self.ChunkIndex,
            self.TotalChunks
        )
    }
}

// TaskEnum 把各具体任务装箱，供 Writer::handleTask 统一 match 分发。
pub enum TaskEnum {
    DatabaseMeta(TaskDatabaseMeta),
    TableMeta(TaskTableMeta),
    ViewMeta(TaskViewMeta),
    SequenceMeta(TaskSequenceMeta),
    PolicyMeta(TaskPolicyMeta),
    // TableData 是唯一需要 mut Data 的变体，Writer 会调用 IR 迭代。
    TableData(TaskTableData),
}
impl Task for TaskEnum {
    fn Brief(&self) -> String {
        match self {
            Self::DatabaseMeta(t) => t.Brief(),
            Self::TableMeta(t) => t.Brief(),
            Self::ViewMeta(t) => t.Brief(),
            Self::SequenceMeta(t) => t.Brief(),
            Self::PolicyMeta(t) => t.Brief(),
            Self::TableData(t) => t.Brief(),
        }
    }
}
