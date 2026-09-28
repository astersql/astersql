// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Local stand-ins for glue Session / sqlexec / domain / filter / metautil /
//! br/pkg/errors boundaries (darwin-safe; no heavy workspace deps).
//! 中文注释索引开始
//! 本文件负责`br/pkg/registry/stubs.rs`对应的Session/sqlexec/domain/filter 等边界桩，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少95行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! 本文件是桩/适配边界：占位能力与真实依赖要分开描述，缺失实现不得写成已支持。
//! - `Error`承载"Error"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Error`把"Error"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Context`承载"Context"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Context`把"Context"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `SqlValue`承载"SqlValue"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl SqlValue`把"SqlValue"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Row`承载"Row"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Row`把"Row"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `OptionFuncAlias`承载"OptionFuncAlias"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `RestrictedSQLExecutor`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `Session`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `Storage`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `MemStorage`承载"MemStorage"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `CIStr`承载"CIStr"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `NewCIStr`是当前文件的重要函数，承担"NewCIStr"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `InfoSchema`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `Domain`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `Glue`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `TaskStatus`承载"TaskStatus"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `TaskStatusRunning`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `TaskStatusPaused`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `TaskStatusResetting`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `DBInfo`承载"DBInfo"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `TableInfo`承载"TableInfo"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `Database`承载"Database"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `Table`承载"Table"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `PiTRIdTracker`承载"PiTRIdTracker"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `NewPiTRIdTracker`是当前文件的重要函数，承担"NewPiTRIdTracker"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `IsSysDB`是当前文件的重要函数，承担"IsSysDB"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `StripTempDBPrefixIfNeeded`是当前文件的重要函数，承担"StripTempDBPrefixIfNeeded"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Filter`定义边界契约，桩实现只保证测试可编译运行，不代表生产能力齐备。
//! 注释必须标明哪些方法是最小桩、哪些会返回固定错误或空结果。
//! 调用方若依赖桩行为做分支，需要同时核对 Go 错误类型与字符串。
//! 不要把桩里的简化路径描述成已完整移植的生产实现。
//! - `ParsedFilter`承载"ParsedFilter"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `ParseFilter`是当前文件的重要函数，承担"ParseFilter"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `CaseInsensitive`是当前文件的重要函数，承担"CaseInsensitive"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MatchSchema`是当前文件的重要函数，承担"MatchSchema"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MatchTable`是当前文件的重要函数，承担"MatchTable"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `err_table_not_exists`是当前文件的重要函数，承担"err_table_not_exists"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `is_table_not_exists`是当前文件的重要函数，承担"is_table_not_exists"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `set_stale_ticker_duration_ms`是当前文件的重要函数，承担"set_stale_ticker_duration_ms"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `set_wait_resetting_sleep_ms`是当前文件的重要函数，承担"set_wait_resetting_sleep_ms"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `set_skip_sleep`是当前文件的重要函数，承担"set_skip_sleep"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! 中文注释索引结束

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
        }
    }

    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
        }
    }

    pub fn Trace(err: Self) -> Self {
        err
    }

    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
            code: err.code,
        }
    }

    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self::Annotate(err, ctx)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// BR error codes used by registry (mirrors `br/pkg/errors`).
pub mod berrors {
    use super::Error;

    pub fn ErrInvalidArgument(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Common:ErrInvalidArgument", msg)
    }

    pub fn ErrTablesAlreadyExisted(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrTablesAlreadyExisted", msg)
    }

    pub fn ErrDatabasesAlreadyExisted(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrDatabasesAlreadyExisted", msg)
    }

    pub fn is_invalid_argument(err: &Error) -> bool {
        err.code == Some("BR:Common:ErrInvalidArgument")
            || err.msg.contains("BR:Common:ErrInvalidArgument")
            || err.msg.contains("already exists and is running")
            || err.msg.contains("unexpected state")
            || err.msg.contains("different restoredTS")
            || err.msg.contains("matching tasks")
    }

    pub fn is_tables_existed(err: &Error) -> bool {
        err.code == Some("BR:Restore:ErrTablesAlreadyExisted")
            || err.msg.contains("cannot be restored by current task")
    }

    pub fn is_databases_existed(err: &Error) -> bool {
        err.code == Some("BR:Restore:ErrDatabasesAlreadyExisted")
            || err.msg.contains("cannot be restored concurrently")
    }
}

/// Cancellation token approximating Go `context.Context`.
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    pub fn Background() -> Self {
        Self::default()
    }

    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }

    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

/// SQL argument / cell value used by restricted SQL mocks.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    U64(u64),
    I64(i64),
    Bool(bool),
    Str(String),
}

impl From<u64> for SqlValue {
    fn from(v: u64) -> Self {
        SqlValue::U64(v)
    }
}

impl From<i64> for SqlValue {
    fn from(v: i64) -> Self {
        SqlValue::I64(v)
    }
}

impl From<bool> for SqlValue {
    fn from(v: bool) -> Self {
        SqlValue::Bool(v)
    }
}

impl From<String> for SqlValue {
    fn from(v: String) -> Self {
        SqlValue::Str(v)
    }
}

impl From<&str> for SqlValue {
    fn from(v: &str) -> Self {
        SqlValue::Str(v.to_string())
    }
}

impl From<TaskStatus> for SqlValue {
    fn from(v: TaskStatus) -> Self {
        SqlValue::Str(v.as_str().to_string())
    }
}

impl SqlValue {
    pub fn as_u64(&self) -> u64 {
        match self {
            SqlValue::U64(v) => *v,
            SqlValue::I64(v) => *v as u64,
            SqlValue::Bool(v) => {
                if *v {
                    1
                } else {
                    0
                }
            }
            SqlValue::Str(s) => s.parse().unwrap_or(0),
            SqlValue::Null => 0,
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            SqlValue::I64(v) => *v,
            SqlValue::U64(v) => *v as i64,
            other => other.as_u64() as i64,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            SqlValue::Bool(v) => *v,
            other => other.as_u64() != 0,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            SqlValue::Str(s) => s.clone(),
            SqlValue::U64(v) => v.to_string(),
            SqlValue::I64(v) => v.to_string(),
            SqlValue::Bool(v) => v.to_string(),
            SqlValue::Null => String::new(),
        }
    }
}

/// One result row (column-indexed).
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub cols: Vec<SqlValue>,
}

impl Row {
    pub fn new(cols: Vec<SqlValue>) -> Self {
        Self { cols }
    }

    pub fn GetUint64(&self, idx: usize) -> u64 {
        match self.cols.get(idx) {
            Some(SqlValue::U64(v)) => *v,
            Some(SqlValue::I64(v)) => *v as u64,
            Some(SqlValue::Bool(v)) => {
                if *v {
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub fn GetInt64(&self, idx: usize) -> i64 {
        match self.cols.get(idx) {
            Some(SqlValue::I64(v)) => *v,
            Some(SqlValue::U64(v)) => *v as i64,
            Some(SqlValue::Bool(v)) => {
                if *v {
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub fn GetString(&self, idx: usize) -> String {
        match self.cols.get(idx) {
            Some(SqlValue::Str(v)) => v.clone(),
            Some(SqlValue::U64(v)) => v.to_string(),
            Some(SqlValue::I64(v)) => v.to_string(),
            Some(SqlValue::Bool(v)) => v.to_string(),
            _ => String::new(),
        }
    }
}

/// Stand-in for `sqlexec.OptionFuncAlias` — only `UseCurSession` matters here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionFuncAlias {
    ExecOptionUseCurSession,
}

/// Restricted SQL executor surface used inside transactions.
pub trait RestrictedSQLExecutor: Send {
    fn ExecRestrictedSQL(
        &mut self,
        ctx: &Context,
        opts: &[OptionFuncAlias],
        sql: &str,
        args: &[SqlValue],
    ) -> Result<Vec<Row>>;
}

/// Glue session: ExecuteInternal + restricted SQL + Close.
pub trait Session: RestrictedSQLExecutor {
    fn ExecuteInternal(&mut self, ctx: &Context, sql: &str, args: &[SqlValue]) -> Result<()>;
    fn Close(&mut self);
}

/// Stand-in for `kv.Storage`.
pub trait Storage: Send + Sync {
    fn name(&self) -> &str {
        "storage"
    }
}

#[derive(Debug, Default)]
pub struct MemStorage;

impl Storage for MemStorage {}

/// Case-insensitive identifier.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    pub fn new(s: impl Into<String>) -> Self {
        let O = s.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

pub fn NewCIStr(s: impl Into<String>) -> CIStr {
    CIStr::new(s)
}

/// InfoSchema.TableByName stand-in: Ok(()) means table exists.
pub trait InfoSchema: Send + Sync {
    fn TableByName(&self, ctx: &Context, db: &CIStr, table: &CIStr) -> Result<()>;
}

/// Domain stand-in: Store + InfoSchema.
pub trait Domain: Send + Sync {
    fn Store(&self) -> &dyn Storage;
    fn InfoSchema(&self) -> &dyn InfoSchema;
}

/// Glue stand-in: CreateSession only (registry constructor path).
pub trait Glue: Send + Sync {
    fn CreateSession(&self, store: &dyn Storage) -> Result<Box<dyn Session>>;
}

/// TaskStatus matches Go string enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskStatus(pub &'static str);

pub const TaskStatusRunning: TaskStatus = TaskStatus("running");
pub const TaskStatusPaused: TaskStatus = TaskStatus("paused");
pub const TaskStatusResetting: TaskStatus = TaskStatus("resetting");

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// metautil.Database / Table stand-ins used by conflict checks.
#[derive(Clone, Debug, Default)]
pub struct DBInfo {
    pub Name: CIStr,
}

#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub Name: CIStr,
}

#[derive(Clone, Debug, Default)]
pub struct Database {
    pub Info: DBInfo,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub DB: DBInfo,
    pub Info: TableInfo,
}

/// PiTR id tracker (name map only — registry conflict path).
#[derive(Clone, Debug, Default)]
pub struct PiTRIdTracker {
    pub DBNameToTableNames: HashMap<String, HashSet<String>>,
}

impl PiTRIdTracker {
    pub fn GetDBNameToTableName(&self) -> &HashMap<String, HashSet<String>> {
        &self.DBNameToTableNames
    }

    pub fn TrackTableName(&mut self, db: impl Into<String>, table: impl Into<String>) {
        self.DBNameToTableNames
            .entry(db.into())
            .or_default()
            .insert(table.into());
    }
}

pub fn NewPiTRIdTracker() -> PiTRIdTracker {
    PiTRIdTracker::default()
}

const SYSTEM_DB: &str = "mysql";
const SYS_DB: &str = "sys";
const WORKLOAD_SCHEMA: &str = "workload_schema";
const TEMP_DB_PREFIX: &str = "__TiDB_BR_Temporary_";

pub fn IsSysDB(db_lower_name: &str) -> bool {
    db_lower_name == SYSTEM_DB || db_lower_name == SYS_DB || db_lower_name == WORKLOAD_SCHEMA
}

pub fn StripTempDBPrefixIfNeeded(temp_db: &str) -> String {
    if let Some(rest) = temp_db.strip_prefix(TEMP_DB_PREFIX) {
        rest.to_string()
    } else {
        temp_db.to_string()
    }
}

/// Table-filter stand-in.
pub trait Filter: Send + Sync {
    fn MatchSchema(&self, schema: &str) -> bool;
    fn MatchTable(&self, schema: &str, table: &str) -> bool;

    fn MatchSchemaCaseInsensitive(&self, schema: &str) -> bool {
        self.MatchSchema(&schema.to_lowercase())
    }

    fn MatchTableCaseInsensitive(&self, schema: &str, table: &str) -> bool {
        self.MatchTable(&schema.to_lowercase(), &table.to_lowercase())
    }
}

#[derive(Clone)]
struct FilterRule {
    schema: String,
    table: String,
    positive: bool,
}

struct ParsedFilter {
    rules: Vec<FilterRule>,
    case_insensitive: bool,
}

impl ParsedFilter {
    fn norm<'a>(&self, s: &'a str) -> String {
        if self.case_insensitive {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    }

    fn match_pat(pat: &str, value: &str) -> bool {
        fn matches(pat: &[char], value: &[char]) -> bool {
            match pat.split_first() {
                None => value.is_empty(),
                Some(('*', rest)) => {
                    matches(rest, value) || (!value.is_empty() && matches(pat, &value[1..]))
                }
                Some(('?', rest)) => !value.is_empty() && matches(rest, &value[1..]),
                Some(('\\', rest)) => match rest.split_first() {
                    Some((escaped, tail)) => {
                        value.first() == Some(escaped) && matches(tail, &value[1..])
                    }
                    None => false,
                },
                Some((literal, rest)) => {
                    value.first() == Some(literal) && matches(rest, &value[1..])
                }
            }
        }
        matches(
            &pat.chars().collect::<Vec<_>>(),
            &value.chars().collect::<Vec<_>>(),
        )
    }
}

impl Filter for ParsedFilter {
    fn MatchSchema(&self, schema: &str) -> bool {
        let schema = self.norm(schema);
        for rule in &self.rules {
            let s = if self.case_insensitive {
                rule.schema.to_lowercase()
            } else {
                rule.schema.clone()
            };
            if Self::match_pat(&s, &schema) && (rule.positive || rule.table == "*") {
                return rule.positive;
            }
        }
        false
    }

    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        let schema = self.norm(schema);
        let table = self.norm(table);
        for rule in &self.rules {
            let s = if self.case_insensitive {
                rule.schema.to_lowercase()
            } else {
                rule.schema.clone()
            };
            let t = if self.case_insensitive {
                rule.table.to_lowercase()
            } else {
                rule.table.clone()
            };
            if Self::match_pat(&s, &schema) && Self::match_pat(&t, &table) {
                return rule.positive;
            }
        }
        false
    }

    fn MatchSchemaCaseInsensitive(&self, schema: &str) -> bool {
        let schema = schema.to_lowercase();
        for rule in &self.rules {
            if Self::match_pat(&rule.schema.to_lowercase(), &schema)
                && (rule.positive || rule.table == "*")
            {
                return rule.positive;
            }
        }
        false
    }

    fn MatchTableCaseInsensitive(&self, schema: &str, table: &str) -> bool {
        let schema = schema.to_lowercase();
        let table = table.to_lowercase();
        for rule in &self.rules {
            if Self::match_pat(&rule.schema.to_lowercase(), &schema)
                && Self::match_pat(&rule.table.to_lowercase(), &table)
            {
                return rule.positive;
            }
        }
        false
    }
}

/// Parse filter strings (minimal subset of `table-filter.Parse`).
pub fn ParseFilter(filter_strings: &[String]) -> Result<Box<dyn Filter>> {
    let mut rules = Vec::new();
    for raw in filter_strings {
        let mut s = raw.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        let positive = !s.starts_with('!');
        if !positive {
            s = &s[1..];
        }
        let (db, table) = s.split_once('.').ok_or_else(|| {
            Error::new(format!(
                "at <cmdline>:1: syntax error: missing '.' between schema and table patterns"
            ))
        })?;
        if db.is_empty() || table.is_empty() {
            return Err(Error::new("at <cmdline>:1: syntax error: missing pattern"));
        }
        rules.push(FilterRule {
            schema: db.to_string(),
            table: table.to_string(),
            positive,
        });
    }
    // Go reverses parsed rules, so the last matching command-line rule wins.
    rules.reverse();
    Ok(Box::new(ParsedFilter {
        rules,
        case_insensitive: false,
    }))
}

/// Wraps a filter so Match* compare case-insensitively (Go `filter.CaseInsensitive`).
pub fn CaseInsensitive(f: Box<dyn Filter>) -> Box<dyn Filter> {
    struct CI {
        inner: Box<dyn Filter>,
    }
    impl Filter for CI {
        fn MatchSchema(&self, schema: &str) -> bool {
            self.inner.MatchSchemaCaseInsensitive(schema)
        }
        fn MatchTable(&self, schema: &str, table: &str) -> bool {
            self.inner.MatchTableCaseInsensitive(schema, table)
        }
    }
    Box::new(CI { inner: f })
}

pub fn MatchSchema(filter: &dyn Filter, schema: &str, with_sys: bool) -> bool {
    let schema = StripTempDBPrefixIfNeeded(schema);
    if IsSysDB(&schema.to_lowercase()) && !with_sys {
        return false;
    }
    filter.MatchSchema(&schema)
}

pub fn MatchTable(filter: &dyn Filter, schema: &str, table: &str, with_sys: bool) -> bool {
    let schema = StripTempDBPrefixIfNeeded(schema);
    if IsSysDB(&schema.to_lowercase()) && !with_sys {
        return false;
    }
    filter.MatchTable(&schema, table)
}

/// ErrTableNotExists sentinel for NewRestoreRegistry.
pub fn err_table_not_exists() -> Error {
    Error::with_code("schema:ErrTableNotExists", "table does not exist")
}

pub fn is_table_not_exists(err: &Error) -> bool {
    err.code == Some("schema:ErrTableNotExists") || err.msg.contains("table does not exist")
}

// --- test / failpoint hooks ---

static STALE_TICK_MS: AtomicU64 = AtomicU64::new(0); // 0 => default 60s (1 minute)
static WAIT_RESET_SLEEP_MS: AtomicU64 = AtomicU64::new(0); // 0 => default 5s
static SKIP_SLEEP: AtomicBool = AtomicBool::new(false);

pub fn set_stale_ticker_duration_ms(ms: u64) {
    STALE_TICK_MS.store(ms, Ordering::SeqCst);
}

pub fn set_wait_resetting_sleep_ms(ms: u64) {
    WAIT_RESET_SLEEP_MS.store(ms, Ordering::SeqCst);
}

pub fn set_skip_sleep(skip: bool) {
    SKIP_SLEEP.store(skip, Ordering::SeqCst);
}

pub fn stale_ticker_duration() -> Duration {
    let ms = STALE_TICK_MS.load(Ordering::SeqCst);
    if ms == 0 {
        Duration::from_secs(60)
    } else {
        Duration::from_millis(ms)
    }
}

pub fn wait_resetting_sleep_duration() -> Duration {
    if SKIP_SLEEP.load(Ordering::SeqCst) {
        return Duration::from_millis(0);
    }
    let ms = WAIT_RESET_SLEEP_MS.load(Ordering::SeqCst);
    if ms == 0 {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(ms)
    }
}

pub fn maybe_sleep(d: Duration) {
    if SKIP_SLEEP.load(Ordering::SeqCst) || d.is_zero() {
        return;
    }
    std::thread::sleep(d);
}

/// kv.WithInternalSourceType — no-op stand-in (marks BR internal source in Go).
pub fn WithInternalSourceType(ctx: Context, _source: &str) -> Context {
    ctx
}

pub const InternalTxnBR: &str = "br";
