// Copyright 2026 AsterSQL.

// `nontransactionaltest` 测试包入口。
//
// 挂接非事务 DML（按分片键切批执行 insert/update/delete，而非单事务一次提交）
// 相关的 harness 与功能测试模块。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod nontransactional_test;
