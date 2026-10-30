# `pkg/executor/mview_refresh_steps.rs`

## 文件定位

物化视图刷新步骤契约，属于来源 a87bf8db6499ac38075781cef7357709f07ea002 的局部增量。

## 核心职责

为 FAST、COMPLETE IN PLACE、COMPLETE DELTA APPLY 和 COMPLETE OUT OF PLACE 生成原 Go 顺序的步骤与 S01 起的 ID，支持观察器 start/end 和计划行深拷贝。

## 主要符号

`ObserveStep`、`StepKind`、`StepObserver`、`new_step_set`、`clone_plan_rows`、`observe_step`。

## 执行流程

本模块通过 lib.rs 导出，由 canonical session 的 mview_refresh 协调器使用；四模式真实事务/DDL回归验证步骤列表、成功失败回调及 panic 清理边界。

## 数据与状态

步骤和时间计算使用调用者提供的数据，不持久化数据库状态。步骤集合与计划行由调用方拥有。

## 依赖与调用关系

本模块通过 lib.rs 导出，由 canonical session 的 mview_refresh 协调器使用；四模式真实事务/DDL回归验证步骤列表、成功失败回调及 panic 清理边界。

## 错误处理与边界

无事务、网络或存储错误处理。时间倒序和缺失端点归零；观察器返回错误仍收到 end 回调，panic 向协调器传播。

## 并发与资源生命周期

纯数据 helper 没有后台线程、连接、锁或事务资源。观察器以独占可变借用使用。

## 与 Go 版本的对应关系

`pkg/executor/mview_refresh_steps.go`，来源 a87bf8db6499ac38075781cef7357709f07ea002 新文件。Rust 使用闭合模式枚举，未知模式必须由 AST 的 Mode 校验拒绝。

## 扩展指南

保留来源的全部分支与步骤顺序；新增 runtime 接线前先补真实存储与事务测试，不能用本模块的纯计算测试替代行为验收。独立测试位于同目录测试文件，不放入生产源文件。
