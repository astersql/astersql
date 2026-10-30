# `pkg/planner/core/lib.rs` 逻辑说明

## 文件定位

planner/core crate 根，装配 planner 模块并提供表达式工厂入口。

## 核心职责

导出逻辑/物理优化能力，将表达式工厂绑定到 canonical PlanBuilder，装配 MV 布局与派生源生成器。

## 主要符号

`PlannerBuildSimpleExpr`、`InstallPlannerExpressionFactory`、`mview_refresh_builder`、`mview_refresh_source`。

## 执行流程

表达式入口从当前 ContextRef 与 InfoSchema 建立 PlanBuilder；新增 MV source 模块供 session runtime 编译真实 SELECT 子树。

## 数据与状态

根模块不保存 MV 行数据、水位或事务；它们分别由 source、metadata 与 canonical session 管理。

## 依赖与调用关系

MV runtime 经 mview_refresh_source 生成 AST，再调用既有 NewPlanBuilder / DoOptimize；沿用此前 sink 布局。

## 错误处理与边界

表达式构造沿用现有 Error；新增 export 不引入回退或错误吞并。

## 并发与资源生命周期

保持既有工厂注册生命周期与 planner context 的线程边界；模块装配不持有新后台任务。

## 与 Go 版本的对应关系

MV source 装配为获授权的 34d5c2b75b4f422f43f82c96b97bdf1667305d71 派生源与 a87bf8db6499ac38075781cef7357709f07ea002 runtime 提供局部入口，其他表达式工厂保持原行为。

## 扩展指南

只在实际拥有逻辑时更新本说明；不得通过 root export 掩盖空源或桩执行器。MV 全流程验证仍在进行。
