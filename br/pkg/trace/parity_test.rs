// Copyright 2026 AsterSQL.

//! BR trace 与 Go 公开契约的 parity 测试。
//! 对齐 Go `TestSpan`：父子 span 树形文本、空森林不落盘、写失败不 panic。
//! 通过 hook 覆盖 trace 文件名，避免污染全局默认路径；结束时必须还原。
//! 断言侧重输出形状与时长区间，而非精确纳秒值（睡眠有调度抖动）。

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use crate::{
    Context, ContextWithSpan, SpanFromContext, TracerFinishSpan, TracerStartSpan,
    format_go_duration, format_go_trace_stamp_with_offset_for_test,
    set_get_trace_file_name_for_test, timestampTraceFileName,
};

#[test]
fn timestamp_uses_local_numeric_offset_like_go() {
    assert_eq!(
        format_go_trace_stamp_with_offset_for_test(UNIX_EPOCH, 8 * 60 * 60),
        "1970-01-01T08.00.00+0800"
    );
    assert_eq!(
        format_go_trace_stamp_with_offset_for_test(UNIX_EPOCH, -(5 * 60 + 30) * 60),
        "1969-12-31T18.30.00-0530"
    );
}

// 构造 jobA→jobB 嵌套：子 span 挂到 ctx 后递归，再 sleep 100ms 后 Finish。
fn jobA(mut ctx: Context) {
    if let Some(span) = SpanFromContext(&ctx) {
        let span1 = span.Tracer().StartSpanChildOf("jobA", &span);
        // 把子 span 写回 ctx，供 jobB 继续作为 ChildOf 父节点。
        ctx = ContextWithSpan(ctx, Arc::clone(&span1));
        jobB(ctx);
        thread::sleep(Duration::from_millis(100));
        span1.Finish();
    } else {
        // 无根 span 时仍跑业务路径，保证无 tracing 也能走通。
        jobB(ctx);
        thread::sleep(Duration::from_millis(100));
    }
}

// jobB 为叶子：有 span 则创建 "jobB" 子节点，sleep 后 Finish。
fn jobB(ctx: Context) {
    if let Some(span) = SpanFromContext(&ctx) {
        let span1 = span.Tracer().StartSpanChildOf("jobB", &span);
        thread::sleep(Duration::from_millis(100));
        span1.Finish();
    } else {
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn go_rust_public_contract_matches() {
    // 进程级临时目录，避免并发测试互相覆盖 trace 文件。
    let tmp = std::env::temp_dir().join(format!("br-trace-parity-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    // --- normal: Go TestSpan tree output (jobA → jobB) ---
    // 正常路径：应写出两行树形文本，时长约 200ms/100ms。
    let filename = tmp.join("br.trace");
    let filename_str = filename.to_string_lossy().into_owned();
    set_get_trace_file_name_for_test(Some(Arc::new({
        let filename_str = filename_str.clone();
        move || filename_str.clone()
    })));

    // Start 返回带根 span 的 ctx 与 MemoryStore，Finish 时落盘。
    let (ctx, store) = TracerStartSpan(Context::Background());
    jobA(ctx.clone());
    TracerFinishSpan(ctx, Arc::clone(&store));

    // 正常路径必须产生可读文件，内容交给形状校验。
    let content = std::fs::read_to_string(&filename).expect("trace file should exist");
    let re = regex_lite_span(&content);
    assert!(
        re,
        "trace content should match Go TestSpan shape, got:\n{content}"
    );

    // --- boundary: no finished child spans → Traces empty → no file rewrite ---
    // 边界：仅 Finish 根 span、无子节点时，空森林不得创建详情文件。
    let boundary_path = tmp.join("br.trace.boundary");
    let boundary_str = boundary_path.to_string_lossy().into_owned();
    set_get_trace_file_name_for_test(Some(Arc::new({
        let boundary_str = boundary_str.clone();
        move || boundary_str.clone()
    })));
    let (ctx, store) = TracerStartSpan(Context::Background());
    // Finish root only via TracerFinishSpan; no child spans collected before Traces().
    TracerFinishSpan(ctx, store);
    assert!(
        !boundary_path.exists(),
        "empty trace forest must not create the detail file"
    );

    // --- error: unwritable path is non-fatal (Go logs and returns) ---
    // 错误路径：不可写路径应对齐 Go——记日志后返回，不得 panic。
    let bad = PathBuf::from("/dev/null/impossible-br-trace/br.trace");
    let bad_str = bad.to_string_lossy().into_owned();
    set_get_trace_file_name_for_test(Some(Arc::new({
        let bad_str = bad_str.clone();
        move || bad_str.clone()
    })));
    let (ctx, store) = TracerStartSpan(Context::Background());
    jobA(ctx.clone());
    // Must not panic.
    TracerFinishSpan(ctx, store);

    // --- resource cleanup: restore getTraceFileName; timestamp helper usable ---
    // 清理：还原全局 hook，并校验时间戳文件名落在 temp 且含 br.trace. 前缀。
    set_get_trace_file_name_for_test(None);
    let name = timestampTraceFileName();
    let temp = std::env::temp_dir().to_string_lossy().into_owned();
    assert!(
        name.starts_with(&temp),
        "timestampTraceFileName must live under temp dir: {name}"
    );
    assert!(
        name.contains("br.trace."),
        "timestampTraceFileName must contain br.trace. prefix: {name}"
    );

    // Duration formatter matches Go ms shape used in the file.
    // 200.621764ms == 200_621_764 ns (Go time.Duration units).
    // 时长格式必须与 Go 写入文件时的 ms 小数形态一致。
    let d = Duration::from_nanos(200_621_764);
    let s = format_go_duration(d);
    assert_eq!(s, "200.621764ms");

    // Ensure prior normal file stayed readable after finish (closed).
    // Finish 后文件句柄应已关闭，内容仍可读且不变。
    let again = std::fs::read_to_string(&filename).expect("file remains readable after close");
    assert_eq!(again, content);

    let _ = std::fs::remove_dir_all(&tmp);
}

// 轻量匹配 Go TestSpan 正则：两行 jobA/jobB，时长分别落在 200–299 / 100–199 ms。
fn regex_lite_span(s: &str) -> bool {
    // Go: `^jobA.*2[0-9][0-9]\.[0-9]+ms\n  └─jobB.*1[0-9][0-9]\.[0-9]+ms\n$`
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() != 2 {
        return false;
    }
    // 首行必须是 jobA 操作名，且含 ms 时长后缀。
    if !lines[0].starts_with("jobA") {
        return false;
    }
    if !lines[0].contains("ms") {
        return false;
    }
    // Extract trailing duration token.
    // 取行末时长 token，再按整数毫秒区间校验（容忍小数部分）。
    let Some(dur_a) = lines[0].split_whitespace().last() else {
        return false;
    };
    if !dur_in_range(dur_a, 200, 299) {
        return false;
    }
    // 次行缩进树形前缀对齐 Go 打印器的 `  └─jobB`。
    if !lines[1].starts_with("  └─jobB") {
        return false;
    }
    let Some(dur_b) = lines[1].split_whitespace().last() else {
        return false;
    };
    // jobB 约 100ms sleep，整数毫秒应落在 100–199。
    dur_in_range(dur_b, 100, 199)
}

// 解析 `NNN.xxxms`，用 floor 后的整毫秒判断是否落在 [lo, hi]。
fn dur_in_range(token: &str, lo: u64, hi: u64) -> bool {
    // 去掉 ms 后缀失败则整行形状不对。
    let Some(num) = token.strip_suffix("ms") else {
        return false;
    };
    let Ok(v) = num.parse::<f64>() else {
        return false;
    };
    // 与 Go 正则用整百区间一致：只看 floor 后的整数部分。
    let whole = v.floor() as u64;
    whole >= lo && whole <= hi
}
