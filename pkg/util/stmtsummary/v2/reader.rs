// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 语句摘要读取器：内存当前窗口与历史日志文件。
//
// `MemReader` 从内存快照按权限（PROCESS 或所属用户）、digest 过滤与时间范围产出行；
// `HistoryReader` 扫描 `tidb-statements*.log`，以 scan/parse worker 流水线并发解析 JSON，
// 跳过非法行与 `evicted` 淘汰汇总行。文件名中的时间戳给出轮转文件的 end 边界。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use crate::{
    ColumnContext, ColumnFactory, ColumnValue, StmtRecord, makeColumnFactories, model, types,
};
use chrono::{Local, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded, select};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 轮转日志文件名中的时间格式（毫秒精度）。
pub const logFileTimeFormat: &str = "%Y-%m-%dT%H-%M-%S%.3f";
/// 单行最大字节数，超限视为损坏。
pub const maxLineSize: usize = 1_073_741_824;
/// 扫描批次大小：每次向 parse worker 投递的行数。
pub const batchScanSize: usize = 64;

/// 可配置的语句摘要日志路径（默认 `tidb-statements.log`）。
static STMT_SUMMARY_FILENAME: LazyLock<RwLock<PathBuf>> =
    LazyLock::new(|| RwLock::new(PathBuf::from("tidb-statements.log")));

/// 读取器错误包装，便于跨通道传递。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderError(pub String);

impl fmt::Display for ReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReaderError {}

impl From<io::Error> for ReaderError {
    fn from(value: io::Error) -> Self {
        Self(value.to_string())
    }
}

impl From<serde_json::Error> for ReaderError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}

/// 设置全局语句摘要日志文件路径（测试与配置共用）。
pub fn setStmtSummaryFilename(path: impl Into<PathBuf>) {
    *STMT_SUMMARY_FILENAME
        .write()
        .expect("statement filename lock poisoned") = path.into();
}

fn stmtSummaryFilename() -> PathBuf {
    STMT_SUMMARY_FILENAME
        .read()
        .expect("statement filename lock poisoned")
        .clone()
}

/// 查询时间窗口 `[Begin, End]`；End=0 表示开放右端。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StmtTimeRange {
    pub Begin: i64,
    pub End: i64,
}

/// 行过滤条件：用户权限、digest 白名单、时间范围。
#[derive(Clone, Debug, Default)]
pub struct StmtChecker {
    user: Option<String>,
    has_process_priv: bool,
    digests: Option<HashSet<String>>,
    time_ranges: Vec<StmtTimeRange>,
}

impl StmtChecker {
    /// 构造过滤器；`has_process_priv` 为真时跳过用户归属校验。
    pub fn new(
        user: Option<String>,
        has_process_priv: bool,
        digests: Option<HashSet<String>>,
        time_ranges: Vec<StmtTimeRange>,
    ) -> Self {
        Self {
            user,
            has_process_priv,
            digests,
            time_ranges,
        }
    }

    /// 无 PROCESS 权限时，要求记录 AuthUsers 包含当前用户。
    pub fn hasPrivilege(&self, auth_users: &HashSet<String>) -> bool {
        match (&self.user, self.has_process_priv) {
            (Some(user), false) => !auth_users.is_empty() && auth_users.contains(user),
            _ => true,
        }
    }

    /// digest 白名单为空表示不过滤。
    pub fn isDigestValid(&self, digest: &str) -> bool {
        self.digests
            .as_ref()
            .is_none_or(|digests| digests.contains(digest))
    }

    /// 记录时间与任一查询区间重叠则有效。
    pub fn isTimeValid(&self, begin: i64, end: i64) -> bool {
        self.time_ranges.is_empty()
            || self
                .time_ranges
                .iter()
                .any(|range| timeRangeOverlap(begin, end, range.Begin, range.End))
    }

    /// 当前文件/批次 begin 已越过所有区间 End 时可提前停止扫描。
    pub fn needStop(&self, current_begin: i64) -> bool {
        !self.time_ranges.is_empty()
            && self
                .time_ranges
                .iter()
                .all(|range| range.End != 0 && range.End < current_begin)
    }
}

/// 内存当前摘要窗口快照：记录列表 + 可选淘汰汇总行。
#[derive(Clone, Debug, Default)]
pub struct MemWindowSnapshot {
    pub begin: i64,
    pub records: Vec<StmtRecord>,
    pub evicted: Option<StmtRecord>,
}

/// 内存摘要数据源，由 stmtsummary 窗口实现。
pub trait MemorySummarySource: Send + Sync {
    fn currentWindowSnapshot(&self) -> Option<MemWindowSnapshot>;
}

/// 当前窗口内存读取器。
pub struct MemReader<'a> {
    source: Option<&'a dyn MemorySummarySource>,
    context: ColumnContext,
    column_factories: Vec<ColumnFactory>,
    checker: StmtChecker,
}

/// 构造 MemReader，内部组装列工厂与 StmtChecker。
pub fn NewMemReader<'a>(
    source: Option<&'a dyn MemorySummarySource>,
    columns: &[model::ColumnInfo],
    instance_addr: impl Into<String>,
    time_location: Tz,
    user: Option<String>,
    has_process_priv: bool,
    digests: Option<HashSet<String>>,
    time_ranges: Vec<StmtTimeRange>,
) -> MemReader<'a> {
    MemReader {
        source,
        context: ColumnContext::new(instance_addr, time_location),
        column_factories: makeColumnFactories(columns),
        checker: StmtChecker::new(user, has_process_priv, digests, time_ranges),
    }
}

impl MemReader<'_> {
    /// 产出当前窗口过滤后的 Datum 行；无 digest 过滤时附带淘汰汇总行。
    pub fn Rows(&self) -> Vec<Vec<types::Datum>> {
        let Some(mut window) = self
            .source
            .and_then(MemorySummarySource::currentWindowSnapshot)
        else {
            return Vec::new();
        };
        let end = unixNow();
        if !self.checker.isTimeValid(window.begin, end) {
            return Vec::new();
        }
        let mut rows =
            Vec::with_capacity(window.records.len() + usize::from(window.evicted.is_some()));
        for mut record in window.records {
            if !self.checker.isDigestValid(&record.Digest)
                || !self.checker.hasPrivilege(&record.AuthUsers)
            {
                continue;
            }
            record.Begin = window.begin;
            record.End = end;
            rows.push(buildRow(&self.context, &self.column_factories, &record));
        }
        // 指定 digest 查询时不返回淘汰“其他”汇总行，避免污染结果。
        if self.checker.digests.is_none()
            && let Some(mut record) = window.evicted.take()
            && record.ExecCount > 0
            && self.checker.hasPrivilege(&record.AuthUsers)
        {
            record.Begin = window.begin;
            record.End = end;
            rows.push(buildRow(&self.context, &self.column_factories, &record));
        }
        rows
    }
}

fn unixNow() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64)
}

/// 已打开的语句日志文件及其时间边界。
#[derive(Debug)]
pub struct StmtFile {
    pub file: File,
    pub begin: i64,
    pub end: i64,
    pub path: PathBuf,
}

/// 打开日志文件：解析首条合法 begin，从文件名解析 end，再 seek 回起点。
pub fn openStmtFile(path: impl AsRef<Path>) -> Result<StmtFile, ReaderError> {
    let path = path.as_ref().to_path_buf();
    let mut file = OpenOptions::new().read(true).open(&path)?;
    let begin = match parseBeginTsAndReseek(&mut file) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => 0,
        Err(error) => return Err(error.into()),
    };
    let end = parseEndTs(&path)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(StmtFile {
        file,
        begin,
        end,
        path,
    })
}

/// 仅反序列化 begin/end 的轻量记录，用于扫描过滤。
#[derive(Deserialize)]
struct stmtTinyRecord {
    #[serde(default, rename = "begin", alias = "Begin")]
    Begin: i64,
    #[serde(default, rename = "end", alias = "End")]
    End: i64,
}

/// 跳过非法行找到首条 JSON 的 Begin，并将文件指针复位到起点。
pub fn parseBeginTsAndReseek(file: &mut File) -> io::Result<i64> {
    file.seek(SeekFrom::Start(0))?;
    let begin = {
        let mut reader = BufReader::new(&mut *file);
        loop {
            let line = readLine(&mut reader)?;
            if let Ok(record) = serde_json::from_slice::<stmtTinyRecord>(&line) {
                break record.Begin;
            }
        }
    };
    file.seek(SeekFrom::Start(0))?;
    Ok(begin)
}

/// 从轮转文件名 `{prefix}-{timestamp}` 解析结束时间；当前活动文件返回 0。
pub fn parseEndTs(path: impl AsRef<Path>) -> Result<i64, ReaderError> {
    let configured = stmtSummaryFilename();
    let prefix = configured
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("tidb-statements");
    let stem = path
        .as_ref()
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let Some(timestamp) = stem.strip_prefix(&format!("{prefix}-")) else {
        return Ok(0);
    };
    let naive = NaiveDateTime::parse_from_str(timestamp, logFileTimeFormat)
        .map_err(|error| ReaderError(error.to_string()))?;
    let local = Local
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| ReaderError("ambiguous statement log timestamp".to_owned()))?;
    Ok(local.timestamp())
}

/// 目录项只保存路径；活动文件额外钉住打开的 inode，避免轮转竞态。
pub(crate) struct StmtFileCandidate {
    pub(crate) path: PathBuf,
    pub(crate) opened: Option<StmtFile>,
}

#[cfg(unix)]
fn sameFile(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    first.dev() == second.dev() && first.ino() == second.ino()
}

#[cfg(not(unix))]
fn sameFile(_first: &fs::Metadata, _second: &fs::Metadata) -> bool {
    false
}

/// 目录下与配置文件名匹配的日志集合，按路径排序。
pub(crate) struct StmtFiles {
    pub(crate) files: Vec<StmtFileCandidate>,
    current_file_info: Option<fs::Metadata>,
}

impl StmtFiles {
    /// 先打开活动文件，再枚举目录；轮转文件到消费时才打开。
    fn new() -> Result<Self, ReaderError> {
        Self::newWithReadDir(|directory| fs::read_dir(directory)?.collect())
    }

    pub(crate) fn newWithReadDir(
        read_dir: impl FnOnce(&Path) -> io::Result<Vec<fs::DirEntry>>,
    ) -> Result<Self, ReaderError> {
        let filename = stmtSummaryFilename();
        let directory = filename
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let extension = filename
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let stem = filename
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let current = openStmtFile(&filename).ok();
        let current_file_info = current
            .as_ref()
            .map(|file| file.file.metadata())
            .transpose()?;
        let mut files = Vec::new();
        if let Some(opened) = current {
            files.push(StmtFileCandidate {
                path: filename.clone(),
                opened: Some(opened),
            });
        }
        for entry in read_dir(directory)? {
            if entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            let candidate_stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if path.extension().and_then(|value| value.to_str()) != Some(extension)
                || !(candidate_stem == stem || candidate_stem.starts_with(&format!("{stem}-")))
            {
                continue;
            }
            if path.file_name() == filename.file_name() {
                if current_file_info.is_none() {
                    files.push(StmtFileCandidate { path, opened: None });
                }
                continue;
            }
            if let (Some(current), Ok(candidate)) = (&current_file_info, entry.metadata()) {
                if sameFile(current, &candidate) {
                    continue;
                }
            }
            files.push(StmtFileCandidate { path, opened: None });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(Self {
            files,
            current_file_info,
        })
    }
}

/// 历史日志异步读取器：后台调度 scan/parse，前台通过通道取行。
pub struct HistoryReader {
    rows_rx: Receiver<Vec<Vec<ColumnValue>>>,
    error_rx: Receiver<ReaderError>,
    cancelled: Arc<AtomicBool>,
    scheduler: Option<JoinHandle<()>>,
}

/// 启动历史读取：并发度至少为 2（一半 scan、一半 parse）。
pub fn NewHistoryReader(
    columns: &[model::ColumnInfo],
    instance_addr: impl Into<String>,
    time_location: Tz,
    user: Option<String>,
    has_process_priv: bool,
    digests: Option<HashSet<String>>,
    time_ranges: Vec<StmtTimeRange>,
    concurrent: usize,
) -> Result<HistoryReader, ReaderError> {
    let files = StmtFiles::new()?;
    let concurrent = concurrent.max(2);
    let (rows_tx, rows_rx) = bounded(concurrent);
    let (error_tx, error_rx) = bounded(concurrent);
    let cancelled = Arc::new(AtomicBool::new(false));
    let checker = Arc::new(StmtChecker::new(
        user,
        has_process_priv,
        digests,
        time_ranges,
    ));
    let factories = makeColumnFactories(columns);
    let context = ColumnContext::new(instance_addr, time_location);
    let scheduler_cancelled = Arc::clone(&cancelled);
    let scheduler = thread::spawn(move || {
        scheduleTasks(
            files,
            concurrent,
            context,
            factories,
            checker,
            scheduler_cancelled,
            rows_tx,
            error_tx,
        );
    });
    Ok(HistoryReader {
        rows_rx,
        error_rx,
        cancelled,
        scheduler: Some(scheduler),
    })
}

impl HistoryReader {
    /// 取下一批行；通道关闭且无错误时返回 None。
    pub fn Rows(&mut self) -> Result<Option<Vec<Vec<types::Datum>>>, ReaderError> {
        loop {
            select! {
                recv(self.error_rx) -> error => match error {
                    Ok(error) => return Err(error),
                    Err(_) => {}
                },
                recv(self.rows_rx) -> rows => match rows {
                    Ok(rows) if rows.is_empty() => continue,
                    Ok(rows) => return Ok(Some(rows.into_iter().map(|row| {
                        row.into_iter().map(ColumnValue::into_datum).collect()
                    }).collect())),
                    Err(_) => {
                        if let Ok(error) = self.error_rx.try_recv() { return Err(error); }
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// 取消并等待调度线程结束。
    pub fn Close(&mut self) -> Result<(), ReaderError> {
        self.cancelled.store(true, Ordering::Release);
        if let Some(scheduler) = self.scheduler.take() {
            scheduler
                .join()
                .map_err(|_| ReaderError("statement history scheduler panicked".to_owned()))?;
        }
        Ok(())
    }
}

impl Drop for HistoryReader {
    fn drop(&mut self) {
        let _ = self.Close();
    }
}

/// 调度文件扫描与解析 worker，并把结果送入 rows/error 通道。
#[allow(clippy::too_many_arguments)]
fn scheduleTasks(
    files: StmtFiles,
    concurrent: usize,
    context: ColumnContext,
    factories: Vec<ColumnFactory>,
    checker: Arc<StmtChecker>,
    cancelled: Arc<AtomicBool>,
    rows_tx: Sender<Vec<Vec<ColumnValue>>>,
    error_tx: Sender<ReaderError>,
) {
    if files.files.is_empty() {
        return;
    }
    // Unbuffered handoff keeps open descriptors bounded by scan workers.
    let (files_tx, files_rx) = bounded(0);
    let (lines_tx, lines_rx) = bounded(concurrent);
    let (scan_done_tx, scan_done_rx) = bounded(concurrent);
    let scan_count = concurrent / 2;
    let mut workers = Vec::with_capacity(concurrent);

    // 前半 worker：先 scan 再转 parse；后半只 parse。
    for _ in 0..scan_count {
        let files_rx = files_rx.clone();
        let lines_tx = lines_tx.clone();
        let lines_rx = lines_rx.clone();
        let scan_done_tx = scan_done_tx.clone();
        let checker = Arc::clone(&checker);
        let parse_checker = Arc::clone(&checker);
        let cancelled = Arc::clone(&cancelled);
        let parse_cancelled = Arc::clone(&cancelled);
        let rows_tx = rows_tx.clone();
        let error_tx = error_tx.clone();
        let parse_error_tx = error_tx.clone();
        let context = context.clone();
        let factories = factories.clone();
        workers.push(thread::spawn(move || {
            scanWorker(
                files_rx,
                lines_tx,
                checker,
                Arc::clone(&cancelled),
                error_tx,
            );
            let _ = scan_done_tx.send(());
            parseWorker(
                lines_rx,
                &context,
                &factories,
                parse_checker,
                parse_cancelled,
                rows_tx,
                parse_error_tx,
            );
        }));
    }
    for _ in scan_count..concurrent {
        let lines_rx = lines_rx.clone();
        let checker = Arc::clone(&checker);
        let cancelled = Arc::clone(&cancelled);
        let rows_tx = rows_tx.clone();
        let error_tx = error_tx.clone();
        let context = context.clone();
        let factories = factories.clone();
        workers.push(thread::spawn(move || {
            parseWorker(
                lines_rx, &context, &factories, checker, cancelled, rows_tx, error_tx,
            );
        }));
    }
    drop(files_rx);
    drop(lines_rx);
    drop(scan_done_tx);
    for candidate in files.files {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let file = match candidate.opened {
            Some(file) => file,
            None => match openStmtFile(&candidate.path) {
                Ok(file) => file,
                Err(_) => continue,
            },
        };
        if let Some(current) = &files.current_file_info {
            if candidate.path.file_name() != stmtSummaryFilename().file_name() {
                match file.file.metadata() {
                    Ok(info) if sameFile(current, &info) => continue,
                    Ok(_) => {}
                    Err(error) => {
                        let _ = error_tx.try_send(error.into());
                        cancelled.store(true, Ordering::Release);
                        break;
                    }
                }
            }
        }
        if !checker.isTimeValid(file.begin, file.end) {
            continue;
        }
        if !sendCancelable(&files_tx, &cancelled, file) {
            break;
        }
    }
    drop(files_tx);
    for _ in 0..scan_count {
        if scan_done_rx.recv().is_err() {
            break;
        }
    }
    drop(lines_tx);
    for worker in workers {
        if worker.join().is_err() {
            let _ = error_tx.try_send(ReaderError("statement history worker panicked".to_owned()));
            cancelled.store(true, Ordering::Release);
        }
    }
}

/// 从文件通道取 StmtFile，按批读行送入 lines 通道。
fn scanWorker(
    files_rx: Receiver<StmtFile>,
    lines_tx: Sender<Vec<Vec<u8>>>,
    checker: Arc<StmtChecker>,
    cancelled: Arc<AtomicBool>,
    error_tx: Sender<ReaderError>,
) {
    while !cancelled.load(Ordering::Acquire) {
        let Ok(mut statement_file) = files_rx.recv() else {
            return;
        };
        let mut reader = BufReader::new(&mut statement_file.file);
        loop {
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            match readBatch(&mut reader, batchScanSize, &checker) {
                Ok(Some(lines)) => {
                    if !sendCancelable(&lines_tx, &cancelled, lines) {
                        return;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = error_tx.try_send(error);
                    cancelled.store(true, Ordering::Release);
                    return;
                }
            }
        }
    }
}

/// 读取一批原始行；遇 needStop 或 EOF 结束。
fn readBatch<R: BufRead>(
    reader: &mut R,
    batch_size: usize,
    checker: &StmtChecker,
) -> Result<Option<Vec<Vec<u8>>>, ReaderError> {
    let first = loop {
        match readLine(reader) {
            Ok(line) => match serde_json::from_slice::<stmtTinyRecord>(&line) {
                Ok(record) if checker.needStop(record.Begin) => return Ok(None),
                Ok(_) => break line,
                Err(_) => continue,
            },
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    };
    let mut lines = Vec::with_capacity(batch_size);
    lines.push(first);
    lines.extend(readLines(reader, batch_size.saturating_sub(1))?);
    Ok(Some(lines))
}

/// 持久化 JSON 行：完整 StmtRecord + 可选 evicted 标记。
#[derive(Deserialize)]
struct stmtPersistedRecord {
    #[serde(flatten)]
    record: StmtRecord,
    #[serde(default, rename = "evicted")]
    evicted: bool,
}

/// 解析行批次，过滤后经列工厂生成 ColumnValue 行。
fn parseWorker(
    lines_rx: Receiver<Vec<Vec<u8>>>,
    context: &ColumnContext,
    factories: &[ColumnFactory],
    checker: Arc<StmtChecker>,
    cancelled: Arc<AtomicBool>,
    rows_tx: Sender<Vec<Vec<ColumnValue>>>,
    _error_tx: Sender<ReaderError>,
) {
    while !cancelled.load(Ordering::Acquire) {
        let Ok(lines) = lines_rx.recv() else {
            return;
        };
        let mut rows = Vec::with_capacity(lines.len());
        for line in lines {
            let Ok(persisted) = serde_json::from_slice::<stmtPersistedRecord>(&line) else {
                continue;
            };
            // 历史查询跳过淘汰汇总行，仅保留真实 digest 记录。
            if persisted.evicted {
                continue;
            }
            let record = persisted.record;
            if checker.needStop(record.Begin) {
                break;
            }
            if checker.isTimeValid(record.Begin, record.End)
                && checker.isDigestValid(&record.Digest)
                && checker.hasPrivilege(&record.AuthUsers)
            {
                rows.push(buildValues(context, factories, &record));
            }
        }
        if !rows.is_empty() && !sendCancelable(&rows_tx, &cancelled, rows) {
            return;
        }
    }
}

fn buildRow(
    context: &ColumnContext,
    factories: &[ColumnFactory],
    record: &StmtRecord,
) -> Vec<types::Datum> {
    factories
        .iter()
        .map(|factory| factory(context, record).into_datum())
        .collect()
}

fn buildValues(
    context: &ColumnContext,
    factories: &[ColumnFactory],
    record: &StmtRecord,
) -> Vec<ColumnValue> {
    factories
        .iter()
        .map(|factory| factory(context, record))
        .collect()
}

/// 可取消的带超时发送，避免取消时永久阻塞。
fn sendCancelable<T>(sender: &Sender<T>, cancelled: &AtomicBool, mut value: T) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        match sender.send_timeout(value, Duration::from_millis(10)) {
            Ok(()) => return true,
            Err(SendTimeoutError::Timeout(returned)) => value = returned,
            Err(SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

/// 读取一行（含长度上限），剥除尾部 `\n`/`\r`。
pub fn readLine<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let read = reader
        .take((maxLineSize + 1) as u64)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    if line.len() > maxLineSize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "statement summary line is too long",
        ));
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(line)
}

/// 连续读取至多 `count` 行，遇 EOF 提前结束。
pub fn readLines<R: BufRead>(reader: &mut R, count: usize) -> io::Result<Vec<Vec<u8>>> {
    let mut lines = Vec::with_capacity(count);
    for _ in 0..count {
        match readLine(reader) {
            Ok(line) => lines.push(line),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
    }
    Ok(lines)
}

/// 判断两时间区间是否重叠；End=0 或 End 小于 Begin 视为开放右端。
pub fn timeRangeOverlap(a_begin: i64, mut a_end: i64, b_begin: i64, mut b_end: i64) -> bool {
    if a_end == 0 || a_end < a_begin {
        a_end = i64::MAX;
    }
    if b_end == 0 || b_end < b_begin {
        b_end = i64::MAX;
    }
    a_begin <= b_end && a_end >= b_begin
}
