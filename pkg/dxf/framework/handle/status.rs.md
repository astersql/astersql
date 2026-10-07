# `pkg/dxf/framework/handle/status.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-handle` crate，是 DXF（Distributed eXecution Framework）面向调用方的只读状态聚合层。crate 入口 [`lib.rs`](lib.rs) 将 `status` 保持为私有模块，但通过 `pub use status::*` 重导出本文件的公开函数；[`Cargo.toml`](Cargo.toml) 则表明状态模型、任务模型和历史页分别来自 `astersql-dxf-framework-schstatus`、`astersql-dxf-framework-proto`、`astersql-dxf-framework-storage`。

它不直接访问 SQL、元数据表、owner 服务或节点发现组件，而是从 [`handle.rs`](handle.rs) 中进程级安装的 `Runtime` 取得这些能力。因此它处在“HTTP/运维或调度调用方”与“任务存储、节点信息、owner、调度配置”之间，负责把底层查询结果整理成稳定的状态结构。

当前 Rust 接线需要谨慎区分：RustCodeGraph 和 Rust 源码搜索只发现本文件内部调用及独立测试对这些公开函数的直接调用，未发现 Rust 生产代码直接调用 `GetScheduleStatus`、`GetActiveTaskSummary`、`ListManagedNodes` 或本文件的 `ListHistoryTasks`。Rust HTTP 实现 [`../../../server/handler/tikvhandler/dxf.rs`](../../../server/handler/tikvhandler/dxf.rs) 通过自己的 `DxfRuntime` trait 获取相同类别的数据；[`../../../server/runtime.rs`](../../../server/runtime.rs) 的历史任务路径目前直接调用任务管理器。相比之下，Go HTTP 入口 [`../../../server/handler/tikvhandler/dxf.go`](../../../server/handler/tikvhandler/dxf.go) 会直接调用同路径 Go `handle` API。这意味着本文件已经提供可复用的 Rust API 和测试行为，但不能仅凭 Go 接线断言 Rust HTTP 主链已经直接接入它。

## 核心职责

本文件承担四组职责，证据均来自 [`status.rs`](status.rs) 中相应符号：

1. `GetScheduleStatus` 聚合运行中/修改中的任务、受管节点数量与 CPU、忙碌节点、所需节点数和调度标志，生成 `schstatus::Status`。
2. `GetActiveTaskSummary`、`ListManagedNodes`、`ListHistoryTasks` 是薄查询门面，把调用参数交给已安装的 `Runtime`，保持统一的 `Context` 和 `Result` 边界。
3. `GetNodesInfo`、`GetBusyNodes` 规范化节点视图：无节点时回退本机 CPU；忙碌节点集合必须包含当前 owner，并正确设置 `IsOwner`。
4. `CalculateRequiredNodes` 与 `getNeededNodes` 估算调度所需节点数；`GetScheduleFlags` 与 `getPauseScaleInFlag` 只暴露当前启用且未过期的暂停缩容标志。

本文件只读、无缓存，也不负责写入调度标志。写入入口 `UpdatePauseScaleInFlag` 位于相邻 [`handle.rs`](handle.rs)，状态文件只负责读取、过期判断和响应组装。

## 主要符号

- `GetScheduleStatus(ctx: &Context) -> Result<schstatus::Status>`：公开的完整调度状态入口。只选取 `TaskStateRunning` 与 `TaskStateModifying` 两类任务；`TaskQueue.ScheduledCount` 使用饱和式转换（`usize` 无法转成 `i32` 时取 `i32::MAX`）；TiDB 与 TiKV 的 `RequiredCount` 相同，但只有 TiDB 组填充 CPU、当前节点数和忙碌节点。
- `GetActiveTaskSummary(ctx) -> Result<storage::ActiveTaskSummary>`：公开门面，调用 `Runtime::get_active_task_summary`。
- `ListManagedNodes(ctx) -> Result<Vec<proto::ManagedNode>>`：公开门面，调用 `Runtime::get_all_nodes`，不在本层过滤、排序或改写节点。
- `ListHistoryTasks(ctx, page_size, page_token, keyspace) -> Result<storage::HistoryTaskPage>`：公开门面，原样传递页大小、keyset token 和可选 keyspace；参数合法性由上层或运行时负责。
- `GetNodesInfo(ctx) -> Result<(i32, i32)>`：返回“受管节点数、单节点 CPU”。CPU 取返回列表第一项；空列表时调用 `Runtime::local_cpu_count`。节点数转换溢出时取 `i32::MAX`。
- `GetBusyNodes(ctx) -> Result<Vec<schstatus::Node>>`：取得 owner 执行 ID 和已有忙碌节点；若 owner 已存在则把该项的 `IsOwner` 设为 `true`，否则追加 owner 节点。
- `CalculateRequiredNodes(tasks, cpu_count) -> i32`：公开纯函数，按任务顺序将每个任务的若干子任务装入已有节点剩余槽位，不足时创建逻辑节点；结果至少为 `1`。
- `getNeededNodes(task) -> i32`：crate 内辅助函数。`ImportInto` 的 `CollectConflicts`、`ConflictResolution`、`PostProcess` 三个步骤固定返回 `1`，其他情况返回 `MaxNodeCount`。
- `GetScheduleFlags(ctx) -> Result<HashMap<schstatus::Flag, schstatus::TTLFlag>>`：公开查询，仅在暂停缩容标志仍启用时把它放入 map。
- `getPauseScaleInFlag(ctx) -> Result<schstatus::TTLFlag>`：私有辅助函数。底层没有值时使用默认值；已启用但 `ExpireTime < SystemTime::now()` 时也整体回退默认值。

文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项；所有持久状态和副作用都经 `Runtime` 抽象提供。

## 执行流程

`GetScheduleStatus` 的执行顺序是固定的：

1. 通过 `runtime()` 取得全局安装的 `Arc<dyn Runtime>`。
2. 调用 `get_task_bases_in_states`，只读取 Running 和 Modifying 任务。返回顺序随后直接影响装箱估算；Go 注释明确要求任务按 `TaskBase` rank 排序，本 Rust 函数自身不重排。
3. `GetNodesInfo` 再次取得运行时，读取全部节点，以第一节点 CPU 作为统一的单节点容量；没有节点时用本机 CPU。
4. `GetBusyNodes` 先查 owner 执行 ID，再读运行子任务涉及的忙碌节点，并保证 owner 恰有一个带 `IsOwner = true` 的表示。代码只处理第一次匹配项，不负责对底层返回的重复 ID 去重。
5. `GetScheduleFlags` 读取暂停缩容 TTL 标志；未设置、禁用或已过期都表现为空 map。
6. `CalculateRequiredNodes` 按输入任务顺序装箱：对每个任务先由 `getNeededNodes` 决定所需节点份数，再逐个扫描已有节点。某节点剩余槽位至少为 `RequiredSlots` 时，在该节点放一个该任务的子任务并扣减槽位；剩余份数各自新建逻辑节点，新节点余额为 `cpu_count - RequiredSlots`。
7. 将上述结果写入 `schstatus::Status`，版本固定为 `Version1`；无任务时所需节点数仍为 `1`，用于保留 owner/小任务响应节点。

三个薄查询函数没有额外分支：获取运行时后直接委派。`ListHistoryTasks` 不在此处实现分页算法，真实页语义由 `Runtime::list_history_tasks` 及 storage 类型决定。

## 数据与状态

- 输入任务使用 `proto::TaskBase` 的 `State`、`Type`、`Step`、`RequiredSlots`、`MaxNodeCount`。状态聚合只取 Running/Modifying；估算函数假设任务已按 rank 排序。
- 节点数据使用 `proto::ManagedNode`。`GetNodesInfo` 只读列表长度和第一项 `CPUCount`，所以它隐含所有受管节点可按同一 CPU 容量估算的模型；本文件不验证节点 CPU 是否异构。
- 输出 `schstatus::Status` 包含版本、任务队列、TiDB/TiKV 两个 `NodeGroup` 及 flags。TiKV 当前仅复用 `RequiredCount`，其他字段保持 `Default`。
- `CalculateRequiredNodes` 的 `available_resources: Vec<i32>` 是一次调用内的临时装箱状态，每个元素表示一个逻辑节点的剩余槽位；它不会写回任务或全局运行时。
- flags 使用 `HashMap<schstatus::Flag, schstatus::TTLFlag>`，当前最多由本文件加入 `PauseScaleInFlag` 一项。过期判断读取墙钟 `SystemTime::now()`，不删除底层持久化值，只在本次结果中将其视为默认关闭。
- 全局 `Runtime` 槽位实际定义在 [`handle.rs`](handle.rs) 的 `OnceLock<RwLock<Option<Arc<dyn Runtime>>>>`；本文件只通过 `runtime()` 读取 `Arc`，不安装或清理它。

## 依赖与调用关系

直接依赖如下：

- `crate::handle::{Context, Result, runtime}`：提供请求上下文、统一错误类型和进程级运行时查找。
- `crate::proto`：提供任务状态、任务类型、步骤、`TaskBase` 与 `ManagedNode`。
- `crate::schstatus`：提供 `Status`、`NodeGroup`、`Node`、flag/TTL 等对外状态模型。
- `crate::storage`：提供活跃任务摘要与历史页返回类型。
- 标准库 `HashMap` 和 `SystemTime`：分别承载 flags 与 TTL 过期判断。

关键内部调用边是 `GetScheduleStatus -> {GetNodesInfo, GetBusyNodes, GetScheduleFlags, CalculateRequiredNodes}`、`CalculateRequiredNodes -> getNeededNodes`、`GetScheduleFlags -> getPauseScaleInFlag`。这些边由 RustCodeGraph 对 `status.rs` 的符号探索确认，也能从本文件调用点直接复核。

下游运行时边界定义在 [`handle.rs`](handle.rs) 的 `Runtime` trait：本文件用到 `get_task_bases_in_states`、`get_all_nodes`、`get_busy_nodes`、`owner_exec_id`、`get_active_task_summary`、`list_history_tasks`、`local_cpu_count` 和 `get_pause_scale_in_flag`。crate 清单对 `proto`、`schstatus`、`storage` 均为普通必需依赖；更重的 domain、kv、session、sqlexec 等集成依赖为 optional，印证本文件本身不直接绑定那些实现。

RustCodeGraph 当前对 Rust 版本只解析出自调用/测试调用；Rust 生产 HTTP 层持有独立 `DxfRuntime` trait。Go 版本的生产调用者则明确包括 `DXFScheduleStatusHandler.ServeHTTP`、`DXFActiveTaskHandler.ServeHTTP`、`DXFNodesHandler` 和历史任务 handler，见 [`../../../server/handler/tikvhandler/dxf.go`](../../../server/handler/tikvhandler/dxf.go)。

## 错误处理与边界

- 所有依赖运行时的函数都返回 crate 的 `Result<T>`，并用 `?` 原样传播运行时获取和底层查询错误；本文件不记录日志、不转换错误类别，也不做部分结果降级。
- 未安装 `Runtime` 时，`runtime()` 返回 `DXF handle runtime is not installed`；运行时 `RwLock` 中毒则由相邻实现的 `expect` 触发 panic，而不是成为 `Result` 错误。
- `GetScheduleStatus` 是串行聚合。任何一步失败都会中止，已经读取的数据不会形成部分 `Status`。
- `ListHistoryTasks` 不验证 `page_size`、`page_token` 或 `keyspace`；调用者必须在进入本层前校验，或由运行时返回错误。Rust HTTP `DxfRuntime` 明确另有 `validate_history_page_size`/`validate_keyspace_name` 边界。
- `CalculateRequiredNodes` 依赖提交路径保证 `RequiredSlots <= cpu_count`，此约束在 Go 源码注释中明确，但 Rust 函数本身未检查。违反时会产生负的剩余槽位；负的 `MaxNodeCount` 会使新建节点循环为空。扩展或直接调用时不能把这些非法输入的当前偶然结果当成受支持语义。
- 任务或节点数量转换成 `i32` 时用 `i32::MAX` 封顶，不会因超大集合而 panic。
- TTL 仅在 `Enabled` 为真时检查过期；禁用值不对外暴露。比较条件为严格小于当前时间，且每次调用重新取墙钟，因此临界时刻存在正常的时间采样差异。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或长生命周期句柄。每个查询从全局槽克隆一个 `Arc<dyn Runtime>`，保证本次调用期间运行时对象存活；各具体底层操作是否开启会话/事务以及如何同步，由 `Runtime` 实现负责。

状态聚合不是跨查询的一致性快照：任务、节点、忙碌节点和 flag 是依次读取的，调用期间集群状态可能变化。因此 `schstatus::Status` 是运维估算视图，而不是事务一致的调度快照。`SystemTime::now()` 也使 TTL 结果与读取时刻绑定。

本文件对输入切片只读，装箱状态全部局部持有，因此 `CalculateRequiredNodes` 本身可并发调用。测试夹具 [`status_testkit_test.rs`](status_testkit_test.rs) 因会替换进程级 Runtime，使用临时目录锁串行化相关测试，并通过 `RuntimeGuard::drop` 清理全局值；这反映的是全局注入点的生命周期约束，而非本文件内部的锁。

## 与 Go 版本的对应关系

同路径 [`status.go`](status.go) 是直接语义基线。Rust 保留了 Go 的函数分组、Running/Modifying 任务筛选、节点 CPU 回退、owner 合并、顺序装箱、ImportInto 三个单节点步骤、至少保留一个节点以及过期 flag 隐藏等核心行为。Rust 独立测试 [`status_test.rs`](status_test.rs) 的装箱用例和 `getNeededNodes` 用例与 [`status_test.go`](status_test.go) 对齐；Rust [`status_testkit_test.rs`](status_testkit_test.rs) 也覆盖了 Go [`status_testkit_test.go`](status_testkit_test.go) 中的空队列状态、CPU 回退、owner/忙碌节点合并和 flag TTL 行为。

实现边界存在以下差异：

- Go 函数直接获取 `storage.TaskManager`、开启 session、查询 infosync/SQL，并给 context 标记 `InternalDistTask`；Rust 把这些行为全部下沉到可注入 `Runtime`，本文件自身不修改 `Context`。
- Go 返回指针及 `errors.Trace` 包装后的错误；Rust 返回拥有所有权的值，并传播统一 `Error`。
- Go `GetNodesInfo`/`GetBusyNodes`/`GetScheduleFlags` 显式接收 manager；Rust 版本在函数内部从全局槽取得 runtime。
- Go 的 HTTP handler 已直接调用这些函数；当前 Rust HTTP 层通过另一个 `DxfRuntime` trait 抽象，尚不能从现有调用图证明生产请求会到达本文件。
- Go 无节点 CPU 回退还会写 warning；Rust 本文件只静默调用 `local_cpu_count`，是否记录日志取决于运行时实现。

这些差异主要是依赖反转和接线状态差异，不应被解释为可以删减 Go 行为。若把 Rust API 接入生产 HTTP 层，需要逐项保留 Go 的上下文标记、参数验证、错误到 HTTP 的映射和可观测性。

## 扩展指南

- 新增 `schstatus::Status` 字段或新的调度数据源时，优先扩展 `GetScheduleStatus`；若需要底层能力，同时在 [`handle.rs`](handle.rs) 的 `Runtime` trait、生产实现和独立测试 runtime 中增加方法。注意串行读取会扩大非快照窗口，必要时应设计一次性 runtime 聚合接口。
- 新增调度 flag 时，在 `GetScheduleFlags` 中显式决定启用/过期规则，并在 `schstatus` 中定义稳定键；不要把过期持久值误报为有效。相应边界测试应放在独立 [`status_testkit_test.rs`](status_testkit_test.rs)，不要内嵌到生产源文件。
- 修改节点估算时，以 `CalculateRequiredNodes`/`getNeededNodes` 为接入点，并同步 [`status_test.rs`](status_test.rs) 与 Go [`status_test.go`](status_test.go) 的表驱动用例。必须保持“同一任务每节点至多一个子任务”“输入顺序代表 rank”“至少一个预留节点”三个 Go 行为，除非 Go 基线也发生对应变更。
- 若支持异构 CPU 节点，不能只改 `CalculateRequiredNodes`：`GetNodesInfo` 当前只返回第一节点 CPU，状态模型和 runtime 接口也需要共同调整，并评估调度估算的兼容性与性能。
- 将这些 API 接入 Rust HTTP 生产路径时，应在 [`../../../server/handler/tikvhandler/dxf.rs`](../../../server/handler/tikvhandler/dxf.rs) 的 `DxfRuntime` 实现处做局部接线，并补独立 handler 测试；不能仅依据 crate 已导出函数就宣称端到端接入完成。
- 性能上，装箱复杂度约为“各任务所需节点份数乘当前逻辑节点数”，且可能为每个新节点分配一个 `Vec` 元素。扩大任务规模或改变 `MaxNodeCount` 上限前，应评估最坏复杂度；优化时必须保持输入顺序和现有测试结果。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/dxf/framework/handle` 确认同目录 Rust/Go 源码及测试均在图中。
- RustCodeGraph `node --file pkg/dxf/framework/handle/status.rs`：读取完整 158 行源码，确认 9 个函数、公开性、调用次序及无条件编译项。
- RustCodeGraph `query GetScheduleStatus --kind function --json` 与 `query getNeededNodes --kind function --json`：消除 Go/Rust 同名歧义，分别定位 `status.rs::GetScheduleStatus` 和 `status.rs::getNeededNodes`。
- RustCodeGraph `explore`：确认本文件内部调用边，并显示 Rust 版本主要为自身/测试调用；因图的 callers/callees 子命令对精确 ID 未输出边，另用限定 `*.rs` 的源码搜索核对生产调用现状。
- 已读 Rust 路径：[`status.rs`](status.rs)、[`lib.rs`](lib.rs)、[`handle.rs`](handle.rs) 的 `Runtime` 与全局槽、[`status_test.rs`](status_test.rs)、[`status_testkit_test.rs`](status_testkit_test.rs)、[`../../../server/handler/tikvhandler/dxf.rs`](../../../server/handler/tikvhandler/dxf.rs)、[`../../../server/runtime.rs`](../../../server/runtime.rs)。
- 已读配置与 Go 对照：[`Cargo.toml`](Cargo.toml)、[`status.go`](status.go)、[`status_test.go`](status_test.go)、[`status_testkit_test.go`](status_testkit_test.go)、[`../../../server/handler/tikvhandler/dxf.go`](../../../server/handler/tikvhandler/dxf.go)。
- 人工复核结论：本文能够回答文件为何存在（DXF 状态聚合/查询门面）、如何运行（Runtime 查询与顺序装箱）、当前如何接线（crate 重导出但 Rust HTTP 尚经独立 runtime 边界）以及如何安全扩展（同步 Runtime、独立测试和 Go 语义）。本任务为纯文档分析，按计划未运行 Cargo。
