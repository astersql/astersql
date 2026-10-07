# `pkg/planner/core/operator/logicalop/logical_index_scan.rs`

源码：[logical_index_scan.rs](./logical_index_scan.rs)

## 文件定位

本文件实现逻辑优化阶段的索引扫描叶子算子 `LogicalIndexScan`。它位于
`astersql-planner-core-operator-logicalop` crate；`lib.rs` 通过
`mod logical_index_scan` 注册模块并以 `pub use logical_index_scan::*` 对外导出。
crate 的 Go 对照包由 `Cargo.toml` 的 `package.metadata.porting.go-package` 指向
`pkg/planner/core/operator/logicalop`。

它不是直接执行存储读取的执行器，而是把 `DataSource` 已选择的索引访问路径固化为逻辑计划节点。
`DataSource::buildIndexGather` 创建该节点并把它挂到 `TiKVSingleGather` 之下；随后传统物理计划枚举
`ExhaustPhysicalPlans` 或旧 Cascades 规则 `ImplIndexScan` 将其转换成物理索引扫描。

## 核心职责

- 保存索引元数据、输出列、索引列布局、访问条件、索引残余过滤条件、范围和双读标记，作为逻辑层到物理层的传递载体（`LogicalIndexScan` 字段）。
- 生成 EXPLAIN 的数据源、索引列和访问条件文本（`ExplainInfo`）。
- 从数据源的全部可用索引及整数主键句柄重建输出 schema 的候选键（`BuildKeyInfo`）。
- 从匹配索引访问路径取得访问后行数，限制列 NDV，并缓存统计信息（`DeriveStats`）。
- 描述索引能够提供的有序性，并判断一个具体物理排序属性是否可由该索引满足（`PreparePossibleProperties`、`MatchIndexProp`）。
- 通过 `LogicalPlan` trait 暴露基础计划、解释、键信息和统计推导接口，使通用优化流程无需依赖具体类型。

本文件不负责条件拆分、Range 构建或真正的 KV 扫描：前两者主要发生在
`logical_datasource.rs`，物理扫描构造及 Range 上限回退位于
`physicalop/base_physical_plan.rs`。

## 主要符号

- `LogicalIndexScan`：核心状态对象。`LogicalSchemaProducer` 承载基础逻辑计划、schema、输出名和统计缓存；`Source` 指回共享的 `DataSourceRef`。
- `Default::default`：构造无上下文、无数据源、无条件和无范围的空节点。空对象主要供分阶段填充及测试使用，不能等同于可物理化扫描。
- `Init(ctx, offset)`：用算子名 `IndexScan` 初始化 `BaseLogicalPlan`，保留查询块偏移。
- `ExplainInfo()`：若无 `Source` 返回保守文本 `index scan`；否则追加索引列和访问条件。隐藏索引列显示生成表达式，普通列显示索引列名。
- `BuildKeyInfo()`：遍历 `Source.AllPossibleAccessPaths`，跳过表路径，以
  `rule_util::CheckIndexCanBeKey` 区分强唯一键和可空唯一键；若表使用整数主键句柄，再将句柄列加入 `Schema.PKOrUK`。
- `DeriveStats(reload)`：在无需重载且已有缓存时返回 `(cached, false)`；否则从表统计起步，以当前索引 ID 对应路径的正数 `CountAfterAccess` 覆盖行数，将全部列 NDV 钳制到行数以内，保存后返回 `(stats, true)`。
- `PreparePossibleProperties()`：对 `IdxCols` 生成从偏移 `0` 到等值前缀末端的索引后缀顺序；同时仅在数据源有 TiFlash 且会话允许 MPP 时设置 `HasTiFlash`。
- `MatchIndexProp(prop)`：空排序总是匹配；混合升降序拒绝；其余情况允许越过常量索引列和至多 `EqCondCount` 个普通等值前缀，再校验完整排序序列。
- `GetPKIsHandleCol(schema)`：仅在 `TableInfo.PKIsHandle` 为真时，从本节点当前 `Columns` 找主键标志，再按列 ID 回查当前 schema。这样能适配列裁剪后本节点与原始 `DataSource` 不同的列集合。
- `matchIndicesProp` / `matchIndicesPropWithCtx`：校验索引列是否逐项覆盖排序项；所有参与排序的索引列都必须是全长列，前缀索引不能保证完整顺序。
- `matchIndicesPropWithConstCols`：在上述校验上跳过 `ConstCols` 标记的列，供 `MatchIndexProp` 使用。
- `impl LogicalPlan for LogicalIndexScan`：提供 `Any` 下转型、基础计划访问和三个行为方法的动态分派入口。

## 执行流程

1. `DataSource` 在访问路径分析中拆分 `AccessConds`、`IndexFilters` 与
   `TableFilters`，计算 Range、索引列长度、等值条件数量和常量列标记。
2. `DataSource::buildIndexGather` 要求路径带有 `Index`，克隆上下文、schema、输出名和列元数据；它用 `IsSingleScan` 判断覆盖索引扫描，并将结果反转为
   `LogicalIndexScan.IsDoubleRead`。
3. 构造函数把 `AccessPath` 中的 `FullIdxCols`、`IdxCols`、列长、`ConstCols`、
   `NoncacheableReason`、条件和 Range 完整复制到扫描节点，再把节点设置为
   `TiKVSingleGather` 的孩子。
4. 优化公共流程可通过 `LogicalPlan` 调用 `BuildKeyInfo` 和 `DeriveStats`；属性准备阶段调用 `PreparePossibleProperties` 枚举索引后缀顺序。
5. 物理化时，传统枚举逻辑在 `physicalop/base_physical_plan.rs` 下转型为
   `LogicalIndexScan`，先以 `MatchIndexProp` 检查排序要求，再复制统计、条件、索引过滤、Range 与不可缓存原因到 `PhysicalIndexScan`。Range 内存上限触发时，该层可能重建较粗 Range 并把丢失的可覆盖访问条件补到索引过滤。
6. 旧 Cascades 流程的 `ImplIndexScan::Match` 同样调用 `MatchIndexProp`；
   `OnImplement` 则复制表、索引、列、Range 和顺序要求，生成物理实现。

排序匹配的关键路径是：找到第一个与首个 `SortItem` 相等的非恒定索引列，然后从该位置逐项匹配；在找到前，只允许经过等值前缀。匹配过程要求每个列长为
`UnspecifiedLength`，因此前缀索引不会被误认为能提供完整值顺序。

## 数据与状态

- 来源与结构：`Source`、`Index`、`Columns` 将扫描绑定到表及索引元数据；
  `LogicalSchemaProducer` 保存实际输出 schema。`Index` 使用值类型且默认可为空元数据，真正构造入口通过 `path.Index.as_ref()?` 保证有效索引。
- 访问路径：`FullIdxCols`/`FullIdxColLens` 表示完整索引布局；
  `IdxCols`/`IdxColLens` 表示当前可用的索引列布局；`EqCondCount` 和
  `ConstCols` 决定可跳过的有序前缀。
- 谓词与范围：`AccessConds` 是参与定界的条件，`IndexFilters` 是能在索引侧执行但未完全转成范围的条件，`Ranges` 是具体索引范围。表侧过滤保留在 gather/data source，不存于本结构。
- 执行特征：`IsDoubleRead` 指示索引后是否需要回表；`NoncacheableReason` 向物理计划传递不可使用计划缓存的原因。
- 派生状态：统计信息和 schema 键集合实际存放在 `LogicalSchemaProducer`。`DeriveStats` 会写统计缓存；`BuildKeyInfo` 会覆盖 `PKOrUK` 和 `NullableUK`，而不是在旧值上累加。

需要保持的长度不变量是 `IdxCols`、`IdxColLens` 与 `ConstCols` 对同一索引位置有一致语义。
辅助函数对短切片采用返回 `false` 或安全的 `get`，但错误对齐会使合法排序被拒绝，属于构造方必须避免的状态。

## 依赖与调用关系

上游直接证据：

- `logical_datasource.rs::DataSource::buildIndexGather` 是正常构造入口，并由
  `Convert2Gathers` 对非表访问路径调用。
- `base_logical_plan.rs` 的通用逻辑计划流程通过 `LogicalPlan` trait 调用键信息和统计推导。
- `cascades/old/implementation_rules.rs::ImplIndexScan` 读取该节点并生成旧 Cascades 物理实现。
- `physicalop/base_physical_plan.rs::ExhaustPhysicalPlans` 下转型该节点，检查排序并生成 `PhysicalIndexScan`；同文件的查找扫描、假想索引判断和计划复杂度统计也识别该类型。

下游依赖：

- `base`/本 crate 的 `LogicalSchemaProducer`、`BaseLogicalPlan`、`ContextRef` 和
  `LogicalPlan` 提供计划骨架。
- `planner_util::AccessPath`（经构造入口）提供本结构的大部分访问路径状态。
- `rule_util::CheckIndexCanBeKey` 判定唯一索引能否成为强键或可空键。
- `property::PhysicalProperty`、`SortItem` 表达所需顺序；`Column::EqualByExprAndID`
  在存在求值上下文时同时考虑表达式与列标识。
- `model`、`mysql`、`ranger`、`expression` 分别提供索引/列元数据、主键标志、Range 与表达式语义。

`Cargo.toml` 将这些依赖声明为工作区内 path crate，没有本文件专属 feature 或条件编译；测试模块仅由 `lib.rs` 的 `#[cfg(test)] mod logical_index_scan_test` 启用。

## 错误处理与边界

- `ExplainInfo` 在 `Source` 缺失时降级返回固定文本；索引列偏移越界时用
  `filter_map` 跳过该列，不 panic。
- `BuildKeyInfo` 在无 `Source` 时无操作；表路径不会参与索引唯一键推导。
- `DeriveStats` 的返回类型是 `Result`，当前实现自身没有产生 `Err` 的分支；无
  `Source` 时使用默认统计。只有正数 `CountAfterAccess` 才覆盖行数，避免把未初始化的零估计当成真实结果。
- `PreparePossibleProperties` 对空 `IdxCols` 返回空顺序，避免 `len() - 1` 下溢；
  `EqCondCount` 通过 `min` 限制在索引列尾部以内。
- `MatchIndexProp` 拒绝混合方向排序、索引列不足、列不相等和任一前缀索引列；空排序无需索引有序性即可满足。
- `GetPKIsHandleCol` 对无 Source、非整数句柄表、没有带主键标志的本地列或 schema
  中找不到相同列 ID 均返回 `None`。
- 真正物理化比本文件更严格：传统流程缺少上下文或 Source 时返回空候选；旧 Cascades
  `OnImplement` 缺 Source 时显式返回 `PlannerError("LogicalIndexScan has no data source")`。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部资源。`Source` 是
`Rc<RefCell<DataSource>>` 风格的 `DataSourceRef`：它属于单线程计划树共享状态，方法用短生命周期
`borrow()` 读取，并在修改本节点 schema 或统计前结束或避免冲突借用。

表达式、列、Range 和元数据在从访问路径进入扫描、再进入物理扫描时以克隆传递；因此物理化不会借用临时访问路径。代价是规划阶段有向量和表达式克隆开销。`SCtx` 由基础计划持有，本文件只临时读取求值上下文和会话变量，不拥有会话资源。统计缓存随逻辑节点生命周期存在，`reload` 控制是否复用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_index_scan.go`。两侧共同保留
`LogicalIndexScan`、`Init`、`ExplainInfo`、`BuildKeyInfo`、`DeriveStats`、
`PreparePossibleProperties`、`MatchIndexProp`、`GetPKIsHandleCol` 和
`matchIndicesProp` 的核心意图：索引扫描状态、唯一键推导、统计、索引顺序匹配及整数句柄主键回查。

当前 Rust 实现并非逐行机械翻译，已存在可核验差异：

- Go 的 `DeriveStats` 委托 `utilfuncp.DeriveStats4LogicalIndexScan`；Rust 在本文件内实现缓存复用、按索引 ID 查找 `CountAfterAccess`、NDV 钳制和缓存写回。
- Rust 结构显式保存 `IndexFilters`、`ConstCols` 与 `NoncacheableReason`，构造入口也从
  `AccessPath` 复制这些字段；当前 Go 对照结构没有这三个显式字段。
- Rust `MatchIndexProp` 可跳过 `ConstCols`；当前 Go 版本只允许跳过普通等值前缀。
- Rust `PreparePossibleProperties` 以 `min` 防御过大的 `EqCondCount`；Go 直接按
  `0..EqCondCount` 切片，依赖上游不变量。
- Rust 的 `Index` 为值类型且 `Source` 为 `Option`，因此提供空节点降级路径；Go 使用指针并在正常调用中假定字段已经装配。
- Go `BuildKeyInfo` 显式清空 `PKOrUK`，Rust 同时用新向量覆盖 `PKOrUK` 和
  `NullableUK`，避免可空键残留。

这些差异应按当前代码事实维护；不能为了表面一致而删除 Rust 已被物理计划消费的字段或防御分支。

## 扩展指南

- 新增访问路径字段时，首先确认其由 `logical_datasource.rs::buildIndexGather` 填充，并在
  `physicalop/base_physical_plan.rs` 和 Cascades `ImplIndexScan::OnImplement` 中按需要传递；否则字段只存在于逻辑节点而不影响执行。
- 修改排序匹配时，应同步审查 `PreparePossibleProperties`、`MatchIndexProp`、两个内部匹配辅助函数及 `ImplIndexScan::Match`。重点覆盖空排序、混合方向、等值前缀、常量列、前缀索引、列数不足和表达式等价列。
- 修改键推导时，应保持使用本节点 `Columns` 与当前 schema，而非直接复用 DataSource
  的未裁剪列；同步扩展独立测试 `logical_index_scan_test.rs`，测试代码不要嵌入生产文件。
- 修改统计推导时，应覆盖缓存命中与 `reload`、找不到索引路径、非正
  `CountAfterAccess`、NDV 大于行数等边界，并核对 Go 委托实现的预期语义。
- 增加 EXPLAIN 内容时，要维持隐藏列使用生成表达式、普通列使用索引名的兼容格式，并检查参数化表达式的求值上下文。
- 性能风险主要在规划阶段的表达式/Range 克隆和线性路径、列扫描；兼容风险集中在
  EXPLAIN 文本、计划缓存标记、排序可满足性和唯一键推导，这些都会影响计划选择而不只是展示。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；对目标文件执行
  `node --file ... --offset 1 --limit 500`，读到完整 323 行源码，并报告直接使用文件为
  `logical_datasource.rs`、`physicalop/base_physical_plan.rs`、
  `cascades/old/implementation_rules.rs` 和 `optimizer_runtime.rs`。
- 主要符号查询：`query LogicalIndexScan --kind struct`、
  `query matchIndicesProp --kind function`、`query MatchIndexProp --kind method`，确认 Rust/Go 对照符号和辅助函数位置。方法级 `callers/callees` 查询未输出边，因此调用关系由上述文件索引结果与直接源码交叉核验，没有据此臆造额外调用者。
- 读过的生产与配置文件：`logical_index_scan.rs`、`logical_datasource.rs`、
  `logicalop/lib.rs`、`logicalop/Cargo.toml`、
  `physicalop/base_physical_plan.rs`、`cascades/old/implementation_rules.rs`、
  `logical_index_scan.go`，以及包契约 `pkg/planner/core/base/doc.go`。
- 测试证据：`logical_index_scan_test.rs::build_key_info_rebuilds_keys_from_all_index_paths_and_handle`
  验证唯一索引键和整数句柄键同时进入 `PKOrUK`；
  `cascades/old/optimize_test.rs::test_prepare_possible_properties` 通过包含
  `LogicalIndexScan` 的计划验证索引顺序传播。当前独立测试没有逐项覆盖本文件的 EXPLAIN、统计和所有排序拒绝分支，属于后续扩展时应补的覆盖面，而非已验证事实。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付前使用任务文件指定的命令校验文档存在且固定二级标题恰为 11 个，并人工复核所有行为描述均可回指上述符号或直接调用文件。
