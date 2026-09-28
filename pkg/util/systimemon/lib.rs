// Copyright 2026 AsterSQL.

// 系统时间回退监控（systimemon）crate 入口。
//
// 重新导出 `StartMonitor`；测试模块覆盖迁移单测、TestMain 配置与原 Go 行为对照。

extern crate self as astersql_util_systimemon;

// 核心实现：周期性采样系统时间，发现回退时回调错误处理函数。
#[path = "systime_mon.rs"]
mod systime_mon;
pub use systime_mon::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "systime_mon_test.rs"]
mod systime_mon_test;
