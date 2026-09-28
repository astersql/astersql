// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Progress bars matching `br/pkg/glue/progressing.go`.
//!
//! 进度条实现：对齐 Go `progressing.go` 的 Inc/Close/Wait 与多任务展示语义。
//! 无真实 mpb 依赖时用 BarState/ProgressGroup 模拟；按是否 TTY 分流到
//! 终端条或 Dummy/Log 路径，保证非交互环境仍可观测完成。
//! 约束：OnlyOneTask=-1 表示单任务 spinner；total=0 视为立即完成。
//! Close 必须幂等且错误路径也要收尾，避免进度条或日志泄漏。
//! MultiProgress 与单条 StartProgressBar 共用 BarState 渲染规则。

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_errors::SharedError;

use crate::console_glue::{ConsoleOperations, ExtraField, color, printFinalMessage};
use crate::glue::{Context, Progress};

// 与 Go 相同：total==OnlyOneTask 时走单任务 spinner 分支。
// 该哨兵值不能与合法非负 total 混淆。
pub const OnlyOneTask: i32 = -1;

// 将 spinner 帧染色为粗体绿色，保持与 Go coloredSpinner 一致的视觉语义。
fn coloredSpinner(mut s: Vec<String>) -> Vec<String> {
    let c = color::New(&[color::Bold, color::FgGreen]);
    for item in s.iter_mut() {
        *item = c.Sprint(item.as_str());
    }
    s
}

// 构造四帧 spinner 文本；调用方可能丢弃返回值，仅为对齐 Go 副作用。
fn spinner_text() -> Vec<String> {
    coloredSpinner(vec![
        "/".to_string(),
        "-".to_string(),
        "\\".to_string(),
        "|".to_string(),
    ])
}

// 单任务完成后缀：绿色 DONE。
fn spinner_done_text() -> String {
    format!(":: {}", color::GreenString("DONE"))
}

/// Lightweight progress bar state (mpb stand-in preserving Go Inc/Close/Wait semantics).
/// 轻量进度状态：替代 mpb，保留原子累计、完成/中止与最终文案回调。
struct BarState {
    // 当前进度（原子，允许多线程 Inc）。
    current: AtomicI64,
    // 目标总量；负数表示未知总量时不自动完成。
    total: AtomicI64,
    completed: AtomicBool,
    aborted: AtomicBool,
    rendered: AtomicBool,
    title: String,
    // true 时完成行用 spinner DONE 样式，而非额外字段回调。
    one_task: bool,
    // 可选最终消息工厂；与 Go ExtraField 收尾打印对齐。
    final_message: Mutex<Option<Box<dyn FnMut() -> String + Send>>>,
}

impl BarState {
    // 包装为 Arc，便于 ProgressGroup 与 Progress 共享同一条。
    fn new(
        title: String,
        total: i64,
        one_task: bool,
        final_message: Option<Box<dyn FnMut() -> String + Send>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            current: AtomicI64::new(0),
            total: AtomicI64::new(total),
            completed: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
            rendered: AtomicBool::new(false),
            title,
            one_task,
            final_message: Mutex::new(final_message),
        })
    }

    // 单步推进，委托 IncrBy。
    fn Increment(&self) {
        self.IncrBy(1);
    }

    // 累加后若已达 total（且 total>=0）则标记 completed。
    fn IncrBy(&self, n: i64) {
        // fetch_add 返回旧值，加 n 得到新当前值再与 total 比较。
        let cur = self.current.fetch_add(n, Ordering::Relaxed) + n;
        let total = self.total.load(Ordering::Relaxed);
        // total<0 表示未知上限，永不因累计自动完成。
        if total >= 0 && cur >= total {
            self.completed.store(true, Ordering::Relaxed);
        }
    }

    // 供 Progress::GetCurrent 读取。
    fn Current(&self) -> i64 {
        self.current.load(Ordering::Relaxed)
    }

    // 是否已达总量或被 SetTotal(..., true)。
    fn Completed(&self) -> bool {
        self.completed.load(Ordering::Relaxed)
    }

    // 是否被 Abort/Close 提前终止。
    fn Aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed)
    }

    // hide 参数保留以对齐 Go Abort 签名，当前实现忽略。
    fn Abort(&self, _hide: bool) {
        self.aborted.store(true, Ordering::Relaxed);
    }

    // 动态改总量；complete=true 时直接视为完成（如 total==0 启动路径）。
    fn SetTotal(&self, total: i64, complete: bool) {
        self.total.store(total, Ordering::Relaxed);
        if complete {
            self.completed.store(true, Ordering::Relaxed);
        }
    }

    // 生成完成/中止行：中止优先，其次 one_task，再次回调或默认 DONE。
    fn render_done(&self) -> String {
        // 未完成却中止 → 红色 ABORTED。
        if self.Aborted() && !self.Completed() {
            return format!("{}  :: {}", self.title, color::RedString("ABORTED"));
        }
        // 单任务：标题 + spinner DONE。
        if self.one_task {
            return format!("{} {}", self.title, spinner_done_text());
        }
        let mut guard = self.final_message.lock().expect("final message lock");
        // 有 ExtraField 回调则拼接自定义尾注，否则高亮 DONE。
        if let Some(cb) = guard.as_mut() {
            format!("{}  :: {}", self.title, cb())
        } else {
            format!("{}  :: {}", self.title, color::HiGreenString("DONE"))
        }
    }

    fn take_render_done(&self) -> Option<String> {
        if self.rendered.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(self.render_done())
        }
    }
}

// 一组进度条：Wait 时一次性写出各条完成行到捕获缓冲。
struct ProgressGroup {
    bars: Mutex<Vec<Arc<BarState>>>,
    // 当前实现主要靠 ConsoleWriter 直写；此缓冲保留 Go 捕获语义占位。
    out: Arc<Mutex<Vec<u8>>>,
    // Wait 幂等：第二次调用直接返回。
    closed: AtomicBool,
}

impl ProgressGroup {
    // 新建可捕获输出的组；多 Progress 共享同一组时 Wait 只生效一次。
    fn new_with_capture() -> Arc<Self> {
        Arc::new(Self {
            bars: Mutex::new(Vec::new()),
            out: Arc::new(Mutex::new(Vec::new())),
            closed: AtomicBool::new(false),
        })
    }

    // 注册子条，供 Wait 遍历渲染。
    fn add(&self, bar: Arc<BarState>) {
        self.bars.lock().expect("bars lock").push(bar);
    }

    // mpb.Progress.Wait 只在所有条已完成或中止后返回。
    fn all_finished(&self) -> bool {
        self.bars
            .lock()
            .expect("bars lock")
            .iter()
            .all(|bar| bar.Completed() || bar.Aborted())
    }

    // 关闭组并渲染每条完成行；已关闭则 no-op。
    fn finish(&self) -> Vec<String> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Vec::new();
        }
        let bars = self.bars.lock().expect("bars lock").clone();
        let mut out = self.out.lock().expect("out lock");
        let mut lines = Vec::with_capacity(bars.len());
        for bar in bars {
            if let Some(line) = bar.take_render_done() {
                let _ = writeln!(out, "{line}");
                lines.push(line);
            }
        }
        lines
    }

    fn Wait(&self) -> Vec<String> {
        while !self.all_finished() {
            thread::sleep(Duration::from_millis(10));
        }
        self.finish()
    }
}

// TTY 路径上的 Progress：持有单条 + 组 + 控制台写端。
struct PbProgress {
    bar: Arc<BarState>,
    progress: Arc<ProgressGroup>,
    writer: Arc<Mutex<dyn Write + Send>>,
}

impl Progress for PbProgress {
    // 转发到 BarState，保持 Progress trait 契约。
    fn Inc(&self) {
        self.bar.Increment();
    }

    fn IncBy(&self, n: i64) {
        self.bar.IncrBy(n);
    }

    fn GetCurrent(&self) -> i64 {
        self.bar.Current()
    }

    fn Close(&self) {
        // 未完成且未中止时强制 Abort，对齐 Go Close 收尾。
        if !(self.bar.Completed() || self.bar.Aborted()) {
            self.bar.Abort(false);
        }
        // Flush finished line to the console output (TTY path).
        // 先把完成行刷到控制台，再 Wait 组。
        if let Some(line) = self.bar.take_render_done()
            && let Ok(mut w) = self.writer.lock()
        {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
        let _ = self.progress.Wait();
    }
}

impl PbProgress {
    // 在后台线程 Wait 组，主循环轮询取消与完成。
    fn Wait(&self, ctx: Context) -> Result<(), SharedError> {
        let progress = Arc::clone(&self.progress);
        let (tx, rx) = std::sync::mpsc::channel();
        // 独立线程执行 Wait，避免阻塞取消检查。
        thread::spawn(move || {
            let lines = progress.Wait();
            let _ = tx.send(lines);
        });
        loop {
            // 上下文取消 → 立即返回，对齐 Go ctx.Done。
            if ctx.is_cancelled() {
                return Err(astersql_errors::New("context canceled"));
            }
            match rx.try_recv() {
                Ok(lines) => {
                    if let Ok(mut w) = self.writer.lock() {
                        for line in lines {
                            let _ = writeln!(w, "{line}");
                        }
                        let _ = w.flush();
                    }
                    return Ok(());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // 短暂休眠降低空转；间隔与 Go 轮询粒度同级。
                    thread::sleep(Duration::from_millis(10));
                }
                // 发送端已退出也视为 Wait 结束。
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}

/// ProgressWaiter extends Progress with Wait.
/// 在 Progress 之上增加可取消 Wait，供 StartProgressBar 返回。
pub trait ProgressWaiter: Progress {
    fn Wait(&self, ctx: Context) -> Result<(), SharedError>;
}

// 非 TTY：包装任意 Progress，Wait 立即成功。
struct NoOpWaiter {
    inner: Arc<dyn Progress>,
}

impl Progress for NoOpWaiter {
    fn Inc(&self) {
        self.inner.Inc();
    }
    fn IncBy(&self, cnt: i64) {
        self.inner.IncBy(cnt);
    }
    fn GetCurrent(&self) -> i64 {
        self.inner.GetCurrent()
    }
    fn Close(&self) {
        self.inner.Close();
    }
}

impl ProgressWaiter for NoOpWaiter {
    // Dummy 路径无需阻塞等待渲染。
    fn Wait(&self, _ctx: Context) -> Result<(), SharedError> {
        Ok(())
    }
}

// redirectLog / 非 TTY：用 stderr 打完成行代替动画条。
struct DummyProgress {
    current: AtomicI64,
    total: i64,
    title: String,
    // Close 幂等标志。
    closed: AtomicBool,
}

impl DummyProgress {
    // 初始 current=0；total 仅用于 Close 日志展示。
    fn new(title: String, total: i64) -> Arc<Self> {
        Arc::new(Self {
            current: AtomicI64::new(0),
            total,
            title,
            closed: AtomicBool::new(false),
        })
    }
}

impl Progress for DummyProgress {
    fn Inc(&self) {
        self.IncBy(1);
    }
    // Dummy 不自动 completed，完成仅由 Close 日志表达。
    fn IncBy(&self, cnt: i64) {
        self.current.fetch_add(cnt, Ordering::Relaxed);
    }
    fn GetCurrent(&self) -> i64 {
        self.current.load(Ordering::Relaxed)
    }
    fn Close(&self) {
        // 重复 Close 直接返回，避免双写日志。
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // redirectLog=true path: emit a completion log line.
        // 非 TTY 完成日志：名称 + 当前/总量，便于流水线抓取。
        eprintln!(
            "progress done name={} current={} total={}",
            self.title,
            self.GetCurrent(),
            self.total
        );
    }
}

impl ConsoleOperations {
    // 探测输出是否为终端，决定 TTY / Dummy 分流。
    pub fn OutputIsTTY(&self) -> bool {
        self.glue.out_is_terminal()
    }

    /// StartProgressBar starts a progress bar with the console operations.
    /// 公开入口：非 TTY 走 Dummy，TTY 走 PbProgress。
    pub fn StartProgressBar(
        &self,
        title: impl Into<String>,
        total: i32,
        extraFields: Vec<ExtraField>,
    ) -> Box<dyn ProgressWaiter> {
        let title = title.into();
        if !self.OutputIsTTY() {
            return self.startProgressBarOverDummy(title, total, extraFields);
        }
        self.startProgressBarOverTTY(title, total, extraFields)
    }

    fn startProgressBarOverDummy(
        &self,
        title: String,
        total: i32,
        _extraFields: Vec<ExtraField>,
    ) -> Box<dyn ProgressWaiter> {
        // Mirrors utils.StartProgress(..., redirectLog=true, nil) without requiring a live TTY.
        // 对齐 Go redirectLog=true：忽略 extraFields，仅日志计数。
        let inner: Arc<dyn Progress> = DummyProgress::new(title, total as i64);
        Box::new(NoOpWaiter { inner })
    }

    fn startProgressBarOverTTY(
        &self,
        title: String,
        total: i32,
        extraFields: Vec<ExtraField>,
    ) -> Box<dyn ProgressWaiter> {
        let group = ProgressGroup::new_with_capture();
        // OnlyOneTask / 普通条由 adjustTotal 分支。
        let bar = adjustTotal(Arc::clone(&group), title, total, extraFields);
        // total==0：立即完成，避免永远等不到 Inc。
        if total == 0 {
            bar.SetTotal(0, true);
        }
        let writer: Arc<Mutex<dyn Write + Send>> =
            Arc::new(Mutex::new(ConsoleWriter { ops: self.clone() }));
        Box::new(PbProgressWaiter(PbProgress {
            bar,
            progress: group,
            writer,
        }))
    }
}

// 把 Write 适配到 ConsoleOperations.Printf，供进度行输出。
struct ConsoleWriter {
    ops: ConsoleOperations,
}

impl Write for ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // 有损 UTF-8 解码：进度文案以可读字符串打印。
        let s = String::from_utf8_lossy(buf);
        self.ops.Printf(&s);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// newtype：让 PbProgress 同时满足 ProgressWaiter 对象安全返回。
struct PbProgressWaiter(PbProgress);

impl Progress for PbProgressWaiter {
    // 全部委托内层 PbProgress。
    fn Inc(&self) {
        self.0.Inc();
    }
    fn IncBy(&self, cnt: i64) {
        self.0.IncBy(cnt);
    }
    fn GetCurrent(&self) -> i64 {
        self.0.GetCurrent()
    }
    fn Close(&self) {
        self.0.Close();
    }
}

impl ProgressWaiter for PbProgressWaiter {
    // 暴露可取消 Wait。
    fn Wait(&self, ctx: Context) -> Result<(), SharedError> {
        self.0.Wait(ctx)
    }
}

// 按 total 选择单任务条或带 ExtraField 的普通条。
fn adjustTotal(
    pb: Arc<ProgressGroup>,
    title: String,
    total: i32,
    extraFields: Vec<ExtraField>,
) -> Arc<BarState> {
    if total == OnlyOneTask {
        // 单任务：内部 total 记为 1，样式走 spinner。
        return buildOneTaskBar(pb, title, 1);
    }
    buildProgressBar(pb, title, total, extraFields)
}

// 普通进度条：挂上 printFinalMessage(extraFields) 作为收尾文案。
fn buildProgressBar(
    pb: Arc<ProgressGroup>,
    title: String,
    total: i32,
    extraFields: Vec<ExtraField>,
) -> Arc<BarState> {
    let bar = BarState::new(
        title,
        total as i64,
        false,
        Some(printFinalMessage(extraFields)),
    );
    pb.add(Arc::clone(&bar));
    bar
}

// 单任务条：触发 spinner 构造副作用，不挂 ExtraField。
fn buildOneTaskBar(pb: Arc<ProgressGroup>, title: String, total: i32) -> Arc<BarState> {
    let _ = spinner_text(); // retain colored spinner construction side effects like Go.
    // 与 Go 一样先构造彩色 spinner，即使当前未逐帧刷新。
    let bar = BarState::new(title, total as i64, true, None);
    pb.add(Arc::clone(&bar));
    bar
}

/// 多进度场景下的单条接口：Increment 推进，Done 主动结束。
pub trait ProgressBar: Send + Sync {
    fn Increment(&self);
    fn Done(&self);
}

/// 多进度容器：AddTextBar 注册子条，Wait 等待全部收尾。
pub trait MultiProgress: Send + Sync {
    fn AddTextBar(&self, name: &str, total: i64) -> Box<dyn ProgressBar>;
    fn Wait(&self);
}

impl ConsoleOperations {
    /// 启动多进度：非 TTY 用 NopMultiProgress（日志倒计时），TTY 用终端组。
    pub fn StartMultiProgress(&self) -> Box<dyn MultiProgress> {
        if !self.OutputIsTTY() {
            return Box::new(NopMultiProgress {});
        }
        Box::new(TerminalMultiProgress {
            group: ProgressGroup::new_with_capture(),
            writer: Arc::new(Mutex::new(ConsoleWriter { ops: self.clone() })),
        })
    }
}

/// 非 TTY 多进度：AddTextBar 返回 LogBar，Wait 空操作。
pub struct NopMultiProgress;

/// 日志条：用原子递减 total，归零时打印 done。
pub struct LogBar {
    name: String,
    // 剩余计数；Increment 每次 -1。
    total: AtomicI64,
}

impl MultiProgress for NopMultiProgress {
    fn AddTextBar(&self, name: &str, total: i64) -> Box<dyn ProgressBar> {
        // 开始行便于流水线对齐 Go 日志格式。
        eprintln!("progress start name={name}");
        Box::new(LogBar {
            name: name.to_string(),
            total: AtomicI64::new(total),
        })
    }

    // 非 TTY 无需阻塞；条自行在 Increment 时打 done。
    fn Wait(&self) {}
}

impl ProgressBar for LogBar {
    fn Increment(&self) {
        // Go: atomic.AddInt64(&total, -1); return value is new value.
        // fetch_sub 返回旧值；减一后若 <=0 则完成。
        if self.total.fetch_sub(1, Ordering::SeqCst) - 1 <= 0 {
            eprintln!("progress done name={}", self.name);
        }
    }

    // 非 TTY LogBar 的 Done 为空：完成靠计数耗尽。
    fn Done(&self) {}
}

// TTY 多进度中的单条：共享 group 与 ConsoleWriter。
pub struct TerminalBar {
    bar: Arc<BarState>,
    group: Arc<ProgressGroup>,
    writer: Arc<Mutex<dyn Write + Send>>,
}

impl ProgressBar for TerminalBar {
    // 推进内部 BarState。
    fn Increment(&self) {
        self.bar.Increment();
    }

    fn Done(&self) {
        // 主动结束：Abort + 写完成行；只等待当前条，不等待整组。
        self.bar.Abort(false);
        if let Some(line) = self.bar.take_render_done()
            && let Ok(mut w) = self.writer.lock()
        {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}

// TTY 多进度实现：条加入同一 ProgressGroup。
pub struct TerminalMultiProgress {
    group: Arc<ProgressGroup>,
    writer: Arc<Mutex<dyn Write + Send>>,
}

impl MultiProgress for TerminalMultiProgress {
    fn AddTextBar(&self, name: &str, total: i64) -> Box<dyn ProgressBar> {
        // 触发 spinner 副作用；条以 one_task 样式创建。
        let _ = spinner_text();
        let bar = BarState::new(name.to_string(), total, true, None);
        self.group.add(Arc::clone(&bar));
        Box::new(TerminalBar {
            bar,
            group: Arc::clone(&self.group),
            writer: Arc::clone(&self.writer),
        })
    }

    // 等待组内所有条写出完成行。
    fn Wait(&self) {
        let lines = self.group.Wait();
        if let Ok(mut w) = self.writer.lock() {
            for line in lines {
                let _ = writeln!(w, "{line}");
            }
            let _ = w.flush();
        }
    }
}
