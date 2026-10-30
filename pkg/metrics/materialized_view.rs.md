# `pkg/metrics/materialized_view.rs`

## 文件定位

物化视图调度间隔指标，属于来源 a87bf8db6499ac38075781cef7357709f07ea002 的局部增量。

## 核心职责

构造并共享 tidb_mv_service_refresh_schedule_duration_seconds，桶为 1 秒起的 25 个二倍指数桶，观测单位为秒。

## 主要符号

`InitMVMetrics`、`MVServiceRefreshScheduleDurationHistogram`、`SCHEDULE_DURATION`。

## 执行流程

生产 InitMetrics 初始化、RegisterMetrics 注册；尚无刷新 runtime 生产观测点。测试验证注册、观测计数、和及全部桶边界。

## 数据与状态

指标句柄保存在进程级 OnceLock；样本和桶计数由 Prometheus collector 管理。

## 依赖与调用关系

生产 InitMetrics 初始化、RegisterMetrics 注册；尚无刷新 runtime 生产观测点。测试验证注册、观测计数、和及全部桶边界。

## 错误处理与边界

固定指标配置构造失败会 panic，初始化幂等；重复注册沿用 RegisterMetrics 的 Prometheus 错误传播。

## 并发与资源生命周期

OnceLock 保证初始化线程安全，collector 内部同步；注册仍由包级入口管理。

## 与 Go 版本的对应关系

`pkg/metrics/materialized_view.go`，来源 a87bf8db6499ac38075781cef7357709f07ea002 新文件。Rust 用 OnceLock 和 getter 替代 Go 可变全局 histogram，保持名称、帮助文本和桶配置。

## 扩展指南

保留来源的全部分支与步骤顺序；新增 runtime 接线前先补真实存储与事务测试，不能用本模块的纯计算测试替代行为验收。独立测试位于同目录测试文件，不放入生产源文件。
