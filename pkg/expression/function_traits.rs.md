# `pkg/expression/function_traits.rs`

## 文件定位

[`function_traits.rs`](function_traits.rs) 是 `astersql-expression` crate 的内置函数语义分类表。crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定；[`lib.rs`](lib.rs) 一方面用 `mod function_traits; pub use function_traits::*;` 将公开常量和查询函数再导出到 crate 根，另一方面以 `function_traits_kernel` 名称装入同一文件，供 crate 内部代码和独立测试稳定引用。它不执行内置函数，而是回答“某个规范化函数名具有什么优化或 DDL 属性”。

该文件位于 SQL 表达式层和多个策略消费者之间：规划器据此决定计划缓存中的延迟求值，DDL 据此拒绝生成列中的不安全函数，表达式工具据此保留有副作用的重复表达式。RustCodeGraph 的文件节点还直接列出 [`pkg/ddl/generated_column.rs`](../ddl/generated_column.rs)、[`builtin_inference_test.rs`](builtin_inference_test.rs) 和 [`pkg/planner/core/expression_rewriter.rs`](../planner/core/expression_rewriter.rs) 三个使用文件。

## 核心职责

- 用进程级只读集合维护不可缓存、不可折叠、禁止子折叠、可尝试折叠、生成列非法、延迟求值、分区白名单、不等式、可变副作用、noop 和布尔函数分类（`UNCACHEABLE_FUNCTIONS` 至 `BOOLEAN_FUNCTIONS`）。
- 通过小型查询 API 隐藏集合实现，使调用者只依赖语义判定；所有字符串查询均为大小写敏感的精确匹配，调用方应传入解析器已规范化的小写名。
- 用 `PartitionOp` 将分区表达式允许运算符从解析器具体 opcode 解耦，并分别表达二元和一元运算规则。
- 在 `IsDeferredFunctions` 中将静态延迟函数表和会话级 `BuildContext::GetSysdateIsNow` 兼容开关合并，保持计划缓存对 `SYSDATE()` 的 Go 语义。

本文件不负责函数注册、参数校验、求值、常量折叠算法或生成列 AST 遍历；这些消费者只读取这里的策略结论。

## 主要符号

- `set(values: &[&'static str]) -> HashSet<&'static str>`：内部构造助手，将编译期字符串切片收集为集合。
- `UNCACHEABLE_FUNCTIONS`：公开的计划缓存禁用集合，包含会话状态函数以及当前与非预处理计划缓存不兼容的函数。
- `UNFOLDABLE_FUNCTIONS`：私有不可常量折叠集合；`is_unfoldable` 是公开查询入口。
- `DISABLE_FOLD_FUNCTIONS`：公开集合，当前仅含 `benchmark`，表示其子作用域也不得折叠。
- `TRY_FOLD_FUNCTIONS`：公开集合，标记 `if`、`case`、逻辑运算等可在无错误/警告时尝试折叠子表达式的控制流函数。
- `ILLEGAL_FUNCTIONS_FOR_GENERATED_COLUMNS`：公开的生成列禁用集合，覆盖非确定性、会话/全局状态依赖和系统交互函数；`embed_text` 也在其中，DDL 只在额外校验通过的 STORED 生成列路径上作专门例外。
- `DEFERRED_FUNCTIONS` 与 `is_deferred(name, sysdate_is_now)`：计划缓存开启时仍应延迟到执行期求值的时间/随机字节函数；`sysdate` 只有兼容开关为真时命中。
- `IsDeferredFunctions(ctx, name)`：从 `BuildContext` 获取兼容开关后调用 `is_deferred`。名称沿用 Go API，因此未采用 Rust snake_case。
- `ALLOWED_PARTITION_FUNCTIONS`、`PartitionOp`、`is_allowed_partition_function`、`is_allowed_partition_binary_op`、`is_allowed_partition_unary_op`：分区表达式函数及运算符白名单。五种枚举值均允许作二元运算，只有 `Plus`、`Minus` 允许作一元运算。
- `INEQUAL_FUNCTIONS` / `is_inequal_function`：当前仅将 `isnull` 标为不能从列等值条件传播的函数。
- `MUTABLE_EFFECT_FUNCTIONS` / `has_mutable_effect`：标记多次求值可能变化或有副作用、不能因重复而删除的函数。
- `NOOP_FUNCTIONS` / `is_noop_function`：与 Go 初始状态一致的空集合，保留 noop 策略入口。
- `BOOLEAN_FUNCTIONS` / `is_boolean_function`：比较、逻辑、真假判断、正则和若干验证函数的布尔语义集合。

## 执行流程

1. 首次访问任一分类时，相应 `LazyLock<HashSet<&'static str>>` 调用 `set` 构建集合；之后所有访问复用同一只读实例。
2. 普通查询函数直接调用 `HashSet::contains`，返回该规范化名称是否属于对应分类，不进行别名展开或大小写转换。
3. `IsDeferredFunctions` 先通过 `BuildContext::GetSysdateIsNow` 取得会话兼容状态，再交给 `is_deferred`；后者先检查固定延迟集合，再处理 `name == "sysdate"` 的条件分支。
4. [`pkg/planner/core/expression_rewriter.rs`](../planner/core/expression_rewriter.rs) 的 `funcCallToExpression` 在使用计划缓存时调用 `expression::IsDeferredFunctions`。命中后创建可延迟表达式；`unix_timestamp` 带参数时仍走普通表达式路径，因此这里的分类只是决策输入，不独占完整规则。
5. [`pkg/ddl/generated_column.rs`](../ddl/generated_column.rs) 的 `check_embedding_function_usage` 调用 `is_illegal_generated_column_function`。`embed_text` 在通用表中为非法，但该调用点对已经进入 embedding 校验的 `embed_text` 明确排除，再叠加函数注册和参数校验。
6. [`util.rs`](util.rs) 的 `IsMutableEffectsExpr` 和 `IsImmutableFunc` 递归遍历表达式树并调用 `has_mutable_effect`；`RemoveDupExprs` 对命中的表达式始终保留，从而避免去重删除副作用。

## 数据与状态

全部策略数据都是 `&'static str` 的 `HashSet`，由 `LazyLock` 延迟初始化并在进程生命周期内保持不变。文件没有可变全局状态、缓存失效协议或持久化数据；查询也不修改 `BuildContext`。集合查询平均为常数时间，代价主要发生在各集合第一次访问时的一次性分配与哈希构建。

分类并非互斥。例如 `embed_text` 同时不可折叠、生成列非法且有可变影响；`benchmark` 同时不可折叠、禁止子折叠且生成列非法；时间函数可能同时属于延迟求值和可变影响集合。调用者应选择与自身优化问题对应的查询，不能从一个集合推导另一个集合。

`PartitionOp` 是值语义枚举，派生 `Clone`、`Copy`、`Debug`、`Eq`、`Hash`、`PartialEq`，不携带解析器节点或运行时状态。`NOOP_FUNCTIONS` 当前为空是显式状态，而不是“未知函数都属于 noop”。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashSet` 和 `std::sync::LazyLock`。唯一 crate 内类型依赖是 `IsDeferredFunctions` 参数中的 `crate::BuildContext`，并调用其 `GetSysdateIsNow` 方法；RustCodeGraph 的 callees 查询确认了 `IsDeferredFunctions -> is_deferred` 和 `IsDeferredFunctions -> BuildContext::GetSysdateIsNow` 两条边。

公开边界由 [`lib.rs`](lib.rs) 建立：crate 根再导出让外部 crate 可调用 `astersql_expression::is_illegal_generated_column_function` 和 `astersql_expression::IsDeferredFunctions`；`function_traits_kernel` 别名则被 [`util.rs`](util.rs) 与 [`function_traits_test.rs`](function_traits_test.rs) 等内部模块使用。`Cargo.toml` 没有为本文件设置 feature gate，本文件也没有条件编译项；它随 expression library 一同编译。

已核实的上游消费者包括：

- planner 的 `funcCallToExpression`：计划缓存延迟表达式选择；
- DDL 的 `check_embedding_function_usage`：生成列/函数索引 embedding 函数合法性；
- expression `IsMutableEffectsExpr`、`IsImmutableFunc`、`RemoveDupExprs`：表达式递归分析与安全去重；
- `builtin_inference_test.rs`：验证 `embed_text` 同时注册并具有三项优化约束。

RustCodeGraph 对若干薄查询函数没有产生函数级 callers 输出，因此不能据此声称所有公开 API 均已在生产路径接线；公开集合和查询仍可能由未建函数调用边的引用、后续模块或外部 crate 使用。

## 错误处理与边界

所有查询均返回 `bool`，没有 `Result`、panic 分支或日志。未知名称、大小写不匹配和未列出的别名一律返回 `false`；这对黑名单型 API（特别是生成列非法集合）意味着新增 builtin 必须作显式安全审查，否则遗漏会表现为“允许”。[`function_traits_test.rs`](function_traits_test.rs) 因此将已注册 builtin 的合法集合与已知列表比较，迫使新增注册函数作出决定。

此文件只对函数名分类，不检查参数个数、返回类型、AST 上下文或用户权限。具体边界可由消费者补充：planner 对有参数的 `unix_timestamp` 覆盖延迟分类，DDL 对受验证的 STORED `embed_text` 覆盖通用生成列禁用结论。调用方不能把单个 `true`/`false` 当作完整合法性判定。

`is_allowed_partition_binary_op` 对 `PartitionOp` 当前所有五个变体都返回真；`is_allowed_partition_unary_op` 只接受正负号。若将来增加枚举变体，`matches!` 默认令新变体不被允许，形成保守边界。

## 并发与资源生命周期

`LazyLock` 保证每张表在并发首次访问时只初始化一次；初始化完成后只进行共享不可变读取，无显式锁管理、任务、通道、异步操作或线程局部状态。集合中的元素引用静态字符串，因此不存在借用对象提前释放的问题；集合本身与进程同生命周期。

`BuildContext` 通过共享 trait object 借用传入，`IsDeferredFunctions` 只同步读取 `GetSysdateIsNow`，不持有引用也不跨线程传播。资源风险局限于首次触达多张集合时的少量堆分配；没有需要调用者关闭、回滚或清理的资源。

## 与 Go 版本的对应关系

直接对照文件是 [`function_traits.go`](function_traits.go)。Rust 的 `UNCACHEABLE_FUNCTIONS`、`UNFOLDABLE_FUNCTIONS`、`DISABLE_FOLD_FUNCTIONS`、`TRY_FOLD_FUNCTIONS`、生成列非法表、延迟表、分区函数/运算符表、不等式表、可变影响表、空 noop 表和布尔函数表均按 Go 同名或同职责变量移植；Go 使用 `ast` 常量和 `opcode.Op`，Rust 当前用对应的小写字符串及本地 `PartitionOp`。

Go `IsDeferredFunctions` 先查 `deferredFunctions`，再在 `SYSDATE` 且 `GetSysdateIsNow()` 时返回真；Rust 的 `IsDeferredFunctions -> is_deferred` 保留相同分支。Go 注释说明延迟函数必须本身可折叠，Rust 将这项约束通过两张独立集合表达，维护时仍须人工保持不冲突。

Go 的 `TestIllegalFunctions4GeneratedColumns` 对全部 builtin 的合法列表做精确快照。Rust [`function_traits_test.rs`](function_traits_test.rs) 保留同一测试意图，但明确只对 Rust 当前已注册 builtin 的交集比较，并额外覆盖正反例、全部分区运算符、`sysdate_is_now` 两个分支、空 noop 和布尔分类。[`builtin_inference_test.rs`](builtin_inference_test.rs) 则补充 `embed_text` 的跨注册表约束。两种实现当前都把 `embed_text` 放入不可折叠、生成列非法和可变影响分类。

## 扩展指南

新增或修改 builtin 时，先判断它是否依赖会话/全局状态、是否非确定、是否有外部副作用、是否适用于计划缓存/常量折叠/生成列/分区表达式，再修改对应集合；不要只为通过单一调用点而漏掉其他分类。若改变 `SYSDATE` 兼容语义，应同时审查 `is_deferred`、`IsDeferredFunctions` 和 planner 对延迟表达式的处理。若增加分区运算符，应扩展 `PartitionOp` 并分别作二元、一元白名单决策。

测试必须继续放在独立文件中。直接分类行为应扩展 [`function_traits_test.rs`](function_traits_test.rs)，至少覆盖命中与未命中、会话开关分支及精确注册表决策；新增 inference builtin 还应同步相关独立 builtin 测试。行为来自 Go 移植时，应同步核对 [`function_traits.go`](function_traits.go) 和 [`function_traits_test.go`](function_traits_test.go)，保留 Go 的安全约束，不用缩减快照来绕过失败。

兼容性风险集中在错误分类：漏标可导致错误复用计划、规划期提前求值、删除副作用表达式或允许不安全生成列；误标会降低缓存/折叠收益或拒绝原本合法的 SQL。性能风险通常不是单次哈希查询，而是新增策略是否迫使更多表达式退出缓存、折叠或去重。名称必须使用解析器/注册表实际采用的规范拼写。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 Rust 与 Go 文件；`node --file pkg/expression/function_traits.rs` 读取了完整 340 行，并报告三个直接使用文件。
- RustCodeGraph `query`：确认 Rust/Go 两个 `IsDeferredFunctions` 定义，以及 Rust 的 `is_illegal_generated_column_function`、`is_allowed_partition_function`、`has_mutable_effect` 定义。
- RustCodeGraph `callees IsDeferredFunctions`：确认 Rust 调用 `is_deferred` 和 `BuildContext::GetSysdateIsNow`；`callers` 对所查薄查询未返回函数级边，因此文档只陈述随后由源码定位核实的调用关系。
- 已读生产与装配文件：[`function_traits.rs`](function_traits.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`util.rs`](util.rs)、[`pkg/ddl/generated_column.rs`](../ddl/generated_column.rs)、[`pkg/planner/core/expression_rewriter.rs`](../planner/core/expression_rewriter.rs)。
- 已读 Go 对照与测试：[`function_traits.go`](function_traits.go)、[`function_traits_test.go`](function_traits_test.go)。
- 已读独立 Rust 测试：[`function_traits_test.rs`](function_traits_test.rs)、[`builtin_inference_test.rs`](builtin_inference_test.rs)，并由代码搜索确认 [`fts_to_like_36_aster_unit_test.rs`](fts_to_like_36_aster_unit_test.rs) 还有一组分类烟雾覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令校验恰有十一个固定二级标题，并人工复核链接、符号名、调用边及边界陈述。
