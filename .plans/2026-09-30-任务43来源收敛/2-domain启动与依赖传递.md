# 任务 2: Domain 启动门禁与依赖传递

批次：【批次 2】依赖批次：1

状态：未开始

目的：仅对齐原始 Go 差异中的外部工作负载角色、TTL 启动门禁、serverinfo 选项和关闭顺序。

来源任务：43，`pkg/domain/domain.go`、`domain_sysvars.go`、`domain_test.go`

预计会话范围：围绕 Domain 的构造、`start`、`close` 和关联测试，不重写 TTL 作业执行器。

## 文件

- 修改：`pkg/domain/domain.rs`、必要时 `pkg/domain/canonical_domain.rs`。
- 测试：`pkg/domain/canonical_domain_test.rs`、`pkg/domain/sysvar_cache_test.rs`。
- Go 对照：`pkg/domain/domain.go`、`domain_sysvars.go`、`domain_test.go`。

## 上下文

- Go `shouldStartTTLJobManager` 在外部负载已配置时仅允许有存活 manager 的 TTL worker 角色启动。
- Go `Init` 将 external workload manager 交给 DDL，serverinfo syncer options 进入真实 syncer；`Close` 关闭 inference providers。
- 当前 Rust 已有这些路径及回归，任务先核对差异，不以再次扩展子系统为完成条件。

## 测试计划

- 行为：没有 controller 时不启动 TTL；有 TTL worker manager 时启动，master 角色才执行相应全局变量转发；构造选项到达真实 serverinfo syncer，关闭释放注册。
- 失败验证测试：如发现未覆盖分支，在 `canonical_domain_test.rs` 加 `go_merge_43_domain_startup_option_parity`，先让缺口分支失败。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-domain cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43`
- 预期失败原因：找到的门禁或选项传递缺口产生错误启动状态/注册状态；修复后相关断言通过。
- 模拟策略：单元层用现有 Domain 测试依赖，etcd 边界使用已有 fake；真实 etcd 只在后续集成阶段验证。

## 步骤

1. 读 `pkg/domain/doc.go`（若存在）和 Go 差异、相邻 Rust 构造与测试。
2. 先跑当前聚焦测试；若发现行为缺口，写失败回归并记录预期失败。
3. 只修复测试证实的 Domain 分支、传参或关闭路径。
4. 复跑聚焦测试，审查是否触发 `make bazel_prepare` 条件。

## 完成

报告 Go 函数→Rust 符号、门禁和选项结果、失败→通过证据、风险与精确命令。变更达到 Ready 后删除本文件；无缺口时以源码和现有测试证据完成，不造新测试。
