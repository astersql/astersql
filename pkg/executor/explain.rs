// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// EXPLAIN / EXPLAIN ANALYZE 执行器。
//
// EXPLAIN 展示优化器生成的物理执行计划（算子树、估计行数、任务类型等）；
// EXPLAIN ANALYZE 还会实际跑一遍计划，把运行时统计写回计划树。
// EXPLAIN FOR CONNECTION 则对指定会话连接上正在执行的语句做计划展示。
//
// 主要内容：
// - [`ExplainExec`]：惰性渲染计划行，并驱动 ANALYZE 子执行器；
// - [`ExplainForConnection`]：按权限解析目标连接的计划；
// - [`MemoryDebugModeHandler`]：ANALYZE 期间可选的堆内存诊断循环。

use std::any::Any;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::foreign_key::WithForeignKeyTrigger;
use astersql_errors as errors;
use astersql_util_chunk as chunk;
use astersql_util_execdetails as execdetails;

/// 1 GiB，用于按堆占用调节内存诊断采样间隔。
const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone)]
/// 透传给 ANALYZE 子执行器与运行时适配的不透明上下文。
pub struct ExplainContext(pub Arc<dyn Any + Send + Sync>);

impl Default for ExplainContext {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

/// 计划侧接口：是否 ANALYZE、目标 plan id、渲染与取出结果行。
pub trait ExplainPlan {
    fn Analyze(&self) -> bool;
    fn TargetPlanID(&self) -> Option<i32>;
    fn RenderResult(&mut self) -> Result<(), errors::SharedError>;
    fn Rows(&self) -> Vec<Vec<String>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 目标连接上的进程快照：连接 id、属主用户与当前 SQL。
pub struct ExplainForProcess {
    pub connection_id: u64,
    pub user: String,
    pub sql: String,
}

/// 提供进程查找与计划渲染，供 EXPLAIN FOR CONNECTION 使用。
pub trait ExplainForConnectionProvider {
    fn GetProcess(&self, connection_id: u64) -> Option<ExplainForProcess>;
    fn RenderProcessPlan(
        &self,
        process: &ExplainForProcess,
        format: &str,
    ) -> Result<Vec<Vec<String>>, errors::SharedError>;
}

/// Resolves EXPLAIN FOR CONNECTION against a live process snapshot. As in Go,
/// owners may inspect their own connection while a privileged caller may
/// inspect any connection; plan rendering happens only after that check.
/// 解析 EXPLAIN FOR CONNECTION：非特权用户只能看自己的连接；
/// 特权用户可看任意连接。权限通过后再渲染计划。
pub fn ExplainForConnection<P: ExplainForConnectionProvider>(
    provider: &P,
    connection_id: u64,
    requesting_user: &str,
    privileged: bool,
    format: &str,
) -> Result<Vec<Vec<String>>, errors::SharedError> {
    let process = provider
        .GetProcess(connection_id)
        .ok_or_else(|| errors::New(format!("Unknown thread id: {connection_id}")))?;
    if !privileged && process.user != requesting_user {
        return Err(errors::New(format!(
            "Access denied to process {} owned by {}",
            process.connection_id, process.user
        )));
    }
    provider.RenderProcessPlan(&process, format)
}

/// Adapter for the executor consumed by EXPLAIN ANALYZE. Every operation is
/// required so adapters cannot accidentally skip Open/Next/Close behavior.
/// EXPLAIN ANALYZE 子执行器适配：Open/Next/Close 等必须全部实现。
pub trait ExplainAnalyzeExecutor: Send {
    fn Open(&mut self, ctx: ExplainContext) -> Result<(), errors::SharedError>;
    fn Next(
        &mut self,
        ctx: ExplainContext,
        chunk: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError>;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
    fn NewCacheChunk(&mut self) -> Box<chunk::Chunk>;
    fn SchemaLen(&mut self) -> usize;
    fn ForeignKeyTrigger(&mut self) -> Option<&mut dyn WithForeignKeyTrigger>;
}

/// Runtime integration needed by EXPLAIN. BuildRURuntimeStats must first drain
/// pending raw RU v2 counters and then clone both RU details and metrics.
/// 运行时集成：先排空 pending RU v2 计数，再克隆 RU 明细与 metrics。
pub trait ExplainRuntime {
    fn MaxChunkSize(&self) -> usize;
    fn MemoryDebugHandler(&self, ctx: ExplainContext) -> Option<MemoryDebugModeHandler>;
    fn BuildRURuntimeStats(
        &mut self,
        ctx: &ExplainContext,
    ) -> Option<execdetails::execdetails::RURuntimeStats>;
    fn RegisterRURuntimeStats(
        &mut self,
        target_plan_id: i32,
        stats: execdetails::execdetails::RURuntimeStats,
    );
}

#[derive(Clone, Copy)]
/// 内存诊断日志级别。
pub enum MemoryDebugLogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone)]
/// 结构化日志字段（key/value）。
pub struct MemoryDebugField {
    pub key: String,
    pub value: String,
}

/// Production diagnostics boundary for heap sampling, tracker inspection,
/// profile generation, and structured logging.
/// 堆采样、tracker 检查、profile 生成与结构化日志的生产诊断边界。
pub trait MemoryDebugDiagnostics: Send + Sync {
    fn ForceReadMemoryUsage(&self, run_gc: bool) -> (u64, u64);
    fn FormatBytes(&self, bytes: i64) -> String;
    fn TrackerTreeMemory(&self) -> BTreeMap<String, i64>;
    fn TrackersAbove(&self, bytes: i64) -> Vec<(i64, i64)>;
    fn TempStoragePath(&self) -> PathBuf;
    fn NowRFC3339(&self) -> String;
    fn WriteHeapProfile(&self, file: &mut File) -> std::io::Result<()>;
    fn Log(&self, level: MemoryDebugLogLevel, message: &str, fields: &[MemoryDebugField]);
}

#[derive(Default)]
/// 控制内存诊断后台循环的停止与等待。
pub struct MemoryDebugControl {
    stopped: AtomicBool,
    wait_lock: Mutex<()>,
    wake: Condvar,
}

impl MemoryDebugControl {
    /// 标记停止并唤醒等待中的诊断循环。
    pub fn Stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    /// 等待至多 duration；若已 Stop 则立即返回 true。
    fn Wait(&self, duration: Duration) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return true;
        }
        let guard = self.wait_lock.lock().expect("memory debug lock poisoned");
        let _ = self
            .wake
            .wait_timeout_while(guard, duration, |_| !self.stopped.load(Ordering::Acquire))
            .expect("memory debug lock poisoned");
        self.stopped.load(Ordering::Acquire)
    }
}

// ExplainExec：EXPLAIN / EXPLAIN ANALYZE 的执行器外壳。
// ExplainExec represents an explain executor.
pub struct ExplainExec {
    pub runtime: Box<dyn ExplainRuntime>,
    pub explain: Box<dyn ExplainPlan>,
    pub analyzeExec: Option<Box<dyn ExplainAnalyzeExecutor>>,
    pub executed: bool,
    pub rows: Option<Vec<Vec<String>>>,
    pub cursor: usize,
}

impl ExplainExec {
    // Open：ANALYZE 时转发到子执行器 Open。
    // Open implements the Executor Open interface.
    pub fn Open(&mut self, ctx: ExplainContext) -> Result<(), errors::SharedError> {
        if self.explain.Analyze()
            && let Some(analyze_executor) = self.analyzeExec.as_mut()
        {
            return analyze_executor.Open(ctx);
        }
        Ok(())
    }

    // Close：若 Open 过但未执行到 Next，仍需恰好 Close 一次 analyze 子执行器。
    // Close implements the Executor Close interface. If Open ran but Next did
    // not, the analyze executor still has to be closed exactly once.
    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        self.rows = None;
        if self.explain.Analyze()
            && !self.executed
            && let Some(analyze_executor) = self.analyzeExec.as_mut()
        {
            return analyze_executor.Close();
        }
        Ok(())
    }

    // Next：惰性生成全部 explain 行，再按 chunk 容量游标输出。
    // Next lazily renders all rows, then returns as many as the requested chunk
    // can hold, matching the Go cursor and capacity behavior.
    pub fn Next(
        &mut self,
        ctx: ExplainContext,
        request: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        if self.rows.is_none() {
            self.rows = Some(self.generateExplainInfo(ctx)?);
        }
        request.GrowAndReset(self.runtime.MaxChunkSize());
        let rows = self.rows.as_ref().expect("explain rows must be generated");
        if self.cursor >= rows.len() {
            return Ok(());
        }
        let current_rows = request.Capacity().min(rows.len() - self.cursor);
        for row in &rows[self.cursor..self.cursor + current_rows] {
            for (column_index, value) in row.iter().enumerate() {
                request.AppendString(column_index, value);
            }
        }
        self.cursor += current_rows;
        Ok(())
    }

    /// 实际跑 ANALYZE 子执行器；可选启动内存诊断线程，并在结束后登记 RU 统计。
    pub fn executeAnalyzeExec(&mut self, ctx: ExplainContext) -> Result<(), errors::SharedError> {
        let should_execute = self.explain.Analyze() && self.analyzeExec.is_some() && !self.executed;
        if should_execute {
            let debug_worker = self.runtime.MemoryDebugHandler(ctx.clone()).map(|handler| {
                let control = handler.control.clone();
                let join = thread::spawn(move || handler.run());
                (control, join)
            });

            self.executed = true;
            let analyze_executor = self
                .analyzeExec
                .as_mut()
                .expect("checked analyze executor presence");
            let mut cache = analyze_executor.NewCacheChunk();
            let mut execution_error = None;
            loop {
                cache.Reset();
                let next_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    analyze_executor.Next(ctx.clone(), &mut cache)
                }));
                match next_result {
                    Ok(Ok(())) if cache.NumRows() != 0 => {}
                    Ok(Ok(())) => break,
                    Ok(Err(error)) => {
                        execution_error = Some(error);
                        break;
                    }
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<&str>()
                            .map(|message| (*message).to_owned())
                            .or_else(|| payload.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".to_owned());
                        execution_error = Some(errors::New(message));
                        break;
                    }
                }
            }

            if let Some((control, join)) = debug_worker {
                control.Stop();
                let _ = join.join();
            }

            // 无论 Next 成功与否都尝试 Close；组合错误时 Next 错误优先。
            // Close is attempted after every Next outcome. A Next failure stays
            // first in the combined error, followed by the Close failure.
            let close_error = analyze_executor.Close().err();
            match (execution_error, close_error) {
                (Some(execution), Some(close)) => {
                    return Err(errors::New(format!("{execution}, {close}")));
                }
                (Some(execution), None) => return Err(execution),
                (None, Some(close)) => return Err(close),
                (None, None) => {}
            }
        }

        // 子执行器结束并 Close 后再登记 RU；适配器会先排空 pending 计数。
        // Register after the analyze executor has finished and closed. The
        // runtime adapter drains pending RU counters before returning a snapshot.
        if self.explain.Analyze()
            && self.analyzeExec.is_some()
            && self.executed
            && let Some(target_plan_id) = self.explain.TargetPlanID()
            && let Some(stats) = self.runtime.BuildRURuntimeStats(&ctx)
        {
            self.runtime.RegisterRURuntimeStats(target_plan_id, stats);
        }
        Ok(())
    }

    /// 必要时先跑 ANALYZE，再 RenderResult 并返回结果行。
    pub fn generateExplainInfo(
        &mut self,
        ctx: ExplainContext,
    ) -> Result<Vec<Vec<String>>, errors::SharedError> {
        if self.explain.Analyze() {
            self.executeAnalyzeExec(ctx)?;
        }
        // RenderResult 负责所有输出格式（含 TiDB_JSON）；ANALYZE 之后调用以保证运行时统计可见。
        // RenderResult owns every output format, including TiDB_JSON. Calling
        // it after analyze guarantees runtime stats are visible to rendering.
        self.explain.RenderResult()?;
        Ok(self.explain.Rows())
    }

    /// Schema 为空的 ANALYZE 子执行器可立即取出执行（无延迟批处理）。
    pub fn getAnalyzeExecToExecutedNoDelay(
        &mut self,
    ) -> Option<&mut (dyn ExplainAnalyzeExecutor + 'static)> {
        let should_take = self.explain.Analyze()
            && self.analyzeExec.is_some()
            && !self.executed
            && self
                .analyzeExec
                .as_mut()
                .is_some_and(|executor| executor.SchemaLen() == 0);
        if should_take {
            self.executed = true;
            return self.analyzeExec.as_deref_mut();
        }
        None
    }

    /// 若 ANALYZE 子执行器带外键触发器，返回其可变引用。
    pub fn getAnalyzeExecWithForeignKeyTrigger(
        &mut self,
    ) -> Option<&mut (dyn WithForeignKeyTrigger + '_)> {
        if !self.explain.Analyze() {
            return None;
        }
        self.analyzeExec
            .as_mut()
            .and_then(|executor| executor.ForeignKeyTrigger())
    }
}

/// ANALYZE 期间周期性采样堆占用并在超阈值时告警的后台处理器。
pub struct MemoryDebugModeHandler {
    pub minHeapInUse: i64,
    pub alarmRatio: i64,
    pub autoGC: bool,
    pub diagnostics: Arc<dyn MemoryDebugDiagnostics>,
    pub control: Arc<MemoryDebugControl>,
    pub infoField: Vec<MemoryDebugField>,
}

impl MemoryDebugModeHandler {
    /// 强制读取当前堆占用与 tracker 记账（可选先 GC）。
    pub fn fetchCurrentMemoryUsage(&self, run_gc: bool) -> (u64, u64) {
        self.diagnostics.ForceReadMemoryUsage(run_gc)
    }

    /// 组装诊断字段；need_profile 时写入堆 profile 路径。
    pub fn genInfo(
        &mut self,
        status: &str,
        need_profile: bool,
        heap_in_use: i64,
        tracked_memory: i64,
    ) -> Result<Vec<MemoryDebugField>, errors::SharedError> {
        self.infoField.clear();
        self.infoField.push(MemoryDebugField {
            key: "sql".to_owned(),
            value: status.to_owned(),
        });
        self.infoField.push(MemoryDebugField {
            key: "heap in use".to_owned(),
            value: self.diagnostics.FormatBytes(heap_in_use),
        });
        self.infoField.push(MemoryDebugField {
            key: "tracked memory".to_owned(),
            value: self.diagnostics.FormatBytes(tracked_memory),
        });
        if need_profile {
            self.infoField.push(MemoryDebugField {
                key: "heap profile".to_owned(),
                value: getHeapProfile(self.diagnostics.as_ref())?,
            });
        }
        Ok(self.infoField.clone())
    }

    /// 把 tracker 树内存用量格式化为日志字段。
    pub fn getTrackerTreeMemUseLogs(&self) -> Vec<MemoryDebugField> {
        self.diagnostics
            .TrackerTreeMemory()
            .into_iter()
            .map(|(key, bytes)| MemoryDebugField {
                key: format!("TrackerTree {key}"),
                value: self.diagnostics.FormatBytes(bytes),
            })
            .collect()
    }

    /// 诊断主循环：按堆占用调节采样间隔，超阈值时打 warning 与 tracker 树。
    pub fn run(mut self) {
        self.diagnostics.Log(
            MemoryDebugLogLevel::Info,
            "Memory Debug Mode",
            &[
                MemoryDebugField {
                    key: "sql".to_owned(),
                    value: "started".to_owned(),
                },
                MemoryDebugField {
                    key: "autoGC".to_owned(),
                    value: self.autoGC.to_string(),
                },
                MemoryDebugField {
                    key: "minHeapInUse".to_owned(),
                    value: self.diagnostics.FormatBytes(self.minHeapInUse),
                },
                MemoryDebugField {
                    key: "alarmRatio".to_owned(),
                    value: self.alarmRatio.to_string(),
                },
            ],
        );

        let mut trigger_interval = Duration::from_secs(5);
        let mut print_mod = 6;
        let mut loop_count = 0;
        let mut loop_error = None;
        while !self.control.Wait(trigger_interval) {
            let (heap_in_use, tracked_memory) = self.fetchCurrentMemoryUsage(self.autoGC);
            loop_count += 1;
            if loop_count % print_mod == 0 {
                match self.genInfo("running", false, heap_in_use as i64, tracked_memory as i64) {
                    Ok(fields) => self.diagnostics.Log(
                        MemoryDebugLogLevel::Info,
                        "Memory Debug Mode",
                        &fields,
                    ),
                    Err(error) => {
                        loop_error = Some(error);
                        break;
                    }
                }
            }
            // 堆越大采样越稀；同时用 alarmRatio 判断 tracker 是否远低于真实堆占用。
            (trigger_interval, print_mod) = updateTriggerIntervalByHeapInUse(heap_in_use);

            let alarm_multiplier = (100_i64 + self.alarmRatio).max(0) as u64;
            let above_threshold = heap_in_use > self.minHeapInUse.max(0) as u64
                && tracked_memory
                    .saturating_div(100)
                    .saturating_mul(alarm_multiplier)
                    < heap_in_use;
            if above_threshold {
                match self.genInfo("warning", true, heap_in_use as i64, tracked_memory as i64) {
                    Ok(fields) => self.diagnostics.Log(
                        MemoryDebugLogLevel::Warn,
                        "Memory Debug Mode",
                        &fields,
                    ),
                    Err(error) => {
                        loop_error = Some(error);
                        break;
                    }
                }
                if self.autoGC {
                    let executor_fields = self
                        .diagnostics
                        .TrackersAbove(self.minHeapInUse / 5)
                        .into_iter()
                        .map(|(label, bytes)| MemoryDebugField {
                            key: format!("Executor_{label}"),
                            value: self.diagnostics.FormatBytes(bytes),
                        })
                        .collect::<Vec<_>>();
                    self.diagnostics.Log(
                        MemoryDebugLogLevel::Warn,
                        "Memory Debug Mode, Log all executors that consumes more than threshold * 20%",
                        &executor_fields,
                    );
                    self.diagnostics.Log(
                        MemoryDebugLogLevel::Warn,
                        "Memory Debug Mode, Log the tracker tree",
                        &self.getTrackerTreeMemUseLogs(),
                    );
                }
            }
        }

        let (heap_in_use, tracked_memory) = self.fetchCurrentMemoryUsage(true);
        if loop_error.is_none() {
            match self.genInfo("finished", true, heap_in_use as i64, tracked_memory as i64) {
                Ok(fields) => {
                    self.diagnostics
                        .Log(MemoryDebugLogLevel::Info, "Memory Debug Mode", &fields)
                }
                Err(error) => loop_error = Some(error),
            }
        } else if let Ok(fields) = self.genInfo(
            "debug_mode_error",
            false,
            heap_in_use as i64,
            tracked_memory as i64,
        ) {
            self.diagnostics
                .Log(MemoryDebugLogLevel::Error, "Memory Debug Mode", &fields);
        }
        if let Some(error) = loop_error {
            self.diagnostics.Log(
                MemoryDebugLogLevel::Error,
                "Memory Debug Mode Exit",
                &[MemoryDebugField {
                    key: "error".to_owned(),
                    value: error.to_string(),
                }],
            );
        }
    }
}

/// 按堆占用选择采样间隔与日志打印模数（堆越高间隔越长）。
pub fn updateTriggerIntervalByHeapInUse(heap_in_use: u64) -> (Duration, i32) {
    if heap_in_use < 30 * GIB {
        (Duration::from_secs(5), 6)
    } else if heap_in_use < 40 * GIB {
        (Duration::from_secs(15), 2)
    } else {
        (Duration::from_secs(30), 1)
    }
}

/// 在临时目录写入 heap profile 文件并返回路径。
pub fn getHeapProfile(
    diagnostics: &dyn MemoryDebugDiagnostics,
) -> Result<String, errors::SharedError> {
    let directory = diagnostics.TempStoragePath().join("record");
    let file_name = directory.join(format!("heapGC{}", diagnostics.NowRFC3339()));
    let mut file = File::create(&file_name).map_err(|error| errors::New(error.to_string()))?;
    diagnostics
        .WriteHeapProfile(&mut file)
        .map_err(|error| errors::New(error.to_string()))?;
    file.sync_all()
        .map_err(|error| errors::New(error.to_string()))?;
    Ok(file_name.to_string_lossy().into_owned())
}
