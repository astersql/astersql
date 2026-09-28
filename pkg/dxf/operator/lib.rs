// Copyright 2026 AsterSQL.

// DXF operator crate 入口：导出数据流算子、管道组合与简易封装。
//
// 子模块职责：
// - `compose`：算子间通道连接（Compose / DataChannel）
// - `operator`：异步算子与可调 worker 池接口
// - `pipeline`：按序 Open/Close 的异步管道
// - `wrapper`：Source / Transform / Sink 的常用封装
//
// 测试模块在 `cfg(test)` 下引入迁移单测与管道集成测。

extern crate self as astersql_dxf_operator;
/// 再导出 workerpool，供算子与管道依赖的线程池/Context 使用。
pub use workerpool;
/// 算子通道组合与 SimpleDataChannel。
pub mod compose;
/// 异步算子（AsyncOperator）与 TunableOperator。
pub mod operator;
/// AsyncPipeline：多算子有序启动与关闭。
pub mod pipeline;
/// SimpleDataSource / SimpleOperator / SimpleSink 封装。
pub mod wrapper;

#[cfg(test)]
mod migration_aster_unit_test;
#[cfg(test)]
mod pipeline_test;
