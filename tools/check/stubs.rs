// Copyright 2026 AsterSQL.
//! Local stand-ins for subprocess / regex / cover-profile boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Production defaults execute real `std::process::Command` and parse cover
//! profiles with a local Go-compatible parser. Tests inject process handlers.
//!
//! 这个模块不是业务主流程，而是 `ut.rs` 依赖的三类外部边界适配层。
//! 第一类边界是进程执行，默认走真实子进程，测试时可替换成内存桩。
//! 第二类边界是正则匹配，使用与 Go RE2 语法相近的线性时间引擎。
//! 第三类边界是 cover profile 解析，读取 Go 文本格式后交给 Rust 合并逻辑。
//! 这里强调“足够支撑工具行为”而不是“完整替代标准库/第三方库”。
//! 因此注释会明确哪些能力只是占位适配，避免维护者误解为通用实现。

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// process (os/exec.Cmd shape)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
/// Go `exec.Cmd` 的最小描述。
/// `ut` 只关心程序名、参数、工作目录和是否抓取组合输出，
/// 因此这里不暴露 stdin/stdout pipe、环境变量等完整进程配置。
pub struct CommandSpec {
    /// 待执行程序名，例如 `go` 或测试二进制路径。
    pub program: String,
    /// 原样传给子进程的参数序列，顺序必须稳定，避免改变 CLI 语义。
    pub args: Vec<String>,
    /// 非空时作为当前工作目录，对齐 Go `Cmd.Dir`。
    pub dir: PathBuf,
    /// 为真时走类似 Go `CombinedOutput` 的路径，把 stdout/stderr 拼成一个文本结果。
    pub capture_output: bool,
}

#[derive(Clone, Debug)]
/// 统一的子进程结果载体。
/// 这里把“命令是否成功”和“是否发生进程层错误”拆开表达，
/// 让调用方既能保留 Go 风格错误串，也能继续消费组合输出文本。
pub struct CommandResult {
    /// 与 Go `error == nil` / 退出码成功相近的结果标记。
    pub ok: bool,
    /// 组合输出文本；失败场景下也可能包含已产生的标准输出/错误输出。
    pub stdout: String,
    /// 当前适配层没有区分独立 stderr，保留字段是为了兼容调用接口形状。
    pub stderr: String,
    /// 进程启动失败或非零退出时的人类可读错误描述。
    pub err: Option<String>,
}

impl CommandResult {
    /// 构造成功结果，保留调用方提供的文本输出。
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            ok: true,
            stdout: stdout.into(),
            stderr: String::new(),
            err: None,
        }
    }

    /// 构造失败结果。
    /// `combined` 仍放进 `stdout`，因为调用方按 Go `CombinedOutput`
    /// 的消费方式处理文本，而不是分别读取 stdout/stderr。
    pub fn failure(err: impl Into<String>, combined: impl Into<String>) -> Self {
        let combined = combined.into();
        Self {
            ok: false,
            stdout: combined,
            stderr: String::new(),
            err: Some(err.into()),
        }
    }
}

/// 测试注入点：传入 `CommandSpec`，返回伪造的执行结果。
/// `Arc + Send + Sync` 允许并发 worker 共享同一个桩实现，
/// 也让 parity 测试能在多个场景间按需安装和清理处理器。
pub type ProcessHandler = Arc<dyn Fn(&CommandSpec) -> CommandResult + Send + Sync>;

fn process_slot() -> &'static Mutex<Option<ProcessHandler>> {
    static SLOT: OnceLock<Mutex<Option<ProcessHandler>>> = OnceLock::new();
    // 进程桩是进程级全局状态；延迟初始化避免生产路径无谓分配。
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Install a process handler for tests. `None` restores the real executor.
/// 这里不暴露栈式安装/恢复机制，调用方需要自己负责测试隔离。
pub fn set_process_handler(handler: Option<ProcessHandler>) {
    *process_slot().lock().unwrap() = handler;
}

/// 清空测试桩，回退到真实子进程执行。
pub fn clear_process_handler() {
    set_process_handler(None);
}

fn real_run(spec: &CommandSpec) -> CommandResult {
    // 真实路径只映射 `ut` 实际会用到的 `Command` 能力，避免把边界适配做成新框架。
    let mut cmd = StdCommand::new(&spec.program);
    cmd.args(&spec.args);
    if !spec.dir.as_os_str().is_empty() {
        cmd.current_dir(&spec.dir);
    }
    if spec.capture_output {
        // 对齐 Go `CombinedOutput`：无论成功失败，都把 stdout/stderr 合并成单个文本。
        match cmd.output() {
            Ok(out) => {
                let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                if out.status.success() {
                    CommandResult {
                        ok: true,
                        stdout: combined,
                        stderr: String::new(),
                        err: None,
                    }
                } else {
                    // 失败时保留退出码描述，供上层原样拼接或打印。
                    let err = out
                        .status
                        .code()
                        .map(|c| format!("exit status {c}"))
                        .unwrap_or_else(|| "exit status unknown".to_string());
                    CommandResult {
                        ok: false,
                        stdout: combined,
                        stderr: String::new(),
                        err: Some(err),
                    }
                }
            }
            // 启动失败时没有组合输出，错误信息来自 `std::io::Error`。
            Err(e) => CommandResult::failure(e.to_string(), String::new()),
        }
    } else {
        // 不抓输出的分支只关心退出状态，贴近 Go `Run` 的使用方式。
        match cmd.status() {
            Ok(status) if status.success() => CommandResult::success(String::new()),
            Ok(status) => {
                let err = status
                    .code()
                    .map(|c| format!("exit status {c}"))
                    .unwrap_or_else(|| "exit status unknown".to_string());
                CommandResult::failure(err, String::new())
            }
            Err(e) => CommandResult::failure(e.to_string(), String::new()),
        }
    }
}

/// 统一执行入口。
/// 测试装了处理器时优先走桩，未安装时再退回真实系统调用，
/// 从而让 `ut.rs` 无需知道当前是生产路径还是测试路径。
pub fn run_command(spec: &CommandSpec) -> CommandResult {
    if let Some(handler) = process_slot().lock().unwrap().as_ref() {
        return handler(spec);
    }
    real_run(spec)
}

// ---------------------------------------------------------------------------
// regexp (golang regexp.Compile / MatchString subset used by ut filter)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
/// 预编译后的简化正则。
/// 内部不是通用 NFA/DFA，而是为 `MatchString` 子集构造的节点序列。
pub struct Regex {
    inner: regex::Regex,
}

impl Regex {
    /// 编译正则文本。
    /// 这里拒绝未消费完的尾部内容，避免“部分成功解析”掩盖表达式错误。
    pub fn compile(pattern: &str) -> Result<Self, String> {
        regex::Regex::new(pattern)
            .map(|inner| Self { inner })
            .map_err(|err| err.to_string())
    }

    /// 近似 Go `MatchString`。
    /// 若模式以 `^` 开头，只允许从文本起点尝试；
    /// 否则按 Go 的搜索语义，从每个可能起点尝试一次。
    pub fn is_match(&self, text: &str) -> bool {
        self.inner.is_match(text)
    }
}

/// Public compile used by ut — Go `regexp.Compile`.
/// 对外只暴露编译入口，具体节点结构维持模块私有，方便以后替换实现。
pub fn compile_regex(pattern: &str) -> Result<Regex, String> {
    Regex::compile(pattern)
}

// ---------------------------------------------------------------------------
// cover profile (golang.org/x/tools/cover.ParseProfilesFromReader)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
/// 单个文件的覆盖率数据。
/// 按 Go cover profile 的天然分组组织，
/// 后续 `ut.rs` 会按文件名合并多个 profile 片段。
pub struct CoverProfile {
    /// 覆盖率所属源码文件。
    pub file_name: String,
    /// 该文件中出现的所有 block 样本。
    pub blocks: Vec<CoverProfileBlock>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 与 Go `cover.ProfileBlock` 对齐的最小字段集合。
/// 这里只保存合并算法需要的坐标、语句数和命中次数，
/// 不引入额外派生信息，保证与 Go 文本输入一一对应。
pub struct CoverProfileBlock {
    /// 起始行号。
    pub start_line: i32,
    /// 起始列号。
    pub start_col: i32,
    /// 结束行号。
    pub end_line: i32,
    /// 结束列号。
    pub end_col: i32,
    /// 该 block 覆盖的语句数，重复 block 合并时必须一致。
    pub num_stmt: i32,
    /// 命中次数或位图值，具体解释由上层 merge 逻辑决定。
    pub count: i32,
}

/// 解析 Go 文本格式的 cover profile。
/// 该函数只负责把文本切成按文件分组的 block 列表，
/// 排序、去重和计数归并都留给 `ut.rs` 中的后续阶段处理。
pub fn parse_profiles_from_reader<R: Read>(r: R) -> Result<Vec<CoverProfile>, String> {
    let mut lines = BufReader::new(r).lines();
    let first = lines
        .next()
        .transpose()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "empty cover profile".to_string())?;
    if !first.starts_with("mode: ") || first == "mode: " {
        // Go profile 第一行必须声明模式；没有 mode 说明输入根本不是合法 cover 文件。
        return Err(format!("bad mode line: {first}"));
    }

    let mut by_file: std::collections::HashMap<String, CoverProfile> =
        std::collections::HashMap::new();
    for line in lines {
        let line = line.map_err(|e| e.to_string())?;
        // file.go:SL.SC,EL.EC NS Count
        // 这里从右向左拆分，避免文件路径里带冒号或空格前缀时过早切断结构。
        let (file_and_loc, rest) = line
            .rsplit_once(' ')
            .ok_or_else(|| format!("bad profile line: {line}"))?;
        let count: i32 = rest.parse().map_err(|_| format!("bad count in: {line}"))?;
        if count < 0 {
            return Err(format!("negative count in: {line}"));
        }
        let (file_and_span, num_stmt_s) = file_and_loc
            .rsplit_once(' ')
            .ok_or_else(|| format!("bad profile line: {line}"))?;
        let num_stmt: i32 = num_stmt_s
            .parse()
            .map_err(|_| format!("bad NumStmt in: {line}"))?;
        if num_stmt < 0 {
            return Err(format!("negative NumStmt in: {line}"));
        }
        let (file, span) = file_and_span
            .rsplit_once(':')
            .ok_or_else(|| format!("bad profile line: {line}"))?;
        let (start, end) = span
            .split_once(',')
            .ok_or_else(|| format!("bad span in: {line}"))?;
        let (sl, sc) = start
            .split_once('.')
            .ok_or_else(|| format!("bad start in: {line}"))?;
        let (el, ec) = end
            .split_once('.')
            .ok_or_else(|| format!("bad end in: {line}"))?;
        let block = CoverProfileBlock {
            start_line: parse_non_negative(sl, "SL", &line)?,
            start_col: parse_non_negative(sc, "SC", &line)?,
            end_line: parse_non_negative(el, "EL", &line)?,
            end_col: parse_non_negative(ec, "EC", &line)?,
            num_stmt,
            count,
        };
        // 同一文件的多个 block 先按出现顺序收集；最终合并阶段再统一排序/规约。
        by_file
            .entry(file.to_string())
            .or_insert_with(|| CoverProfile {
                file_name: file.to_string(),
                blocks: Vec::new(),
            })
            .blocks
            .push(block);
    }
    let mode = &first["mode: ".len()..];
    let mut profiles: Vec<_> = by_file.into_values().collect();
    profiles.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    for profile in &mut profiles {
        profile
            .blocks
            .sort_by_key(|block| (block.start_line, block.start_col));
        let mut merged: Vec<CoverProfileBlock> = Vec::with_capacity(profile.blocks.len());
        for block in profile.blocks.drain(..) {
            if let Some(last) = merged.last_mut()
                && (last.start_line, last.start_col, last.end_line, last.end_col)
                    == (
                        block.start_line,
                        block.start_col,
                        block.end_line,
                        block.end_col,
                    )
            {
                if last.num_stmt != block.num_stmt {
                    return Err(format!(
                        "inconsistent NumStmt: changed from {} to {}",
                        last.num_stmt, block.num_stmt
                    ));
                }
                if mode == "set" {
                    last.count |= block.count;
                } else {
                    last.count += block.count;
                }
            } else {
                merged.push(block);
            }
        }
        profile.blocks = merged;
    }
    Ok(profiles)
}

fn parse_non_negative(value: &str, field: &str, line: &str) -> Result<i32, String> {
    let value = value
        .parse::<i32>()
        .map_err(|_| format!("bad {field}: {line}"))?;
    if value < 0 {
        return Err(format!("negative {field} in: {line}"));
    }
    Ok(value)
}
