// Copyright 2026 AsterSQL.

// `pkg/util` 通用工具包入口：聚合 CPU、错误、etcd、安全、会话池等子模块。
//
// 对应 Go `pkg/util`。各 `pub mod` 为独立能力单元；测试子模块在 `cfg(test)` 下按路径挂载，
// 与 Go 侧同名 `*_test.go` 对齐。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

pub mod cpu_posix;
pub mod cpu_windows;
pub mod errors;
pub mod etcd;
pub mod gogc;
pub mod id_generator;
pub mod misc;
pub mod prefix_helper;
pub mod printer;
pub mod rlimit_other;
pub mod rlimit_windows;
pub mod security;
pub mod service_url;
pub mod session_pool;
pub mod split;
pub mod tokenlimiter;
pub mod urls;
pub mod util;
pub mod wait_group_wrapper;
pub mod worker_pool;

#[cfg(test)]
mod cpu_posix_1_aster_unit_test;
#[cfg(test)]
mod cpu_windows_test;
#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;
#[cfg(test)]
#[path = "etcd_test.rs"]
mod etcd_test;
#[cfg(test)]
mod go_merge_34_service_url_test;
#[cfg(test)]
#[path = "misc_test.rs"]
mod misc_test;
#[cfg(test)]
#[path = "prefix_helper_test.rs"]
mod prefix_helper_test;
#[cfg(test)]
#[path = "printer_test.rs"]
mod printer_test;
#[cfg(test)]
#[path = "rlimit_other_test.rs"]
mod rlimit_other_test;
#[cfg(test)]
#[path = "security_test.rs"]
mod security_test;
#[cfg(test)]
#[path = "session_pool_test.rs"]
mod session_pool_test;
#[cfg(test)]
#[path = "split_test.rs"]
mod split_test;
#[cfg(test)]
#[path = "urls_test.rs"]
mod urls_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
#[cfg(test)]
#[path = "wait_group_wrapper_test.rs"]
mod wait_group_wrapper_test;
