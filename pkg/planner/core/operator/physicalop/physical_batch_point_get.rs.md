# `pkg/planner/core/operator/physicalop/physical_batch_point_get.rs`

## 文件定位

本文件定义 planner 物理算子 crate 中的单点查与批量点查数据结构：`PointGetPlan` 表示按一个整数 handle 或一组唯一索引值直接定位一行，`BatchPointGetPlan` 在前者之上保存多组键并完成批量键清理和分区裁剪。它属于 Cargo 包 `astersql-planner-core-operator-physicalop`；包清单 `pkg/planner/core/operator/physicalop/Cargo.toml` 的 `[package.metadata.porting]` 将其对应到 Go 包 `pkg/planner/core/operator/physicalop`。

模块入口 `pkg/planner/core/operator/physicalop/lib.rs` 以 `mod physical_batch_point_get` 声明本文件，并通过 `pub use physical_batch_point_get::*` 导出类型。该入口还为两个类型接入 `ConcretePhysicalOperator`/`PhysicalPlan`：`PointGetPlan` 直接使用自身的 `PhysicalSchemaProducer`，`BatchPointGetPlan` 复用内嵌 `PointGetPlan` 的 schema producer。因此本文件负责算子自身的数据和算法，统一物理计划 trait 的转接位于 `lib.rs`。

已找到的生产入口包括 `pkg/planner/core/point_get_plan_runtime.rs` 创建快速 `PointGetPlan`，以及 `pkg/planner/core/planbuilder_runtime.rs` 为部分计划构造 `PointGetPlan`/`BatchPointGetPlan`。下游会在 `pkg/executor/statement_ru_result.rs`、`pkg/executor/statement_ru_plan_walk.rs`、`pkg/planner/core/optimizer_runtime.rs` 和 `physical_utils.rs` 中按具体类型识别它们；缓存快照在 `cache_snapshot.rs` 捕获和恢复二者。`pkg/executor/builder.rs` 另有 `BatchPointGetPlanData` 抽象，但仓库搜索没有发现本文件类型对该 trait 的直接实现，不能据此声称 Rust 批量算子已完整接入该执行器构建路径。

## 核心职责

- `PointGetPlan` 聚合点查所需的计划上下文、schema、表/索引元数据、handle 或索引值、锁参数、访问列和缓存代价，并提供 EXPLAIN、克隆、代价、PB 占位编码及内存估算接口。
- `BatchPointGetPlan` 通过组合一个 `PointGetPlan` 共享上述元数据，再增加 `Handles`、`IndexValueRows`、`PartitionIdxs`、顺序和锁标志。
- `BatchPointGetPruningContext` 把语句时区/错误策略、表达式类型转换环境、分区表、显式分区选择及 handle 列位置借入裁剪过程。
- `PrunePartitionsAndValues` 对批量键执行 NULL 过滤、稳定去重、公共句柄编码、分区路由和三路数组同步压缩，并用返回布尔值表示是否可退化为无结果的 TableDual。
- `compact_batch_partition_values` 是分区裁剪的对齐核心：在原地稳定保留允许的元素，并保证索引值、动态 handle 和分区下标仍按同一位置对应。

本文件不是存储读取实现，也不执行 SQL。它准备并描述快速访问计划；真正的计划构建、执行器构造、类型识别和缓存生命周期分散在上述调用方中。

## 主要符号

`PointGetPlan` 的关键字段分为五组：

- 基础计划：`PhysicalSchemaProducer`、私有的 `output_names`、`DBName`。
- 访问目标：`TblInfo`、`IndexInfo`、`PartitionIdx`、`Handle`、`IndexValues`、`IdxCols`、`IdxColLens`。
- 语义标志：`UnsignedHandle`、`IsTableDual`、`Lock`、`LockWaitTime`。
- 表达式与列：`AccessConditions`、`Columns`、`AccessColumns`。
- 估算缓存：`CostValue`。

主要方法如下：

- `New` 建立 `TypePointGet` 的空计划；`Init` 重建基础计划、写入统计信息和查询块偏移。
- `Clone` 更换上下文，克隆 schema、表/索引元数据、表达式和列；可能传播 `expression::Error`。
- `Schema`/`SetSchema`、`StatsInfo`、`GetCtx`/`SetCtx`、`OutputNames`/`SetOutputNames` 和访问列、代价、子计划相关方法提供物理计划适配面。
- `OperatorInfo`、`AccessObject`、`ExplainInfo` 和 `ExplainNormalizedInfo` 生成可见或规范化说明；规范化 handle 固定显示为 `?`。
- `ResolveIndices` 只对当前 schema 的虚拟列调用 `ResolveIndicesForVirtualColumn`；它不遍历 `AccessConditions`。
- `Attach2Task` 克隆算子并包装成 `RootTask`；`GetPlanCostVer1`/`GetPlanCostVer2` 委托基础计划。
- `ToPB` 当前只生成带 table ID 的 `tipb::TableScan` 外壳，源码注释也将其标成点查下推占位。

`BatchPointGetPruningContext<'a>` 是纯借用上下文，并实现 `table::CastContext`，把 `TypeCtx`、`ErrCtx`、SQL mode 和连接 ID 委托给 `EvalContext`。其私有 `encode_unique_values` 校验唯一索引列数，按表列类型规范化字符串/枚举或调用 `table::CastValue`，最后以语句时区调用 `kv::codec::EncodeKey`。

`compact_batch_partition_values` 接收可变索引值、动态 handle、每项分区结果、输出分区下标、单分区约束、分区定义和显式分区名。它用读指针 `source` 与写位置 `position` 做原地 `swap`，最后 `truncate`，返回保留项数。

`BatchPointGetPlan` 的 `New`、`Init` 和 `Clone` 分别负责默认构造、把内嵌计划类型改为 `TypeBatchPointGet`、以及克隆批量键状态。`PrunePartitions` 只过滤 `PartitionIdxs`；`PrunePrecomputedPartitionsAndValues` 才会按既有分区数组同步重建整数 handle 与索引值行。它们与 session-aware 的 `PrunePartitionsAndValues` 是三种不同入口，扩展时不能混用其对齐保证。

## 执行流程

单点计划的典型流程是：调用 `PointGetPlan::New` 建立基础类型，构造方填写表、索引、键、schema、列和锁配置；需要统计信息时再调用 `Init`。EXPLAIN 先由 `AccessObject` 输出表、可选分区和可选索引，再由 `OperatorInfo` 附加 handle/锁信息。规范化形式隐藏具体 handle。进入物理计划统一接口后，`lib.rs` 的 `ConcretePhysicalOperator` 实现把解析、代价、PB 和内存操作转回本文件的方法。

批量计划以 `BatchPointGetPlan::New` 内嵌一个空 `PointGetPlan`，`Init` 先执行单点初始化再把 plan type 改成 `TypeBatchPointGet`。批量键的关键处理发生在 `PrunePartitionsAndValues`：

1. 必须先取得 `TblInfo`，否则返回“batch point get requires table metadata”。全局索引跳过表分区裁剪；非全局索引读取 `GetPartitionInfo`。动态分区模式会先清空旧 `PartitionIdxs`。
2. 公共主键路径逐行过滤 NULL，调用 `encode_unique_values` 做按列转换并编码公共 handle，使用 `HashSet` 按编码值稳定去重；保留首个出现项，并同步把 `IndexValueRows` 压到前缀。公共 handle 只通过返回值交给执行端，不写入整数 `Handles`。
3. 普通二级索引路径只移除含 NULL 的键行，保留重复项；整数 handle 路径用 `HashSet` 稳定去重并构造 `kv::IntHandle`。
4. 若存在分区信息，必须提供 `PartitionedTable`。算法将索引值写入对应表列，或按 handle 列的 unsigned 标志构造行，然后调用 `GetPartitionIdxByRow`，再用 `ReplaceWithOverlappingPartitionIdx` 修正重叠分区。
5. 无匹配分区、单分区不符或显式分区名不符的结果记为 `-1`。整数/二级索引路径若完全无匹配会在压缩前直接返回空 handle 与 `true`；公共句柄路径始终先同步压缩。
6. `compact_batch_partition_values` 稳定压缩有效项；动态分区模式把分区下标写入 `PartitionIdxs`。保留数为零时返回 TableDual 标志。
7. 整数 handle 路径将压缩后的动态 handle 回写为 `i64` 的 `Handles`。最终返回动态 handle 列表和 `false`。

批量 EXPLAIN 展示批量 handle（索引路径不展示）、`KeepOrder`、`Desc` 和 `Lock`；`AccessObject` 对 `PartitionIdxs` 排序去重后补充分区名。批量代价为单点 `GetCost` 乘以 `Handles` 与 `IndexValueRows` 较大的长度。

## 数据与状态

两个计划都是拥有型、可变的数据对象，没有内部共享锁。`PointGetPlan` 持有可选表/索引元数据的克隆、表达式 trait object、schema producer 和若干向量；`BatchPointGetPlan` 再拥有批量整数键、二维 Datum 行和分区下标。`Clone` 对这些容器和表达式做独立克隆，但 `NameSlice::Shallow` 的精确共享语义应以该类型实现为准。

批量裁剪维护以下不变量：有效项的相对顺序不变；公共句柄路径中返回的 handle 与 `IndexValueRows` 一一对应；动态分区模式下 `PartitionIdxs` 与保留键一一对应；单分区模式不追加分区下标。`compact_batch_partition_values` 根据输入是否为空决定移动 `values`，根据最初 `handles` 是否为空决定移动 handle，所以调用者必须保证非空平行数组与 `indexes` 至少覆盖相同的源位置。

状态更新是破坏性的：NULL、重复键和不匹配分区会从计划向量中删除；动态分区结果会重建 `PartitionIdxs`。特别是整数/二级索引完全无匹配的早退发生在统一压缩前，源码注释明确其原值可能保留；返回的 `is_table_dual` 才是“无结果”的权威信号。`PrunePartitions` 只改变分区数组，不同步键数组，只有调用方本来不依赖位置对齐时才安全。

`CostValue` 是调用者写入的简单缓存；`GetCost` 对普通单点保证至少为 `1.0`，TableDual 为零。批量成本再乘批量基数。`MemoryUsage` 是近似值：统计基础 producer、字符串、Datum/Column/Expression 以及批量 handle 容量，但不是分配器级精确测量。

## 依赖与调用关系

Cargo 直接依赖证明了本文件的边界：`base`/`property`/`costusage` 提供物理计划与代价协议，`expression` 提供 schema、列、相关列、求值/类型上下文和错误，`model` 提供表/索引/分区元数据，`types` 提供 Datum/名称，`kv` 提供 handle 与 key 编码，`table` 提供类型转换和分区路由，`stmtctx` 提供时区和错误处理，`mysql`/`parser_ast` 提供类型标志与分区名，`plancodec`/`tipb` 提供计划类型和 PB 外壳。

主要上游证据为：

- `pkg/planner/core/point_get_plan_runtime.rs`：创建并填充本文件的 `PointGetPlan` 快速计划。
- `pkg/planner/core/planbuilder_runtime.rs`：创建 `PointGetPlan` 或 `BatchPointGetPlan`，填写当前库、表、列、访问列、schema 和初始代价。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs`：对两种具体计划做缓存快照和恢复。

主要下游/观察者为：

- `pkg/planner/core/operator/physicalop/lib.rs`：实现 `ConcretePhysicalOperator` 并通过宏接入统一 `PhysicalPlan`。
- `pkg/planner/core/optimizer_runtime.rs` 和 `physical_utils.rs`：按具体类型应用优化器/计划分类逻辑。
- `pkg/executor/statement_ru_result.rs` 与 `statement_ru_plan_walk.rs`：识别点查类型以处理 RU 计划遍历/结果。
- `pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/executor/typed_point_get.rs` 和 `pkg/executor/builder.rs`：消费 `PointGetPlan` 的类型或引用。

RustCodeGraph 的文件节点报告本文件被 7 个文件使用，其中点名 `logical_plan_builder_runtime.rs`、`index_join_probe.rs`、`physical_batch_point_get_test.rs`、`physical_utils.rs` 和 `physical_utils_test.rs`。精确 callers/callees 子命令在本次查询中挂起，因此上述具体调用边由随后针对符号的 `rg` 搜索补齐；不把未找到的边推断为不存在。

## 错误处理与边界

`PointGetPlan::Clone`、`ResolveIndices`、两套代价方法和 `ToPB` 使用 `expression::Error`。`Attach2Task` 对克隆错误调用 `expect("point get clone")`，因此这里是 panic 边界，而不是可恢复传播。`AccessObject` 在缺表时显示 `table:unknown`，分区下标越界时显示 `partition:dual`，用于解释输出而非校验计划合法性。

`encode_unique_values` 对键值数量与索引列数不等、索引列 offset 越界返回错误。字符串统一保留 collation；枚举名称解析失败和截断型转换返回 `Ok(None)`，表示该键不可能命中而应被跳过；其他转换错误升级为 `expression::Error`。`EncodeKey` 错误交给 `StatementContext::HandleError`：若策略返回错误则传播，若错误被吞掉则以空字节继续，这与语句错误策略绑定。

分区路径在缺少分区表、handle 列 offset 越界、公共 handle 构造失败时返回错误。整数 handle 的 `ErrNoPartitionForGivenValue` 被视为不匹配，其他错误传播；索引行的分区路由错误则经过重叠分区替换后统一折叠为不匹配。显式分区名按 `CIStr.L` 做不区分大小写匹配。

当前实现还存在必须如实记录的能力边界：`LoadTableStats` 为空；`ExtractCorrelatedCols` 固定返回空；`GetAvgRowSize` 只是访问列数乘八；`ToPB` 只是 TableScan 形态占位；Rust 的 `PointGetPlan::Clone`/`Attach2Task` 比当前 Go 对照更积极地生成可用对象，而 Go 当前相应方法返回不支持或 nil。扩展者不能把这些接口的存在等同于与 Go 完整等价。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道或事务，也没有 `Arc`/`Mutex` 等内部同步。所有裁剪方法都要求 `&mut self`，由 Rust 独占借用保证同一计划在一次变换中不会被并发修改；跨线程可用性仍取决于字段中 trait object 和上下文类型的 `Send`/`Sync` 实现，本文件没有额外保证。

`BatchPointGetPruningContext<'a>` 只在调用期间借用 statement/eval context、可选分区表和分区名，不取得所有权，也不延长会话或事务资源的生命周期。动态 `kv::Handle` 在裁剪时临时构造并随返回 `Vec` 移交调用者；公共 handle 不塞入只能表达 `i64` 的 `Handles`，避免丢失编码信息。

原地 `swap` 加 `truncate` 使被拒绝元素在函数结束前释放，不做每次删除导致的后缀搬移。该设计降低批量裁剪的额外分配，但 `HashSet` 去重、行缓冲、分区结果和公共 handle 仍会产生与批量大小相关的内存；算法整体是线性扫描，`allowed.contains`、显式分区名查找等小切片查询则各自为线性查找。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/physical_batch_point_get.go`，独立 Go 回归是同目录 `physical_batch_point_get_test.go`。类型名称、PointGet/BatchPointGet 的职责、唯一索引 NULL 过滤、公共主键编码去重、显式/单分区筛选、重叠分区处理、EXPLAIN 标志以及 TableDual 返回意图均来自该 Go 实现。

Rust 的 `common_primary_duplicate_values_keep_first_occurrence` 与 Go 的 `TestPruneCommonHandleDuplicateValues` 使用同样的空输入、无重复、重复加 NULL、全重复、全 NULL 用例，验证保留首次出现且 handle 数量与索引值行一致。Rust 独立测试还覆盖 Go 流程中的更多局部不变量：公共 handle/值同步压缩、整数去重、二级索引 NULL 过滤但不去重、枚举转换、索引列数错误、单分区筛选和空压缩。

仍存在显著结构/行为差异：

- Go 两个计划包含 plan-cache 参数、字段类型、probe parents、显式分区名、统计成本缓存等更多字段；Rust 只移植了当前调用路径所需子集，并把批量公共元数据组合进 `PointGetPlan`。
- Go 的单点 `PrunePartitions`、真实 `LoadTableStats`、基于统计直方图的平均行宽、日志脱敏策略和更完整 `AccessObject` 尚未在本 Rust 文件等价实现。
- Go 的 `BatchPointGetPlan::Init` 同时安装 schema 和输出名；Rust `Init` 仅安装上下文/统计/offset 并改 plan type，schema 和名称由其他 setter/构造方负责。
- Go 的 `Attach2Task`/`ToPB` 返回 nil，`Clone` 报不支持；Rust 提供 RootTask 包装、TableScan PB 占位和深克隆。二者都不是“完整点查下推已经实现”的证据。
- Rust 把 session 依赖显式拆成 `BatchPointGetPruningContext`，且以 `Vec<i64>` 保存整数 handle；公共 handle 必须经返回值传递。Go 直接接受完整 session context 并以 `[]kv.Handle` 保存 handle。
- Go 的内存统计覆盖更多字段和容器容量；Rust 当前为较粗估算。

因此安全移植应以具体方法逐项对齐，而不是仅凭同名类型认定功能等价。

## 扩展指南

新增计划字段时，应同时检查 `PointGetPlan::New`/`Clone`/`MemoryUsage` 或 `BatchPointGetPlan::New`/`Clone`/`MemoryUsage`，并同步 `cache_snapshot.rs` 的捕获与恢复；若字段参与统一物理计划行为，还要检查 `lib.rs` 中对应 `ConcretePhysicalOperator`。新增 Go 对齐字段时先判断其是否属于本提交/任务需要，避免把其他基础设施缺口递归扩大。

修改键清理或分区路由时，优先在 `PrunePartitionsAndValues` 和 `compact_batch_partition_values` 保持以下性质：稳定保留顺序；NULL 不产生点查；整数/公共 handle 去重保留首项；二级唯一索引值不额外去重；handle、值行和分区下标严格对齐；显式分区名大小写不敏感；公共 handle 通过返回值保留完整编码。必须在独立文件 `physical_batch_point_get_test.rs` 增加回归，不要把测试内嵌到生产源文件；同时检查 Go 测试意图是否需要等价用例。

增强类型转换时修改 `BatchPointGetPruningContext::encode_unique_values`，并区分“键不可能存在”（`Ok(None)`）与真正执行错误。增加新分区模式时同时检查全局索引跳过逻辑、`SinglePartition`、`PartitionNames`、重叠分区修正和 TableDual 早退顺序。

增强 EXPLAIN/脱敏时修改 `OperatorInfo`、`AccessObject` 及规范化方法，并注意当前 Rust 非规范化输出没有读取 Go 的 session redaction mode。实现真实 PB 或执行器接线时，应从 `ToPB`、`pkg/executor/builder.rs` 的数据 trait 和实际构建入口一起核验，不能只把占位 TableScan 扩写后就宣称端到端完成。改动 Rust 源码时还必须遵循仓库要求先同步独立测试、完成后运行 `cargo fmt --all`；本次仅写文档，没有修改源码或运行 Cargo。

## 验证依据

本说明使用了以下可复核证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `explore "pkg/planner/core/operator/physicalop/physical_batch_point_get.rs PhysicalBatchPointGetPlan"`：返回目标文件上下文；随后 `query PointGetPlan`、`query BatchPointGetPlan` 定位 Rust/Go 同名定义和相关构造符号。
- RustCodeGraph `node --file pkg/planner/core/operator/physicalop/physical_batch_point_get.rs --offset 1 --limit 1200`：读取完整 839 行目标源码，并报告文件级使用者。
- RustCodeGraph 精确 `callers/callees` 查询曾执行，但后端长时间无返回后被终止；因此具体上游/下游使用以 `rg` 对本文件符号的仓库搜索补证，没有伪造图边。
- 已读源码/配置：目标 `.rs`、同目录 `Cargo.toml`、`lib.rs` 的模块导出和两个 trait 接线、Go 对照 `physical_batch_point_get.go`。
- 已读测试：`physical_batch_point_get_test.rs` 与 `physical_batch_point_get_test.go`。Rust 测试覆盖固定基数/空相关列、EXPLAIN、公共句柄稳定去重和编码、数组对齐、整数去重、二级索引 NULL、枚举转换错误及单分区压缩；Go 测试提供公共句柄重复值的原始语义基线。
- 调用关系补证来自对 `point_get_plan_runtime.rs`、`planbuilder_runtime.rs`、`optimizer_runtime.rs`、`cache_snapshot.rs`、`physical_utils.rs`、executor 和 session runtime 中具体类型名的搜索。

本任务是纯文档分析，按计划不运行 Cargo 或代码测试。交付验证仅检查目标文档存在且恰好含有规定的十一个二级标题，并人工复核重要结论均指向上述源码符号、调用位置、Cargo 边界或测试证据。
