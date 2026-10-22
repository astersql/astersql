# `pkg/planner/mview/mview.rs`

## 文件定位

[`mview.rs`](mview.rs) 是 `astersql-planner` 的 `mview` 公共模块，由 [`../lib.rs`](../lib.rs) 的 path 属性装配，不是独立 mview crate；依赖来自 [`../Cargo.toml`](../Cargo.toml)。本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准，增量范围为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`；不纳入工作区后续源码。

## 核心职责

作为包级 API 入口再导出 core 的物化视图构建/lookup 能力，并装配、公开 stats 子模块；实现保留在 core 以避免 crate 循环依赖。该文件本身仅导出，无规划算法。

## 主要符号

构建导出 `BuildMView`、`BuildCompleteDiffSource`、`MViewDefinition`、`MViewColumn`、`MViewAggregate`、`MViewHandleKind` 和结果类型；`MViewBuildOptions` 别名为 `BuildOptions`，`MViewBuildResult` 为 `BuildResult`。lookup 导出三个函数、`MViewIndex`、`MViewFullUpdateLookupTemplate`。`#[path="stats.rs"] mod stats` 后 `pub use stats::*`。

## 执行流程

调用方经 `astersql_planner::mview` 选择构建、lookup 或统计 API；名称在编译期绑定到 core 原实现，没有额外运行时包装。BuildMView 计算 FAST layout，BuildCompleteDiffSource 计算 COMPLETE DELTA APPLY 行像；公开用户刷新和内部实现入口没有在此显式再导出。

## 数据与状态

该文件不拥有刷新进度、表信息或统计缓存。导出类型的元数据与结果所有权遵循 core 定义；别名没有引入新类型或额外拷贝。

## 依赖与调用关系

上游装配是 planner lib，依赖方向为 planner -> planner-core；core 不反向依赖 planner mview。实现见 [`../core/mview_refresh_builder.rs`](../core/mview_refresh_builder.rs)、[`../core/mview_refresh_lookup.rs`](../core/mview_refresh_lookup.rs)；统计再导出见 [`stats.rs`](stats.rs)。导出可用性不证明服务器或 executor 已自动调用这些入口。

## 错误处理与边界

无自身错误检查；Build/lookup 的 Result<String> 与 stats 的 Option 原样保留。不能从导出函数名认定完整 Go AST、存储刷新或统计过滤识别能力已移植。

## 并发与资源生命周期

无函数体、线程、锁、异步、资源创建或清理，生命周期取决于导出函数和数据类型。

## 与 Go 版本的对应关系

对应 [`mview.go`](mview.go) 的包级职责，但 Go 文件包含 SQL AST 重建、增量/全量 diff 构造与 hints 等实际算法；Rust 此文件为再导出层，算法子集位于 core，不能以文件同名认定逐功能等价。

## 扩展指南

新增导出先确认实现所属 crate，保持依赖单向；需要公开用户刷新/内部实现入口时明确增补 API，而非假设通配导出已包含。行为测试放在实现所属 core 的独立测试文件。

## 验证依据

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准，增量范围为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`；不纳入工作区后续源码。

已审计新增源码全体、core `lib.rs`、core 与 planner 的 `Cargo.toml`、planner `lib.rs`，以及独立 `core/casetest/mview/mv_refresh_test.rs`。RustCodeGraph 当前索引包含 Rust；冻结事实采用 `git show`/`git diff`，不将当前图关系当成冻结调用关系。目标 core/mview 包均无 `doc.go`。Ready 技能目录缺失，使用任务指定结构、diff、空白和相对链接检查；纯文档任务不运行 Cargo/Go 测试，不把测试源码当作已执行结果。

核对全部 pub use 项及两个别名；对应实现测试为 `core/casetest/mview/mv_refresh_test.rs`，没有本文件独立运行时行为测试。虽然总体计划通常排除薄转发，本文件是任务明确要求的新增说明，按指定范围记录其实际导出边界。
