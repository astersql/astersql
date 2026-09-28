// Copyright 2026 AsterSQL.

// 外键（Foreign Key，FK）DDL 测试 crate 入口。
//
// 对应 Go `pkg/ddl/tests/fk`：覆盖外键定义校验、创建状态机
// （None → WriteOnly → WriteReorganization → Public）、引用完整性
// 保护以及 DROP/MODIFY COLUMN 与索引依赖检查。
// 外键约束子表列必须引用父表已有索引列，并支持 ON DELETE/UPDATE 动作。

#![allow(dead_code)]

#[cfg(test)]
mod foreign_key_test;
#[cfg(test)]
mod main_test;
