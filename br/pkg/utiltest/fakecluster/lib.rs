// Copyright 2026 AsterSQL.

//! 假集群测试夹具入口：导出 core 与 gRPC/流式相关 stubs。
//! 供 streamhelper 等单测模拟 PD/region checkpoint 与 flush 事件。
//! stubs 含 protobuf 请求响应与错误码；core 提供时钟推进等行为。
//! 扁平再导出降低测试引用路径深度。
//! 非生产代码，勿在运行时路径依赖。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "core.rs"]
pub mod core;

pub use core::*;
pub use stubs::{
    CancelHandle, Code, Context, Error, ErrorPb, FlushEvent, FlushNowRequest, FlushNowResponse,
    FlushResult, GetLastFlushTSOfRegionRequest, GetLastFlushTSOfRegionResponse, KeyRange, Lock,
    RegionCheckpoint, RegionIdentity, Result, StatusError, SubscribeFlushEventRequest,
    SubscribeFlushEventResponse, codec, oracle, status_error,
};

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "core_test.rs"]
mod core_test;
