// Copyright 2026 AsterSQL.

//! CRR 检查点服务包入口，对齐 Go `br/pkg/stream/crr/service`。
//! 职责：挂载 http/metrics/service/status 子模块，并扁平导出服务主 API。
//! 初始化顺序：先声明实现模块，再 `pub use`；测试仅在 cfg(test) 下 path 挂载。
//! metrics 保持 crate 私有，供 status 观察与 parity 测试读取，不对外暴露。
//! 本文件不承载业务逻辑，避免与 Go 单包多文件布局错位。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    clippy::all
)]

// 探测与状态 HTTP 端点（livez/readyz/status）。
#[path = "http.rs"]
pub mod http;

// 注册到默认 registry 的 Prometheus gauge，镜像 Go 指标名与标签。
#[path = "metrics.rs"]
mod metrics;

// 服务主循环、依赖注入与 Run/Shutdown。
#[path = "service.rs"]
pub mod service;

// 状态机、快照编码与 StatusObserver。
#[path = "status.rs"]
pub mod status;

// 扁平导出，使调用方写 `service::New` 而非 `service::service::New`。
pub use service::*;
pub use status::*;

// Go/Rust 公共契约对照（HTTP、默认值、错误路径、资源清理）。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

// 对应 Go service_test.go 的行为用例，与实现分文件。
#[cfg(test)]
#[path = "service_test.rs"]
mod service_test;

// status.rs 的 JSON 边界与 Go encoding/json 对抗测试。
#[cfg(test)]
#[path = "status_test.rs"]
mod status_test;
