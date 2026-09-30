# 任务 4: crossks owner 适配层必要性与 Go 语义

批次：【批次 4】依赖批次：1,3

状态：未开始

目的：判定 `crossks_owner.rs` 中每项自定义 DDL owner 行为是否必须，并与 Go owner/DDL worker 语义对齐。

来源任务：43 的 crossks 生产接线扩展；该 Rust 文件无同名 Go 文件

预计会话范围：只审查和修正目标 keyspace DDL owner 的选举、持久作业消费与关闭，不扩展新 DDL 作业类型。

## 文件

- 修改：`pkg/session/runtime/crossks_owner.rs`、必要时 `crossks_runtime.rs`。
- 测试：`pkg/session/runtime/crossks_owner_test.rs`、`crossks_runtime_test.rs`。
- Go 对照：`pkg/domain/crossks/cross_ks.go`、`pkg/domain/domain.go`、`pkg/ddl/ddl.go`、`pkg/ddl/job_worker.go`；按符号追到实际 owner/worker 函数。

## 上下文

- `crossks_owner.rs` 目前同时含自建 etcd compare-and-put 路径及共享 `astersql_owner::Manager` 路径；原始 Go crossks diff 本身没有定义新的 owner 算法。
- 先证明其必要边界；能用现有 Rust Go 对应 owner/worker 时优先复用，无法证明的分支应收敛，而非继续增加策略。

## 测试计划

- 行为：目标 keyspace 仅一个 DDL owner 消费持久 AlterTableMode 作业；失主后另一实例接续；关闭释放 lease，不能误领其它 DDL 类型。
- 失败验证测试：在 `crossks_owner_test.rs` 新增或扩展 `go_merge_43_crossks_owner_go_semantics`，针对核查发现的一个具体偏差先失败。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-session cargo test --manifest-path pkg/session/Cargo.toml --lib go_merge_43_crossks_owner`
- 预期失败原因：选举或作业过滤与 Go 源码不同，导致重复执行/错误领取/未释放。
- 模拟策略：单元回归用现有 fake etcd；真实 PD/TiKV 的既有独立回归用于最终阶段，不以 fake 证明真实租约行为。

## 步骤

1. 读 Go owner/worker 实际函数及 Rust 调用链，不按文件名猜映射。
2. 记录每个自定义分支的 Go 来源或 Rust 非 `Send` 会话适配理由。
3. 针对确切偏差先写失败回归，再替换/缩减代码并复跑。
4. 审查移除路径是否仍被其它共享任务使用；保留未确认归属的代码并记录。

## 完成

报告 `crossks_owner.rs` 分支级保留/收敛判定、选举与消费结果、失败→通过命令。满足 Ready 后删除本文件；来源仍有歧义时只在本文件写阻塞证据。
