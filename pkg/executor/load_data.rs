// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// LOAD DATA 执行器：从文件/远程源批量导入行。
//
// 采用「编码线程 + 提交线程」流水线：读入解析 → 列映射/表达式赋值 →
// 按批事务提交。支持服务端/远端与客户端本地文件两种来源，以及
// REPLACE / IGNORE / ERROR 三种主键冲突策略。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::io::{Read as IoRead, Seek as IoSeek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

/// 编码线程向提交线程投递批次任务的同步队列容量。
const TASK_QUEUE_SIZE: usize = 16;
/// 语句级告警条数上限（与 MySQL 兼容的 u16 最大值）。
const MAX_WARNINGS: u64 = u16::MAX as u64;

#[derive(Clone, Debug, PartialEq)]
/// 导入过程中的标量值：空、整数、浮点、字节、文本、时间戳。
pub enum Datum {
    Null,
    Integer(i64),
    Unsigned(u64),
    Float(f64),
    Bytes(Vec<u8>),
    Text(String),
    Timestamp(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// LOAD DATA 路径上的错误分类。
pub enum LoadDataError {
    Backend(String),
    Cancelled,
    EndOfFile,
    ReaderMissing,
    ColumnCount {
        expected: usize,
        actual: usize,
        row: u64,
    },
    InvalidAutoRandom,
    DuplicateKey(String),
    UnsupportedSeek {
        offset: i64,
        whence: String,
    },
    Panic(String),
}

impl Display for LoadDataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for LoadDataError {}

/// 可读入字节流的数据源，关闭时释放底层句柄。
pub trait LoadDataReader: IoRead + Send {
    fn close(&mut self) -> Result<(), LoadDataError>;
}

/// 按行解析器：读一行 Datum，并可回收行缓冲。
pub trait DataParser: Send {
    fn read_row(&mut self) -> Result<Vec<Datum>, LoadDataError>;
    fn recycle_row(&mut self, row: Vec<Datum>);
    fn close(&mut self) -> Result<(), LoadDataError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单个输入片段的路径、起始偏移与可选长度。
pub struct LoadDataReaderInfo {
    pub path: String,
    pub offset: u64,
    pub length: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 文件位置：服务端/远端，或客户端本地流。
pub enum FileLocRef {
    ServerOrRemote,
    Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 主键/唯一键冲突时的处理策略。
pub enum OnDuplicateKeyHandling {
    Replace,
    Ignore,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 插入列元信息：生成列、时间类型、非空、额外句柄等。
pub struct ColumnInfo {
    pub name: String,
    pub generated: bool,
    pub time_type: bool,
    pub not_null: bool,
    pub extra_handle: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 输入字段映射到表列或用户变量。
pub enum FieldMapping {
    Column(ColumnInfo),
    UserVariable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// LOAD DATA 控制器：路径、列映射、冲突策略与批大小等。
pub struct LoadDataController {
    pub path: String,
    pub restrictive: bool,
    pub ignore_lines: usize,
    pub field_count: usize,
    pub field_mappings: Vec<FieldMapping>,
    pub insert_columns: Vec<ColumnInfo>,
    pub assignment_count: usize,
    pub expression_warnings: Vec<String>,
    pub on_duplicate: OnDuplicateKeyHandling,
    pub max_rows_in_batch: usize,
    pub low_priority: bool,
    pub shard_allocate_step: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 语句执行统计：记录数、复制数、删除数、告警与耗时。
pub struct StatementStats {
    pub record_rows: u64,
    pub copied_rows: u64,
    pub deleted_rows: u64,
    pub warnings: Vec<String>,
    pub message: String,
    pub commit_tasks: u64,
    pub runtime_nanos: u128,
    pub non_restrictive: bool,
}

/// 后端边界：打开解析器、事务、写行、批插入与会话开关。
pub trait LoadDataBackend: Send + Sync + 'static {
    fn init_data_files(&self, controller: &LoadDataController) -> Result<(), LoadDataError>;
    fn remote_reader_infos(
        &self,
        controller: &LoadDataController,
    ) -> Result<Vec<LoadDataReaderInfo>, LoadDataError>;
    fn local_reader_info(
        &self,
        path: &str,
        reader: Box<dyn LoadDataReader>,
    ) -> Result<LoadDataReaderInfo, LoadDataError>;
    fn open_parser(
        &self,
        reader: &LoadDataReaderInfo,
    ) -> Result<Box<dyn DataParser>, LoadDataError>;
    fn close_controller(&self, controller: &LoadDataController) -> Result<(), LoadDataError>;
    fn evaluate_assignment(
        &self,
        assignment: usize,
        variables: &BTreeMap<String, Datum>,
    ) -> Result<Datum, LoadDataError>;
    fn normalize_row(&self, row_number: u64, row: Vec<Datum>) -> Result<Vec<Datum>, LoadDataError>;
    fn current_timestamp(&self, column: &ColumnInfo) -> Datum;
    fn begin_transaction(&self) -> Result<(), LoadDataError>;
    fn set_transaction_low_priority(&self) -> Result<(), LoadDataError>;
    fn add_record(
        &self,
        row: &[Datum],
        duplicate_check: bool,
        size_hint: Option<usize>,
    ) -> Result<(), LoadDataError>;
    fn batch_check_and_insert(
        &self,
        rows: &[Vec<Datum>],
        replace: bool,
    ) -> Result<(u64, u64), LoadDataError>;
    fn statement_commit(&self) -> Result<(), LoadDataError>;
    fn commit_transaction(&self) -> Result<(), LoadDataError>;
    fn rollback_transaction(&self) -> Result<(), LoadDataError>;
    fn allow_write_row_id(&self) -> bool;
    fn killed(&self) -> Result<(), LoadDataError>;
}

/// 客户端本地读取器工厂：Build 打开路径，Wait 等待传输完成。
pub struct LoadDataReaderBuilder {
    pub Build: Arc<dyn Fn(&str) -> Result<Box<dyn LoadDataReader>, LoadDataError> + Send + Sync>,
    pub Wait: Arc<dyn Fn() + Send + Sync>,
}

/// LOAD DATA 物理算子入口。
pub struct LoadDataExec<B: LoadDataBackend> {
    pub FileLocRef: FileLocRef,
    pub loadDataWorker: Option<LoadDataWorker<B>>,
    pub infileReader: Option<Box<dyn LoadDataReader>>,
    pub readerBuilder: Option<LoadDataReaderBuilder>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 计划侧信息：列名与生成列表达式占位。
pub struct planInfo {
    pub ID: i32,
    pub Columns: Vec<String>,
    pub GenColExprs: Vec<String>,
}

/// 实际导入工作器：持有控制器、表名与语句统计。
pub struct LoadDataWorker<B: LoadDataBackend> {
    pub backend: Arc<B>,
    pub controller: Arc<LoadDataController>,
    pub planInfo: planInfo,
    pub table: String,
    pub stats: Arc<Mutex<StatementStats>>,
    pub closed: bool,
}

/// 编码侧工作器：解析行、列映射、组装提交批次。
pub struct encodeWorker<B: LoadDataBackend> {
    pub backend: Arc<B>,
    pub controller: Arc<LoadDataController>,
    pub stats: Arc<Mutex<StatementStats>>,
    pub exprWarnings: Vec<String>,
    pub userVariables: BTreeMap<String, Datum>,
    pub rows: Vec<Vec<Datum>>,
    pub rowCount: u64,
    pub curBatchCnt: u64,
    pub maxRowsInBatch: usize,
}

#[derive(Clone, Debug, PartialEq)]
/// 提交任务：行数与行缓冲。
pub struct commitTask {
    pub cnt: u64,
    pub rows: Vec<Vec<Datum>>,
}

/// 提交侧工作器：按批开启事务并写入存储。
pub struct commitWorker<B: LoadDataBackend> {
    pub backend: Arc<B>,
    pub controller: Arc<LoadDataController>,
    pub stats: Arc<Mutex<StatementStats>>,
}

/// 插入值上下文：是否含 _tidb_rowid 额外句柄等。
pub struct InsertValues<B: LoadDataBackend> {
    pub backend: Arc<B>,
    pub controller: Arc<LoadDataController>,
    pub plan: planInfo,
    pub table: String,
    pub has_extra_handle: bool,
    pub stats: Arc<Mutex<StatementStats>>,
}

/// 标记非严格模式（列数/赋值错误转告警而非失败）。
pub fn setNonRestrictiveFlags(stats: &Arc<Mutex<StatementStats>>) {
    stats
        .lock()
        .expect("statement stats mutex poisoned")
        .non_restrictive = true;
}

/// 构造 LoadDataWorker；非严格模式时设置统计标志。
pub fn NewLoadDataWorker<B: LoadDataBackend>(
    backend: Arc<B>,
    controller: LoadDataController,
    plan: planInfo,
    table: String,
) -> Result<LoadDataWorker<B>, LoadDataError> {
    let stats = Arc::new(Mutex::new(StatementStats::default()));
    if !controller.restrictive {
        setNonRestrictiveFlags(&stats);
    }
    Ok(LoadDataWorker {
        backend,
        controller: Arc::new(controller),
        planInfo: plan,
        table,
        stats,
        closed: false,
    })
}

impl<B: LoadDataBackend> LoadDataExec<B> {
    /// 打开算子：若有 readerBuilder 则构建本地 infile 读取器。
    pub fn Open(&mut self) -> Result<(), LoadDataError> {
        if let (Some(builder), Some(worker)) = (&self.readerBuilder, &self.loadDataWorker) {
            self.infileReader = Some((builder.Build)(worker.GetInfilePath())?);
        }
        Ok(())
    }

    /// 关闭工作器与本地读取器，保留首个错误。
    pub fn Close(&mut self) -> Result<(), LoadDataError> {
        let worker_error = self
            .loadDataWorker
            .as_mut()
            .and_then(|worker| worker.Close().err());
        self.closeLocalReader(worker_error)
    }

    /// 关闭本地读取器并 Wait 传输；合并关闭错误。
    pub fn closeLocalReader(
        &mut self,
        original_error: Option<LoadDataError>,
    ) -> Result<(), LoadDataError> {
        let mut result = original_error;
        if let Some(mut reader) = self.infileReader.take() {
            if let Err(close_error) = reader.close() {
                if result.is_none() {
                    result = Some(close_error);
                }
            }
        }
        if let Some(builder) = &self.readerBuilder {
            (builder.Wait)();
        }
        result.map_or(Ok(()), Err)
    }

    /// 按 FileLocRef 分派远端或本地加载。
    pub fn Next(&mut self) -> Result<(), LoadDataError> {
        let worker = self
            .loadDataWorker
            .as_mut()
            .ok_or_else(|| LoadDataError::Backend("load-data worker is missing".into()))?;
        match self.FileLocRef {
            FileLocRef::ServerOrRemote => worker.loadRemote(),
            FileLocRef::Client => {
                let reader = self
                    .infileReader
                    .take()
                    .ok_or(LoadDataError::ReaderMissing)?;
                let result = worker.LoadLocal(reader);
                if let Err(error) = result {
                    return self.closeLocalReader(Some(error));
                }
                Ok(())
            }
        }
    }
}

impl<B: LoadDataBackend> LoadDataWorker<B> {
    /// 初始化远端数据文件并加载全部 reader 信息。
    pub fn loadRemote(&mut self) -> Result<(), LoadDataError> {
        self.backend.init_data_files(&self.controller)?;
        let readers = self.backend.remote_reader_infos(&self.controller)?;
        self.load(readers)
    }

    /// 将客户端读取器登记为本地 reader 后进入流水线。
    pub fn LoadLocal(&mut self, reader: Box<dyn LoadDataReader>) -> Result<(), LoadDataError> {
        let reader_info = self
            .backend
            .local_reader_info(self.GetInfilePath(), reader)?;
        self.load(vec![reader_info])
    }

    /// 启动编码/提交双线程流水线，投递全部 reader 并汇总结果。
    pub fn load(&mut self, reader_infos: Vec<LoadDataReaderInfo>) -> Result<(), LoadDataError> {
        let (encoder, committer) = initEncodeCommitWorkers(self)?;
        let (reader_sender, reader_receiver) = mpsc::sync_channel::<LoadDataReaderInfo>(1);
        let (task_sender, task_receiver) = mpsc::sync_channel::<commitTask>(TASK_QUEUE_SIZE);
        let cancelled = Arc::new(AtomicBool::new(false));
        let first_error = Arc::new(Mutex::new(None::<LoadDataError>));

        // 编码线程消费 reader → 产出 commitTask；提交线程消费任务写库
        thread::scope(|scope| {
            let encode_cancelled = Arc::clone(&cancelled);
            let encode_error = Arc::clone(&first_error);
            let encode_handle = scope.spawn(move || {
                let mut worker = encoder;
                let result = worker.processStream(reader_receiver, task_sender, &encode_cancelled);
                if let Err(error) = &result {
                    encode_cancelled.store(true, Ordering::Release);
                    store_first_error(&encode_error, error.clone());
                }
                (worker.exprWarnings, result)
            });

            let commit_cancelled = Arc::clone(&cancelled);
            let commit_error = Arc::clone(&first_error);
            let commit_handle = scope.spawn(move || {
                let mut worker = committer;
                let result = worker.commitWork(task_receiver, &commit_cancelled);
                if let Err(error) = &result {
                    commit_cancelled.store(true, Ordering::Release);
                    store_first_error(&commit_error, error.clone());
                }
                result
            });

            for reader in reader_infos {
                let mut pending = reader;
                loop {
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    match reader_sender.try_send(pending) {
                        Ok(()) => break,
                        Err(mpsc::TrySendError::Full(reader)) => {
                            pending = reader;
                            thread::yield_now();
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => {
                            cancelled.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
            }
            drop(reader_sender);

            let (warnings, encode_result) = encode_handle.join().map_err(panic_error)?;
            let commit_result = commit_handle.join().map_err(panic_error)?;
            self.setResult(&warnings);
            encode_result?;
            commit_result?;
            first_error
                .lock()
                .expect("load error mutex poisoned")
                .clone()
                .map_or(Ok(()), Err)
        })
    }

    /// 汇总 Records/Deleted/Skipped/Warnings 消息与告警列表。
    pub fn setResult(&mut self, column_assignment_warnings: &[String]) {
        let mut stats = self.stats.lock().expect("statement stats mutex poisoned");
        let records = stats.record_rows;
        let deleted = stats.deleted_rows;
        let skipped = records.saturating_sub(stats.copied_rows);
        let total_warnings = (stats.warnings.len() as u64)
            .saturating_add(records.saturating_mul(column_assignment_warnings.len() as u64))
            .min(MAX_WARNINGS);
        let mut warnings = Vec::with_capacity(total_warnings as usize);
        warnings.extend(stats.warnings.iter().take(total_warnings as usize).cloned());
        while warnings.len() < total_warnings as usize {
            let remaining = total_warnings as usize - warnings.len();
            warnings.extend(column_assignment_warnings.iter().take(remaining).cloned());
            if column_assignment_warnings.is_empty() {
                break;
            }
        }
        stats.message = format!(
            "Records: {records}  Deleted: {deleted}  Skipped: {skipped}  Warnings: {total_warnings}"
        );
        stats.warnings = warnings;
    }

    /// 关闭控制器资源（幂等）。
    pub fn Close(&mut self) -> Result<(), LoadDataError> {
        if self.closed {
            return Ok(());
        }
        self.backend.close_controller(&self.controller)?;
        self.closed = true;
        Ok(())
    }

    /// 返回 INFILE 路径。
    pub fn GetInfilePath(&self) -> &str {
        &self.controller.path
    }

    /// 返回控制器引用。
    pub fn GetController(&self) -> &LoadDataController {
        &self.controller
    }

    /// 测试用单线程本地加载：跳过 ignore_lines 后编码并提交一批。
    pub fn TestLoadLocal(&mut self, mut parser: Box<dyn DataParser>) -> Result<(), LoadDataError> {
        setNonRestrictiveFlags(&self.stats);
        let (mut encoder, mut committer) = initEncodeCommitWorkers(self)?;
        self.backend.begin_transaction()?;
        for _ in 0..self.controller.ignore_lines {
            match parser.read_row() {
                Ok(row) => parser.recycle_row(row),
                Err(LoadDataError::EndOfFile) => break,
                Err(error) => return Err(error),
            }
        }
        encoder.readOneBatchRows(parser.as_mut())?;
        let rows = std::mem::take(&mut encoder.rows);
        committer.checkAndInsertOneBatch(rows, encoder.curBatchCnt)?;
        encoder.resetBatch();
        self.backend.statement_commit()?;
        self.backend.commit_transaction()?;
        self.setResult(&encoder.exprWarnings);
        Ok(())
    }
}

/// 由 InsertValues 派生 encodeWorker 与 commitWorker。
pub fn initEncodeCommitWorkers<B: LoadDataBackend>(
    worker: &LoadDataWorker<B>,
) -> Result<(encodeWorker<B>, commitWorker<B>), LoadDataError> {
    let insert_values = createInsertValues(worker)?;
    let encoder = encodeWorker {
        backend: Arc::clone(&insert_values.backend),
        controller: Arc::clone(&insert_values.controller),
        stats: Arc::clone(&insert_values.stats),
        exprWarnings: insert_values.controller.expression_warnings.clone(),
        userVariables: BTreeMap::new(),
        rows: Vec::new(),
        rowCount: 0,
        curBatchCnt: 0,
        maxRowsInBatch: insert_values.controller.max_rows_in_batch.max(1),
    };
    let committer = commitWorker {
        backend: insert_values.backend,
        controller: insert_values.controller,
        stats: insert_values.stats,
    };
    Ok((encoder, committer))
}

/// 校验 _tidb_rowid 写权限并构造 InsertValues。
pub fn createInsertValues<B: LoadDataBackend>(
    worker: &LoadDataWorker<B>,
) -> Result<InsertValues<B>, LoadDataError> {
    let has_extra_handle = worker
        .controller
        .insert_columns
        .iter()
        .any(|column| column.extra_handle);
    if has_extra_handle && !worker.backend.allow_write_row_id() {
        return Err(LoadDataError::Backend(
            "load data statement for _tidb_rowid is not supported".into(),
        ));
    }
    Ok(InsertValues {
        backend: Arc::clone(&worker.backend),
        controller: Arc::clone(&worker.controller),
        plan: worker.planInfo.clone(),
        table: worker.table.clone(),
        has_extra_handle,
        stats: Arc::clone(&worker.stats),
    })
}

impl<B: LoadDataBackend> encodeWorker<B> {
    /// 循环接收 reader：跳过 ignore_lines 后按流编码批次。
    pub fn processStream(
        &mut self,
        input: mpsc::Receiver<LoadDataReaderInfo>,
        output: mpsc::SyncSender<commitTask>,
        cancelled: &AtomicBool,
    ) -> Result<(), LoadDataError> {
        while !cancelled.load(Ordering::Acquire) {
            let reader = match input.recv() {
                Ok(reader) => reader,
                Err(_) => return Ok(()),
            };
            let mut parser = self.backend.open_parser(&reader)?;
            let result = (|| {
                for _ in 0..self.controller.ignore_lines {
                    match parser.read_row() {
                        Ok(row) => parser.recycle_row(row),
                        Err(LoadDataError::EndOfFile) => return Ok(()),
                        Err(error) => return Err(error),
                    }
                }
                self.processOneStream(parser.as_mut(), &output, cancelled)
            })();
            // Go uses terror.Log for parser cleanup: a close failure must not
            // replace the stream-processing result.
            let _ = parser.close();
            result?;
        }
        Err(LoadDataError::Cancelled)
    }

    /// 从解析器读满一批即投递 commitTask，直到 EOF 或取消。
    pub fn processOneStream(
        &mut self,
        parser: &mut dyn DataParser,
        output: &mpsc::SyncSender<commitTask>,
        cancelled: &AtomicBool,
    ) -> Result<(), LoadDataError> {
        loop {
            self.readOneBatchRows(parser)?;
            if self.curBatchCnt == 0 {
                return Ok(());
            }
            let task = commitTask {
                cnt: self.curBatchCnt,
                rows: std::mem::take(&mut self.rows),
            };
            let mut pending = task;
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err(LoadDataError::Cancelled);
                }
                self.backend.killed()?;
                match output.try_send(pending) {
                    Ok(()) => break,
                    Err(mpsc::TrySendError::Full(task)) => {
                        pending = task;
                        thread::yield_now();
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        return Err(LoadDataError::Cancelled);
                    }
                }
            }
            self.resetBatch();
        }
    }

    /// 清空当前批缓冲。
    pub fn resetBatch(&mut self) {
        self.rows = Vec::with_capacity(self.maxRowsInBatch);
        self.curBatchCnt = 0;
    }

    /// 读入至多 maxRowsInBatch 行并做列映射。
    pub fn readOneBatchRows(&mut self, parser: &mut dyn DataParser) -> Result<(), LoadDataError> {
        loop {
            let parser_row = match parser.read_row() {
                Ok(row) => row,
                Err(LoadDataError::EndOfFile) => return Ok(()),
                Err(error) => {
                    return Err(LoadDataError::Backend(format!(
                        "cannot read LOAD DATA input: {error}"
                    )));
                }
            };
            self.rowCount = self.rowCount.saturating_add(1);
            let converted = self.parserData2TableData(&parser_row)?;
            parser.recycle_row(parser_row);
            self.rows.push(converted);
            self.curBatchCnt = self.curBatchCnt.saturating_add(1);
            if self.curBatchCnt as usize >= self.maxRowsInBatch {
                return Ok(());
            }
        }
    }

    /// 字段映射、生成列/时间默认值、SET 赋值与行规范化。
    pub fn parserData2TableData(
        &mut self,
        parser_data: &[Datum],
    ) -> Result<Vec<Datum>, LoadDataError> {
        if parser_data.len() != self.controller.field_count {
            let error = LoadDataError::ColumnCount {
                expected: self.controller.field_count,
                actual: parser_data.len(),
                row: self.rowCount,
            };
            // 严格模式列数不符直接失败；否则记告警继续
            if self.controller.restrictive {
                return Err(error);
            }
            self.stats
                .lock()
                .expect("statement stats mutex poisoned")
                .warnings
                .push(error.to_string());
        }

        let mut row = Vec::with_capacity(
            self.controller.insert_columns.len() + self.controller.assignment_count,
        );
        for (index, mapping) in self.controller.field_mappings.iter().enumerate() {
            let value = parser_data.get(index).cloned().unwrap_or(Datum::Null);
            match mapping {
                FieldMapping::UserVariable(name) => {
                    let name = name.to_lowercase();
                    if value == Datum::Null {
                        self.userVariables.remove(&name);
                    } else {
                        self.userVariables.insert(name, value);
                    }
                }
                FieldMapping::Column(column) if column.generated => row.push(Datum::Null),
                FieldMapping::Column(column)
                    if index >= parser_data.len() && column.time_type && column.not_null =>
                {
                    row.push(self.backend.current_timestamp(column))
                }
                FieldMapping::Column(_) => row.push(value),
            }
        }

        for assignment in 0..self.controller.assignment_count {
            match self
                .backend
                .evaluate_assignment(assignment, &self.userVariables)
            {
                Ok(value) => row.push(value),
                Err(error) if self.controller.restrictive => return Err(error),
                Err(error) => {
                    self.stats
                        .lock()
                        .expect("statement stats mutex poisoned")
                        .warnings
                        .push(error.to_string());
                    row.push(Datum::Null);
                }
            }
        }

        match self.backend.normalize_row(self.rowCount, row) {
            Ok(row) => Ok(row),
            Err(LoadDataError::InvalidAutoRandom) => Err(LoadDataError::InvalidAutoRandom),
            Err(error) if self.controller.restrictive => Err(error),
            Err(error) => {
                self.stats
                    .lock()
                    .expect("statement stats mutex poisoned")
                    .warnings
                    .push(error.to_string());
                Ok(Vec::new())
            }
        }
    }
}

impl<B: LoadDataBackend> commitWorker<B> {
    /// 提交线程主循环：逐个消费 commitTask。
    pub fn commitWork(
        &mut self,
        input: mpsc::Receiver<commitTask>,
        cancelled: &AtomicBool,
    ) -> Result<(), LoadDataError> {
        while !cancelled.load(Ordering::Acquire) {
            match input.recv() {
                Ok(task) => self.commitOneTask(task)?,
                Err(_) => return Ok(()),
            }
        }
        Err(LoadDataError::Cancelled)
    }

    /// 单任务事务：插入批次 → statement_commit → commit；失败则回滚。
    pub fn commitOneTask(&mut self, task: commitTask) -> Result<(), LoadDataError> {
        self.backend.begin_transaction()?;
        let result = self
            .checkAndInsertOneBatch(task.rows, task.cnt)
            .and_then(|()| self.backend.statement_commit())
            .and_then(|()| self.backend.commit_transaction());
        if let Err(error) = result {
            let _ = self.backend.rollback_transaction();
            return Err(error);
        }
        self.stats
            .lock()
            .expect("statement stats mutex poisoned")
            .commit_tasks += 1;
        Ok(())
    }

    /// 按冲突策略批插或逐行 add_record，并累计统计。
    pub fn checkAndInsertOneBatch(
        &mut self,
        rows: Vec<Vec<Datum>>,
        count: u64,
    ) -> Result<(), LoadDataError> {
        let started = std::time::Instant::now();
        if count == 0 {
            return Ok(());
        }
        let count =
            usize::try_from(count).map_err(|error| LoadDataError::Backend(error.to_string()))?;
        if count > rows.len() {
            return Err(LoadDataError::Backend(
                "commit task count exceeds row buffer".into(),
            ));
        }
        if self.controller.low_priority {
            self.backend.set_transaction_low_priority()?;
        }
        {
            let mut stats = self.stats.lock().expect("statement stats mutex poisoned");
            stats.record_rows = stats.record_rows.saturating_add(count as u64);
        }
        match self.controller.on_duplicate {
            OnDuplicateKeyHandling::Replace => {
                let (copied, deleted) =
                    self.backend.batch_check_and_insert(&rows[..count], true)?;
                let mut stats = self.stats.lock().expect("statement stats mutex poisoned");
                stats.copied_rows = stats.copied_rows.saturating_add(copied);
                stats.deleted_rows = stats.deleted_rows.saturating_add(deleted);
            }
            OnDuplicateKeyHandling::Ignore => {
                let (copied, deleted) =
                    self.backend.batch_check_and_insert(&rows[..count], false)?;
                let mut stats = self.stats.lock().expect("statement stats mutex poisoned");
                stats.copied_rows = stats.copied_rows.saturating_add(copied);
                stats.deleted_rows = stats.deleted_rows.saturating_add(deleted);
            }
            // Error 模式：逐行插入，可按 shard_allocate_step 给 size_hint
            OnDuplicateKeyHandling::Error => {
                for (index, row) in rows[..count].iter().enumerate() {
                    let size_hint = if self.controller.shard_allocate_step > 0
                        && index % self.controller.shard_allocate_step == 0
                    {
                        Some(self.controller.shard_allocate_step.min(count - index))
                    } else {
                        None
                    };
                    self.addRecordLD(row, true, size_hint)?;
                    self.stats
                        .lock()
                        .expect("statement stats mutex poisoned")
                        .copied_rows += 1;
                }
            }
        }
        self.stats
            .lock()
            .expect("statement stats mutex poisoned")
            .runtime_nanos += started.elapsed().as_nanos();
        Ok(())
    }

    /// 空行跳过；否则委托 backend.add_record。
    pub fn addRecordLD(
        &mut self,
        row: &[Datum],
        duplicate_check: bool,
        size_hint: Option<usize>,
    ) -> Result<(), LoadDataError> {
        if row.is_empty() {
            return Ok(());
        }
        self.backend.add_record(row, duplicate_check, size_hint)
    }
}

/// 仅保留流水线中的第一个错误。
fn store_first_error(slot: &Mutex<Option<LoadDataError>>, error: LoadDataError) {
    let mut slot = slot.lock().expect("load error mutex poisoned");
    if slot.is_none() {
        *slot = Some(error);
    }
}

/// 将工作线程 panic 载荷转为 LoadDataError::Panic。
fn panic_error(payload: Box<dyn std::any::Any + Send>) -> LoadDataError {
    if let Some(message) = payload.downcast_ref::<&str>() {
        LoadDataError::Panic((*message).into())
    } else if let Some(message) = payload.downcast_ref::<String>() {
        LoadDataError::Panic(message.clone())
    } else {
        LoadDataError::Panic("unknown worker panic".into())
    }
}

/// 仅支持「当前位置」查询的简易 Seek 包装。
pub struct SimpleSeekerOnReadCloser {
    pub r: Box<dyn LoadDataReader>,
    pub pos: u64,
}

/// 构造位置从 0 开始的 SimpleSeekerOnReadCloser。
pub fn NewSimpleSeekerOnReadCloser(reader: Box<dyn LoadDataReader>) -> SimpleSeekerOnReadCloser {
    SimpleSeekerOnReadCloser { r: reader, pos: 0 }
}

impl SimpleSeekerOnReadCloser {
    /// 读取并推进内部位置计数。
    pub fn Read(&mut self, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
        let count = self.r.read(buffer)?;
        self.pos = self.pos.saturating_add(count as u64);
        Ok(count)
    }

    /// 仅允许 Seek(0, Current) 返回当前位置，其余报 UnsupportedSeek。
    pub fn Seek(&mut self, offset: i64, whence: SeekFrom) -> Result<u64, LoadDataError> {
        if offset == 0 && whence == SeekFrom::Current(0) {
            Ok(self.pos)
        } else {
            Err(LoadDataError::UnsupportedSeek {
                offset,
                whence: format!("{whence:?}"),
            })
        }
    }

    /// 关闭底层读取器。
    pub fn Close(&mut self) -> Result<(), LoadDataError> {
        self.r.close()
    }

    /// 不支持获取文件大小。
    pub fn GetFileSize(&self) -> Result<u64, LoadDataError> {
        Err(LoadDataError::Backend(
            "unsupported GetFileSize on SimpleSeekerOnReadCloser".into(),
        ))
    }
}

impl IoRead for SimpleSeekerOnReadCloser {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.Read(buffer)
    }
}

impl IoSeek for SimpleSeekerOnReadCloser {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match position {
            SeekFrom::Current(0) => Ok(self.pos),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("unsupported seek: {other:?}"),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// 会话变量键类型（对应 Go 侧 load_data_var）。
pub struct loadDataVarKeyType(pub i32);

impl loadDataVarKeyType {
    /// 稳定字符串名。
    pub fn String(&self) -> &'static str {
        "load_data_var"
    }
}

impl Display for loadDataVarKeyType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.String())
    }
}

/// 默认 LOAD DATA 会话变量键。
pub const LoadDataVarKey: loadDataVarKeyType = loadDataVarKeyType(0);
/// 本地 readerBuilder 会话键。
pub const LoadDataReaderBuilderKey: loadDataVarKeyType = loadDataVarKeyType(1);
/// 本地读取 WaitGroup 会话键。
pub const LoadDataReaderWg: loadDataVarKeyType = loadDataVarKeyType(2);
