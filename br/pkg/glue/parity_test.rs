// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/glue` public contracts vs Go sources.
//!
//! 汇总校验 glue / console_glue / progressing 的公开契约是否与 Go 侧语义对齐。
//! 覆盖彩色字符串、Frame 排版、列表截断、非 TTY 提示、WithProgress 生命周期，
//! 以及客户端常量与可构造性；失败即视为 Rust 移植偏离 Go 行为。
//! 约束：本文件只解释意图与断言依据，不得改输入向量或期望文案。

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use astersql_errors::SharedError;

use crate::console_glue::{
    BufferConsoleGlue, ConsoleGlue, ConsoleOperations, GetConsole, NewPrettyString,
    NoOPConsoleGlue, PrintList, StdIOGlue, WithCallbackExtraField, WithConstExtraField,
    WithTimeCost, color,
};
use crate::glue::{
    BatchCreateTableSession, ClientCLP, ClientSql, Context, CreateTableOption, Domain, Glue,
    GlueClient, Progress, SecurityOption, Session, Storage, TableInfo, WithProgress,
};
use crate::progressing::{MultiProgress, OnlyOneTask, ProgressWaiter};

// 进度替身：用原子计数记录 Inc，并用共享标志观察 Close 是否被调用。
struct MockProgress {
    current: AtomicU64,
    // SeqCst 标志供断言错误/成功路径都执行了 Close。
    closed: Arc<AtomicBool>,
}

impl Progress for MockProgress {
    // Inc 委托 IncBy(1)，与真实进度条语义一致。
    fn Inc(&self) {
        self.IncBy(1);
    }
    // 允许一次跳多项，供成功路径累计到 3。
    fn IncBy(&self, cnt: i64) {
        self.current.fetch_add(cnt as u64, Ordering::Relaxed);
    }
    // 供断言读取当前进度。
    fn GetCurrent(&self) -> i64 {
        self.current.load(Ordering::Relaxed) as i64
    }
    // Close 只翻标志，不依赖真实 TTY/mpb。
    fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

// Glue 替身：仅实现 StartProgress / 常量路径，其余返回 not implemented。
struct MockGlue {
    closed_progress: Arc<AtomicBool>,
}

// Go's BatchCreateTableSession is independent and does not embed Session.
struct BatchOnlySession;

impl BatchCreateTableSession for BatchOnlySession {
    fn CreateTables(
        &mut self,
        _ctx: Context,
        _tables: HashMap<String, Vec<TableInfo>>,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        Ok(())
    }
}

#[derive(Clone)]
struct TtyBufferGlue {
    inner: BufferConsoleGlue,
}

impl ConsoleGlue for TtyBufferGlue {
    fn Out(&self) -> io::Result<Box<dyn Write + Send>> {
        self.inner.Out()
    }

    fn In(&self) -> io::Result<Box<dyn Read + Send>> {
        self.inner.In()
    }

    fn out_is_terminal(&self) -> bool {
        true
    }

    fn terminal_width(&self) -> i32 {
        self.inner.width
    }
}

impl Glue for MockGlue {
    // Domain/Session/Open 本用例不走到，统一返回错误以暴露误调用。
    fn GetDomain(&self, _store: &dyn Storage) -> Result<Arc<Domain>, SharedError> {
        Err(astersql_errors::New("not implemented"))
    }

    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        Err(astersql_errors::New("not implemented"))
    }

    fn Open(&self, _path: &str, _option: SecurityOption) -> Result<Box<dyn Storage>, SharedError> {
        Err(astersql_errors::New("not implemented"))
    }

    // CLI 路径通常拥有 Open 返回的存储。
    fn OwnsStorage(&self) -> bool {
        true
    }

    // 把 closed_progress 注入 MockProgress，供 WithProgress 断言生命周期。
    fn StartProgress(
        &self,
        _ctx: Context,
        _cmdName: &str,
        _total: i64,
        _redirectLog: bool,
    ) -> Box<dyn Progress> {
        Box::new(MockProgress {
            current: AtomicU64::new(0),
            closed: Arc::clone(&self.closed_progress),
        })
    }

    // 指标记录空操作即可。
    fn Record(&self, _name: &str, _value: u64) {}

    // 固定版本串，避免依赖真实二进制版本。
    fn GetVersion(&self) -> String {
        "test-version".to_string()
    }

    // 空实现即可：本测试不调用一次性会话。
    fn UseOneShotSession(
        &self,
        _store: &dyn Storage,
        _closeDomain: bool,
        _fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        Ok(())
    }

    // 固定为 CLI 客户端，便于与 ClientCLP 常量对照。
    fn GetClient(&self) -> GlueClient {
        ClientCLP
    }
}

/// 单一大用例串起多场景，避免分散测试重复构造 BufferConsoleGlue。
#[test]
fn go_rust_public_contract_matches() {
    // 与 console_glue_test 相同：强制着色，避免 CI 无 TTY 时跳过 ANSI 路径。
    color::NoColor.store(false, Ordering::Relaxed);

    // normal: PrettyString raw length ignores ANSI (TestPrettyString)
    // 正常路径：着色后 Len/Raw 仍对应裸文本 "hello, world"。
    let ps = NewPrettyString(color::New(&[color::FgHiGreen, color::Bold]).Sprint("hello, world"));
    assert_eq!(ps.Len(), 12, "{} vs {}", ps.Pretty(), ps.Raw());
    assert_eq!(ps.Raw(), "hello, world");

    // boundary: SplitAt keeps raw slices and internal consistency (TestPrettyStringSlicing)
    // 边界：按 raw 下标切开后左右片段可再解析且 Raw 复原。
    let bs = NewPrettyString(format!(
        "{}{}",
        color::HiBlackString("hello, world"),
        color::CyanString(", and my friend")
    ));
    // Pretty→NewPrettyString 往返后 Raw 不变即内部一致。
    let check = |s: &crate::console_glue::PrettyString| {
        let sp = NewPrettyString(s.Pretty().to_string());
        assert_eq!(sp.Raw(), s.Raw());
    };
    // n 必须相对 Raw，不能按带转义的 Pretty 长度切。
    let split = |s: crate::console_glue::PrettyString, n: isize| {
        let raw = s.Raw().to_string();
        let (left, right) = s.SplitAt(n);
        assert_eq!(left.Raw(), &raw[..n as usize]);
        assert_eq!(right.Raw(), &raw[n as usize..]);
        check(&left);
        check(&right);
        (left, right)
    };
    // 覆盖段内、跨颜色、边界切点，顺序对齐 Go 子用例。
    let _ = split(bs.clone(), 5);
    let (l, r) = split(bs.clone(), 15);
    let _ = split(l, 5);
    let _ = split(r, 3);
    let _ = split(bs, 12);

    // normal: Frame wraps at width with left offset (TestPrintFrame)
    // Frame：宽 10 + 左偏移 10，输出应含截断片段与缩进换行。
    let buf = BufferConsoleGlue::new();
    let ops = ConsoleOperations::new(Arc::new(buf.clone()));
    let (mut f, ok) = ops.RootFrame().OffsetLeft(10);
    assert!(ok);
    f = f.WithWidth(10);
    let text = NewPrettyString(format!(
        "{}{}{}",
        color::HiGreenString("hello, world"),
        color::CyanString(", and my friend"),
        color::New(&[color::FgHiRed]).Sprint(", and all good people.")
    ));
    f.Print(text);
    let out = buf.output_string();
    // 首段应在 "hello, wor" 处断行。
    assert!(out.contains("hello, wor"), "frame output: {out:?}");
    // 换行后应有 10 个空格缩进，对应 OffsetLeft(10)。
    assert!(
        out.contains("\n          "),
        "expected indent newline: {out:?}"
    );

    // boundary: PrintList truncation
    // 列表截断：limit=2 时只打印 a/b，并提示剩余 2 项。
    let buf = BufferConsoleGlue::new();
    let ops = ConsoleOperations::new(Arc::new(buf.clone()));
    PrintList(&ops, "items", &["a", "b", "c", "d"], 2);
    let out = buf.output_string();
    assert!(out.contains("items"));
    assert!(out.contains("- a"));
    assert!(out.contains("- b"));
    // c 不应出现；省略提示文案对齐 Go。
    assert!(!out.contains("- c"));
    assert!(out.contains("... and 2 more ..."));

    // error / non-interactive: PromptBool returns true when not a TTY
    // 非交互：无 TTY 时 PromptBool 默认 true，避免自动化卡住。
    let buf = BufferConsoleGlue::with_input(b"");
    let ops = ConsoleOperations::new(Arc::new(buf));
    assert!(ops.PromptBool("continue?"));

    // resource / lifecycle: WithProgress always closes even on callback error
    // 资源：回调出错时仍必须 Close，防止进度条泄漏。
    let closed = Arc::new(AtomicBool::new(false));
    let g = MockGlue {
        closed_progress: Arc::clone(&closed),
    };
    let err = WithProgress(Context::new(), &g, "cmd", 10, true, |_p| {
        Err(astersql_errors::New("boom"))
    });
    assert!(err.is_err());
    assert!(
        closed.load(Ordering::SeqCst),
        "progress must Close on error path"
    );

    // success path also closes
    // 成功路径同样 Close，并用 Inc/IncBy 校验累计值。
    let closed = Arc::new(AtomicBool::new(false));
    let g = MockGlue {
        closed_progress: Arc::clone(&closed),
    };
    WithProgress(Context::new(), &g, "cmd", 3, true, |p| {
        p.Inc();
        p.IncBy(2);
        assert_eq!(p.GetCurrent(), 3);
        Ok(())
    })
    .expect("with progress ok");
    assert!(closed.load(Ordering::SeqCst));

    // GetConsole falls back to NoOP when Glue has no console
    // MockGlue 未实现 AsConsoleGlue 时回退 NoOP，输出应被丢弃。
    let g = MockGlue {
        closed_progress: Arc::new(AtomicBool::new(false)),
    };
    let console = GetConsole(&g);
    console.Println(&["should be discarded"]);

    // ShowTask + dummy progress (non-TTY) Inc/Close lifecycle
    // 非 TTY ShowTask：额外字段与耗时回调可挂接，done() 结束任务。
    let buf = BufferConsoleGlue::new();
    let ops = ConsoleOperations::new(Arc::new(buf));
    let done = ops.ShowTask(
        "one-task",
        vec![
            WithConstExtraField("k", "v"),
            WithCallbackExtraField("cb", || "x".to_string()),
            WithTimeCost(),
        ],
    );
    done();

    // MultiProgress non-TTY LogBar countdown
    // 非 TTY 多进度：AddTextBar 递增满额后 Wait 收尾。
    let buf = BufferConsoleGlue::new();
    let ops = ConsoleOperations::new(Arc::new(buf));
    let mp = ops.StartMultiProgress();
    let bar = mp.AddTextBar("load", 2);
    // total=2：两次 Increment 后条完成。
    bar.Increment();
    bar.Increment();
    // Wait 写出完成行并关闭组，对齐 Go MultiProgress.Wait。
    mp.Wait();

    // client constants match Go iota
    // 常量与 Go iota / OnlyOneTask=-1 对齐。
    assert_eq!(ClientCLP, 0);
    assert_eq!(ClientSql, 1);
    assert_eq!(OnlyOneTask, -1);

    // StdIO / NoOP glue constructible
    // 冒烟：标准 IO 与 NoOP 控制台、默认 TLS 选项可构造。
    let _ = StdIOGlue;
    let _ = NoOPConsoleGlue;
    let _ = SecurityOption::default();

    // BatchCreateTableSession must be implementable without the unrelated Session API.
    let mut batch = BatchOnlySession;
    let batch_api: &mut dyn BatchCreateTableSession = &mut batch;
    batch_api
        .CreateTables(Context::new(), HashMap::new(), Vec::new())
        .expect("batch-only session");
}

#[test]
fn progress_waiter_blocks_until_the_bar_finishes() {
    let buffer = BufferConsoleGlue::new();
    let ops = ConsoleOperations::new(Arc::new(TtyBufferGlue {
        inner: buffer.clone(),
    }));
    let waiter: Arc<dyn ProgressWaiter> = Arc::from(ops.StartProgressBar("load", 2, Vec::new()));
    let wait_in_thread = Arc::clone(&waiter);
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        tx.send(wait_in_thread.Wait(Context::new()))
            .expect("send wait result");
    });

    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    waiter.Inc();
    waiter.Inc();
    rx.recv_timeout(Duration::from_secs(1))
        .expect("wait should finish")
        .expect("wait result");
    assert!(buffer.output_string().contains("DONE"));
}

#[test]
fn terminal_bar_done_is_rendered_only_once() {
    let buffer = BufferConsoleGlue::new();
    let ops = ConsoleOperations::new(Arc::new(TtyBufferGlue {
        inner: buffer.clone(),
    }));
    let progress = ops.StartMultiProgress();
    let bar = progress.AddTextBar("load", 2);

    bar.Done();
    progress.Wait();

    assert_eq!(buffer.output_string().matches("ABORTED").count(), 1);
}

#[test]
fn with_progress_closes_during_unwinding() {
    let closed = Arc::new(AtomicBool::new(false));
    let glue = MockGlue {
        closed_progress: Arc::clone(&closed),
    };

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = WithProgress(Context::new(), &glue, "cmd", 1, true, |_progress| {
            panic!("callback panic")
        });
    }));

    assert!(panic.is_err());
    assert!(closed.load(Ordering::SeqCst));
}

/// Go exports these concrete progress implementations from the package root.
/// Keep the Rust crate-root facade equivalent instead of requiring callers to
/// know about the port-only `progressing` module split.
#[test]
fn crate_root_exports_go_public_progress_types() {
    fn assert_public<T>() {}

    assert_public::<crate::TerminalBar>();
    assert_public::<crate::TerminalMultiProgress>();
}
