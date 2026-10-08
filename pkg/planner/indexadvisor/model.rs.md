# `pkg/planner/indexadvisor/model.rs`

## 文件定位

本文件是 `astersql-planner-indexadvisor` crate 的纯数据模型层。crate 根 `pkg/planner/indexadvisor/lib.rs` 通过 `pub mod model` 暴露它；`pkg/planner/indexadvisor/Cargo.toml` 指定入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/planner/indexadvisor"` 标明对应的 Go 包。文件本身不解析 SQL、不访问元数据，也不执行 what-if 优化，而是为这些流程提供查询、列、索引、索引集合代价及最终推荐结果的统一表示。

在完整推荐链中，上游入口 `indexadvisor.rs::advise_indexes_for_sql` 先把 SQL 归并为 `Query`，工具层收集 `Column`，算法层组合并比较 `Index`/`IndexSetCost`，最后 `indexadvisor.rs::prepare_recommendations` 组装 `Recommendation`。因此这里既是算法的值对象边界，也是对外推荐结果的 DTO 边界。

## 核心职责

- 通过私有函数 `go_lowercase` 和 `Column::new`、`Index::new` 统一规范化 schema、表、列与索引名称，降低大小写差异对集合查找和键生成的影响。
- 以 `Query` 表示带频率的 workload 项，以 `Column` 和 `Index` 表示候选索引空间。
- 通过 `Column::key`、`Index::key` 与 `Index::prefix_contains` 提供确定性身份字符串和复合索引左前缀判断。
- 通过 `IndexSetCost::less` 实现“显著代价差、总列数、稳定键”三级择优规则。
- 通过 `ImpactedQuery`、`WorkloadImpact`、`IndexDetail`、`Recommendation` 承载最终推荐的解释、收益和大小信息。

本文件没有数据库 I/O、全局注册、缓存或后台任务；所有行为都是对传入值的同步构造、格式化或比较。

## 主要符号

- `fn go_lowercase(value: &str) -> String`：逐个 Unicode 标量调用 `to_lowercase`，每个输入字符只取映射结果的第一个标量。它是私有规范化基础函数；`model_test.rs::constructors_use_unicode_lowercase_like_go` 用 `İDB`、`ÉCLAIR`、`ÉIDX` 验证当前移植目标。
- `pub struct Query`：字段为 `alias`、`schema_name`、`text`、`frequency: usize`。`Query::key(&self) -> &str` 返回 SQL 文本；结构体同时派生 `Eq`、`Ord`、`Hash`，供 Rust 集合直接使用。
- `pub struct Column`：由 `schema_name`、`table_name`、`column_name` 三元组标识。`Column::new` 规范化三段名称，`Column::key` 生成 `schema.table.column`。
- `pub struct Index`：包含规范化的库、表、索引名和有序 `columns`。`Index::new` 从列名迭代器构造；`Index::with_columns` 以第一列的库表身份重建所有列；`Index::key` 生成 `schema.table(col1,col2,...)`；`Index::prefix_contains` 判断另一个索引是否是同库同表的左前缀。
- `pub struct IndexSetCost`：保存频率加权总成本、索引总列数和稳定键串。`less` 不是 `Ord` 实现，而是算法显式调用的领域比较函数。
- `ImpactedQuery`、`WorkloadImpact`、`IndexDetail`：分别表示单查询改善比例、workload 总体改善比例、推荐原因及预估字节数。
- `Recommendation`：最终结果，含库表、索引名、列序列、可选明细、可选总体影响及受益最大的查询列表。

所有上述结构体和公开方法都是 `pub`；唯一内部符号是 `go_lowercase`。文件没有 feature gate 或条件编译项。

## 执行流程

1. `indexadvisor.rs::advise_indexes_for_sql` 对输入 SQL 做 normalize/digest 归并并累计次数，创建 `Query { alias, schema_name, text, frequency }`。
2. `utils.rs` 的列收集函数根据查询和优化器元数据创建 `Column::new`；名称在进入候选集合前被规范化。
3. `algorithm.rs::single_column_indexes`、`extend_indexes` 和 `append_permutations` 调用 `Index::with_columns` 生成单列及多列候选。`with_columns` 取首列库表身份，将所有输入列的列名重新交给 `Index::new`，所以输出列全部归属于首列的表。
4. 算法用 `Index::prefix_contains` 删除相互覆盖的候选或最终窄索引；例如 `algorithm.rs::cut_down` 会移除被同表更长索引左前缀覆盖的条目。
5. `utils.rs::evaluate_index_set_cost` 按 `Query::frequency` 加权查询计划成本，统计所有索引列数，并排序、拼接 `Index::key` 得到 `IndexSetCost`。算法在候选选择和贪心扩展中调用 `IndexSetCost::less`。
6. `indexadvisor.rs::prepare_recommendations` 分别评估无索引和单个推荐索引的成本，生成 `ImpactedQuery`，再填充 `IndexDetail`、`WorkloadImpact` 和 `Recommendation`；结果按库、表、索引名排序。

`IndexSetCost::less` 的具体顺序是：任一方总成本为零时把零视为“未初始化”；否则只有当绝对差大于 `10` 且相对差大于 `0.001` 时才直接比较成本；成本不够显著时优先总列数更少者；仍相同时按 `index_keys` 字典序决定，保证候选枚举结果稳定。

## 数据与状态

模型全部按值持有数据，没有内部可变性。`String`、`Vec` 和 `Option` 的所有权随结构体移动或通过 `Clone` 复制；不存在借用外部会话对象的生命周期参数。

关键不变量如下：

- 通过构造器创建的 `Column` 与 `Index` 名称均经过 `go_lowercase`；直接使用公开字段构造结构体可绕过这一不变量，调用方需自行保持规范化。
- `Index.columns` 的顺序有语义：它既影响 `Index::key`，也决定 `prefix_contains` 的左前缀判断。
- `Index::with_columns` 要求至少一列，并刻意以第一列的 schema/table 覆盖后续列的来源身份；独立测试 `model_test.rs::with_columns_rebuilds_all_columns_from_the_first_table_like_go` 固化了这一行为。
- `IndexSetCost.total_workload_query_cost == 0.0` 是未初始化哨兵，不被视为真正的最优零成本。
- `Recommendation.index_detail` 与 `workload_impact` 可缺省；`top_impacted_queries` 是拥有元素的 `Vec<ImpactedQuery>`。

集合身份需特别注意：`Query`、`Column`、`Index` 派生的 Rust `Eq`/`Ord` 会比较全部字段，而 `Query::key` 只返回文本、`Index::key` 不含索引名。当前主入口用 digest `BTreeMap` 归并查询，算法用 `BTreeSet` 保存索引；扩展时不能误以为派生集合身份与 `key()` 完全等价。

## 依赖与调用关系

本文件只依赖 Rust 标准库预导入类型与方法，没有直接第三方 crate 依赖。`Cargo.toml` 中大量依赖仅在 Windows target 下声明，是整个 indexadvisor crate 其他模块的边界，不是 `model.rs` 的直接依赖。

已核对的直接消费关系：

- `algorithm.rs` 导入 `Column`、`Index`、`IndexSetCost`、`Query`；调用 `Index::with_columns`、`Index::prefix_contains` 和 `IndexSetCost::less` 完成候选生成、去重与择优。
- `utils.rs` 导入 `Column`、`Index`、`IndexSetCost`、`Query`；列收集路径调用 `Column::new`，`evaluate_index_set_cost` 调用 `Index::key` 并构造代价对象。
- `indexadvisor.rs` 导入 `ImpactedQuery`、`Index`、`IndexDetail`、`Query`、`Recommendation`、`WorkloadImpact`；它是查询模型的主要生产者和结果模型的主要生产者。
- `optimizer.rs` 的 `Optimizer` trait 在列元数据、已有索引前缀检查和假设索引成本接口中使用 `Column` 与 `Index`，构成模型与优化器边界。
- `lib.rs` 注册独立测试模块 `model_test.rs`、`indexadvisor_test.rs` 和 `optimizer_test.rs`，生产源码与测试逻辑保持分文件。

RustCodeGraph 显示 `model.rs` 已被索引，并报告它被多个 planner 文件和测试引用；由于 `Index`、`Query`、`less` 等名称在全仓库高度重名，精确 method 调用图未能消歧，以上直接边通过限定 `pkg/planner/indexadvisor` 的符号搜索和已索引源码逐项核实。

## 错误处理与边界

- `Index::with_columns` 是本文件唯一返回 `Result` 的 API。空列输入返回 `Err("index requires at least one column")`，避免对首元素直接索引。
- `go_lowercase` 对每个字符的 lowercase 迭代器调用 `expect`；标准库的字符小写映射保证至少产生一个字符，因此该断言表达内部不变量，而非面向用户的可恢复错误。
- `Index::new` 接受空列迭代器并生成空列索引；只有 `with_columns` 禁止空输入。业务算法目前通过非空前缀调用它，新增调用者若要求真实可建索引，应自行校验非空。
- `prefix_contains` 先检查库、表及长度，再逐列比较列名；它不比较索引名，也不重新规范化直接构造的字段。
- `IndexSetCost::less` 使用浮点精确零值、绝对差和相对差。`NaN`、负成本或无穷值没有单独验证或报错，行为由 Rust `f64` 比较规则决定；优化器实现应提供有限、非负成本。
- DTO 类型不校验改善比例范围、索引大小、空名称或空列；这些约束由 `prepare_recommendations` 和元数据/优化器层承担。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、线程、异步任务、事务、文件描述符或网络资源，也没有 `Drop` 实现。每次构造和比较均在调用线程同步完成。

各模型由拥有型字段组成，编译器可在字段类型允许时自动推导 `Send`/`Sync`；源码没有承诺特定并发协议。资源生命周期仅表现为普通所有权：构造器分配 `String`/`Vec`，克隆候选时复制其内容，离开作用域后自动释放。频繁候选枚举会放大字符串和列向量克隆成本，但本文件没有共享缓存来延长资源寿命。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/indexadvisor/model.go`：Rust 的八类公开结构与 Go 的 `Query`、`Column`、`Index`、`IndexSetCost`、`ImpactedQuery`、`WorkloadImpact`、`IndexDetail`、`Recommendation` 一一对应；构造器、键生成、前缀判断和三级代价比较也按相同顺序移植。

已确认的语言层差异：

- Go `Query.Frequency` 和 `IndexSetCost.TotalNumberOfIndexColumns` 为 `int`，Rust 使用 `usize`，因而 Rust 类型层面不允许负数。
- Go `NewIndexWithColumns` 对空切片访问 `columns[0]` 会 panic；Rust `Index::with_columns` 返回可恢复的 `Result::Err`。非空输入时，两者都以首列库表身份重建全部列。
- Go 的 `strings.ToLower` 与 Rust 字符迭代 API 不完全同构；私有 `go_lowercase` 采用“每个字符的小写映射取首字符”以满足当前 Go 对齐测试，相关 Unicode 行为由 `model_test.rs` 固化。
- Go 推荐结果用 `*IndexDetail`、`*WorkloadImpact`、`[]*ImpactedQuery` 表达可空指针；Rust 对前两者使用 `Option<T>`，对查询列表使用拥有值的 `Vec<T>`，列表元素本身不可为 null。
- Go 的泛型 set 可依据 `Key()` 去重，而 Rust 的 `BTreeSet` 依据派生 `Ord` 的全部字段。`Query::key` 和 `Index::key` 保留了 Go 风格稳定键，但不是 Rust 集合的比较器；这是扩展集合操作时必须显式评估的迁移差异。

Go 侧没有单独的 `model_test.go`；模型行为分散在 `algorithm.go`、`indexadvisor_test.go`、`optimizer_test.go` 和 `utils_test.go` 的调用与断言中。Rust 则将构造语义放在 `model_test.rs`，前缀与稳定键放在 `indexadvisor_test.rs::advisor_model_preserves_index_prefix_semantics`，成本比较放在 `optimizer_test.rs::optimizer_cost_order_matches_go_tie_breakers`。

## 扩展指南

- 新增或修改模型字段时，先同步检查 `model.go`、`indexadvisor.rs::advise_indexes_for_sql`、`prepare_recommendations`、`utils.rs::evaluate_index_set_cost` 及 `optimizer.rs::Optimizer` 的生产/消费边，避免只有 DTO 定义更新而业务链遗漏赋值。
- 修改名称规范化时，应集中调整 `go_lowercase`、`Column::new`、`Index::new`，并在独立文件 `model_test.rs` 增加 ASCII、Unicode 多标量映射和直接构造绕过规范化的边界用例。
- 修改索引身份、键格式或前缀语义时，应同步更新 `Index::key`、`prefix_contains`，以及 `indexadvisor_test.rs`、`algorithm_test.rs`、`utils_test.rs` 中的稳定顺序和候选去重断言。特别要决定索引名是否属于身份，并同时处理派生 `Ord` 与 `key()` 的差异。
- 修改代价择优阈值或哨兵语义时，应修改 `IndexSetCost::less`，并扩充 `optimizer_test.rs` 覆盖零值、绝对差边界 `10`、相对差边界 `0.001`、列数平局、键字符串平局及非有限浮点输入。
- 若 `Index::with_columns` 需要拒绝混合库表列，应先确认 Go 兼容要求；当前测试明确要求保留“首列身份覆盖后续列”的 Go 语义，不能仅以更严格校验替换。
- 新增测试必须继续放在同目录独立 `*_test.rs` 文件，并由 `lib.rs` 的 `#[cfg(test)]` 模块接线，不应把测试嵌入 `model.rs`。
- 性能敏感扩展应关注候选枚举中的 `String`/`Vec<Column>` 克隆和重复 `key()` 分配；若引入缓存，需要重新说明并发、失效和身份一致性，而不能改变稳定排序结果。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/planner/indexadvisor` 确认目标源码、Go 对照和测试均已索引；`node --file pkg/planner/indexadvisor/model.rs --offset 1 --limit 400` 读取了目标文件完整 272 行。
- 目标与 crate 边界：`pkg/planner/indexadvisor/model.rs`、`pkg/planner/indexadvisor/lib.rs`、`pkg/planner/indexadvisor/Cargo.toml`。
- Rust 直接调用证据：`pkg/planner/indexadvisor/algorithm.rs`（候选构造、代价比较、前缀淘汰）、`utils.rs::evaluate_index_set_cost`（加权成本、总列数、稳定键）、`indexadvisor.rs::advise_indexes_for_sql` 与 `prepare_recommendations`（查询和推荐对象生产）、`optimizer.rs`（模型与元数据/成本接口边界）。
- Go 对照：`pkg/planner/indexadvisor/model.go` 完整定义；同目录 `algorithm.go`、`indexadvisor_test.go`、`optimizer_test.go`、`utils_test.go` 提供实际消费和行为断言。
- Rust 测试：`model_test.rs::constructors_use_unicode_lowercase_like_go`、`model_test.rs::with_columns_rebuilds_all_columns_from_the_first_table_like_go`、`indexadvisor_test.rs::advisor_model_preserves_index_prefix_semantics`、`optimizer_test.rs::optimizer_cost_order_matches_go_tie_breakers`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令校验文件存在且恰有十一个固定二级章节，并人工复核未把注释中的旧草案当作当前实现。
