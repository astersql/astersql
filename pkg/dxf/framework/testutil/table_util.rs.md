# `pkg/dxf/framework/testutil/table_util.rs`

## 文件定位

本文件是 DXF（Distributed eXecution Framework）Rust 测试工具 crate `astersql-dxf-framework-testutil` 中的任务表辅助模块，源码见 [`table_util.rs`](./table_util.rs)。crate 入口 [`lib.rs`](./lib.rs) 以 `pub mod table_util` 声明该模块，并用 `pub use table_util::*` 再导出其公开 API；[`Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `pkg/dxf/framework/testutil`。

它不实现生产任务表，也不直接执行 SQL。文件用 `TaskTable`、`TableTestRuntime` 两个 trait 把存储查询、嵌入式测试环境初始化和节点资源替换抽象出来，再提供与 Go 测试工具同名的薄封装及 RAII 清理守卫。仓库搜索未找到任何 `impl TableTestRuntime`，所以当前初始化与资源守卫属于可注入接口，不能视为已经接通真实 `storage.TaskManager` 或嵌入式存储。

## 核心职责

1. `TaskTable` 描述测试所需的最小任务/子任务存储能力：待处理任务查询、当前表与历史表查询、执行节点更新、历史迁移、删除、取消状态检查和诊断输出。
2. `TableTestRuntime` 描述测试环境生命周期：`initialize` 建立环境并返回 store 标识及任务表，`shutdown` 关闭该 store，`set_node_resource` 替换节点资源并返回旧值。
3. `InitTableTest`、`InitTableTestWithCancel` 和私有 `init_table_test` 把运行时产物包装成 `TableTestGuard`，让退出作用域时自动发出取消标志并关闭 store。
4. `GetOneTask` 至 `PrintSubtaskInfo` 将 Go 测试辅助函数迁移为对 `TaskTable` 的类型化委托，并保留少数 Go 特有的容错语义。
5. `MockNodeResource` 按 CPU 数构造测试资源，并用 `NodeResourceGuard` 在析构时恢复旧值。

该模块只服务测试与测试式集成场景，不在 DXF owner 调度、task executor 或任务状态机的生产主链中实现持久化逻辑。DXF 整体职责与任务/子任务状态机可由上层包说明 [`../doc.go`](../doc.go) 复核。

## 主要符号

- `pub trait TaskTable: Send + Sync`：线程安全的任务表边界。其 15 个方法全部返回 `Result<_, DxfError>`；数据类型来自 [`context.rs`](./context.rs) 的 `Task`、`Subtask`、`TaskState`、`DxfError`，写入参数 `NewSubtask` 来自 [`task_util.rs`](./task_util.rs)。本文件自身的查询封装不会持有具体数据库连接。
- `pub trait TableTestRuntime: Send + Sync`：环境注入边界。`initialize(Option<usize>)` 返回 `(String, Arc<dyn TaskTable>)`；`shutdown(&str)` 负责收尾；`set_node_resource(NodeResource)` 返回替换前的资源。
- `pub struct TableTestGuard`：持有 `runtime`、公开的 `store_id`、公开的 `task_manager` 和共享 `cancelled` 原子标志。`cancellation()` 返回共享同一标志的 `Cancellation`。
- `pub struct Cancellation(Arc<AtomicBool>)`：可克隆的协作式取消句柄；`cancel` 用 `Release` 写入，`is_cancelled` 用 `Acquire` 读取。
- `InitTableTest`：给 `initialize` 传入 `Some(8)`，表达 Go 版本默认 mock 8 个 CPU 的意图。
- `InitTableTestWithCancel`：给 `initialize` 传入 `None`，并额外返回与守卫共享的 `Cancellation`。
- `GetOneTask`、`GetSubtasksFromHistory`、`GetSubtasksFromHistoryByTaskID`、`GetSubtasksByTaskID`、`GetTasksFromHistory`、`GetSubtaskNodes`、`UpdateSubtaskExecID`、`TransferSubTasks2History`、`DeleteSubtasksByTaskID`、`IsTaskCancelling`：参数转换后直接委托同名语义的 trait 方法，保留底层错误。
- `GetTaskEndTime`、`GetSubtaskEndTime`：把底层查询错误转换成 `Ok(None)`，即诊断失败与没有结束时间对调用者表现相同。
- `GetTasksFromHistoryInStates`：空状态切片立即返回空向量，不调用任务表；非空时委托 `history_tasks_in_states`。
- `PrintSubtaskInfo`：调用诊断方法但无条件返回 `Ok(())`，明确忽略诊断失败。
- `NodeResourceGuard` 与 `MockNodeResource`：保存旧的 `NodeResource`，作用域结束时尽力恢复；新资源为 `cpu_count = cpu`、内存 `cpu * 2 GiB`、磁盘固定 `100 GiB`。

本文件没有模块级常量、枚举、条件编译项或具体 trait 实现；全部公开函数保留 Go 风格大写命名，并逐项用 `#[allow(non_snake_case)]` 允许。

## 执行流程

初始化路径如下：

1. 调用者把 `Arc<dyn TableTestRuntime>` 交给 `InitTableTest` 或 `InitTableTestWithCancel`。
2. 两个公开入口分别选择 `Some(8)` 或 `None`，再调用私有 `init_table_test`。
3. `init_table_test` 调用 `runtime.initialize`；失败时用 `?` 原样返回 `DxfError`，成功时把 store ID、任务表和初始为 `false` 的原子取消标志装入 `TableTestGuard`。
4. `InitTableTestWithCancel` 从守卫克隆取消句柄。句柄与守卫共享同一个 `Arc<AtomicBool>`。
5. 守卫离开作用域时，`Drop` 先把取消标志设为 `true`，再调用 `shutdown(store_id)`；关闭错误被忽略，因为析构函数无法返回错误。

表辅助函数的通用路径是“接收 `&dyn TaskTable` → 调用单个 trait 方法 → 返回结果”。三个例外是：结束时间辅助函数将错误降级为 `None`；状态列表为空时跳过存储；诊断打印忽略底层错误。

资源替换路径为：`MockNodeResource` 计算资源值 → `set_node_resource` 安装并取得旧值 → `NodeResourceGuard` 保存旧值 → guard 析构时 `take()` 旧值并调用 `set_node_resource` 恢复。`take()` 保证同一个 guard 最多尝试恢复一次。

## 数据与状态

- 任务表状态不存放在本文件中，而由 `TaskTable` 实现拥有。接口使用 `i64` 任务/子任务 ID、`TaskState` 状态切片、`Task`/`Subtask` 值和节点 ID 字符串表达查询结果。
- `TableTestGuard.store_id` 是运行时关闭资源的身份标识；`task_manager` 是共享 trait object，供测试继续执行表操作。两者都公开，调用者可以直接取用。
- 取消状态只有一个布尔值，初始为 `false`。所有 `Cancellation` 克隆和 guard 都引用同一个 `Arc<AtomicBool>`；调用 `cancel()` 或析构 guard 后，后续 `is_cancelled()` 应观察到 `true`。它不携带取消原因、等待机制或回调。
- `NodeResource` 在 [`context.rs`](./context.rs) 中由 `cpu_count: usize`、`memory_bytes: u64`、`disk_bytes: u64` 组成。`MockNodeResource` 的乘法在 `u64` 上进行，但先把输入 `usize` 转成 `u64`；函数没有范围检查。
- `NodeResourceGuard.previous` 使用 `Option` 是为了在析构时转移旧值；正常构造成功后它总是 `Some`，但 `take()` 让恢复操作具备单次消费不变量。

## 依赖与调用关系

直接 Rust 依赖很小：标准库提供 `Arc`、`AtomicBool`、`Ordering` 和 `SystemTime`；crate 内 [`context.rs`](./context.rs) 提供错误、资源、任务与状态类型，[`task_util.rs`](./task_util.rs) 提供 `NewSubtask`。虽然 crate 的 [`Cargo.toml`](./Cargo.toml) 声明了 storage、scheduler、taskexecutor、testkit、mockstore 等多个工作区依赖，本文件没有直接导入这些具体 crate；真实接线被留给 `TableTestRuntime`/`TaskTable` 实现。

RustCodeGraph 将本文件列为被 13 个文件使用，并能定位 `TaskTable`、`TableTestRuntime`、两个 guard 和全部公开辅助函数；但对精确符号执行 `callers`/`callees` 查询未在 30 秒内返回结果。源码搜索给出的直接 Rust 关系是：[`task_util.rs`](./task_util.rs) 调用本文件的 `TaskTable::insert_subtask` 边界；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 和 [`table_util_test.rs`](./table_util_test.rs) 提供内存/错误型 `TaskTable` 假实现。仓库内未发现 `TableTestRuntime` 实现，也未发现这些初始化或资源 guard 的直接 Rust 调用，因此其运行时接线状态应标记为尚未验证/尚未落地，而不是推断为生产可用。

Go 对照文件 [`table_util.go`](./table_util.go) 则直接依赖 `storage.TaskManager`、session pool、mockstore、testkit 和 failpoint，通过 SQL 操作 `mysql.tidb_global_task`、`mysql.tidb_background_subtask` 及其历史表；这是 Rust trait 方法应由未来适配器实现的行为依据，不是本 Rust 文件已经执行的调用边。

## 错误处理与边界

- 初始化和节点资源替换是强失败路径：`initialize` 或首次 `set_node_resource` 返回错误时，不构造 guard，错误直接传播。
- 常规查询/写操作也直接传播 `DxfError`，包括空结果以外的存储错误。`GetOneTask` 用 `Option<Task>` 表达“没有 Pending 任务”，`GetTaskEndTime`/`GetSubtaskEndTime` 用 `Option<SystemTime>` 表达无结束时间。
- `GetTaskEndTime` 与 `GetSubtaskEndTime` 有意吞掉所有查询错误；[`table_util_test.rs`](./table_util_test.rs) 的 `end_time_helpers_ignore_query_errors_like_go` 用返回错误的 `ErrorTable` 固定了该行为。调用者不能借这两个函数区分“记录不存在/时间为空”和“查询失败”。
- `PrintSubtaskInfo` 也是 best-effort；同一测试文件的 `print_subtask_info_ignores_diagnostic_query_errors_like_go` 验证底层失败仍返回成功。
- `GetTasksFromHistoryInStates([])` 返回空结果且不访问存储，保持 Go 版本空可变参数的短路语义。
- 两个 `Drop` 实现均忽略清理错误，避免析构期间传播或二次 panic，但也意味着关闭/恢复失败只能由运行时自身记录；本接口没有提供读取清理错误的通道。
- `MockNodeResource` 没有检查 `cpu == 0`，也没有显式处理极大 CPU 值的乘法溢出；测试调用者应使用合理的小整数。并发嵌套多个资源 guard 时，全局资源最终值依赖析构顺序，正确用法是严格按栈顺序离开作用域。

## 并发与资源生命周期

两个 trait 都要求 `Send + Sync`，trait object 又由 `Arc` 持有，因此任务表和运行时可以在线程间共享。`Cancellation` 的 `Release`/`Acquire` 配对让设置取消之前的写入可在观察到取消的线程中建立同步关系；它只提供轮询标志，不会阻塞、唤醒任务或强制终止工作。

`TableTestGuard` 的所有权定义环境生命周期：只要 guard 存活，store 标识和任务表仍可使用；guard 析构时先发布取消，再尽力 shutdown。克隆出的 `Cancellation` 可在 guard 析构后继续存在并读取 `true`，因为其 `Arc` 独立维持原子值生命周期。`task_manager` 也可能被调用者另行克隆，所以 guard 的析构只请求运行时关闭，不保证所有 trait object 引用立即释放。

`NodeResourceGuard` 采用相同的 RAII 思路恢复进程级资源。它不加锁，原子取消也不保护资源替换；若多个线程并行调用 `MockNodeResource`，`TableTestRuntime::set_node_resource` 的具体实现必须自行提供同步，并且调用方需要避免交错析构导致恢复到非预期快照。

## 与 Go 版本的对应关系

[`table_util.go`](./table_util.go) 是逐项命名和语义对照：

- Go `InitTableTest` 直接创建 embedded unistore、容量为 10 的 session pool、设置内部 source type、启用禁用 dist-task 的 failpoint，并 mock 8 CPU；Rust 只把 `Some(8)` 交给抽象 `initialize`，这些具体动作是否发生完全取决于尚未提供的 `TableTestRuntime` 实现。
- Go 用 `testing.T.Cleanup` 关闭 pool、恢复节点资源，并返回 `context.Context`/`CancelFunc`；Rust 改用 `TableTestGuard`、`NodeResourceGuard` 的 `Drop` 和原子 `Cancellation`。Rust cancellation 不是完整的 Go context：没有 deadline、value 或级联取消。
- Go 表辅助函数在本文件内组装 SQL并把行转换为 proto；Rust 把这些细节下沉到 `TaskTable`，所以同名函数只是稳定的测试 API 门面。
- Go 的结束时间查询在 SQL 错误时返回零时间且 nil error；Rust 对应为 `Ok(None)`。Rust 同时避免在门面中索引结果行，空行如何处理由 trait 实现决定。
- Go `GetTasksFromHistoryInStates` 的空参数返回 nil slice；Rust 返回空 `Vec`。二者都不查库，调用层面的“无结果”语义一致，但容器表示不同。
- Go `PrintSubtaskInfo` 自己查询当前表和历史表并记录每条子任务；Rust 只调用一次 `print_subtask_info`，具体查询与日志组合必须由实现者保留。
- Go `MockNodeResource` 与 Rust 使用相同的 CPU、每 CPU 2 GiB 内存、固定 100 GiB 磁盘公式；Rust guard 保存 `set_node_resource` 返回的旧值来恢复。

因此该文件是“测试 API/生命周期语义移植”，不是 Go SQL 实现的完整复刻。扩展时必须同时检查 Go 文件，避免 trait 适配器遗漏 SQL、上下文标记、failpoint 或全局 manager 安装等副作用。

## 扩展指南

- 增加新的任务表辅助操作时，先在 `TaskTable` 添加最小方法，再添加与 Go 辅助函数一致的门面；不要把 SQL、session pool 或具体 storage 类型塞回本文件。同步更新独立测试 [`table_util_test.rs`](./table_util_test.rs)，并让所有 `TaskTable` 假实现补齐新方法；其中 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 已有一个完整假实现。
- 实现 `TableTestRuntime` 时，应逐项对齐 Go `getResourcePool`/`getTaskManager`：禁用 dist-task 自启动、创建 embedded unistore 与有界 session pool、安装全局 task manager、注册可靠清理，并明确内部 source type 如何在 Rust 上表达。实现和测试应放在独立源/测试文件，不能把测试内嵌进本生产文件。
- 改动结束时间或诊断接口时，要保留已由独立测试固定的 best-effort 行为；若确需暴露错误，应新增严格版本而不是静默改变现有门面。
- 为 guard 增加清理可观测性时，不要让 `Drop` panic。可考虑在运行时记录错误，或提供显式 `close`/`restore` 方法，使调用者能在析构前处理失败。
- 资源公式或全局资源语义变更需与 Go `MockNodeResource` 同步，并增加覆盖零 CPU、嵌套 guard、并发替换及恢复失败的独立测试。并发实现必须说明全局锁或序列化约束。
- 性能风险主要在未来 `TaskTable` 实现：`subtask_nodes` 需要合并当前/历史表并去重，历史计数与状态过滤应避免无界扫描；当前门面本身只做一次动态分派，除 `Arc` 克隆外没有显著分配。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/framework/testutil` 确认目标 crate 的 17 个 Go/Rust 文件均已索引。
- RustCodeGraph `node --file pkg/dxf/framework/testutil/table_util.rs`：读取目标文件全部 290 行，确认符号、分支、原子顺序和两个 `Drop` 实现；结果标记该文件被 13 个文件使用。
- RustCodeGraph `query`：精确定位 `TaskTable`、`TableTestRuntime`、`TableTestGuard`、`Cancellation`、各公开辅助函数和 `NodeResourceGuard`；`node` 进一步核对 [`context.rs`](./context.rs) 的 `DxfError`/`NodeResource` 与 [`task_util.rs`](./task_util.rs) 的 `NewSubtask` 字段。精确 `callers`/`callees` 查询尝试运行 30 秒未返回，故没有据此虚构调用边。
- crate 与模块边界：读取 [`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs) 和 [`BUILD.bazel`](./BUILD.bazel)；读取上层 [`../doc.go`](../doc.go) 核对 DXF 测试工具所处的任务调度/执行架构背景。
- Go 对照：读取 [`table_util.go`](./table_util.go) 全部 265 行，核对初始化、SQL 表、空状态短路、容错及节点资源公式。
- Rust 测试：读取 [`table_util_test.rs`](./table_util_test.rs) 全部 63 行和 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 中的 `TaskTable` 假实现；前者直接验证结束时间与诊断打印的错误吞掉语义，后者证明该 trait 也供 `task_util` 插入辅助测试使用。
- 仓库搜索：`rg` 核对 Rust 符号使用、`TableTestRuntime` 实现和 Cargo 依赖方；未找到 `impl TableTestRuntime`，因此文档明确记录当前未接线边界。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定的 11 个固定二级标题结构命令及文档链接/内容人工复核作为交付验证。
