// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Go-equivalent tests for `br/pkg/glue/console_glue_test.go`.
//!
//! 本文件对齐 Go `console_glue_test.go`：验证彩色 TUI 辅助类型在
//! 关 `NoColor` 后的 PrettyString 长度、按 raw 位置切片，以及固定宽度
//! Frame 换行时 ANSI 跨度与左缩进换行是否与 Go 一致。
//! 约束：只改注释与断言说明，不得改动测试输入、期望串或回调副作用。

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::console_glue::{
    ConsoleGlue, ConsoleOperations, NewPrettyString, NoOPConsoleGlue, PrettyString, color,
    format_duration,
};

/// TestColorfulTUIFunctions — Go entry: disable NoColor then run PrettyString / Frame subtests.
/// 入口测试：强制启用着色后再串行跑三个子场景，避免 CI 无 TTY 时 NoColor 掩盖断言。
#[test]
fn test_colorful_tui_functions() {
    // when testing, the teriminal would be redirected to the dummy terminal.
    // 测试环境终端常被重定向；显式关掉 NoColor，保证 Sprint 产出 ANSI。
    color::NoColor.store(false, std::sync::atomic::Ordering::Relaxed);

    test_pretty_string();
    test_pretty_string_slicing();
    test_print_frame();
}

/// testPrettyString — attribute combinations; raw length ignores ANSI.
/// 笛卡尔积覆盖前景/背景/字体属性；`-1` 表示该维跳过，与 Go 测试一致。
/// 断言依据：PrettyString.Len 只计 raw 可见字符，不受转义序列长度影响。
fn test_pretty_string() {
    let fg_attrs: Vec<color::Attribute> = vec![
        -1, // -1 means skip this type of attr.
        // -1：跳过该维属性，用于测「无前景/无背景/无字体」组合。
        color::FgHiBlack,
        color::FgHiRed,
        color::FgHiGreen,
        color::FgHiYellow,
        color::FgHiBlue,
        color::FgHiMagenta,
        color::FgHiCyan,
        color::FgHiWhite,
    ];
    // 背景色维同样以 -1 表示「不设置」，保证与前景/字体可独立组合。
    let bg_attrs: Vec<color::Attribute> = vec![
        -1,
        color::BgBlack,
        color::BgRed,
        color::BgGreen,
        color::BgYellow,
        color::BgBlue,
        color::BgMagenta,
        color::BgCyan,
        color::BgWhite,
    ];
    // 字体样式维；Blink/Concealed 等在部分终端无可见效果，但仍应不计入 Len。
    let font_attrs: Vec<color::Attribute> = vec![
        -1,
        color::Bold,
        color::Faint,
        color::Italic,
        color::Underline,
        color::BlinkSlow,
        color::BlinkRapid,
        color::ReverseVideo,
        color::Concealed,
        color::CrossedOut,
    ];

    // 固定原文 "hello, world"：raw 长度恒为 12，用于隔离属性对 Len 的干扰。
    let run_test = |c: &color::Color| {
        let ps = NewPrettyString(c.Sprint("hello, world"));
        assert_eq!(ps.Len(), 12, "{} vs {}", ps.Pretty(), ps.Raw());
    };

    // 三重循环对齐 Go：任意属性子集下 Len 仍等于裸文本长度。
    for fg in &fg_attrs {
        for bg in &bg_attrs {
            for ft in &font_attrs {
                let mut attrs = Vec::with_capacity(3);
                for attr in [*fg, *bg, *ft] {
                    if attr != -1 {
                        attrs.push(attr);
                    }
                }
                run_test(&color::New(&attrs));
            }
        }
    }
}

/// testPrettyStringSlicing — SplitAt on raw positions; Pretty/Raw stay consistent.
/// SplitAt 按 raw 字节偏移切开；再经 NewPrettyString 回解析后 Raw 必须复原。
fn test_pretty_string_slicing() {
    // 两段不同颜色拼接，切开点可能落在颜色边界两侧。
    let bs = NewPrettyString(format!(
        "{}{}",
        color::HiBlackString("hello, world"),
        color::CyanString(", and my friend")
    ));

    // 内部一致性：Pretty 再解析得到的 Raw 必须等于原片段 Raw。
    let check_internal_consistency = |ss: &[PrettyString]| {
        for s in ss {
            let sp = NewPrettyString(s.Pretty().to_string());
            assert_eq!(sp.Raw(), s.Raw(), "{ss:?}");
        }
    };

    // 左右片段的 Raw 拼接应等于原串；n 为 raw 下标而非带 ANSI 的 Pretty 下标。
    let test_split = |s: PrettyString, n: isize| -> (PrettyString, PrettyString) {
        let raw = s.Raw().to_string();
        let (left, right) = s.SplitAt(n);
        assert_eq!(
            left.Raw(),
            &raw[..n as usize],
            "{s:?}(@{n}) -> ({left:?} {right:?})"
        );
        assert_eq!(
            right.Raw(),
            &raw[n as usize..],
            "{s:?}(@{n}) -> ({left:?} {right:?})"
        );
        check_internal_consistency(&[left.clone(), right.clone()]);
        (left, right)
    };

    // 覆盖段内切、跨颜色切、以及整段边界切，与 Go 子用例顺序一致。
    let _ = test_split(bs.clone(), 5);
    let (l, r) = test_split(bs.clone(), 15);
    let _ = test_split(l, 5);
    let _ = test_split(r, 3);
    let _ = test_split(bs, 12);
}

/// writerGlue — embeds NoOPConsoleGlue, overrides Out() to a shared buffer (Go test helper).
/// 测试替身：Out 写入共享缓冲以便断言 Frame 输出；In 复用 NoOP。
struct WriterGlue {
    // 多线程 Frame 写入时通过 Mutex 串行化缓冲。
    // 与 Go 测试里自定义 ConsoleGlue 捕获 stdout 的意图相同。
    w: Arc<Mutex<Vec<u8>>>,
}

// WriterGlue 只覆盖 Out；其余 ConsoleGlue 行为保持最小替身。
impl ConsoleGlue for WriterGlue {
    // 每次 Out 返回包装同一缓冲的 Write，模拟可捕获的控制台。
    fn Out(&self) -> io::Result<Box<dyn Write + Send>> {
        struct Shared(Arc<Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().expect("out lock").write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        Ok(Box::new(Shared(Arc::clone(&self.w))))
    }

    // 输入侧无交互需求，直接委托 NoOPConsoleGlue。
    fn In(&self) -> io::Result<Box<dyn Read + Send>> {
        NoOPConsoleGlue.In()
    }
}

/// testPrintFrame — fixed-width Frame keeps color spans and inserts indent newlines.
/// 宽度 10、左偏移 10：换行后插入与偏移等长空白，且颜色段在断行处被切开重贴。
fn test_print_frame() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let ops = ConsoleOperations::new(Arc::new(WriterGlue {
        w: Arc::clone(&buf),
    }));
    // OffsetLeft 失败则无法构造有效 Frame；与 Go 一样要求 ok。
    let (mut f, ok) = ops.RootFrame().OffsetLeft(10);
    assert!(ok);
    f = f.WithWidth(10);
    // 三段颜色串接，期望输出按宽度切分并在换行后补缩进。
    let bs = NewPrettyString(format!(
        "{}{}{}",
        color::HiGreenString("hello, world"),
        color::CyanString(", and my friend"),
        color::RedString(", and all good people.")
    ));
    // 左偏移 10 空格：每行续写前都必须重新打印缩进，对齐 Go Frame 语义。
    let indent = " ".repeat(10);
    /*
        hello, wor
        ld, and my
         friend, a
        nd all goo
        d people.
    */
    // expected 手工拼出与 Go 金标准一致的带缩进换行与颜色边界。
    // 绿/青/红三段各自在断行处闭合再开启，避免颜色泄漏到下一物理行。
    let expected = format!(
        "{}{}{}",
        color::HiGreenString(format!("hello, wor\n{indent}ld")),
        color::CyanString(format!(", and my\n{indent} friend")),
        color::RedString(format!(", a\n{indent}nd all goo\n{indent}d people."))
    );
    // Print 将 PrettyString 按 Frame 宽度排版写入 WriterGlue 缓冲。
    f.Print(bs);
    let got = String::from_utf8_lossy(&buf.lock().expect("out lock")).into_owned();
    // 整串精确相等：既校验换行位置，也校验 ANSI 跨度未被破坏。
    assert_eq!(expected, got);
}

/// Go `fmt.Fscanln` with one destination reads the first whitespace-delimited token.
#[test]
fn scanln_reads_one_token_and_reports_one_assignment() {
    let glue = crate::console_glue::BufferConsoleGlue::with_input(b"  y  \n".to_vec());
    let ops = ConsoleOperations::new(Arc::new(glue));
    let mut answer = String::new();

    let assigned = ops.Scanln(&mut answer).expect("scan token");

    assert_eq!(assigned, 1);
    assert_eq!(answer, "y");
}

#[test]
fn scanln_rejects_extra_tokens_after_the_destination() {
    let glue = crate::console_glue::BufferConsoleGlue::with_input(b"y extra\n".to_vec());
    let ops = ConsoleOperations::new(Arc::new(glue));
    let mut answer = String::new();

    assert!(ops.Scanln(&mut answer).is_err());
    assert_eq!(answer, "y");
}

#[test]
fn time_cost_rounds_to_the_nearest_millisecond() {
    assert_eq!(format_duration(Duration::from_micros(1_600)), "2ms");
    assert_eq!(format_duration(Duration::from_micros(1_400)), "1ms");
}

/// Go's ANSI regexp accepts an empty parameter list (`ESC[m`), but rejects a
/// leading semicolon because every semicolon must follow at least one digit.
#[test]
fn pretty_string_matches_go_ansi_regexp_grammar() {
    let pretty = NewPrettyString("a\x1b[mb\x1b[;mc");

    assert_eq!(pretty.Raw(), "ab\x1b[;mc");
    assert_eq!(pretty.Len(), 7);
}
