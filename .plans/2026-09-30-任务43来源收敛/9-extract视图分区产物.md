# 任务 9: Extract 视图与分区产物

批次：【批次 9】依赖批次：7,8

状态：未开始

目的：验证 Extract 在真实视图依赖和分区表上输出 Go 可读的 schema 与统计产物。

来源任务：43 当前任务文件的 Extract 生产验证；超出原始 `extract.go` 单行差异

预计会话范围：只处理视图和分区表的扩展归档，不重复普通表格式工作。

## 文件

- 修改：`pkg/server/extract_runtime.rs`、必要时 `pkg/domain/plan_replayer_dump.rs`。
- 测试：`pkg/server/runtime_test.rs`、必要时 `pkg/domain/plan_replayer_dump_test.rs`。
- Go 对照：`pkg/domain/extract.go` 的 `handleIsView`/`dumpExtractPlanPackage` 与 `pkg/domain/plan_replayer_dump.go` 的 schema/stats helper。

## 上下文

- Go 在视图 AST 中继续遍历实际表；分区统计写成按物理分区命名的数据，而不是只有逻辑表的 SHOW STATS 行数组。
- 依赖任务 7 的真实 AST 结果和任务 8 的普通表 ZIP 格式。

## 测试计划

- 行为：含嵌套视图的 SQL 归档包括视图 DDL 和底层表 DDL；分区表归档包含每个物理分区所需统计，Go replayer 能解析。
- 失败验证测试：在 `runtime_test.rs` 新增 `go_merge_43_extract_archive_view_and_partitions`，先断言缺失的具体归档条目/内容。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-server cargo test --manifest-path pkg/server/Cargo.toml --lib go_merge_43_extract_archive_view_and_partitions`
- 预期失败原因：现有 dump 仅按 `package.tables` 的逻辑名扫描，缺少分区格式或视图依赖产物。
- 模拟策略：使用真实 parser、InfoSchema、SQL 表及 ExtStorage；不模拟归档内容。

## 步骤

1. 读取 Go 视图/分区 dump helper 与 Rust 当前归档路径。
2. 建立真实视图和分区表测试，先运行失败断言。
3. 仅补齐缺失归档项并验证 Go/Rust 读取端。
4. 检查测试对象与 ExtStorage 产物清理。

## 完成

报告视图、底层表、每个分区的实际 ZIP 条目和解析结果；不足以证明 Go 消费端兼容时不得标记完成。Ready 通过后删除本文件。
