# `pkg/expression/column.rs`

## 文件定位

本文件属于 Cargo crate `astersql-expression`。`pkg/expression/lib.rs` 通过 `#[path = "column.rs"] mod expression_column` 挂载该模块，并以 `pub use expression_column::*` 把其中的公开类型和函数暴露为表达式 crate 的公共 API。文件本身实现两种最基础的叶子表达式：从当前输入行取值的 `Column`，以及从外层查询运行期槽位取值的 `CorrelatedColumn`。

这些固有方法并不是孤立接口。`pkg/expression/core_impl.rs` 为二者实现 `Expression`、`VecExpr`、`CollationInfo`、`SafeToShareAcrossSession` 等 trait，并把 trait 调用转发回本文件，因此规划器、执行器和表层代码通常通过 `dyn Expression` 使用它们。直接可见的应用接线包括：`pkg/table/column.rs::FillVirtualColumnValue` 调用 `Column::EvalVirtualColumn` 生成虚拟列，`pkg/util/ranger/detacher.rs::IsValidShardIndex` 调用 `GcColumnExprIsTidbShard` 校验分片索引，planner 的 property/handle-column 代码调用 `ResolveIndices` 和 `HashCode`，session 的 EXPLAIN 查询构造调用 `Column2Exprs`。

## 核心职责

- 表达列身份：`Column` 同时保存物理访问位置 `Index`、表列 ID `ID` 和计划内逻辑身份 `UniqueID`。普通表达式相等判断使用 `UniqueID`，而完整缓存/结构相等还比较类型、索引、虚拟表达式、显示信息、排序规则和标志位。
- 行式与向量化求值：各 `Eval*` 从 `chunk::Row` 的 `Index` 位置读取指定物理类型；各 `VecEval*` 从 `chunk::Chunk` 重建输出列。混合类型（BIT/ENUM/SET）和 FLOAT 有专门转换路径，不能简单复制底层缓冲。
- 关联子查询取值：`CorrelatedColumn` 内嵌一个 `Column`，并通过 `Option<Arc<RwLock<types::Datum>>>` 保存执行期由外层行绑定的值；其向量化求值把这个值按常量表达式广播到所有输入行。
- schema 重绑定和去关联：`Column::ResolveIndices` 把逻辑列重新定位到目标 schema；`ResolveIndicesByVirtualExpr` 在表达式索引场景先选精确 `UniqueID`，找不到时才用虚拟表达式相等的第一个候选。`CorrelatedColumn::Decorrelate` 在 schema 已含基础列时降为普通列。
- 稳定标识和缓存支持：`HashCode` 生成由列标志和 `UniqueID` 组成的紧凑键，`Hash64`/`Equals` 覆盖完整结构，`ToCacheSnapshot` 委托 `CachedColumn` 生成递归快照。
- 列元数据辅助：包含显示文本、类型/排序规则推导、`ColumnInfo` 转换、列数组转换/查找/排序、内存估算和 `tidb_shard` 虚拟表达式识别。

## 主要符号

- `CorrelatedDatum = Arc<RwLock<types::Datum>>` 与 `NewCorrelatedDatum`：Rust 对 Go `*types.Datum` 的线程安全共享槽位表示。`Arc` 使克隆后的关联列共享同一运行期值，`RwLock` 保护读写。
- `CorrelatedColumn { column, data }`：关联列对象。`Clone` 浅克隆 `data` 的 `Arc`；`SafeToShareAcrossSession` 恒为 `false`；`IsCorrelated` 为 `true`；`ConstLevel` 为 `None`。
- `Column`：核心列引用。`RetType` 是可选字段类型；`ID` 用于元数据匹配；`UniqueID` 是逻辑身份；`Index` 是当前输入 schema 的位置；`VirtualExpr` 保存虚拟生成列表达式；`OrigName`/`IsHidden` 控制展示；`IsPrefix`、`InOperand`、`CorrelatedColUniqueID` 保存规划语义；`hashcode` 与 `collation_info` 是 crate 内缓存/状态。
- `Column::new`：构造已经具备类型、ID、唯一 ID 和输入位置的列，其余字段取默认值。
- `Eval`、`EvalInt/Real/String/Decimal/Time/Duration/JSON/VectorFloat32`：行式求值族。返回值都携带 SQL NULL 标志；需要类型转换的方法用 `Result` 传播转换错误。
- `VecEvalInt/Real/String/Decimal/Time/Duration/JSON/VectorFloat32`：向量化求值族。普通固定类型走 `CopyReconstruct(input.Sel(), None)`；混合整数/字符串逐行转换； FLOAT 从 `f32` 扩展到 `f64` 并遵守 selection vector 和 NULL 位图。
- `ResolveIndices`/`resolveIndices`：在 `Schema::ColumnIndex` 中找列并写入 `Index`，失败时返回包含目标列和 schema 列表的错误。
- `ResolveIndicesByVirtualExpr`/`resolveIndicesByVirtualExpr`：用于隐藏生成列或表达式索引的重绑定，保证精确列身份优先于仅虚拟表达式相等。
- `HashCode`、`CanonicalHashCode`、`CleanHashCode`：维护紧凑的列身份缓存。`CleanHashCode` 使下一次访问重新编码。
- `Hash64`/`Equals`：完整结构哈希与相等协议。`CorrelatedColumn` 特意忽略当前 `data` 值，只加入关联列标志和内嵌列。
- `StringWithCtx`、`StringWithCtxForExplain`、`String`、私有 `string`：隐藏虚拟列显示其表达式，显式 `OrigName` 优先，否则显示 `Column#<UniqueID>` 或无编号的 `Column`。
- `Coercibility`/`Repertoire`：延迟推导列的排序规则强制性；JSON/非 ASCII 字符串使用 Unicode repertoire，其余默认 ASCII。
- `Column2Exprs`、`ColInfo2Col`、`SortColumns`、`GcColumnExprIsTidbShard`：分别完成列到 trait 对象转换、按 `ColumnInfo.ID` 查找、按 `UniqueID` 排序副本、识别 `tidb_shard` 标量函数。

## 执行流程

普通列的典型路径如下：规划阶段创建 `Column` 并赋予 `UniqueID`；进入具体算子前，`ResolveIndices` 根据该身份在算子输入 `Schema` 中更新 `Index`；执行阶段通过 `Expression` trait 进入 `core_impl.rs` 的转发实现，再落回本文件的 `Eval*` 或 `VecEval*`；求值使用 `Index` 从行或列缓冲读取数据。SQL NULL 先被识别并返回类型零值加 `is_null = true`，非 NULL 再按静态类型读取或转换。

向量化路径有三类。大多数类型直接对输入列执行 `CopyReconstruct`，从而同时应用 chunk 的 selection vector；BIT/ENUM/SET 等 `Hybrid()` 类型逐逻辑行调用行式求值，保持转换语义和错误传播；MySQL FLOAT 的物理存储是 `f32`，`VecEvalReal` 逐行扩展为 `f64`，有 selection 时用选中源行并单独复制 NULL 状态。

关联列不读取传入行。执行器应先把外层值写入 `data` 指向的共享 `Datum`，行式求值随后取得读锁并克隆或按类型读取该值；向量化入口统一调用 `genVecFromConstExpr`，把同一外层值广播为结果列。`Decorrelate` 在给定 schema 包含内嵌列时返回普通列引用，否则保留关联表达式。

虚拟列重绑定先扫描 schema：一旦 `EqualColumn` 命中相同 `UniqueID` 就立即采用；扫描过程中仅记录第一个 `EqualByExprAndID` 的表达式相等候选；没有精确命中时才应用该回退。虚拟列实际计算由 `EvalVirtualColumn` 调用 `VirtualExpr::Eval`，其生产调用点是 `pkg/table/column.rs::FillVirtualColumnValue`，之后表模块还会按列定义转换结果。

## 数据与状态

`ID`、`UniqueID` 与 `Index` 不可混用：`ID` 对接 `model::ColumnInfo`，`UniqueID` 决定计划表达式中的同一性，`Index` 只说明当前算子输入行中的物理位置。schema 变化后必须重新解析 `Index`；仅复制旧列而不重绑可能读取错误位置。

`RetType` 在类型化求值、`GetStaticType`、`ToInfo`、repertoire 和向量路径中被当作已存在，不满足该构造不变量会触发 `unwrap` panic。`VirtualExpr` 存在时列不能跨 session 安全共享；其表达式也参与完整哈希、相等和内存估算。`hashcode` 是惰性缓存，内容只编码 `columnFlag + UniqueID`，修改 `UniqueID` 后调用者必须先 `CleanHashCode`，否则会读到旧缓存。

`CorrelatedDatum` 是跨克隆共享的可变执行状态。`CorrelatedColumn::Clone` 和 `RemapColumn` 都保留同一个 `Arc`；映射只替换内嵌列元数据。`MemoryUsage` 读取锁内 Datum 的内存用量，但该估算把共享值计入每个引用，不能视为进程级去重统计。

`collation_info` 包含内部可变的 coercibility、repertoire、charset 和 collation 状态。`Coercibility` 在首次请求时调用 `deriveCoercibilityForColumn` 并缓存结果；`Repertoire` 若未显式设置，则根据字段求值类型和字符集计算。

## 依赖与调用关系

下游依赖主要经 `use crate::*` 引入：`chunk` 提供行、数据列、chunk 与 selection vector；`types` 提供 `Datum`、`FieldType` 和所有 SQL 值类型；`Schema` 负责逻辑列定位；`Expression`、`VecExpr`、`EvalContext` 和 `TraverseAction` 定义表达式协议；`codec` 生成紧凑哈希；`base::Hasher` 实现结构哈希；`collationInfo`、charset 常量提供排序规则；`model::ColumnInfo` 承接元数据；`ast::TiDBShard` 是分片函数名。`pkg/expression/Cargo.toml` 将这些边界映射到 `astersql-types`、`astersql-util-chunk`、parser AST/MySQL、meta model、planner base 等 workspace crate。

trait 接线集中在 `pkg/expression/core_impl.rs`：`forward_vec_expr!(Column/CorrelatedColumn)` 转发所有向量方法；`direct_collation!(Column, collation_info)` 和关联列委托实现排序规则；两个 `impl Expression` 转发行式求值、解析、重映射、哈希和内存估算。因此搜索调用者时不能只找本文件固有方法，还要包括 `dyn Expression` 调用。

已核对的直接上游包括：

- `pkg/table/column.rs::FillVirtualColumnValue` → `Column::EvalVirtualColumn`，用于生成列物化。
- `pkg/util/ranger/detacher.rs::IsValidShardIndex` → `GcColumnExprIsTidbShard`，并继续检查标量函数只有一个列参数且与第二索引列相同。
- `pkg/planner/property/physical_property.rs`、`pkg/planner/util/handle_cols.rs` → `HashCode`/`ResolveIndices`，用于属性键和 schema 重绑定。
- `pkg/session/runtime/explain_query.rs` → `Column2Exprs`，把 schema 列转成表达式列表。
- `pkg/expression/scalar_function.rs` → 子表达式 `RemapColumn`、`CleanHashCode`、排序规则接口，使列行为参与复合表达式递归。

## 错误处理与边界

- `Column::ResolveIndices` 找不到逻辑列时返回错误，并列出目标和 schema 中的列/唯一 ID；它不会静默保留旧 `Index`。
- `RemapColumn` 在 mapping 缺少 `UniqueID` 时返回 `Can't remap column ...`，普通列和关联列均如此。
- 混合类型的 `EvalInt`/`EvalString` 以及相应向量方法传播 Datum 转换错误；BIT 专门走二进制字面量到整数的转换。
- SQL NULL 不作为错误：各类型返回其零值以及 `true`。但未绑定的关联列不是 SQL NULL，而是执行器不变量被破坏；`CorrelatedColumn::Eval` 明确以 `expect("correlated column data is not bound")` panic，`column_test.rs::unbound_correlated_column_eval_does_not_become_null` 固化了这一行为。其他关联列类型入口对缺失 `data` 使用 `unwrap`，同样要求先绑定。
- `RwLock` 中毒会以 `expect("correlated datum lock poisoned")` panic，没有恢复分支。
- `GetStaticType`、虚拟列求值和若干向量入口对 `RetType`/`VirtualExpr` 使用 `unwrap`；这些是构造阶段保证的内部契约，不是面向任意缺省 `Column` 的容错 API。
- `ResolveIndicesByVirtualExpr` 找不到候选时只返回 `false`，不会产生错误；调用者必须检查布尔值。
- `shouldRemoveColumnNumbers` 在 Rust 当前恒为 `false`，所以 `StringWithCtx` 不会自动因 plan-tree EXPLAIN 去掉编号；只有显式调用 `StringWithCtxForExplain(..., true)` 会得到 `Column`。这是当前实现事实，不应按 Go 行为推断为已自动支持。

## 并发与资源生命周期

普通 `Column` 主要是拥有型值：字符串、哈希缓冲和字段类型随克隆复制，`VirtualExpr` 通过 trait 对象克隆递归复制。它没有后台任务、channel、事务或显式 I/O；生命周期跟随计划/表达式树。

关联值是本文件唯一显式同步资源。`NewCorrelatedDatum` 创建 `Arc<RwLock<Datum>>`，`CorrelatedColumn` 的克隆和重映射增加强引用计数，最后一个引用释放时 Datum 被销毁。求值和内存估算持有短期读锁；本文件没有写锁入口，写入应由负责绑定外层行的执行阶段通过共享槽完成。虽然槽位类型可跨线程，`SafeToShareAcrossSession` 仍恒为 `false`，因为值属于一次 session/执行上下文，线程安全不等于跨会话语义安全。

向量求值借用输入 chunk 并写入调用者提供的结果列；普通快速路径以重建后的拥有型 `chunk::Column` 替换结果，不在本文件保留对输入缓冲的长期借用。无异步任务或需要显式关闭的资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/column.go`，独立测试是 `pkg/expression/column_test.go` 与 Rust 的 `pkg/expression/column_test.rs`。两端总体保持相同职责和分支：关联列按常量向量化、普通列的 Hybrid/FLOAT 特殊路径、schema 解析、虚拟表达式精确 ID 优先、哈希/相等字段集合、排序规则推导、列辅助函数和内存估算均能逐项对应。

语言表示差异包括：Go 以嵌入的 `Column` 和 `*types.Datum` 表示关联列，Rust 使用命名字段及 `Arc<RwLock<Datum>>`；Go 的 `[]*Column`/`Expression` 接口在 Rust 中分别变为拥有型 `Vec<Column>` 和 `Box<dyn Expression>`；Go 的 nil 分支在 Rust 多由 `Option` 表达，但若 `RetType`、`VirtualExpr` 或关联数据属于已建立的不变量，Rust 当前使用 `unwrap`/`expect`。

已确认的语义差异或迁移注意点：

- Go `shouldRemoveColumnNumbers` 会在实际 EXPLAIN 且格式为 `plan_tree` 时返回 `true`；Rust 实现当前无视上下文并恒为 `false`。Rust 测试仅验证显式 `StringWithCtxForExplain(..., true)` 且 `OrigName` 不被改写。
- Go 的 `CorrelatedColumn::Clone` 共享 Datum 指针；Rust 通过克隆 `Arc` 保持这一浅共享语义，同时增加锁保护。
- Go 可表达 nil receiver，并在 `MemoryUsage`/`Equals` 中有相关分支；Rust 方法需要有效引用，不存在 nil receiver，对应的 Go “nil wrapped in any” 测试不能原样出现。
- Rust `column_test.rs` 包含真实的基础 parity suite、未绑定关联列 panic、显式列名和哈希字段测试，但不少 Go 用例主体以对照注释保留；不能仅凭这些注释声称所有 Go 边界都已在 Rust 执行验证。

## 扩展指南

新增列字段时，应同步更新 `Default`、`Clone` 语义、`Hash64`、`Equals`、`MemoryUsage` 和缓存快照类型；若字段影响紧凑身份键，还要评估 `HashCode` 编码及所有依赖该键的 planner/property 逻辑。哈希和相等字段必须成对变化，避免“相等对象哈希不同”或缓存错误复用。

新增求值类型或改变物理表示时，应同时补齐 `Column` 与 `CorrelatedColumn` 的行式、向量化入口，以及 `pkg/expression/core_impl.rs` 中 `VecExpr`/`Expression` trait 的对应接口。关注 selection vector、NULL 位图、Hybrid 转换、FLOAT 精度和错误传播；不要把需要转换的类型降级为直接缓冲复制。

改变 schema/虚拟表达式匹配时，应保留“精确 `UniqueID` 优先、表达式相等仅回退”的确定性，并在 `pkg/expression/column_test.rs` 的独立测试中覆盖多候选次序、无匹配返回值及错误信息。生成列行为还应检查 `pkg/table/column.rs::FillVirtualColumnValue`，分片函数识别要同步检查 `pkg/util/ranger/detacher.rs::IsValidShardIndex`。

修改关联列生命周期时，必须保持 `SafeToShareAcrossSession = false`，除非同时证明执行期 Datum 不再携带会话状态；还应明确共享/深拷贝语义、锁中毒策略和写入方契约。测试继续放在独立的 `pkg/expression/column_test.rs`，不要内嵌到生产源文件；若追求 Go 完整对齐，应把现有注释型 Go 用例逐步变成真实 Rust 断言，而不是删减分支。

修复 EXPLAIN 自动去编号时，最可能修改 `shouldRemoveColumnNumbers` 及上下文适配层，并同时覆盖：非 EXPLAIN、非 plan-tree、大小写/空白格式、显式 `OrigName`、隐藏虚拟列。此处当前与 Go 不一致，扩展文档或调用方不得假设已经自动生效。

## 验证依据

- 源码全量阅读：`pkg/expression/column.rs`（907 行），核对 `CorrelatedColumn`、`Column`、全部行式/向量式求值、解析/映射、哈希/相等、排序规则和辅助函数。
- crate 与模块接线：`pkg/expression/Cargo.toml`；`pkg/expression/lib.rs` 中 `expression_column` 的路径挂载、公开再导出和独立 `column_test` 测试模块；`pkg/expression/core_impl.rs` 中 `Column`/`CorrelatedColumn` 的 trait 实现。
- Go 对照与测试：`pkg/expression/column.go`、`pkg/expression/column_test.go`；Rust 独立测试：`pkg/expression/column_test.rs`。重点核对 Hybrid 求值、虚拟表达式解析优先级、未绑定关联列、辅助函数、HashCode、Hash64/Equals 与虚拟表达式字段。
- 调用证据：`pkg/table/column.rs::FillVirtualColumnValue`、`pkg/util/ranger/detacher.rs::IsValidShardIndex`、`pkg/planner/property/physical_property.rs`、`pkg/planner/util/handle_cols.rs`、`pkg/session/runtime/explain_query.rs`、`pkg/expression/scalar_function.rs`。
- RustCodeGraph：`status` 成功，索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；但 `files --filter pkg/expression/column` 未命中，精确 `query Column` 也未返回目标文件，`resolve_indices_from_schema`/`eval_virtual_expr` 查询为空。因此目标文件及调用关系使用上述源码、模块接线和 `rg` 直接引用补证，未把缺失的图结果伪装成已验证调用边。
- 结构验证使用任务指定命令，要求目标存在且固定二级标题恰好为 11 个；本任务为纯文档分析，按计划不运行 Cargo。
