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

//! Console glue matching `br/pkg/glue/console_glue.go`.
//!
//! BR 控制台胶水层：抽象 stdin/stdout、交互提示、进度任务收尾与定宽彩色排版。
//! `ConsoleGlue` 可替换为 StdIO / Buffer / NoOP，供 CLI、测试与嵌入式 BRIE 共用。
//! `PrettyString` 剥离 ANSI 后按可见宽度切分，避免颜色序列破坏 Frame 换行。
//! `Table`/`Frame` 负责键值对齐与折行；`ExtraField` 在任务 DONE 行附加耗时等信息。
//! 默认终端宽度 80；无 libc ioctl 时 StdIO 回退该常量，与 Go 行为一致。
//! color 子模块模拟 fatih/color 的 SGR 包装，可由 NoColor 全局关闭。

use std::io::{self, IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::glue::Glue;
use crate::progressing::{OnlyOneTask, ProgressWaiter};

/// 无法探测终端宽度时的默认列数（与 Go 一致）。
pub const defaultTerminalWidth: i32 = 80;

///
/// 轻量 ANSI 着色：属性为 SGR 码；NoColor 为真时 Sprint 原样返回。
/// Minimal fatih/color-compatible helpers emitting ANSI SGR sequences.
pub mod color {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 全局禁色开关，对齐 NO_COLOR / fatih 语义。
    pub static NoColor: AtomicBool = AtomicBool::new(false);

    /// SGR 属性码别名。
    pub type Attribute = i32;

    // 文本样式属性码（1–9）。
    pub const Bold: Attribute = 1;
    pub const Faint: Attribute = 2;
    pub const Italic: Attribute = 3;
    pub const Underline: Attribute = 4;
    pub const BlinkSlow: Attribute = 5;
    pub const BlinkRapid: Attribute = 6;
    pub const ReverseVideo: Attribute = 7;
    pub const Concealed: Attribute = 8;
    pub const CrossedOut: Attribute = 9;

    // 高亮前景色 90–97。
    pub const FgHiBlack: Attribute = 90;
    pub const FgHiRed: Attribute = 91;
    pub const FgHiGreen: Attribute = 92;
    pub const FgHiYellow: Attribute = 93;
    pub const FgHiBlue: Attribute = 94;
    pub const FgHiMagenta: Attribute = 95;
    pub const FgHiCyan: Attribute = 96;
    pub const FgHiWhite: Attribute = 97;

    // 标准前景绿/红。
    pub const FgGreen: Attribute = 32;
    pub const FgRed: Attribute = 31;

    // 背景色 40–47。
    pub const BgBlack: Attribute = 40;
    pub const BgRed: Attribute = 41;
    pub const BgGreen: Attribute = 42;
    pub const BgYellow: Attribute = 43;
    pub const BgBlue: Attribute = 44;
    pub const BgMagenta: Attribute = 45;
    pub const BgCyan: Attribute = 46;
    pub const BgWhite: Attribute = 47;

    #[derive(Clone, Debug, Default)]
    /// 一组可叠加的 SGR 属性。
    pub struct Color {
        attrs: Vec<Attribute>,
    }

    /// 由属性切片构造 Color。
    pub fn New(attrs: &[Attribute]) -> Color {
        Color {
            attrs: attrs.to_vec(),
        }
    }

    impl Color {
        /// 用 ESC 序列包装文本；禁色或空属性则原样。
        pub fn Sprint(&self, s: impl AsRef<str>) -> String {
            let text = s.as_ref();
            // 禁色或无属性：不发射转义序列。
            if NoColor.load(Ordering::Relaxed) || self.attrs.is_empty() {
                return text.to_string();
            }
            // 多属性以分号拼接为单一 SGR 参数串。
            let codes = self
                .attrs
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(";");
            // ESC[codes m + 文本 + 复位。
            format!("\x1b[{codes}m{text}\x1b[0m")
        }
    }

    /// 高亮绿色便捷包装。
    pub fn HiGreenString(s: impl AsRef<str>) -> String {
        New(&[FgHiGreen]).Sprint(s)
    }

    /// 标准绿色便捷包装。
    pub fn GreenString(s: impl AsRef<str>) -> String {
        New(&[FgGreen]).Sprint(s)
    }

    /// 红色便捷包装。
    pub fn RedString(s: impl AsRef<str>) -> String {
        New(&[FgRed]).Sprint(s)
    }

    /// 高亮青色便捷包装。
    pub fn CyanString(s: impl AsRef<str>) -> String {
        New(&[FgHiCyan]).Sprint(s)
    }

    /// 高亮黑色（灰色）便捷包装。
    pub fn HiBlackString(s: impl AsRef<str>) -> String {
        New(&[FgHiBlack]).Sprint(s)
    }
}

///
/// 面向业务的控制台操作门面，持有可替换的 ConsoleGlue 实现。
/// ConsoleOperations are some operations based on ConsoleGlue.
#[derive(Clone)]
pub struct ConsoleOperations {
    /// 底层 IO/终端能力提供者。
    pub glue: Arc<dyn ConsoleGlue>,
}

impl ConsoleOperations {
    /// 注入 ConsoleGlue 构造操作门面。
    pub fn new(glue: Arc<dyn ConsoleGlue>) -> Self {
        Self { glue }
    }
}

///
/// 任务收尾附加字段：闭包返回键值字符串对。
/// An extra field appending to the task.
pub type ExtraField = Box<dyn FnMut() -> [String; 2] + Send>;

///
/// 记录首次求值耗时并缓存，key 固定为 "take"。
/// WithTimeCost adds the task information of time costing for `ShowTask`.
pub fn WithTimeCost() -> ExtraField {
    // 闭包捕获起点；首次求值写入 cached。
    let start = Instant::now();
    let mut cached = Duration::ZERO;
    Box::new(move || {
        // Go 以 0 为未缓存哨兵；四舍五入后仍为 0 时下次会重算。
        if cached.is_zero() {
            cached = round_to_millisecond(start.elapsed());
        }
        // 固定 key=take，value 为格式化时长。
        ["take".to_string(), format_duration(cached)]
    })
}

fn round_to_millisecond(d: Duration) -> Duration {
    let millis = (d.as_nanos() + 500_000) / 1_000_000;
    Duration::from_millis(millis.min(u64::MAX as u128) as u64)
}

/// 将毫秒取整后的时长格式化为接近 Go Duration.String 的文本。
pub(crate) fn format_duration(d: Duration) -> String {
    // 0→0s；整秒→Ns；不足 1s→Nms；否则带小数秒。
    // Match Go's time.Duration.String() for millisecond-rounded values.
    let ms = round_to_millisecond(d).as_millis();
    // 零耗时统一为 0s。
    if ms == 0 {
        return "0s".to_string();
    }
    // 整秒不带小数。
    if ms % 1000 == 0 {
        return format!("{}s", ms / 1000);
    }
    // 亚秒用毫秒单位。
    if ms < 1000 {
        return format!("{}ms", ms);
    }
    let secs = ms as f64 / 1000.0;
    format!("{secs}s")
}

///
/// 常量键值 ExtraField，适合静态元数据。
/// WithConstExtraField adds an extra field with constant values.
pub fn WithConstExtraField(key: impl Into<String>, value: impl ToString) -> ExtraField {
    let key = key.into();
    let rendered = value.to_string();
    Box::new(move || [key.clone(), rendered.clone()])
}

///
/// 每次求值时回调生成 value，适合动态计数。
/// WithCallbackExtraField adds an extra field with the callback.
pub fn WithCallbackExtraField(
    key: impl Into<String>,
    value: impl Fn() -> String + Send + 'static,
) -> ExtraField {
    let key = key.into();
    Box::new(move || [key.clone(), value()])
}

/// 组装绿色 DONE 行：加粗渲染各 ExtraField 的 value。
pub(crate) fn printFinalMessage(
    mut extraFields: Vec<ExtraField>,
) -> Box<dyn FnMut() -> String + Send> {
    Box::new(move || {
        let mut fields = Vec::with_capacity(extraFields.len());
        // 逐字段求值并加粗 value。
        for fieldFunc in extraFields.iter_mut() {
            let field = fieldFunc();
            fields.push(format!(
                "{} = {}",
                field[0],
                color::New(&[color::Bold]).Sprint(&field[1])
            ));
        }
        format!(
            "{} {{ {} }}",
            // DONE 高亮绿，字段列表放在花括号内。
            color::HiGreenString("DONE"),
            fields.join(", ")
        )
    })
}

impl ConsoleOperations {
    ///
    /// 启动单任务进度条；返回的闭包 Inc+Close 标记完成并打印 DONE。
    /// ShowTask prints a task start information, and mark as finished when the returned function called.
    pub fn ShowTask(
        &self,
        message: impl Into<String>,
        extraFields: Vec<ExtraField>,
    ) -> Box<dyn FnOnce() + Send> {
        // OnlyOneTask：单步进度，关闭时触发 final message。
        let bar = self.StartProgressBar(message.into(), OnlyOneTask, extraFields);
        Box::new(move || {
            // 单任务完成：推进并关闭进度条。
            bar.Inc();
            bar.Close();
        })
    }

    /// 以当前终端宽度构造根 Frame，offset=0。
    pub fn RootFrame(&self) -> Frame<'_> {
        Frame {
            width: self.GetWidth(),
            offset: 0,
            console: self,
        }
    }
}

///
/// 打印标题与列表；maxItemsDisplay>0 时截断并提示剩余数量。
/// PrintList prints a title and up to `maxItemsDisplay` items.
pub fn PrintList<T: std::fmt::Display + std::fmt::Debug>(
    ops: &ConsoleOperations,
    title: &str,
    items: &[T],
    maxItemsDisplay: i32,
) {
    // 同步打 stderr 便于观测完整 items，不依赖额外日志库。
    // Keep Go log.Info call shape via stderr for observability without an extra log crate.
    eprintln!("Print list: all items. title={title} items={items:?}");
    ops.Println(&[title]);
    // 非正 max 表示不截断。
    let limit = if maxItemsDisplay > 0 {
        std::cmp::min(items.len(), maxItemsDisplay as usize)
    } else {
        items.len()
    };
    for item in &items[..limit] {
        ops.Printf(&format!("- {item}\n"));
    }
    // 超出部分用省略提示。
    if items.len() > limit {
        ops.Printf(&format!("... and {} more ...", items.len() - limit));
    }
}

impl ConsoleOperations {
    ///
    /// 非交互直接 true；交互循环读 y/N，EOF/错误视为 false。
    /// PromptBool prompts a boolean from the user.
    pub fn PromptBool(&self, p: &str) -> bool {
        // 非 TTY：默认同意，避免嵌入式路径卡住。
        if !self.IsInteractive() {
            return true;
        }
        loop {
            let mut ans = String::new();
            self.Print(&[&format!("{p}(y/N) ")]);
            match self.Scanln(&mut ans) {
                // EOF 或读错误当作拒绝。
                Ok(0) | Err(_) => return false,
                Ok(_) => {}
            }
            let trimmed = ans.trim();
            // y/Y 同意；空或 n/N 拒绝；其它重新提示。
            if trimmed.eq_ignore_ascii_case("y") {
                return true;
            }
            // 默认 N：空输入等同拒绝。
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("n") {
                return false;
            }
        }
    }

    /// 输入是否为终端（决定是否真正提示）。
    pub fn IsInteractive(&self) -> bool {
        self.glue.in_is_terminal()
    }

    /// 读至换行并只赋值第一个空白分隔 token；返回成功赋值数。
    pub fn Scanln(&self, ans: &mut String) -> io::Result<usize> {
        let mut input = self.glue.In()?;
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        let mut ended_by_newline = false;
        loop {
            match input.read(&mut byte)? {
                0 => break,
                _ => {
                    // 遇 LF 结束一行。
                    if byte[0] == b'\n' {
                        ended_by_newline = true;
                        break;
                    }
                    buf.push(byte[0]);
                }
            }
        }
        let line = String::from_utf8_lossy(&buf);
        let mut tokens = line.split_whitespace();
        let Some(token) = tokens.next() else {
            ans.clear();
            let kind = if ended_by_newline {
                io::ErrorKind::InvalidData
            } else {
                io::ErrorKind::UnexpectedEof
            };
            return Err(io::Error::new(kind, "unexpected end of line"));
        };
        *ans = token.to_string();
        if tokens.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected newline",
            ));
        }
        Ok(1)
    }

    /// 查询终端宽度，供 Frame/Table 布局。
    pub fn GetWidth(&self) -> i32 {
        self.glue.terminal_width()
    }

    /// 创建绑定本 console 的空键值表。
    pub fn CreateTable(&self) -> Table<'_> {
        Table {
            console: self,
            items: Vec::new(),
        }
    }

    /// 无分隔拼接写出；拿不到 Out 则静默。
    pub fn Print(&self, args: &[&str]) {
        let mut out = match self.glue.Out() {
            Ok(o) => o,
            // Out 失败静默返回，避免嵌入路径 panic。
            Err(_) => return,
        };
        for a in args {
            let _ = write!(out, "{a}");
        }
        let _ = out.flush();
    }

    /// 空格分隔写出并换行。
    pub fn Println(&self, args: &[&str]) {
        let mut out = match self.glue.Out() {
            Ok(o) => o,
            Err(_) => return,
        };
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                let _ = write!(out, " ");
            }
            let _ = write!(out, "{a}");
        }
        let _ = writeln!(out);
        let _ = out.flush();
    }

    /// 写出已格式化字符串（调用方负责格式）。
    pub fn Printf(&self, formatted: &str) {
        let mut out = match self.glue.Out() {
            Ok(o) => o,
            Err(_) => return,
        };
        let _ = write!(out, "{formatted}");
        let _ = out.flush();
    }

    /// 透传底层输入流。
    pub fn In(&self) -> io::Result<Box<dyn Read + Send>> {
        self.glue.In()
    }

    /// 透传底层输出流。
    pub fn Out(&self) -> io::Result<Box<dyn Write + Send>> {
        self.glue.Out()
    }
}

/// 两列表格：左键右值，Print 时按最大键宽右对齐。
pub struct Table<'a> {
    console: &'a ConsoleOperations,
    items: Vec<[String; 2]>,
}

impl<'a> Table<'a> {
    /// 追加一行键值。
    pub fn Add(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.items.push([key.into(), value.into()]);
    }

    /// 计算键列最大字符长度，用于对齐。
    fn maxKeyLen(&self) -> usize {
        self.items
            .iter()
            .map(|item| item[0].len())
            .max()
            .unwrap_or(0)
    }

    ///
    /// 尝试左偏移 Frame；宽度不足则退化为无缩进并先空行再打值。
    /// Print prints the table.
    pub fn Print(&self) {
        let value = color::New(&[color::Bold]);
        let mut maxLen = self.maxKeyLen();
        let (f, ok) = self
            .console
            .RootFrame()
            .OffsetLeftWithMinWidth(maxLen as i32 + 2, 40);
        // 剩余宽度不够最小列宽：放弃缩进对齐。
        if !ok {
            maxLen = 0;
        }
        for item in &self.items {
            self.console
                .Printf(&format!("{:>width$}: ", item[0], width = maxLen));
            // 值加粗并以 PrettyString 交给 Frame 折行。
            let vs = NewPrettyString(value.Sprint(&item[1]));
            if !ok {
                self.console.Println(&[]);
            }
            f.Print(vs);
            self.console.Println(&[]);
        }
    }
}

///
/// 控制台 IO 抽象；默认非交互、宽度 80，实现可覆盖。
/// ConsoleGlue is the glue between BR and some type of console.
pub trait ConsoleGlue: Send + Sync {
    fn Out(&self) -> io::Result<Box<dyn Write + Send>>;
    fn In(&self) -> io::Result<Box<dyn Read + Send>>;
    /// 默认非终端输入。
    fn in_is_terminal(&self) -> bool {
        false
    }
    /// 默认非终端输出。
    fn out_is_terminal(&self) -> bool {
        false
    }
    /// 默认回退宽度。
    fn terminal_width(&self) -> i32 {
        defaultTerminalWidth
    }
}

///
/// 空输入 + sink 输出：嵌入路径丢弃所有打印。
/// NoOPConsoleGlue drops all console operations (embedded BR / BRIE via SQL).
#[derive(Default)]
pub struct NoOPConsoleGlue;

impl ConsoleGlue for NoOPConsoleGlue {
    fn In(&self) -> io::Result<Box<dyn Read + Send>> {
        // 空输入游标。
        Ok(Box::new(io::Cursor::new(Vec::<u8>::new())))
    }

    fn Out(&self) -> io::Result<Box<dyn Write + Send>> {
        // 丢弃全部输出。
        Ok(Box::new(io::sink()))
    }
}

///
/// Glue 若实现 AsConsoleGlue 则用之，否则 NoOP。
/// GetConsole returns console operations for a Glue value.
pub fn GetConsole(g: &dyn Glue) -> ConsoleOperations {
    // 优先使用 Glue 自带控制台实现。
    if let Some(cg) = g.AsConsoleGlue() {
        ConsoleOperations { glue: cg }
    } else {
        ConsoleOperations {
            glue: Arc::new(NoOPConsoleGlue),
        }
    }
}

///
/// 绑定真实 stdin/stdout；宽度暂固定默认值。
/// StdIOGlue is the console glue for CLI applications.
#[derive(Default, Clone, Copy)]
pub struct StdIOGlue;

impl ConsoleGlue for StdIOGlue {
    fn Out(&self) -> io::Result<Box<dyn Write + Send>> {
        // CLI 标准输出。
        Ok(Box::new(io::stdout()))
    }

    fn In(&self) -> io::Result<Box<dyn Read + Send>> {
        // CLI 标准输入。
        Ok(Box::new(io::stdin()))
    }

    fn in_is_terminal(&self) -> bool {
        // 探测 stdin 是否 TTY。
        io::stdin().is_terminal()
    }

    fn out_is_terminal(&self) -> bool {
        // 探测 stdout 是否 TTY。
        io::stdout().is_terminal()
    }

    /// StdIO 宽度查询入口。
    fn terminal_width(&self) -> i32 {
        #[cfg(unix)]
        {
            terminal_width_from_fd(io::stdin().as_raw_fd()).unwrap_or(defaultTerminalWidth)
        }
        #[cfg(not(unix))]
        {
            defaultTerminalWidth
        }
    }
}

#[cfg(unix)]
#[repr(C)]
struct TerminalSize {
    rows: u16,
    columns: u16,
    xpixel: u16,
    ypixel: u16,
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
const TIOCGWINSZ: usize = 0x5413;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const TIOCGWINSZ: usize = 0x4008_7468;

#[cfg(unix)]
unsafe extern "C" {
    fn ioctl(fd: i32, request: usize, ...) -> i32;
}

#[cfg(unix)]
fn terminal_width_from_fd(fd: i32) -> Option<i32> {
    let mut size = TerminalSize {
        rows: 0,
        columns: 0,
        xpixel: 0,
        ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes exactly one TerminalSize to the valid mutable pointer.
    let status = unsafe { ioctl(fd, TIOCGWINSZ, &mut size) };
    (status == 0).then_some(i32::from(size.columns))
}

///
/// 内存缓冲控制台：可预设输入并捕获输出字符串。
/// Buffer console used by tests and non-TTY callers that need capture.
#[derive(Clone, Default)]
pub struct BufferConsoleGlue {
    pub out: Arc<Mutex<Vec<u8>>>,
    pub inn: Arc<Mutex<io::Cursor<Vec<u8>>>>,
    pub width: i32,
}

impl BufferConsoleGlue {
    /// 空缓冲与默认宽度。
    pub fn new() -> Self {
        Self {
            out: Arc::new(Mutex::new(Vec::new())),
            inn: Arc::new(Mutex::new(io::Cursor::new(Vec::new()))),
            width: defaultTerminalWidth,
        }
    }

    /// 预设输入字节，供 Prompt/Scanln 测试。
    pub fn with_input(input: impl Into<Vec<u8>>) -> Self {
        let mut c = Self::new();
        c.inn = Arc::new(Mutex::new(io::Cursor::new(input.into())));
        c
    }

    /// 以有损 UTF-8 读取已捕获输出。
    pub fn output_string(&self) -> String {
        let guard = self.out.lock().expect("out lock");
        String::from_utf8_lossy(&guard).into_owned()
    }
}

/// 共享写缓冲适配 Write，供多次 Out() 追加同一 Vec。
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("out lock").write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 共享读游标适配 Read。
struct SharedReader(Arc<Mutex<io::Cursor<Vec<u8>>>>);

impl Read for SharedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.lock().expect("in lock").read(buf)
    }
}

impl ConsoleGlue for BufferConsoleGlue {
    fn Out(&self) -> io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(SharedWriter(Arc::clone(&self.out))))
    }

    fn In(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(SharedReader(Arc::clone(&self.inn))))
    }

    fn terminal_width(&self) -> i32 {
        // 测试可覆盖的缓冲控制台宽度。
        self.width
    }
}

///
/// 同时保存带色 pretty、去色 raw 与转义区间，供按可见宽度切片。
/// PrettyString is a string with ANSI escape sequence which would change its color.
#[derive(Clone, Debug, Default)]
pub struct PrettyString {
    pretty: String,
    raw: String,
    escapeSequencePlace: Vec<[usize; 2]>,
}

///
/// 扫描 ESC[ ... m 区间；非法候选放弃并前进一字节。
/// Find ANSI SGR sequences matching Go's `\x1b\[(?:(?:\d+;)*\d+)?m`.
fn find_ansi_escapes(s: &str) -> Vec<[usize; 2]> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // 识别 ESC[ 前缀开启 SGR 扫描。
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let start = i;
            i += 2;
            // Go regexp permits either an empty parameter list (`ESC[m`) or
            // one or more decimal groups separated by single semicolons.
            let mut valid = i < bytes.len() && bytes[i] == b'm';
            if valid {
                i += 1;
            } else {
                loop {
                    let group_start = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == group_start {
                        break;
                    }
                    if i < bytes.len() && bytes[i] == b'm' {
                        i += 1;
                        valid = true;
                        break;
                    }
                    if i >= bytes.len() || bytes[i] != b';' {
                        break;
                    }
                    i += 1;
                }
            }
            if valid {
                out.push([start, i]);
            } else {
                // Not a valid SGR; abandon this candidate and keep scanning.
                i = start + 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// 按区间表剔除 ANSI，得到可见 raw 文本。
fn strip_ansi(s: &str, places: &[[usize; 2]]) -> String {
    // 无转义：直接克隆。
    if places.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for p in places {
        // 拼接转义前的可见片段。
        out.push_str(&s[last..p[0]]);
        last = p[1];
    }
    out.push_str(&s[last..]);
    out
}

///
/// 解析转义并生成 raw；无转义时 raw 等于 pretty。
/// NewPrettyString wraps a string with ANSI escape sequences with PrettyString.
pub fn NewPrettyString(s: impl Into<String>) -> PrettyString {
    let pretty = s.into();
    let escapeSequencePlace = find_ansi_escapes(&pretty);
    let raw = strip_ansi(&pretty, &escapeSequencePlace);
    PrettyString {
        pretty,
        raw,
        escapeSequencePlace,
    }
}

impl PrettyString {
    /// 可见长度（raw 长度）。
    pub fn Len(&self) -> usize {
        self.raw.len()
    }

    /// 含 ANSI 的原始着色串。
    pub fn Pretty(&self) -> &str {
        &self.pretty
    }

    /// 去色后的可见串。
    pub fn Raw(&self) -> &str {
        &self.raw
    }

    ///
    /// 按可见下标 n 切开；负 n panic；越界则右半为空。
    /// SplitAt splits a pretty string at the place and ignoring all formats.
    pub fn SplitAt(&self, n: isize) -> (PrettyString, PrettyString) {
        // 负下标与 Go 一样 panic。
        if n < 0 {
            panic!(
                "PrettyString::SplitAt: index out of bound ({} vs {})",
                n,
                self.raw.len()
            );
        }
        let n = n as usize;
        // 切点不小于 raw 长：左=全文，右=空。
        if n >= self.raw.len() {
            return (
                PrettyString {
                    pretty: self.pretty.clone(),
                    raw: self.raw.clone(),
                    escapeSequencePlace: self.escapeSequencePlace.clone(),
                },
                PrettyString::default(),
            );
        }
        // 映射可见切点到 pretty 字节位置。
        let (realSlicePoint, endAt) = self.slicePointOf(n);
        let left = PrettyString {
            pretty: self.pretty[..realSlicePoint].to_string(),
            raw: self.raw[..n].to_string(),
            escapeSequencePlace: self.escapeSequencePlace[..endAt].to_vec(),
        };
        let mut right = PrettyString {
            pretty: self.pretty[realSlicePoint..].to_string(),
            raw: self.raw[n..].to_string(),
            escapeSequencePlace: Vec::new(),
        };
        // 右半转义坐标相对 realSlicePoint 重标定。
        for rp in &self.escapeSequencePlace[endAt..] {
            right
                .escapeSequencePlace
                .push([rp[0] - realSlicePoint, rp[1] - realSlicePoint]);
        }
        (left, right)
    }

    /// 将可见下标映射到 pretty 字节下标。
    fn slicePointOf(&self, s: usize) -> (usize, usize) {
        let mut endAt = 0;
        let mut realSlicePoint = s;
        for (i, m) in self.escapeSequencePlace.iter().enumerate() {
            let start = m[0];
            let end = m[1];
            let length = end - start;
            // 切点落在下一转义之前：可直接返回。
            if realSlicePoint <= start {
                endAt = i;
                return (realSlicePoint, endAt);
            }
            // 跳过转义长度，使可见下标与 pretty 对齐。
            realSlicePoint += length;
        }
        (realSlicePoint, endAt)
    }
}

///
/// 固定宽度打印区域：支持左偏移与按宽折行。
/// Frame is a fix-width place for printing.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    offset: i32,
    width: i32,
    console: &'a ConsoleOperations,
}

impl<'a> Frame<'a> {
    /// 换行并补齐 offset 空格，保持缩进列。
    fn newLine(&self) {
        self.console
            .Printf(&format!("\n{}", " ".repeat(self.offset.max(0) as usize)));
    }

    /// 按 width 循环 SplitAt 打印；width<=0 则整串一次写出。
    pub fn Print(&self, s: PrettyString) {
        // 无有效宽度：不做折行。
        if self.width <= 0 {
            self.console.Print(&[s.Pretty()]);
            return;
        }
        let mut left;
        let mut right;
        (left, right) = s.SplitAt(self.width as isize);
        // 循环切段直至剩余为空。
        while left.Len() > 0 {
            self.console.Print(&[left.Pretty()]);
            // 仍有剩余则换行缩进再继续。
            if right.Len() > 0 {
                self.newLine();
            }
            let next = right.SplitAt(self.width as isize);
            left = next.0;
            right = next.1;
        }
    }

    /// 克隆 Frame 并覆盖宽度。
    pub fn WithWidth(&self, width: i32) -> Frame<'a> {
        Frame {
            offset: self.offset,
            width,
            console: self.console,
        }
    }

    /// 左移 offset 列；剩余宽不足 minWidth 则失败。
    pub fn OffsetLeftWithMinWidth(&self, offset: i32, minWidth: i32) -> (Frame<'a>, bool) {
        // 剩余空间不足最小宽度：调用方应退化布局。
        if self.width - offset < minWidth {
            return (*self, false);
        }
        (
            Frame {
                offset,
                width: self.width - offset,
                console: self.console,
            },
            true,
        )
    }

    /// 等价于最小宽度为 1 的左偏移。
    pub fn OffsetLeft(&self, offset: i32) -> (Frame<'a>, bool) {
        self.OffsetLeftWithMinWidth(offset, 1)
    }
}
