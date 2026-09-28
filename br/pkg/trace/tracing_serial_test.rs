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

//! Go-equivalent of `br/pkg/trace/tracing_serial_test.go`.
//!
//! Mapping:
//! - `jobA` / `jobB` helpers → same span nesting + 100ms sleeps
//! - `TestSpan` → `test_span`
//!
//! Real filesystem write via `set_get_trace_file_name_for_test`; no network mock.
//!
//! 串行对照 Go `tracing_serial_test.go`：真实写盘校验 span 树文本。
//! 与 parity 测试场景重叠，但保留独立文件以对齐 Go 串行套件组织。
//! 必须在结束时还原 `getTraceFileName` hook，避免污染后续用例。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::{
    Context, ContextWithSpan, SpanFromContext, TracerFinishSpan, TracerStartSpan,
    set_get_trace_file_name_for_test, timestampTraceFileName,
};

/// jobA: if ctx has a span, start child "jobA", run jobB, sleep 100ms, then Finish.
/// 有父 span 时创建 jobA 子节点并下传 ctx；sleep 放在 jobB 之后以拉长父时长。
fn jobA(mut ctx: Context) {
    if let Some(span) = SpanFromContext(&ctx) {
        let span1 = span.Tracer().StartSpanChildOf("jobA", &span);
        ctx = ContextWithSpan(ctx, Arc::clone(&span1));
        jobB(ctx);
        thread::sleep(Duration::from_millis(100));
        span1.Finish();
    } else {
        // 无 tracing 时仍执行业务与睡眠，保证路径可独立跑通。
        jobB(ctx);
        thread::sleep(Duration::from_millis(100));
    }
}

/// jobB: if ctx has a span, start child "jobB", sleep 100ms, then Finish.
/// 叶子 span：100ms sleep 使输出时长落在 Go 正则的 1xx ms 区间。
fn jobB(ctx: Context) {
    if let Some(span) = SpanFromContext(&ctx) {
        let span1 = span.Tracer().StartSpanChildOf("jobB", &span);
        thread::sleep(Duration::from_millis(100));
        span1.Finish();
    } else {
        thread::sleep(Duration::from_millis(100));
    }
}

/// TestSpan: override trace filename, run jobA/jobB, assert tree-shaped output.
/// 覆盖文件名后跑完整 Start/Finish，断言内容匹配 Go TestSpan 正则形状。
#[test]
fn test_span() {
    let tmp = tempfile_dir();
    let filename = tmp.join("br.trace");
    let filename_str = filename.to_string_lossy().into_owned();
    // 注入固定路径，避免默认时间戳文件名导致断言困难。
    set_get_trace_file_name_for_test(Some(Arc::new({
        let filename_str = filename_str.clone();
        move || filename_str.clone()
    })));

    // 完整 Start→业务→Finish：对齐 Go TestSpan 主路径。
    let (ctx, store) = TracerStartSpan(Context::Background());
    jobA(ctx.clone());
    TracerFinishSpan(ctx, Arc::clone(&store));

    // 文件应已落盘；形状由 match_go_span_regexp 严格对齐 Go 正则。
    let content = std::fs::read_to_string(&filename).expect("trace file should exist");
    // Go: `^jobA.*2[0-9][0-9]\.[0-9]+ms\n  └─jobB.*1[0-9][0-9]\.[0-9]+ms\n$`
    assert!(
        match_go_span_regexp(&content),
        "trace content should match Go TestSpan regexp, got:\n{content}"
    );

    // Restore package-level getTraceFileName (Go defer).
    // 对齐 Go defer：还原 hook 并顺带调用一次时间戳命名，确认无残留副作用。
    set_get_trace_file_name_for_test(None);
    let _ = timestampTraceFileName();
    let _ = std::fs::remove_dir_all(&tmp);
}

// 使用 pid+纳秒构造唯一临时目录，避免串行套件内文件名冲突。
fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "br-trace-serial-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// 复刻 Go require.Regexp：要求末尾换行、两行树形前缀，且小数毫秒存在。
fn match_go_span_regexp(s: &str) -> bool {
    // Equivalent to Go require.Regexp:
    // `^jobA.*2[0-9][0-9]\.[0-9]+ms\n  └─jobB.*1[0-9][0-9]\.[0-9]+ms\n$`
    let re_line = |line: &str, prefix: &str, lo: u64, hi: u64| -> bool {
        if !line.starts_with(prefix) {
            return false;
        }
        // 行末 token 即时长；缺失则形状不完整。
        let Some(dur) = line.split_whitespace().last() else {
            return false;
        };
        let Some(num) = dur.strip_suffix("ms") else {
            return false;
        };
        // `[0-9]+` fractional part required by Go regexp (`\.` present).
        // Go 正则要求小数点后至少一位数字，纯整数 ms 不合格。
        if !num.contains('.') {
            return false;
        }
        let Ok(v) = num.parse::<f64>() else {
            return false;
        };
        // 整毫秒区间与 Go `2xx`/`1xx` 字符类一致。
        let whole = v.floor() as u64;
        whole >= lo && whole <= hi
    };

    // Go 正则锚定 `$`，要求内容以换行结束。
    if !s.ends_with('\n') {
        return false;
    }
    // 去掉末尾换行后再按行拆分，避免空尾行干扰计数。
    let body = &s[..s.len() - 1];
    let lines: Vec<&str> = body.split('\n').collect();
    if lines.len() != 2 {
        return false;
    }
    // jobA≈200ms、jobB≈100ms：分别对应父子 sleep 叠加。
    re_line(lines[0], "jobA", 200, 299) && re_line(lines[1], "  └─jobB", 100, 199)
}
