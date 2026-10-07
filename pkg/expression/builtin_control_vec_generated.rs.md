# `pkg/expression/builtin_control_vec_generated.rs`

## 文件定位

本文件属于 `astersql-expression` crate。`pkg/expression/Cargo.toml` 将该 crate 的入口指定为 `pkg/expression/lib.rs`，后者在第 185—186 行用 `#[path = "builtin_control_vec_generated.rs"] mod builtin_control_vec_generated_kernel;` 把本文件作为私有模块编入库。文件头明确标注它是控制流向量求值的生成代码，覆盖 SQL `CASE WHEN`、`IFNULL` 和 `IF`。

这里需要区分“crate 内存在”与“完整运行时已接线”：模块不是 `pub mod`，其中类型虽然多为 `pub`，可见范围仍受私有父模块限制；仓库内对这些 Rust 签名的直接构造仅见 `builtin_control_vec_generated_test.rs` 和 `builtin_control_vec_generated_8_aster_unit_test.rs`。`builtin.rs`、`distsql_builtin.rs` 和 `builtin_threadsafe_generated.rs` 出现的同名文本是 Go 签名名称/移植元数据，未形成对本模块类型的 Rust 调用。因此，本文件当前是可编译、可独立验证的 Rust 语义实现，但没有证据表明它已经替代 Go 控制表达式并进入 Rust SQL 执行主链。

## 核心职责

文件承担三层职责：

1. 定义最小求值协议 `VectorExpression<T>`、错误 `EvalError` 和带警告水位的 `EvalContext`，使控制表达式可以在不依赖现有 Go `chunk.Column`/`EvalContext` 的情况下被测试。
2. 用泛型核心 `BuiltinCaseWhen<T>`、`BuiltinIfNull<T>`、`BuiltinIf<T>` 实现向量求值和逐行标量回退，保持 SQL 的 NULL/真假选择规则以及 Go 版本的投机求值回退策略。
3. 通过 `case_when_signature!`、`if_null_signature!`、`if_signature!` 为 Int、Real、Decimal、String、Time、Duration、JSON 七类返回值展开 21 个 Go 风格包装类型及 `fallbackEval*`、`vecEval*` 方法。

它不是表达式解析器、类型推导器或通用向量执行器；构造者必须预先提供已类型化的子表达式和行数。

## 主要符号

- `Decimal = rust_decimal::Decimal`、`Time = chrono::NaiveDateTime`、`Duration = chrono::Duration`、`BinaryJSON = serde_json::Value`：七类型矩阵中四个非原生类型的本地别名。它们直接对应 `Cargo.toml` 中的 `rust_decimal`、`chrono`、`serde_json` 依赖，但不是 TiDB/AsterSQL 完整类型系统包装。
- `EvalError { message }`：可克隆、可比较的轻量错误，`new` 接收字符串，`Display` 原样输出消息，并实现 `std::error::Error`。
- `EvalContext { warnings }`：保存 `Vec<String>` 警告；`warning_count` 提供投机前水位，`truncate_warnings` 执行回滚，`warnings` 供观察验证。
- `VectorExpression<T>: Send + Sync`：核心子表达式接口。实现者必须提供 `eval_row(ctx, row)`；默认 `vec_eval(ctx, rows)` 按 `0..rows` 顺序调用标量接口并在首个错误处返回。
- `CaseBranch<T>`：`(Box<dyn VectorExpression<i64>>, Box<dyn VectorExpression<T>>)`，分别表示 WHEN 条件和 THEN 结果。
- `check_column_len`：拒绝子表达式返回的列长度与 `rows` 不一致；错误中记录实际与期望行数。
- `rollback_speculative_warnings`：只在当前警告数高于投机前水位时截断，因而保留调用前已有警告。
- `BuiltinCaseWhen<T>`：保存有序 `branches` 与可选 `else_expr`；首个非 NULL、非零条件命中，否则取 ELSE，无 ELSE 时产生 NULL。
- `BuiltinIfNull<T>`：保存 `lhs`/`rhs`；左值非 NULL 时取左，否则取右。
- `BuiltinIf<T>`：保存整数条件、真分支和假分支；条件为非 NULL 且非零时取真分支，零或 NULL 时取假分支。
- 三个签名宏：生成薄包装、新建函数、类型专属回退/向量方法以及恒为 `true` 的 `vectorized()`。文件末尾的 21 次宏调用是对外形态的完整清单。

所有泛型核心要求 `T: Clone + Send + Sync + 'static`：`Clone` 用于从预计算列按行组装结果，线程安全界限来自 trait 对象，`'static` 使装箱表达式不携带短生命周期借用。

## 执行流程

`BuiltinCaseWhen::vec_eval` 的流程如下：

1. 记录 `EvalContext` 的警告水位。
2. 按声明顺序投机向量求值每一对 WHEN/THEN，并验证每列长度；任一向量错误、长度错误或新增警告都会回滚新增警告，然后对整个表达式调用 `fallback_eval`。
3. 若有 ELSE，同样投机求整列并应用相同回退判据。
4. 所有列成功后按行扫描分支；首个条件为 `Some(nonzero)` 的 THEN 被克隆到结果，NULL 与零条件均跳过；未命中时取 ELSE，缺少 ELSE 时保留 NULL。

`BuiltinIfNull::vec_eval` 先强制向量求值左列并检查长度；该步骤的错误直接传播。随后记录警告水位并投机求右列，右列错误、错长或新增警告触发全表达式逐行回退。成功时用 `lhs.or(rhs)` 合并每行。

`BuiltinIf::vec_eval` 先强制向量求值条件列并检查长度；错误直接传播。之后依次投机求真假两列，任一分支错误、错长或新增警告都回滚并逐行重算。成功时条件 `Some(nonzero)` 选择真列，`Some(0)` 或 `None` 选择假列。

三个 `fallback_eval` 都按行顺序执行，并保持短路语义：CASE 只标量求值首个命中条件对应的 THEN 或 ELSE；IFNULL 仅在左值 NULL 时求右值；IF 只求选中的分支。任何实际求值路径上的标量错误用 `?` 立即返回。

## 数据与状态

输入行没有实体 `Row` 或 `Chunk`，仅由 `usize` 行号和总行数表示；列表示为 `Vec<Option<T>>`，其中 `None` 表示 SQL NULL。条件固定为 `Option<i64>`：NULL 和 0 为假，其余整数为真。

`BuiltinCaseWhen`、`BuiltinIfNull`、`BuiltinIf` 拥有其装箱子表达式；每次调用只分配本次结果及投机列，不缓存跨调用结果。CASE 的临时列规模约为所有 WHEN/THEN 列加可选 ELSE，IF/IFNULL 则会同时持有参数列；这与 Go 版从缓冲分配器借列并 `defer put` 的资源模型不同。

唯一可变共享入参是 `&mut EvalContext`。警告列表既是可观察状态，也是决定是否放弃投机向量路径的控制信号。回滚只删除水位之后的警告，已有警告保持不变。返回列由调用者拥有，内部表达式对象通过不可变借用调用。

## 依赖与调用关系

向上游看，`pkg/expression/lib.rs` 是模块装配入口；两份 Rust 测试通过 `use crate::builtin_control_vec_generated_kernel::*` 或 `include!("builtin_control_vec_generated.rs")` 构造 21 类签名。RustCodeGraph 的文件节点显示本文件被 crate 大量文件“使用”，但精确符号查询和仓库引用复核表明，生产 Rust 源中没有对这些具体类型/方法的调用；图的文件级“used by”不能当作运行时调用边。

向下游看，泛型核心只调用 `VectorExpression::vec_eval`/`eval_row`、`EvalContext` 的警告方法、`check_column_len` 和 `rollback_speculative_warnings`；标准库提供错误/格式化和 `Box`/`Vec`，三方 crate 仅用于值类型别名。没有 I/O、网络、事务、存储或调度器依赖。

Go 主链的对应入口在 `builtin_control_vec_generated.go`：具体 `builtin*Sig.vecEval*` 由 Go 表达式框架和 `chunk.Chunk` 调用。`pkg/expression/generator/control_vec.go` 是 Go 原始生成器；当前 Rust 生成器 `pkg/expression/generator/control_vec.rs` 的默认产物也是 `.go` 与 `_test.go`。未找到由该生成器直接产出本 `.rs` 文件的路径，所以文件头“由 go generate 产出”应理解为对齐生成式 Go 文件的来源标记，而不能据此断言 Rust 文件可由现有生成命令重建。

## 错误处理与边界

- 强制输入错误：IFNULL 的左列和 IF 的条件列是每行选择所必需，向量错误或列长错误直接返回，不进入回退；相关行为由 `mandatory_first_argument_vector_errors_are_not_hidden_by_fallback` 验证。
- 投机分支错误：CASE 的任意预求列、IFNULL 右列、IF 真/假列发生错误或新增警告时，错误本身被放弃并转入标量路径。这不是吞错：若标量短路后仍需该分支，标量错误仍传播；若分支未被选择，其错误不会污染结果。
- 警告回滚：回退前删除投机阶段新增警告，避免向量预求未选择分支产生 Go 不会产生的可观察副作用；旧警告保留。若回退标量求值本身追加警告，当前实现不会再次回滚。
- 列长度：所有成功返回的向量列必须恰好等于 `rows`。投机分支的错长会触发标量回退；强制列的错长直接报错。默认 `VectorExpression::vec_eval` 按请求行数求值，越界行为由具体实现决定。
- 空输入与空分支：`rows == 0` 自然返回空列；空 CASE 分支加无 ELSE 会产生全 NULL；构造函数不验证 CASE 分支数或 SQL 参数合法性。
- `usize` 容量和索引依赖内存可用性；代码没有显式处理分配失败或极端行数溢出。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务。`VectorExpression<T>: Send + Sync` 以及 `T: Send + Sync` 允许表达式对象安全地跨线程转移/共享，但每次求值需要独占的 `&mut EvalContext`，单次调用本身是同步串行的，不能并发写同一上下文。

子表达式由 `Box` 和容器拥有，随外层 builtin 析构；临时向量与结果使用 Rust 所有权自动释放，没有 Go 版 `bufAllocator.get/put` 的显式池化生命周期。投机求值会先执行所有分支，即使某行最终不选择该分支；因此子表达式应把可观察警告写入给定上下文，并理解错误/警告可能触发整表达式重新按行求值。除警告外的外部副作用没有回滚协议，不应假设会被撤销。

## 与 Go 版本的对应关系

`builtin_control_vec_generated.go` 为 CASE、IFNULL、IF × 七种返回类型生成与本文件同名的 21 类签名和 `fallbackEval*`、`vecEval*`、`vectorized()`。核心语义对应如下：CASE 首真优先、无 ELSE 为 NULL；IFNULL 左非 NULL 优先；IF 的 NULL 条件与零条件均选假分支；投机分支新增警告或报错时先截断警告再标量回退；IFNULL 左参和 IF 条件的向量错误直接返回。

差异也很明确：

- Go 使用真实 `EvalContext`、`chunk.Chunk`、类型专属 `chunk.Column` 和 `bufAllocator`；Rust 使用本文件自定义上下文、`Vec<Option<T>>` 与箱装 trait。
- Go 的标量回退委托既有 `b.eval*`，Rust 在三个泛型核心中直接重现短路循环。
- Go 各类型展开为大段生成代码，Rust 用三个泛型实现加宏包装减少重复。
- Rust 的 `Time`/`Duration`/`BinaryJSON` 是 `chrono`/`serde_json` 类型别名，并非 Go `types.Time`、`types.Duration`、`types.BinaryJSON` 的完整兼容表示；时区、精度、MySQL 特殊时间值、JSON 内部格式等能力不能仅由本文件证明等价。
- Go 生成测试通过 `vecBuiltinControlCases` 对各类型及多种 CASE 参数形态运行通用向量/标量一致性测试；Rust 两份独立测试覆盖主要 NULL/选择规则、类型矩阵、错误与警告，但没有证据表明覆盖了 Go 类型系统的全部边界。

## 扩展指南

新增返回类型时，应同时扩展类型别名/真实类型依赖（如需要）、在三个宏调用区各增加一个签名，并在独立测试文件中覆盖 CASE、IFNULL、IF 的构造、`vectorized()`、NULL、错误与警告回退。若目标是与 Go 生成矩阵一致，还需同步核对 `generator/control_vec.go`、`builtin_control_vec_generated.go` 和 Go 生成测试；不要只添加 Rust 包装名称。

修改回退规则时，最可能涉及三个 `vec_eval`、三个 `fallback_eval`、`rollback_speculative_warnings` 和 `check_column_len`。必须分别考虑强制输入与投机输入，避免把必须暴露的错误错误地隐藏，也避免重复保留未选择分支的警告。回归测试应放在现有独立文件 `builtin_control_vec_generated_test.rs` 或 `builtin_control_vec_generated_8_aster_unit_test.rs`，不要内嵌到生产文件。

若要接入完整 Rust 表达式执行主链，不能只把模块改成公开：还需把本地 `VectorExpression`、`EvalContext` 和值别名适配到 crate 的真实表达式/列/类型体系，并在注册或构造路径建立可检索的调用边。性能上需重点评估全分支预求的峰值内存、克隆变长值的成本，以及与列缓冲池的整合；兼容性上需验证 MySQL 时间、Duration、Decimal 与 BinaryJSON 语义。

本文件自称生成代码，直接手工编辑存在再生成覆盖风险；但现有 Rust 生成器未证明能重建该 `.rs`。扩展前应先确认并补齐真实生成源，或明确把 Rust 文件视为手工维护的移植产物，避免生成源与结果漂移。

## 验证依据

- 源码：`pkg/expression/builtin_control_vec_generated.rs`，重点符号为 `VectorExpression`、`BuiltinCaseWhen`、`BuiltinIfNull`、`BuiltinIf`、`check_column_len`、`rollback_speculative_warnings` 及三个签名宏。
- crate 边界：`pkg/expression/Cargo.toml`（`astersql-expression`、`lib.rs`、`autotests = false`、类型依赖）和 `pkg/expression/lib.rs`（私有模块装配与两份测试模块装配）。目标包未发现 `doc.go`。
- RustCodeGraph：`status` 显示索引包含本文件；`node --file ... --offset 1/500` 读取完整 607 行；`query BuiltinCaseWhen/BuiltinIfNull/BuiltinIf --kind struct` 定位三个泛型核心；`query VectorExpression --kind trait` 与 `query vec_eval --kind function` 区分同名符号。对精确 `callers/callees` 的查询未返回可用边，因此调用关系又用 Rust 引用搜索复核，未把文件级使用计数误作函数调用证据。
- Rust 测试：`pkg/expression/builtin_control_vec_generated_test.rs` 验证首分支、ELSE、NULL、IF/IFNULL 和长度错误；`pkg/expression/builtin_control_vec_generated_8_aster_unit_test.rs` 验证七类型矩阵、投机错误/警告回滚、选中分支标量错误、强制首参错误与全部 21 签名。
- Go 对照：`pkg/expression/builtin_control_vec_generated.go`、`pkg/expression/builtin_control_vec_generated_test.go`；生成源核对 `pkg/expression/generator/control_vec.go` 与 `pkg/expression/generator/control_vec.rs`。Go 测试入口为 `TestVectorizedBuiltinControlEvalOneVecGenerated` 和 `TestVectorizedBuiltinControlFuncGenerated`。
- 接线边界：仓库 Rust 引用搜索仅在上述测试中发现具体包装类型构造；`builtin.rs`、`distsql_builtin.rs`、`builtin_threadsafe_generated.rs` 的同名字符串未形成 Rust 类型调用。这一限制是当前代码事实，不代表未来设计意图。
- 按任务约束，本次是纯文档分析，未运行 Cargo，也未修改 Rust、Go、Cargo 或总计划。
