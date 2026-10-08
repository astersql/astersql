# `pkg/planner/core/operator/logicalop/logical_table_scan.rs`

源码：[logical_table_scan.rs](./logical_table_scan.rs)

## 文件定位

本文件实现逻辑优化阶段的表路径扫描叶子算子 `LogicalTableScan`。它属于
`astersql-planner-core-operator-logicalop` crate；同目录 `lib.rs` 以
`mod logical_table_scan` 注册模块、以 `pub use logical_table_scan::*` 导出符号，并在
`#[cfg(test)]` 下装配独立测试 `logical_table_scan_test.rs`。`Cargo.toml` 的
`package.metadata.porting.go-package` 将该 crate 对应到 Go 包
`pkg/planner/core/operator/logicalop`。

该算子不直接访问 KV。`logical_datasource.rs::DataSource::buildTableGather` 把一个已选定的表访问路径固化为本节点，再将其置于 `TiKVSingleGather` 之下；旧 Cascades 实现规则
`ImplTableScan` 随后读取本节点状态并构造 `PhysicalTableScan`。因此，本文件处在“数据源访问路径选择”和“物理表扫描执行计划”之间，负责保存路径语义并提供逻辑属性。

## 核心职责

- 以 `LogicalTableScan` 保存共享数据源、句柄列、访问条件、扫描后过滤条件、键范围和目标存储类型。
- 通过 `Init` 建立名为 `TableScan` 的基础逻辑计划，并保留查询块偏移和计划上下文。
- 通过 `ExplainInfo` 输出数据源说明、数据源句柄列和访问条件，供 EXPLAIN 使用。
- 通过 `BuildKeyInfo` 让数据源先完成键推导，再把 `PKOrUK` 与 `NullableUK` 复制到扫描节点的输出 schema。
- 通过 `DeriveStats` 维护统计缓存、估算行数、约束 NDV，并依据句柄类型和访问条件重建表范围。
- 通过 `PreparePossibleProperties` 声明句柄列顺序以及 TiFlash/MPP 可用性。
- 通过 `impl LogicalPlan` 提供下转型、基础状态访问和上述核心行为的动态分派入口。

本文件不负责选择访问路径、拆分谓词或执行存储读取；这些职责分别位于 `DataSource` 访问路径逻辑、`buildTableGather` 的装配过程和后续物理计划/执行层。

## 主要符号

- `LogicalTableScan`：核心结构。`LogicalSchemaProducer` 承载 `BaseLogicalPlan`、输出 schema、名称和统计缓存；其余字段描述一次表路径扫描。
- `Source: Option<DataSourceRef>`：共享的 `DataSource`。正常构造路径必填；`Option` 允许默认对象和测试对象存在。
- `HandleCols: Option<Box<dyn HandleCols>>`：扫描自身使用的句柄列，可表示整数主键、隐式行 ID 或复合句柄抽象。
- `AccessConds`：已能参与范围构造的表达式；`TableFilters`：未完全成为范围、需要在扫描链后续过滤的表达式；`Ranges`：实际键范围；`StoreType`：TiKV、TiFlash 等存储选择。
- `Default::default()`：产生无上下文、无来源、空条件/范围且默认 `StoreType::TiKV` 的未装配节点。它是分阶段构造起点，不是可直接物理化的完整扫描。
- `Init(ctx, offset)`：调用 `NewBaseLogicalPlan(ctx, "TableScan", offset)` 初始化基础计划。
- `ExplainInfo()`：无来源时返回 `table scan`；有来源时基于 `DataSource::ExplainInfo`，追加 `Source.HandleCols` 和本节点 `AccessConds` 的格式化文本。
- `BuildKeyInfo()`：无来源时无操作；有来源时调用 `DataSource::BuildKeyInfo`，再克隆来源 schema 的强唯一键和可空唯一键集合。
- `DeriveStats(reload)`：支持缓存命中；从 `Source.TableStats` 起步，以首个表路径的正数 `CountAfterAccess` 覆盖行数，将每列 NDV 限制到行数以内，构建范围并写回统计缓存。
- `PreparePossibleProperties()`：若有句柄列，将 `IterColumns2` 的完整句柄列序列作为唯一候选顺序；仅当来源存在可用 TiFlash 副本且会话允许 MPP 时返回 `HasTiFlash = true`。
- `impl LogicalPlan for LogicalTableScan`：使公共优化流程能通过 trait 对象调用 `ExplainInfo`、`BuildKeyInfo` 和 `DeriveStats`，并通过 `Any` 恢复具体类型。

本文件没有模块级常量、条件编译项或内部辅助函数；所有生产行为都集中在上述结构、`Default`、固有 impl 与 `LogicalPlan` impl 中。

## 执行流程

1. `DataSource` 完成候选访问路径分析后，`DataSource::buildTableGather(source, path)` 从来源取得上下文、查询块偏移、schema、输出名和句柄列克隆。
2. 构造入口创建 `LogicalTableScan::default()`，在存在上下文时调用 `Init`，随后从 `AccessPath` 复制 `AccessConds`、`TableFilters`、`Ranges` 和 `StoreType`，并设置 schema/输出名。
3. 构造入口再建立 `TiKVSingleGather`，把扫描作为唯一孩子；gather 同时保留存储类型和表过滤条件。由此，本节点是 gather 下方的逻辑叶子。
4. 通用优化过程通过 `LogicalPlan` trait 调用 `BuildKeyInfo` 和 `DeriveStats`。`BuildKeyInfo` 同步来源键；`DeriveStats` 可复用缓存，或重新计算统计和 Range。
5. 有句柄列时，`DeriveStats` 取第 0 个句柄列的返回类型，将克隆的 `AccessConds`、ranger 上下文和类型交给 `ranger::BuildTableRange`。无句柄列时，它根据整数主键是否无符号选择 `ranger::FullIntRange`。
6. 属性准备阶段调用 `PreparePossibleProperties`，将句柄顺序和 TiFlash/MPP 能力交给优化器。Cascades pattern 将具体类型识别为 `OperandTableScan`。
7. 旧 Cascades 的 `ImplTableScan::Match` 只接受无排序要求，或仅要求第一个句柄列顺序的属性；`OnImplement` 校验来源，构造物理扫描并复制表、列、Range，以及 `KeepOrder`/`Desc`。

统计推导的顺序很重要：先确定行数，再把 `ColNDVs` 全部钳制到 `RowCount`，最后构建 Range 和写缓存。这样不会留下“列不同值数大于输出行数”的内部矛盾。

## 数据与状态

- `Source` 使用 `DataSourceRef`（共享、内部可变引用），让多个规划节点读取同一份表元数据、统计和路径信息。`LogicalTableScan` 自身持有独立的 schema、统计缓存和 Range。
- `HandleCols` 与 `Source.HandleCols` 用途不同：前者决定 Range 与候选顺序；`ExplainInfo` 明确读取后者。独立测试用不同列 ID 验证 EXPLAIN 显示来源句柄而非扫描字段。
- `AccessConds` 既影响统计选择性语义，又在有句柄时参与 Range 构建。`TableFilters` 和 `StoreType` 由正常构造入口保存，但本文件方法不直接消费；它们供扫描链装配或后续阶段使用。
- `Ranges` 是派生且可变的状态。构造入口会复制访问路径已有 Range，之后 `DeriveStats` 可能用当前访问条件重新覆盖它。
- 统计缓存存于基础计划。`reload == false` 且缓存存在时返回 `(cached, false)`，不会重建 Range；否则返回 `(stats, true)`。
- `PreparePossibleProperties` 只返回属性值，不把 `HasTiFlash` 写入本节点字段。只有来源可用 TiFlash 且 `SCtx` 存在并允许 MPP 时才报告可用。

关键不变量包括：正常物理化前必须有 `Source`；非空 `HandleCols` 必须至少能提供第 0 列且该列必须带返回类型；句柄列顺序必须与扫描可保持的记录键顺序一致；`Ranges` 必须与当前 `AccessConds` 和句柄类型相容。

## 依赖与调用关系

上游与主链证据：

- `logical_datasource.rs::DataSource::buildTableGather` 是正常构造入口，负责从 `AccessPath` 填满字段并建立 `TiKVSingleGather -> LogicalTableScan` 关系。
- `base_logical_plan.rs::BaseLogicalPlan::RecursiveDeriveStats` 先递归孩子再经 trait 调用具体节点的 `DeriveStats`；公共键推导同样通过 `LogicalPlan::BuildKeyInfo` 分派。
- `cascades/memo/group_expr.rs::GroupExpression` 把 `BuildKeyInfo` 和 `DeriveStats` 转发给包装的具体逻辑算子，并消费可能属性的 TiFlash 标记。
- `cascades/pattern/pattern.rs` 将该类型分类为 `OperandTableScan`；`cascades/old/transformation_rules.rs` 可克隆扫描并追加下推后的访问条件。
- `cascades/old/implementation_rules.rs::ImplTableScan` 是直接物理化消费者，调用 `physical_table_scan.rs::GetPhysicalScan4LogicalTableScan` 并复制表、列和范围。

直接下游依赖：

- 本 crate 的 `LogicalSchemaProducer`、`BaseLogicalPlan`、`LogicalPlan`、`DataSourceRef`、`StatsInfo` 和 `PossiblePropertiesInfo` 提供逻辑计划骨架与共享类型。
- `expression::Expression` 及其 `StringWithCtx` 负责条件展示；`planner_util::HandleCols` 抽象句柄列。
- `ranger::BuildTableRange`、`ranger::FullIntRange` 和 `ranger::Range` 负责表键范围。
- `model::TableInfo` 与 `mysql::HasUnsignedFlag` 经来源判断整数主键是否无符号；`kv::StoreType` 记录目标存储。
- `base::ContextRef` 提供表达式求值上下文、ranger 上下文和会话 MPP 开关。

`Cargo.toml` 以工作区 path dependency 声明 `base`、`expression`、`kv`、`model`、`mysql`、`planner_util`、`ranger` 等依赖；没有本文件专属 feature 或条件编译。

## 错误处理与边界

- `ExplainInfo` 对缺失 `Source` 安全降级为固定文本；上下文缺失时仍能以无求值上下文方式格式化句柄和条件。
- `BuildKeyInfo` 对缺失来源直接返回，不修改当前 schema；正常路径会先让来源重建键信息，再覆盖本节点两类键集合。
- `DeriveStats` 对缺失来源使用默认统计，但范围仍按 `HandleCols` 分支处理。无句柄时可安全构造全整数范围。
- 有句柄时，缺失计划上下文返回 `PlannerError("LogicalTableScan has no plan context")`；第 0 个句柄列或其类型缺失返回 `PlannerError("table scan handle column has no type")`。
- `ranger::BuildTableRange` 的错误被转换为 `PlannerError(error.to_string())` 并向上传播；发生错误时不会执行末尾的 `SetStats`。
- 只有严格大于零的表路径 `CountAfterAccess` 才覆盖表统计行数，避免把未初始化的零估计当作确定空表；不过实现选择的是首个 `IsTablePath()` 路径，构造方需保证它对应当前表扫描语义。
- `PreparePossibleProperties` 在来源、上下文或 MPP 任一条件缺失时保守报告无 TiFlash；无句柄时返回空 `Orders`。
- `ImplTableScan::OnImplement` 比默认对象更严格：缺少来源会返回 `PlannerError("LogicalTableScan has no data source")`，因此默认对象不能完成物理化。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部资源。规划状态以普通拥有值和 `DataSourceRef` 管理；从用法看，来源通过 `borrow()`/`borrow_mut()` 进行单线程运行时借用检查，而非跨线程同步。

方法把来源借用限制在局部作用域：`ExplainInfo` 只读来源；`BuildKeyInfo` 先短暂可变借用调用来源推导，再只读借用并克隆键集合；`DeriveStats` 的来源借用只用于克隆统计、扫描路径和读取表元数据。这避免在写本节点 schema、Range 或统计缓存时保持冲突借用。

表达式、Range、统计与键集合在逻辑阶段以克隆传递，物理化也复制 Range 和元数据，因此后续计划不借用临时访问路径。相应性能成本主要是规划期向量/表达式克隆和线性路径查找，而不是运行期同步或 I/O。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_table_scan.go`；Go 的统计实现委托到 `pkg/planner/core/stats.go::deriveStats4LogicalTableScan`。两侧共同保留
`LogicalTableScan`、`Init`、`ExplainInfo`、`BuildKeyInfo`、`DeriveStats` 和
`PreparePossibleProperties` 的核心意图：表示表访问路径、展示条件、继承来源键、推导统计与 Range、报告句柄顺序和 TiFlash 能力。

当前可核验差异如下：

- Go 结构只有 `Source`、`HandleCols`、`AccessConds` 和 `Ranges`；Rust 还显式保存
  `TableFilters` 与 `StoreType`，且 `buildTableGather` 从 `AccessPath` 填充它们。
- Go `DeriveStats` 通过函数指针委托 `deriveStats4LogicalTableScan`，后者用
  `deriveStatsByFilter(Source, AccessConds, nil)`；Rust 在本文件内实现，默认克隆
  `TableStats`，并用表路径的 `CountAfterAccess` 覆盖行数、钳制 NDV。两者不是逐行等价的统计计算实现。
- 两侧都用访问条件构建句柄 Range，并在无句柄时依据无符号主键生成全整数范围；Rust 额外显式处理无上下文、无句柄列类型和 ranger 错误。
- Go `BuildKeyInfo` 直接把当前 schema 参数交给 `Source.BuildKeyInfo`；Rust 先让来源更新自己的 schema，再把来源的 `PKOrUK`、`NullableUK` 克隆到扫描 schema。
- 两侧 EXPLAIN 都使用 `Source.HandleCols`，而非扫描结构的 `HandleCols`。Rust 对来源缺失提供降级文本，Go 正常路径假定来源和上下文存在。
- Go 将 `hasTiFlash` 缓存在嵌入的基础计划字段；Rust 返回 `PossiblePropertiesInfo` 中的值，不在本节点额外保存该字段。

扩展或修复时应以这些当前事实为准，并核对 Go 行为意图；不能为了表面字段一致而删除 Rust 构造链已经传递的状态。

## 扩展指南

- 新增或改变访问路径字段时，应同步检查 `DataSource::buildTableGather` 是否填充、扫描变换克隆是否保留，以及物理化规则是否消费；只给 `LogicalTableScan` 加字段不会自动影响执行。
- 修改统计推导时，应重点对照 Go 的 `deriveStats4LogicalTableScan`，明确选择性来源、缓存语义和零行数语义；独立测试至少覆盖缓存命中/强制 reload、路径行数、NDV 钳制、ranger 错误和无来源边界。
- 修改 Range 逻辑时，应保持句柄第 0 列类型与 `AccessConds` 的约束，并覆盖有符号/无符号整数全范围、缺失类型和复合句柄场景。不要把测试写进生产文件，应扩展同目录 `logical_table_scan_test.rs`。
- 修改候选顺序时，应同步检查 `ImplTableScan::Match`：当前物理化规则只验证第一个句柄列，若要支持复合句柄排序，属性声明与物理匹配必须一起调整。
- 修改 TiFlash 判定时，应同时保留“副本可用”和“会话允许 MPP”两个条件，并检查 memo 属性传播；否则可能让优化器枚举不可执行的 MPP 计划。
- 修改 EXPLAIN 格式要注意其兼容性和日志脱敏语义；当前 Rust 使用求值上下文格式化表达式，但 Go 还显式传入 `EnableRedactLog`，两侧行为需要专项核验。
- 修改 `Source` 借用或可变状态时，应保持短借用作用域，避免 `RefCell` 运行时借用冲突。性能风险集中在条件/Range 克隆和路径线性查找，兼容风险集中在统计基数、顺序属性、Range 边界、MPP 可用性和 EXPLAIN 文本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和
  1,848,419 条边；`node --file pkg/planner/core/operator/logicalop/logical_table_scan.rs --offset 1 --limit 400` 读取到完整 206 行目标源码，并报告该文件被 32 个文件使用。
- 符号与图查询：`query LogicalTableScan --limit 20 --json` 同时确认 Rust 结构、Go 对照结构/方法和物理扫描工厂；对精确 Rust 符号执行 `callers`/`callees` 未返回方法级边，且 `files --filter` 未命中该路径，因此调用关系用图的文件使用摘要和局部源码搜索交叉核验，没有据此臆造调用者。
- 读过的生产与配置路径：`logical_table_scan.rs`、`logicalop/Cargo.toml`、
  `logicalop/lib.rs`、`logical_datasource.rs::buildTableGather`、
  `base_logical_plan.rs`、`cascades/memo/group_expr.rs`、
  `cascades/pattern/pattern.rs`、`cascades/old/transformation_rules.rs`、
  `cascades/old/implementation_rules.rs`、`physicalop/physical_table_scan.rs`、
  `logical_table_scan.go` 和 `core/stats.go::deriveStats4LogicalTableScan`。目标 Go 包及其父级未提供适用的 `doc.go` 包契约。
- 独立测试 `logical_table_scan_test.rs`：
  `explain_uses_source_handle_and_formats_access_conditions_like_go` 验证来源句柄优先及条件文本；
  `tiflash_property_requires_an_mpp_enabled_context_like_go` 验证仅有可用副本而无 MPP 上下文仍返回 false；
  `derive_stats_builds_a_full_integer_range_without_handle_columns` 验证表行数、重新计算标志和无句柄全整数范围。
- 当前独立测试没有覆盖有句柄的 `BuildTableRange`、错误传播、统计缓存、正数
  `CountAfterAccess`、NDV 钳制、`BuildKeyInfo` 或真实 MPP 上下文成功分支；这些是扩展时应补的测试范围，不能表述为已经验证。
- 本任务只新增说明文档，按任务约束未运行 Cargo。交付前使用任务指定的 shell 结构检查确认文件存在且固定二级标题恰为 11 个，并人工复核文档能回答文件定位、运行路径与安全扩展方式。
