// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.
// This code is copied from https://github.com/pingcap/dm/blob/master/pkg/context/context.go

//! Dumpling context wrapper matching Go `dumpling/context`.
//!
//! Records a Go-style cancel context plus a dumpling logger, preserving the
//! immutable wrapper semantics of the Go `*Context` helpers.
//!
//! 本模块把 Go 风格的 cancel context 和 dumpling logger 绑定成一个轻量包装，
//! 让调用方既能沿用 `Done/Err` 取消语义，也能像 Go 版本一样随手取 logger。
//! 各个 `With*` 方法都返回新值而不是原地修改，目的是保留 Go helper
//! “派生新上下文、旧上下文仍可继续使用”的不可变包装风格。

use astersql_dumpling_log as log;

use crate::stubs as gcontext;

/// Context is used to in dm to record some context field like
/// * go context
/// * logger
/// 公开字段 `Context` 保留了 Go embedding 的直觉，便于转发 `Done/Err`。
#[derive(Clone, Debug)]
pub struct Context {
    /// Embedded Go context (Go promotes `Done` / `Err` via embedding).
    pub Context: gcontext::Context,
    logger: log::Logger,
}

/// Background return a nop context
pub fn Background() -> Context {
    // 背景上下文默认配一个 nop logger，保持与 Go `log.Zap()` 路径一致。
    Context {
        Context: gcontext::Background(),
        logger: log::Zap(),
    }
}

/// NewContext return a new Context
pub fn NewContext(ctx: gcontext::Context, logger: log::Logger) -> Context {
    // 直接把外部传入的 go-style context 和 logger 组合起来，不做额外派生。
    Context {
        Context: ctx,
        logger,
    }
}

impl Context {
    /// WithContext set go context
    pub fn WithContext(&self, ctx: gcontext::Context) -> Context {
        // 只替换底层 context，logger 沿用原值，贴近 Go helper 的职责拆分。
        Context {
            Context: ctx,
            logger: self.logger.clone(),
        }
    }

    /// WithCancel sets a cancel context.
    pub fn WithCancel(&self) -> (Context, gcontext::CancelFunc) {
        // 子上下文从当前 go context 派生，但继续共享同一个 logger。
        let (ctx, cancel) = gcontext::Context::WithCancel(&self.Context);
        (
            Context {
                Context: ctx,
                logger: self.logger.clone(),
            },
            cancel,
        )
    }

    /// WithLogger set logger
    pub fn WithLogger(&self, logger: log::Logger) -> Context {
        // 换 logger 不应该断开取消链，因此这里保留原有 go context。
        Context {
            Context: self.Context.clone(),
            logger,
        }
    }

    /// L returns real logger
    pub fn L(&self) -> log::Logger {
        // 返回 clone 让调用方能像 Go 一样自由拿走并继续记录日志。
        self.logger.clone()
    }

    /// Done mirrors Go embedded `context.Context.Done` (channel closed ⇒ true).
    pub fn Done(&self) -> bool {
        self.Context.Done()
    }

    /// Err mirrors Go embedded `context.Context.Err`.
    pub fn Err(&self) -> Option<gcontext::Canceled> {
        self.Context.Err()
    }
}
