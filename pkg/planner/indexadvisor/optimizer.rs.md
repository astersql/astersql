# `pkg/planner/indexadvisor/optimizer.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-indexadvisor`。crate 由同目录 [`lib.rs`](lib.rs) 装配，并通过公开模块 `optimizer` 暴露本文件中的 what-if 优化器抽象；[`Cargo.toml`](Cargo.toml) 的 `[lib]` 将 crate 根指定为 `lib.rs`，移植元数据把它对应到 Go 包 `pkg/planner/indexadvisor`。

它位于索引顾问的“算法与真实规划器/元数据之间”这一边界：上层候选枚举、列收集和推荐结果整理只依赖 `Optimizer` trait，不直接访问表元数据或 planner。当前可执行 Rust 实现是 `InMemoryOptimizer`，以调用方提供的 `TableMetadata` 和代价回调模拟该边界，主要服务算法联调和独立测试；它不是 Go `optimizerImpl` 的 session-backed 生产适配器。文件第 22—84 行的大段内容均为注释中的迁移设计草案，不参与编译，不能视为已经接线的能力。

## 核心职责

- 用 `Optimizer` 统一七类索引顾问能力：查询列类型、判断候选索引是否已被已有索引前缀覆盖、按 schema 消歧同名列、枚举表列、检查索引名、估算索引大小，以及在一组假设索引下计算 SQL 计划代价。
- 用 `FieldType` 提供索引顾问所需的简化类型分类；[`utils.rs`](utils.rs) 的 `is_indexable_column_type` 据此排除 `Json`、`Blob`、`Geometry` 和 `Vector`。
- 用 `TableMetadata` 保存内存表目录和统计摘要，用 `InMemoryOptimizer` 提供确定性、无外部 I/O 的 trait 实现。
- 在 `query_plan_cost` 调用代价回调前验证每个假设索引引用的表和列确实存在，防止测试桩绕过最基本的元数据约束。
- 用 `is_system_schema` 阻止索引顾问从系统库和内存库收集候选列。

## 主要符号

- `FieldType`：公开枚举，覆盖整数、浮点、定点数、日期时间、字符串、字节，以及四类不可索引的复杂/大对象类型。它是顾问层分类，不是 Go `types.FieldType` 的完整等价物。
- `Optimizer`：公开 trait。所有方法都通过 `Result<_, String>` 传播失败；接收者均为 `&self`，但 trait 本身没有声明 `Send`、`Sync` 或线程安全保证。
- `TableMetadata`：公开、可克隆的表快照，包含小写列名到 `FieldType` 的映射、已有索引列表、实时行数和按列统计的总字节数。
- `CostHook`：私有 trait-object 类型别名，签名为 `Fn(&str, &[Index]) -> Result<f64, String> + Send + Sync`。
- `InMemoryOptimizer`：公开、可克隆的实现；`tables` 以 `(schema, table)` 为键，`cost_hook` 由 `Arc` 共享。
- `InMemoryOptimizer::new`：注入整个内存目录与代价回调。构造器不规范化传入映射，调用方必须按小写键组织表和列。
- `InMemoryOptimizer::table`：私有统一查表入口；只对查询参数做小写化，查不到时返回 `table <schema>.<table> not found`。
- `Optimizer for InMemoryOptimizer`：七个接口方法的实际实现。
- `is_system_schema`：私有纯函数，不区分输入大小写，识别 `mysql`、`information_schema`、`performance_schema`、`metrics_schema` 和 `sys`。

## 执行流程

典型索引推荐链路如下：

1. [`indexadvisor.rs`](indexadvisor.rs) 的 `advise_indexes_for_sql` 规范化查询、恢复 schema 并过滤系统查询，然后调用 [`utils.rs`](utils.rs) 的 `collect_indexable_columns`。
2. `collect_indexable_columns` 从谓词、排序和分组位置抽取列名；对每个相关 schema 调用 `possible_columns` 消歧，再用 `column_type` 和 `is_indexable_column_type` 排除不适合建索引的列。这里对单次元数据查询错误采用跳过候选的策略。
3. [`algorithm.rs`](algorithm.rs) 的 `advise_indexes` 从单列候选开始迭代。`usable_candidates` 调用 `prefix_contain_index`，滤掉已经被现有索引前缀覆盖的候选。
4. 算法通过 `evaluate_index_set_cost` 对每个查询调用 `query_plan_cost`，按查询频次累计 workload 代价，并据此选择索引集合。
5. `prepare_recommendations` 再调用 `estimate_index_size`、`query_plan_cost` 和 `index_name_exists`，生成大小、改善比例、受影响查询和无冲突名称。

`InMemoryOptimizer` 的方法内部流程是：

- 元数据类操作先经 `table` 使用小写 schema/table 定位 `TableMetadata`。
- `column_type` 将请求列名小写化后查映射；`table_columns` 按 `BTreeMap` 键序生成列，返回的 schema/table 字符串保持调用参数原样。
- `possible_columns` 先排除系统 schema，再遍历同一小写 schema 下全部表；列名参数不会自动小写化，因此匹配依赖调用方传入规范化名称。
- `prefix_contain_index` 只接受“候选列序列等于已有索引起始列序列”的情况；已有索引必须至少与候选一样宽。
- `estimate_index_size` 对各列累加已加载的总字节数；某列无统计时回退为 `row_count.saturating_mul(8)`，饱和乘法避免 `u64` 溢出。
- `query_plan_cost` 逐一验证假设索引中的表和列，再把原 SQL 与索引切片原样交给 `cost_hook`；本文件不解析 SQL，也不自行比较代价。

## 数据与状态

`InMemoryOptimizer` 的持久状态只有两个字段：不可变的 `tables` 和共享的 `cost_hook`。所有 trait 方法只借用 `&self`，不会添加、删除或恢复假设索引，也不会缓存查询结果。克隆优化器时，`BTreeMap` 元数据被深拷贝，而回调的 `Arc` 只增加引用计数。

大小估算中的 `column_total_size` 是每列总存储字节数，`row_count` 仅在该列统计缺失时生效；多个缺统计列会各自贡献 `row_count * 8`。`BTreeMap` 还使表列枚举和表扫描顺序稳定，便于索引顾问产生可重复结果。

名称规范化并非全局自动完成。查表统一小写 schema/table；`column_type`、`prefix_contain_index` 和 `query_plan_cost` 验证会将请求列名小写化；`possible_columns`、`estimate_index_size` 的统计键和 `index_name_exists` 则按传入字符串直接匹配。该差异是当前行为的一部分，独立 Rust 测试明确覆盖了大小写边界。

## 依赖与调用关系

直接源码依赖很小：本文件只导入 [`model.rs`](model.rs) 的 `Column`、`Index`，以及标准库的 `BTreeMap`、`Arc`。虽然 [`Cargo.toml`](Cargo.toml) 在 Windows 目标下列出了 domain、infoschema、sessionctx、parser、types 等 Go 移植所需 crate，当前可执行实现没有引用它们；这些依赖与文件顶部的注释草案表明生产 session 适配仍是预留方向，而不是当前调用链。

已核对的上游调用边包括：

- [`utils.rs`](utils.rs)：`filter_invalid_queries`、`collect_indexable_columns`、`evaluate_index_set_cost` 调用代价、列解析和类型接口。
- [`algorithm.rs`](algorithm.rs)：`usable_candidates` 调用 `prefix_contain_index`；整个算法以 `&dyn Optimizer` 接收实现。
- [`indexadvisor.rs`](indexadvisor.rs)：`prepare_recommendations` 调用大小和代价接口，`graceful_index_name` 间接调用名称检查。
- 独立测试和 SQL/TPC-H 场景测试构造 `InMemoryOptimizer`，将其转为 `&dyn Optimizer` 后驱动完整顾问流程。

RustCodeGraph 能定位 `InMemoryOptimizer`、trait 方法和文件使用者，但当前索引未解析 trait-object 方法的具体 caller/callee 边；因此上述动态调用关系由 RustCodeGraph 的文件/符号结果与调用点搜索共同核验。

## 错误处理与边界

- 所有接口使用字符串错误，没有结构化错误类型或上下文链。找不到表由 `table` 报错；找不到列由 `column_type` 或 `query_plan_cost` 报错；回调错误原样返回。
- `prefix_contain_index` 比较列顺序而非集合，符合 B-tree 索引最左前缀语义；空列候选会因 `zip` 的全称判断而被任意已有索引匹配，但正常候选由 `Index::with_columns` 等上游构造约束，扩展时仍应显式考虑空索引输入。
- `possible_columns` 对系统 schema 返回空集合，而不是错误。它直接用传入列名查询 `TableMetadata.columns`，所以大写列名不会命中小写目录。
- `index_name_exists` 对索引名直接做字符串相等比较。调用方 `indexadvisor.rs::index_name_exists` 会先转为小写，但直接调用 trait 方法不会替调用方规范化。
- `estimate_index_size` 不验证列是否存在于 `columns`；缺少统计键（包括大小写不一致或未知列）都会使用 `row_count * 8` 回退。这与当前 Go 版按 `ColumnByName` 缺失统计回退的形状一致，但也意味着该方法不能独立承担列存在性校验。
- `query_plan_cost` 只验证元数据后调用 hook；SQL 语法、假设索引是否真正改变计划、代价是否有限或非负，都由 hook 负责。本实现也不模拟 Go 版 session 状态的设置与恢复。

## 并发与资源生命周期

文件不创建线程、异步任务、通道、锁、事务、连接或外部 I/O。`tables` 在构造后只读，回调要求 `Send + Sync` 并由 `Arc` 管理生命周期；因此具体 `InMemoryOptimizer` 的字段适合安全共享，但 `Optimizer` trait 没有把 `Send + Sync` 写入接口契约，其他实现不能据此被假定为线程安全。Go 接口注释也明确说明优化器不是线程安全对象，上层算法当前以串行 `&dyn Optimizer` 调用它。

资源释放依赖 Rust 所有权：最后一个优化器/回调 `Arc` 被丢弃后回调释放，克隆的元数据随各实例分别释放。与 Go `QueryPlanCost` 不同，Rust 内存实现没有需要以 `defer`/RAII 恢复的 fix-control、warning、`HypoIndexes` 或 explain 状态。

## 与 Go 版本的对应关系

[`optimizer.go`](optimizer.go) 是直接 Go 对照文件。Rust `Optimizer` 的七个方法逐项对应 Go 接口的 `ColumnType`、`PrefixContainIndex`、`PossibleColumns`、`TableColumns`、`IndexNameExist`、`EstIndexSize`、`QueryPlanCost`，前缀判断、系统库过滤和缺列统计按每列 `8 * RealtimeCount` 回退等核心规则保持一致。

两者的关键实现差异是：

- Go `NewOptimizer` 持有真实 `sessionctx.Context`，从最新 `InfoSchema` 和 domain stats 读取数据；Rust 只有调用方构造的 `TableMetadata` 快照。
- Go `QueryPlanCost` 解析 SQL，临时打开 fix 43817 与 explain 标志，清空并安装 `IndexTypeHypo` 假设索引，调用全局 `QueryPlanCostHook`，最后恢复 fix-control、warnings、额外 warnings、假设索引和 explain 状态；Rust 只做表列校验并调用注入的纯回调。
- Go 返回完整 `types.FieldType`，Rust 返回顾问层简化 `FieldType`；生产适配器未来需要定义完整类型到简化分类的可靠映射。
- Go 从 `InfoSchema` 获得规范化 `CIStr` 名称；Rust 依赖调用方用小写键准备内存目录，因此部分方法存在显式的大小写敏感边界。
- Go 接口明确非线程安全，因为它临时修改 session；Rust 内存实现不修改 session，但 trait 仍未承诺线程安全。

[`optimizer_test.go`](optimizer_test.go) 以 mock store 覆盖真实表类型、索引最左前缀、多表同名列、表列枚举、名称检查、统计大小和假设索引降低计划代价。Rust [`optimizer_test.rs`](optimizer_test.rs) 覆盖内存实现的对应元数据契约与大小写/错误边界，但真实 SQL 解析、session 状态恢复和 planner 假设索引效果仍只由 Go 测试验证。

## 扩展指南

- 接入生产 Rust planner 时，应新增独立的 session-backed `Optimizer` 实现，而不是把 session 逻辑塞入 `InMemoryOptimizer`。实现必须复刻 Go `QueryPlanCost` 的状态快照与无条件恢复，并为解析错误、缺表/缺列、hook 错误及恢复后的状态增加独立测试文件中的回归用例。
- 扩展字段类型时，同步修改 `FieldType`、[`utils.rs`](utils.rs) 的 `is_indexable_column_type` 以及相关 `utils_test.rs`/`optimizer_test.rs`；不要假设新增类型默认可索引。
- 修改名称或大小写规则时，同时检查 `table`、`possible_columns`、`index_name_exists`、统计键查找，以及 [`indexadvisor.rs`](indexadvisor.rs) 中调用前的小写化。现有测试刻意固定了这些不对称行为。
- 修改前缀覆盖规则时，以 [`algorithm.rs`](algorithm.rs) 的候选过滤链路和 Go `TestOptimizerPrefixContainIndex` 为兼容基准，并在独立 Rust 测试中补齐列序、宽度和大小写场景。
- 修改大小估算时，需要说明缺统计回退、溢出语义和单位；同步更新 `optimizer_test.rs` 及 Go 统计测试所表达的意图。
- 为 trait 增加方法会影响 `InMemoryOptimizer`、测试中的 `SizeErrorOptimizer` 等所有实现；应先搜索 `impl Optimizer for`，再更新独立测试，不要把测试逻辑内嵌到 `optimizer.rs`。
- 代价回调处于候选组合的高频热路径。新增克隆、解析或 I/O 前应评估算法枚举放大效应，并保留确定性错误传播。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/planner/indexadvisor` 确认目标、Go 对照和相关测试；`node --file pkg/planner/indexadvisor/optimizer.rs` 阅读完整 268 行并确认文件使用者；`query InMemoryOptimizer --kind struct`、`query query_plan_cost --kind method`、`query possible_columns --kind method`、`query prefix_contain_index --kind method` 核对主要符号。trait-object caller/callee 查询未返回可用边，因此另以调用点搜索补齐动态调用证据。
- Rust 源与 crate 边界：[`optimizer.rs`](optimizer.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`model.rs`](model.rs)。
- 直接 Rust 调用者：[`algorithm.rs`](algorithm.rs)、[`utils.rs`](utils.rs)、[`indexadvisor.rs`](indexadvisor.rs)。
- 独立 Rust 测试：[`optimizer_test.rs`](optimizer_test.rs)；补充集成语义可见 `algorithm_test.rs`、`utils_test.rs`、`indexadvisor_test.rs`、`indexadvisor_sql_test.rs` 和 `indexadvisor_tpch_test.rs`。
- Go 对照与测试：[`optimizer.go`](optimizer.go)、[`optimizer_test.go`](optimizer_test.go)。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务规定的 11 章节结构检查，并人工复核当前实现与注释草案、Rust 与 Go 能力边界没有混写。
