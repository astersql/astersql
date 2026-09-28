// Copyright 2026 AsterSQL.

// `pkg/errors`：TiDB/Go `github.com/pingcap/errors` 的 Rust 映射。
//
// 子模块职责概览：
// - [`core`]：共享错误值、格式化参数与基础 `New`/`Errorf`
// - [`wrap`]：cause 链包装、堆栈附加与查找
// - [`stack`]：调用栈捕获与 [`StackTracer`] 接口
// - [`group`] / [`join`]：多原因错误组与合并
// - [`normalize`]：带错误码/脱敏的规范化错误（terror）
// - [`adaptor`]：Juju 风格 Trace/Annotate/分类后缀 API
//
// 本文件仅做模块声明与公开符号再导出，无运行时逻辑。

mod adaptor;
mod core;
mod group;
mod join;
mod normalize;
mod stack;
mod wrap;

pub use adaptor::{
    AlreadyExistsf, Annotate, Annotatef, BadRequestf, ErrorStack, IsAlreadyExists, IsNotFound,
    NewNoStackError, NewNoStackErrorf, NotFoundf, NotSupportedf, NotValidf, SuspendStack, Trace,
};
pub use core::{DynError, ErrorArg, Errorf, New, SharedError};
pub use group::{ErrorGroup, Errors, WalkDeep};
pub use join::Join;
pub use normalize::{
    AtomicRedactLogState, ErrCode, ErrCodeText, Error, ErrorEqual, ErrorID, ErrorNotEqual,
    HackedStr, MySQLErrorCode, Normalize, NormalizeOption, RFCCodeText, RFCErrorCode, RedactArgs,
    RedactErrorArg, RedactLogDisable, RedactLogEnable, RedactLogEnabled, RedactLogMarker,
};
pub use stack::{
    Frame, GetStackTracer, NewStack, Stack, StackTrace, StackTraceCarrier, StackTracer,
};
pub use wrap::{
    AddStack, Cause, Find, GetErrStackMsg, HasStack, Unwrap, WithMessage, WithStack, Wrap, Wrapf,
};
