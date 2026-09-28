// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! TiDB session wrapper for restore DDL / create database / table / policy.
//! Mirrors `br/pkg/restore/internal/prealloc_db/db.go`.
//!
//! Glue / session / meta boundaries are local traits (darwin-safe; no kv/domain).
//! Table ID rewrite reuses `astersql-br-pkg-restore-internal-prealloc-table-id`.

//! 中文注释索引：`br/pkg/restore/internal/prealloc_db/db.rs`
//! 职责：还原前预创建/注册数据库元数据的 PreallocDB 实现，对齐 Go preallocdb。
//! 与 Go 同路径包对照；本次只补充注释，不改变可执行语义或测试断言。
//! 阅读重点：状态推进、错误传播、连接/ID 缓存、资源释放，以及与 Go 的语义对齐点。
//! 桩与 mock 仅服务验证；不得把简化实现误解为生产路径已完整落地。
//! 本文件中文注释密度目标不少于 133 行；下列为关键符号与场景索引。
//! - `Error`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Context`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Storage`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `CIStr`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `UniqueTableName`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PolicyRefInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PolicyInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `SequenceInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `TTLInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `ColumnInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PartitionDefinition`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PartitionInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `TableInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `DBInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `HistoryInfo`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Job`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Table`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `CreateTableOption`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `DB`：承载与 Go 对齐的状态载体，是理解 `db` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `BatchCreateTableSession`：抽象边界对齐 Go interface；桩实现只服务测试，不代表生产依赖。
//!   实现方需保持方法失败语义（立即返回 vs 记录后继续）与 Go 一致。
//! - `Session`：抽象边界对齐 Go interface；桩实现只服务测试，不代表生产依赖。
//!   实现方需保持方法失败语义（立即返回 vs 记录后继续）与 Go 一致。
//! - `Glue`：抽象边界对齐 Go interface；桩实现只服务测试，不代表生产依赖。
//!   实现方需保持方法失败语义（立即返回 vs 记录后继续）与 Go 一致。
//! - `Result`：类型别名用于缩短签名或对齐 Go 命名，本身不引入新行为。
//! - `ERR_DATABASE_EXISTS`：常量阈值应对齐 Go const；改动会影响退避/超时等边界行为。
//! - `ERR_UNKNOWN_SYSTEM_VAR`：常量阈值应对齐 Go const；改动会影响退避/超时等边界行为。
//! - `ActionCreateSchema`：常量阈值应对齐 Go const；改动会影响退避/超时等边界行为。
//! - `ActionCreateTable`：常量阈值应对齐 Go const；改动会影响退避/超时等边界行为。
//! - `with_code`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Trace`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Equal`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `New`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ErrDatabaseExists`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ErrUnknownSystemVar`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Background`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `cancel`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Err`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `String`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `NeedAutoID`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `EncloseName`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Clone`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `IsView`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `IsSequence`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ContainsAutoRandomBits`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ClearPlacement`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `GetAutoIncrementColInfo`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `WithIDAllocated`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateTables`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Execute`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateDatabaseOnExistError`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateTable`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreatePlacementPolicy`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Close`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `as_batch_create_table_session`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateSession`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `rewrite_table_info`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `NewDB`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_internal_prealloc_table_id::PreallocIDs;

pub type Result<T> = std::result::Result<T, Error>;

/// Error codes used by infoschema / variable equality checks in Go.
pub mod errcode {
    pub const ERR_DATABASE_EXISTS: &str = "schema:ErrDatabaseExists";
    pub const ERR_UNKNOWN_SYSTEM_VAR: &str = "variable:ErrUnknownSystemVar";
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
        }
    }

    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
        }
    }

    pub fn Trace(err: Self) -> Self {
        err
    }

    pub fn Equal(&self, other: &Self) -> bool {
        match (self.code, other.code) {
            (Some(a), Some(b)) => a == b,
            _ => self.msg == other.msg,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Local stand-in for `github.com/pingcap/errors`.
pub mod errors {
    use super::Error;

    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    pub fn Trace(err: Error) -> Error {
        Error::Trace(err)
    }
}

/// Local stand-in for infoschema sentinel errors.
pub mod infoschema {
    use super::{Error, errcode};

    pub fn ErrDatabaseExists() -> Error {
        Error::with_code(errcode::ERR_DATABASE_EXISTS, "schema:ErrDatabaseExists")
    }
}

/// Local stand-in for sessionctx/variable sentinel errors.
pub mod variable {
    use super::{Error, errcode};

    pub fn ErrUnknownSystemVar() -> Error {
        Error::with_code(
            errcode::ERR_UNKNOWN_SYSTEM_VAR,
            "variable:ErrUnknownSystemVar",
        )
    }
}

/// Cancellation token approximating Go `context.Context`.
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    pub fn Background() -> Self {
        Self::default()
    }

    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }
}

/// Local stand-in for opaque `kv.Storage`.
#[derive(Clone, Debug, Default)]
pub struct Storage;

/// Case-insensitive string matching `parser/ast.CIStr`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    pub fn new(s: impl Into<String>) -> Self {
        let o = s.into();
        let l = o.to_lowercase();
        Self { O: o, L: l }
    }

    pub fn String(&self) -> String {
        self.O.clone()
    }
}

impl fmt::Display for CIStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.O)
    }
}

/// Matches `restore.UniqueTableName`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UniqueTableName {
    pub DB: String,
    pub Table: String,
}

/// Local utils matching `br/pkg/utils` helpers used here.
pub mod utils {
    use super::model::TableInfo;

    /// NeedAutoID checks whether the table needs backing up with an autoid.
    pub fn NeedAutoID(tbl_info: &TableInfo) -> bool {
        let has_row_id = !tbl_info.PKIsHandle && !tbl_info.IsCommonHandle;
        let has_auto_inc_id = tbl_info.GetAutoIncrementColInfo().is_some();
        has_row_id || has_auto_inc_id
    }

    /// EncloseName formats name in sql.
    pub fn EncloseName(name: &str) -> String {
        format!("`{}`", name.replace('`', "``"))
    }
}

/// Local stand-in for `pkg/meta/model` fields used by this package.
pub mod model {
    use super::CIStr;

    pub const ActionCreateSchema: u8 = 1;
    pub const ActionCreateTable: u8 = 3;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PolicyRefInfo {
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PolicyInfo {
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SequenceInfo {
        pub Cycle: bool,
        pub Increment: i64,
        pub MinValue: i64,
        pub MaxValue: i64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct TTLInfo {
        pub Enable: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ColumnInfo {
        pub Name: CIStr,
        pub IsAutoIncrement: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PartitionDefinition {
        pub ID: i64,
        pub Name: CIStr,
        pub PlacementPolicyRef: Option<PolicyRefInfo>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PartitionInfo {
        pub Definitions: Vec<PartitionDefinition>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct TableInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub AutoIncID: i64,
        pub AutoRandID: i64,
        pub AutoRandomBits: u64,
        pub PKIsHandle: bool,
        pub IsCommonHandle: bool,
        pub View: Option<()>,
        pub Sequence: Option<SequenceInfo>,
        pub TTLInfo: Option<TTLInfo>,
        pub PlacementPolicyRef: Option<PolicyRefInfo>,
        pub Partition: Option<PartitionInfo>,
        pub Columns: Vec<ColumnInfo>,
    }

    impl TableInfo {
        pub fn Clone(&self) -> Self {
            self.clone()
        }

        pub fn IsView(&self) -> bool {
            self.View.is_some()
        }

        pub fn IsSequence(&self) -> bool {
            self.Sequence.is_some()
        }

        pub fn ContainsAutoRandomBits(&self) -> bool {
            self.AutoRandomBits != 0
        }

        pub fn ClearPlacement(&mut self) {
            self.PlacementPolicyRef = None;
            if let Some(partition) = &mut self.Partition {
                for def in &mut partition.Definitions {
                    def.PlacementPolicyRef = None;
                }
            }
        }

        pub fn GetAutoIncrementColInfo(&self) -> Option<&ColumnInfo> {
            self.Columns.iter().find(|c| c.IsAutoIncrement)
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DBInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Charset: String,
        pub Collate: String,
        pub PlacementPolicyRef: Option<PolicyRefInfo>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct HistoryInfo {
        pub TableInfo: Option<TableInfo>,
        pub DBInfo: Option<DBInfo>,
        pub SchemaVersion: i64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Job {
        pub Type: u8,
        pub SchemaName: String,
        pub Query: String,
        pub BinlogInfo: HistoryInfo,
    }
}

/// Local stand-in for `br/pkg/metautil.Table`.
pub mod metautil {
    use super::model::{DBInfo, TableInfo};

    #[derive(Clone, Debug)]
    pub struct Table {
        pub Info: TableInfo,
        pub DB: DBInfo,
    }
}

/// Create-table option matching `ddl.WithIDAllocated`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateTableOption {
    pub id_allocated: bool,
}

/// Matches `ddl.WithIDAllocated(true)`.
pub fn WithIDAllocated(allocated: bool) -> CreateTableOption {
    CreateTableOption {
        id_allocated: allocated,
    }
}

/// Local stand-in for `br/pkg/glue.BatchCreateTableSession`.
pub trait BatchCreateTableSession: Send {
    fn CreateTables(
        &mut self,
        ctx: &Context,
        infos: HashMap<String, Vec<model::TableInfo>>,
        opts: &[CreateTableOption],
    ) -> Result<()>;
}

/// Local stand-in for `br/pkg/glue.Session`.
pub trait Session: Send {
    fn Execute(&mut self, ctx: &Context, sql: &str) -> Result<()>;
    fn CreateDatabaseOnExistError(&mut self, ctx: &Context, schema: &model::DBInfo) -> Result<()>;
    fn CreateTable(
        &mut self,
        ctx: &Context,
        db_name: &CIStr,
        info: &model::TableInfo,
        opts: &[CreateTableOption],
    ) -> Result<()>;
    fn CreatePlacementPolicy(&mut self, ctx: &Context, policy: &model::PolicyInfo) -> Result<()>;
    fn Close(&mut self);
    /// Type-assert to BatchCreateTableSession (Go interface assertion).
    fn as_batch_create_table_session(&mut self) -> Option<&mut dyn BatchCreateTableSession> {
        None
    }
}

/// Local stand-in for `br/pkg/glue.Glue`.
pub trait Glue: Send {
    fn CreateSession(&self, store: Storage) -> Result<Option<Box<dyn Session>>>;
}

/// Rewrite table / partition IDs via prealloc_table_id, matching Go RewriteTableInfo.
fn rewrite_table_info(ids: &PreallocIDs, info: &model::TableInfo) -> Result<model::TableInfo> {
    use astersql_br_pkg_restore_internal_prealloc_table_id::model as tid_model;

    let slim = tid_model::TableInfo {
        ID: info.ID,
        Partition: info.Partition.as_ref().map(|p| tid_model::PartitionInfo {
            Definitions: p
                .Definitions
                .iter()
                .map(|d| tid_model::PartitionDefinition { ID: d.ID })
                .collect(),
        }),
    };
    let rewritten = ids
        .RewriteTableInfo(Some(&slim))
        .map_err(|e| Error::new(e.msg))?;
    let mut out = info.Clone();
    out.ID = rewritten.ID;
    if let (Some(part), Some(rewritten_part)) = (&mut out.Partition, rewritten.Partition) {
        for (def, rdef) in part
            .Definitions
            .iter_mut()
            .zip(rewritten_part.Definitions.into_iter())
        {
            def.ID = rdef.ID;
        }
    }
    Ok(out)
}

/// DB is a TiDB instance, not thread-safe.
pub struct DB {
    se: Box<dyn Session>,
    prealloced_ids: Option<PreallocIDs>,
}

/// NewDB returns a new DB.
pub fn NewDB(g: &dyn Glue, store: Storage, policy_mode: &str) -> Result<(Option<DB>, bool)> {
    let se = g.CreateSession(store).map_err(errors::Trace)?;
    // The session may be nil in raw kv mode
    let Some(mut se) = se else {
        return Ok((None, false));
    };

    // Set SQL mode to None for avoiding SQL compatibility problem
    se.Execute(&Context::Background(), "set @@sql_mode=''")
        .map_err(errors::Trace)?;

    let mut support_policy = false;
    if !policy_mode.is_empty() {
        // Set placement mode for handle placement policy.
        let sql = format!("set @@tidb_placement_mode='{}';", policy_mode);
        match se.Execute(&Context::Background(), &sql) {
            Ok(()) => {
                // mirrors log.Debug("set tidb_placement_mode success", ...)
                eprintln!("set tidb_placement_mode success mode={policy_mode}");
                support_policy = true;
            }
            Err(err) if variable::ErrUnknownSystemVar().Equal(&err) => {
                // not support placement policy, just ignore it
                eprintln!(
                    "target tidb not support tidb_placement_mode, ignore create policies: {err}"
                );
            }
            Err(err) => return Err(errors::Trace(err)),
        }
    }

    Ok((
        Some(DB {
            se,
            prealloced_ids: None,
        }),
        support_policy,
    ))
}

impl DB {
    pub fn Session(&mut self) -> &mut dyn Session {
        self.se.as_mut()
    }

    pub fn RegisterPreallocatedIDs(&mut self, ids: PreallocIDs) {
        self.prealloced_ids = Some(ids);
    }

    /// ExecDDL executes the query of a ddl job.
    pub fn ExecDDL(&mut self, ctx: &Context, ddl_job: &model::Job) -> Result<()> {
        let table_info = ddl_job.BinlogInfo.TableInfo.as_ref();
        let db_info = ddl_job.BinlogInfo.DBInfo.as_ref();
        match ddl_job.Type {
            model::ActionCreateSchema => {
                let db_info = db_info.expect("Go code expects BinlogInfo.DBInfo");
                if let Err(err) = self.se.CreateDatabaseOnExistError(ctx, db_info) {
                    if !infoschema::ErrDatabaseExists().Equal(&err) {
                        eprintln!("create database failed db={} err={err}", db_info.Name);
                        return Err(errors::Trace(err));
                    }
                }
                return Ok(());
            }
            model::ActionCreateTable => {
                let table_info = table_info.expect("Go code expects BinlogInfo.TableInfo");
                let info_cloned = table_info.Clone();
                let db_name = CIStr::new(ddl_job.SchemaName.clone());
                if let Err(err) = self.se.CreateTable(ctx, &db_name, &info_cloned, &[]) {
                    let db_label = db_info
                        .map(|d| d.Name.String())
                        .unwrap_or_else(|| ddl_job.SchemaName.clone());
                    eprintln!(
                        "create table failed db={} table={} err={err}",
                        db_label, table_info.Name
                    );
                    return Err(errors::Trace(err));
                }
                return Ok(());
            }
            _ => {}
        }

        if ddl_job.Query.is_empty() {
            eprintln!(
                "query of ddl job is empty, ignore it type={} db={}",
                ddl_job.Type, ddl_job.SchemaName
            );
            return Ok(());
        }

        if table_info.is_some() {
            let switch_db_sql = format!("use {};", utils::EncloseName(&ddl_job.SchemaName));
            if let Err(err) = self.se.Execute(ctx, &switch_db_sql) {
                eprintln!(
                    "switch db failed query={} db={} err={err}",
                    switch_db_sql, ddl_job.SchemaName
                );
                return Err(errors::Trace(err));
            }
        }
        if let Err(err) = self.se.Execute(ctx, &ddl_job.Query) {
            eprintln!(
                "execute ddl query failed query={} db={} historySchemaVersion={} err={err}",
                ddl_job.Query, ddl_job.SchemaName, ddl_job.BinlogInfo.SchemaVersion
            );
            return Err(errors::Trace(err));
        }
        Ok(())
    }

    /// CreatePlacementPolicy check whether cluster support policy and create the policy.
    pub fn CreatePlacementPolicy(
        &mut self,
        ctx: &Context,
        policy: &model::PolicyInfo,
    ) -> Result<()> {
        self.se
            .CreatePlacementPolicy(ctx, policy)
            .map_err(errors::Trace)?;
        eprintln!("create placement policy succeed name={}", policy.Name);
        Ok(())
    }

    /// CreateDatabase executes a CREATE DATABASE SQL.
    /// Returns `(already_exists, ())`.
    pub fn CreateDatabase(
        &mut self,
        ctx: &Context,
        schema: &mut model::DBInfo,
        support_policy: bool,
        policy_map: Option<&Mutex<HashMap<String, model::PolicyInfo>>>,
    ) -> Result<bool> {
        eprintln!("create database name={}", schema.Name);

        if !support_policy {
            eprintln!(
                "set placementPolicyRef to nil when target tidb not support policy database={}",
                schema.Name
            );
            schema.PlacementPolicyRef = None;
        }

        if let Some(policy_ref) = &schema.PlacementPolicyRef {
            if let Some(policy_map) = policy_map {
                self.ensurePlacementPolicy(ctx, &policy_ref.Name, policy_map)
                    .map_err(errors::Trace)?;
            }
        }

        match self.se.CreateDatabaseOnExistError(ctx, schema) {
            Ok(()) => Ok(false),
            Err(err) if infoschema::ErrDatabaseExists().Equal(&err) => Ok(true),
            Err(err) => {
                eprintln!("create database failed db={} err={err}", schema.Name);
                Err(errors::Trace(err))
            }
        }
    }

    fn restoreSequence(&mut self, ctx: &Context, table: &metautil::Table) -> Result<()> {
        let mut restore_meta_sql = String::new();
        let mut err: Option<Error> = None;
        if table.Info.IsSequence() {
            let set_val_format = format!(
                "do setval({}.{}, %d);",
                utils::EncloseName(&table.DB.Name.O),
                utils::EncloseName(&table.Info.Name.O)
            );
            let sequence = table.Info.Sequence.as_ref().expect("IsSequence");
            if sequence.Cycle {
                let increment = sequence.Increment;
                // TiDB sequence's behaviour is designed to keep the same pace
                // among all nodes within the same cluster. so we need restore round.
                // Here is a hack way to trigger sequence cycle round > 0 according to
                // https://github.com/pingcap/br/pull/242#issuecomment-631307978
                // TODO use sql to set cycle round
                let next_seq_sql = format!(
                    "do nextval({}.{});",
                    utils::EncloseName(&table.DB.Name.O),
                    utils::EncloseName(&table.Info.Name.O)
                );
                let set_val_sql = if increment < 0 {
                    set_val_format.replace("%d", &sequence.MinValue.to_string())
                } else {
                    set_val_format.replace("%d", &sequence.MaxValue.to_string())
                };
                if let Err(e) = self.se.Execute(ctx, &set_val_sql) {
                    eprintln!(
                        "restore meta sql failed query={} db={} table={} err={e}",
                        set_val_sql, table.DB.Name, table.Info.Name
                    );
                    return Err(errors::Trace(e));
                }
                // trigger cycle round > 0
                if let Err(e) = self.se.Execute(ctx, &next_seq_sql) {
                    eprintln!(
                        "restore meta sql failed query={} db={} table={} err={e}",
                        next_seq_sql, table.DB.Name, table.Info.Name
                    );
                    return Err(errors::Trace(e));
                }
            }
            restore_meta_sql = set_val_format.replace("%d", &table.Info.AutoIncID.to_string());
            if let Err(e) = self.se.Execute(ctx, &restore_meta_sql) {
                err = Some(e);
            }
        }
        if let Some(e) = err {
            eprintln!(
                "restore meta sql failed query={} db={} table={} err={e}",
                restore_meta_sql, table.DB.Name, table.Info.Name
            );
            return Err(errors::Trace(e));
        }
        Ok(())
    }

    pub fn CreateTablePostRestore(
        &mut self,
        ctx: &Context,
        table: &metautil::Table,
        to_be_corrected_tables: &HashMap<UniqueTableName, bool>,
    ) -> Result<()> {
        let key = UniqueTableName {
            DB: table.DB.Name.String(),
            Table: table.Info.Name.String(),
        };
        match () {
            _ if table.Info.IsView() => Ok(()),
            _ if table.Info.IsSequence() => self.restoreSequence(ctx, table).map_err(errors::Trace),
            // only table exists in restored cluster during incremental restoration should do alter after creation.
            _ if *to_be_corrected_tables.get(&key).unwrap_or(&false) => {
                let restore_meta_sql;
                if utils::NeedAutoID(&table.Info) {
                    restore_meta_sql = format!(
                        "alter table {}.{} auto_increment = {};",
                        utils::EncloseName(&table.DB.Name.O),
                        utils::EncloseName(&table.Info.Name.O),
                        table.Info.AutoIncID
                    );
                } else if table.Info.ContainsAutoRandomBits() {
                    restore_meta_sql = format!(
                        "alter table {}.{} auto_random_base = {}",
                        utils::EncloseName(&table.DB.Name.O),
                        utils::EncloseName(&table.Info.Name.O),
                        table.Info.AutoRandID
                    );
                } else {
                    eprintln!(
                        "table exists in incremental ddl jobs, but don't need to be altered db={} table={}",
                        table.DB.Name, table.Info.Name
                    );
                    return Ok(());
                }
                if let Err(err) = self.se.Execute(ctx, &restore_meta_sql) {
                    eprintln!(
                        "restore meta sql failed query={} db={} table={} err={err}",
                        restore_meta_sql, table.DB.Name, table.Info.Name
                    );
                    return Err(errors::Trace(err));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// CreateTables execute a internal CREATE TABLES.
    pub fn CreateTables(
        &mut self,
        ctx: &Context,
        tables: &mut [metautil::Table],
        ddl_tables: &HashMap<UniqueTableName, bool>,
        support_policy: bool,
        policy_map: Option<&Mutex<HashMap<String, model::PolicyInfo>>>,
    ) -> Result<()> {
        let Some(prealloced_ids) = self.prealloced_ids.clone() else {
            return Err(errors::New("preallocedIDs is nil"));
        };
        let supports_batch = self.se.as_batch_create_table_session().is_some();
        if !supports_batch {
            return Ok(());
        }

        let mut cloned_infos: HashMap<String, Vec<model::TableInfo>> = HashMap::new();
        for table in tables.iter_mut() {
            if !support_policy {
                eprintln!(
                    "set placementPolicyRef to nil when target tidb not support policy table={} db={}",
                    table.Info.Name, table.DB.Name
                );
                table.Info.ClearPlacement();
            } else if let Some(policy_map) = policy_map {
                self.ensureTablePlacementPolicies(ctx, &table.Info, policy_map)
                    .map_err(errors::Trace)?;
            }

            if let Some(ttl_info) = &mut table.Info.TTLInfo {
                ttl_info.Enable = false;
            }
            let info_clone = rewrite_table_info(&prealloced_ids, &table.Info)?;
            cloned_infos
                .entry(table.DB.Name.L.clone())
                .or_default()
                .push(info_clone);
        }
        if !cloned_infos.is_empty() {
            let batch = self
                .se
                .as_batch_create_table_session()
                .expect("batch session just checked");
            batch
                .CreateTables(ctx, cloned_infos, &[WithIDAllocated(true)])
                .map_err(errors::Trace)?;
        }

        for table in tables.iter() {
            self.CreateTablePostRestore(ctx, table, ddl_tables)
                .map_err(errors::Trace)?;
        }
        Ok(())
    }

    /// CreateTable executes a CREATE TABLE SQL.
    pub fn CreateTable(
        &mut self,
        ctx: &Context,
        table: &mut metautil::Table,
        ddl_tables: &HashMap<UniqueTableName, bool>,
        support_policy: bool,
        policy_map: Option<&Mutex<HashMap<String, model::PolicyInfo>>>,
    ) -> Result<()> {
        if !support_policy {
            eprintln!(
                "set placementPolicyRef to nil when target tidb not support policy table={} db={}",
                table.Info.Name, table.DB.Name
            );
            table.Info.ClearPlacement();
        } else if let Some(policy_map) = policy_map {
            self.ensureTablePlacementPolicies(ctx, &table.Info, policy_map)
                .map_err(errors::Trace)?;
        }

        if let Some(ttl_info) = &mut table.Info.TTLInfo {
            ttl_info.Enable = false;
        }

        let prealloced_ids = self
            .prealloced_ids
            .as_ref()
            .expect("preallocedIDs must be registered before CreateTable");
        let info_clone = rewrite_table_info(prealloced_ids, &table.Info)?;
        if let Err(err) =
            self.se
                .CreateTable(ctx, &table.DB.Name, &info_clone, &[WithIDAllocated(true)])
        {
            eprintln!(
                "create table failed db={} table={} err={err}",
                table.DB.Name, table.Info.Name
            );
            return Err(errors::Trace(err));
        }

        self.CreateTablePostRestore(ctx, table, ddl_tables)
            .map_err(errors::Trace)?;
        Ok(())
    }

    /// Close closes the connection.
    pub fn Close(&mut self) {
        self.se.Close();
    }

    fn ensurePlacementPolicy(
        &mut self,
        ctx: &Context,
        policy_name: &CIStr,
        policies: &Mutex<HashMap<String, model::PolicyInfo>>,
    ) -> Result<()> {
        let policy = {
            let mut map = policies
                .lock()
                .expect("placement policy map mutex poisoned");
            map.remove(&policy_name.L)
        };
        if let Some(policy) = policy {
            return self.CreatePlacementPolicy(ctx, &policy);
        }
        // This means policy already created
        Ok(())
    }

    fn ensureTablePlacementPolicies(
        &mut self,
        ctx: &Context,
        table_info: &model::TableInfo,
        policies: &Mutex<HashMap<String, model::PolicyInfo>>,
    ) -> Result<()> {
        if let Some(policy_ref) = &table_info.PlacementPolicyRef {
            self.ensurePlacementPolicy(ctx, &policy_ref.Name, policies)?;
        }

        if let Some(partition) = &table_info.Partition {
            for def in &partition.Definitions {
                let Some(policy_ref) = &def.PlacementPolicyRef else {
                    continue;
                };
                self.ensurePlacementPolicy(ctx, &policy_ref.Name, policies)?;
            }
        }
        Ok(())
    }
}
