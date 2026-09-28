// Copyright 2026 AsterSQL.

// server 通用集成测试包入口：声明 cursor 与 tidb 分卷测试模块。

#![allow(dead_code)]

#[cfg(test)]
/// 惰性游标（Lazy Cursor）相关测试。
mod cursor_test;
#[cfg(test)]
/// 测试入口/公共脚手架。
mod main_test;
#[cfg(test)]
/// TiDB 通用测试分卷 1（协议与 DSN）。
mod tidb_part1_aster_unit_test;
#[cfg(test)]
/// TiDB 通用测试分卷 2（TopSQL）。
mod tidb_part2_aster_unit_test;
#[cfg(test)]
/// TiDB 通用测试分卷 3（扩展事件与连接生命周期）。
mod tidb_part3_aster_unit_test;
#[cfg(test)]
/// TiDB 通用测试分卷 4（预处理语句与连接状态）。
mod tidb_part4_aster_unit_test;
