# `pkg/types/field_name.rs`

## 文件定位

本文件实现 SQL 规划阶段使用的结果列命名元数据。它保存一列的原始表/列名、解析后的库/表/列名以及可见性标志，并提供列名列表的复制和 AST 列名匹配能力。源码不是由顶层 `pkg/types/lib.rs` 直接声明，而是由 `pkg/types/internal/metadata/lib.rs` 的 `field_name_defs` 通过 `include!("../../field_name.rs")` 编译进 `astersql-types-metadata`；随后顶层 `astersql-types` 将该子 crate 再导出为 `metadata`。因此常见调用路径是 `types::metadata::{FieldName, NameSlice, EmptyName}`，而 `pkg/expression/lib.rs` 又把这些符号暴露给表达式和规划器代码。

直接所属 crate 是 `pkg/types/internal/metadata/Cargo.toml` 中的 `astersql-types-metadata`，其 `parser-ast` 和 `tidb-size` 依赖分别提供 `ast::CIStr`、`ast::ColumnName` 与内存大小常量。`pkg/types/Cargo.toml` 则以 `types-group-4` 路径依赖组装该子 crate，并记录 Go 包对照为 `pkg/types`。

## 核心职责

- `FieldName` 表示结果或计划 Schema 中一列的命名身份：`OrigTblName`、`OrigColName` 保留来源名称，`DBName`、`TblName`、`ColName` 保存当前解析名称（包括别名）。证据为 `FieldName` 的八个公开字段。
- `Hidden` 控制对外显示；`NotExplicitUsable` 表示列不可被 SQL 显式引用；`Redundant` 标识名称解析中的冗余列。后两个标志由本文件保存和复制，本文件本身不解释或改变它们。
- `NameSlice` 维护与 Schema 列位置对齐的名称序列。其元素为 `Option<Arc<FieldName>>`，所以既能表达“该位置没有名称”，也能让多个计划节点共享不可变的字段名对象。
- `EmptyName` 是隐藏列的全局占位对象。规划器在新增仅供内部计算的列、补齐名称列表或隔离子查询名称可见性时使用它，例如 `pkg/planner/core/expression_rewriter.rs` 和 `pkg/planner/core/planbuilder_runtime.rs`。

## 主要符号

- `FieldName`：公开结构体，派生 `Debug` 和 `Default`；五个 `ast::CIStr` 字段保存名称，三个 `bool` 字段保存状态。
- `FieldName::String(&self) -> String`：隐藏列返回常量 `EMPTY_NAME`；否则按非空限定符拼成 `db.tbl.col`。它读取 `CIStr.L`，因此输出是规范化的小写形式，而不是保留原样的 `CIStr.O`。
- `FieldName::MemoryUsage(&self) -> i64`：累加五个 `CIStr` 的估算值与三个布尔值。私有函数 `cistr_memory_usage` 对每个 `CIStr` 计入两个字符串头和 `O`、`L` 两份内容长度；这是估算接口，并非分配器的精确驻留字节数。
- `FieldName::Clone(&self) -> FieldName`：复制全部名称和状态。Rust `String::clone` 会复制字符串内容，回归测试以底层指针不同锁定这一深拷贝行为。
- `NameSlice(pub Vec<Option<Arc<FieldName>>>)`：公开 newtype；`None` 是合法位置占位，`Some(Arc<_>)` 表示共享字段名。
- `NameSlice::Shallow(&self) -> NameSlice`：复制外层 `Vec`，但 `Arc` 只增加引用计数，字段名对象保持共享。
- `NameSlice::FindAstColName(&self, name: &ast::ColumnName) -> bool`：线性扫描非空元素；Schema 或 Table 为空时视为不限定，否则比较对应的规范化 `L` 值，列名始终必须相等。
- `EmptyName: LazyLock<Arc<FieldName>>`：首次访问时创建 `Hidden = true`、其余字段为默认值的全局共享实例。

## 执行流程

字段名通常在表达式或计划构建时生成，随 Schema 一起在逻辑/物理计划节点间传递。`pkg/planner/core/expression_rewriter.rs::rewriteAstExprWithPlanCtx` 用 `NameSlice::Shallow` 给临时计划复制名称列表，避免修改调用方的外层容器，同时继续共享各 `FieldName`。表达式重写结束时，同文件的 `rewriteExprNode` 截断旧列名，并用 `EmptyName` 为新增的内部列补位，防止子查询内部名称泄漏到后续解析。

插入语句的 ON DUPLICATE 路径在 `pkg/planner/core/planbuilder_runtime.rs` 调用 `table_names.FindAstColName(&column)`，判断赋值表达式引用的 AST 列是否已属于目标表；匹配使用 Schema/Table 可省略、列名不可省略的规则。该路径随后以 `EmptyName` 初始化新行列名数组，并通过 `Shallow` 扩展计划输出名称。

需要展示列名时，调用方使用 `FieldName::String`：隐藏占位统一显示 `EMPTY_NAME`，普通字段按已有限定符从宽到窄拼接。需要计划内存估算时，计划节点可将每个字段名的 `MemoryUsage` 纳入自身统计，`pkg/planner/core/operator/physicalop/physical_schema_producer_test.rs` 对这一组合方式有断言。

## 数据与状态

`FieldName` 自身没有内部可变性。`Orig*` 与解析后的名称并存，使调用方既能保留真实来源，又能使用别名后的名字做解析或展示；本文件不会自动同步两组名称。所有比较和显示均使用 `CIStr.L`，大小写折叠行为由 `ast::CIStr` 的构造逻辑负责。

`NameSlice` 的位置与计划 Schema 列位置具有隐含对应关系，但类型本身不校验长度一致性。外层 `Vec` 可变；共享的 `FieldName` 包在 `Arc` 中且本文件未提供可变入口。`None` 与 `Some(EmptyName.clone())` 语义不同：前者表示缺少字段名对象，后者表示存在一个明确隐藏的占位名。`pkg/expression/cache_snapshot.rs::CachedNameSlice` 在快照往返中分别保留这两种状态，相关测试覆盖了普通名称、`None` 和隐藏名称三种元素。

## 依赖与调用关系

下游依赖仅有两类：`ast::CIStr`/`ast::ColumnName` 提供大小写不敏感名称及 AST 查询对象，`size::SizeOfString`/`SizeOfBool` 提供内存估算常量；`std::sync::{Arc, LazyLock}` 提供共享所有权和延迟初始化。模块接线来自 `pkg/types/internal/metadata/lib.rs::field_name_defs`。

上游消费集中在表达式与规划器：`pkg/expression/expression.rs` 构造 `FieldName` 和 `NameSlice`；`pkg/expression/cache_snapshot.rs` 序列化式捕获并恢复全部字段；`pkg/planner/core/expression_rewriter.rs` 使用 `Shallow` 与 `EmptyName` 管理名称可见性；`pkg/planner/core/planbuilder_runtime.rs` 使用 `FindAstColName` 和 `EmptyName` 完成 INSERT/ON DUPLICATE 的名称接线；逻辑、物理 Schema producer 保存并转交 `NameSlice`。RustCodeGraph 的文件节点还报告该文件被 `pkg/expression/cache_snapshot.rs`、`pkg/expression/expression.rs`、`pkg/expression/simple_rewriter.rs` 等 9 个索引文件使用，但对这些重名方法的精确 callers/callees 查询未返回边，因此上述边均以直接引用搜索和源码片段复核。

## 错误处理与边界

本文件所有 API 都是不返回 `Result` 的确定性操作。`String`、`MemoryUsage`、`Clone` 和 `Shallow` 的主要失败模式仅是内存分配失败或极端长度下的容量/整数溢出，代码没有单独恢复路径。`FindAstColName` 在空切片或全部为 `None` 时返回 `false`，通过 `iter().flatten()` 跳过缺失元素；它不会把空列名当通配符，只有空 Schema 和空 Table 具备通配语义。

调用方必须维持名称序列和 Schema 的位置关系，并在按索引取值前处理 `None`。`String` 对仅有列名的对象输出 `col`，只带表名时输出 `tbl.col`，只带库名时输出 `db.col`；它不会产生多余的前导或连续句点。`Hidden` 优先于所有名称字段，一旦为真就固定返回占位字符串。

## 并发与资源生命周期

`Arc<FieldName>` 允许名称对象跨计划结构安全共享所有权；`NameSlice::Shallow` 只克隆 `Arc`，所以复杂度为 O(n)，字段名内容不会重复分配。最后一个 `Arc` 被释放时对象自动销毁，无需显式关闭或清理。`EmptyName` 使用标准库 `LazyLock`，初始化只发生一次且并发安全，之后每次克隆只调整原子引用计数。

文件不创建线程、任务、锁、通道、事务或外部资源。虽然 `FieldName` 字段公开，但共享实例只能通过不可变 `Arc` 访问；若未来引入内部可变性，需要重新评估跨线程同步成本、浅拷贝的别名效应和缓存快照一致性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/field_name.go`。字段集合、隐藏名称字符串、限定名拼接顺序、内存估算组成、外层切片浅拷贝、空限定符匹配规则及 `EmptyName` 的用途均与 Go 实现一致。Rust 用 `Option<Arc<FieldName>>` 对应 Go 的 `[]*FieldName`，用 `LazyLock<Arc<_>>` 对应包级 `*FieldName` 变量。

存在三项需要保持清醒的语言边界差异。第一，Go `MemoryUsage` 对 nil receiver 返回 0；Rust 方法接收 `&self`，安全 Rust 中不能以空引用调用。第二，Go `FindAstColName` 若切片含 nil 且扫描到该元素会解引用失败；Rust 使用 `flatten()` 安全跳过 `None`。第三，Go 注释称 `Clone` 为浅拷贝，因为 `CIStr` 是值字段；Rust 显式克隆其中的字符串，当前测试要求克隆后 `DBName.L` 的存储指针不同，因此是内容独立的深拷贝。扩展时应保持可观察 SQL 语义一致，不应为了逐字翻译而撤销这些 Rust 所有权边界。

## 扩展指南

新增字段时，应同步修改 `FieldName`、`Clone`、`MemoryUsage`（若字段拥有堆内存）、`EmptyName` 默认构造以及 `pkg/expression/cache_snapshot.rs::CachedFieldName::{capture,restore}`，并核对 Go `pkg/types/field_name.go` 的对应增量。新增名称匹配规则时，入口是 `FindAstColName`；必须明确空限定符、大小写规范化、`None` 和隐藏字段的语义，并检查 `pkg/planner/core/planbuilder_runtime.rs` 的 ON DUPLICATE 使用场景。

测试逻辑应继续放在独立文件，而不是嵌入本源文件。基础行为应扩展 `pkg/types/enum_4_aster_unit_test.rs::field_names_match_rendering_memory_clone_shallow_and_lookup_semantics`；快照字段变化应扩展 `pkg/expression/cache_snapshot_test.rs`；计划内存计费变化应扩展 `pkg/planner/core/operator/physicalop/physical_schema_producer_test.rs`；名称解析流程变化还应覆盖 `pkg/expression/simple_rewriter_test.rs` 或对应规划器独立测试。性能风险主要来自 `FindAstColName` 的 O(n) 扫描、`String` 的分配以及 `Clone` 对五组字符串的复制；兼容风险主要是字段位置对齐、隐藏列可见性和别名/原名混用。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件且目标文件已索引；`files --filter pkg/types/field_name.rs` 显示 9 个符号；`node --file pkg/types/field_name.rs --offset 1 --limit 400` 完整读取 117 行并报告 9 个使用文件；`query FieldName --kind struct`、`query NameSlice --kind struct` 核对 Rust/Go 定义及相关消费者。精确 callers/callees 查询无输出，未将其当作不存在调用者的证据。
- 源码与接线：`pkg/types/field_name.rs`、`pkg/types/internal/metadata/lib.rs`、`pkg/types/internal/metadata/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`。
- Go 对照：`pkg/types/field_name.go`。
- 直接调用证据：`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/planbuilder_runtime.rs`、`pkg/expression/cache_snapshot.rs`。
- 独立测试证据：`pkg/types/enum_4_aster_unit_test.rs` 覆盖显示、内存估算、深拷贝、Arc 浅拷贝、通配匹配和隐藏名；`pkg/expression/cache_snapshot_test.rs` 覆盖全部状态字段及 `None`/隐藏占位往返；`pkg/planner/core/operator/physicalop/physical_schema_producer_test.rs` 覆盖字段名进入计划内存估算。
- 本任务是纯文档分析，按计划未运行 Cargo，也未修改 Rust、Go、Cargo 或只读总计划；交付前使用任务指定命令检查文档恰有 11 个固定二级章节。
