# 任务 7: MPP、CTE 与执行站点

批次：【批次 7】依赖批次：6

状态：未开始

目的：补齐 Go 的 MPP/TiFlash、shuffle、CTE、scalar subquery 及算子执行站点判定。

来源任务：187

预计会话范围：聚焦跨引擎和特殊树分支；不处理发布层。

## 文件

- 修改：`pkg/executor/statement_ru_plan_walk.rs`、`pkg/executor/statement_ru_reporting.rs`
- 测试：`pkg/executor/statement_ru_plan_walk_test.rs`、`pkg/executor/statement_ru_reporting_test.rs`

## 上下文

对照 Go `calculateStatementRUPlanChildFirst`、`collectStatementRUShuffleUnits`、`statementRUOperatorRunsAtSupportedSite` 及 `statementRUEngineResult`，保留 TiFlash 归属。

## 测试计划

- 行为：特殊树与 MPP 算子只在 Go 支持的站点计入相应引擎；缺运行统计时失败封闭。
- 失败验证测试：增加 `go_merge_187_mpp_cte_site`。
- 失败验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187_mpp_cte_site`
- 预期失败原因：Rust 尚缺对应树分支、shuffle 单元或站点判断。
- 通过验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187_mpp_cte_site`
- 模拟策略：使用真实算子树，TiFlash 统计只在响应边界构造确定快照。

## 步骤

1. 列出 Go 特殊树和站点分支并写失败测试。
2. 确认失败；逐项移植，保留引擎归属和失败状态。
3. 运行通过命令与格式检查。

## 验证

- 格式：Rust 代码修改完成后，先运行 `cargo fmt --all` 自动格式化，再运行 `cargo fmt --all -- --check` 校验；自审格式化产生的差异。
- 运行：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187_mpp_cte_site`
- 预期：总 RU 和 TiFlash/TiKV 分配吻合 Go 公式。
- 所需证据：失败与通过输出、分支清单、退出码。

## 完成

只交付跨引擎与特殊树；在本文件记录阻塞与验证。遵守 Ready 检查。
