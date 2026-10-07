# `pkg/planner/core/operator/logicalop/logical_cte_table.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-logicalop` crate，crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 以私有模块 `logical_cte_table` 装载它，再通过 `pub use logical_cte_table::*` 导出 `LogicalCTETable`。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 Go 包 `pkg/planner/core/operator/logicalop`；本文件直接使用该 crate 重导出的逻辑计划、schema 和统计类型，并通过 `base`、`expression` 两个路径依赖完成上下文与空 schema 初始化。

在 SQL 规划链中，它表示递归 CTE 对工作表（中间物化结果）的逻辑引用，而不是 CTE 定义本身。`pkg/planner/core/logical_plan_builder_runtime.rs` 的 CTE 绑定构建逻辑仅在 `binding.recursive_reference` 为真且未内联时构造 `LogicalCTETable`；普通非内联引用构造 `LogicalCTE`，内联引用则直接消费种子计划。

## 核心职责

- `LogicalCTETable` 保存递归 CTE 工作表扫描所需的名称、存储 ID、种子 schema 和共享种子统计。
- `Init` 把通用逻辑计划基类初始化为类型字符串 `"CTETable"`，并保留查询块 offset，供统一逻辑计划接口和后续物理化使用。
- `DeriveStats` 在允许使用缓存时直接返回节点已有统计；需要重算时从 `SeedStat` 读取快照，写入节点自身缓存并返回。
- `LogicalPlan` 实现只提供动态类型访问、基类访问以及统计推导分派；其余通用逻辑计划行为来自 `BaseLogicalPlan`/`LogicalSchemaProducer`，本文件不实现底层表扫描或 CTE 迭代。

## 主要符号

- `pub struct LogicalCTETable`：公开逻辑算子。`LogicalSchemaProducer` 承载基类、输出 schema、输出名和统计缓存；`SeedStat: Arc<RwLock<StatsInfo>>` 是与 CTE 生产侧共享的统计容器；`Name` 用于物理扫描说明；`IDForStorage` 关联工作表缓冲；`SeedSchema` 保存种子侧列布局。
- `impl Default for LogicalCTETable`：建立空基类、空字符串、零存储 ID、默认统计锁和空 schema。该值主要服务结构体更新语法和初始化流程，本身不包含可执行查询所需的上下文或真实绑定。
- `pub fn Init(self, ctx, offset) -> Self`：调用 `crate::NewBaseLogicalPlan(ctx, "CTETable", offset)`，返回已带计划上下文和类型标记的值。
- `pub fn DeriveStats(&mut self, reload) -> Result<(StatsInfo, bool)>`：本类型的实际统计逻辑。布尔返回值表示本次是否重新装载统计，而不是执行是否成功。
- `impl LogicalPlan for LogicalCTETable`：`as_any`/`as_any_mut` 支持下游按具体类型分派；`base`/`base_mut` 暴露嵌入基类；trait 的 `DeriveStats` 委托到固有方法，避免复制逻辑。

## 执行流程

1. `logical_plan_builder_runtime.rs` 解析到未内联的递归 CTE 引用时，从 CTE binding 取出共享 `seed_stat`、名称、`storage_id` 和种子 schema，构造 `LogicalCTETable`，再调用 `Init(builder.ctx.clone(), query_block)`。
2. 构建器随后通过通用 `LogicalPlan` API 设置该引用可见的 schema 和输出名，并开启列裁剪、键推导优化标志；这些状态位于嵌入的 `LogicalSchemaProducer`，不由本文件另存一份。
3. 优化器经 `LogicalPlan::DeriveStats` 请求统计。若 `reload == false` 且 `StatsInfo()` 已有缓存，函数克隆缓存并返回 `(stats, false)`，不读取共享锁。
4. 否则函数取得 `SeedStat` 读锁、克隆统计快照并立即释放锁，再以 `SetStats` 写入本节点缓存，返回 `(stats, true)`。
5. 物理选优代码 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 通过 `as_any_mut().downcast_mut::<LogicalCTETable>()` 识别节点；它要求 root task 且不能有无法添加 enforcer 的排序要求，然后用 `Name`、`IDForStorage`、当前 schema 与统计创建 `PhysicalCteScan`/`RootTask`。

## 数据与状态

`SeedStat` 是跨逻辑 CTE 生产者和工作表引用共享的状态。其 `Arc` 提供共享所有权，`RwLock` 允许生产侧更新、消费侧读取；`LogicalCTETable::DeriveStats` 总是克隆锁内值，因此返回值及节点缓存不借用锁保护的数据。相邻 `LogicalCTE::DeriveStats` 在完成种子计划统计推导后通过同一写锁更新共享值，这构成工作表引用观察生产侧统计的直接通道。

节点同时有两层统计状态：共享的 `SeedStat` 和 `LogicalSchemaProducer` 中由 `SetStats` 管理的本地缓存。未要求 reload 时，本地缓存优先，因此共享值后续改变并不会自动使已缓存结果失效；调用方必须以 `reload == true` 请求刷新。

`IDForStorage` 是逻辑定义与物理 CTE 缓冲之间的身份键；物理化代码把它编码进 `CTE:<name> data:CTE_<id>`。`SeedSchema` 在 Rust 构建器中被赋值，但当前 Rust 搜索未发现目标文件外的读取点；Go 对照明确将其用于列统计收集器的列映射。因而不能把 Rust 字段的存在等同于该消费链已经接通。

## 依赖与调用关系

上游直接入口是 `pkg/planner/core/logical_plan_builder_runtime.rs` 的 CTE 表源构建分支，它创建并初始化 `LogicalCTETable`。RustCodeGraph 对固有 `DeriveStats` 的调用轨迹显示：trait 实现中的同名方法直接委托它，优化器的 `refresh_join_order_stats` 与 `is_empty` 也会沿逻辑计划统计接口触发该实现。

下游依赖包括：

- `crate::NewBaseLogicalPlan`：建立通用计划上下文、类型和查询块 offset。
- `LogicalSchemaProducer::{StatsInfo, SetStats}`（经方法提升调用）：读取和更新节点统计缓存。
- `Arc<RwLock<StatsInfo>>`：读取生产侧共享统计。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：按具体类型将该逻辑叶子转换为根任务上的 `PhysicalCteScan`，并在 CTE 自连接排序辅助逻辑中读取 `IDForStorage`。
- `pkg/planner/core/operator/physicalop/physical_projection.rs`：把 `LogicalCTETable` 识别为特定逻辑节点类别，说明动态类型能力也被其他物理规则使用。

该结构由 `logicalop/lib.rs` 重导出，调用方不需要访问私有模块路径。

## 错误处理与边界

`DeriveStats` 的签名采用 crate 的 `Result`，但当前函数体没有产生业务错误的分支；成功路径只有缓存命中或共享统计读取。读锁若曾被 panic 污染，代码以 `PoisonError::into_inner` 继续读取最后保留的数据，而不是 panic 或返回错误。这保持规划继续进行，但也意味着锁污染不会通过返回值显式暴露。

缓存命中条件严格为 `!reload && StatsInfo().is_some()`；默认的共享 `StatsInfo` 可能是零值，函数不会验证它是否对应已经优化的种子计划。正确性依赖构建/优化顺序以及生产侧及时更新 `SeedStat`。`Default` 产生的空上下文、空名称、零 ID 和空 schema 仅是占位状态；真正进入计划树前应由构建器填充并调用 `Init`。

物理边界不在本文件内实现：`base_physical_plan.rs` 会拒绝非 root task，或拒绝带排序且不能添加 enforcer 的属性请求。该逻辑节点本身没有 children，也不承诺排序。

## 并发与资源生命周期

共享统计的所有权随 `Arc` 克隆跨 `LogicalCTE` 与 `LogicalCTETable` 延续；最后一个持有者释放时统计对象才销毁。读锁只覆盖一次 `StatsInfo::clone`，`SetStats` 在锁释放后执行，避免把节点缓存更新包含在共享锁临界区内。生产侧在 `LogicalCTE::DeriveStats` 末尾取得写锁替换种子统计；读写由 `RwLock` 串行化，因此不会出现 Rust 数据竞争。

本类型没有线程、异步任务、通道、文件句柄或事务资源，也不拥有物化缓冲本身。`IDForStorage` 只是缓冲身份；实际缓冲和扫描生命周期由物理计划/执行器负责。缓存与共享值之间没有通知或版本号，刷新生命周期由调用方传入 `reload` 控制。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/operator/logicalop/logical_cte_table.go`。两端都嵌入 schema producer，保存 `SeedStat`、`Name`、`IDForStorage`、`SeedSchema`，并以 CTETable 类型和查询块 offset 初始化基类。两端在无需 reload 且已有缓存时都返回缓存和 `false`，否则复制/设置种子统计并返回 `true`。

关键实现差异如下：

- Go 的 `SeedStat` 是 `*property.StatsInfo`，生产侧通过覆盖指针指向的值传播更新；Rust 用 `Arc<RwLock<StatsInfo>>` 表达共享可变所有权，并在消费时克隆快照。
- Go 从 `reloads []bool` 仅在长度恰为 1 时取值；Rust trait 已把调用约定收敛为单个 `bool`，不再表达非法长度这一输入形态。
- Go `SetStats` 保存共享指针；Rust `SetStats` 保存克隆值，因此 Rust 本地缓存不会随共享对象自动变化，必须依赖 reload 刷新。
- Go 注释明确 `SeedSchema` 供 `columnStatsUsageCollector` 做列映射，并且 `collect_column_stats_usage.go` 有 `LogicalCTETable` 分支；当前 Rust 代码只见构建器赋值，未找到等价消费点，应视为尚未验证/可能未接线，而非已完整移植。
- Go `Init` 把具体计划指针交给基类；Rust 通过 `Any` 方法和 trait object 下转型实现具体类型识别。

## 扩展指南

若修改统计刷新语义，首要接入点是固有 `LogicalCTETable::DeriveStats`，同时核对 `LogicalCTE::DeriveStats` 写入 `SeedStat` 的时机以及 `optimizer_runtime.rs` 中统计刷新调用；不要只改 trait 委托层。需要明确缓存失效规则，尤其是共享统计更新后 `reload == false` 的行为，并评估克隆大型 `ColNDVs` 映射的成本。

若接通列统计使用收集，应以 Go 的 `collect_column_stats_usage.go` 为行为证据，在 Rust 对应规则中消费 `SeedSchema` 并验证可见列到种子列的映射；不要在本逻辑叶子中臆造扫描逻辑。若扩展物理属性或执行行为，应同步检查 `base_physical_plan.rs` 对 root task、排序和 `PhysicalCteScan` 的约束，以及 `physical_cte_table.rs`/执行器的存储 ID 契约。

测试必须放在独立测试文件中。当前最接近的 Rust 测试是 `logical_datasource_aster_unit_test.rs::cte_uses_shared_seed_statistics`（验证 `LogicalCTE` 共享统计）和 `physicalop/physical_cte_table_test.rs`（验证物理表选优/克隆），但它们不直接实例化本类型。新增行为应在同目录独立 `*_test.rs` 中直接覆盖：缓存命中返回 `false`、强制 reload 观察共享更新、锁污染恢复、初始化类型/offset，以及存储 ID 到物理扫描的传递；并在 `logicalop/lib.rs` 的测试模块区接线。

## 验证依据

- 目标实现：`pkg/planner/core/operator/logicalop/logical_cte_table.rs`，已核对结构体、`Default`、`Init`、固有/trait `DeriveStats` 和动态类型/基类访问方法。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml` 与 `lib.rs`，已核对 crate 名、`base`/`expression` 路径依赖、Go 包映射、模块声明和重导出。
- 构建入口：`pkg/planner/core/logical_plan_builder_runtime.rs` 的递归引用分支，已核对字段来源、`Init`、schema/输出名设置和普通 CTE 分流。
- 共享状态生产侧：`pkg/planner/core/operator/logicalop/logical_cte.rs::LogicalCTE::DeriveStats`，已核对读写锁、种子统计推导和写回时机。
- 物理消费侧：`pkg/planner/core/operator/physicalop/base_physical_plan.rs`，已核对具体类型下转、root/sort 边界、名称/存储 ID/schema/统计传递，以及自连接存储 ID 辅助逻辑。
- Go 语义：`pkg/planner/core/operator/logicalop/logical_cte_table.go`、`logical_cte.go` 和 `pkg/planner/core/rule/collect_column_stats_usage.go`；相关 Go 测试引用见 `collect_column_stats_usage_test.go`。
- Rust 测试现状：`pkg/planner/core/operator/logicalop/logical_datasource_aster_unit_test.rs` 与 `pkg/planner/core/operator/physicalop/physical_cte_table_test.rs`；搜索未发现直接实例化 `LogicalCTETable` 的独立 Rust 单测。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；精确 `node` 查询确认两层 `DeriveStats` 的委托边，并显示固有方法经逻辑计划统计路径被 `refresh_join_order_stats`、`is_empty` 触发。综合 `explore`/宽调用者查询曾超时，因此调用链结论同时以精确节点轨迹和上述源码入口交叉验证。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构验证要求文档存在且固定二级标题恰好 11 个。
