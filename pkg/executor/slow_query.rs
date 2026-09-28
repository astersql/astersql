// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 慢查询（slow query）日志检索与解析。
//
// 从慢日志文件批量/反向读取条目，按权限与时间范围过滤，异步解析为行（Row），
// 并跟踪内存与运行时统计。`SlowQueryRuntime` 抽象文件系统、列工厂与会话依赖。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// 信号上下文键占位类型（对应 Go context 中的 signals key）。
pub struct signalsKey;
/// 异步解析慢日志时每批行数（默认 64）。
pub static ParseSlowLogBatchSize: AtomicUsize = AtomicUsize::new(64);
/// 时间范围重叠判断时的容差（1 秒）。
pub const slowLogTimeRangeInternalTolerance: Duration = Duration::from_secs(1);
/// 从文件尾部回读时的最大缓存字节数（64MiB）。
pub const maxReadCacheSize: usize = 64 * 1024 * 1024;

/// 查询时间窗口：`startTime`..=`endTime`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct timeRange<T> {
    /// 窗口起始时间。
    pub startTime: T,
    /// 窗口结束时间。
    pub endTime: T,
}

/// 单个慢日志文件描述：句柄、文件内最早时间戳、是否压缩。
#[derive(Clone, Debug)]
pub struct logFile<F, T> {
    /// 文件句柄（由 Runtime 提供）。
    pub file: F,
    /// 该文件内最早的 `# Time:`。
    pub start: T,
    /// 是否为 gzip 等压缩日志。
    pub compressed: bool,
}

/// 日志中的文件+行偏移，用于错误定位。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct offset {
    /// 文件下标。
    pub file: usize,
    /// 行号。
    pub line: usize,
}

/// 一条慢日志条目的原始行块。
pub type slowLogBlock = Vec<String>;

/// 投递给异步解析器的一批日志行及起始偏移。
pub struct slowLogTask {
    /// 待解析的日志行。
    pub log: Vec<String>,
    /// 该批在文件中的起始偏移。
    pub offset: offset,
}

/// 异步解析结果：成功行集或错误。
pub struct parsedSlowLog<Row, Error> {
    /// 解析出的结果行。
    pub rows: Vec<Row>,
    /// 解析错误（若有）。
    pub err: Option<Error>,
}

/// 权限与时间过滤检查器。
#[derive(Clone, Debug)]
pub struct slowLogChecker<T> {
    /// 是否拥有 PROCESS 权限（可看全部用户慢日志）。
    pub hasProcessPriv: bool,
    /// 当前会话用户名。
    pub user: String,
    /// 是否启用时间窗口过滤。
    pub enableTimeCheck: bool,
    /// 允许的时间窗口列表。
    pub timeRanges: Vec<timeRange<T>>,
}

impl<T> slowLogChecker<T> {
    /// 无 PROCESS 权限时仅允许查看本人慢日志。
    pub fn hasPrivilege(&self, user_name: &str) -> bool {
        self.hasProcessPriv || self.user == user_name
    }
}

impl<T: Ord> slowLogChecker<T> {
    /// 时间是否落在任一允许窗口内；未启用时间检查则恒为 true。
    pub fn isTimeValid(&self, time: &T) -> bool {
        !self.enableTimeCheck
            || self
                .timeRanges
                .iter()
                .any(|range| &range.startTime <= time && time <= &range.endTime)
    }
}

/// Production slow-log boundary. The retriever keeps TiDB's batching, file
/// ordering, reverse scan, privilege/time filtering, memory and statistics
/// lifecycle. Implementations must supply real filesystem/gzip, Datum and
/// session warning operations; no method has a successful default.
///
/// 慢查询运行时边界：实现方需提供真实文件系统/gzip、Datum 与会话告警等能力。
pub trait SlowQueryRuntime {
    /// 会话/请求上下文。
    type Context;
    /// 日志文件句柄类型。
    type File: Clone;
    /// 按行读取器。
    type Reader;
    /// 输出结果行。
    type Row;
    /// 列 Datum 类型。
    type Datum;
    /// 可比较的时间戳类型。
    type Time: Clone + Ord;
    /// 错误类型。
    type Error;
    /// 按列名设置字段值的工厂。
    type ColumnFactory: Clone;
    /// 取消令牌。
    type Cancel;

    /// 输出列名列表。
    fn output_columns(&self) -> Vec<String>;
    /// 当前用户名。
    fn current_user(&self) -> String;
    /// 是否有 PROCESS 权限。
    fn has_process_privilege(&self) -> bool;
    /// 查询时间窗口。
    fn time_ranges(&self) -> Vec<timeRange<Self::Time>>;
    /// 是否按时间降序扫描。
    fn descending(&self) -> bool;
    /// 结果行数上限。
    fn limit(&self) -> u64;
    /// 慢日志路径。
    fn slow_log_path(&self) -> String;
    /// 上下文是否已取消。
    fn context_cancelled(&self, context: &Self::Context) -> Option<Self::Error>;
    /// 创建取消令牌。
    fn create_cancel(&mut self, context: &Self::Context) -> Self::Cancel;
    /// 触发取消。
    fn cancel(&mut self, cancel: Self::Cancel);

    /// 列出路径下全部慢日志文件。
    fn list_log_files(
        &mut self,
        context: &Self::Context,
        path: &str,
    ) -> Result<Vec<logFile<Self::File, Self::Time>>, Self::Error>;
    /// 打开文件读取器（可处理压缩）。
    fn reader(&mut self, file: &Self::File, compressed: bool) -> Result<Self::Reader, Self::Error>;
    /// 读取下一行原始字节。
    fn read_line(&mut self, reader: &mut Self::Reader) -> Result<Option<Vec<u8>>, Self::Error>;
    /// 从 end_cursor 向前读取末尾若干行。
    fn read_last_lines(
        &mut self,
        context: &Self::Context,
        file: &Self::File,
        end_cursor: i64,
        max_cache: usize,
    ) -> Result<(Vec<String>, usize), Self::Error>;
    /// 文件大小（字节）。
    fn file_size(&self, file: &Self::File) -> Result<i64, Self::Error>;
    /// 将文件指针重置到开头。
    fn rewind(&mut self, file: &Self::File) -> Result<(), Self::Error>;
    /// 关闭文件。
    fn close_file(&mut self, file: &Self::File) -> Result<(), Self::Error>;

    /// 按列名获取列值工厂；未知列返回 None。
    fn column_factory(
        &self,
        name: &str,
        index: usize,
    ) -> Result<Option<Self::ColumnFactory>, Self::Error>;
    /// INSTANCE 列工厂。
    fn instance_factory(&self, index: usize) -> Result<Self::ColumnFactory, Self::Error>;
    /// 新建空结果行。
    fn new_row(&self) -> Self::Row;
    /// 用工厂把字段值写入行；返回 false 表示该行应丢弃（如时间不匹配）。
    fn set_column(
        &mut self,
        row: &mut Self::Row,
        factory: &Self::ColumnFactory,
        value: &str,
        checker: &slowLogChecker<Self::Time>,
    ) -> Result<bool, Self::Error>;
    /// 写入 INSTANCE 列。
    fn set_instance(&mut self, row: &mut Self::Row, factory: &Self::ColumnFactory);
    /// 填充未出现字段的默认值。
    fn set_default_values(&mut self, row: &mut Self::Row);
    /// 解析 `# Time:` 字符串。
    fn parse_time(&self, value: &str) -> Result<Self::Time, Self::Error>;
    /// 写入 Query SQL 文本。
    fn set_query_sql(&mut self, row: &mut Self::Row, value: &str) -> Result<(), Self::Error>;
    /// 追加会话 warning。
    fn append_warning(&mut self, error: Self::Error);
    /// 估算一行占用内存。
    fn estimated_row_size(&self, row: &Self::Row) -> i64;
    /// 增加内存记账。
    fn consume_memory(&mut self, bytes: i64);
    /// 释放内存记账。
    fn release_memory(&mut self, bytes: i64);
    /// 解码 tidb_decode_plan 载荷。
    fn decode_plan(&self, plan: &str) -> Result<String, Self::Error>;

    /// 启动异步解析 worker。
    fn spawn_parser(
        &mut self,
        context: &Self::Context,
        batch_size: usize,
    ) -> Result<(), Self::Error>;
    /// 接收一批已解析结果；None 表示结束。
    fn receive_parsed(
        &mut self,
        context: &Self::Context,
    ) -> Result<Option<parsedSlowLog<Self::Row, Self::Error>>, Self::Error>;
    /// 等待解析 worker 退出。
    fn wait_parser(&mut self);
}

/// 慢查询检索器：管理文件列表、列工厂、异步解析与内存统计。
pub struct slowQueryRetriever<R: SlowQueryRuntime> {
    /// 运行时实现。
    pub runtime: R,
    /// 是否已完成 initialize。
    pub initialized: bool,
    /// 按时间排序后的日志文件列表。
    pub files: Vec<logFile<R::File, R::Time>>,
    /// 当前扫描到的文件下标。
    pub fileIdx: usize,
    /// 当前文件内行号。
    pub fileLine: usize,
    /// 权限/时间检查器。
    pub checker: Option<slowLogChecker<R::Time>>,
    /// 列名 → 列值工厂。
    pub columnValueFactoryMap: HashMap<String, R::ColumnFactory>,
    /// INSTANCE 列工厂。
    pub instanceFactory: Option<R::ColumnFactory>,
    /// 运行时统计。
    pub stats: slowQueryRuntimeStats,
    /// 上一批 fetch 占用的内存字节数。
    pub lastFetchSize: i64,
    /// 取消令牌。
    pub cancel: Option<R::Cancel>,
}

impl<R: SlowQueryRuntime> slowQueryRetriever<R> {
    /// 取下一批慢查询结果行；首次调用时完成初始化并启动异步解析。
    pub fn retrieve(&mut self, context: &R::Context) -> Result<Vec<R::Row>, R::Error> {
        if !self.initialized {
            self.initialize(context)?;
            self.cancel = Some(self.runtime.create_cancel(context));
            self.initializeAsyncParsing(context)?;
        }
        self.dataForSlowLog(context)
    }

    /// 构建列工厂、检查器，并按时间排序（可选降序）收集全部日志文件。
    pub fn initialize(&mut self, context: &R::Context) -> Result<(), R::Error> {
        self.columnValueFactoryMap.clear();
        for (index, column) in self.runtime.output_columns().into_iter().enumerate() {
            if column.eq_ignore_ascii_case("INSTANCE") {
                self.instanceFactory = Some(self.runtime.instance_factory(index)?);
            } else if let Some(factory) = self.runtime.column_factory(&column, index)? {
                self.columnValueFactoryMap.insert(column, factory);
            }
        }
        self.checker = Some(slowLogChecker {
            hasProcessPriv: self.runtime.has_process_privilege(),
            user: self.runtime.current_user(),
            enableTimeCheck: !self.runtime.time_ranges().is_empty(),
            timeRanges: self.runtime.time_ranges(),
        });
        self.files = self.getAllFiles(context, &self.runtime.slow_log_path())?;
        if self.runtime.descending() {
            self.files.reverse();
        }
        self.initialized = true;
        Ok(())
    }

    /// 关闭全部文件、取消并等待解析器，释放上一批内存记账。
    pub fn close(&mut self) -> Result<(), R::Error> {
        let mut first = None;
        for file in &self.files {
            if let Err(error) = self.runtime.close_file(&file.file)
                && first.is_none()
            {
                first = Some(error);
            }
        }
        if let Some(cancel) = self.cancel.take() {
            self.runtime.cancel(cancel);
        }
        self.runtime.wait_parser();
        self.memConsume(-self.lastFetchSize);
        first.map_or(Ok(()), Err)
    }

    /// 推进到下一个日志文件并更新读文件统计。
    pub fn getNextFile(&mut self) -> Option<logFile<R::File, R::Time>> {
        let file = self.files.get(self.fileIdx)?.clone();
        self.fileIdx += 1;
        if let Ok(size) = self.runtime.file_size(&file.file) {
            self.stats.readFileNum += 1;
            self.stats.readFileSize += size;
        }
        Some(file)
    }

    /// 打开「当前文件的前一个」文件的读取器（反向扫描辅助）。
    pub fn getPreviousReader(&mut self) -> Result<Option<R::Reader>, R::Error> {
        let Some(index) = self.fileIdx.checked_sub(2) else {
            return Ok(None);
        };
        let file = &self.files[index];
        self.runtime.reader(&file.file, file.compressed).map(Some)
    }

    /// 打开下一个文件的读取器。
    pub fn getNextReader(&mut self) -> Result<Option<R::Reader>, R::Error> {
        let Some(file) = self.getNextFile() else {
            return Ok(None);
        };
        self.runtime.reader(&file.file, file.compressed).map(Some)
    }

    /// 按全局批次大小启动异步解析。
    pub fn parseDataForSlowLog(&mut self, context: &R::Context) -> Result<(), R::Error> {
        self.runtime
            .spawn_parser(context, ParseSlowLogBatchSize.load(Ordering::Acquire))
    }

    /// 从异步通道取一批已解析行，并更新内存记账。
    pub fn dataForSlowLog(&mut self, context: &R::Context) -> Result<Vec<R::Row>, R::Error> {
        // 先释放上一批占用，再记账本批。
        self.memConsume(-self.lastFetchSize);
        let Some(parsed) = self.runtime.receive_parsed(context)? else {
            self.lastFetchSize = 0;
            return Ok(Vec::new());
        };
        if let Some(error) = parsed.err {
            return Err(error);
        }
        self.lastFetchSize = calculateDatumsSize(&self.runtime, &parsed.rows);
        self.memConsume(self.lastFetchSize);
        Ok(parsed.rows)
    }

    /// 从 reader 连续读至多 batch_size 行，期间检查上下文取消。
    pub fn getBatchLog(
        &mut self,
        context: &R::Context,
        reader: &mut R::Reader,
        position: &mut offset,
        batch_size: u64,
    ) -> Result<Vec<String>, R::Error> {
        let mut lines = Vec::with_capacity(batch_size as usize);
        while lines.len() < batch_size as usize {
            if let Some(error) = self.runtime.context_cancelled(context) {
                return Err(error);
            }
            match self.runtime.read_line(reader)? {
                Some(line) => {
                    position.line += 1;
                    lines.push(String::from_utf8_lossy(&line).trim_end().to_owned());
                }
                None => break,
            }
        }
        Ok(lines)
    }

    /// 正向批量解析入口（委托 spawn_parser）。
    pub fn parseSlowLog(&mut self, context: &R::Context, batch_size: u64) -> Result<(), R::Error> {
        self.runtime.spawn_parser(context, batch_size as usize)
    }

    /// 反向批量解析入口（委托 spawn_parser）。
    pub fn parseSlowLogReversed(
        &mut self,
        context: &R::Context,
        batch_size: u64,
    ) -> Result<(), R::Error> {
        self.runtime.spawn_parser(context, batch_size as usize)
    }

    /// 按会话 limit 启动批量解析。
    pub fn parseSlowLogByBatchGetter(&mut self, context: &R::Context) -> Result<(), R::Error> {
        self.parseSlowLogByBatchGetterWithLimit(context, self.runtime.limit())
    }

    /// 带显式 limit 的批量解析（当前实现忽略 limit，直接 spawn）。
    pub fn parseSlowLogByBatchGetterWithLimit(
        &mut self,
        context: &R::Context,
        _limit: u64,
    ) -> Result<(), R::Error> {
        self.parseDataForSlowLog(context)
    }

    /// 发送已解析结果时的内存记账钩子。
    pub fn sendParsedSlowLogCh(&mut self, parsed: parsedSlowLog<R::Row, R::Error>) {
        let bytes = calculateDatumsSize(&self.runtime, &parsed.rows);
        self.memConsume(bytes);
    }

    /// 将原始日志行解析为结果行：识别 `# Time:` 开条目、`# ` 字段行与以 `;` 结尾的 SQL。
    pub fn parseLog(
        &mut self,
        _context: &R::Context,
        lines: &[String],
        base: offset,
    ) -> Result<Vec<R::Row>, R::Error> {
        let checker = self.checker.as_ref().expect("slow log initialized").clone();
        let mut result = Vec::new();
        let mut row = None;
        let mut user = String::new();
        for (index, line) in lines.iter().enumerate() {
            if let Some(value) = line.strip_prefix("# Time: ") {
                // 新条目开始：写入 Time 列，时间不匹配则丢弃该行。
                let mut next = self.runtime.new_row();
                if !self.setColumnValue(&mut next, "Time", value, &checker, base.line + index)? {
                    row = None;
                    continue;
                }
                row = Some(next);
            } else if let Some(fields) = line.strip_prefix("# ") {
                let Some(current) = row.as_mut() else {
                    continue;
                };
                let (names, values) = splitByColon(fields);
                for (name, value) in names.iter().zip(values.iter()) {
                    if name == "User" {
                        user = parseUserOrHostValue(value);
                    }
                    if !self.setColumnValue(current, name, value, &checker, base.line + index)? {
                        row = None;
                        break;
                    }
                }
            } else if line.ends_with(';') && !line.starts_with("use ") {
                // SQL 行结束当前条目；无权限则丢弃。
                if let Some(mut current) = row.take()
                    && checker.hasPrivilege(&user)
                {
                    self.runtime.set_query_sql(&mut current, line)?;
                    if let Some(factory) = &self.instanceFactory {
                        self.runtime.set_instance(&mut current, factory);
                    }
                    self.runtime.set_default_values(&mut current);
                    result.push(current);
                }
            }
        }
        Ok(result)
    }

    /// 按字段名写入列值；未知字段忽略，Time 走时间合法性检查。
    pub fn setColumnValue(
        &mut self,
        row: &mut R::Row,
        field: &str,
        value: &str,
        checker: &slowLogChecker<R::Time>,
        _line: usize,
    ) -> Result<bool, R::Error> {
        if let Some(factory) = self.columnValueFactoryMap.get(field).cloned() {
            self.runtime.set_column(row, &factory, value, checker)
        } else if field == "Time" {
            self.runtime
                .parse_time(value)
                .map(|time| checker.isTimeValid(&time))
        } else {
            Ok(true)
        }
    }

    /// 填充默认列值。
    pub fn setDefaultValue(&mut self, row: &mut R::Row) {
        self.runtime.set_default_values(row);
    }

    /// 列出路径下日志文件并按 start 时间升序排序。
    pub fn getAllFiles(
        &mut self,
        context: &R::Context,
        path: &str,
    ) -> Result<Vec<logFile<R::File, R::Time>>, R::Error> {
        let mut files = self.runtime.list_log_files(context, path)?;
        files.sort_by(|left, right| left.start.cmp(&right.start));
        self.stats.totalFileNum = files.len();
        Ok(files)
    }

    /// 扫描文件前若干行，提取最早的 `# Time:`。
    pub fn getFileStartTime(
        &mut self,
        file: &R::File,
        compressed: bool,
    ) -> Result<R::Time, R::Error> {
        let mut reader = self.runtime.reader(file, compressed)?;
        for _ in 0..128 {
            let Some(line) = self.runtime.read_line(&mut reader)? else {
                break;
            };
            let text = String::from_utf8_lossy(&line);
            if let Some(value) = text.strip_prefix("# Time: ") {
                return self.runtime.parse_time(value.trim());
            }
        }
        self.runtime.parse_time("")
    }

    /// 返回当前运行时统计快照。
    pub fn getRuntimeStats(&self) -> slowQueryRuntimeStats {
        self.stats.clone()
    }

    /// 从文件尾部回读，提取最晚的 `# Time:`。
    pub fn getFileEndTime(
        &mut self,
        context: &R::Context,
        file: &R::File,
    ) -> Result<R::Time, R::Error> {
        let mut cursor = self.runtime.file_size(file)?;
        let mut tried = 0;
        while cursor > 0 && tried < 128 {
            let (lines, bytes) =
                self.runtime
                    .read_last_lines(context, file, cursor, maxReadCacheSize)?;
            if bytes == 0 {
                break;
            }
            cursor -= bytes as i64;
            tried += lines.len();
            // 从后往前找最后一个 Time 标记。
            for line in lines.iter().rev() {
                if let Some(value) = line.strip_prefix("# Time: ") {
                    return self.runtime.parse_time(value.trim());
                }
            }
        }
        self.runtime.parse_time("")
    }

    /// 启动异步解析流水线。
    pub fn initializeAsyncParsing(&mut self, context: &R::Context) -> Result<(), R::Error> {
        self.parseDataForSlowLog(context)
    }

    /// 按符号增减内存记账：正数 consume，负数 release。
    pub fn memConsume(&mut self, bytes: i64) {
        if bytes >= 0 {
            self.runtime.consume_memory(bytes);
        } else {
            self.runtime.release_memory(bytes.saturating_abs());
        }
    }
}

/// 反向扫描器：按块从后往前取出慢日志条目行。
pub struct slowLogReverseScanner<'a, R: SlowQueryRuntime> {
    /// 所属检索器（借用可变以访问 runtime）。
    pub retriever: &'a mut slowQueryRetriever<R>,
    /// 当前文件下标。
    pub file_index: usize,
    /// 已加载、按逆序排列的日志块。
    pub blocks: Vec<slowLogBlock>,
}

impl<'a, R: SlowQueryRuntime> slowLogReverseScanner<'a, R> {
    /// 凑满至多 batch_size 行的反向批次。
    pub fn nextBatch(
        &mut self,
        context: &R::Context,
        batch_size: u64,
    ) -> Result<Vec<String>, R::Error> {
        let mut batch = Vec::new();
        while batch.len() < batch_size as usize {
            let block = self.nextBlock(context)?;
            if block.is_empty() {
                break;
            }
            batch.extend(block);
        }
        batch.truncate(batch_size as usize);
        Ok(batch)
    }

    /// 弹出下一块日志条目；空表示耗尽。
    pub fn nextBlock(&mut self, _context: &R::Context) -> Result<slowLogBlock, R::Error> {
        Ok(self.blocks.pop().unwrap_or_default())
    }

    /// 读取压缩文件全部行，按 `# Time:` 切分为条目并反转顺序。
    pub fn loadCompressedBlocks(
        &mut self,
        context: &R::Context,
        file: &R::File,
    ) -> Result<(), R::Error> {
        let mut reader = self.retriever.runtime.reader(file, true)?;
        let mut block = Vec::new();
        while let Some(line) = self.retriever.runtime.read_line(&mut reader)? {
            if self.retriever.runtime.context_cancelled(context).is_some() {
                break;
            }
            block.push(String::from_utf8_lossy(&line).trim_end().to_owned());
        }
        self.blocks = split_into_entries(block);
        self.blocks.reverse();
        Ok(())
    }
}

/// 构造空的反向扫描器。
pub fn newSlowLogReverseScanner<R: SlowQueryRuntime>(
    retriever: &mut slowQueryRetriever<R>,
) -> slowLogReverseScanner<'_, R> {
    slowLogReverseScanner {
        retriever,
        file_index: 0,
        blocks: Vec::new(),
    }
}

/// 按 `# Time:` 边界将连续行切分为多条慢日志条目。
fn split_into_entries(lines: Vec<String>) -> Vec<Vec<String>> {
    let mut entries = Vec::new();
    let mut current = Vec::new();
    for line in lines {
        if line.starts_with("# Time: ") && !current.is_empty() {
            entries.push(std::mem::take(&mut current));
        }
        current.push(line);
    }
    if !current.is_empty() {
        entries.push(current);
    }
    entries
}

/// 从 reader 读取一行（Runtime 封装）。
pub fn getOneLine<R: SlowQueryRuntime>(
    runtime: &mut R,
    reader: &mut R::Reader,
) -> Result<Option<Vec<u8>>, R::Error> {
    runtime.read_line(reader)
}

/// 计算批次内某行的绝对行号。
pub fn getLineIndex(base: offset, index: usize) -> usize {
    base.line.saturating_add(index)
}

/// 从 left 位置匹配 Go 慢日志语法允许的嵌套 `[]` / `{}`。
pub fn findMatchedRightBracket(line: &str, left: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let left_bracket = *bytes.get(left)?;
    let right_bracket = match left_bracket {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate().skip(left) {
        if *byte == left_bracket {
            depth += 1;
        } else if *byte == right_bracket {
            let Some(next_depth) = depth.checked_sub(1) else {
                return None;
            };
            depth = next_depth;
            if depth == 0 {
                if bytes.get(index + 1).is_some_and(|next| *next != b' ') {
                    return None;
                }
                return Some(index);
            }
        }
    }
    None
}

/// Go 用它寻找字段起点；只接受 ASCII 字母或数字。
pub fn isLetterOrNumeric(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// 将 `Field: value Field2: value2` 拆成字段名与值列表。
pub fn splitByColon(line: &str) -> (Vec<String>, Vec<String>) {
    let mut fields = Vec::with_capacity(1);
    let mut values = Vec::with_capacity(1);
    let bytes = line.as_bytes();
    let mut parse_key = true;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if parse_key {
            while cursor < bytes.len() && !isLetterOrNumeric(bytes[cursor]) {
                cursor += 1;
            }
            let start = cursor;
            if cursor >= bytes.len() {
                break;
            }
            while cursor < bytes.len() && bytes[cursor] != b':' {
                cursor += 1;
            }
            fields.push(line[start..cursor].to_owned());
            parse_key = false;
            cursor = cursor.saturating_add(2);
            if cursor >= bytes.len() {
                values.push(String::new());
            }
        } else {
            let start = cursor;
            if matches!(bytes.get(cursor), Some(b'{' | b'[')) {
                let Some(right) = findMatchedRightBracket(line, cursor) else {
                    return (Vec::new(), Vec::new());
                };
                cursor = right + 1;
            } else {
                while cursor < bytes.len() && bytes[cursor] != b' ' {
                    cursor += 1;
                }
                if cursor > 0 && bytes[cursor - 1] == b':' {
                    values.push(String::new());
                    cursor = start;
                    parse_key = true;
                    continue;
                }
            }
            values.push(line[start..cursor.min(bytes.len())].to_owned());
            parse_key = true;
        }
    }
    if fields.len() == values.len() {
        (fields, values)
    } else {
        (Vec::new(), Vec::new())
    }
}

/// 从 `user[host] @ ...` 形式中取出 `[` 前的用户名。
pub fn parseUserOrHostValue(value: &str) -> String {
    value.split('[').next().unwrap_or(value).trim().to_owned()
}

/// 按列名获取列值工厂。
pub fn getColumnValueFactoryByName<R: SlowQueryRuntime>(
    runtime: &R,
    name: &str,
    index: usize,
) -> Result<Option<R::ColumnFactory>, R::Error> {
    runtime.column_factory(name, index)
}

/// 获取 INSTANCE 列工厂。
pub fn getInstanceColumnValueFactory<R: SlowQueryRuntime>(
    runtime: &R,
    index: usize,
) -> Result<R::ColumnFactory, R::Error> {
    runtime.instance_factory(index)
}

/// 剥离 `tidb_decode_plan('...')` 包装后解码执行计划文本。
pub fn parsePlan<R: SlowQueryRuntime>(runtime: &R, value: &str) -> String {
    let stripped = value
        .strip_prefix("tidb_decode_plan('")
        .and_then(|value| value.strip_suffix("')"))
        .unwrap_or(value);
    runtime
        .decode_plan(stripped)
        .unwrap_or_else(|_| stripped.to_owned())
}

/// 解析慢日志时间字符串。
pub fn ParseTime<R: SlowQueryRuntime>(runtime: &R, value: &str) -> Result<R::Time, R::Error> {
    runtime.parse_time(value)
}

/// 判断 `[start,end]` 与查询窗口是否可能重叠（无容差）。
pub fn slowLogMayOverlapTimeRangeWithTolerance<T: Ord>(
    start: &T,
    end: &T,
    range: &timeRange<T>,
) -> bool {
    !(start > &range.endTime || end < &range.startTime)
}

/// 对时间加减容差；溢出时回退原值。
pub fn slowLogTimeWithTolerance(time: SystemTime, tolerance: Duration, add: bool) -> SystemTime {
    if add {
        time.checked_add(tolerance).unwrap_or(time)
    } else {
        time.checked_sub(tolerance).unwrap_or(time)
    }
}

/// 慢查询检索过程的运行时统计（文件数、读耗时、解析并发等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct slowQueryRuntimeStats {
    /// 日志文件总数。
    pub totalFileNum: usize,
    /// 实际读取的文件数。
    pub readFileNum: usize,
    /// 读文件累计耗时。
    pub readFile: Duration,
    /// 初始化耗时。
    pub initialize: Duration,
    /// 已读字节数。
    pub readFileSize: i64,
    /// 解析日志累计耗时。
    pub parseLog: Duration,
    /// 解析并发度（合并时取 max）。
    pub concurrent: usize,
}

impl slowQueryRuntimeStats {
    /// 格式化为可读统计字符串。
    pub fn String(&self) -> String {
        format!(
            "initialize: {:?}, read_file: {:?}, parse_log: {{time:{:?}, concurrency:{}}}, total_file: {}, read_file: {}, read_size: {}",
            self.initialize,
            self.readFile,
            self.parseLog,
            self.concurrent,
            self.totalFileNum,
            self.readFileNum,
            astersql_util_memory::tracker::FormatBytes(self.readFileSize)
        )
    }

    /// 合并另一份统计：计数/耗时累加，并发取较大值。
    pub fn Merge(&mut self, other: &Self) {
        self.totalFileNum += other.totalFileNum;
        self.readFileNum += other.readFileNum;
        self.readFile += other.readFile;
        self.initialize += other.initialize;
        self.readFileSize += other.readFileSize;
        self.parseLog += other.parseLog;
        self.concurrent = self.concurrent.max(other.concurrent);
    }

    /// 克隆统计快照。
    pub fn Clone(&self) -> Self {
        std::clone::Clone::clone(self)
    }

    /// 统计类型标识（对应 Go Tp）。
    pub fn Tp(&self) -> i32 {
        1
    }
}

/// 通过 Runtime 从文件尾部读取末尾行。
pub fn readLastLines<R: SlowQueryRuntime>(
    runtime: &mut R,
    context: &R::Context,
    file: &R::File,
    end_cursor: i64,
) -> Result<(Vec<String>, usize), R::Error> {
    runtime.read_last_lines(context, file, end_cursor, maxReadCacheSize)
}

/// Reads the slow-log block ending at `end_cursor` from a real file.
///
/// A cloned file descriptor preserves the caller's cursor, matching the
/// positional read semantics used by the Go retriever. At most `max_cache`
/// bytes are materialized, including an unterminated final line.
///
/// 从真实文件按 end_cursor 向前读取至多 max_cache 字节并按行切分；
/// clone 文件描述符以保留调用方游标，语义对齐 Go 位置读。
pub fn ReadLastLinesFromFile(
    file: &mut File,
    end_cursor: i64,
    max_cache: usize,
) -> std::io::Result<(Vec<String>, usize)> {
    let requested_end = u64::try_from(end_cursor).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "slow-log end cursor must not be negative",
        )
    })?;
    let end = requested_end.min(file.metadata()?.len());
    let read_bytes = usize::try_from(end.min(max_cache as u64)).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "slow-log read range does not fit in memory",
        )
    })?;
    if read_bytes == 0 {
        return Ok((Vec::new(), 0));
    }

    // 从 end 向前读 read_bytes，再按行切分。
    let start = end - read_bytes as u64;
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(start))?;
    let mut buffer = vec![0; read_bytes];
    reader.read_exact(&mut buffer)?;
    let lines = splitSlowLogLines(&String::from_utf8_lossy(&buffer));
    Ok((lines, read_bytes))
}

/// 规范化 CRLF 后按 `\n` 切分行，丢弃末尾空行。
pub fn splitSlowLogLines(value: &str) -> Vec<String> {
    let normalized = value.replace("\r\n", "\n");
    let mut lines = normalized
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// 估算日志行文本总字节数。
pub fn calculateLogSize(log: &[String]) -> i64 {
    log.iter()
        .map(|line| line.len() as i64)
        .fold(0, i64::saturating_add)
}

/// 估算结果行集合的 Datum 总内存。
pub fn calculateDatumsSize<R: SlowQueryRuntime>(runtime: &R, rows: &[R::Row]) -> i64 {
    rows.iter()
        .map(|row| runtime.estimated_row_size(row))
        .fold(0, i64::saturating_add)
}

/// Dashboard 测试用：记录慢日志读块次数。
static DashboardSlowLogReadBlockCnt4Test: AtomicI64 = AtomicI64::new(0);

/// 读取 Dashboard 测试计数器。
pub fn dashboardSlowLogReadBlockCount() -> i64 {
    DashboardSlowLogReadBlockCnt4Test.load(Ordering::Acquire)
}

impl fmt::Display for slowQueryRuntimeStats {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}
