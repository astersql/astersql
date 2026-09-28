// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 日志与敏感信息脱敏（redact），对齐 Go `pkg/util/redact`。
//
// 支持 `tidb_redact_log` 三种模式：OFF 原样、ON 清空/问号、MARKER 用 ‹› 包裹并转义。
// 提供字符串/Stringer 脱敏、逐行反脱敏（DeRedact）、全局开关，以及对 BR 备份任务
// 存储后端凭证的展示遮盖（TaskInfoRedacted）。

#![allow(non_snake_case)]

use kvproto::brpb as backup;
use path_clean::PathClean;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write as IoWrite};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// 关闭日志脱敏（对应 Go errors.RedactLogDisable）。
// Go 的 errors.RedactLog* 常量来自 pingcap/errors；这里用本地常量保留模式字符串含义。
const REDACT_LOG_DISABLE: &str = "OFF";
/// 开启日志脱敏（对应 Go errors.RedactLogEnable）。
const REDACT_LOG_ENABLE: &str = "ON";
/// 标记模式：用 ‹› 包裹敏感内容（对应 Go errors.RedactLogMarker）。
const REDACT_LOG_MARKER: &str = "MARKER";

/// 全局脱敏模式存储；Go 用 errors.RedactLogEnabled，此处用 RwLock 近似。
// Go 代码通过 errors.RedactLogEnabled.Store/Load 保存全局脱敏模式。
// Rust 用 RwLock 近似表达可变全局状态，真实实现仍应以后续跨包接线为准。
static REDACT_LOG_ENABLED: OnceLock<RwLock<String>> = OnceLock::new();

type RedactResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// 对齐 Go `fmt.Stringer`，保留方法名 `String` 便于对照。
// FmtStringer 对应 Go 的 fmt.Stringer 接口。
// 这里保留 Go 方法名 String，方便人工和来源文件逐项对照。
pub trait FmtStringer {
    /// 返回可打印字符串表示。
    fn String(&self) -> String;
}

fn redact_log_enabled() -> &'static RwLock<String> {
    REDACT_LOG_ENABLED.get_or_init(|| RwLock::new(REDACT_LOG_DISABLE.to_string()))
}

fn clean_path(path: &str) -> PathBuf {
    Path::new(path).clean()
}

/// 按 `tidb_redact_log` 模式处理输入字符串：MARKER 包裹并转义，OFF 原样，ON 清空。
// String will redact the input string according to 'mode'. Check 'tidb_redact_log': https://github.com/pingcap/tidb/blob/acf9e3128693a5a13f31027f05f4de41edf8d7b2/pkg/sessionctx/variable/sysvar.go#L2154.
// String 按 Go 的 tidb_redact_log 模式处理输入字符串：MARKER 包裹并转义标记，OFF 原样返回，ON 清空。
pub fn String(mode: &str, input: &str) -> String {
    match mode {
        "MARKER" => {
            // Go strings.Builder.Grow(len(input)) 按字节预留容量；Rust String 同样以 UTF-8 字节容量表达。
            let mut b = String::with_capacity(input.len() + "‹›".len());
            b.push('‹');
            for c in input.chars() {
                // 已存在的左右标记在 Go 中写两次，避免后续 DeRedact 把用户原文误当作边界。
                if c == '‹' || c == '›' {
                    b.push(c);
                    b.push(c);
                } else {
                    b.push(c);
                }
            }
            b.push('›');
            b
        }
        "OFF" => input.to_string(),
        "ON" => String::new(),
        _ => {
            // should never happen
            // Go 通过 intest.Assert(false, ...) 暴露非法模式；用 debug_assert 保留调试期检查。
            debug_assert!(false, "invalid redact mode");
            String::new()
        }
    }
}

/// 延迟脱敏包装器：保存模式与底层 Stringer，调用时再执行 String。
// redactStringer 对应 Go 的同名结构体，保存脱敏模式和一个 fmt.Stringer 输入。
#[allow(non_camel_case_types)]
pub struct redactStringer<'a> {
    /// 脱敏模式字符串（OFF / ON / MARKER）。
    pub mode: String,
    /// 被包装的可打印对象。
    pub stringer: &'a dyn FmtStringer,
}

impl FmtStringer for redactStringer<'_> {
    // String 保留 Go 值接收者方法语义；用不可变引用读取内部 stringer。
    fn String(&self) -> String {
        String(&self.mode, &self.stringer.String())
    }
}

/// 构造按 mode 脱敏的 Stringer 包装，对应 Go 返回 `redactStringer{mode, input}`。
// Stringer will redact the input stringer according to 'mode', similar to String().
// Stringer 构造一个延迟调用的脱敏包装器，对应 Go 返回 redactStringer{mode, input}。
pub fn Stringer<'a>(mode: &str, input: &'a dyn FmtStringer) -> redactStringer<'a> {
    redactStringer {
        mode: mode.to_string(),
        stringer: input,
    }
}

/// 对文件逐行反脱敏：可去掉标记内容或仅去除 ‹› 边界。
// DeRedactFile will deredact the input file, either removing marked contents, or remove the marker. It works line by line.
// DeRedactFile 打开输入文件和输出目标，然后把逐行反脱敏逻辑委托给 DeRedact。
pub fn DeRedactFile(remove: bool, input: &str, output: &str) -> RedactResult<()> {
    // filepath.Clean(input) 的语义在 clean_path 中以注释占位；File::open 对应 os.Open。
    let ifile = File::open(clean_path(input))?;

    if output == "-" {
        // Go 在 output == "-" 时写 os.Stdout；Rust 需要显式锁住 stdout handle。
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        DeRedact(remove, ifile, &mut handle, "\n")
    } else {
        // Go 的 os.OpenFile 使用 O_TRUNC|O_CREATE|O_WRONLY 和 0644；Unix 上显式对齐创建模式。
        let mut options = OpenOptions::new();
        options.truncate(true).create(true).write(true);
        #[cfg(unix)]
        options.mode(0o644);
        let file = options.open(clean_path(output))?;
        // Go defer file.Close() 由 Rust Drop 自动完成；错误传播仍通过 Result 表达。
        DeRedact(remove, ifile, file, "\n")
    }
}

/// 对 reader/writer 逐行反脱敏：`remove=true` 时标记内容变 `?`，否则去掉外层标记。
// DeRedact is similar to DeRedactFile, but act on reader/writer, it works line by line.
// DeRedact 对 reader/writer 执行逐行反脱敏：remove=true 时把被标记内容替换成 '?'，否则去掉外层标记。
pub fn DeRedact<R, W>(remove: bool, input: R, output: W, sep: &str) -> RedactResult<()>
where
    R: Read,
    W: IoWrite,
{
    // Go 使用 bufio.Scanner 按行读取、bufio.Writer 缓冲输出。
    // Rust 用 BufReader::lines 近似表达；Scanner 的 token 限制没有在此处建模。
    let sc = BufReader::new(input);
    let mut out = BufWriter::new(output);
    let mut buf = String::new();

    for line in sc.lines() {
        let text = line?;
        let chars: Vec<char> = text.chars().collect();
        let mut idx = 0;
        let mut start = false;

        while idx < chars.len() {
            let ch = chars[idx];
            idx += 1;

            if ch == '‹' {
                if start {
                    // must be '<'
                    // Go 在标记内部遇到第二个左标记时，再读取一个 rune 判断是否为转义的 '‹‹'。
                    if idx >= chars.len() {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "unexpected EOF after redact start marker",
                        )
                        .into());
                    }
                    let pch = chars[idx];
                    idx += 1;
                    if pch == ch {
                        buf.push(ch);
                    } else {
                        // Go 这里不 UnreadRune：非成对左标记和其后字符都留在缓冲区。
                        buf.push(ch);
                        buf.push(pch);
                    }
                } else {
                    // 第一次遇到左标记表示进入脱敏区间，并清空之前暂存内容。
                    start = true;
                    buf.clear();
                }
            } else if ch == '›' {
                if start {
                    // peek the next
                    // Go 读取下一个 rune 来判断 '››' 转义；若不是转义，需要 UnreadRune 回退。
                    if idx < chars.len() {
                        let pch = chars[idx];
                        if pch == ch {
                            idx += 1;
                            buf.push(ch);
                        } else {
                            start = false;
                            // 非转义闭合标记：idx 不前进，相当于 Go 的 UnreadRune。
                            if remove {
                                out.write_all(b"?")?;
                            } else {
                                out.write_all(buf.as_bytes())?;
                            }
                        }
                    } else {
                        // 行尾的右标记在 Go 中也会闭合当前区间，因为 ReadRune 返回 io.EOF。
                        start = false;
                        if remove {
                            out.write_all(b"?")?;
                        } else {
                            out.write_all(buf.as_bytes())?;
                        }
                    }
                } else {
                    // 未处于脱敏区间时，右标记只是普通字符。
                    write!(out, "{ch}")?;
                }
            } else if start {
                // 脱敏区间内的普通字符先进入缓冲区，直到确认遇到闭合右标记才输出或替换。
                buf.push(ch);
            } else {
                write!(out, "{ch}")?;
            }
        }

        if start {
            // 行尾仍未闭合时，Go 会把起始标记和缓冲内容原样写回，避免误删用户文本。
            out.write_all("‹".as_bytes())?;
            out.write_all(buf.as_bytes())?;
        }
        out.write_all(sep.as_bytes())?;
    }

    // Go defer out.Flush() 在函数返回前执行；显式 flush 以保留资源收尾点。
    out.flush()?;
    Ok(())
}

/// 初始化全局日志脱敏开关（写入近似于 Go `errors.RedactLogEnabled`）。
// InitRedact inits the enableRedactLog
// InitRedact 初始化全局日志脱敏开关；Go 中写入 errors.RedactLogEnabled。
pub fn InitRedact(redactLog: bool) {
    let mode = if redactLog {
        REDACT_LOG_ENABLE
    } else {
        REDACT_LOG_DISABLE
    };

    // Store 在 Go 里不会返回错误；Rust RwLock 可能因 panic poisoning 失败，这里保留为调试断言语义。
    let mut enabled = redact_log_enabled()
        .write()
        .expect("redact log enabled lock poisoned");
    *enabled = mode.to_string();
}

/// 当前全局模式是否需要脱敏（非 OFF 且非空）。
// NeedRedact returns whether to redact log
// NeedRedact 判断当前全局模式是否需要脱敏；既不是 OFF 也不是空字符串时返回 true。
pub fn NeedRedact() -> bool {
    let mode = redact_log_enabled()
        .read()
        .expect("redact log enabled lock poisoned");
    mode.as_str() != REDACT_LOG_DISABLE && !mode.is_empty()
}

/// 需要脱敏时隐藏普通字符串值，否则原样返回。
// Value receives string argument and return omitted information if redact log enabled
// Value 在需要脱敏时隐藏普通字符串值，否则原样返回。
pub fn Value(arg: &str) -> String {
    if NeedRedact() {
        return "?".to_string();
    }
    arg.to_string()
}

/// 需要脱敏时隐藏 key，否则返回大写十六进制编码。
// Key receives a key return omitted information if redact log enabled
// Key 在需要脱敏时隐藏 key，否则返回大写十六进制字符串，对应 Go strings.ToUpper(hex.EncodeToString(key))。
pub fn Key(key: &[u8]) -> String {
    if NeedRedact() {
        return "?".to_string();
    }

    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key {
        // Go 的 hex.EncodeToString 输出小写，再统一 ToUpper；这里直接按两位大写十六进制写入。
        encoded.push_str(&format!("{byte:02X}"));
    }
    encoded
}

/// 按传入模式把字符串写入 builder：MARKER 包裹，ON 写 `?`，其他原样。
// WriteRedact is to write string with redact into `strings.Builder`
// WriteRedact 根据传入 redact 模式把字符串写入 builder：MARKER 包裹，ON 写问号，其他模式原样写。
pub fn WriteRedact(build: &mut String, v: &str, redact: &str) {
    if redact == REDACT_LOG_MARKER {
        build.push_str("‹");
        build.push_str(v);
        build.push_str("›");
        return;
    } else if redact == REDACT_LOG_ENABLE {
        build.push_str("?");
        return;
    }
    build.push_str(v);
}

/// 包装 BR `StreamBackupTaskInfo`，展示时遮盖存储后端敏感字段且不改原对象。
// TaskInfoRedacted is a wrapper of backup.StreamBackupTaskInfo to redact sensitive information
// TaskInfoRedacted 包装 BR 的 StreamBackupTaskInfo，用于展示时遮盖存储后端的敏感字段。
pub struct TaskInfoRedacted<'a> {
    /// 任务信息引用；`None` 对应 Go 的 nil 指针。
    // Go 字段是 *backup.StreamBackupTaskInfo；用 Option<&...> 表达 nil/非 nil。
    pub Info: Option<&'a backup::StreamBackupTaskInfo>,
}

impl FmtStringer for TaskInfoRedacted<'_> {
    // String returns the redacted string of the task info
    // String 复制任务信息并只改副本里的敏感字段，避免影响调用方持有的原始配置。
    fn String(&self) -> String {
        let Some(info) = self.Info else {
            return "nil".to_string();
        };

        let mut infoCopy = info.clone();

        if let Some(storage) = info.storage.as_ref() {
            // Create a copy of StorageBackend to modify
            // Go 先复制 StorageBackend 再修改 oneof 后端，避免写回原始 task info。
            let mut storageCopy = storage.clone();

            // Handle different backend types
            match storageCopy.backend.as_mut() {
                Some(backup::StorageBackend_oneof_backend::S3(s3)) => {
                    s3.access_key = "[REDACTED]".to_string();
                    s3.secret_access_key = "[REDACTED]".to_string();
                    s3.sse_kms_key_id = "[REDACTED]".to_string();
                }
                Some(backup::StorageBackend_oneof_backend::Gcs(gcs)) => {
                    gcs.credentials_blob = "[REDACTED]".to_string();
                }
                Some(backup::StorageBackend_oneof_backend::AzureBlobStorage(azure)) => {
                    azure.shared_key = "[REDACTED]".to_string();
                    azure.access_sig = "[REDACTED]".to_string();
                    let mut encryption_key = backup::AzureCustomerKey::default();
                    encryption_key.encryption_key = "[REDACTED]".to_string();
                    azure.set_encryption_key(encryption_key);
                }
                _ => {}
            }

            infoCopy.set_storage(storageCopy);
        }

        // kvproto's generated Debug implementation uses its compact PbPrint formatter,
        // matching Go's proto.CompactTextString without requiring reflection descriptors.
        format!("{infoCopy:?}")
    }
}
