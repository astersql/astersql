// Copyright 2026 AsterSQL.
//! Local cobra-style CLI stand-ins (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 为 `utils` 测试提供精简 cobra 风格命令树与 flag 解析桩。
//! 不依赖真实 cobra/kvproto；未知 flag 与缺少参数按 Cobra 语义报错。
//! 仅支持本测试需要的 String flag 与子命令分发，不是完整 CLI 框架。
//! Execute 优先使用 SetArgs，便于单测不污染进程 argv。
//! 子命令匹配取 Use 首段空白分隔 token。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// 本桩统一 Result。
pub type Result<T> = std::result::Result<T, Error>;

/// 轻量错误，仅承载消息字符串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 构造错误消息。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// 子命令 Run 回调签名。
pub type RunFn = fn(&mut Command, &[String]);

/// 命令 ID 分配器，保证 Default 唯一。
static CMD_ID: AtomicU64 = AtomicU64::new(1);

/// 字符串 flag 集合：defs 存默认，values 存当前值。
#[derive(Clone, Default)]
pub struct FlagSet {
    /// 当前解析到的值。
    values: HashMap<String, String>,
    /// 注册时的默认值与占位 usage。
    defs: HashMap<String, (String, String)>,
}

impl FlagSet {
    /// 注册 String flag；usage 忽略，仅占位对齐 cobra API。
    pub fn String(&mut self, name: &str, default: &str, _usage: &str) {
        self.defs
            .insert(name.to_string(), (default.to_string(), String::new()));
        self.values
            .entry(name.to_string())
            .or_insert_with(|| default.to_string());
    }

    /// 读取 flag：先 values 再 defs，皆无则报错。
    pub fn GetString(&self, name: &str) -> Result<String> {
        if let Some(v) = self.values.get(name) {
            return Ok(v.clone());
        }
        if let Some((default, _)) = self.defs.get(name) {
            return Ok(default.clone());
        }
        Err(Error::new(format!("flag not found: {name}")))
    }

    /// 直接写入当前值（解析路径使用）。
    pub fn set(&mut self, name: &str, value: String) {
        self.values.insert(name.to_string(), value);
    }
}

/// 命令节点：Use/Short/Run/flags/子命令，模拟 cobra.Command 子集。
pub struct Command {
    pub id: u64,
    pub Use: String,
    pub Short: String,
    pub Run: Option<RunFn>,
    pub flags: FlagSet,
    pub children: Vec<Command>,
    pub args: Vec<String>,
    /// Whether SetArgs was called, including with an explicitly empty vector.
    pub args_set: bool,
}

impl Default for Command {
    /// 分配新 id，其余字段空默认。
    fn default() -> Self {
        Self {
            id: CMD_ID.fetch_add(1, Ordering::SeqCst),
            Use: String::new(),
            Short: String::new(),
            Run: None,
            flags: FlagSet::default(),
            children: Vec::new(),
            args: Vec::new(),
            args_set: false,
        }
    }
}

impl Command {
    /// 可变借用 flags。
    pub fn Flags(&mut self) -> &mut FlagSet {
        &mut self.flags
    }

    /// 挂载子命令。
    pub fn AddCommand(&mut self, cmd: Command) {
        self.children.push(cmd);
    }

    /// 预设 argv（测试注入，避免读真实进程参数）。
    pub fn SetArgs(&mut self, args: Vec<String>) {
        self.args = args;
        self.args_set = true;
    }

    /// 打印简易 Usage。
    pub fn Usage(&self) -> Result<()> {
        println!("Usage: {} {}", "utils", self.Use);
        for child in &self.children {
            println!("  {}  {}", child.Use, child.Short);
        }
        Ok(())
    }

    /// 执行：优先 SetArgs，否则 `env::args().skip(1)`。
    pub fn Execute(&mut self) -> Result<()> {
        let args = if self.args_set {
            self.args.clone()
        } else {
            std::env::args().skip(1).collect::<Vec<_>>()
        };
        execute_command(self, &args)
    }
}

/// 递归分发：空参跑 Run/Usage；首段匹配子命令；否则本命令 Run。
fn execute_command(cmd: &mut Command, args: &[String]) -> Result<()> {
    // 无参数：有 Run 则执行，否则打印 Usage。
    if args.is_empty() {
        if let Some(run) = cmd.Run {
            run(cmd, args);
            return Ok(());
        }
        let _ = cmd.Usage();
        return Ok(());
    }

    let name = args[0].as_str();
    let rest = &args[1..];

    // 按 Use 首 token 匹配子命令名。
    let child_idx = cmd
        .children
        .iter()
        .position(|c| c.Use.split_whitespace().next() == Some(name));

    if let Some(idx) = child_idx {
        // 子命令路径：先应用 flag，再调用其子 Run。
        let (flag_args, positional) = split_flags(rest)?;
        apply_flags(&mut cmd.children[idx], &flag_args)?;
        let positional = positional;
        if let Some(run) = cmd.children[idx].Run {
            run(&mut cmd.children[idx], &positional);
            return Ok(());
        }
        return execute_command(&mut cmd.children[idx], rest);
    }

    if let Some(run) = cmd.Run {
        // 本命令自带 Run：把全部 args 当 flag+位置参。
        let (flag_args, positional) = split_flags(args)?;
        apply_flags(cmd, &flag_args)?;
        run(cmd, &positional);
        return Ok(());
    }

    // 未知子命令名。
    Err(Error::new(format!("unknown command {name}")))
}

/// 拆分 `--name value`/`--name=value`；未定义短选项按 Cobra 语义报错。
fn split_flags(args: &[String]) -> Result<(Vec<(String, String)>, Vec<String>)> {
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            positional.extend_from_slice(&args[i + 1..]);
            break;
        } else if let Some(rest) = a.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                flags.push((k.to_string(), v.to_string()));
            } else if i + 1 < args.len() {
                flags.push((rest.to_string(), args[i + 1].clone()));
                i += 1;
            } else {
                return Err(Error::new(format!("flag needs an argument: --{rest}")));
            }
        } else if a.starts_with('-') && a.len() > 1 && !a.starts_with("--") {
            return Err(Error::new(format!("unknown shorthand flag: {a}")));
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    Ok((flags, positional))
}

/// 应用已注册 flag；未知名按 Cobra 语义报错。
fn apply_flags(cmd: &mut Command, flags: &[(String, String)]) -> Result<()> {
    for (k, v) in flags {
        if cmd.flags.defs.contains_key(k) || cmd.flags.values.contains_key(k) {
            cmd.flags.set(k, v.clone());
        } else {
            return Err(Error::new(format!("unknown flag: --{k}")));
        }
    }
    Ok(())
}

/// 命名空间兼容：`cobra::Command` 重导出。
pub mod cobra {
    pub use super::Command;
}
