// Copyright 2026 AsterSQL.

// server/handler 根模块：聚合 Auto ID owner、TiKV、升级等 HTTP 处理器子模块。
//
// 对应 Go `pkg/server/handler`，对外以子模块形式暴露各类运维/诊断 HTTP API。

#![allow(dead_code)]

pub mod auto_id_owner_handler;
pub mod tikv_handler;
pub mod upgrade_handler;
pub mod util;

#[cfg(test)]
#[path = "auto_id_owner_handler_test.rs"]
mod auto_id_owner_handler_test;

#[cfg(test)]
#[path = "handler_aster_unit_test.rs"]
mod handler_aster_unit_test;

#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;

#[cfg(test)]
#[path = "tikv_handler_test.rs"]
mod tikv_handler_test;

#[cfg(test)]
#[path = "upgrade_handler_test.rs"]
mod upgrade_handler_test;
