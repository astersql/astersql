// Copyright 2026 AsterSQL.
//! 本文件把 `cmd/mirror` 依赖的外部世界缩减成可替换边界。
//! 这样主流程既能在真实环境运行，也能在测试里完全脱离网络和 Bazel。
//! 错误类型需要同时表达普通失败、退出错误和文件不存在三种情况。
//! 因为主流程会对这些情况做不同的 Go 风格处理。
//! `Capture` 负责收集输出，让测试可以直接断言生成文本和提示文本。
//! `Bazel`、`Runner`、`Fs` 三个 trait 则分别描述 runfile、子进程和文件系统边界。
//! 内存文件系统与脚本环境让 parity test 能稳定重放各种成功与失败场景。
//! 生产实现只是在相同接口上绑定真实 OS 和真实 `go` 命令。
//! 因此理解这些桩的意义，本质上就是理解 mirror 主流程为何可以被完全注入。
//! 注释会重点解释每个边界为什么存在、调用方依赖它的哪类语义。
//! 本次改动不改变任何 trait 形状、默认值或故障注入方式。
//! 后续如果要换实现，也应先确保这些边界暴露的外部行为保持不变。
//! 特别是 `not_exist`、`exit`、runfile 解析顺序和环境变量覆盖顺序。
//! 这些看似细小的约定，都会直接影响 mirror 主流程的兼容性。
//! 因此本文件更像一组“可测试的系统边界声明”而不只是若干测试桩。
//! 读者可以把后续逐符号注释当成这些边界声明的详细说明。
//! 它们共同定义了主流程与外部世界之间允许交换的最小信息。
//! 也正因如此，这些桩常常比业务函数本身更需要解释约束。
//! 理解约束后，再看 parity test 就会更容易明白各类断言的来源。
//! 这也是本文件补中文注释的主要价值所在。

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Error type standing in for OS / exec / bazel failures at the mirror boundary.
/// 这里把文件系统、进程执行与 bazel 解析失败统一折叠成 mirror 可消费的错误形状。
/// 调用方后续只依赖消息文本、退出标记和不存在标记，所以这里不能随意扩展语义分支。
#[derive(Clone, Debug, PartialEq, Eq)]
/// `Error` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Error {
    pub msg: String,
    pub stderr: Vec<u8>,
    pub is_exit: bool,
    pub is_not_exist: bool,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            stderr: Vec::new(),
            is_exit: false,
            is_not_exist: false,
        }
    }

    /// `not_exist` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn not_exist(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            stderr: Vec::new(),
            is_exit: false,
            is_not_exist: true,
        }
    }

    /// `exit` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn exit(stderr: Vec<u8>) -> Self {
        Self {
            msg: "exit status".to_string(),
            stderr,
            is_exit: true,
            is_not_exist: false,
        }
    }

    /// `Error` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl std::error::Error for Error {}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        let is_not_exist = e.kind() == io::ErrorKind::NotFound;
        Self {
            msg: e.to_string(),
            stderr: Vec::new(),
            is_exit: false,
            is_not_exist,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Go `*exec.ExitError` shape used by `main` panic wrapping.
/// 这个壳类型只保留 `stderr`，用于让 `main` 层按 Go 的 panic 包装路径拿到退出输出。
#[derive(Clone, Debug)]
/// `ExitError` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ExitError {
    pub Stderr: Vec<u8>,
}

/// Deprecated CLI flags matching Go `flag.BoolVar` registrations.
/// 这两个布尔位保留旧命令行入口的可观测行为，避免迁移后把历史脚本参数静默丢掉。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `Flags` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Flags {
    pub is_mirror: bool,
    pub is_upload: bool,
}

/// Parse argv like Go `flag` for `-mirror` / `-upload` (and `--` forms).
/// `parse_flags` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_flags(args: &[String]) -> Flags {
    parse_flags_checked(args).unwrap_or_else(|err| panic!("{err}"))
}

/// Parse the two registered bool flags with Go `flag.FlagSet` semantics.
pub fn parse_flags_checked(args: &[String]) -> Result<Flags> {
    let mut flags = Flags::default();
    for arg in args {
        if arg == "--" || arg == "-" || !arg.starts_with('-') {
            break;
        }

        let flag = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .expect("flag prefix checked above");
        let (name, raw_value) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (flag, None),
        };
        if name != "mirror" && name != "upload" {
            return Err(Error::new(format!(
                "flag provided but not defined: -{name}"
            )));
        }
        let value = match raw_value {
            Some(value) => parse_go_bool(name, value)?,
            None => true,
        };
        match name {
            "mirror" => flags.is_mirror = value,
            "upload" => flags.is_upload = value,
            _ => unreachable!("registered flag checked above"),
        }
    }
    Ok(flags)
}

fn parse_go_bool(name: &str, value: &str) -> Result<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(Error::new(format!(
            "invalid value {value:?} for flag -{name}: parse error"
        ))),
    }
}

/// Args from process environment (skip argv0).
/// `args_from_env` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn args_from_env() -> Vec<String> {
    env::args().skip(1).collect()
}

/// A recorded `go` subprocess invocation.
/// 测试会读取这份记录来断言 gobin、参数、工作目录和额外环境变量是否按预期拼装。
#[derive(Clone, Debug, PartialEq, Eq)]
/// `CommandSpec` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct CommandSpec {
    pub gobin: String,
    pub args: Vec<String>,
    pub dir: String,
    pub env: Vec<String>,
}

/// Captured stdout/stderr writers for tests.
/// 它模拟可写输出端，目的是让 parity test 直接观察命令执行期间写出的文本。
#[derive(Clone, Default)]
/// `Capture` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Capture {
    inner: Arc<Mutex<Vec<u8>>>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Capture {
    pub fn new() -> Self {
        Self::default()
    }

    /// `bytes` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().unwrap().clone()
    }

    /// `string` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn string(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }

    /// `clear` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    // `flush` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Go `bazel.NewTmpDir` / `Runfile` / `RunfilesPath` surface.
/// `Bazel` 抽象了一类可替换外部边界。
/// 主流程通过它在生产实现与测试桩之间切换，而无需改业务代码。
/// 这也是该模块能做契约测试而不依赖真实环境的基础。
pub trait Bazel {
    // 临时目录创建失败要原样传给上层，因为 mirror 会直接把它当作初始化失败处理。
    fn NewTmpDir(&self, prefix: &str) -> Result<String>;
    // `Runfile` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn Runfile(&self, path: &str) -> Result<String>;
    // runfiles 根目录的来源顺序会影响后续路径拼装，因此必须稳定暴露成单独接口。
    fn RunfilesPath(&self) -> Result<String>;
}

/// Go `exec.Command(...).Output()` surface.
/// `Runner` 抽象了一类可替换外部边界。
/// 主流程通过它在生产实现与测试桩之间切换，而无需改业务代码。
/// 这也是该模块能做契约测试而不依赖真实环境的基础。
pub trait Runner {
    fn output(&self, gobin: &str, args: &[String], dir: &str, env: &[String]) -> Result<Vec<u8>>;
}

/// File / directory helpers used by createTmpDir / dumpPatchArgsForRepo / cleanup.
/// `Fs` 抽象了一类可替换外部边界。
/// 主流程通过它在生产实现与测试桩之间切换，而无需改业务代码。
/// 这也是该模块能做契约测试而不依赖真实环境的基础。
pub trait Fs {
    // 目录创建被单独抽出来，是为了让测试精确断言创建目标而不触碰真实磁盘。
    fn mkdir_all(&self, path: &str) -> Result<()>;
    // `copy_file` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn copy_file(&self, src: &str, dst: &str) -> Result<()>;
    // 清理阶段允许被注入失败，以覆盖 mirror 对临时目录回收错误的处理路径。
    fn remove_all(&self, path: &str) -> Result<()>;
    /// `os.Stat` — Ok when exists; Err with `is_not_exist` when missing.
    // `stat` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn stat(&self, path: &str) -> Result<()>;
}

/// In-memory FS for parity tests (no real disk / network).
/// 它把文件、目录和删除记录都留在内存里，便于一次测试同时核对副作用和清理顺序。
#[derive(Clone, Default)]
/// `MemFs` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct MemFs {
    pub files: Rc<RefCell<HashMap<String, Vec<u8>>>>,
    pub dirs: Rc<RefCell<Vec<String>>>,
    pub removed: Rc<RefCell<Vec<String>>>,
    pub fail_remove: Rc<RefCell<Option<Error>>>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// `put` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn put(&self, path: &str, data: impl AsRef<[u8]>) {
        self.files
            .borrow_mut()
            .insert(path.to_string(), data.as_ref().to_vec());
    }

    /// `get` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.borrow().get(path).cloned()
    }

    /// `removed_paths` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn removed_paths(&self) -> Vec<String> {
        self.removed.borrow().clone()
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Fs for MemFs {
    fn mkdir_all(&self, path: &str) -> Result<()> {
        self.dirs.borrow_mut().push(path.to_string());
        Ok(())
    }

    // `copy_file` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn copy_file(&self, src: &str, dst: &str) -> Result<()> {
        let data = self
            .files
            .borrow()
            .get(src)
            .cloned()
            .ok_or_else(|| Error::not_exist(format!("open {src}")))?;
        self.files.borrow_mut().insert(dst.to_string(), data);
        Ok(())
    }

    // `remove_all` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn remove_all(&self, path: &str) -> Result<()> {
        if let Some(err) = self.fail_remove.borrow().clone() {
            return Err(err);
        }
        self.removed.borrow_mut().push(path.to_string());
        let mut files = self.files.borrow_mut();
        files.retain(|k, _| !k.starts_with(path));
        Ok(())
    }

    // `stat` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn stat(&self, path: &str) -> Result<()> {
        if self.files.borrow().contains_key(path) {
            Ok(())
        } else {
            Err(Error::not_exist(format!("stat {path}")))
        }
    }
}

/// Real OS filesystem (production default).
/// 生产路径默认直接绑定标准库文件系统，以保证 mirror 的外部行为和真实命令一致。
#[derive(Clone, Debug, Default)]
/// `OsFs;` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct OsFs;

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Fs for OsFs {
    fn mkdir_all(&self, path: &str) -> Result<()> {
        fs::create_dir_all(path)?;
        Ok(())
    }

    // `copy_file` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn copy_file(&self, src: &str, dst: &str) -> Result<()> {
        let mut input = fs::File::open(src)?;
        let mut output = fs::File::create(dst)?;
        io::copy(&mut input, &mut output)?;
        Ok(())
    }

    // `remove_all` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn remove_all(&self, path: &str) -> Result<()> {
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    // `stat` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn stat(&self, path: &str) -> Result<()> {
        fs::metadata(path)?;
        Ok(())
    }
}

/// Scripted bazel + go runner for tests.
/// 它同时实现 bazel 与 runner 两组边界，让单个脚本环境就能重放 mirror 的完整调用链。
#[derive(Clone)]
/// `ScriptedEnv` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ScriptedEnv {
    pub tmp_counter: Rc<RefCell<u32>>,
    pub tmp_prefix: String,
    pub runfiles: HashMap<String, String>,
    pub runfiles_root: String,
    pub list_json: Vec<u8>,
    pub download_json: Vec<u8>,
    pub commands: Rc<RefCell<Vec<CommandSpec>>>,
    pub list_err: Option<Error>,
    pub download_err: Option<Error>,
    pub fail_new_tmp: Option<Error>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Default for ScriptedEnv {
    fn default() -> Self {
        Self {
            tmp_counter: Rc::new(RefCell::new(0)),
            tmp_prefix: "/tmp/gomirror".to_string(),
            runfiles: HashMap::new(),
            runfiles_root: "/runfiles".to_string(),
            list_json: Vec::new(),
            download_json: Vec::new(),
            commands: Rc::new(RefCell::new(Vec::new())),
            list_err: None,
            download_err: None,
            fail_new_tmp: None,
        }
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Bazel for ScriptedEnv {
    fn NewTmpDir(&self, prefix: &str) -> Result<String> {
        if let Some(err) = &self.fail_new_tmp {
            return Err(err.clone());
        }
        let mut c = self.tmp_counter.borrow_mut();
        *c += 1;
        Ok(format!("{}/{}-{}", self.tmp_prefix, prefix, *c))
    }

    // `Runfile` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn Runfile(&self, path: &str) -> Result<String> {
        self.runfiles
            .get(path)
            .cloned()
            .ok_or_else(|| Error::not_exist(format!("runfile {path}")))
    }

    // `RunfilesPath` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn RunfilesPath(&self) -> Result<String> {
        Ok(self.runfiles_root.clone())
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Runner for ScriptedEnv {
    fn output(&self, gobin: &str, args: &[String], dir: &str, env: &[String]) -> Result<Vec<u8>> {
        self.commands.borrow_mut().push(CommandSpec {
            gobin: gobin.to_string(),
            args: args.to_vec(),
            dir: dir.to_string(),
            env: env.to_vec(),
        });
        // Distinguish list vs download by args.
        if args.get(0).map(|s| s.as_str()) == Some("list") {
            if let Some(err) = &self.list_err {
                return Err(err.clone());
            }
            return Ok(self.list_json.clone());
        }
        if args.get(0).map(|s| s.as_str()) == Some("mod") {
            if let Some(err) = &self.download_err {
                return Err(err.clone());
            }
            return Ok(self.download_json.clone());
        }
        Err(Error::new(format!("unexpected go args: {args:?}")))
    }
}

/// Production bazel helpers — resolve via `TEST_SRCDIR` / `RUNFILES_DIR` when set,
/// otherwise relative to the process working directory (local non-bazel runs).
/// 这里的解析顺序直接决定本地运行与 bazel 运行是否共享同一套 runfile 查找语义。
#[derive(Clone, Debug, Default)]
/// `ProdBazel;` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ProdBazel;

pub fn resolve_runfiles_path(
    runfiles_dir: Option<&str>,
    test_srcdir: Option<&str>,
    workspace: Option<&str>,
) -> Result<PathBuf> {
    let root = runfiles_dir
        .or(test_srcdir)
        .ok_or_else(|| Error::new("could not locate runfiles directory"))?;
    let workspace = workspace.ok_or_else(|| Error::new("could not locate runfiles workspace"))?;
    Ok(PathBuf::from(root).join(workspace))
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Bazel for ProdBazel {
    fn NewTmpDir(&self, prefix: &str) -> Result<String> {
        static NEXT_TMP_DIR: AtomicU64 = AtomicU64::new(0);

        let base = env::var_os("TEST_TMPDIR")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        for _ in 0..10_000 {
            let sequence = NEXT_TMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("{prefix}{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(path.to_string_lossy().into_owned()),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(Error::new(format!(
            "failed to create a unique temporary directory in {}",
            base.display()
        )))
    }

    // `Runfile` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn Runfile(&self, path: &str) -> Result<String> {
        let cwd = env::current_dir()?;
        let p = cwd.join(path);
        if p.exists() {
            return Ok(p.to_string_lossy().into_owned());
        }

        let runfiles_dir = env::var("RUNFILES_DIR").ok();
        let test_srcdir = env::var("TEST_SRCDIR").ok();
        let workspace = env::var("TEST_WORKSPACE").ok();
        if let Ok(root) = resolve_runfiles_path(
            runfiles_dir.as_deref(),
            test_srcdir.as_deref(),
            workspace.as_deref(),
        ) {
            let candidate = root.join(path);
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
        Err(Error::not_exist(format!("runfile {path}")))
    }

    // `RunfilesPath` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn RunfilesPath(&self) -> Result<String> {
        let runfiles_dir = env::var("RUNFILES_DIR").ok();
        let test_srcdir = env::var("TEST_SRCDIR").ok();
        let workspace = env::var("TEST_WORKSPACE").ok();
        resolve_runfiles_path(
            runfiles_dir.as_deref(),
            test_srcdir.as_deref(),
            workspace.as_deref(),
        )
        .map(|path| path.to_string_lossy().into_owned())
    }
}

/// Production `go` subprocess runner.
/// 真实执行器只负责启动 `go` 并回传 stdout/stderr，不在这里混入 mirror 自己的业务判断。
#[derive(Clone, Debug, Default)]
/// `ProdRunner;` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ProdRunner;

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Runner for ProdRunner {
    fn output(
        &self,
        gobin: &str,
        args: &[String],
        dir: &str,
        env_extra: &[String],
    ) -> Result<Vec<u8>> {
        let mut cmd = std::process::Command::new(gobin);
        cmd.args(args);
        cmd.current_dir(dir);
        // Start from process env, then apply extras (Go appends GOSUMDB).
        for kv in env_extra {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        let out = cmd.output().map_err(Error::from)?;
        if !out.status.success() {
            return Err(Error::exit(out.stderr));
        }
        Ok(out.stdout)
    }
}

/// Read a file into bytes (used only by real copy paths in tests of OsFs).
/// `read_file` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}
