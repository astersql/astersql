// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/progress_test.go`.
//! 通过 testWriter 注入接收进度 JSON，验证百分比封顶与取消行为。
//! recv_containing 忽略无关进度帧，直到匹配目标百分比。
//! 睡眠 2s 大于 1s 打印周期，保证至少一次 emit。
//! 超额 Inc 后百分比仍封顶，防止 UI 显示超过百分百。
//! Close 与 cancel 行为相反：前者抬满，后者冻结。
//! progress4 在 25% 后 Close，验证 Finish=100%。
//! progress8 验证取消不抬满。
//! JSON 断言使用精确两位小数文本，对齐格式化。
//! 通道缓冲依赖 mpsc 无界，避免测试写入阻塞。
//! Context 克隆出 cancel 句柄，与打印循环共享取消态。
//! 三段场景共用同一 ctx 时注意 cancel 影响后续；本测最后才 cancel。

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::progress::NewProgressPrinter;
use crate::stubs::context::Context;

/// 在截止时间内等到包含 needle 的进度行；超时 panic 以便暴露卡死。
fn recv_containing(rx: &mpsc::Receiver<String>, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remain.min(Duration::from_secs(2))) {
            Ok(p) if p.contains(needle) => return p,
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(e) => panic!("recv failed: {e}"),
        }
    }
    panic!("timed out waiting for progress containing {needle}");
}

#[test]
fn test_progress() {
    // 三段场景：total=2 递增、Close 强制 100%、cancel 冻结当前百分比。
    let ctx = Context::new();
    let cancel = ctx.clone();

    // total=2：Inc 一次→50%，再 Inc→100%；超额 Inc 仍封顶 100%。
    let (tx2, rx2) = mpsc::channel::<String>();
    let progress2 = NewProgressPrinter("test", 2, false);
    progress2.goPrintProgress(
        ctx.clone(),
        None,
        Some(Arc::new(move |p: &str| {
            let _ = tx2.send(p.to_string());
        })),
    );
    progress2.Inc();
    thread::sleep(Duration::from_secs(2));
    let p = recv_containing(&rx2, r#""P":"50.00%""#);
    assert!(p.contains(r#""P":"50.00%""#), "got {p}");
    progress2.Inc();
    thread::sleep(Duration::from_secs(2));
    let p = recv_containing(&rx2, r#""P":"100.00%""#);
    assert!(p.contains(r#""P":"100.00%""#), "got {p}");
    progress2.Inc();
    thread::sleep(Duration::from_secs(2));
    let p = recv_containing(&rx2, r#""P":"100.00%""#);
    assert!(p.contains(r#""P":"100.00%""#), "got {p}");
    progress2.Close();

    // total=4：25% 后 Close，收尾行强制写成 100%（对齐 Go bar.Finish）。
    let (tx4, rx4) = mpsc::channel::<String>();
    let progress4 = NewProgressPrinter("test", 4, false);
    progress4.goPrintProgress(
        ctx.clone(),
        None,
        Some(Arc::new(move |p: &str| {
            let _ = tx4.send(p.to_string());
        })),
    );
    progress4.Inc();
    thread::sleep(Duration::from_secs(2));
    let p = recv_containing(&rx4, r#""P":"25.00%""#);
    assert!(p.contains(r#""P":"25.00%""#), "got {p}");
    progress4.Inc();
    progress4.Close();
    thread::sleep(Duration::from_secs(2));
    let p = recv_containing(&rx4, r#""P":"100.00%""#);
    assert!(p.contains(r#""P":"100.00%""#), "got {p}");

    // total=8：进度到 25% 后 cancel，最终进度应停在当前值而非跳到 100%。
    let (tx8, rx8) = mpsc::channel::<String>();
    let progress8 = NewProgressPrinter("test", 8, false);
    progress8.goPrintProgress(
        ctx.clone(),
        None,
        Some(Arc::new(move |p: &str| {
            let _ = tx8.send(p.to_string());
        })),
    );
    progress8.Inc();
    progress8.Inc();
    thread::sleep(Duration::from_secs(2));
    let p = recv_containing(&rx8, r#""P":"25.00%""#);
    assert!(p.contains(r#""P":"25.00%""#), "got {p}");

    // Cancel should stop progress at the current position.
    // 取消路径对齐 Go：Finish 时使用当前计数，不抬到 total。
    cancel.cancel();
    let p = recv_containing(&rx8, r#""P":"25.00%""#);
    assert!(p.contains(r#""P":"25.00%""#), "got {p}");
    progress8.Close();
}

#[test]
fn progress_waits_for_first_tick_but_close_is_immediate() {
    let ctx = Context::new();
    let (tx, rx) = mpsc::channel::<String>();
    let progress = NewProgressPrinter("ticker", 2, false);
    progress.goPrintProgress(
        ctx,
        None,
        Some(Arc::new(move |p: &str| {
            let _ = tx.send(p.to_string());
        })),
    );
    progress.Inc();
    assert!(
        matches!(
            rx.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "Go progress output is driven by the one-second ticker"
    );
    let started = Instant::now();
    progress.Close();
    let line = rx
        .recv_timeout(Duration::from_millis(200))
        .expect("Close must wake the printer immediately");
    assert!(line.contains(r#""P":"100.00%""#));
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn cancel_preserves_last_rendered_progress_before_first_tick() {
    let ctx = Context::new();
    let cancel = ctx.clone();
    let (tx, rx) = mpsc::channel::<String>();
    let progress = NewProgressPrinter("cancel", 2, false);
    progress.goPrintProgress(
        ctx,
        None,
        Some(Arc::new(move |p: &str| {
            let _ = tx.send(p.to_string());
        })),
    );

    progress.Inc();
    cancel.cancel();

    let line = rx
        .recv_timeout(Duration::from_millis(200))
        .expect("cancellation must finish the progress printer promptly");
    assert!(
        line.contains(r#""P":"0.00%""#),
        "Go pb.Finish preserves the last rendered value before the first tick; got {line}"
    );
    progress.Close();
}
