// Copyright 2026 AsterSQL.

// DXF storage crate：任务/子任务表访问与测试替身。
//
// 本文件是 crate 根：提供 Error/Value/proto/chunk/session 等基础设施，
// 并通过 include! 挂入 converter/history/nodes/subtask_state 等实现模块。
// 内存 SQLExecutor 用于单测回放 SQL 与结果行。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug)]
/// 简化的错误类型，兼容 PingCAP 错误码与 JSON 编解码。
pub struct Error {
    class: i32,
    code: i32,
    message: &'static str,
    rfccode: &'static str,
    nil: bool,
    cause: Option<GoError>,
}

impl Error {
    /// 用静态消息构造错误（无堆分配）。
    pub const fn static_new(message: &'static str) -> Self {
        Self {
            class: 0,
            code: 0,
            message,
            rfccode: "",
            nil: false,
            cause: None,
        }
    }

    /// 将消息泄漏为 'static 后构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            class: 0,
            code: 0,
            message: Box::leak(message.into().into_boxed_str()),
            rfccode: "",
            nil: false,
            cause: None,
        }
    }

    /// 构造带 RFC code 与数字 code 的 PingCAP 风格错误。
    pub fn pingcap(message: impl Into<String>, rfccode: impl Into<String>, code: i32) -> Self {
        Self {
            class: 0,
            code,
            message: Box::leak(message.into().into_boxed_str()),
            rfccode: Box::leak(rfccode.into().into_boxed_str()),
            nil: false,
            cause: None,
        }
    }

    /// 在保留原始 Go 错误分类的同时前置上下文。
    fn annotated(cause: GoError, detail: String) -> Self {
        Self {
            class: 0,
            code: 0,
            message: Box::leak(format!("{detail}: {cause}").into_boxed_str()),
            rfccode: "",
            nil: false,
            cause: Some(cause),
        }
    }

    /// 是否为 Go 语义上的 nil 错误。
    pub fn is_nil(&self) -> bool {
        self.nil
    }

    /// 有 rfccode 或非零 code 时视为 PingCAP 错误。
    pub fn as_pingcap_error(&self) -> Option<&Self> {
        (!self.rfccode.is_empty() || self.code != 0).then_some(self)
    }

    /// 返回 RFC 错误码字符串。
    pub fn RFCCode(&self) -> &str {
        &self.rfccode
    }

    /// 返回数字错误码。
    pub fn Code(&self) -> i32 {
        self.code
    }

    /// 序列化为包含 class/code/message/rfccode 的 JSON。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(&serde_json::json!({
            "class": self.class,
            "code": self.code,
            "message": self.message,
            "rfccode": self.rfccode,
        }))
        .map_err(errors::Trace)
    }

    /// 从 JSON 填充错误字段（message/rfccode 会泄漏为 'static）。
    pub fn UnmarshalJSON(&mut self, bytes: Vec<u8>) -> Result<(), Error> {
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(errors::Trace)?;
        self.class = value
            .get("class")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default() as i32;
        self.code = value
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default() as i32;
        self.message = Box::leak(
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
                .into_boxed_str(),
        );
        self.rfccode = Box::leak(
            value
                .get("rfccode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
                .into_boxed_str(),
        );
        self.cause = None;
        Ok(())
    }
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.class == other.class
            && self.code == other.code
            && self.message == other.message
            && self.rfccode == other.rfccode
            && self.nil == other.nil
    }
}

impl Eq for Error {}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 仅携带静态消息的轻量 Go 风格错误包装。
pub struct GoError(&'static str);

impl GoError {
    /// 构造 GoError。
    pub const fn new(message: &'static str) -> Self {
        Self(message)
    }
}

impl fmt::Display for GoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for GoError {}

impl From<GoError> for Error {
    fn from(value: GoError) -> Self {
        Self {
            cause: Some(value),
            ..Error::static_new(value.0)
        }
    }
}

impl PartialEq<GoError> for Error {
    fn eq(&self, other: &GoError) -> bool {
        self.message == other.0 || self.cause == Some(*other)
    }
}

/// 错误辅助：Trace / Annotatef / 取堆栈消息。
pub mod errors {
    use super::{Error, GoError};
    use std::fmt::Display;

    /// 将任意 Display 错误包装为 Error。
    pub fn Trace(error: impl Display) -> Error {
        Error::new(error.to_string())
    }

    /// 在 GoError 消息后追加细节。
    pub fn Annotatef(error: GoError, detail: String) -> Error {
        Error::annotated(error, detail)
    }

    /// 返回错误的展示字符串（简化堆栈）。
    pub fn GetErrStackMsg(error: &Error) -> String {
        error.to_string()
    }
}

#[cfg(test)]
#[path = "errors_aster_unit_test.rs"]
mod errors_aster_unit_test;

#[cfg(test)]
#[path = "lib_aster_unit_test.rs"]
mod lib_aster_unit_test;

#[derive(Clone, Debug, PartialEq, Serialize)]
/// SQL 绑定参数 / 单元格值的枚举表示。
pub enum Value {
    Null,
    Int(i64),
    U64(u64),
    String(String),
    Bytes(Vec<u8>),
    Time(SystemTime),
    Json(String),
    Decimal(i64),
}

/// 行单元格别名，与 Value 相同。
pub type Cell = Value;

impl Default for Value {
    fn default() -> Self {
        Self::Null
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(value as i64)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}
impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}
impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::U64(value as u64)
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

/// DXF 任务/子任务/节点等协议层类型与常量（对齐 Go proto 包）。
pub mod proto {
    use super::*;

    /// 任务状态字符串别名（驻留静态串）。
    pub type TaskState = &'static str;
    /// 子任务状态字符串别名。
    pub type SubtaskState = &'static str;
    /// 任务类型字符串别名。
    pub type TaskType = &'static str;
    /// 任务步骤编号。
    pub type Step = i64;

    pub const TaskTypeExample: TaskType = "Example";
    pub const ImportInto: TaskType = "ImportInto";
    pub const Backfill: TaskType = "backfill";

    pub const TaskStatePending: TaskState = "pending";
    pub const TaskStateRunning: TaskState = "running";
    pub const TaskStateSucceed: TaskState = "succeed";
    pub const TaskStateFailed: TaskState = "failed";
    pub const TaskStateReverting: TaskState = "reverting";
    pub const TaskStateAwaitingResolution: TaskState = "awaiting-resolution";
    pub const TaskStateReverted: TaskState = "reverted";
    pub const TaskStateCancelling: TaskState = "cancelling";
    pub const TaskStatePausing: TaskState = "pausing";
    pub const TaskStatePaused: TaskState = "paused";
    pub const TaskStateResuming: TaskState = "resuming";
    pub const TaskStateModifying: TaskState = "modifying";

    pub const SubtaskStatePending: &str = "pending";
    pub const SubtaskStateRunning: &str = "running";
    pub const SubtaskStateSucceed: &str = "succeed";
    pub const SubtaskStateFailed: &str = "failed";
    pub const SubtaskStateCanceled: &str = "canceled";
    pub const SubtaskStatePaused: &str = "paused";

    pub const StepInit: Step = -1;
    pub const StepDone: Step = -2;
    pub const StepPrepared: Step = -3;
    pub const StepOne: Step = 1;
    pub const NormalPriority: i32 = 512;
    pub const ModifyRequiredSlots: &str = "modify_concurrency";

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 任务额外参数：手动恢复、运行时 slot 上限、目标步骤。
    pub struct ExtraParams {
        #[serde(
            default,
            rename = "manual_recovery",
            skip_serializing_if = "std::ops::Not::not"
        )]
        pub ManualRecovery: bool,
        #[serde(
            default,
            rename = "pause_on_kv_disk_full",
            skip_serializing_if = "std::ops::Not::not"
        )]
        pub PauseOnKVDiskFull: bool,
        #[serde(
            default,
            rename = "max_runtime_slots",
            skip_serializing_if = "is_zero_i32"
        )]
        pub MaxRuntimeSlots: i32,
        #[serde(
            default,
            rename = "target_steps",
            skip_serializing_if = "Vec::is_empty"
        )]
        pub TargetSteps: Vec<Step>,
        #[serde(default, rename = "prepare_mode", skip_serializing_if = "is_zero_i32")]
        pub PrepareMode: i32,
    }

    fn is_zero_i32(value: &i32) -> bool {
        *value == 0
    }

    #[derive(Clone, Debug, Default, Serialize)]
    /// 单次修改项：类型（如 modify_concurrency）与目标值。
    pub struct Modification {
        #[serde(rename = "type")]
        pub Type: &'static str,
        #[serde(rename = "to")]
        pub To: i64,
    }

    #[derive(Clone, Debug, Default, Serialize)]
    /// 任务修改参数：修改前状态 + 修改列表。
    pub struct ModifyParam {
        #[serde(rename = "prev_state")]
        pub PrevState: TaskState,
        #[serde(rename = "modifications")]
        pub Modifications: Vec<Modification>,
    }

    /// TaskState 扩展方法命名空间。
    pub struct TaskStateExt;
    impl TaskStateExt {
        /// pending/running/paused 才允许进入 modifying。
        pub fn CanMoveToModifying(state: &TaskState) -> bool {
            matches!(
                *state,
                TaskStatePending | TaskStateRunning | TaskStatePaused
            )
        }
    }

    #[derive(Clone, Debug)]
    /// 任务基础字段（对应 global_task 表核心列）。
    pub struct TaskBase {
        pub ID: i64,
        pub Key: String,
        pub Type: TaskType,
        pub State: TaskState,
        pub Step: Step,
        pub Priority: i32,
        pub RequiredSlots: i32,
        pub TargetScope: String,
        pub CreateTime: SystemTime,
        pub MaxNodeCount: i32,
        pub ExtraParams: ExtraParams,
        pub Keyspace: String,
    }

    impl Default for TaskBase {
        fn default() -> Self {
            Self {
                ID: 0,
                Key: String::new(),
                Type: "",
                State: "",
                Step: StepInit,
                Priority: NormalPriority,
                RequiredSlots: 0,
                TargetScope: String::new(),
                CreateTime: SystemTime::UNIX_EPOCH,
                MaxNodeCount: 0,
                ExtraParams: ExtraParams::default(),
                Keyspace: String::new(),
            }
        }
    }

    #[derive(Clone, Debug)]
    /// 完整任务：TaskBase + 调度器/时间/meta/error/modify。
    pub struct Task {
        pub TaskBase: TaskBase,
        pub SchedulerID: String,
        pub StartTime: SystemTime,
        pub StateUpdateTime: SystemTime,
        pub Meta: Vec<u8>,
        pub Error: Option<String>,
        pub ModifyParam: ModifyParam,
    }

    impl Default for Task {
        fn default() -> Self {
            Self {
                TaskBase: TaskBase::default(),
                SchedulerID: String::new(),
                StartTime: SystemTime::UNIX_EPOCH,
                StateUpdateTime: SystemTime::UNIX_EPOCH,
                Meta: Vec::new(),
                Error: None,
                ModifyParam: ModifyParam::default(),
            }
        }
    }

    impl Deref for Task {
        type Target = TaskBase;
        fn deref(&self) -> &Self::Target {
            &self.TaskBase
        }
    }
    impl DerefMut for Task {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.TaskBase
        }
    }

    #[derive(Clone, Debug)]
    /// 子任务基础字段。
    pub struct SubtaskBase {
        pub ID: i64,
        pub Step: Step,
        pub Type: TaskType,
        pub TaskID: i64,
        pub State: SubtaskState,
        pub Concurrency: i32,
        pub ExecID: String,
        pub CreateTime: SystemTime,
        pub StartTime: SystemTime,
        pub Ordinal: i32,
    }

    impl Default for SubtaskBase {
        fn default() -> Self {
            Self {
                ID: 0,
                Step: StepInit,
                Type: "",
                TaskID: 0,
                State: "",
                Concurrency: 0,
                ExecID: String::new(),
                CreateTime: SystemTime::UNIX_EPOCH,
                StartTime: SystemTime::UNIX_EPOCH,
                Ordinal: 0,
            }
        }
    }

    #[derive(Clone, Debug)]
    /// 完整子任务：基座 + 更新时间/meta/summary。
    pub struct Subtask {
        pub SubtaskBase: SubtaskBase,
        pub UpdateTime: SystemTime,
        pub Meta: Vec<u8>,
        pub Summary: String,
    }

    impl Default for Subtask {
        fn default() -> Self {
            Self {
                SubtaskBase: SubtaskBase::default(),
                UpdateTime: SystemTime::UNIX_EPOCH,
                Meta: Vec::new(),
                Summary: String::new(),
            }
        }
    }

    impl Subtask {
        /// 测试用快速构造 Subtask。
        pub fn for_test(id: i64, meta: Vec<u8>) -> Self {
            Self {
                SubtaskBase: SubtaskBase {
                    ID: id,
                    ..SubtaskBase::default()
                },
                Meta: meta,
                ..Self::default()
            }
        }
    }

    impl Deref for Subtask {
        type Target = SubtaskBase;
        fn deref(&self) -> &Self::Target {
            &self.SubtaskBase
        }
    }
    impl DerefMut for Subtask {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.SubtaskBase
        }
    }

    #[derive(Clone, Debug, Default)]
    /// dist_framework_meta 中的托管节点视图。
    pub struct ManagedNode {
        pub ID: String,
        pub Role: String,
        pub CPUCount: i32,
    }

    #[derive(Clone, Copy, Debug, Default)]
    /// 本机 CPU/内存/磁盘资源快照。
    pub struct NodeResource {
        pub TotalCPU: i32,
        pub TotalMem: i64,
        pub TotalDisk: u64,
    }

    /// 构造 NodeResource。
    pub fn NewNodeResource(cpu: i32, memory: i64, disk: u64) -> NodeResource {
        NodeResource {
            TotalCPU: cpu,
            TotalMem: memory,
            TotalDisk: disk,
        }
    }

    /// 任务类型字符串 → 整型编码。
    pub fn Type2Int(task_type: TaskType) -> i32 {
        match task_type {
            TaskTypeExample => 1,
            ImportInto => 2,
            Backfill => 3,
            _ => 0,
        }
    }

    /// 整型编码 → 任务类型字符串。
    pub fn Int2Type(task_type: i32) -> TaskType {
        match task_type {
            1 => TaskTypeExample,
            2 => ImportInto,
            3 => Backfill,
            _ => "",
        }
    }

    /// 返回最大并发任务数上限。
    pub fn GetMaxConcurrentTask() -> i32 {
        16
    }
}

/// 执行侧摘要类型（如子任务 row_count）。
pub mod execute {
    use super::*;
    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 子任务执行摘要。
    pub struct SubtaskSummary {
        #[serde(default, rename = "row_count")]
        pub RowCount: i64,
    }
}

/// 结果行抽象：按列下标读取各类型单元格。
pub mod chunk {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq)]
    /// 一行结果，内部为 Value 向量。
    pub struct Row(pub Vec<Value>);

    impl Row {
        /// 由单元格列表构造行。
        pub fn new(cells: Vec<Value>) -> Self {
            Self(cells)
        }
        /// 按列读取字符串（兼容 Json/Bytes/数值）。
        pub fn GetString(&self, index: usize) -> String {
            match self.0.get(index) {
                Some(Value::String(value)) | Some(Value::Json(value)) => value.clone(),
                Some(Value::Bytes(value)) => String::from_utf8_lossy(value).into_owned(),
                Some(Value::Int(value)) | Some(Value::Decimal(value)) => value.to_string(),
                Some(Value::U64(value)) => value.to_string(),
                _ => String::new(),
            }
        }
        /// 按列读取 i64。
        pub fn GetInt64(&self, index: usize) -> i64 {
            match self.0.get(index) {
                Some(Value::Int(value)) | Some(Value::Decimal(value)) => *value,
                Some(Value::U64(value)) => *value as i64,
                Some(Value::String(value)) => value.parse().unwrap_or_default(),
                _ => 0,
            }
        }
        /// 按列读取 u64。
        pub fn GetUint64(&self, index: usize) -> u64 {
            self.GetInt64(index) as u64
        }
        /// 按列读取字节串。
        pub fn GetBytes(&self, index: usize) -> Vec<u8> {
            match self.0.get(index) {
                Some(Value::Bytes(value)) => value.clone(),
                Some(Value::String(value)) | Some(Value::Json(value)) => value.as_bytes().to_vec(),
                _ => Vec::new(),
            }
        }
        /// 列是否为 Null 或越界。
        pub fn IsNull(&self, index: usize) -> bool {
            matches!(self.0.get(index), None | Some(Value::Null))
        }
        /// 按列读取时间。
        pub fn GetTime(&self, index: usize) -> MysqlTime {
            match self.0.get(index) {
                Some(Value::Time(value)) => MysqlTime(*value),
                _ => MysqlTime(SystemTime::UNIX_EPOCH),
            }
        }
        /// 按列读取 JSON（底层仍为字符串）。
        pub fn GetJSON(&self, index: usize) -> JSONValue {
            JSONValue(self.GetString(index))
        }
        /// 按列读取 Decimal（测试中用 i64 近似）。
        pub fn GetMyDecimal(&self, index: usize) -> Decimal {
            Decimal(self.GetInt64(index))
        }
    }

    #[derive(Clone, Copy)]
    /// MySQL 时间包装，GoTime 返回底层 SystemTime。
    pub struct MysqlTime(pub SystemTime);
    impl MysqlTime {
        /// 转为 Go time.Time 语义的 SystemTime。
        pub fn GoTime(&self, _local: crate::time::LocalType) -> (SystemTime, Option<crate::Error>) {
            (self.0, None)
        }
    }

    /// JSON 单元格包装。
    pub struct JSONValue(String);
    impl JSONValue {
        /// 取出 JSON 文本。
        pub fn String(self) -> String {
            self.0
        }
    }

    /// Decimal 近似类型。
    pub struct Decimal(i64);
    impl Decimal {
        /// 转为整数。
        pub fn ToInt(&self) -> (i64, Option<crate::Error>) {
            (self.0, None)
        }
    }
}

/// 时间工具：Local 与 Unix 秒构造。
pub mod time {
    use super::SystemTime;
    pub type Time = SystemTime;
    #[derive(Clone, Copy)]
    pub struct LocalType;
    pub const Local: LocalType = LocalType;
    /// 由 Unix 秒+纳秒构造 Time（对齐 Go time.Unix）。
    pub fn Unix(seconds: i64, nanos: u32) -> Time {
        if seconds >= 0 {
            SystemTime::UNIX_EPOCH + std::time::Duration::new(seconds as u64, nanos)
        } else {
            SystemTime::UNIX_EPOCH - std::time::Duration::new((-seconds) as u64, nanos)
        }
    }
}

/// 上下文占位类型（测试中为空元组）。
pub type Context = ();

/// 提供 background 上下文的 trait。
pub trait BackgroundContext {
    fn background() -> Self;
}
impl BackgroundContext for Context {
    fn background() -> Self {}
}

#[derive(Default)]
/// SQL 后端与语句上下文；未挂接后端时保留单测结果队列。
struct ExecutorState {
    backend: Option<Arc<dyn SQLBackend>>,
    affected_rows: Option<Arc<AtomicU64>>,
    rows: VecDeque<Result<Vec<chunk::Row>, Error>>,
    statements: Vec<(String, Vec<Value>)>,
    task_state: proto::TaskState,
}

/// A real session backend returns both rows and the statement's affected-row
/// count. Task state transitions rely on that count for ownership/CAS checks.
pub struct SQLResult {
    pub rows: Vec<chunk::Row>,
    pub affected_rows: u64,
}
pub trait SQLBackend: Send + Sync {
    fn execute(&self, sql: &str, args: Vec<Value>) -> Result<SQLResult, Error>;
    /// 返回当前已开启事务的 start_ts，供统计版本号使用。
    fn txn_start_ts(&self) -> Result<u64, Error> {
        Err(Error::new(
            "SQL backend cannot read transaction start timestamp",
        ))
    }
    /// 同一事务中的 DDL 表模式切换；真实会话后端应调用 DDL executor。
    fn alter_table_mode_for_import(&self, _database_id: i64, _table_id: i64) -> Result<(), Error> {
        Err(Error::new(
            "SQL backend cannot alter table mode for IMPORT INTO",
        ))
    }
    fn alter_table_mode_for_normal(&self, _database_id: i64, _table_id: i64) -> Result<(), Error> {
        Err(Error::new(
            "SQL backend cannot reset table mode after IMPORT INTO",
        ))
    }
}

#[derive(Clone, Default)]
/// SQL 执行器，可绑定真实会话后端或使用默认的测试结果队列。
pub struct SQLExecutor(Arc<Mutex<ExecutorState>>);

impl SQLExecutor {
    fn txn_start_ts(&self) -> Result<u64, Error> {
        let backend = self
            .0
            .lock()
            .unwrap()
            .backend
            .clone()
            .ok_or_else(|| Error::new("SQL backend cannot read transaction start timestamp"))?;
        backend.txn_start_ts()
    }
    fn alter_table_mode_for_import(&self, database_id: i64, table_id: i64) -> Result<(), Error> {
        let backend = self
            .0
            .lock()
            .unwrap()
            .backend
            .clone()
            .ok_or_else(|| Error::new("SQL backend cannot alter table mode for IMPORT INTO"))?;
        backend.alter_table_mode_for_import(database_id, table_id)
    }
    fn alter_table_mode_for_normal(&self, database_id: i64, table_id: i64) -> Result<(), Error> {
        let backend =
            self.0.lock().unwrap().backend.clone().ok_or_else(|| {
                Error::new("SQL backend cannot reset table mode after IMPORT INTO")
            })?;
        backend.alter_table_mode_for_normal(database_id, table_id)
    }
    fn with_backend(backend: Arc<dyn SQLBackend>, affected_rows: Arc<AtomicU64>) -> Self {
        Self(Arc::new(Mutex::new(ExecutorState {
            backend: Some(backend),
            affected_rows: Some(affected_rows),
            ..Default::default()
        })))
    }
    fn has_backend(&self) -> bool {
        self.0.lock().unwrap().backend.is_some()
    }

    /// 记录 SQL；对 begin 与按 id 查 task 的特殊路径返回注入状态。
    pub fn execute(&self, sql: String, args: Vec<Value>) -> Result<Vec<chunk::Row>, Error> {
        let mut state = self.0.lock().unwrap();
        if let Some(backend) = state.backend.clone() {
            let affected_rows = state
                .affected_rows
                .clone()
                .expect("backend statement context");
            drop(state);
            affected_rows.store(0, Ordering::SeqCst);
            let result = backend.execute(&sql, args)?;
            affected_rows.store(result.affected_rows, Ordering::SeqCst);
            return Ok(result.rows);
        }
        if sql == "begin" {
            return Ok(Vec::new());
        }
        if sql.contains("from mysql.tidb_global_task t where id = %?")
            && state.rows.is_empty()
            && !state.task_state.is_empty()
        {
            let task_state = state.task_state;
            return Ok(vec![chunk::Row::new(vec![
                Value::Int(1),
                Value::String(String::new()),
                Value::String(String::new()),
                Value::String(task_state.to_owned()),
                Value::Int(0),
                Value::Int(0),
                Value::Int(0),
                Value::Time(SystemTime::UNIX_EPOCH),
                Value::String(String::new()),
                Value::Int(0),
                Value::Json("{}".into()),
                Value::String(String::new()),
            ])]);
        }
        state.statements.push((sql, args));
        state.rows.pop_front().unwrap_or_else(|| Ok(Vec::new()))
    }
}

/// ExecSQL 封装，转发到 SQLExecutor::execute。
pub mod sqlexec {
    use super::*;
    pub type SQLExecutor = super::SQLExecutor;

    /// 执行 SQL 并返回结果行。
    pub fn ExecSQL(
        _ctx: Context,
        executor: SQLExecutor,
        sql: impl Into<String>,
        args: Vec<Value>,
    ) -> Result<Vec<chunk::Row>, Error> {
        executor.execute(sql.into(), args)
    }
}

#[derive(Clone, Default)]
/// 语句上下文：保存 AffectedRows。
pub struct StatementContext(Arc<AtomicU64>);
impl StatementContext {
    /// 返回最近语句影响行数。
    pub fn AffectedRows(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
/// Session 变量：内存配额与 StmtCtx。
pub struct SessionVars {
    pub MemQuotaQuery: i64,
    pub StmtCtx: StatementContext,
}

impl Default for SessionVars {
    fn default() -> Self {
        Self {
            MemQuotaQuery: vardef::DefTiDBMemQuotaQuery,
            StmtCtx: StatementContext::default(),
        }
    }
}

impl SessionVars {
    /// 克隆 StmtCtx。
    pub fn StmtCtx(&self) -> StatementContext {
        self.StmtCtx.clone()
    }

    /// 设置系统变量（测试空实现）。
    pub fn SetSystemVar(&self, _name: &str, _value: String) -> Result<(), Error> {
        Ok(())
    }
}

/// Session 上下文：绑定 SQLExecutor、变量与事务 entry size 限制。
pub mod sessionctx {
    use super::*;

    #[derive(Clone)]
    /// 单个假 Session。
    pub struct Context {
        pub(crate) executor: SQLExecutor,
        pub(crate) vars: SessionVars,
        txn_entry_size_limit: Arc<AtomicU64>,
    }

    impl Default for Context {
        fn default() -> Self {
            let affected_rows = Arc::new(AtomicU64::new(0));
            Self {
                executor: SQLExecutor::default(),
                vars: SessionVars {
                    MemQuotaQuery: vardef::DefTiDBMemQuotaQuery,
                    StmtCtx: StatementContext(affected_rows),
                },
                txn_entry_size_limit: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    impl Context {
        /// 当前事务的 start_ts；生产后端从 KV 事务读取。
        pub fn TxnStartTS(&self) -> Result<u64, Error> {
            self.executor.txn_start_ts()
        }
        /// Bind a concrete SQL session while retaining per-session statement state.
        pub fn with_backend(backend: Arc<dyn SQLBackend>) -> Self {
            let mut context = Self::default();
            context.executor = SQLExecutor::with_backend(backend, context.vars.StmtCtx.0.clone());
            context
        }
        /// 取得绑定的 SQL 执行器。
        pub fn GetSQLExecutor(&self) -> SQLExecutor {
            self.executor.clone()
        }
        /// 在当前事务所绑定的真实会话后端上切换表模式。
        pub fn AlterTableModeForImport(
            &self,
            database_id: i64,
            table_id: i64,
        ) -> Result<(), Error> {
            self.executor
                .alter_table_mode_for_import(database_id, table_id)
        }
        pub fn AlterTableModeForNormal(
            &self,
            database_id: i64,
            table_id: i64,
        ) -> Result<(), Error> {
            self.executor
                .alter_table_mode_for_normal(database_id, table_id)
        }
        /// 取得 Session 变量。
        pub fn GetSessionVars(&self) -> SessionVars {
            self.vars.clone()
        }
        /// 读取事务条目大小限制。
        pub fn TxnEntrySizeLimit(&self) -> u64 {
            self.txn_entry_size_limit.load(Ordering::SeqCst)
        }
        /// 设置事务条目大小限制。
        pub fn SetTxnEntrySizeLimit(&self, value: u64) {
            self.txn_entry_size_limit.store(value, Ordering::SeqCst);
        }
        /// 提交当前后端事务；默认测试执行器无需提交。
        pub fn CommitTxn(&self, _ctx: super::Context) -> Result<(), Error> {
            if self.executor.has_backend() {
                self.executor.execute("commit".into(), Vec::new())?;
            }
            Ok(())
        }
        /// 回滚当前后端事务；默认测试执行器无需回滚。
        pub fn RollbackTxn(&self, _ctx: super::Context) {
            if self.executor.has_backend() {
                let _ = self.executor.execute("rollback".into(), Vec::new());
            }
        }
    }
}

/// Session 池工具，供 TaskManager 借用独立会话或测试会话。
pub mod util {
    use super::*;

    #[derive(Clone, Default)]
    /// 由工厂创建独立会话；默认模式复用测试会话。
    pub struct SessionPool {
        pub(crate) session: sessionctx::Context,
        factory: Option<Arc<dyn Fn() -> Result<sessionctx::Context, Error> + Send + Sync>>,
    }

    impl SessionPool {
        /// 用给定 session 构造池。
        pub fn new(session: sessionctx::Context) -> Self {
            Self {
                session,
                factory: None,
            }
        }
        /// Each lease owns an independent concrete session/transaction.
        pub fn with_factory(
            factory: impl Fn() -> Result<sessionctx::Context, Error> + Send + Sync + 'static,
        ) -> Self {
            Self {
                session: sessionctx::Context::default(),
                factory: Some(Arc::new(factory)),
            }
        }
        /// 取出池中 session 包装值。
        pub fn Get(&self) -> Result<PoolValue, Error> {
            Ok(PoolValue(match &self.factory {
                Some(factory) => factory()?,
                None => self.session.clone(),
            }))
        }
        /// 释放租约；最后一个会话引用释放时关闭其后端。
        pub fn Put(&self, _value: PoolValue) {}
    }

    #[derive(Clone)]
    /// 池元素，可 downcast 回 sessionctx::Context。
    pub struct PoolValue(sessionctx::Context);
    impl PoolValue {
        /// 返回内部 session 克隆。
        pub fn downcast<T>(&self) -> sessionctx::Context {
            self.0.clone()
        }
    }
}

/// 模拟 Go atomic.Pointer 的 OnceLock+RwLock 包装。
pub struct AtomicGoPointer<T> {
    value: OnceLock<RwLock<Option<T>>>,
}

impl<T> AtomicGoPointer<T> {
    pub const fn new() -> Self {
        Self {
            value: OnceLock::new(),
        }
    }
    /// 存储指针值。
    pub fn Store(&self, value: T) {
        *self
            .value
            .get_or_init(|| RwLock::new(None))
            .write()
            .unwrap() = Some(value);
    }
}

impl<T: Clone> AtomicGoPointer<T> {
    /// 加载指针值副本。
    pub fn Load(&self) -> Option<T> {
        self.value
            .get_or_init(|| RwLock::new(None))
            .read()
            .unwrap()
            .clone()
    }
}

/// Go channel 占位类型（测试中 recv 为空操作）。
pub struct GoChannel<T>(PhantomData<T>);
impl<T> GoChannel<T> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
    /// 阻塞接收占位（空实现）。
    pub fn recv(&self) {}
}

/// 容量单位常量（MiB/GiB）。
pub mod units {
    pub const MiB: usize = 1024 * 1024;
    pub const GiB: i64 = 1024 * 1024 * 1024;
}

/// KV 相关常量与事务总大小限制。
pub mod kv {
    use super::*;
    pub const InternalDistTask: &str = "DistTask";
    pub struct TxnTotalSizeLimit;
    static LIMIT: AtomicU64 = AtomicU64::new(100 * 1024 * 1024);
    impl TxnTotalSizeLimit {
        /// 读取事务总大小限制。
        pub fn Load() -> u64 {
            LIMIT.load(Ordering::SeqCst)
        }
        /// 写入事务总大小限制。
        pub fn Store(value: u64) {
            LIMIT.store(value, Ordering::SeqCst);
        }
    }
}

/// 测试辅助：设置事务总大小限制。
pub fn setTxnTotalSizeLimitForTest(size: u64) {
    kv::TxnTotalSizeLimit::Store(size);
}

/// 系统变量默认值与名字（如 tidb_mem_quota_query）。
pub mod vardef {
    use super::*;
    pub const DefTiDBMemQuotaQuery: i64 = 1 << 30;
    pub const TiDBMemQuotaQuery: &str = "tidb_mem_quota_query";
    pub struct TxnEntrySizeLimit;
    impl TxnEntrySizeLimit {
        pub fn Load() -> u64 {
            6 * 1024 * 1024
        }
    }
}

/// DXF 随机错误注入点（默认恒成功）。
pub mod injectfailpoint {
    use super::Error;
    /// 千分之一概率错误注入占位。
    pub fn DXFRandomErrorWithOnePerThousand() -> Result<(), Error> {
        Ok(())
    }
    /// 百分之一概率错误注入占位。
    pub fn DXFRandomErrorWithOnePercent() -> Result<(), Error> {
        Ok(())
    }
}

/// failpoint 注入钩子占位。
pub mod failpoint {
    /// 按名称注入回调（空实现）。
    pub fn Inject<F>(_name: &str, _callback: F) {}
    /// 按名称注入可变参数（空实现）。
    pub fn InjectCall<T>(_name: &str, _argument: T) {}
}

/// 内部 SQL 来源标记工具。
pub mod clitutil {
    use super::Context;
    /// 标记内部请求来源类型（透传 context）。
    pub fn WithInternalSourceType(context: Context, _source: &str) -> Context {
        context
    }
}

/// 内核类型探测（NextGen 等）。
pub mod kerneltype {
    /// 是否为 NextGen 内核（测试默认 false）。
    pub fn IsNextGen() -> bool {
        false
    }
}
/// 全局配置读取占位。
pub mod config {
    /// 返回全局 keyspace 名（测试默认空）。
    pub fn GetGlobalKeyspaceName() -> String {
        String::new()
    }
}
/// keyspace 常量。
pub mod keyspace {
    pub const System: &str = "SYSTEM";
}

/// 重导出 schstatus crate，供节点忙碌列表等使用。
pub mod schstatus {
    pub use schstatus_crate::*;
}

/// SQL 拼接转义辅助。
pub mod sqlescape {
    use super::Error;

    /// 将 SQL 片段追加到输出缓冲。
    pub fn FormatSQL(output: &mut String, sql: &str) -> Result<(), Error> {
        output.push_str(sql);
        Ok(())
    }
}

/// JSON Marshal/Unmarshal 与 RawMessage。
pub mod json {
    use super::*;
    /// 序列化为 JSON 字节。
    pub fn Marshal<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(value).map_err(errors::Trace)
    }
    /// 从 JSON 字节反序列化。
    pub fn Unmarshal<T: for<'de> Deserialize<'de>>(bytes: Vec<u8>) -> Result<T, Error> {
        serde_json::from_slice(&bytes).map_err(errors::Trace)
    }
    #[derive(Clone)]
    /// 原始 JSON 消息包装。
    pub struct RawMessage(pub Vec<u8>);
}

impl From<json::RawMessage> for Value {
    fn from(value: json::RawMessage) -> Self {
        Value::Json(String::from_utf8_lossy(&value.0).into_owned())
    }
}

/// 字节与字符串互转（对齐 Go hack 包）。
pub mod hack {
    /// 字节 → 字符串（lossy）。
    pub fn String(bytes: Vec<u8>) -> std::string::String {
        std::string::String::from_utf8_lossy(&bytes).into_owned()
    }
    /// 字符串 → 字节。
    pub fn Slice(value: String) -> Vec<u8> {
        value.into_bytes()
    }
}

/// 简易 tracing Region 占位。
pub mod tracing {
    use super::Context;
    /// 追踪区间句柄。
    pub struct Region;
    /// 开始命名 Region。
    pub fn StartRegion(_ctx: Context, _name: &str) -> Region {
        Region
    }
    impl Region {
        /// 结束 Region。
        pub fn End(self) {}
    }
}

// 挂入行转换、任务表、历史、节点、子任务/任务状态实现。
include!("converter.rs");

#[cfg(test)]
#[path = "converter_test.rs"]
mod converter_test;
include!("task_table.rs");
include!("history.rs");
include!("nodes.rs");
include!("subtask_state.rs");
include!("task_state.rs");

#[cfg(test)]
mod table_test;
#[cfg(test)]
mod task_state_test;
#[cfg(test)]
mod task_table_test;

impl TaskManager {
    /// 测试用 TaskManager：固定 SessionPool。
    pub fn for_test(_cpu_count: i32) -> Self {
        NewTaskManager(util::SessionPool::new(sessionctx::Context::default()))
    }

    /// 默认测试 TaskManager（8 核）。
    pub fn new() -> Self {
        Self::for_test(8)
    }

    /// 注入 StmtCtx.AffectedRows，供 CAS 类测试。
    pub fn set_affected_rows(&self, rows: u64) {
        self.sePool
            .session
            .vars
            .StmtCtx
            .0
            .store(rows, Ordering::SeqCst);
    }

    /// 注入按 id 查询任务时返回的状态。
    pub fn set_task_state(&self, state: proto::TaskState) {
        self.sePool.session.executor.0.lock().unwrap().task_state = state;
    }

    /// 向执行器结果队列追加一批行。
    pub fn push_result(&self, rows: Vec<chunk::Row>) {
        self.sePool
            .session
            .executor
            .0
            .lock()
            .unwrap()
            .rows
            .push_back(Ok(rows));
    }

    /// 导出已记录的 SQL 调用列表。
    pub fn calls(&self) -> Vec<SqlCall> {
        self.sePool
            .session
            .executor
            .0
            .lock()
            .unwrap()
            .statements
            .iter()
            .map(|(sql, args)| SqlCall {
                sql: sql.clone(),
                args: args.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 单次 SQL 调用记录：语句文本与参数。
pub struct SqlCall {
    /// SQL 文本。
    pub sql: String,
    /// 绑定参数。
    pub args: Vec<Value>,
}

impl SessionExecutor for TaskManager {
    fn WithNewSession<F>(&self, callback: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>,
    {
        TaskManager::WithNewSession(self, callback)
    }

    fn WithNewTxn<F>(&self, context: Context, callback: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>,
    {
        TaskManager::WithNewTxn(self, context, callback)
    }
}

#[cfg(test)]
#[path = "sql_backend_test.rs"]
mod sql_backend_test;
