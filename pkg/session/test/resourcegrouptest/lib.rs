// Copyright 2026 AsterSQL.

// `resourcegrouptest` 测试包入口。
//
// 挂接资源组（Resource Group：按 RU 配额隔离 SQL/事务资源消耗）hint 与事务阶段传递相关测试。

#![allow(dead_code)]

#[cfg(test)]
mod resource_group_test;
