// Copyright 2026 AsterSQL.
//! Local stand-ins for go-build / file / flag / clock boundaries (arm64-safe).
//!
//! Mirrors the external surfaces `cmd/pluginpkg` needs from `os/exec`,
//! filesystem IO, and wall-clock time without pulling heavy TiDB crates.
//!
//!
//! - 这个文件不是业务逻辑本体，而是 `pluginpkg` 主流程依赖的“系统边界替身”。
//! - Go 原版直接调用 `os`、`exec`、`flag` 和 `time`，Rust 版本把这些能力拆成可注入接口。
//! - 这样做的目标不是抽象得更通用，而是为了在不拉入整套 TiDB 依赖的情况下复刻行为。
//! - `stubs` 同时承担生产实现和测试假实现两类职责，因此注释要特别说明哪些是桩、哪些是真实系统调用。
//! - 文件里的类型大多围绕“路径处理、文件读写、命令执行、帮助退出、时间戳”几个边界展开。
//! - 这里并不试图实现完整的 Go 标准库语义，只覆盖 `cmd/pluginpkg` 当前实际使用到的子集。
//! - 对齐重点是控制流和错误文本，而不是把 Rust API 包装成一模一样的 Go 形状。
//! - 由于该模块被主流程和对照测试同时依赖，任何看似无害的行为变化都可能破坏 Go parity。

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Error type standing in for OS / exec failures at the pluginpkg boundary.
///
/// - 这里统一承接文件系统和子进程层面的失败，避免在上层逻辑里混用不同错误类型。
/// - `is_exit` 不表示真实进程已经退出，而是标记该错误在 Go 语义上对应非零退出。
/// - 这样测试可以区分“普通 IO 失败”和“命令失败导致应当退出”两类分支。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    pub is_exit: bool,
}

impl Error {
    ///
    /// - 构造普通错误，不携带“退出态”语义。
    /// - 用于模拟 Go 中返回 `error` 但还没有走到 `os.Exit` 的场景。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            is_exit: false,
        }
    }

    ///
    /// - 构造与 Go `exit status ...` 类似的错误。
    /// - 上层仍会决定是否转成真正的 `fatal_exit`，这里仅保留原因分类。
    pub fn exit(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            is_exit: true,
        }
    }

    ///
    /// - 保留 Go 风格的 `Error()` 访问器，便于迁移代码和对照测试直接取消息文本。
    /// - 这里返回借用字符串，不额外分配。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self {
            msg: e.to_string(),
            is_exit: false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Process-fatal exit matching Go `os.Exit(1)` / `flag.Usage`.
/// Catchable in tests via `std::panic::catch_unwind`.
///
/// - 生产语义上这里代表“立即终止流程”，对应 Go 的 `os.Exit(1)`。
/// - Rust 测试中不能真的结束整个测试进程，所以改用带前缀的 panic 来承载退出信号。
/// - 约定前缀 `pluginpkg-exit:` 让测试能稳定识别这是受控退出，而不是意外 panic。
pub fn fatal_exit(msg: impl AsRef<str>) -> ! {
    #[cfg(test)]
    panic!("pluginpkg-exit: {}", msg.as_ref());

    #[cfg(not(test))]
    {
        let _ = msg;
        std::process::exit(1);
    }
}

/// Captured stdout / log writers for tests.
///
/// - Go 测试常直接观察 `Stdout`/`Stderr` 文本；Rust 这里用内存缓冲区完成同类断言。
/// - `Capture` 既可当 `Write` 使用，也能在断言阶段回放累计输出。
#[derive(Clone, Default)]
pub struct Capture {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl Capture {
    ///
    /// - 返回空缓冲写入器，便于测试分别捕获日志流和标准输出流。
    pub fn new() -> Self {
        Self::default()
    }

    ///
    /// - 复制当前缓冲区内容，避免调用方持锁跨越后续断言逻辑。
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().unwrap().clone()
    }

    ///
    /// - 以宽松 UTF-8 方式读取输出，和命令行日志场景更贴近。
    /// - 非 UTF-8 字节会被替换，而不是让测试在解码阶段失败。
    pub fn string(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }

    ///
    /// - 允许单个 `Capture` 在多个测试步骤里重复使用，避免重新装配依赖。
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// CLI flags matching Go `flag.StringVar` / `flag.BoolVar` registrations.
///
/// - 字段集合严格对应 Go 命令注册的四个 flag。
/// - 这里保持扁平结构，方便解析后直接传给主流程，不引入额外配置层。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub pkg_dir: String,
    pub out_dir: String,
    pub pgo_file: String,
    pub next_gen: bool,
}

/// Parse argv like Go `flag` for pluginpkg options (skip argv0).
///
/// - 这里只解析 `pluginpkg` 真正使用到的参数形式，不尝试复刻完整 Go `flag` 包。
/// - 支持 `--x=y`、`-x=y` 以及值放在下一参数位这几种写法。
/// - 遇到首个位置参数或 `--` 后停止解析，保持 Go `flag` 的顺序语义。
/// - 未识别参数、缺失参数值和非法布尔值按 Go `flag.ExitOnError` 终止。
pub fn parse_flags(args: &[String]) -> Flags {
    match try_parse_flags(args) {
        Ok(flags) => flags,
        Err(message) => flag_parse_exit(message),
    }
}

pub fn try_parse_flags(args: &[String]) -> std::result::Result<Flags, String> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" || a == "-" || !a.starts_with('-') {
            break;
        }
        if a.starts_with("---") {
            return Err(format!("bad flag syntax: {a}"));
        }
        if matches!(a, "-h" | "--h" | "-help" | "--help") {
            return Err(String::new());
        }
        if let Some(v) = strip_flag(a, "pkg-dir") {
            flags.pkg_dir = v;
            i += 1;
            continue;
        }
        if a == "-pkg-dir" || a == "--pkg-dir" {
            if let Some(v) = args.get(i + 1) {
                flags.pkg_dir = v.clone();
                i += 2;
                continue;
            }
            return Err("flag needs an argument: -pkg-dir".to_string());
        }
        if let Some(v) = strip_flag(a, "out-dir") {
            flags.out_dir = v;
            i += 1;
            continue;
        }
        if a == "-out-dir" || a == "--out-dir" {
            if let Some(v) = args.get(i + 1) {
                flags.out_dir = v.clone();
                i += 2;
                continue;
            }
            return Err("flag needs an argument: -out-dir".to_string());
        }
        if let Some(v) = strip_flag(a, "pgo-file") {
            flags.pgo_file = v;
            i += 1;
            continue;
        }
        if a == "-pgo-file" || a == "--pgo-file" {
            if let Some(v) = args.get(i + 1) {
                flags.pgo_file = v.clone();
                i += 2;
                continue;
            }
            return Err("flag needs an argument: -pgo-file".to_string());
        }
        if matches!(a, "-next-gen" | "--next-gen") {
            flags.next_gen = true;
            i += 1;
            continue;
        }
        if a.starts_with("-next-gen=") || a.starts_with("--next-gen=") {
            let value = a
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or_default();
            flags.next_gen = match value {
                "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                _ => {
                    return Err(format!(
                        "invalid boolean value {value:?} for -next-gen: parse error"
                    ));
                }
            };
            i += 1;
            continue;
        }
        let name = a
            .trim_start_matches('-')
            .split_once('=')
            .map_or_else(|| a.trim_start_matches('-'), |(name, _)| name);
        return Err(format!("flag provided but not defined: -{name}"));
    }
    Ok(flags)
}

fn flag_parse_exit(message: String) -> ! {
    #[cfg(test)]
    panic!("pluginpkg-flag-exit: {message}");

    #[cfg(not(test))]
    {
        eprintln!("{message}");
        std::process::exit(2);
    }
}

fn strip_flag(arg: &str, name: &str) -> Option<String> {
    //
    // - 统一处理 `-name=value` 与 `--name=value` 两种内联赋值形式。
    // - 返回 `String` 是为了让调用方直接写入结果结构体，不再保留对原参数切片的借用。
    let prefixes = [format!("-{name}="), format!("--{name}=")];
    for p in &prefixes {
        if let Some(rest) = arg.strip_prefix(p.as_str()) {
            return Some(rest.to_string());
        }
    }
    None
}

/// Args from process environment (skip argv0).
///
/// - 与 Go `os.Args[1:]` 对齐，只返回业务参数，不包含程序名本身。
pub fn args_from_env() -> Vec<String> {
    env::args().skip(1).collect()
}

/// A recorded `go` subprocess invocation.
///
/// - 测试不会真的执行 `go build`，而是把调用快照记录成这个结构体。
/// - 其中 `dir` 和 `env` 很关键，因为它们决定了构建语义是否与 Go 原版一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub dir: String,
    pub env: Vec<String>,
}

/// Go `os` file + path helpers used by plugin packaging.
///
/// - 这个接口抽出的是 `pluginpkg` 所需的最小文件系统能力集合。
/// - 既包含真实读写，也包含 `base/join/abs` 这类会影响输出路径的路径运算。
/// - 默认方法直接复用 `std::path`，让内存实现只需关注与状态相关的几个原语。
pub trait Fs {
    fn abs(&self, path: &str) -> Result<String>;
    fn read_to_string(&self, path: &str) -> Result<String>;
    fn write(&self, path: &str, data: &[u8], mode: u32) -> Result<()>;
    fn remove(&self, path: &str) -> Result<()>;
    fn base(&self, path: &str) -> String {
        Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    fn join(&self, a: &str, b: &str) -> String {
        Path::new(a).join(b).to_string_lossy().into_owned()
    }
}

/// Wall clock for `time.Now().String()`.
///
/// - 主流程只关心“拿到一段可写入 manifest 的当前时间文本”，不需要完整时间 API。
/// - 这种窄接口让测试可以精确断言生成结果，而不受真实时钟波动影响。
pub trait Clock {
    fn now_string(&self) -> String;
}

/// Go `exec.CommandContext(...).Run()` surface for `go build`.
///
/// - 这里不是通用命令执行器，而是专门承接 `go build` 这一条调用链。
/// - `env_extra` 采用附加变量列表，匹配 Go 代码里 `append(os.Environ(), ...)` 的写法。
pub trait Runner {
    fn run(&self, program: &str, args: &[String], dir: &str, env_extra: &[String]) -> Result<()>;
}

/// In-memory FS for parity tests (no real disk / go toolchain).
///
/// - `MemFs` 用于对照测试，把文件内容、权限和删除记录都留在内存里。
/// - 它刻意不模拟真正文件系统的全部细节，只覆盖当前流程要观察的状态。
/// - `fail_abs` 和 `fail_remove` 允许测试按路径注入失败，验证错误分支是否与 Go 对齐。
#[derive(Clone, Default)]
pub struct MemFs {
    pub files: Rc<RefCell<HashMap<String, Vec<u8>>>>,
    pub modes: Rc<RefCell<HashMap<String, u32>>>,
    pub removed: Rc<RefCell<Vec<String>>>,
    pub fail_abs: Rc<RefCell<HashMap<String, Error>>>,
    pub fail_remove: Rc<RefCell<Option<Error>>>,
}

impl MemFs {
    ///
    /// - 返回全空状态的内存文件系统，适合作为每个测试用例的起点。
    pub fn new() -> Self {
        Self::default()
    }

    ///
    /// - 预先写入文件内容，模拟测试场景下已有的 manifest 或生成文件。
    pub fn put(&self, path: &str, data: impl AsRef<[u8]>) {
        self.files
            .borrow_mut()
            .insert(path.to_string(), data.as_ref().to_vec());
    }

    ///
    /// - 读取原始字节，供测试检查写入结果是否与模板输出完全一致。
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.borrow().get(path).cloned()
    }

    ///
    /// - 提供字符串视角，方便断言生成的 Go 源码或 JSON 文本。
    pub fn get_string(&self, path: &str) -> Option<String> {
        self.get(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    ///
    /// - 删除记录单独保留，便于确认成功路径确实执行了“延迟清理”。
    pub fn removed_paths(&self) -> Vec<String> {
        self.removed.borrow().clone()
    }

    ///
    /// - 权限信息也是对照点之一，特别是临时 `.gen.go` 文件的 `0700`。
    pub fn mode_of(&self, path: &str) -> Option<u32> {
        self.modes.borrow().get(path).copied()
    }
}

impl Fs for MemFs {
    fn abs(&self, path: &str) -> Result<String> {
        //
        // - 内存实现不依赖真实当前目录，因此统一把相对路径映射到 `/abs/...`。
        // - 只要映射规则稳定，测试就能验证后续 join 和日志输出逻辑。
        if let Some(err) = self.fail_abs.borrow().get(path) {
            return Err(err.clone());
        }
        if path.starts_with('/') {
            return Ok(path.to_string());
        }
        Ok(format!("/abs/{path}"))
    }

    fn read_to_string(&self, path: &str) -> Result<String> {
        //
        // - 文件不存在时返回与 Go 场景相近的 open 错误文本，便于日志断言。
        self.files
            .borrow()
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .ok_or_else(|| Error::new(format!("open {path}: no such file")))
    }

    fn write(&self, path: &str, data: &[u8], mode: u32) -> Result<()> {
        //
        // - 写入同时记录权限，供主流程测试确认临时文件模式没有漂移。
        self.files
            .borrow_mut()
            .insert(path.to_string(), data.to_vec());
        self.modes.borrow_mut().insert(path.to_string(), mode);
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<()> {
        //
        // - 删除既更新文件映射，也留下审计轨迹；失败则整个状态保持不变。
        if let Some(err) = self.fail_remove.borrow().clone() {
            return Err(err);
        }
        self.removed.borrow_mut().push(path.to_string());
        self.files.borrow_mut().remove(path);
        self.modes.borrow_mut().remove(path);
        Ok(())
    }
}

/// Real OS filesystem (production default).
///
/// - 生产环境默认走这个实现，真正落到宿主机文件系统。
/// - 接口仍保持很窄，确保主流程与测试实现共享同一套调用顺序。
#[derive(Clone, Debug, Default)]
pub struct OsFs;

impl Fs for OsFs {
    fn abs(&self, path: &str) -> Result<String> {
        //
        // - 这里模仿 Go `filepath.Abs`：即使路径不存在，也先做绝对化与规范化。
        // - 因此不能调用需要路径真实存在的 canonicalize。
        let p = PathBuf::from(path);
        let abs = if p.is_absolute() {
            p
        } else {
            env::current_dir()?.join(p)
        };
        // Normalize `.` / `..` without requiring the path to exist (Go filepath.Abs).
        Ok(normalize_abs(&abs).to_string_lossy().into_owned())
    }

    fn read_to_string(&self, path: &str) -> Result<String> {
        Ok(fs::read_to_string(path)?)
    }

    fn write(&self, path: &str, data: &[u8], mode: u32) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        //
        // - 通过 `OpenOptionsExt::mode` 保留 Go `OpenFile(..., 0700)` 的权限语义。
        // - `truncate(true)` 对应重复打包时覆盖旧的临时生成文件。
        let mut opts = fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(true);
        opts.mode(mode);
        let mut f = opts.open(path)?;
        f.write_all(data)?;
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<()> {
        fs::remove_file(path)?;
        Ok(())
    }
}

fn normalize_abs(path: &Path) -> PathBuf {
    //
    // - 只消解 `.` 和 `..`，不解析符号链接，也不要求路径存在。
    // - 这与 Go `filepath.Abs` 更接近，而不是 `realpath` 风格的强规范化。
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Fixed clock for tests.
///
/// - 测试可用固定时间串锁定 manifest 输出，避免因为当前时间不同导致快照波动。
#[derive(Clone, Debug)]
pub struct FixedClock {
    pub value: String,
}

impl Clock for FixedClock {
    fn now_string(&self) -> String {
        self.value.clone()
    }
}

/// Production wall clock formatted like Go `time.Time.String()`.
///
/// - 使用真实 UTC 日期、纳秒和进程内单调时长，避免把 Unix 秒错误写入 1970 日期。
/// - 真正需要精确对齐的场景会在测试里注入 `FixedClock`。
#[derive(Clone, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_string(&self) -> String {
        chrono_like_now()
    }
}

fn chrono_like_now() -> String {
    use std::sync::OnceLock;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    //
    // - 当前实现基于 Unix 时间戳拼出稳定格式，重点是提供类似 Go `String()` 的文本槽位。
    // - 这里不引入额外时间库，避免给这个轻量命令增加不必要依赖。
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = dur.as_secs();
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = second_of_day % 3_600 / 60;
    let second = second_of_day % 60;
    static START: OnceLock<Instant> = OnceLock::new();
    let monotonic = START.get_or_init(Instant::now).elapsed();
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{:09} +0000 UTC m=+{}.{:09}",
        dur.subsec_nanos(),
        monotonic.as_secs(),
        monotonic.subsec_nanos()
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

/// Scripted `go` runner for tests.
///
/// - 与 `MemFs` 配套使用，负责记录命令调用并按需注入失败。
/// - 它不执行任何真实子进程，因此非常适合验证参数、目录和环境变量拼装是否正确。
#[derive(Clone, Default)]
pub struct ScriptedRunner {
    pub commands: Rc<RefCell<Vec<CommandSpec>>>,
    pub fail: Rc<RefCell<Option<Error>>>,
}

impl ScriptedRunner {
    ///
    /// - 返回空脚本执行器，默认所有命令都视为成功。
    pub fn new() -> Self {
        Self::default()
    }

    ///
    /// - 暴露已记录的调用快照，便于测试按顺序断言命令构造结果。
    pub fn commands(&self) -> Vec<CommandSpec> {
        self.commands.borrow().clone()
    }
}

impl Runner for ScriptedRunner {
    fn run(&self, program: &str, args: &[String], dir: &str, env_extra: &[String]) -> Result<()> {
        //
        // - 先记录再决定是否失败，确保即使模拟失败也能检查调用参数。
        self.commands.borrow_mut().push(CommandSpec {
            program: program.to_string(),
            args: args.to_vec(),
            dir: dir.to_string(),
            env: env_extra.to_vec(),
        });
        if let Some(err) = self.fail.borrow().clone() {
            return Err(err);
        }
        Ok(())
    }
}

/// Production `go` subprocess runner.
///
/// - 这是唯一真正触发 `go build` 的执行器。
/// - 它继承父进程标准输出和标准错误，使编译日志行为与 Go 原版主程序一致。
#[derive(Clone, Debug, Default)]
pub struct ProdRunner;

impl Runner for ProdRunner {
    fn run(&self, program: &str, args: &[String], dir: &str, env_extra: &[String]) -> Result<()> {
        //
        // - 子进程环境默认继承当前进程，只对传入的附加变量做覆盖/补充。
        // - 非零退出会被包装成 `Error::exit`，供上层转成统一的 fatal 分支。
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(dir);
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());
        for kv in env_extra {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        // Inherit process env; Go appends GO111MODULE=on.
        let status = cmd.status().map_err(Error::from)?;
        if !status.success() {
            // Rust displays a numeric status as `exit status: N`, while Go's
            // `exec.ExitError` uses `exit status N`.
            let message = status
                .to_string()
                .replacen("exit status: ", "exit status ", 1);
            return Err(Error::exit(message));
        }
        Ok(())
    }
}

/// Log sink matching Go `log.Printf` (no timestamp prefix in tests).
///
/// - 这里只保留“格式化后换行写出”的最小语义。
/// - 测试通常不关心 Go `log` 默认时间前缀，因此这里故意不自动补时间戳。
pub fn log_printf(out: &mut dyn Write, args: fmt::Arguments<'_>) {
    let _ = writeln!(out, "{args}");
}
