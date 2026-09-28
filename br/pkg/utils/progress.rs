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

//! Progress reporting ported from `br/pkg/utils/progress.go`.
//! 备份/恢复进度条：原子计数 + 后台打印线程；支持日志 JSON、终端条与测试注入。
//! Close 将进度抬到 100%；Context 取消则保留当前百分比（对齐 Go bar.Finish）。
//! ProgressPrinter 字段命名保留 Go 风格（redirectLog/closeCh）。
//! Inc/IncBy 使用 Relaxed，进度展示允许短暂滞后。
//! Close 持 closeMu，防止并发 Close 双重 take。
//! goPrintProgress 包可见，便于测试直接注入 sink。
//! 打印周期固定 1s，与 Go ticker 粒度一致。
//! use_terminal 当前恒假，避免 CI 非 TTY 输出差异。
//! emit_progress 三通道互斥：终端 / 测试 / 日志。
//! JSON 字段字母缩写对齐 Go 日志消费者。
//! estimate_remaining 线性外推，不处理变速任务。
//! StartProgress 组合构造与启动，减少调用方样板。
//! StartProgressWithWriter 专供单测，不进生产路径。
//! progress_error 占位避免 SharedError import 被优化掉。
//! cancel 路径注释强调与 Close 的百分比差异。
//! Close 路径强制 total/total，对应用户可见 100%。
//! shown=min(current,total) 防止超额 Inc 显示>100%。
//! speed 用 speed_base/elapsed，起步阶段可能为 0/s。
//! remaining 在未开始或已完成时归零。
//! log_fn 可替换，便于测试断言日志字段。
//! done 通道保证 Close 等待最后一帧写出。
//! closeCh 为 None 表示尚未启动打印循环。
//! name 字段作为 step 标签进入结构化日志。
//! total==0 特例避免除零并显示完成。

use std::io::{self, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::stubs::context::Context;
use astersql_br_pkg_logutil::{Field, log};
use astersql_errors::SharedError;
use serde_json::Value;

/// 自定义日志回调；缺省走 `log::L().Info`。
type LogFunc = Arc<dyn Fn(&str, Vec<Field>) + Send + Sync>;
/// 测试用进度接收器，收到 JSON 行而非写日志。
type TestProgressSink = Arc<dyn Fn(&str) + Send + Sync>;

fn info_log(message: &str, fields: impl IntoIterator<Item = Field>) {
    log::L().Info(message, fields);
}

/// 进度打印机：`total` 为分母，`redirectLog` 为真时强制走日志而非终端条。
pub struct ProgressPrinter {
    name: String,
    total: i64,
    redirectLog: bool,
    progress: Arc<AtomicI64>,
    /// 串行化 Close，避免双重关闭竞态。
    closeMu: Mutex<()>,
    /// 通知打印循环退出的 sender；None 表示尚未 Start。
    closeCh: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    /// Close 等待打印线程确认退出的 receiver。
    closed: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

/// 构造未启动的打印机；需再调用 `goPrintProgress` / `StartProgress`。
pub fn NewProgressPrinter(
    name: impl Into<String>,
    total: i64,
    redirectLog: bool,
) -> ProgressPrinter {
    ProgressPrinter {
        name: name.into(),
        total,
        redirectLog,
        progress: Arc::new(AtomicI64::new(0)),
        closeMu: Mutex::new(()),
        closeCh: Mutex::new(None),
        closed: Mutex::new(None),
    }
}

impl ProgressPrinter {
    /// 进度 +1。
    pub fn Inc(&self) {
        self.progress.fetch_add(1, Ordering::Relaxed);
    }

    /// 进度增加 cnt（可为批量完成单元）。
    pub fn IncBy(&self, cnt: i64) {
        self.progress.fetch_add(cnt, Ordering::Relaxed);
    }

    /// 读取当前原子进度（可能暂时超过 total，展示时会封顶）。
    pub fn GetCurrent(&self) -> i64 {
        self.progress.load(Ordering::Relaxed)
    }

    /// 通知打印循环以 100% 收尾并等待线程退出；未启动则仅告警。
    pub fn Close(&self) {
        let _guard = self.closeMu.lock().expect("closeMu lock poisoned");
        let sender = self.closeCh.lock().expect("closeCh lock poisoned").take();
        if let Some(sender) = sender {
            let _ = sender.send(());
            // 等待打印线程发出 done，保证最后一行 100% 已写出。
            if let Some(closed) = self.closed.lock().expect("closed lock poisoned").take() {
                let _ = closed.recv();
            }
        } else {
            log::Warn("closing no-started progress printer", []);
        }
    }

    /// Starts the progress printer loop. Package-visible for Go-equivalent tests.
    /// 启动后台循环：每秒 emit 一次；cancel 保留当前值，close 强制 total。
    pub fn goPrintProgress(
        &self,
        ctx: Context,
        log_func: Option<LogFunc>,
        test_writer: Option<TestProgressSink>,
    ) {
        let name = self.name.clone();
        let total = self.total;
        let redirect_log = self.redirectLog;

        let (close_tx, close_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        *self.closeCh.lock().expect("closeCh lock poisoned") = Some(close_tx);
        *self.closed.lock().expect("closed lock poisoned") = Some(done_rx);

        let shared_progress = Arc::clone(&self.progress);
        thread::spawn(move || {
            let start = Instant::now();
            let log_fn = log_func.unwrap_or_else(|| Arc::new(|msg, fields| info_log(msg, fields)));
            // 终端条仅在非 redirect、非测试注入且判定为 TTY 时启用。
            let use_terminal = !redirect_log && test_writer.is_none() && is_terminal_output();
            let mut next_tick = Instant::now() + Duration::from_secs(1);
            // Go only copies the atomic counter into pb.Bar on a ticker event.
            // pb.Finish therefore preserves the last rendered value on cancellation.
            let mut rendered = 0;
            loop {
                let wait = next_tick
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10));
                match close_rx.recv_timeout(wait) {
                    Ok(()) => {
                        // Close：强制按 total/total 输出 100%。
                        emit_progress(
                            use_terminal,
                            &name,
                            total,
                            total,
                            start.elapsed(),
                            total,
                            &log_fn,
                            test_writer.as_ref(),
                        );
                        let _ = done_tx.send(());
                        return;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if ctx.is_cancelled() {
                    // Match Go bar.Finish() on cancel: keep the last value copied
                    // into the bar rather than observing a newer atomic increment.
                    emit_progress(
                        use_terminal,
                        &name,
                        rendered,
                        total,
                        start.elapsed(),
                        rendered,
                        &log_fn,
                        test_writer.as_ref(),
                    );
                    let _ = done_tx.send(());
                    return;
                }
                if Instant::now() < next_tick {
                    continue;
                }
                let current = shared_progress.load(Ordering::Relaxed);
                rendered = current.min(total);
                emit_progress(
                    use_terminal,
                    &name,
                    rendered,
                    total,
                    start.elapsed(),
                    rendered,
                    &log_fn,
                    test_writer.as_ref(),
                );
                next_tick += Duration::from_secs(1);
            }
        });
    }
}

/// 当前环境是否视为交互终端；Rust 侧暂恒 false，统一走日志/测试通道。
fn is_terminal_output() -> bool {
    false
}

/// 日志字段结构：P 百分比、C 计数、E 已用时、R 剩余、S 速率。
#[derive(Default)]
struct ProgressLogLine {
    P: String,
    C: String,
    E: String,
    R: String,
    S: String,
}

impl ProgressLogLine {
    /// 从 JSON Value 解析五字段；缺任一字段则返回 None。
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            P: value.get("P")?.as_str()?.to_string(),
            C: value.get("C")?.as_str()?.to_string(),
            E: value.get("E")?.as_str()?.to_string(),
            R: value.get("R")?.as_str()?.to_string(),
            S: value.get("S")?.as_str()?.to_string(),
        })
    }
}

/// 按通道输出进度：终端条 / 测试 sink / 结构化日志。
fn emit_progress(
    use_terminal: bool,
    name: &str,
    current: i64,
    total: i64,
    elapsed: Duration,
    speed_base: i64,
    log_fn: &LogFunc,
    test_writer: Option<&TestProgressSink>,
) {
    // total==0 时视为已完成 100%，避免除零。
    let percent = if total == 0 {
        100.0
    } else {
        (current as f64) * 100.0 / total as f64
    };
    if use_terminal {
        let bar_width = 20usize;
        let filled = ((percent / 100.0) * bar_width as f64).round() as usize;
        let bar = format!(
            "[{:<width$}]",
            "=".repeat(filled.min(bar_width)),
            width = bar_width
        );
        let _ = writeln!(
            io::stdout(),
            "{name} {bar} {percent:.1}% ({current}/{total}) elapsed={elapsed:?}"
        );
        return;
    }
    let remaining = estimate_remaining(elapsed, speed_base, total);
    let speed = if elapsed.as_secs_f64() > 0.0 {
        format!("{:.2}/s", speed_base as f64 / elapsed.as_secs_f64())
    } else {
        "0/s".to_string()
    };
    let json_line = serde_json::json!({
        "P": format!("{percent:.2}%"),
        "C": format!("{current}/{total}"),
        "E": format!("{elapsed:?}"),
        "R": format!("{remaining:?}"),
        "S": speed,
    });
    // 测试优先：直接投递 JSON 字符串，便于断言百分比文本。
    if let Some(writer) = test_writer {
        writer(&json_line.to_string());
        return;
    }
    if let Some(info) = ProgressLogLine::from_value(&json_line) {
        log_fn(
            "progress",
            vec![
                Field::string("step", name),
                Field::string("progress", &info.P),
                Field::string("count", &info.C),
                Field::string("speed", &info.S),
                Field::string("elapsed", &info.E),
                Field::string("remaining", &info.R),
            ],
        );
    }
}

/// 线性外推剩余时间；current<=0 或已完成则返回 0。
fn estimate_remaining(elapsed: Duration, current: i64, total: i64) -> Duration {
    if current <= 0 || current >= total {
        return Duration::ZERO;
    }
    let per_item = elapsed.as_secs_f64() / current as f64;
    Duration::from_secs_f64(per_item * (total - current) as f64)
}

/// 创建并立即启动进度打印，返回可 Inc/Close 的句柄。
pub fn StartProgress(
    ctx: Context,
    name: impl Into<String>,
    total: i64,
    redirect_log: bool,
    log_func: Option<LogFunc>,
) -> ProgressPrinter {
    let progress = NewProgressPrinter(name, total, redirect_log);
    progress.goPrintProgress(ctx, log_func, None);
    progress
}

// Expose a test hook matching Go's testWriter injection.
/// 带测试 sink 的启动入口，对应 Go testWriter 注入。
pub fn StartProgressWithWriter(
    ctx: Context,
    name: impl Into<String>,
    total: i64,
    redirect_log: bool,
    log_func: Option<LogFunc>,
    test_writer: TestProgressSink,
) -> ProgressPrinter {
    let progress = NewProgressPrinter(name, total, redirect_log);
    progress.goPrintProgress(ctx, log_func, Some(test_writer));
    progress
}

// Keep SharedError imported for future error-aware progress hooks.
/// 预留错误透传钩子，保持 SharedError 引用不过期。
#[allow(dead_code)]
fn progress_error(err: SharedError) -> SharedError {
    err
}
