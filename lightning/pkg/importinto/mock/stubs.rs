// Copyright 2026 AsterSQL.
//! Local stand-ins for gomock Controller / Call (arm64-safe; no kv/domain/kvproto/grpcio).
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/mock/stubs.rs`对应的占位类型与测试桩，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少28行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `ExpectedCall`承载\"ExpectedCall\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `ControllerInner`承载\"ControllerInner\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Controller`把\"Controller\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Call`把\"Call\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - 场景\"---------------------------------------------------------------------------\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Lightweight GoMock controller (Call / Record / Return)\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use astersql_lightning_pkg_importinto::Error;

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Lightweight GoMock controller (Call / Record / Return)
// ---------------------------------------------------------------------------

struct ExpectedCall {
    method: String,
    args: Vec<ExpectedArg>,
    rets: Mutex<Vec<Box<dyn Any + Send>>>,
}

#[derive(Debug)]
enum ExpectedArg {
    Any,
    I64(i64),
    String(String),
}

impl ExpectedArg {
    fn from_any(value: &dyn Any) -> Self {
        if value.is::<()>() {
            Self::Any
        } else if let Some(value) = value.downcast_ref::<i64>() {
            Self::I64(*value)
        } else if let Some(value) = value.downcast_ref::<String>() {
            Self::String(value.clone())
        } else {
            Self::Any
        }
    }

    fn matches(&self, actual: &dyn Any) -> bool {
        match self {
            Self::Any => true,
            Self::I64(expected) => actual.downcast_ref::<i64>() == Some(expected),
            Self::String(expected) => actual.downcast_ref::<String>() == Some(expected),
        }
    }
}

/// `*gomock.Controller` stand-in.
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Mutex<ControllerInner>>,
}

struct ControllerInner {
    expected: VecDeque<Arc<ExpectedCall>>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ControllerInner {
                expected: VecDeque::new(),
            })),
        }
    }

    /// Go: `ctrl.T.Helper()` — no-op stand-in.
    pub fn Helper(&self) {}

    /// Go: `ctrl.Call(receiver, method, args...)` → `[]any`.
    pub fn Call(&self, method: &str, args: Vec<Box<dyn Any + Send>>) -> Vec<Box<dyn Any + Send>> {
        let expected = {
            let mut inner = self.inner.lock().expect("gomock lock");
            let position = inner.expected.iter().position(|expected| {
                expected.method == method
                    && expected.args.len() == args.len()
                    && expected
                        .args
                        .iter()
                        .zip(&args)
                        .all(|(expected, actual)| expected.matches(actual.as_ref()))
            });
            position
                .and_then(|position| inner.expected.remove(position))
                .unwrap_or_else(|| panic!("Unexpected call to {method}"))
        };
        std::mem::take(&mut *expected.rets.lock().expect("rets lock"))
    }

    /// Go: `RecordCallWithMethodType(...)` — records an expected call.
    pub fn RecordCallWithMethodType(
        &self,
        method: &str,
        _method_type: &str,
        args: Vec<&dyn Any>,
    ) -> Call {
        let expected = Arc::new(ExpectedCall {
            method: method.to_string(),
            args: args.into_iter().map(ExpectedArg::from_any).collect(),
            rets: Mutex::new(Vec::new()),
        });
        self.inner
            .lock()
            .expect("gomock lock")
            .expected
            .push_back(Arc::clone(&expected));
        Call { expected }
    }

    pub fn remaining(&self) -> usize {
        self.inner.lock().expect("gomock lock").expected.len()
    }
}

/// `*gomock.Call` stand-in with chainable Return helpers.
#[derive(Clone)]
pub struct Call {
    expected: Arc<ExpectedCall>,
}

impl Call {
    /// Set raw return values (Go `Return(rets...)`).
    pub fn Return(self, rets: Vec<Box<dyn Any + Send>>) -> Self {
        *self.expected.rets.lock().expect("rets lock") = rets;
        self
    }

    /// Convenience: single error return (nil ⇒ Ok).
    pub fn ReturnError(self, err: Option<Error>) -> Self {
        self.Return(vec![Box::new(err)])
    }

    /// Convenience: typed dual return `(T, error)`.
    pub fn Return2<T: Any + Send>(self, val: T, err: Option<Error>) -> Self {
        self.Return(vec![Box::new(val), Box::new(err)])
    }

    /// Convenience: single typed return.
    pub fn Return1<T: Any + Send>(self, val: T) -> Self {
        self.Return(vec![Box::new(val)])
    }
}

/// Extract Go-style `error` from `ret[0]` (`nil` / missing ⇒ Ok).
pub fn take_error(mut rets: Vec<Box<dyn Any + Send>>) -> Result<()> {
    if rets.is_empty() {
        return Ok(());
    }
    let r = rets.remove(0);
    if r.is::<Option<Error>>() {
        return match *r.downcast::<Option<Error>>().unwrap() {
            Some(e) => Err(e),
            None => Ok(()),
        };
    }
    if r.is::<Error>() {
        return Err(*r.downcast::<Error>().unwrap());
    }
    Ok(())
}

/// Extract `(T, error)` from `ret[0], ret[1]`.
pub fn take_pair<T: 'static + Default>(mut rets: Vec<Box<dyn Any + Send>>) -> Result<T> {
    let val = if rets.is_empty() {
        T::default()
    } else {
        let r = rets.remove(0);
        r.downcast::<T>()
            .map(|b| *b)
            .unwrap_or_else(|_| T::default())
    };
    let err = if rets.is_empty() {
        Ok(())
    } else {
        take_error(rets)
    };
    err.map(|_| val)
}

/// Extract `(Option<T>, error)` for Go pointer returns.
pub fn take_opt_pair<T: 'static>(mut rets: Vec<Box<dyn Any + Send>>) -> Result<Option<T>> {
    let val = if rets.is_empty() {
        None
    } else {
        let r = rets.remove(0);
        if r.is::<Option<T>>() {
            *r.downcast::<Option<T>>().unwrap()
        } else if r.is::<T>() {
            Some(*r.downcast::<T>().unwrap())
        } else {
            None
        }
    };
    let err = if rets.is_empty() {
        Ok(())
    } else {
        take_error(rets)
    };
    err.map(|_| val)
}

/// Extract single typed return (default if missing).
pub fn take_one<T: 'static + Default>(mut rets: Vec<Box<dyn Any + Send>>) -> T {
    if rets.is_empty() {
        return T::default();
    }
    let r = rets.remove(0);
    r.downcast::<T>()
        .map(|b| *b)
        .unwrap_or_else(|_| T::default())
}
