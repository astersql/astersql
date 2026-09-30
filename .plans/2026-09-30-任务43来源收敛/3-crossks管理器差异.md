# 任务 3: crossks 管理器差异收敛

批次：【批次 3】依赖批次：1,2

状态：未开始

目的：对齐 Go crossks manager 新增的虚拟 serverinfo 登记、清理和失败路径。

来源任务：43，`pkg/domain/crossks/cross_ks.go` 及两个测试文件

预计会话范围：仅处理 manager 创建/关闭和虚拟 serverinfo 生命周期；DDL owner 执行器由下一任务单独核查。

## 文件

- 修改：`pkg/domain/crossks/cross_ks.rs`、必要时 `pkg/domain/crossks/export_test.rs`。
- 测试：`pkg/domain/crossks/cross_ks_test.rs`。
- Go 对照：`pkg/domain/crossks/cross_ks.go`、`cross_ks_test.go`、`export_test.go`。

## 上下文

- Go 在目标 keyspace runtime 创建时登记虚拟 serverinfo，失败和关闭时撤销，且在关闭 SQL session pool 前停掉使用它的循环。
- 现有 Rust `new_manager_with_server_info` 和 factory 已实现部分路径；只在源码对照发现差异时修改。

## 测试计划

- 行为：成功、创建失败、晚到登记和关闭四类路径的 serverinfo lease 与 session 释放顺序一致。
- 失败验证测试：在 `cross_ks_test.rs` 扩展 `go_merge_43_` 测试，覆盖核查发现的具体遗漏。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-crossks cargo test --manifest-path pkg/domain/crossks/Cargo.toml --lib go_merge_43`
- 预期失败原因：遗漏的登记/清理路径使 fake etcd 记录或关闭顺序与 Go 不同。
- 模拟策略：使用已有 fake etcd 和 session lifecycle；最终真实 lease 行为沿用已存在的独立集成证据。

## 步骤

1. 逐 hunk 对照 Go 变更与 Rust 符号和相邻回归。
2. 如有缺口，先构造失败回归并运行。
3. 只改 manager 生命周期代码，复跑聚焦测试。
4. 审查所有后续任务共享的 `pkg/domain` 文件，避免覆盖其他工作。

## 完成

报告四类路径的 Go→Rust 对照和失败→通过证据；无需新增行为时说明现有覆盖。满足 Ready 后删除本文件。
