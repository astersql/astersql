// Copyright 2026 AsterSQL.

// Join 执行器综合测试 crate 入口。
//
// 对应 Go `pkg/executor/test/jointest`：覆盖普通 join、outer/semi join、
// index join、hash join、资源泄漏关闭、OOM 与并发 failpoint 场景。
// Join（连接）按谓词把多表行组合成结果集。
// 本文件仅在 `#[cfg(test)]` 下挂接测试模块。

#![allow(dead_code)]

#[cfg(test)]
/// Join 回归用例：以 CaseRecorder 保留 Go SQL fixture 与断言顺序。
mod join_test;
#[cfg(test)]
/// 包级 TestMain 参考归档与冒烟校验。
mod main_test;
