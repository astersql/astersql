# `pkg/executor/internal/util/partition_table.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-executor-internal-util`；包边界由 `pkg/executor/internal/util/Cargo.toml` 的 `[lib] path = "lib.rs"` 定义。`pkg/executor/internal/util/lib.rs` 将私有模块 `partition_table` 的公开项全部重导出，根 facade 又在 `pkg/lib.rs` 的 `executor::internal::util` 下重导出该 crate。因此，它提供的是 executor 内部可复用的 Rust API，而不是进程入口或独立执行器。

文件建立了一套只覆盖 ID 改写所需字段的最小 tipb Executor 树模型，并实现 `UpdateExecutorTableID`。仓库搜索未发现该 Rust 函数被 Rust 生产代码调用；当前 Rust 直接调用者只有同 crate 的独立测试 `pkg/executor/internal/util/migration_aster_unit_test.rs` 和函数自身的递归调用。因此应把它描述为“已经实现并有迁移测试、但尚未接入 Rust 生产执行链”的移植边界，不能把 Go 调用链当作 Rust 已接线事实。

## 核心职责

- 用 `ExecType`、私有 `ExecutorData` 和 `Executor` 表示本算法关心的扫描、一元算子、Join 与终端节点，而不是完整的 `tipb::Executor`（`partition_table.rs:25-190`）。
- 沿指定执行器路径就地替换物理表或分区 ID：`TableScan`、`IndexScan` 使用第一个 ID，`PartitionTableScan` 使用完整列表（`partition_table.rs:221-255`）。
- 在 `recursive == true` 时穿过一元算子；遇到 Join 时只沿非 inner 一侧继续；遇到 ExchangeReceiver 或 CteSource 时停止（`partition_table.rs:257-292`）。
- 将未知协议类型与类型/载荷不一致分别报告为结构化错误（`partition_table.rs:207-215, 286-288`）。
- 通过可选的 `UpdateExecutorTableIDContext` 记录被 TableScan 采用的下一分区 ID，复刻 Go 中仅为覆盖/观测使用的 context value 行为（`partition_table.rs:192-205, 238-240`）。

## 主要符号

- `pub enum ExecType`：算法的节点类别判别器。扫描类有 `TableScan`、`PartitionTableScan`、`IndexScan`；十二种节点按一元节点处理；`Join` 单独选择 outer child；`ExchangeReceiver`、`CteSource` 为停止点；`Unknown(i32)` 保留原始未知协议码。
- `enum ExecutorData`：私有载荷枚举，分别保存单个表 ID、分区 ID 向量、装箱的一元子节点、两个 Join 子节点及 `inner_idx`，或无载荷的 `Terminal`。它与 `ExecType` 是两套字段，函数会显式验证二者是否匹配。
- `pub struct Executor`：`tp` 与 `data` 均为私有，外部只能借助构造器创建和借助只读访问器观察。`table_scan`、`partition_table_scan`、`index_scan`、`unary`、`terminal`、`join`、`unknown` 构造相应节点；`child` 和 `join_children` 用于检查树形结构。
- `Executor::unary`、`Executor::terminal`：只用 `debug_assert!` 约束允许的 `ExecType`，优化构建中错误调用仍可构造类型/载荷不匹配的节点，随后由更新函数返回 `MalformedExecutor`。`Executor::join` 则用始终生效的 `assert!` 保证 `inner_idx < 2`（`partition_table.rs:117-170`）。
- `pub struct UpdateExecutorTableIDContext<'a>`：可选持有 `&'a mut HashSet<i64>`；`Default` 表示不记录，`recording` 建立记录模式。字段私有，避免调用者绕过构造约束。
- `pub enum UpdateExecutorTableIDError`：`UnknownProtocol(i32)` 保留未知协议码；`MalformedExecutor` 表示 `ExecType` 与 `ExecutorData` 不一致。错误展示由 `thiserror` 派生。
- `pub fn UpdateExecutorTableID(...) -> Result<(), UpdateExecutorTableIDError>`：唯一行为入口。名称保留 Go 风格，crate 根通过 `#![allow(non_snake_case)]` 接受该命名。

## 执行流程

1. `exec` 为 `None` 时立即成功返回；这对应 Go 的 nil executor 分支。
2. 按 `exec.tp` 匹配当前节点，并同时解构 `exec.data`。若预期载荷不匹配，返回 `MalformedExecutor`，不继续下降。
3. `TableScan` 把 `partition_ids[0]` 写入 `table_id`，并在 recording context 存在时把同一个 ID 插入集合；`IndexScan` 同样取首个 ID但不记录；`PartitionTableScan` 用传入切片生成的完整向量替换旧列表。
4. 一元算子返回其唯一 child 作为候选下一节点。ExchangeReceiver 和 CteSource 返回空候选，形成协议上的递归边界。
5. Join 根据 `inner_idx` 选择 `children[1 - inner_idx]`，即只递归进入 outer side，inner side 保持不变。
6. `Unknown(code)` 直接返回 `UnknownProtocol(code)`。
7. 仅当 `recursive` 为真时才对候选 child 递归调用；扫描节点和终端节点没有候选 child，因此结束。递归调用使用 `?` 原样传播错误（`partition_table.rs:227-294`）。

这里的“路径”是单链而不是全树遍历：一元节点只有一个 child，Join 明确只选一个 child，函数不会同时访问 Join 两侧。

## 数据与状态

算法修改调用者传入的 `&mut Executor`，没有返回新树。扫描 ID 为 `i64`，分区扫描保存 `Vec<i64>`；`PartitionTableScan` 的替换会复制输入切片，更新后不借用调用者的 `partition_ids`。其他节点只改变遍历位置，不改变自身载荷。

`UpdateExecutorTableIDContext` 在一次同步调用链中转移；其可变集合引用随递归继续传递。记录集合使用 `HashSet`，重复 TableScan ID 会去重。当前实现的单链遍历通常至多触达一个扫描节点，但该集合形态与 Go 的 `map[int64]struct{}` 观测契约一致。

关键不变量是 `ExecType` 与 `ExecutorData` 的组合必须匹配，以及 Join 的 `inner_idx` 必须为 0 或 1。公共字段私有且正常构造器维护这些不变量；但 `unary`/`terminal` 的类别检查仅是 debug assertion，未来内部扩展仍必须保留运行时的 `MalformedExecutor` 防线。

## 依赖与调用关系

上游导出链为 `pkg/executor/internal/util/lib.rs` → `pub use partition_table::*`，再由 `pkg/lib.rs` 的 `executor::internal::util` facade 重导出。根 `Cargo.toml` 以 `facade_executor_internal_util` 指向该路径。直接 Rust 测试通过 crate 名 `astersql_executor_internal_util` 导入公开符号。

下游依赖很小：标准库 `HashSet` 提供记录集合，`Box`/`Vec` 表示树与 ID 列表，`thiserror::Error`（见该 crate 的 `Cargo.toml`）生成错误展示和 `Error` 实现。文件没有依赖真实 tipb crate，这也说明当前 `Executor` 是迁移期最小模型而非线上 protobuf 对象。

RustCodeGraph 的文件查询识别本文件 17 个符号，精确 `query UpdateExecutorTableID` 同时命中 Rust 与 Go 定义；但限定符号的 `callers/callees` 没有给出可用调用边。补充 `rg` 证据显示，Rust 中除独立测试与自递归外没有调用者。Go 的生产调用点位于 `pkg/executor/table_reader.go`：分别为每组物理范围写入单个 partition ID，以及为分区表扫描写入完整 PID 列表；另一个调用点位于 `pkg/executor/internal/mpp/local_mpp_coordinator.go`，在序列化 MPP DAG 前根据任务的分区列表或物理 TableID 改写根执行器。

## 错误处理与边界

- `exec == None` 是合法空操作，返回 `Ok(())`。
- `Unknown(code)` 返回可诊断的 `UnknownProtocol(code)`；其显示文本为 `unknown new tipb protocol {code}`，与 Go 错误信息对齐。
- 类型判别与载荷不一致返回 `MalformedExecutor`。正常外部调用无法直接改写私有字段，但错误仍保护 crate 内未来构造或扩展。
- `TableScan` 与 `IndexScan` 直接索引 `partition_ids[0]`，空切片会 panic，而不是返回 `UpdateExecutorTableIDError`。这是对齐 Go 空切片索引 panic 的有意边界，独立测试用 `catch_unwind` 固定该行为。
- `PartitionTableScan` 接受空切片，并把 ID 列表清空。
- `recursive == false` 仍处理当前节点，但不会处理候选 child；所以包裹扫描的一元根节点不会改写其扫描 child。
- ExchangeReceiver 与 CteSource 即使递归开启也停止；Join 的 inner child 永不由此函数改写。
- 函数没有回滚语义：若未来递归路径在较深处报错，先前节点可能已被修改。当前模型中中间一元/Join 节点本身不改 ID，实际部分更新风险主要与未来扩展有关。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。所有变更在一次同步、独占可变借用期间完成；Rust 借用规则防止同一个 `Executor` 或 recording 集合在调用期间被并发可变访问。

树的拥有关系由 `Box<Executor>` 与 `Vec<i64>` 管理，函数返回后没有后台工作或悬挂资源。递归深度等于被选路径的节点深度，因此极深的人造树存在调用栈增长风险；当前没有显式深度上限或迭代式降级。分区列表替换会分配/复制一个新 `Vec`，成本与传入 ID 数量线性相关。

## 与 Go 版本的对应关系

Rust 入口与 `pkg/executor/internal/util/partition_table.go::UpdateExecutorTableID` 保持同名及同一分支结构：nil/None 成功；三种扫描的写入方式相同；Selection、Aggregation/StreamAgg、TopN、Limit、ExchangeSender、CTESink、Projection、Window、Sort、Expand、Expand2 沿 child 下降；ExchangeReceiver、CTESource 停止；Join 使用 `children[1-inner_idx]`；未知类型产生包含协议码的错误。

Go 版本直接操作 `tipb.Executor` 的各 protobuf 字段，Rust 版本则操作本地最小强类型模型。这是最重要的迁移差异：Rust 逻辑目前不能直接接收线上 protobuf DAG。Go 从 `context.Context` 中读取字符串键 `nextPartitionUpdateDAGReq`，进行未经类型检查的 map 断言；Rust 用显式生命周期和 `Option<&mut HashSet<i64>>` 表达同一记录能力，消除了动态类型断言。

Go `PartitionIds = partitionIDs` 赋切片头，Rust 将切片复制进自有 `Vec`；两者在函数后都表达完整替换，但拥有关系不同。Go 用 `errors.Trace` 包装递归错误，Rust 用 `?` 传播枚举错误。Go 的真实生产调用点已接入 TableReader 与 MPP coordinator，而当前 Rust 仓库尚无等价生产调用边，因此功能状态只能表述为行为移植与单元覆盖完成、生产集成未验证。

## 扩展指南

- 新增 tipb 算子时，先判断它是扫描、一元透传、递归停止点、Join 类多输入节点还是未知边界；同步修改 `ExecType`、必要的 `ExecutorData`/构造器以及 `UpdateExecutorTableID` 的穷尽匹配。多输入算子必须明确选择规则，不能默认改写所有 child。
- 若要接入真实 Rust 执行主链，应在 DAG 请求构建/MPP 序列化边界增加真实 protobuf 适配或改为操作真实 tipb 类型，并以 Go 的 `table_reader.go`、`local_mpp_coordinator.go` 调用时机为兼容基准；不要仅把最小模型误接到线上数据结构。
- 改变空 `partition_ids` 行为前必须评估 Go 兼容性。若改成返回错误，需要同时调整错误枚举及 `migration_aster_unit_test.rs` 中对 panic 的断言，不能静默“安全化”而造成语义漂移。
- 修改递归规则时，在独立测试 `pkg/executor/internal/util/migration_aster_unit_test.rs` 扩展 `update_executor_table_id_matches_recursive_go_paths`；扫描与 context 行为对应 `update_executor_table_id_handles_scan_variants_and_context`；终端、未知协议与 None 对应 `update_executor_table_id_covers_terminal_unknown_and_nil`；空 ID 边界对应 `update_executor_table_id_matches_go_partition_id_boundaries`。测试逻辑应继续留在独立文件，不能内嵌进生产源文件。
- 性能敏感点是分区 ID 向量复制与递归深度；兼容敏感点是 Join outer-side 选择、终端停止集合、TableScan 专属记录行为及错误文本。新增 recording 行为时还要确认 HashSet 去重是否仍符合 Go map 语义。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 7,032 个 Rust 文件；`files --filter pkg/executor/internal/util` 列出目标源、Go 对照、crate 入口与独立测试；`node --file pkg/executor/internal/util/partition_table.rs --offset 1 --limit 420` 读取到完整 295 行并报告 17 个符号；`query UpdateExecutorTableID --json` 同时定位 Rust `partition_table.rs:221` 与 Go `partition_table.go:26`。限定符号的 `callers/callees` 未返回可用边，因此调用关系另以文本搜索核验，未把空结果推断为完整调用图。
- Rust 源与模块边界：`pkg/executor/internal/util/partition_table.rs`、`pkg/executor/internal/util/lib.rs`、`pkg/executor/internal/util/Cargo.toml`、根 `Cargo.toml` 和 `pkg/lib.rs`。
- Go 对照与生产入口：`pkg/executor/internal/util/partition_table.go`、`pkg/executor/table_reader.go`、`pkg/executor/internal/mpp/local_mpp_coordinator.go`。
- 独立 Rust 测试：`pkg/executor/internal/util/migration_aster_unit_test.rs`，覆盖扫描变体、recording context、全部一元节点、递归开关、Join 两种 inner 下标、终端、None、未知协议以及空 ID 边界。仓库搜索未发现直接针对该函数的 Go `*_test.go`；Go 行为依据来自实现和上述生产调用点。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令确认目标文件存在且恰有十一个固定二级标题；人工复核重点为当前 Rust 未接线事实、Go/Rust 所有分支对应、错误与 panic 边界、扩展测试位置。
