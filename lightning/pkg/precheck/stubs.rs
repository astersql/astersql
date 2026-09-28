// Copyright 2026 AsterSQL.
//! Local stand-ins for `context` and `errors` boundaries (arm64-safe; no
//! kv/domain/kvproto/grpcio).
//! `precheck` 只需要最小上下文与错误形状，因此这里故意不接真实依赖。
//! 这些桩服务于契约对齐和单元测试，不表示生产路径会使用此实现。
//! 如果未来切回真实依赖，上层 `Checker` 契约也不应因此改变。

use std::fmt;

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape)
// ---------------------------------------------------------------------------
// 错误模型保留 `Error()` 命名与字符串外观，
// 让上层断言可以按 Go 风格直接比对文本。
// 这里不实现堆栈和错误分类，因为当前包并未消费那些能力。

#[derive(Clone, Debug)]
pub struct Error {
    pub msg: String,
}

impl Error {
    // 统一从字符串构造，足以覆盖当前包对 pingcap/errors 的使用面。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

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

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.msg == other.msg
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn errors_New(msg: impl Into<String>) -> Error {
    Error::new(msg)
}

pub fn errors_Errorf(msg: impl fmt::Display) -> Error {
    Error::new(msg.to_string())
}

pub mod errors {
    pub use super::{Error, Result, errors_Errorf as Errorf, errors_New as New};
}

// ---------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------
// `Context` 保留检查调度所需的取消标记和类型擦除值。
// importer 会把调用方 context 原样转换到这里，避免检查器丢失任务管理器等值。

pub mod context {
    use std::any::Any;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    pub struct Context {
        pub str_values: Arc<HashMap<String, Arc<dyn Any + Send + Sync>>>,
        pub cancelled: bool,
    }

    impl std::fmt::Debug for Context {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Context")
                .field("value_count", &self.str_values.len())
                .field("cancelled", &self.cancelled)
                .finish()
        }
    }

    pub fn Background() -> Context {
        Context::default()
    }

    impl Context {
        pub fn Value(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
            self.str_values.get(key).cloned()
        }
    }
}
