// Copyright 2026 AsterSQL.
//! Local stand-ins for `go.uber.org/zap` (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Mirrors the minimal zap surface that `tests/llmtest/logger` needs:
//! development logger construction and Info/Error/Debug with fields.

// 本文件对应 `tests/llmtest/logger/stubs.rs`，本次任务只补中文解释，不改行为。
// 本文件提供轻量测试桩，而不是完整生产实现。
// 桩只覆盖当前测试真正触达的接口形状。
// 关键阅读点是全局开关、记录点和资源回收。
// 未覆盖的真实能力不会被假装支持。
// 中文注释会帮助区分桩职责与真实边界。
// 这类文件最怕隐式状态污染，因此会强调 reset 和 cleanup。
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// When true, [`zap::NewDevelopment`] returns an error (for init-failure parity).
// `FORCE_NEW_DEVELOPMENT_ERR` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
static FORCE_NEW_DEVELOPMENT_ERR: AtomicBool = AtomicBool::new(false);

/// Test hook: next [`zap::NewDevelopment`] fails when `force` is true.
// `set_force_new_development_error` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
pub fn set_force_new_development_error(force: bool) {
    FORCE_NEW_DEVELOPMENT_ERR.store(force, Ordering::SeqCst);
}

// 模块 `zap` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
pub mod zap {
    use super::*;

    /// Structured log field (Go `zap.Field`).
    #[derive(Clone, Debug, PartialEq, Eq)]
    // `Field` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    pub struct Field {
        pub key: String,
        pub value: String,
    }

    // `String` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn String(k: impl Into<String>, v: impl ToString) -> Field {
        Field {
            key: k.into(),
            value: v.to_string(),
        }
    }

    // `Int` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn Int(k: impl Into<String>, v: i32) -> Field {
        String(k, v)
    }

    // `Int64` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn Int64(k: impl Into<String>, v: i64) -> Field {
        String(k, v)
    }

    // `Any` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn Any(k: impl Into<String>, v: impl fmt::Debug) -> Field {
        String(k, format!("{v:?}"))
    }

    // `Error` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn Error(err: impl ToString) -> Field {
        String("error", err.to_string())
    }

    /// Captured log line for observable side effects in tests.
    #[derive(Clone, Debug, PartialEq, Eq)]
    // `Record` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    pub struct Record {
        pub level: &'static str,
        pub msg: String,
        pub fields: Vec<Field>,
    }

    /// Go `*zap.Logger` stand-in with development-logger semantics.
    #[derive(Clone, Debug)]
    // `Logger` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    pub struct Logger {
        /// True after a successful Sync (resource flush).
        synced: Arc<Mutex<bool>>,
        records: Arc<Mutex<Vec<Record>>>,
        /// Development config marker (Go NewDevelopment uses development config).
        pub development: bool,
    }

    // 这里实现 `Default` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    impl Default for Logger {
        // `default` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        fn default() -> Self {
            Self {
                synced: Arc::new(Mutex::new(false)),
                records: Arc::new(Mutex::new(Vec::new())),
                development: true,
            }
        }
    }

    // 这里实现 `Logger` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    impl Logger {
        // `push` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        fn push(&self, level: &'static str, msg: &str, fields: &[Field]) {
            if let Ok(mut g) = self.records.lock() {
                g.push(Record {
                    level,
                    msg: msg.to_string(),
                    fields: fields.to_vec(),
                });
            }
        }

        /// Go `(*Logger).Info(msg string, fields ...Field)`.
        // `Info` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        pub fn Info(&self, msg: &str, fields: &[Field]) {
            self.push("info", msg, fields);
        }

        /// Go `(*Logger).Error(msg string, fields ...Field)`.
        // `Error` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        pub fn Error(&self, msg: &str, fields: &[Field]) {
            self.push("error", msg, fields);
        }

        /// Go `(*Logger).Debug(msg string, fields ...Field)`.
        // `Debug` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        pub fn Debug(&self, msg: &str, fields: &[Field]) {
            self.push("debug", msg, fields);
        }

        /// Go `(*Logger).Sync() error` — flushes buffers; development logger is a no-op success.
        // `Sync` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        pub fn Sync(&self) -> Result<(), String> {
            if let Ok(mut s) = self.synced.lock() {
                *s = true;
            }
            Ok(())
        }

        /// Test helper: whether Sync completed.
        // `is_synced` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        pub fn is_synced(&self) -> bool {
            self.synced.lock().map(|s| *s).unwrap_or(false)
        }

        /// Test helper: captured records (observable side effects).
        // `records` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        pub fn records(&self) -> Vec<Record> {
            self.records.lock().map(|r| r.clone()).unwrap_or_default()
        }
    }

    /// Go `zap.NewDevelopment() (*Logger, error)`.
    // `NewDevelopment` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn NewDevelopment() -> Result<Logger, String> {
        if FORCE_NEW_DEVELOPMENT_ERR.load(Ordering::SeqCst) {
            return Err("forced NewDevelopment failure".to_string());
        }
        Ok(Logger {
            development: true,
            ..Default::default()
        })
    }
}
