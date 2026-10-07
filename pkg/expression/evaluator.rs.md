# `pkg/expression/evaluator.rs`

## 文件定位

`evaluator.rs` 属于 `astersql-expression` crate。crate 根文件 `pkg/expression/lib.rs` 用 `#[path = "evaluator.rs"] mod evaluator_kernel;` 将它编译为私有子模块；同一根文件只在 `cfg(test)` 下装入独立测试 `pkg/expression/evaluator_test.rs`。模块没有在 `lib.rs` 中公开再导出，因此文件内虽然有 `pub` 符号，它们仍受私有父模块限制，当前不能作为 crate 外 API 访问。

这个文件负责“一组表达式对一个输入 `Chunk` 的批量投影”：纯 `Column` 表达式可交给 `chunk::ColumnSwapHelper` 复用列存储，其他表达式由内部 `defaultEvaluator` 选择向量或逐单元格求值。它位于表达式树（`Expression`、`ScalarFunction`）与底层 chunk 求值内核（`chunk_executor.rs`）之间。

`pkg/expression/Cargo.toml` 声明包名为 `astersql-expression`、库入口为 `lib.rs`，关闭自动测试发现和 doctest，并以 `[package.metadata.porting].go-package = "pkg/expression"` 指向 Go 对照包。目标文件直接使用 crate 根再导出的 expression、context、error、type、chunk API；chunk 实现来自 path 依赖 `astersql-util-chunk`，可选求值属性来自 path 依赖 `astersql-expression-exprctx`。本模块没有独立 feature gate。

当前接线状态需要特别注意：精确 Rust 引用搜索只找到 `lib.rs` 的模块声明和 `evaluator_test.rs` 的测试使用，没有找到 Rust 生产模块对 `NewEvaluatorSuite`、`EvaluatorSuite` 或 `GetOptionalEvalPropsForExpr` 的调用。相对地，Go 同名实现已被 executor projection/expand、detach 和 mock coprocessor 等路径使用。因此本文描述的是已编译、已有单元测试的 Rust 实现，不把 Go 生产接线等同于 Rust 已接线事实。

## 核心职责

1. `NewEvaluatorSuite` 将输出表达式按原始输出位置分流：允许优化的纯列引用进入列映射，其余表达式进入默认求值器。
2. `defaultEvaluator::run` 根据整组表达式的顺序约束、会话的向量化开关和单个表达式的 `Vectorized()` 能力，在整列求值与逐单元格求值之间选择。
3. `EvaluatorSuite::Run` 强制先完成非列表达式，再交换/复用输入列，避免列交换提前改变输入 chunk 后破坏其他表达式的读取。
4. `GetOptionalEvalPropsForExpr` 递归遍历 `ScalarFunction` 参数树，汇总函数及其所有标量函数子节点要求的 optional eval property 位集；`EvaluatorSuite` 再对所有默认表达式取并集。
5. `Vectorizable` 对外报告默认求值器是否满足跨表达式的向量化/重排约束；只有列快路径或空套件时也返回 `true`。

## 主要符号

- `defaultEvaluator { output_idxes, exprs, vectorizable }`：私有执行器。两个向量以相同顺序一一对应，保存“表达式应写入哪个输出列”；`vectorizable` 是对整组非列表达式调用 `chunk_executor_kernel::Vectorizable` 的结果。
- `defaultEvaluator::run(&self, ctx, vec_enabled, input, output)`：执行非列表达式。返回 `Result<(), Error>`，第一个错误立即终止。
- `defaultEvaluator::RequiredOptionalEvalProps()`：对 `exprs` 中每棵表达式树调用 `GetOptionalEvalPropsForExpr`，以位或合并结果。
- `GetOptionalEvalPropsForExpr(expr: &dyn Expression)`：公开于私有模块的递归函数。只有能下转为正式 `ScalarFunction` 的节点贡献属性：先取当前 `builtinFunc::RequiredOptionalEvalProps()`，再递归其 `GetArgs()`；常量、列以及其他表达式类型返回空集。
- `EvaluatorSuite { pub ColumnSwapHelper, default_evaluator }`：组合列交换快路径与默认求值器。列 helper 可由同 crate、能访问该私有模块的代码观察；默认求值器保持封装。
- `NewEvaluatorSuite(exprs, avoid_column_evaluator)`：构造入口。`avoid_column_evaluator = true` 时不做列分流，连纯列引用也交给默认求值器。
- `EvaluatorSuite::Vectorizable()`：不存在默认求值器时为 `true`，否则返回其整组判定结果。
- `EvaluatorSuite::Run(...)`：套件执行入口；按“默认求值器在前、列交换在后”的固定顺序运行。
- `EvaluatorSuite::RequiredOptionalEvalProps()`：只汇总默认求值器；被分流的纯列节点本身不要求 optional eval property，所以没有遗漏。

文件没有模块级常量、trait、条件编译项或内嵌测试；测试保持在独立文件 `pkg/expression/evaluator_test.rs`。

## 执行流程

构造阶段：

1. `NewEvaluatorSuite` 以表达式数为容量建立 `defaultEvaluator`，并创建 `HashMap<输入列下标, 输出列下标列表>`。
2. 按输入表达式顺序枚举输出位置。若未禁止列优化且节点可下转为 `Column`，就把该 `Column.Index` 转为 `usize` 并记录输出位置；否则把表达式及其输出位置追加到默认求值器的两个平行向量。
3. 没有非列表达式时，`default_evaluator` 保存为 `None`。否则调用 `chunk_executor_kernel::Vectorizable(&exprs)`：该函数拒绝 GetVar/SetVar 等跨行状态函数，并检查 sequence function 的组合约束，决定能否改变原始逐行求值次序。
4. 列映射为空时不创建 helper；否则调用 `chunk::ColumnSwapHelper::New`。同一输入列映射到多个输出位置时，helper 会让多个输出列共享同一底层列引用。

运行阶段：

1. `EvaluatorSuite::Run` 先调用 `defaultEvaluator::run`。如果整组 `vectorizable` 为 `true`，执行次序以表达式为外层：当 `vec_enabled && expression.Vectorized()` 时调用 `evalOneVec` 一次生成整列，否则对每个输入行调用 `evalOneCell`。
2. 如果整组 `vectorizable` 为 `false`，执行次序改为“行在外、表达式在内”，逐行按原始表达式顺序调用 `evalOneCell`。这个次序用于保留状态函数或 sequence function 的跨表达式语义，不能简单改成表达式在外。
3. 任一默认表达式失败即通过 `?` 返回；此时列交换不会执行。
4. 默认表达式全部成功后，若有 `ColumnSwapHelper`，调用 `SwapColumns(input, output)`。helper 首次运行会合并输入中本来就共享底层列的映射，交换首个输出列，再用 `MakeRef` 让其他输出位置引用它。
5. 列交换成功后返回 `Ok(())`。交换会改变输入 chunk 的列内容，所以这一步必须保持在最后。

optional property 流程独立于实际求值：从套件中的每个非列表达式开始，遇到 `ScalarFunction` 就把当前函数要求的位集与所有参数子树的位集做按位或；遇到非标量节点立即返回空集。

## 数据与状态

- `output_idxes[i]` 与 `exprs[i]` 是强关联的不变量。二者只在构造时由同一分支同时追加，运行时用 `zip` 读取；扩展构造逻辑时必须继续保持长度和顺序一致。
- `column_mapping` 保存从输入列到一个或多个输出列的位置关系。重复引用同一输入列不会重复求值，而是由 `ColumnSwapHelper` 在首个输出列完成交换后为其他输出列建立共享引用。
- `vectorizable` 不是“所有表达式一定走向量 API”的意思。它只说明整组可以按表达式分批处理；单个节点仍需同时满足运行开关 `vec_enabled` 和 `Expression::Vectorized()`，否则走逐单元格路径。
- `OptionalEvalPropKeySet` 在 `exprctx/optional.rs` 中是 `u64` 新类型。本文件直接对内部位值 `.0` 做按位或，默认值表示空集。
- 套件借用调用者提供的 `EvalContext` 和输入/输出 chunk，不持有会话、行迭代器或 chunk 的长期引用。表达式对象由 `Vec<ExprBox>` 所有，并随套件生命周期释放。
- 输出 chunk 必须由调用者按目标类型和列数准备好；本文件按原表达式位置写入对应列，不负责从表达式列表构造输出 schema。

## 依赖与调用关系

- 装配上游：`pkg/expression/lib.rs` 的私有 `evaluator_kernel` 模块声明。当前 Rust 生产源码没有进一步引用；`pkg/expression/evaluator_test.rs` 通过 `crate::evaluator_kernel::{...}` 直接测试构造、执行与属性收集。
- 表达式输入：crate 根的 `ExprBox` / `Expression`、`Column`、`ScalarFunction` 和 `EvalContext`。`Expression::as_any()` 支持列和标量函数下转，`ScalarFunction::GetArgs()` 提供递归边。
- 执行下游：`pkg/expression/chunk_executor.rs` 的 `Vectorizable`、`evalOneVec` 和 `evalOneCell`。前者保护状态/sequence 函数顺序；后两者根据 `FieldType::EvalType()` 分派到整列或标量类型化求值，并传播表达式错误。
- 列快路径：`pkg/util/chunk/chunk_util.rs` 的 `ColumnSwapHelper::New`、`SwapColumns`、`mergeInputIdxToOutputIdxes`，以及 chunk 的列交换和 `MakeRef` 机制。
- 错误边界：crate 根 `Error` 和 `errors::New`。底层表达式/类型求值错误原样向上传播；列交换的 chunk error 在本文件中转成 expression error 文本。
- Cargo 边界：直接相关的 path 依赖是 `chunk-dependency` 与 `exprctx-dependency`；Expression、types 和 parser/mysql 等由 crate 根统一组织。manifest 没有为本文件声明 feature。
- Go 生产上游：`pkg/executor/builder.go` 构造 projection/expand 的 suite，`pkg/executor/projection.go` 持有并执行它，`pkg/executor/detach.go` 查询 optional props，`pkg/store/mockstore/unistore/cophandler/mpp_exec.go` 构造禁用列优化的 suite。这些是 Go 对照链证据，不是 Rust 已接线证据。

RustCodeGraph 能定位目标符号和内部边，例如 `EvaluatorSuite::Run -> defaultEvaluator::run`，以及 `defaultEvaluator::run -> evalOneVec/evalOneCell` 的源码目标；但本次 `callers` 返回为空且部分 `callees` 缺失。跨文件接线因此用精确 Rust/Go 引用搜索补齐，不能从空图边推断不存在 Go 调用。

## 错误处理与边界

- 构造列快路径时，`Column.Index` 通过 `usize::try_from(...).expect(...)` 转换。未解析或负下标会直接 panic，而不是返回 `Result`；调用者必须先完成列索引解析。设置 `avoid_column_evaluator = true` 会绕开这个构造分支，但不等于修复无效列索引。
- `evalOneVec` / `evalOneCell` 返回的函数错误、类型错误或不支持的 eval type 由 `?` 立即传播。默认求值器没有回滚，错误前已经写入的输出列/单元格可能保留，调用者只应在 `Ok(())` 后把输出视为完整结果。
- 默认求值失败时列交换尚未发生，因此输入列不会被本文件的列快路径掏空；列交换自身失败时则可能已有默认输出，而且应以底层 chunk 实现的错误边界为准。
- `SwapColumns` 的错误被 `map_err(|error| errors::New(error.to_string()))` 重新包装，只保留展示文本，不保留底层错误类型。
- `GetOptionalEvalPropsForExpr` 只递归正式 `ScalarFunction`。新增带子表达式的其他 Expression 容器若也会要求 optional properties，当前函数不会自动遍历它，必须显式扩展并增加回归测试。
- 空表达式列表、只有列快路径、只有默认表达式以及混合套件都合法。空套件没有 helper/default evaluator，`Vectorizable()` 为 `true`，`Run()` 是空操作，required props 为空。
- 本文件不校验输出列数、输出类型或容量；这些前置条件由 chunk API 和上游构造者承担。

## 并发与资源生命周期

`EvaluatorSuite::Run` 自身不创建线程、任务、锁或通道，也不修改表达式列表；它通过 `&self` 读取套件，并独占借用 `&mut input`、`&mut output`，所以同一次调用期间 Rust 借用规则阻止其他安全代码并发修改这两个 chunk。能否跨线程共享 suite 还取决于其中 `Expression` trait objects 和 `ColumnSwapHelper` 的实现约束；当前 Rust 生产调用链尚未接入，本文不据此声称已有多 worker 共享行为。

`ColumnSwapHelper` 内部含 `atomic::Pointer<HashMap<...>>`，第一次 `SwapColumns` 时惰性计算输入列别名合并结果，并以 compare-and-swap 发布；若其他 worker 已发布，当前结果不覆盖它。这是 helper 的并发缓存语义，而不是 `EvaluatorSuite` 自己维护的锁。缓存绑定 helper 生命周期并供后续运行复用，因此同一个 suite 预期面对结构兼容的输入 chunk；若扩展调用模式，应同步核对 `pkg/util/chunk/chunk_util.rs` 的缓存前提。

列交换是所有权/引用生命周期中的关键副作用：首个输出列与输入列交换，后续输出列引用首个输出列，因此 input 可能被“掏空”，多个 output 列也可能共享同一底层数据。调用者不能假设运行后 input 仍保留原值，也不能在不了解 chunk 写时行为的情况下独立修改共享输出列。

表达式、索引向量和 helper 都由 suite 持有；`EvalContext` 只在调用期间借用。错误不会启动后台清理任务，部分输出的处置责任仍在调用者。

## 与 Go 版本的对应关系

- Rust `defaultEvaluator`、`EvaluatorSuite`、`NewEvaluatorSuite`、`Vectorizable`、`Run`、`RequiredOptionalEvalProps` 与 `GetOptionalEvalPropsForExpr` 逐一对应 `pkg/expression/evaluator.go` 的同名概念，保持“列/其他表达式分流、其他表达式先执行、属性递归取并集”的主要结构。
- Go 的 `defaultEvaluator.run` 在整组可重排但单个表达式不走向量路径时调用 `evalOneColumn`；Rust 直接对 `0..input.NumRows()` 调用 `evalOneCell`。对普通无 selection 的 chunk，两者都是逐行填充该输出列；若未来 Rust chunk 引入/启用 Go iterator 的 selection 语义，需要重新核对这一差异。
- Go 使用内嵌 `*defaultEvaluator`，Rust 使用 `Option<defaultEvaluator>` 并提供显式转发方法；空/纯列 suite 的外部语义相同。
- Go 列索引是 `int` map key；Rust helper 要求 `usize`，因此 Rust 新增了负数/未解析索引的 panic 边界。
- Go 直接返回 `ColumnSwapHelper.SwapColumns` 的错误；Rust将其字符串化后包装成 crate `Error`。正常执行次序一致。
- Go `TestOptionalProp` 验证父子标量函数与兄弟表达式的属性并集；Rust `optional_properties_are_collected_recursively_across_the_suite` 保留这一意图。Rust 独立测试还明确覆盖纯列交换与共享引用、禁用列优化后保留输入、常量的向量/标量一致性、空 suite 和非标量属性为空。
- 接线成熟度不同：Go suite 已处在 executor 和 mock coprocessor 生产链；Rust 当前只有私有模块编译和 crate 内测试证据。扩展文档或架构图时应标注这一迁移状态。

## 扩展指南

1. 新增求值策略时优先修改 `defaultEvaluator::run`，并保持两种执行顺序：可安全重排时可以表达式为外层；存在 GetVar/SetVar 或 sequence 约束时必须行优先。不要只根据单个 `Expression::Vectorized()` 决定整组顺序。
2. 改变列优化判定时修改 `NewEvaluatorSuite`，继续维护 `exprs`/`output_idxes` 平行不变量，并保留默认求值先于 `SwapColumns`。若支持新的列式门面，需证明它和 `Column` 一样没有 optional property 且交换不会破坏尚未执行的表达式。
3. 扩展 optional property 收集时修改 `GetOptionalEvalPropsForExpr`。如果新增可包含子表达式的非 `ScalarFunction` 节点，应明确其递归规则，避免漏报构建 EvalContext 所需的 provider。
4. 若要把 suite 接入 Rust executor，应先在 `pkg/expression/lib.rs` 设计受控的 crate/public 导出，再在 executor adapter 中接线；不能仅因为函数标记 `pub` 就假设 crate 外可见。接线还需核对 `pkg/executor/projection.rs` 的泛型 `EvaluatorSuite` backend 契约。
5. 修改列交换共享或并发行为前，同时检查 `pkg/util/chunk/chunk_util.rs::{ColumnSwapHelper, SwapColumns, mergeInputIdxToOutputIdxes}`；它的惰性缓存和输入列别名合并属于不可忽略的下游契约。
6. 测试继续放在独立 `pkg/expression/evaluator_test.rs`，不要内嵌到源文件。至少覆盖混合列/非列 suite、错误后的输入/输出状态、非向量化状态函数的顺序、无效列索引边界，以及新增表达式容器的 optional props。
7. 与 Go 对齐时同步检查 `pkg/expression/evaluator.go` 和 `pkg/expression/evaluator_test.go::TestOptionalProp`，保留分支和错误语义；不得以只覆盖常量或空 suite 的简化实现代替生产行为。

性能风险主要来自把可安全整列执行的表达式退化为逐单元格循环、破坏列引用复用，或对同一表达式树重复递归收集属性。正确性风险集中在求值顺序、列交换时机、无效列下标以及错误后的部分输出；兼容性风险集中在 Go iterator/selection 语义和 Rust 模块对外暴露边界。

## 验证依据

- RustCodeGraph 索引：`rustcodegraph status` 显示本仓库已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`node pkg/expression/evaluator.rs --file ...` 核对了目标文件全部 194 行与真实符号。
- RustCodeGraph 符号/调用查询：对 `defaultEvaluator`、`EvaluatorSuite`、`NewEvaluatorSuite`、`GetOptionalEvalPropsForExpr` 执行 `query`；对构造、运行、向量化和属性函数执行 `callers`/`callees`。图确认内部 `EvaluatorSuite::Run -> defaultEvaluator::run`，并定位 `chunk_executor.rs` 的 `Vectorizable`、`evalOneVec`、`evalOneCell` 以及 `chunk_util.rs` 的 `ColumnSwapHelper`、`SwapColumns`、`mergeInputIdxToOutputIdxes`。图的 callers 为空且部分 callees 缺失，故未把空结果作为“无调用”的唯一证据。
- Rust 装配与精确引用：读取 `pkg/expression/lib.rs` 的 `evaluator_kernel` 和 `evaluator_test` 声明；用 `rg` 搜索 Rust 全仓 `evaluator_kernel|NewEvaluatorSuite|GetOptionalEvalPropsForExpr`，确认除目标文件外只有模块声明与独立测试引用，当前未发现 Rust 生产调用者。
- crate 边界：读取 `pkg/expression/Cargo.toml`，核对库入口、`autotests = false`、path 依赖、无独立 feature，以及 porting metadata。目标包不存在 `pkg/expression/doc.go`。
- Go 对照：读取 `pkg/expression/evaluator.go` 全部实现；精确搜索并核对 `pkg/executor/builder.go`、`projection.go`、`detach.go` 与 `pkg/store/mockstore/unistore/cophandler/mpp_exec.go` 的生产调用位置。
- 测试证据：读取完整 `pkg/expression/evaluator_test.rs`，覆盖列交换/共享引用、禁用优化、向量/标量常量一致性、空 suite、非标量属性和递归属性并集；读取 `pkg/expression/evaluator_test.go::TestOptionalProp` 核对 Go 属性语义。测试文件与源文件保持分离。
- 本任务只新增说明文档，按计划不运行 Cargo。交付时执行固定十一章节结构检查，并人工复核本文能够回答文件为何存在、实际如何运行、当前接线程度以及如何安全扩展。
