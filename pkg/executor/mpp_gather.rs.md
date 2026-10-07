# `pkg/executor/mpp_gather.rs`

## 文件定位

本文件位于 `astersql-executor` crate，模块由 `pkg/executor/lib.rs` 的 `pub mod mpp_gather` 对外导出；crate 根与特性声明见 `pkg/executor/Cargo.toml`。它把 MPP（Massively Parallel Processing）根任务的调度、响应拉取和虚拟列补值描述为一组泛型协议，并提供与 Go `pkg/executor/mpp_gather.go` 同形的 `MPPGather<R>` 生命周期实现。

在 SQL 执行链上，Go 版本由 `pkg/executor/builder.go::buildTableReader` 在 `useMPPExecution` 成立时构造；Rust 构建器 `pkg/executor/builder.rs::buildTableReader` 则依据计划的 `use_mpp()` 进入 `buildMPPGather`，再委托 `ExecutorBuilderDependencies::build_mpp_gather_executor`。当前 RustCodeGraph 没有找到生产代码对本文件 `MPPGather<R>`、`useMPPExecution`、`getMPPQueryID` 或 `getMPPQueryTS` 的静态调用边，因此本文件应视为可测试的泛型移植实现与集成边界，而不能据此断言 Rust 生产构建器已经直接实例化它。

## 核心职责

1. `useMPPExecution` 同时检查会话是否允许 MPP，以及表侧物理计划根是否为 ExchangeSender；两项都成立才选择 MPP。
2. `getMPPQueryID`、`getMPPQueryTS` 为语句级 MPP 查询标识做并发安全的惰性初始化，使同一上下文中的后续读取复用首个成功写入的值。
3. `collectPlanIDs` 深度优先收集整棵物理计划树的节点 ID，供响应解码、运行时统计和重试层识别计划节点。
4. `MPPGather::Open` 在普通模式创建带重试的 MPP 执行器并建立响应迭代器；在 `dummy` 模式只生成根任务键范围。
5. `MPPGather::Next` 每次重置输出批次、拉取下一批结果，并仅在存在行时补齐虚拟列。
6. `MPPGather::Close` 关闭响应迭代器并保留 Go 的异常状态语义；`Table` 和 `setDummy` 分别支持数据源识别与只生成范围的模式切换。

这些职责对应 Go 文件中的同名函数和方法；真正的 DistSQL、重试、schema、表和内存跟踪操作均由 `MPPGatherRuntime` 提供，本文件没有默认“成功”桩。

## 主要符号

- `MPPTableReader`：最小表读取计划协议，仅用 `table_plan_is_exchange_sender()` 暴露根节点类型判断。
- `MPPSessionContext`：最小会话协议，提供 MPP 开关和语句级 `MPPQueryInfo`。
- `MPPQueryInfo { QueryID, QueryTS }`：两个 `AtomicU64`；分别缓存查询序号与纳秒时间戳。字段采用 Go 风格命名以保持移植对应。
- `useMPPExecution<C, T>(context, reader) -> bool`：无副作用的 MPP 选择谓词。
- `getMPPQueryID(context, allocate_mpp_query_id) -> u64`：调用传入分配器并以 compare-exchange 尝试写入零值槽，最后返回槽内最终值。
- `getMPPQueryTS(context) -> u64`：读取系统时钟相对 Unix epoch 的纳秒数，以同样方式只写入一次。
- `PhysicalPlan`：可克隆计划抽象，暴露 `id`、`plan_type`、子节点和向 ExchangeSender 的受检转换。
- `collectPlanIDs(plan, ids)`：前序深度优先遍历；先加入当前节点 ID，再按 `children()` 返回顺序递归。
- `MPPChunk`：输出批次抽象，仅要求重置和行数查询。
- `MPPGatherRuntime`：外部副作用的完整依赖注入边界，关联类型覆盖上下文、计划、schema、查询 ID、响应、执行器、内存跟踪器、列/字段类型、表和键范围。
- `MPPGather<R>`：持有计划、快照时间戳、响应迭代器、重试执行器、虚拟列元数据、表和键范围的执行器状态。
- `MPPGather::{Open, Next, Close, Table, setDummy}`：与 Go `MPPGather` 生命周期和辅助接口对应的公开方法（`setDummy` 仅在模块内公开）。

文件没有条件编译项。测试模块的条件编译发生在 `pkg/executor/lib.rs`：`mpp_gather_test` 只在 `cfg(test)` 下纳入。

## 执行流程

普通执行从构建阶段决定是否使用 MPP。Rust 构建入口 `pkg/executor/builder.rs::buildTableReader` 在 `plan.use_mpp()` 为真时调用 `buildMPPGather`；该函数先取得快照时间戳，拒绝“多个 TableScan 且又有虚拟列或 UnionScan”的组合，然后通过依赖边界创建具体执行器。Go 对照链为 `builder.go::buildTableReader -> buildMPPGather -> MPPGather`。

`Open` 的普通分支按以下顺序运行：

1. 以 `collectPlanIDs` 收集 `originalPlan` 全树的 ID。
2. 调用 `new_executor_with_retry`，传入执行上下文、可变内存跟踪器、计划 ID、克隆后的原始计划、`startTS`、克隆后的 `mppQueryID` 和 infoschema。
3. 若创建失败且错误携带了部分执行器，先尽力调用 `close_mpp_executor`，保存该执行器到 `mppExec`，再返回原始创建错误。
4. 创建成功后，从执行器复制 `kvRanges`，将执行器存入 `mppExec`。
5. 用计划 ID、当前执行器 ID 和可变执行器调用 `select_result_from_mpp_response`，把返回值存入 `respIter`。

`dummy` 分支不创建响应迭代器：先用 `as_exchange_sender()` 验证根计划类型，再调用 `generate_root_mpp_tasks` 填充 `kvRanges`，随后返回。该范围供类似 Go `UnionScanExec` 的内存表读取逻辑使用；Go 直接证据位于 `pkg/executor/union_scan.go` 对 `*MPPGather` 的分支。

每次 `Next` 先无条件 `chunk.reset()`。`dummy` 模式立即返回空批次；普通模式要求 `Open` 已经设置 `respIter`，调用 `next_result` 后，零行直接返回，非零行再用三个虚拟列元数据数组和 `columns` 调用 `fill_virtual_column_values`。

`Close` 在普通模式下只关闭已存在的 `respIter`，不存在则成功返回。在 `dummy` 模式下，`respIter` 理应不存在；若异常存在，则尽力关闭它，并返回固定内部错误。关闭不会把 `respIter` 置空，因此重复关闭的行为由运行时实现决定，且当前测试刻意验证它会再次调用关闭。

## 数据与状态

`MPPGather<R>` 的核心只读输入是 `is`、`originalPlan`、`startTS` 和 `mppQueryID`。`startTS` 固定查询快照；`mppQueryID` 标识一次 MPP 查询，两者同时传给根任务生成或带重试执行器创建。

生命周期状态主要由以下字段表达：

- `respIter: Option<R::SelectResult>`：`Open` 普通分支成功后存在，`Next` 必须依赖它；`Close` 不移除它。
- `mppExec: Option<R::MPPExecutor>`：保存普通模式的底层 MPP 执行器；创建失败时也可能保存部分构造对象，以保留 Go 字段状态。
- `kvRanges: Vec<R::KeyRange>`：dummy 模式来自 `generate_root_mpp_tasks`，普通模式来自 `executor_key_ranges`；主要为上层（例如 UnionScan）暴露实际扫描范围。
- `dummy: bool`：一旦由 `setDummy` 置真，本文件没有恢复方法；此模式禁止结果消费。
- `memTracker`：仅在创建带重试执行器时以可变引用交给运行时，资源归属和父子挂接由具体运行时负责。
- `columns`、`virtualColumnIndex`、`virtualColumnRetFieldTypes`：只有非空结果批次才参与虚拟列补值。
- `table`：由 `Table()` 克隆返回；具体表类型必须实现 `Clone`。

`MPPQueryInfo` 独立于 `MPPGather` 实例，表达语句上下文共享状态。两个原子字段初始为零，零同时承担“尚未初始化”的哨兵含义，因此分配器若返回零，后续调用仍可能再次尝试初始化。

## 依赖与调用关系

上游直接证据：

- `pkg/executor/lib.rs` 导出 `mpp_gather`，并在测试配置下加载 `mpp_gather_test`。
- `pkg/executor/builder.rs::buildTableReader` 根据 `plan.use_mpp()` 进入 `buildMPPGather`；`buildMPPGather` 进行快照与计划形状检查后调用抽象的 `build_mpp_gather_executor`。
- Go 生产实现中，`pkg/executor/builder.go::buildTableReader` 调用 `useMPPExecution`，`buildMPPGather` 直接构造 Go `MPPGather`，同时设置查询 ID、时间戳、内存跟踪器、虚拟列和表。

本文件的直接下游全部通过 trait 方法表达：计划树通过 `PhysicalPlan::{id, children, as_exchange_sender, plan_type}`；结果批次通过 `MPPChunk::{reset, num_rows}`；外部系统通过 `MPPGatherRuntime` 的九类方法完成任务生成、执行器创建/关闭、键范围提取、响应包装/拉取/关闭、虚拟列补值和执行器 ID 查询。

Go 的具体下游包括 `physicalop.GenerateRootMPPTasks`、`mpp.NewExecutorWithRetry`、`distsql.GenSelectResultFromMPPResponse`、`SelectResult.Next/Close` 和 `table.FillVirtualColumnValue`（见 `pkg/executor/mpp_gather.go`）。`pkg/executor/internal/mpp/executor_with_retry.go` 进一步说明层次为 Gather → selectResult → ExecutorWithRetry → localMppCoordinator → MPP RPC；Rust trait 保留了这条链所需的效果，但没有绑定具体类型。

RustCodeGraph 对 `collectPlanIDs` 的 callee 查询确认其直接调用 `PhysicalPlan::id` 与 `PhysicalPlan::children`；对本文件主要入口的 caller 查询没有返回 Rust 生产调用边。因而构建器中的抽象 MPP 分支只能证明 MPP 构建入口存在，不能证明它最终使用了本文件的泛型结构体。

## 错误处理与边界

- `useMPPExecution` 是严格的双条件判断：会话禁止 MPP 或根节点不是 ExchangeSender 都返回 `false`。
- dummy `Open` 若 `originalPlan.as_exchange_sender()` 失败，返回包含实际 `plan_type()` 的错误，不生成任务。
- `generate_root_mpp_tasks` 的错误用 `?` 原样传播。
- 普通 `Open` 的创建错误类型可携带部分执行器；实现忽略清理错误并优先返回原创建错误，这与 Go “close best effort” 一致。
- `select_result_from_mpp_response` 本身不返回 `Result`，因此 runtime 契约假设包装响应不会在该调用点失败；后续读取错误由 `next_result` 返回。
- `Next` 在 `Open` 前调用，或普通 `Open` 未建立 `respIter` 后调用，会触发 `expect` panic，而不是返回业务错误。这是调用顺序不变量。
- `next_result` 的错误立即传播，不执行虚拟列补值；补值错误也直接传播。
- dummy `Close` 遇到意外存在的迭代器时忽略关闭错误，返回固定状态错误；普通 `Close` 则保留并返回实际关闭错误。
- `getMPPQueryTS` 在系统时间早于 Unix epoch 时使用补码式 wrapping 负纳秒值转换为 `u64`；`duration.as_nanos() as u64` 也会截断超过 `u64` 的高位。这是当前实现事实，不应解释为有符号 Unix 时间。

## 并发与资源生命周期

`MPPQueryInfo` 使用 `compare_exchange(0, value, SeqCst, SeqCst)` 和 `load(SeqCst)`。多个线程同时初始化时只有一个候选值能写入，所有调用最终加载同一个已提交值；顺序一致性排序提供最强的跨线程可见性。需要注意，`getMPPQueryID` 会在 compare-exchange 之前无条件调用分配器，因此竞争失败的调用也消耗一个分配号，这与 Go `CompareAndSwap(0, AllocMPPQueryID())` 的求值顺序一致。

`MPPGather<R>` 自身没有锁、通道、线程或异步任务，也没有声明 `Send`/`Sync` 约束；是否可跨线程移动或共享取决于所有关联类型和运行时实现。`Open`、`Next`、`Close` 均要求 `&mut self`，在安全 Rust 中排除了同一实例的并发可变调用。

资源顺序为：构建阶段准备计划与跟踪器，`Open` 创建底层执行器并包装响应，反复 `Next` 消费批次，最后 `Close` 关闭响应。普通成功路径中 `Close` 只关闭 `respIter`，不直接调用 `close_mpp_executor`；底层执行器的最终释放应由响应包装器或关联类型的析构/运行时契约承担。唯一显式关闭底层执行器的路径是 `new_executor_with_retry` 失败且返回部分执行器时。

## 与 Go 版本的对应关系

总体控制流逐项对应 `pkg/executor/mpp_gather.go`：MPP 选择谓词、QueryID/QueryTS 的 CAS 初始化、计划 ID 前序遍历、dummy/普通 `Open`、批次 `Next`、`Close`、`Table` 和 `setDummy` 均保留。

Rust 为可独立移植而增加了 `MPPTableReader`、`MPPSessionContext`、`PhysicalPlan`、`MPPChunk` 和 `MPPGatherRuntime` 等 trait，把 Go 中具体的 sessionctx、planner、DistSQL、memory、table 与 MPP retry 类型变成关联类型。`MPPQueryInfo` 也在本文件中定义，而 Go 使用语句上下文已有字段。

已确认的语义细节包括：

- 两种语言都在 CAS 前求值/分配 QueryID，因此失败竞争也会消耗候选 ID。
- 两种语言都按当前节点后子节点的顺序收集 plan ID。
- dummy `Open` 都要求 ExchangeSender 并只获得键范围。
- 创建带重试执行器失败时，两者都尽力关闭部分执行器并返回原错误。
- 两者的 `Close` 都不清空响应字段；`pkg/executor/mpp_gather_test.rs` 的两个测试专门验证普通与 dummy 模式重复关闭时的 Go 兼容行为。

差异与迁移限制：Rust 时间戳使用 `SystemTime` 并显式处理 epoch 之前的时间，Go 使用 `time.Now().UnixNano()`；Rust `collectPlanIDs` 就地修改 `Vec<i32>`，Go 返回追加后的 slice；Rust `Table` 克隆关联表值，Go 返回接口值。更重要的是，Go builder 直接构造具体 `MPPGather`，而 Rust builder 当前只展示抽象依赖调用，RustCodeGraph 未证明 `MPPGather<R>` 已进入生产执行链。

未发现 Go `*_test.go` 对这些同名符号的直接测试；当前直接回归证据是独立 Rust 测试 `pkg/executor/mpp_gather_test.rs`。

## 扩展指南

- 新增 MPP 打开阶段的外部副作用时，优先扩展 `MPPGatherRuntime`，并在 `Open` 中保持调用顺序和失败清理；不要在本文件加入默认成功实现掩盖未接线依赖。
- 改变计划遍历时修改 `PhysicalPlan`/`collectPlanIDs`，并增加独立测试覆盖多层树、兄弟顺序和重复/特殊 ID；不要把测试嵌入 `mpp_gather.rs`。
- 改变结果读取或虚拟列逻辑时修改 `MPPChunk`、`Next` 或 runtime 补值接口，并在 `pkg/executor/mpp_gather_test.rs` 覆盖空批次、读取错误、补值错误和调用顺序。
- 改变 dummy 语义时同时核对 `Open`、`Next`、`Close`、`setDummy`、`kvRanges`，以及 Go `pkg/executor/union_scan.go` 对键范围的使用。
- 改变查询标识分配时同时核对 CAS 的竞争语义、零值哨兵、分配器副作用和 Go `getMPPQueryID/getMPPQueryTS`；并添加多线程独立测试。
- 要把本泛型实现接入 Rust 生产链，需要为真实计划、chunk、DistSQL/MPP retry、schema、表、内存跟踪器实现这些 trait，并让 `ExecutorBuilderDependencies::build_mpp_gather_executor` 返回包装该实现的 `ExecutorBox`。接线后应重新运行 RustCodeGraph callers/callees，确认生产边而非只依赖模块导出。
- 兼容风险集中在 Go 错误优先级、重复关闭、dummy 不变量和虚拟列补值时机；性能风险集中在 `children()` 每层分配 `Vec`、计划克隆、键范围复制和 `SeqCst` 原子排序。任何优化都应先证明不改变上述可观察行为。

## 验证依据

已读取并交叉核对：

- `pkg/executor/mpp_gather.rs`：目标文件全部 290 行，符号、分支和状态的主要依据。
- `pkg/executor/Cargo.toml`：确认所属 crate 为 `astersql-executor`、crate 根为 `lib.rs`、唯一显式 feature 为 `nextgen`，以及 executor 对 DistSQL、MPP、planner、table、memory 等相邻 crate 的依赖边界。
- `pkg/executor/lib.rs`：确认生产模块导出与独立测试模块的 `cfg(test)` 装配。
- `pkg/executor/builder.rs`：确认 Rust MPP 构建选择、计划形状检查和 `build_mpp_gather_executor` 依赖边界。
- `pkg/executor/mpp_gather.go`、`pkg/executor/builder.go`：确认 Go 原实现、直接生产构建接线、虚拟列/表初始化和错误语义。
- `pkg/executor/union_scan.go`：确认 Go UnionScan 使用 `MPPGather.kvRanges`。
- `pkg/executor/internal/mpp/executor_with_retry.go`：确认 MPP 响应、重试协调器与 RPC 的分层关系。
- `pkg/executor/mpp_gather_test.rs`：确认测试位于独立文件，并验证普通与 dummy 模式重复 `Close` 的响应字段保留和调用次数。

RustCodeGraph 证据：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/executor/mpp_gather.rs` 覆盖完整目标文件；`query` 同时定位 Rust/Go 的 `MPPGather`、`useMPPExecution`、`getMPPQueryID`、`collectPlanIDs`；`callees collectPlanIDs` 显示 Rust 直接边到 `PhysicalPlan::id/children`；主要 Rust 入口的 `callers` 查询未返回生产调用边。结构验证另以任务指定命令执行；本任务为纯文档分析，按计划不运行 Cargo。
