# `pkg/planner/core/operator/logicalop/logical_show_ddl_jobs.rs`

## 文件定位

本文件位于 `astersql-planner-core-operator-logicalop` crate，定义 `ADMIN SHOW DDL JOBS` 对应的 Rust 逻辑计划节点 `LogicalShowDDLJobs`。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 通过 `mod logical_show_ddl_jobs` 装配模块并用 `pub use logical_show_ddl_jobs::*` 对外导出该类型；`Cargo.toml` 的 `[package.metadata.porting]` 又把该 crate 映射到 Go 包 `pkg/planner/core/operator/logicalop`。

它处在语句解析/计划构建与物理执行之间的逻辑算子层，但当前 Rust 接线尚不等同于 Go 主链：Rust `pkg/planner/core/planbuilder.rs::buildAdmin` 对 `AdminStatement::ShowDdlJobs` 返回通用的 `BuiltPlan::Admin`，仓库内未找到该构建器实例化 `LogicalShowDDLJobs`；该类型现有的 Rust 外部引用主要是 cascades memo 的哈希/相等分派和独立测试。Go 版本则由 `pkg/planner/core/planbuilder.go` 直接构造该逻辑节点，并在 `pkg/planner/core/operator/physicalop/physical_show.go` 转成 `PhysicalShowDDLJobs`。因此，本文件描述的是已实现的逻辑节点能力，不代表 Rust 规划到执行的完整生产链已经接通。

## 核心职责

- 用 `LogicalShowDDLJobs` 保存该语句的输出 schema、逻辑计划公共状态和请求展示的任务数量 `JobNumber`。
- 用 `Init` 把会话规划上下文写入 `BaseLogicalPlan`，分配计划 ID，并把节点类型标记为 `"ShowDDLJobs"`。
- 用 `DeriveStats` 为无用户表扫描的叶子节点建立简单统计：估计行数固定为 `1.0`，每个输出列的 NDV 固定为 `1.0`，并缓存到计划基座。
- 实现 `LogicalPlan` 的类型擦除和基座访问入口，使通用优化代码可以把它当作 trait object 使用；统计推导方法显式转发到本文件的固有实现。

本文件不读取 DDL 元数据、不生成结果行，也不实现物理算子。真实 DDL job 枚举和展示属于执行层；`JobNumber` 在本逻辑节点中仅作为后续阶段所需的载荷。

## 主要符号

- `pub struct LogicalShowDDLJobs`：唯一的生产类型，派生 `Default`。
  - `LogicalSchemaProducer: LogicalSchemaProducer`：组合字段，内部持有 `BaseLogicalPlan`；schema、输出列名、计划上下文、ID 和统计缓存均由该基座管理。
  - `JobNumber: i64`：请求展示的最近 DDL 任务数量上限。本文件不校验其范围，也不在统计估计中使用它。
- `LogicalShowDDLJobs::Init(self, ctx: base::ContextRef) -> Self`：按值接收默认或已填充载荷的节点，调用 `NewBaseLogicalPlan(ctx, "ShowDDLJobs", 0)` 替换基座后返回。第三个参数 `0` 是 query-block offset。
- `LogicalShowDDLJobs::DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)>`：读取或生成统计。返回元组第二项表示这次是否产生了新统计，而不是操作是否成功。
- `impl LogicalPlan for LogicalShowDDLJobs`：提供 `as_any`/`as_any_mut` 下转型入口，提供 `base`/`base_mut` 基座入口，并把 trait 的 `DeriveStats` 调用转发给固有方法。其余 trait 默认行为来自 `BaseLogicalPlan`。
- `Hash64` 与 `Equals` 不在本文件中，而由 `pkg/planner/core/operator/logicalop/hash64_equals_generated.rs` 为本类型补充：两者只比较/散列 `LogicalSchemaProducer`，明确忽略 `JobNumber`。

## 执行流程

1. 调用者先构造节点并设置 `JobNumber`，再调用 `Init`。`NewBaseLogicalPlan` 通过上下文分配 ID，保存上下文和类型名，并以空 children、空 schema、无统计缓存等默认状态初始化基座。
2. 计划构建方需要另行设置输出 schema。Rust `buildShowDDLJobsFields` 定义了 `JOB_ID`、`DB_NAME`、`TABLE_NAME`、`JOB_TYPE`、`SCHEMA_STATE`、`SCHEMA_ID`、`TABLE_ID`、`ROW_COUNT`、`START_TIME`、`STATE` 十列，但当前通用 `BuiltPlan::Admin` 路径并未在仓库中实例化本类型。
3. 优化阶段通过 `LogicalPlan::DeriveStats` 进入本类型的 `DeriveStats`。
4. 当 `reload == false` 且基座已有 `StatsInfo` 时，函数克隆缓存并返回 `(cached, false)`；schema 后续若发生变化，调用方必须用 `reload == true` 才会重建列 NDV。
5. 需要重算时，函数创建 `RowCount = 1.0` 的默认 `StatsInfo`，遍历 `self.Schema().Columns`，以每列 `UniqueID` 为键写入 `ColNDVs[UniqueID] = 1.0`，然后用 `SetStats` 保存克隆值并返回 `(stats, true)`。
6. cascades memo 在 `pkg/planner/cascades/memo/group_expr.rs` 中可向下转型到该类型，使用生成的 `Hash64`/`Equals` 参与逻辑表达式去重；由于生成契约忽略 `JobNumber`，同 schema、不同数量限制的节点会被视为相等。

## 数据与状态

节点自身的业务载荷只有 `JobNumber`；其余可变状态均位于嵌入的 `LogicalSchemaProducer.BaseLogicalPlan`。与本文件直接相关的基座状态包括计划上下文、类型字符串、计划 ID、query-block offset、输出 `Schema`、输出列名以及可选的 `StatsInfo` 缓存。

`StatsInfo` 来自 `astersql-planner-property`，本文件只赋值 `RowCount` 和 `ColNDVs`。`HistColl`、`StatsVersion` 和 `GroupNDVs` 保持默认值。列 NDV 的键是 schema 列的 `UniqueID`；若 schema 中出现重复 `UniqueID`，后写入会覆盖前值，但结果仍为 `1.0`。空 schema 会得到行数为 `1.0`、空 `ColNDVs` 的统计。

`Default` 构造的节点尚无上下文，类型名为空、ID 为零且 schema 为空。只有经过 `Init` 才获得可用于正常规划的上下文和类型标识；本文件没有用类型系统强制这一初始化顺序。

## 依赖与调用关系

- 直接上游接口：`LogicalPlan::DeriveStats` 的动态分派会调用本类型的转发实现；`pkg/planner/cascades/memo/group_expr.rs` 的 `hash_logical_plan` 和 `equal_logical_plans` 会向下转型为 `LogicalShowDDLJobs`。
- 模块与生成代码：`lib.rs` 注册并导出本模块；`hash64_equals_generated.rs` 为该类型实现语义哈希和相等判断；生成器配置 `pkg/planner/core/generator/hash64_equals/hash64_equals_generator.rs` 只登记 `LogicalSchemaProducer`，所以 `JobNumber` 被排除。
- 直接下游：`Init` 调用同 crate 的 `NewBaseLogicalPlan`；`DeriveStats` 通过 `LogicalPlan` 默认访问器调用 `StatsInfo`、`Schema` 和 `SetStats`，并构造 re-export 自 `property` crate 的 `StatsInfo`。
- crate 边界：本文件显式使用 `base::ContextRef`，而当前 crate 的 `Cargo.toml` 以路径依赖连接 `astersql-planner-core-base`、`astersql-planner-property`、`astersql-expression` 等规划基础 crate；本文件没有网络、存储或 DDL crate 依赖。
- Go 生产调用链：`planbuilder.go` 构造并设置 schema；`physical_show.go::findBestTask4LogicalShowDDLJobs` 读取 `JobNumber` 并生成物理节点。此链是 Go 对照证据，不应当作 Rust 当前已接线的调用边。

RustCodeGraph 对本文件列出的直接使用文件为 `pkg/planner/cascades/memo/group_expr.rs`、`pkg/planner/core/operator/logicalop/hash64_equals_generated_test.rs` 和 `pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.rs`。精确方法名查询当前只解析到同名 Go 方法，因而没有把缺失的 Rust 方法级调用边推断为“无调用”。

## 错误处理与边界

`Init` 不返回 `Result`；上下文分配计划 ID 的接口在这里没有可传播错误。`DeriveStats` 使用 crate 的 `Result<_, PlannerError>` 签名以满足统一规划接口，但当前函数体没有显式失败分支，正常路径均返回 `Ok`。

需要关注的边界如下：

- `reload == false` 时优先复用任何现有缓存；函数不会判断 schema 或 `JobNumber` 是否在缓存建立后改变。
- `reload == true` 总会重建并覆盖统计，即使已有缓存。
- `JobNumber` 可以是零或负数，本文件不验证；语义校验必须由语句构建或执行边界承担。
- 固定行数 `1.0` 是避免缺少统计的伪估计，不是实际返回 DDL job 数量，也不随 `JobNumber` 改变。
- 生成的 `Hash64`/`Equals` 忽略 `JobNumber` 是与 Go 生成契约一致的当前事实。若 memo 等价性需要区分数量限制，必须同时修改生成器契约、生成文件和测试，而不能只改本结构体。

## 并发与资源生命周期

本文件没有锁、原子变量、异步任务、通道、事务或显式 I/O。节点由调用者独占地以 `&mut self` 初始化统计，`StatsInfo` 以值克隆返回；这里不提供跨线程共享协议。

计划上下文的所有权由 `base::ContextRef` 的实际定义管理，本节点只把它交给 `BaseLogicalPlan` 保存。统计缓存随节点生命周期存在，重算时由新值替换；schema 和 children 同样随计划树节点销毁。DDL job 的读取、迭代和执行期资源不属于本文件，不能从该逻辑节点推断其锁或事务语义。

## 与 Go 版本的对应关系

Rust 类型和字段与 `pkg/planner/core/operator/logicalop/logical_show_ddl_jobs.go` 基本一一对应：两侧都有 `LogicalSchemaProducer`、`JobNumber`、`Init` 和自定义统计推导，并都以 `ShowDDLJobs` 作为计划类型。Go `Init` 通过 `plancodec.TypeShowDDLJobs` 传入类型常量并返回指针；Rust 当前传入等值字符串并按值返回。

统计语义保持核心意图：Go 在非 reload 且有缓存时复用统计，否则调用 `getFakeStats(selfSchema)`；Rust在相同缓存条件下复用，否则显式产生 `RowCount = 1.0` 且每列 NDV 为 `1.0` 的统计。签名存在迁移差异：Go 从 `reloads []bool` 仅在长度为一时取 reload，并接收子统计/self schema/子 schema；Rust 简化为单个 `reload: bool`，直接从节点 schema 读取列，且当前没有子节点参数。

Go 的生成代码同样只散列和比较 `LogicalSchemaProducer`，不包含 `JobNumber`；Rust 的 `hash64_equals_generated_test.rs::show_ddl_job_number_is_excluded_like_go_generated_contract` 专门锁定这一兼容行为。Go `logicalop_test/hash64_equals_test.go::TestLogicalShowDDLJobs` 与 Rust `logicalop_test/hash64_equals_test.rs::TestLogicalShowDDLJobs` 都验证 schema 改变会改变哈希并导致不相等。

生产接线尚有明确差异：Go `planbuilder.go` 和 `physical_show.go` 使用该具体类型贯穿逻辑到物理计划；Rust `planbuilder.rs` 当前走通用 `BuiltPlan::Admin`，且仓库 Rust 搜索未发现相应物理转换。后续迁移不能仅凭类型已存在就声称完整行为对齐。

## 扩展指南

- 调整节点初始化或类型编码时修改 `Init`，并优先改为复用 `plancodec::TypeShowDDLJobs`，同时检查 stringer、memo 类型判断和计划编解码兼容性。
- 调整统计估计时修改 `DeriveStats`，新增独立测试文件或扩展同目录独立测试模块，覆盖首次推导、缓存复用、强制 reload、空 schema 和多列 NDV；不要把测试内嵌到本生产文件。
- 新增参与 memo 等价性的字段时，必须修改 `pkg/planner/core/generator/hash64_equals/hash64_equals_generator.rs` 的字段登记并重新生成/同步 `hash64_equals_generated.rs`，同时更新两个 Rust 哈希测试以及 Go 对照生成契约。否则可能发生语义不同的节点错误合并。
- 若要接通 Rust 生产主链，需要在计划构建层明确创建并设置 `LogicalShowDDLJobs` 的 schema/输出名，并在物理化与 executor builder 边界传递 `JobNumber`；应以 Go `planbuilder.go`、`physical_show.go` 和 executor 行为为对照，但不要在本叶子文件中塞入 DDL 元数据访问。
- 若改变 `JobNumber` 的合法范围或实际输出语义，应在 AST/planbuilder 或执行层增加验证和用户可见错误，并同步 `pkg/executor/test/executor/executor_test.rs` 中 ADMIN SHOW DDL JOBS 相关测试；本文件只承载计划数据。
- 性能风险主要来自错误的基数估计影响优化器选择，以及哈希等价字段遗漏造成 memo 合并；兼容风险主要来自列 schema、类型编码和 Go/Rust 语义漂移。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file ...logical_show_ddl_jobs.rs` 完整读取了本文件 82 行，并报告 memo 分派与两处 Rust 测试使用。对 `LogicalShowDDLJobs` 的查询同时定位到 Rust 结构体、Go 对照类型/方法和两侧测试。
- 目标源码：`pkg/planner/core/operator/logicalop/logical_show_ddl_jobs.rs`，核对结构体、`Init`、固有 `DeriveStats` 和 `LogicalPlan` 实现。
- 基座与数据定义：`logical_schema_producer.rs`、`base_logical_plan.rs`、`pkg/planner/property/stats_info.rs`，核对 schema/统计访问器、初始化默认值、计划 ID 分配和 `StatsInfo` 字段。
- crate 与模块：`pkg/planner/core/operator/logicalop/Cargo.toml`、`lib.rs`，核对 crate 名称、路径依赖、Go 包映射以及公开导出。
- Rust 调用与接线：`pkg/planner/cascades/memo/group_expr.rs`、`pkg/planner/core/planbuilder.rs`、`pkg/planner/core/operator/logicalop/hash64_equals_generated.rs` 和生成器配置，核对 memo 分派、当前通用 ADMIN 构建路径及哈希字段集合。
- Go 对照：`logical_show_ddl_jobs.go`、`pkg/planner/core/planbuilder.go`、`pkg/planner/core/operator/physicalop/physical_show.go`、`hash64_equals_generated.go`，核对构建、统计、物理化和生成契约。
- 测试证据：`hash64_equals_generated_test.rs::show_ddl_job_number_is_excluded_like_go_generated_contract`、Rust/Go `logicalop_test/...::TestLogicalShowDDLJobs`；仓库搜索未发现针对本 Rust 类型 `DeriveStats` 的专门测试，因此其缓存与 NDV行为是源码直接验证、不是测试覆盖结论。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证文档存在且恰好包含十一个固定二级章节。
