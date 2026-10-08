# `pkg/session/runtime/paging.rs`

## 文件定位

本文件属于 `astersql-session` crate 的 session 运行时实现。`pkg/session/runtime.rs` 以私有模块 `mod paging` 装配它，并仅向 crate 外重新导出 `SetResourceGroupRuntimeStates`；其余函数服务于 `ConcreteSession` 内部。文件处在“资源组状态/全局变量”到“具体 coprocessor 请求”的边界：它决定本条语句是否应携带 `Paging.PagingSizeBytes`，真正把预算写进请求的入口位于 `runtime/relational_scan.rs`、`runtime/dispatch.rs` 和 `runtime/canonical_table_reader.rs`。

`pkg/session/Cargo.toml` 将该代码归入 `astersql-session`，并直接依赖 `astersql-domain`、`astersql-meta-model`、`astersql-sessionctx-stmtctx`、`astersql-sessionctx-vardef` 和 `astersql-store-driver`。其中 `astersql-store-driver` 在 `pkg/store/driver/lib.rs` 重新导出固定 tag `v0.4.2-aster.10` 的 `tikv-client` 运行时资源组状态。

## 核心职责

1. 用 `ControllerRuntimeStates` 把 TiKV client 维护的 `ResourceGroupRuntimeStates` 适配成 Domain 所需的 `ResourceGroupRuntimeStateProvider` trait。
2. 由 `SetResourceGroupRuntimeStates` 安装或卸载上述状态视图，使 session 的分页判断能看到 PD token response 的最新 burst 状态。
3. 由 `resource_group_allows_paging_size_bytes` 判断指定资源组当前是否属于“有限 burst”组：已有运行时状态优先；没有可用运行时状态时才回退到本进程的资源组元数据。
4. 由 `effective_paging_size_bytes` 同时应用预算值、资源控制开关和资源组资格规则。
5. 由 `ConcreteSession::cop_paging_size_bytes` 在语句级 DistSQL cache 中捕获最终预算，保证同一语句已构造的请求不会因后续全局配置或 token 状态变化而漂移。

本文件只计算和缓存预算，不执行分页、不发送 RPC，也不实现响应翻页。下游 store/coprocessor 代码如何消费 `PagingSizeBytes` 不属于这里的职责。

## 主要符号

- `ControllerRuntimeStates(Arc<ResourceGroupRuntimeStates>)`：私有 newtype，持有共享的 client-rust 运行时状态集合。
- `impl ResourceGroupRuntimeStateProvider for ControllerRuntimeStates`：`has_limited_burst(name)` 查询 `get_resource_group_runtime_state(name)`；已收到状态时返回其中的 `has_limited_burst`，未知组或尚无响应时返回 `None`。
- `pub fn SetResourceGroupRuntimeStates(domain, states)`：Go 风格公开 API。`Some` 被封装成 trait object 后交给 `Domain::set_resource_group_runtime_states`，`None` 清除 provider。`runtime.rs` 对它执行 `pub use`。
- `pub(super) fn resource_group_allows_paging_size_bytes(domain, name) -> bool`：资格判断。无 Domain 或空名称立即拒绝；Domain provider 的 `Some(true/false)` 是权威结果；仅 `None` 才查询 `RUNTIME_RESOURCE_GROUPS`。
- `pub(super) fn effective_paging_size_bytes(domain, name, budget, enabled) -> i64`：如果正预算遇到资源控制关闭或资源组不合格，返回 `0`；否则原样返回预算。保留非正值使纯判定函数不擅自改变输入，最终公开请求值再由调用方钳制。
- `PagingByteBudget(u64)`：语句缓存中的私有强类型包装，派生 `Clone + Copy`。
- `ConcreteSession::cop_paging_size_bytes(name) -> u64`：请求构造使用的主入口。首次调用读取全局预算与资源控制状态，计算并缓存 `PagingByteBudget`；后续调用复用缓存。

## 执行流程

1. 资源组 controller 建立后，调用方可通过 `SetResourceGroupRuntimeStates` 把 `Arc<ResourceGroupRuntimeStates>` 绑定到所属 Domain。Domain 内部用可选 trait object 保存该 provider。
2. SQL 请求构造路径先确定 `ResourceGroupName`，随后调用 `ConcreteSession::cop_paging_size_bytes`。已确认的调用点包括 `relational_scan.rs` 的普通/ANN/校验扫描，`dispatch.rs` 的并行请求，以及 `canonical_table_reader.rs` 的规范表读取。
3. `cop_paging_size_bytes` 调用 statement context 的 `GetOrInitDistSQLFromCache`。缓存未初始化时：
   - 从 `astersql_sessionctx_vardef::PagingSizeBytes.Load()` 读取进程内全局预算；
   - 优先从 Domain 的全局系统变量缓存读取 `tidb_enable_resource_control`，缺失时回退到 `EnableResourceControl.Load()`；
   - 调用 `effective_paging_size_bytes` 完成资格过滤；
   - 用 `.max(0) as u64` 将负值钳制为请求可表达的零，并缓存 `PagingByteBudget`。
4. 资格过滤调用 `resource_group_allows_paging_size_bytes`。若 Domain provider 对该名称有运行时结论，直接采用；否则锁住 `RUNTIME_RESOURCE_GROUPS`，按 `runtime_domain_id(domain)` 定位 Domain，再以小写资源组名查询元数据。
5. 元数据命中后临时构造 `astersql_meta_model::group_3::ResourceGroupSettings`，调用 `GetBurstLimitAdjusted()`；结果大于等于零代表有限 burst，允许字节预算。缺失、无限速率或无限 burst 等不满足条件的状态均拒绝。
6. 请求构造器把返回值写入 `request.Paging.PagingSizeBytes`。下一条语句或 `StmtCtx.ResetForRetry()` 建立新的 DistSQL cache 时才重新捕获配置与运行时状态。

## 数据与状态

- controller 状态通过 `Arc<ResourceGroupRuntimeStates>` 共享，内容由真实 PD token response 更新；本文件只读，不复制快照。
- Domain 保存 `Option<Arc<dyn ResourceGroupRuntimeStateProvider>>`。`Some(false)` 与 `Some(true)` 都是权威运行时事实，只有 `None` 表示可以回退元数据。这一区分避免把“明确无限 burst”误当成“尚无状态”。
- 元数据回退源是 `RUNTIME_RESOURCE_GROUPS`，先按 `runtime_domain_id` 隔离不同 Domain，再按小写资源组名称查找，因此名称匹配不区分大小写。
- `PagingSizeBytes` 与 `EnableResourceControl` 是进程级可更新状态；启用开关另有 Domain 全局系统变量缓存作为优先来源。
- `PagingByteBudget` 存入 `StmtCtx` 的 DistSQL cache，生命周期与 statement context 一致。其 `u64` 形态正好匹配 cop 请求字段，且负输入在写入缓存前被归零。

关键不变量是：正的分页字节预算只有在资源控制开启且资源组具有有限 burst 时才能进入请求；运行时状态一旦存在便覆盖元数据；已经捕获的语句预算不被后续全局更新或 controller 更新原地改写。

## 依赖与调用关系

上游装配与状态来源：

- `pkg/session/runtime.rs` 声明模块并公开 `SetResourceGroupRuntimeStates`。
- `pkg/domain/resource_group_runtime.rs` 定义 `ResourceGroupRuntimeStateProvider: Send + Sync`。
- `pkg/domain/domain.rs` 的 `set_resource_group_runtime_states` 和 `resource_group_has_limited_burst` 分别管理、读取 provider；Domain 用读写锁保护它。
- `pkg/store/driver/lib.rs` 从带 tag 的 `tikv-client` 重新导出 `resource_group_runtime`，供适配器读取 PD token response 派生状态。

下游请求调用者：

- `pkg/session/runtime/relational_scan.rs` 在多种关系扫描请求上设置 `Paging.PagingSizeBytes`。
- `pkg/session/runtime/dispatch.rs` 为并行构造的请求设置同一语句捕获的预算。
- `pkg/session/runtime/canonical_table_reader.rs` 在 canonical table reader 请求上设置预算。

内部调用链为 `cop_paging_size_bytes` → `effective_paging_size_bytes` → `resource_group_allows_paging_size_bytes`。RustCodeGraph 能确认后两条直接调用边及 `ControllerRuntimeStates`/`PagingByteBudget` 的构造边；对跨文件 method 调用，索引因同名符号未完整给出 callers，已用上述真实请求赋值点补充核验。

## 错误处理与边界

- API 不返回 `Result`：无 Domain、空资源组名、未知资源组或无有效元数据均安全降级为“不允许”，不会把正预算发送出去。
- controller 尚未收到某组的 token response 时返回 `None`，允许元数据回退；controller 明确返回 `false` 时禁止回退。
- `budget <= 0` 时 `effective_paging_size_bytes` 原样返回；`cop_paging_size_bytes` 再通过 `max(0)` 保证 RPC 字段不会发生有符号转无符号环绕。
- `RUNTIME_RESOURCE_GROUPS` 的互斥锁即使被 poison，也通过 `PoisonError::into_inner` 继续读取现有数据。
- Domain 的 provider 读写锁若 poison，会在 `domain.rs` 中以明确消息 panic；本文件不吞掉该一致性故障。
- statement cache 若含有非 `PagingByteBudget` 类型，`cache_downcast_ref(...).expect(...)` 会 panic。这要求该 cache 插槽始终由本逻辑以一致类型初始化。
- 名称在 controller 查询时保持原样，在元数据回退时转为小写；扩展 provider 时必须继续保证与资源组名称规则兼容。

## 并发与资源生命周期

`ControllerRuntimeStates` 和 Domain provider 都由 `Arc` 持有，trait 又要求 `Send + Sync`，因此多个 session/线程可共享同一 controller 状态视图。Domain 用 `RwLock` 原子替换 provider；安装新的 `Arc` 不会破坏仍持有旧引用的调用栈，传入 `None` 只影响后续查询。

元数据回退在 `RUNTIME_RESOURCE_GROUPS` 的互斥锁保护下完成，临界区包含 Domain/名称查找及 burst 计算；这里没有异步等待或网络调用。语句缓存把变化中的全局预算和 controller 状态收敛为一次性 `u64`：同一 statement context 内并发构造的请求看到一致预算，而重置/新建 statement context 后重新采样。文件本身不创建线程、任务、通道或事务，也不负责 controller/Domain 的关闭。

## 与 Go 版本的对应关系

直接 Go 对照位于 `pkg/session/session.go`：

- Go `GetDistSQLCtx` 在首次创建 statement 的 `DistSQLContext` 时读取 `vardef.PagingSizeBytes`，并在资源控制关闭或 `resourceGroupAllowsPagingSizeBytes` 为假时清零；Rust 将相同决策拆为 `effective_paging_size_bytes` 与 `ConcreteSession::cop_paging_size_bytes`。
- Go `resourceGroupAllowsPagingSizeBytes` 同样先查询 `Domain.ResourceGroupsController()` 的 runtime state，再回退 `InfoSchema().ResourceGroupByName(...)`，并以 `GetBurstLimitAdjusted() >= 0` 判定有限 burst。
- Rust 没有直接把具体 controller 类型塞进 session 判断，而是增加 `ResourceGroupRuntimeStateProvider` 适配层，并用 `RUNTIME_RESOURCE_GROUPS` 作为当前 Rust SQL DDL/information_schema 路径的元数据所有者。这是实现接线差异，不改变“运行时状态优先、未知才回退元数据”的意图。
- Go 把预算随完整 `DistSQLContext` 缓存；Rust 当前只把 `PagingByteBudget` 放入已有 statement DistSQL cache，并在各 concrete cop 请求构造点显式写入字段。两者都保证已初始化上下文/请求不因全局变量更新而改变。

Go 回归证据位于 `pkg/session/tidb_test.go`：`TestDistSQLCtxPagingSizeBytesRequiresHardCappedResourceGroup` 覆盖硬限流、无限 burst 和资源控制关闭；`TestDistSQLCtxPagingSizeBytesGlobalUpdate` 覆盖跨 session 全局更新及已有上下文稳定性。Rust 对应覆盖见下一节列出的独立测试。

## 扩展指南

- 若新增一种 cop 请求构造路径，必须在资源组名最终确定后调用 `cop_paging_size_bytes`，并把结果写入 `Paging.PagingSizeBytes`；应搜索现有赋值点，避免只覆盖普通 table scan 而遗漏 checksum、ANN 或并行请求。
- 若改变资源组资格规则，优先修改 `resource_group_allows_paging_size_bytes`，保持 `Some(false)` 的运行时权威性和 `None` 才回退元数据的不变量；同步更新 `runtime/scan_adapter_runtime_test.rs` 中 capped/unlimited/moderated、未知组、空名称、tombstone 和大小写案例。
- 若改变预算或资源控制变量读取方式，应修改 `cop_paging_size_bytes`，并验证 statement cache 仍能隔离已捕获请求；同步更新 `pkg/session/tests/paging_global.rs` 与 Go `pkg/session/tidb_test.go` 的跨 session/重置语义。
- 若改变 controller 类型或上游 client-rust 状态结构，应在独立上游仓库完成依赖移植并发布 tag，再更新 `astersql-store-driver` 依赖；不要在本仓库复制或本地 patch 依赖。
- 测试逻辑应继续放在独立 Rust 测试文件，不能内嵌进 `paging.rs`。RPC 字段传播与完整多页结果应扩展 `pkg/session/tests/paging_rpc.rs`；session 决策单元/集成场景应扩展 `runtime/scan_adapter_runtime_test.rs` 或 `tests/paging_global.rs`。
- 性能上应避免在请求热路径增加网络访问或扩大全局锁临界区；兼容性上必须保持预算为零时的既有行为，以及 `tidb_enable_resource_control=off` 时强制禁用正预算。

## 验证依据

- RustCodeGraph：`status` 确认索引含目标文件；`files --filter pkg/session/runtime/paging.rs` 确认文件与 11 个符号；`node --file ... --offset 1 --limit 400` 读取完整 112 行；`query` 定位 `SetResourceGroupRuntimeStates`、`resource_group_allows_paging_size_bytes`、`effective_paging_size_bytes`；`callers/callees` 确认内部调用和类型构造关系，并暴露跨文件 method callers 未完整解析的限制。
- 生产源码：`pkg/session/runtime/paging.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/relational_scan.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/canonical_table_reader.rs`、`pkg/domain/resource_group_runtime.rs`、`pkg/domain/domain.rs`、`pkg/store/driver/lib.rs`。
- crate/依赖：`pkg/session/Cargo.toml`；上游运行时状态的实际 tag 由 `pkg/store/driver/Cargo.toml` 核验。
- Go 对照：`pkg/session/session.go` 的 `GetDistSQLCtx` 与 `resourceGroupAllowsPagingSizeBytes`；`pkg/session/tidb_test.go` 的两个分页预算回归测试。
- Rust 独立测试：`pkg/session/runtime/scan_adapter_runtime_test.rs` 覆盖 SQL 请求传播、运行时状态覆盖元数据、statement retry cache 和资源控制开关；`pkg/session/tests/paging_global.rs` 覆盖全局更新、事务可见性与旧请求稳定性；`pkg/session/tests/paging_rpc.rs` 覆盖预算更新期间的真实 RPC 多页与完整 SQL 结果。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前用任务规定的命令确认恰有十一个固定二级标题，并人工复核符号、调用边、边界与扩展测试位置。
