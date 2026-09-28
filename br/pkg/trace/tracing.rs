// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! BR tracing helpers ported from `br/pkg/trace/tracing.go`.
//!
//! Local MemoryStore / Tracer / Span stubs mirror the appdash + opentracing
//! subset BR actually uses, without pulling those Go dependencies.
//!
//! 自 Go `br/pkg/trace/tracing.go` 移植的 BR 分布式追踪辅助模块。
//!
//! 本地 `MemoryStore` / `Tracer` / `Span` 桩复刻 appdash + opentracing 中
//! BR 实际用到的子集，避免引入完整 Go 依赖；**并非**生产级 tracing 后端。

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// 追踪输出文件名回调：与 Go `getTraceFileName` 包级变量对应。
type TraceFileNameFn = Arc<dyn Fn() -> String + Send + Sync>;

// Go `getTraceFileName` is a process-global package var; Go tests are serial.
// Rust libtests run in parallel, so the test override is thread-local: each
// test thread sees only its own replacement, matching Go serial semantics.
// Go 侧为进程级包变量且测试串行；Rust libtest 并行，故用 thread_local 隔离覆盖，
// 使各测试线程仅见自身替换，语义对齐 Go 非 Parallel 场景。
thread_local! {
    static TRACE_FILE_NAME_OVERRIDE: RefCell<Option<TraceFileNameFn>> = const { RefCell::new(None) };
}

// 调用当前线程的 trace 文件名生成逻辑：测试覆盖优先，否则走默认时间戳命名。
fn call_get_trace_file_name() -> String {
    TRACE_FILE_NAME_OVERRIDE.with(|slot| match slot.borrow().as_ref() {
        Some(f) => f(),
        None => timestampTraceFileName(),
    })
}

/// Replace the Go `getTraceFileName` package variable (tests / callers).
///
/// Override is thread-local so parallel Rust tests do not race (Go package
/// tests do not call `t.Parallel()` around this var).
/// 测试/调用方替换 Go `getTraceFileName`；thread-local 避免并行 libtest 竞态。
pub fn set_get_trace_file_name_for_test(f: Option<TraceFileNameFn>) {
    TRACE_FILE_NAME_OVERRIDE.with(|slot| {
        *slot.borrow_mut() = f;
    });
}

/// timestampTraceFileName mirrors Go: temp dir + `br.trace.<timestamp>`.
/// 默认 trace 文件路径：系统临时目录 + `br.trace.<UTC 时间戳>`。
pub fn timestampTraceFileName() -> String {
    let now = SystemTime::now();
    let path = std::env::temp_dir().join(format!("br.trace.{}", format_go_trace_stamp(now)));
    // 转为 lossy 字符串路径（非 UTF-8 字节保留）。
    path.to_string_lossy().into_owned()
}

// 将 SystemTime 格式化为 Go trace 文件名中的时间戳片段。
fn format_go_trace_stamp(now: SystemTime) -> String {
    format_go_trace_stamp_with_offset(now, local_utc_offset_seconds(now))
}

#[cfg(test)]
pub(crate) fn format_go_trace_stamp_with_offset_for_test(
    now: SystemTime,
    offset_seconds: i32,
) -> String {
    format_go_trace_stamp_with_offset(now, offset_seconds)
}

fn format_go_trace_stamp_with_offset(now: SystemTime, offset_seconds: i32) -> String {
    let unix_seconds = match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let local_seconds = unix_seconds.saturating_add(i64::from(offset_seconds));
    let local_stamp = format_utc_stamp(local_seconds);
    if offset_seconds == 0 {
        return local_stamp;
    }

    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let offset_minutes = offset_seconds.unsigned_abs() / 60;
    format!(
        "{}{sign}{:02}{:02}",
        local_stamp.trim_end_matches('Z'),
        offset_minutes / 60,
        offset_minutes % 60
    )
}

#[cfg(unix)]
fn local_utc_offset_seconds(now: SystemTime) -> i32 {
    use std::ffi::{c_char, c_int, c_long};

    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    unsafe extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }

    let unix_seconds = match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let mut local_tm = std::mem::MaybeUninit::<Tm>::uninit();
    // SAFETY: both pointers remain valid for the call and localtime_r initializes
    // the caller-owned Tm on success.
    let result = unsafe { localtime_r(&unix_seconds, local_tm.as_mut_ptr()) };
    if result.is_null() {
        return 0;
    }
    // SAFETY: a non-null localtime_r result means local_tm was initialized.
    let offset = unsafe { local_tm.assume_init() }.tm_gmtoff;
    i32::try_from(offset).unwrap_or(0)
}

#[cfg(not(unix))]
fn local_utc_offset_seconds(_now: SystemTime) -> i32 {
    0
}

// 由 Unix 秒数格式化为 `YYYY-MM-DDTHH.MM.SSZ` 字符串。
fn format_utc_stamp(secs: i64) -> String {
    const DAY: i64 = 86400;
    const HOUR: i64 = 3600;
    const MIN: i64 = 60;
    // 整除得天序号与日内秒数。
    let days = secs.div_euclid(DAY);
    let day_secs = secs.rem_euclid(DAY);
    let (y, m, d) = civil_from_days(days);
    // 日内时分秒。
    let hh = day_secs / HOUR;
    let mm = (day_secs % HOUR) / MIN;
    let ss = day_secs % MIN;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}.{mm:02}.{ss:02}Z")
}

// Howard Hinnant civil_from_days：自 1970-01-01 起算天数转 (年, 月, 日)。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    // Howard Hinnant civil_from_days (UTC).
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Lightweight context carrying the active OpenTracing-style span.
/// 轻量上下文，携带当前 OpenTracing 风格 span（与 Go context 中 span 对应）。
#[derive(Clone, Default)]
pub struct Context {
    // 当前活跃 span；None 表示无追踪上下文。
    span: Option<Arc<Span>>,
}

impl Context {
    // 空上下文，对齐 Go `context.Background()`。
    pub fn Background() -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, Default)]
// 查询 trace 时的选项占位；当前桩实现忽略具体字段。
pub struct TracesOpts {}

#[derive(Clone, Debug)]
// 单个 span 的记录：名称与起止时刻。
pub struct SpanRec {
    pub name: String,
    pub start: SystemTime,
    pub end: SystemTime,
}

#[derive(Clone, Debug)]
// 树形 trace 节点：根/子 span 及递归子树（字段名与 Go 导出一致）。
pub struct Trace {
    pub Span: SpanRec,
    pub Sub: Vec<Trace>,
}

impl Trace {
    // 提取 span 起止时间，供 dfsTree 与排序使用。
    pub fn TimespanEvent(&self) -> Result<TimespanEvent, String> {
        Ok(TimespanEvent {
            start: self.Span.start,
            end: self.Span.end,
        })
    }
}

#[derive(Clone, Debug)]
// 时间区间事件：封装 span 的 start/end。
pub struct TimespanEvent {
    start: SystemTime,
    end: SystemTime,
}

impl TimespanEvent {
    // span 开始时刻。
    pub fn Start(&self) -> SystemTime {
        self.start
    }
    // span 结束时刻。
    pub fn End(&self) -> SystemTime {
        self.end
    }
}

/// Queryer mirrors `appdash.Queryer`.
/// 查询已完成 trace 的接口，对齐 `appdash.Queryer`。
pub trait Queryer {
    fn Traces(&self, opts: TracesOpts) -> Result<Vec<Trace>, String>;
}

#[derive(Debug)]
// 内存中已收集的单条 span：含 id、父 id 与时间戳。
struct CollectedSpan {
    id: u64,
    parent_id: Option<u64>,
    name: String,
    start: SystemTime,
    end: SystemTime,
}

/// MemoryStore mirrors `appdash.MemoryStore` for finished spans only.
/// 内存 span 存储桩，仅保留 Finish 后的 span；非持久化 appdash 后端。
#[derive(Debug, Default)]
pub struct MemoryStore {
    // 已完成 span 向量，Mutex 保证并发 Finish 安全。
    spans: Mutex<Vec<CollectedSpan>>,
}

impl MemoryStore {
    // 构造共享所有权 MemoryStore，供 Tracer 与 Queryer 共用。
    pub fn NewMemoryStore() -> Arc<Self> {
        Arc::new(Self {
            spans: Mutex::new(Vec::new()),
        })
    }

    // Span.Finish 写入一条 CollectedSpan。
    fn collect(&self, span: CollectedSpan) {
        let mut guard = self.spans.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(span);
    }

    // 将扁平 span 列表按 parent_id 重建为多棵 Trace 森林。
    fn build_traces(&self) -> Vec<Trace> {
        let guard = self.spans.lock().unwrap_or_else(|e| e.into_inner());
        let mut by_parent: std::collections::HashMap<Option<u64>, Vec<usize>> =
            std::collections::HashMap::new();
        let id_set: std::collections::HashSet<u64> = guard.iter().map(|s| s.id).collect();
        // 按 parent_id 分组；无效父 id 提升为根。
        for (idx, sp) in guard.iter().enumerate() {
            let parent_key = match sp.parent_id {
                Some(pid) if id_set.contains(&pid) => Some(pid),
                // Parent missing (e.g. unfinished root) → treat as forest root.
                // 父 span 缺失或未 Finish 时，将该节点视为森林根。
                _ => None,
            };
            by_parent.entry(parent_key).or_default().push(idx);
        }
        let roots = by_parent.get(&None).cloned().unwrap_or_default();
        // 自各根节点递归 build_tree，得到完整 Trace 树。
        roots
            .into_iter()
            .map(|idx| build_tree(&guard, &by_parent, idx))
            .collect()
    }
}

// 自 spans[idx] 递归构造 Trace 子树。
fn build_tree(
    spans: &[CollectedSpan],
    by_parent: &std::collections::HashMap<Option<u64>, Vec<usize>>,
    idx: usize,
) -> Trace {
    let sp = &spans[idx];
    // 查找当前 span 的全部直接子节点索引。
    let children = by_parent
        .get(&Some(sp.id))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|cidx| build_tree(spans, by_parent, cidx))
        .collect();
    // 组装 Trace 节点：SpanRec + 递归 Sub。
    Trace {
        Span: SpanRec {
            name: sp.name.clone(),
            start: sp.start,
            end: sp.end,
        },
        Sub: children,
    }
}

impl Queryer for MemoryStore {
    // 忽略 opts，从内存 store 重建 trace 森林。
    fn Traces(&self, _opts: TracesOpts) -> Result<Vec<Trace>, String> {
        Ok(self.build_traces())
    }
}

impl Queryer for Arc<MemoryStore> {
    // Arc 包装转发，便于与 Tracer 共享同一 store。
    fn Traces(&self, opts: TracesOpts) -> Result<Vec<Trace>, String> {
        (**self).Traces(opts)
    }
}

/// Tracer mirrors `appdash/opentracing.NewTracer(store)`.
/// 追踪器桩：分配 span id 并将 Finish 的 span 写入 MemoryStore。
#[derive(Debug)]
pub struct Tracer {
    store: Arc<MemoryStore>,
    // 单调递增 span id 计数器。
    next_id: AtomicU64,
}

impl Tracer {
    // 绑定 MemoryStore 创建 Tracer。
    pub fn NewTracer(store: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            store,
            next_id: AtomicU64::new(1),
        })
    }

    // 创建根 span（无 parent）。
    pub fn StartSpan(self: &Arc<Self>, name: &str) -> Arc<Span> {
        self.start_span_inner(name, None)
    }

    // 创建子 span，parent 为给定 Span。
    pub fn StartSpanChildOf(self: &Arc<Self>, name: &str, parent: &Span) -> Arc<Span> {
        self.start_span_inner(name, Some(parent.id))
    }

    // 内部分配 id、记录 start 时刻，Finish 前 span 仅存于内存。
    fn start_span_inner(self: &Arc<Self>, name: &str, parent_id: Option<u64>) -> Arc<Span> {
        let id = self.next_id.fetch_add(1, AtomicOrdering::Relaxed);
        // 新 span 尚未 Finish，不会进入 store。
        Arc::new(Span {
            tracer: Arc::clone(self),
            id,
            parent_id,
            name: name.to_string(),
            start: SystemTime::now(),
            finished: AtomicBool::new(false),
        })
    }
}

/// Span mirrors the opentracing span BR uses (name + child-of + finish).
/// OpenTracing span 桩：名称、父子关系与 Finish 收集。
#[derive(Debug)]
pub struct Span {
    tracer: Arc<Tracer>,
    id: u64,
    parent_id: Option<u64>,
    name: String,
    start: SystemTime,
    // 是否已 Finish；重复 Finish 被忽略。
    finished: AtomicBool,
}

impl Span {
    // 返回创建该 span 的 Tracer 引用。
    pub fn Tracer(&self) -> Arc<Tracer> {
        Arc::clone(&self.tracer)
    }

    // 幂等 Finish：CAS 保证仅首次写入 store。
    pub fn Finish(&self) {
        if self
            .finished
            .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
            .is_err()
        {
            // 已 Finish 的 span 直接返回。
            return;
        }
        // 写入 MemoryStore 供后续 Queryer 重建树。
        self.tracer.store.collect(CollectedSpan {
            id: self.id,
            parent_id: self.parent_id,
            name: self.name.clone(),
            start: self.start,
            end: SystemTime::now(),
        });
    }
}

// 将 span 注入 Context，对齐 Go `opentracing.ContextWithSpan`。
pub fn ContextWithSpan(mut ctx: Context, span: Arc<Span>) -> Context {
    ctx.span = Some(span);
    ctx
}

// 从 Context 取出当前 span，无则 None。
pub fn SpanFromContext(ctx: &Context) -> Option<Arc<Span>> {
    ctx.span.clone()
}

/// Tabby-like table writer with Go `tabwriter` padding=2 semantics.
/// 三列表格写入器，列宽对齐 Go `tabwriter` padding=2 语义。
pub struct Tabby {
    // 待输出的三列行缓冲。
    rows: Vec<[String; 3]>,
    out: Box<dyn Write + Send>,
}

impl Tabby {
    // 指定输出目标（通常为 trace 文件）。
    pub fn NewCustom(out: Box<dyn Write + Send>) -> Self {
        Self {
            rows: Vec::new(),
            out,
        }
    }

    // 追加一行三列文本，Print 时按列宽补空格。
    pub fn AddLine(&mut self, c0: impl Into<String>, c1: impl Into<String>, c2: impl Into<String>) {
        self.rows.push([c0.into(), c1.into(), c2.into()]);
    }

    // 计算各列最大宽度后逐行写出并 flush。
    pub fn Print(&mut self) {
        let mut widths = [0usize; 3];
        // 第一遍扫描求各列最大宽度。
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                // Go text/tabwriter measures UTF-8 cells in runes, not bytes.
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
        let padding = 2usize;
        // 每行按列宽 + padding 补空格对齐。
        for row in &self.rows {
            let mut line = String::new();
            for (i, cell) in row.iter().enumerate() {
                line.push_str(cell);
                if i + 1 < row.len() {
                    let pad = widths[i].saturating_sub(cell.chars().count()) + padding;
                    line.push_str(&" ".repeat(pad));
                }
            }
            line.push('\n');
            let _ = self.out.write_all(line.as_bytes());
        }
        let _ = self.out.flush();
    }
}

/// TracerStartSpan starts the tracer for BR.
/// BR 入口：创建 MemoryStore、根 span "trace" 并写入 Context。
pub fn TracerStartSpan(ctx: Context) -> (Context, Arc<MemoryStore>) {
    let store = MemoryStore::NewMemoryStore();
    let tracer = Tracer::NewTracer(Arc::clone(&store));
    let span = tracer.StartSpan("trace");
    let ctx = ContextWithSpan(ctx, span);
    (ctx, store)
}

/// TracerFinishSpan finishes the tracer for BR and writes the first trace tree.
/// 结束追踪：Finish 当前 span，将首棵 trace 树 DFS 写入临时文件。
pub fn TracerFinishSpan(ctx: Context, store: impl Queryer) {
    let span = SpanFromContext(&ctx);
    let mut traces = match store.Traces(TracesOpts {}) {
        Ok(v) => v,
        Err(err) => {
            // 查询失败时仅 stderr 告警，不 panic。
            eprintln!("fail to get traces: {err}");
            return;
        }
    };
    // Go calls span.Finish() unconditionally after a successful query; a
    // context not produced by TracerStartSpan therefore panics on a nil span.
    span.expect("TracerFinishSpan requires an active span")
        .Finish();
    if traces.is_empty() {
        // 无已收集 span 则无需写文件。
        return;
    }
    let trace = &mut traces[0];

    let filename = call_get_trace_file_name();
    let file = match File::create(&filename) {
        Ok(v) => v,
        Err(err) => {
            // 无法创建 trace 文件时提前返回。
            eprintln!("fail to open trace file: {err}");
            return;
        }
    };
    // Go: defer file.Close — File drops here at end of scope.
    // Go defer Close；Rust 作用域结束自动 drop File。
    let _ = writeln!(io::stderr(), "Detail BR trace in {filename} ");
    eprintln!("Detail BR trace filename={filename}");
    let mut tub = Tabby::NewCustom(Box::new(file));

    // 仅输出 traces[0] 首棵树（与 Go 行为一致）。
    dfsTree(trace, "", false, &mut tub);
    tub.Print();
}

/// dfsTree mirrors Go: tree prefixes, timespan columns, children sorted by start.
/// DFS 输出 trace 树：ASCII 前缀、起止时刻、耗时列；子节点按 start 排序。
pub fn dfsTree(t: &mut Trace, prefix: &str, isLast: bool, tub: &mut Tabby) {
    // 根据是否最后一个兄弟选择 ├─ / └─ 与前缀延续符。
    let (newPrefix, suffix) = if prefix.is_empty() {
        (format!("{prefix}  "), String::new())
    } else if !isLast {
        (format!("{prefix}│ "), "├─".to_string())
    } else {
        (format!("{prefix}  "), "└─".to_string())
    };

    let mut start = UNIX_EPOCH;
    let mut duration = Duration::ZERO;
    // 读取 span 时间区间；失败则用零值占位。
    if let Ok(e) = t.TimespanEvent() {
        start = e.Start();
        let end = e.End();
        duration = end.duration_since(start).unwrap_or(Duration::ZERO);
    }

    tub.AddLine(
        format!("{prefix}{suffix}{}", t.Span.name),
        format_clock_micros(start),
        format_go_duration(duration),
    );

    // Go slices.SortFunc mutates t.Sub in place.
    // 子 span 按开始时间升序，并与 Go 一样原地更新 Trace。
    t.Sub.sort_unstable_by(|i, j| {
        let istart = i.TimespanEvent().map(|e| e.Start()).unwrap_or(UNIX_EPOCH);
        let jstart = j.TimespanEvent().map(|e| e.Start()).unwrap_or(UNIX_EPOCH);
        istart.cmp(&jstart)
    });

    let last = t.Sub.len().saturating_sub(1);
    // 递归 DFS 输出各子树。
    for (i, sp) in t.Sub.iter_mut().enumerate() {
        dfsTree(sp, &newPrefix, i == last, tub);
    }
}

// 格式化为 `HH:MM:SS.ffffff` 墙钟微秒（自 epoch 日内秒数）。
fn format_clock_micros(t: SystemTime) -> String {
    format_clock_micros_with_offset(t, local_utc_offset_seconds(t))
}

fn format_clock_micros_with_offset(t: SystemTime, offset_seconds: i32) -> String {
    const MICROS_PER_SECOND: i128 = 1_000_000;
    const MICROS_PER_DAY: i128 = 86_400 * MICROS_PER_SECOND;
    let unix_micros = match t.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_micros() as i128,
        Err(error) => -(error.duration().as_micros() as i128),
    };
    let local_micros =
        unix_micros.saturating_add(i128::from(offset_seconds).saturating_mul(MICROS_PER_SECOND));
    // Go time.Format uses the time's local location for the wall-clock column.
    let day_micros = local_micros.rem_euclid(MICROS_PER_DAY);
    let day_secs = day_micros / MICROS_PER_SECOND;
    let micros = day_micros % MICROS_PER_SECOND;
    let hh = day_secs / 3600;
    let mm = (day_secs % 3600) / 60;
    let ss = day_secs % 60;
    format!("{hh:02}:{mm:02}:{ss:02}.{micros:06}")
}

#[cfg(test)]
pub(crate) fn format_clock_micros_with_offset_for_test(
    t: SystemTime,
    offset_seconds: i32,
) -> String {
    format_clock_micros_with_offset(t, offset_seconds)
}

/// Format a duration like Go's `time.Duration.String()`.
/// 按 Go `time.Duration.String()` 规则格式化耗时（ns/µs/ms/h/m/s）。
pub fn format_go_duration(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns == 0 {
        return "0s".to_string();
    }
    // 单位阈值与 Go time.Duration 一致：ns → µs → ms → h/m/s。
    const US: u128 = 1_000;
    const MS: u128 = 1_000_000;
    const S: u128 = 1_000_000_000;
    const M: u128 = 60 * S;
    const H: u128 = 60 * M;

    if ns < US {
        return format!("{ns}ns");
    }
    // 亚毫秒：整除则省略小数，否则 format_frac。
    if ns < MS {
        if ns % US == 0 {
            return format!("{}µs", ns / US);
        }
        return format_frac(ns, US, "µs");
    }
    if ns < S {
        if ns % MS == 0 {
            return format!("{}ms", ns / MS);
        }
        return format_frac(ns, MS, "ms");
    }

    // Go always emits seconds for values >= 1s, and emits a zero minute
    // component when an hour component is present (for example 1h0m0s).
    let hours = ns / H;
    let rem_after_hours = ns % H;
    let minutes = rem_after_hours / M;
    let seconds = rem_after_hours % M;
    let mut out = String::new();
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if hours > 0 || minutes > 0 {
        out.push_str(&format!("{minutes}m"));
    }
    if seconds % S == 0 {
        out.push_str(&format!("{}s", seconds / S));
    } else {
        out.push_str(&format_frac(seconds, S, "s"));
    }
    out
}

// 带小数的 duration 字符串，去除尾随零以对齐 Go 输出。
fn format_frac(ns: u128, unit: u128, suffix: &str) -> String {
    let whole = ns / unit;
    let mut frac = ns % unit;
    let mut scale = unit;
    // 约分尾随零，使 "1.50ms" 类输出与 Go 一致。
    while frac > 0 && frac % 10 == 0 && scale % 10 == 0 {
        frac /= 10;
        scale /= 10;
    }
    if frac == 0 {
        return format!("{whole}{suffix}");
    }
    // 小数位宽由 scale 数量级决定。
    let width = {
        let mut w = 0;
        let mut s = scale;
        while s > 1 {
            s /= 10;
            w += 1;
        }
        w
    };
    let mut frac_str = format!("{frac:0width$}");
    // 去掉小数部分尾随零。
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    format!("{whole}.{frac_str}{suffix}")
}
