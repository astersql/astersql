# `pkg/dxf/framework/storage/nodes.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-dxf-framework-storage`，不是一个独立的 Rust 模块：crate 根 `pkg/dxf/framework/storage/lib.rs` 通过 `include!("nodes.rs")` 将它直接展开到 crate 根作用域。因此这里的自由函数和 `impl TaskManager` 方法都是 storage crate 的公开 API，可直接使用 `astersql_dxf_framework_storage::GetDXFCPUCount`、`TaskManager::GetAllNodes` 等路径访问。

它位于 DXF（分布式执行框架）的持久化边界，连接三类信息：进程内的本机资源快照、`mysql.dist_framework_meta` 中的受管节点记录，以及 `mysql.tidb_background_subtask` 中用于调度判断的执行节点/并发信息。节点记录经 `pkg/dxf/framework/scheduler/storage_adapter.rs` 提供给调度器，经 `pkg/dxf/framework/handle/status.rs` 提供给状态查询；执行器启动与恢复路径则通过 `pkg/dxf/framework/taskexecutor/manager.rs` 更新本机记录。

## 核心职责

1. 用全局 `nodeResource` 保存本进程可供 DXF 使用的 CPU、内存和磁盘快照，并向注册逻辑暴露 CPU 数。
2. 通过 `InitMeta` / `RecoverMeta` 将执行节点的 ID、角色和 CPU 数写入 `mysql.dist_framework_meta`；恢复时刻意不覆盖角色。
3. 查询或清理受管节点：按 host 稳定排序读取全部节点，并在事务中删除已确认死亡的 host。
4. 从活跃子任务推导调度状态：列出有 pending/running 子任务的节点，并按执行节点汇总已占用 slot。
5. 为调度/扩缩容选择节点 CPU：可取任意第一个有效节点，或只取指定 role 下第一个 CPU 大于零的节点。

本文件不负责判断节点是否死亡、选举 Owner、计算任务所需节点数或执行重试；这些策略分别位于 scheduler、handle 和 taskexecutor 层。本文件只提供资源快照和 SQL 存取原语。

## 主要符号

- `static nodeResource: RwLock<Option<proto::NodeResource>>`：进程级资源快照。源码静态初始化为 `Some(8 CPU, 16 GiB, 100 GiB)`。
- `GetNodeResource() -> Option<proto::NodeResource>`：持读锁，将三个字段复制到一个新 `NodeResource` 后返回；调用者不能借由返回值修改全局对象。
- `SetNodeResource(proto::NodeResource)`：持写锁整体替换快照。当前公开 API 没有将快照设置为 `None` 的入口。
- `GetDXFCPUCount() -> i32`：读取快照的 `TotalCPU`；理论上的 `None` 返回 0。
- `TaskManager::InitMeta` / `InitMetaSession`：前者获取新 session，后者执行带 upsert 的注册 SQL；冲突时同时更新 `cpu_count` 与 `role`。
- `TaskManager::RecoverMeta`：使用新 session 执行 upsert；冲突时只更新 `cpu_count`，保留已有 `role`，以避免与 `tidb_service_scope` 更新竞态。
- `TaskManager::DeleteDeadNodes`：空列表直接成功；非空列表组装 `host in (...)` 删除语句，并在 `WithNewTxn` 中执行。
- `TaskManager::GetAllNodes` / 私有 `getAllNodesWithSession`：在 tracing region 和新 session 中查询节点，按 `host` 排序，并把三列转换为 `proto::ManagedNode`。
- `TaskManager::GetBusyNodes`：查询 pending/running 子任务的 distinct `exec_id`，转换为 `schstatus::Node { IsOwner: false }`。Owner 标记由 `handle/status.rs::GetBusyNodes` 后续补充。
- `TaskManager::GetUsedSlotsOnNodes`：先按 `(exec_id, task_key)` 取 `max(concurrency)`，再按 `exec_id` 求和，避免同一任务的多个 subtask 重复计算相同并发配额。
- `TaskManager::GetCPUCountOfNode` / `GetCPUCountOfNodeByRole` / 私有 `getCPUCountOfNodeByRole`：复用全部节点查询，选择第一个符合角色条件且 `CPUCount > 0` 的节点。
- `init()`：再次写入与静态初值相同的默认资源。它是普通 Rust 函数而非语言自动执行的初始化钩子；即使未显式调用，静态值已经提供同一默认值。

## 执行流程

节点注册流程为：taskexecutor 的 `Manager::InitMeta` 通过抽象 `TaskTable::InitMeta` 发起带重试的注册，storage 实现进入 `TaskManager::InitMeta`，从 session 池借出 session，再由 `InitMetaSession` 注入故障点、读取当前 CPU 快照并执行 upsert。后台恢复循环走 `RecoverMeta`，仍刷新 CPU，但不改变已有 role。

调度器读取流程为：`scheduler/storage_adapter.rs` 的 `all_nodes` 调用 `GetAllNodes`；后者启动 region、获取 session、执行 `order by host` 的查询并逐行映射。调度节点维护逻辑根据该结果识别死亡节点，再由适配器的 `delete_dead_nodes` 调用 `DeleteDeadNodes` 完成事务删除。稳定的 host 排序使上层节点选择顺序可重复。

slot 汇总流程为：`used_slots_on_nodes` 调用 `GetUsedSlotsOnNodes`；内层 SQL 只考虑 pending/running 状态，对同一节点同一任务的各 subtask 取最大 concurrency，外层再对节点求和，最后把 decimal 结果转为整数并建立 `HashMap<exec_id, slots>`。

状态查询流程中，`GetBusyNodes` 只返回数据库中有活跃子任务的执行节点，初始 `IsOwner=false`；`handle/status.rs` 再查询 Owner ID，更新已有项或追加 Owner。`GetNodesInfo` 同样通过 runtime/adapter 取得全部受管节点，并使用首项 CPU 计算展示信息。

CPU 按角色选择流程为：公开方法获取 session，私有方法复用 `getAllNodesWithSession`；无任何节点时报 `no managed nodes`，有节点但没有匹配 role 且 CPU 大于零的记录时报 `no managed node have enough resource for dist task`。`arbitrary=true` 时忽略角色，但仍要求 CPU 为正。

## 数据与状态

进程内状态只有 `nodeResource`。读操作返回字段副本，写操作原子地替换整个 `Option`；不会出现只更新 CPU 而内存/磁盘仍是旧值的部分快照。默认容量使用 `units::GiB`，即 16 GiB 内存与 100 GiB 磁盘。

持久化节点状态位于 `mysql.dist_framework_meta`：`host` 对应执行节点 ID（通常由 `GenerateExecID` 形成 IP:port）、`role` 表示服务作用域、`cpu_count` 是注册当时的 DXF CPU 快照，`keyspace_id` 在这里固定写为 `-1`。`GetAllNodes` 只读取前三列。

调度占用状态来自 `mysql.tidb_background_subtask` 的 `exec_id`、`task_key`、`concurrency` 和 `state`。只有 `SubtaskStatePending` 与 `SubtaskStateRunning` 被视为忙碌/占用资源；终态子任务不会进入结果。

## 依赖与调用关系

- crate 边界由 `pkg/dxf/framework/storage/Cargo.toml` 定义；本文件直接使用 crate 根提供的 `proto`、`schstatus`、`sessionctx`、`sqlexec`、`sqlescape`、`tracing`、`injectfailpoint`、`units`、`Context`、`Error` 和 `TaskManager`。其中 `proto` 与 `schstatus` 分别重导出相邻 workspace crate。
- `TaskManager::WithNewSession`、`WithNewTxn`、`ExecuteSQLWithNewSession` 定义于 `task_table.rs`：它们负责 session 借还、事务提交/回滚和 SQL 执行，本文件不自行管理连接池。
- `pkg/dxf/framework/taskexecutor/manager.rs` 是 `InitMeta` / `RecoverMeta` 的直接业务上游；它在初始化及后台恢复循环中重试这些操作。
- `pkg/dxf/framework/scheduler/storage_adapter.rs` 把 `GetAllNodes`、`DeleteDeadNodes`、`GetUsedSlotsOnNodes` 转换为 scheduler 内部接口；上层节点维护和 slot 分配策略不在本文件。
- `pkg/dxf/framework/scheduler/autoscaler.rs::GetExecCPUNode` 按当前 target scope 调用 `GetCPUCountOfNodeByRole`（该路径带 `target_os = "windows"` 条件编译）。
- `pkg/dxf/framework/handle/status.rs` 消费全部节点和忙碌节点，负责 API 状态组装及 Owner 标记。
- `proto::ManagedNode` 与 `proto::NodeResource` 的定义位于 `pkg/dxf/framework/proto/node.rs`；本文件不定义节点标识格式或资源限额计算。

RustCodeGraph 将本文件标为被 8 个文件使用，并可定位 taskexecutor、scheduler adapter、handle status 等入口；由于方法是经 `include!` 展开，索引的具名 callers 结果不完整，以上精确边由相邻 Rust 源码引用核验。

## 错误处理与边界

所有数据库入口都以 `Result<_, Error>` 返回，并用 `?` 原样传播故障注入、session 获取、SQL 执行、事务提交或行数值转换错误。`GetAllNodes` 无论内部闭包成功与否都会在传播错误前调用 `Region::End`；当前 crate 的 `tracing::Region` 是占位实现，不应据此推断已有真实 span 上报。

资源锁中毒通过 `expect("node resource lock poisoned")` 触发 panic，而不是转换成 `Error`。这是进程内不变量失败路径。`GetDXFCPUCount` 保留无快照时返回 0 的兼容分支，但当前静态初始化和公开 setter 都不会产生 `None`。

`DeleteDeadNodes` 对空输入不打开事务。非空输入把节点字符串插入双引号列表；这与 Go 版本的 `fmt.Sprintf` 行为一致，但本函数自身没有对 host 内容做参数绑定或额外转义，调用方必须只传入可信、规范化的节点 ID。删除操作经 `WithNewTxn` 执行，闭包成功才提交，失败则回滚。

CPU 查询只接受严格大于零的 `CPUCount`。存在节点不代表一定可调度：角色不匹配、CPU 为零或负数都会被跳过。`GetUsedSlotsOnNodes` 沿用 Go 行为，忽略 `ToInt()` 返回的辅助状态，只使用转换出的整数；扩展数值类型或溢出处理时需要单独评估。

## 并发与资源生命周期

`nodeResource` 使用标准库 `RwLock`：多读可并发，更新独占；`GetNodeResource` 在释放读锁前复制字段，所以返回对象的生命周期与锁无关。该机制对齐 Go `atomic.Pointer` 的整快照语义，但不是无锁实现。

每个公开数据库操作按调用获取独立 session。`WithNewSession` 临时覆盖事务 entry size 限制，并在闭包结束后恢复限制、归还 session；`WithNewTxn` 依次执行 BEGIN、业务闭包、Commit 或 Rollback。`DeleteDeadNodes` 是本文件唯一显式事务操作，其余操作均为单条查询/upsert 或在一个 session 内完成。

本文件不创建线程、异步任务或通道。周期性恢复线程由 taskexecutor `Manager::Start` 创建，重试和退出生命周期也由该层管理。全局资源快照和数据库节点表之间没有跨两者的联合事务：注册时只读取一次快照并将该时刻的 CPU 值写入表。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/storage/nodes.go`。Rust 保留了 Go 的 SQL 结构、状态过滤、host 排序、两阶段 slot 聚合、错误文案，以及 `InitMeta` 更新 role 而 `RecoverMeta` 不更新 role 的关键差异。`pkg/dxf/framework/storage/Cargo.toml` 的 `package.metadata.porting.go-package` 也将该 crate 映射到 `pkg/dxf/framework/storage`。

主要语言映射如下：Go 的 `atomic.Pointer<NodeResource>` 对应 Rust 的 `RwLock<Option<NodeResource>>`；Go 指针返回对应 Rust 字段复制后的 owned 值；Go `[]ManagedNode` / `map[string]int` 对应 Rust `Vec<ManagedNode>` / `HashMap<String, i32>`；Go `defer r.End()` 对应 Rust 在错误传播前显式调用 `r.End()`。

两处当前实现差异需要扩展者留意。第一，Go 包级 `init()` 会自动执行，而 Rust 的同名函数只是普通函数；Rust 静态值已直接设置为相同默认资源，因此当前默认行为一致。第二，Go 的初始 `atomic.Pointer` 在 `init` 前语义上可为 nil，Rust 静态初始化后立即为 `Some`，且公开 setter 不能清空它。

Go 的真实表回归 `table_test.go::TestDistFrameworkMeta` 验证了排序、角色更新/保留、删除、无节点/无有效资源错误和按角色选 CPU；`TestInitMeta` 验证 service scope 交互。Rust 侧 `converter_1_aster_unit_test.rs::node_queries_preserve_order_role_filter_and_slot_aggregation` 与 `table_test.rs::TestGetUsedSlotsOnNodesAndBusyNodes` 验证 SQL 结果映射、slot 聚合、忙碌节点及角色过滤。Rust 测试目前不是 Go 全部节点生命周期用例的一比一复刻，不能把 Go 独有覆盖视作 Rust 已执行验证。

## 扩展指南

- 新增节点持久化字段时，应同步修改 `proto::ManagedNode`、注册 upsert、`GetAllNodes` 的 select/行映射、scheduler adapter 的转换，以及独立 Rust/Go 测试；保持列顺序与类型一致。
- 修改“忙碌”定义时，应同时审查 `GetBusyNodes` 和 `GetUsedSlotsOnNodes` 的状态集合，避免状态展示与 slot 计费产生分歧，并更新 `table_test.rs::TestGetUsedSlotsOnNodesAndBusyNodes`。
- 修改 role 语义时，必须保留或有意调整 `InitMeta` 与 `RecoverMeta` 的不同 upsert 行为，并检查 `tidb_service_scope` 竞态及 `table_test.go::TestInitMeta`、`TestDistFrameworkMeta`。
- 修改节点选择顺序时，应从 `getAllNodesWithSession` 的 `order by host` 和 `getCPUCountOfNodeByRole` 的“第一个有效节点”一起评估；这会影响调度可重复性和 CPU 估算。
- 修改死亡节点删除方式时，优先改为参数化、可验证的 SQL 构造，同时维持空输入短路与事务原子性；调用方识别死亡节点的策略仍应留在 scheduler。
- 资源快照新增字段或允许清空时，应重新定义 `GetNodeResource` 的复制语义、锁中毒策略及注册时一致性，并把测试放在独立测试文件（现有最近位置为 `converter_1_aster_unit_test.rs` 或 `table_test.rs`），不要嵌入生产源文件。
- 任何行为迁移都应以 `nodes.go` 和相关 Go 测试为语义基线；本任务仅说明现状，不建议为简化 Rust 实现而删减 Go 分支。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/dxf/framework/storage` 确认 `nodes.rs`、Go 对照与相关测试均在索引中；`node --file pkg/dxf/framework/storage/nodes.rs --offset 1 --limit 400` 读取了完整 272 行源码及 8 个使用文件摘要；另读取了 taskexecutor manager、scheduler storage adapter 与 handle status 的索引节点。具名 `callers GetAllNodes` 未返回边，结合 `include!` 形态判断该查询覆盖不足，随后以精确源码引用补证。
- 生产源码：`pkg/dxf/framework/storage/nodes.rs`、`lib.rs`、`task_table.rs`，`pkg/dxf/framework/proto/node.rs`，`pkg/dxf/framework/taskexecutor/manager.rs`，`pkg/dxf/framework/scheduler/storage_adapter.rs`、`autoscaler.rs`，`pkg/dxf/framework/handle/status.rs`。
- crate/移植边界：`pkg/dxf/framework/storage/Cargo.toml`；Go 对照：`pkg/dxf/framework/storage/nodes.go`、`pkg/dxf/framework/proto/node.go`。
- 独立测试：Rust 的 `pkg/dxf/framework/storage/converter_1_aster_unit_test.rs`、`pkg/dxf/framework/storage/table_test.rs`；Go 的 `pkg/dxf/framework/storage/table_test.go`。本任务遵循纯文档约束，未运行 Cargo 或代码测试。
- 人工复核结论：该文件存在是为了把本机 DXF 资源与系统表中的节点/活跃子任务状态转换成调度可消费的存储 API；安全扩展必须同时维护 SQL、行映射、role 恢复不变量、session/事务边界以及独立测试。
