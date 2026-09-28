// Copyright 2026 AsterSQL.
//! Local stand-ins for common/mydump/errors boundaries (arm64-safe).
//! `progress` 逻辑主要关心表名格式化、元数据大小和错误文本，
//! 这里因此只保留最小可观察行为，不引入真实 Lightning 依赖树。
//! 这些桩帮助测试稳定复刻 Go 语义，而不是模拟完整生产环境。
//! 因此字段和函数只覆盖 `progress.rs` 真正读取到的那一小部分。

use std::fmt;

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape)
// ---------------------------------------------------------------------------
// 错误桩额外保留 `not_found` 标记，
// 因为 `MarshalTableCheckpoints` 需要与 Go 的 NotFound 行为对齐。
// 错误文本仍然保持简单，便于直接进入 JSON 或断言。

#[derive(Clone, Debug)]
pub struct Error {
    pub msg: String,
    pub not_found: bool,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            not_found: false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub mod errors {
    use std::fmt;

    pub use super::{Error, Result};

    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    /// Matches pingcap/errors NotFoundf: `format+" not found"`.
    /// 文本后缀同样固定成 `not found`，方便测试直接断言字符串。
    pub fn NotFoundf(msg: impl fmt::Display) -> Error {
        Error {
            msg: format!("{msg} not found"),
            not_found: true,
        }
    }

    /// Matches pingcap/errors ErrorStack: nil → ""; else error text.
    /// 这里不展开堆栈，只返回最小文本表示，足够覆盖进度接口语义。
    pub fn ErrorStack(err: Option<&Error>) -> String {
        match err {
            None => String::new(),
            Some(e) => e.msg.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// common
// ---------------------------------------------------------------------------
// 表名必须继续使用 Go 风格的反引号转义，
// 因为它会作为进度表 map key 与 JSON 字段名出现。
// 这也是 parity test 会显式覆盖的外观契约之一。

pub mod common {
    pub fn EscapeIdentifier(identifier: &str) -> String {
        let mut builder = String::with_capacity(identifier.len() + 2);
        builder.push('`');
        for ch in identifier.chars() {
            if ch == '`' {
                builder.push_str("``");
            } else {
                builder.push(ch);
            }
        }
        builder.push('`');
        builder
    }

    pub fn UniqueTable(schema: &str, table: &str) -> String {
        format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
    }
}

// ---------------------------------------------------------------------------
// mydump (only fields BroadcastInitProgress needs)
// ---------------------------------------------------------------------------
// 这里只保留数据库名、表名和总大小，
// 足够支撑 `BroadcastInitProgress` 的初始化流程。
// 更多 mydump 字段即使真实存在，也不会影响本包对外行为。

pub mod mydump {
    #[derive(Clone, Debug, Default)]
    pub struct MDTableMeta {
        pub Name: String,
        pub TotalSize: i64,
    }

    #[derive(Clone, Debug, Default)]
    pub struct MDDatabaseMeta {
        pub Name: String,
        pub Tables: Vec<MDTableMeta>,
    }
}
