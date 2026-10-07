# pkg/domain/resource_group_runtime.rs

## 文件定位

[pkg/domain/resource_group_runtime.rs](resource_group_runtime.rs) 是 astersql-domain crate 的资源组运行时状态边界。它由 [pkg/domain/lib.rs](lib.rs) 通过公开模块 resource_group_runtime 暴露，但不拥有 PD token 响应状态，也不实现分页策略；文件只定义 Domain 能依赖的抽象查询接口。

crate 边界由 [pkg/domain/Cargo.toml](Cargo.toml) 确定：包名为 astersql-domain，library 入口为 lib.rs。本文件不引用外部 crate；真实控制器类型留在 astersql-store-driver/tikv-client 一侧，避免 Domain 层绑定具体实现。

## 核心职责

- 定义公开 trait ResourceGroupRuntimeStateProvider，为 Domain 提供“指定资源组是否具有受限 burst”的运行时信号。
- 用 Option<bool> 保留三态语义：Some(true) 和 Some(false) 都是已知、可信的运行时结果；None 表示没有可用的本地状态，包括尚未收到首个 token 响应。
- 以 Send + Sync 作为并发契约，使实现可作为 Arc<dyn ResourceGroupRuntimeStateProvider> 在线程间共享。

这是刻意保持微小的依赖倒置边界而非桩：生产实现、Domain 存储和 session 消费链均已接线。

## 主要符号

### pub trait ResourceGroupRuntimeStateProvider: Send + Sync

文件中唯一的类型定义和公开 API。Send + Sync 要求实现者可在线程间转移和并发共享；接口不要求 Clone、Default 或具体存储形式。公开模块声明见 pkg/domain/lib.rs。

### fn has_limited_burst(&self, resource_group_name: &str) -> Option<bool>

按资源组名称查询运行时状态：

- Some(true)：状态确认受限 burst，分页字节预算可用。
- Some(false)：状态确认不受限；这是权威 false，不得再被元数据覆盖。
- None：provider 未安装、指定组无可用状态，或尚无完成的 token 响应；上层可回退到资源组元数据。

resource_group_name 是借用的查询键，不被接口保留。本 trait 没有默认方法、关联类型、常量、条件编译项或文件内实现。

## 执行流程

1. pkg/session/runtime/paging.rs::SetResourceGroupRuntimeStates 接收 Arc<ResourceGroupRuntimeStates>，用 ControllerRuntimeStates 包装并转换为 Arc<dyn ResourceGroupRuntimeStateProvider>；传入 None 则卸载 provider。
2. pkg/domain/domain.rs::Domain::set_resource_group_runtime_states 将 trait object 写入 RwLock<Option<Arc<dyn ...>>>。
3. session 的 resource_group_allows_paging_size_bytes 调用 Domain::resource_group_has_limited_burst(name)，Domain 读取 provider 并调用本接口。
4. ControllerRuntimeStates::has_limited_burst 转调 ResourceGroupRuntimeStates::get_resource_group_runtime_state(name)，再读取状态的 has_limited_burst 字段。该具体状态经 pkg/store/driver/lib.rs 从带 tag 的 tikv-client 依赖重导出。
5. 得到 Some(value) 时 session 直接采用 value；仅得到 None 时，才查询 Rust runtime 拥有的资源组元数据，以 GetBurstLimitAdjusted() >= 0 判定。
6. 结果进入 effective_paging_size_bytes：正预算在资源控制未启用或资源组不允许时归零，否则保留。

## 数据与状态

本文件不存储状态，只规定查询的输入和三态输出。实际状态分布在边界两侧：

- Domain 以 RwLock<Option<Arc<dyn ResourceGroupRuntimeStateProvider>>> 保存当前 provider，构造时为 None（pkg/domain/domain.rs::Domain 及其构造逻辑）。
- provider 的具体状态是 ResourceGroupRuntimeStates，来自 tikv-client 并经 astersql-store-driver 重导出；本 trait 既不注册资源组，也不处理 token 响应。

关键不变量是“已知 false”和“未知”必须区分。若将 Option<bool> 简化成 bool，会丢失元数据回退条件，或让陈旧元数据错误覆盖实时 false。

## 依赖与调用关系

上游与实现者：

- pkg/session/runtime/paging.rs::ControllerRuntimeStates 是仓库内唯一实现。
- pkg/session/runtime/paging.rs::SetResourceGroupRuntimeStates 完成具体类型到 trait object 的适配并调用 Domain setter。
- pkg/domain/domain.rs::Domain::resource_group_has_limited_burst 是 has_limited_burst 的直接调用者；RustCodeGraph 的 symbol trail 给出该调用边。

下游与数据来源：

- trait 方法无函数体，因此没有固定 callee；生产实现调用 ResourceGroupRuntimeStates::get_resource_group_runtime_state。
- pkg/store/driver/lib.rs 重导出 tikv_client::resource_group_runtime。pkg/store/driver/Cargo.toml 与 pkg/domain/Cargo.toml 都固定使用 tikv-client tag v0.4.2-aster.10，本仓库没有复制该上游实现。
- 最终消费者是 pkg/session/runtime/paging.rs::resource_group_allows_paging_size_bytes 与 effective_paging_size_bytes，结果进入 statement 级 DistSQL 分页字节预算。

## 错误处理与边界

接口不返回 Result，因为“无可用状态”是正常分支，以 None 表示：

- provider 未安装或通过 SetResourceGroupRuntimeStates(..., None) 卸载：Domain 返回 None。
- 资源组仅注册、尚未收到 token 响应：具体实现返回 None，session 回退到元数据。
- 查询得到 Some(false)：必须直接拒绝分页预算，不得回退。
- 名称不存在、状态被 tombstone 或名称为空：接口只规定 provider 契约；回退和最终 false 判定在 session 层完成。

Domain 容器的 RwLock 使用 expect("resource group runtime state lock poisoned")，所以锁中毒会 panic；这是 pkg/domain/domain.rs 的容器策略，不是 trait 内部错误分支。

## 并发与资源生命周期

Send + Sync 是本文件的核心并发约束。Domain 通过 Arc<dyn ResourceGroupRuntimeStateProvider> 共享 provider，并以 RwLock<Option<...>> 保护安装和卸载：多个 session 查询可共享读，替换 provider 需要写锁。Arc 保证替换时仍被在途读取持有的旧 provider 不会提前释放。

Domain 构造时 provider 为 None；控制器就绪后由 session runtime 安装包装器；PD token 响应在具体 ResourceGroupRuntimeStates 中更新；查询观察当前值；传入 None 可解绑。本 trait 不启动线程、任务或通道，也不负责取消或关闭控制器。

## 与 Go 版本的对应关系

Go 没有同路径 resource_group_runtime.go 或对应 interface。直接行为对照位于 pkg/session/session.go::resourceGroupAllowsPagingSizeBytes：

1. 先拒绝 nil Domain 或空资源组名。
2. 从 Domain::ResourceGroupsController() 获取具体 Go 控制器；若 GetResourceGroupRuntimeState 命中，直接返回 state.HasLimitedBurst。
3. 仅在无可用运行时状态时，从 InfoSchema 查询资源组并检查调整后的 burst limit。

Rust 保留相同优先级和三态语义，但因 crate 分层不同，将 Go Domain 直接持有 controller 改为“Domain 持有 trait object，session 适配 ResourceGroupRuntimeStates”。Rust 的元数据回退使用 runtime 资源组表，而非 Go InfoSchema；这是当前存储位置差异，不改变接口表达的优先级。

Go 回归 pkg/session/tidb_test.go::TestDistSQLCtxPagingSizeBytesRequiresHardCappedResourceGroup 验证 hard-capped 与 unlimited 元数据分支；Rust 独立测试进一步验证 token 响应覆盖元数据。行为来源 Go 提交为 10292a4f8697f6fe7ae3965f3ebd6656c1ab37ed，Rust 对齐提交为 c2c8c005ec1346b28981b650499378fcbdd18109。

## 扩展指南

- 新增运行时信号时，若仍属于 Domain 需要的最小稳定边界，可在 ResourceGroupRuntimeStateProvider 增加精确方法，并同步 ControllerRuntimeStates 实现和 Domain 转发；不要让 Domain 直接依赖 store-driver 的具体类型。
- 必须保留“未知”和“已知 false”的区别。返回类型变化时同步审查 Domain::resource_group_has_limited_burst 与 resource_group_allows_paging_size_bytes 的回退条件。
- 修改名称、大小写或 tombstone 语义时，需要与带 tag 的 astersql/client-rust 上游实现协同：在上游移植、提交和发布新 tag，再让 Cargo manifests 一致引用；不要复制实现或用本地 patch。
- 测试应留在独立 Rust 测试文件。直接回归位于 pkg/session/runtime/scan_adapter_runtime_test.rs；至少覆盖首个响应前回退、Some(true)、Some(false)、tombstone、provider 卸载和真实 SQL 请求字节数。Go 语义变化时同步对照 pkg/session/session.go 与 pkg/session/tidb_test.go。
- 此查询位于 statement/DistSQL 上下文构建路径；实现中应避免网络 I/O、阻塞等待或长时间持锁。

## 验证依据

- RustCodeGraph 状态：索引包含目标文件；文件节点报告它被 pkg/domain/domain.rs 和 pkg/session/runtime/paging.rs 使用。
- RustCodeGraph 符号：ResourceGroupRuntimeStateProvider 位于第 6 行；has_limited_burst 签名位于第 7 行；方法 trail 指向 pkg/domain/domain.rs::resource_group_has_limited_burst。
- 已读 Rust 生产路径：pkg/domain/resource_group_runtime.rs、pkg/domain/lib.rs、pkg/domain/domain.rs、pkg/session/runtime/paging.rs、pkg/store/driver/lib.rs。
- crate/依赖证据：pkg/domain/Cargo.toml 确认 astersql-domain 与 lib.rs 入口；pkg/store/driver/Cargo.toml 和 Domain manifest 一致引用 tikv-client tag v0.4.2-aster.10。
- 已读 Rust 独立测试：pkg/session/runtime/scan_adapter_runtime_test.rs::paging_runtime_grants_override_real_resource_group_metadata、paging_context_caches_grants_and_budget_until_statement_retry_reset、paging_runtime_grants_are_consumed_by_sql_select_requests。第一项直接覆盖无响应、运行时 true/false、tombstone 和卸载后回退。
- 已读 Go 对照：pkg/session/session.go::resourceGroupAllowsPagingSizeBytes、pkg/domain/domain.go::ResourceGroupsController、pkg/session/tidb_test.go::TestDistSQLCtxPagingSizeBytesRequiresHardCappedResourceGroup，以及 Go 行为提交 10292a4f8697f6fe7ae3965f3ebd6656c1ab37ed。
- 实现历史：c2c8c005ec1346b28981b650499378fcbdd18109 同时新增 trait、Domain 接线、session 适配、store-driver 重导出和 Rust 回归，证明本文件是已接线边界。
- 本任务按计划不运行 Cargo。结构验证应确认文档存在且恰含要求的 11 个二级标题。
