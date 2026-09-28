// Copyright 2026 AsterSQL.

// 事务（Transaction）执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/txn` 包。覆盖临时表/缓存表的 stale read 限制、
// SAVEPOINT 在乐观/悲观事务下的语义、回滚释放悲观锁、大事务与外键场景，
// 以及 innodb_lock_wait_timeout 等事务执行路径。
// 事务是一组要么全部提交、要么全部回滚的读写操作；SAVEPOINT 可在事务内
// 建立可回滚的中间点。本 crate 仅在 `#[cfg(test)]` 下编译测试源，不导出生产 API。

#![allow(dead_code)]

/// 包级 TestMain 草稿与可序列化 SAVEPOINT 顺序冒烟用例。
#[cfg(test)]
mod main_test;
/// 事务 SAVEPOINT / stale read / 悲观锁等 Go 草稿归档与最小 Rust 冒烟用例。
#[cfg(test)]
mod txn_test;
