# `pkg/expression/chunk_executor.rs`

## 文件定位

本文件是 `astersql-expression` crate 的 Chunk 级表达式执行内核，源文件由 [`pkg/expression/lib.rs`](lib.rs) 以 `chunk_executor_kernel` 私有模块装配。它位于表达式对象（`Expression`）、列式容器（`chunk::Chunk`）和上层求值器之间：负责判断一组表达式能否改变为向量化求值顺序，将投影结果写入输出列，并把一组谓词转换为逐物理行的选择位图。`lib.rs` 只把 `VectorizedFilter` 重导出为 crate 公共 API；其余 `pub`/`pub(crate)` 符号主要供 crate 内的求值器和测试使用。

crate 边界由 [`pkg/expression/Cargo.toml`](Cargo.toml) 定义：包名是 `astersql-expression`，库入口为 `lib.rs`，直接使用的 Chunk、类型和解析器常量分别来自工作区依赖 `astersql-util-chunk`、`astersql-types`、`astersql-parser-ast` 与 `astersql-parser-mysql`，在 `lib.rs` 中经 crate 级别别名/重导出后由本文件的 `use crate::*` 引入。

## 核心职责

1. `Vectorizable`、`HasGetSetVarFunc` 和 `checkSequenceFunction` 保守识别跨行有状态表达式，避免向量化改变用户变量与 sequence 函数的求值顺序。
2. `VectorizedExecute`、`evalOneColumn`、`evalOneCell` 以及 `executeTo*` 系列函数执行逐列表达式求值，并按 MySQL 字段类型选择正确的 Chunk 物理表示。
3. `evalOneVec` 调用 `Expression::VecEval*` 批量计算整列，并对 BIT、FLOAT、ENUM、SET 做二次物理转换后原子替换目标列。
4. `VectorizedFilterConsiderNull` 在整批 `VecEvalBool` 与逐行过滤之间选择路径，维护 `selected`/`isNull` 位图，并将结果与输入 Chunk 原有的 selection 相交。
5. `rowBasedFilter` 临时移除输入 selection，使结果位图仍以原 Chunk 的物理行号为下标；`SelectionGuard::drop` 保证正常返回、`Result::Err` 及 panic 展开时都恢复原 selection。

## 主要符号

- `Vectorizable(exprs) -> bool`：先递归拒绝任何 `SetVar`/`GetVar`，再检查顶层 `NEXTVAL`、`LASTVAL`、`SETVAL` 组合。一个 `NEXTVAL` 与 `LASTVAL`/`SETVAL` 并存，或存在多个 `NEXTVAL`，都会返回 `false`；只有 `LASTVAL` 与 `SETVAL` 并存不会触发该规则（`chunk_executor.rs:25-49`）。
- `HasGetSetVarFunc(expr) -> bool`：只对 `ScalarFunction` 递归遍历参数树；非标量节点立即返回 `false`（`chunk_executor.rs:52-63`）。
- `HasAssignSetVarFunc(expr) -> bool`：识别某个 `SetVar` 的直接参数中含标量函数，或其后代中存在同样结构；它是附加语义探测器，不参与本文件的 `Vectorizable` 决策（`chunk_executor.rs:66-82`）。
- `VectorizedExecute(...) -> Result<(), Error>`：按表达式下标映射输出列，依次调用逐行的 `evalOneColumn`；任一列失败即停止，已追加的先前结果不会回滚（`chunk_executor.rs:85-95`）。
- `evalOneVec(...)`：crate 内向量化列执行入口。支持 `ETInt`、`ETReal`、`ETDecimal`、`ETDatetime`、`ETTimestamp`、`ETDuration`、`ETJson`、`ETVectorFloat32` 和 `ETString`，其余求值类型返回 unsupported-type 错误（`chunk_executor.rs:99-192`）。
- `evalOneCell(...)` 与 `executeToInt/Real/Decimal/Datetime/Duration/JSON/VectorFloat32/String`：按单行、单列分派，统一传播表达式错误和 SQL NULL（`chunk_executor.rs:230-428`）。
- `VectorizedFilter(...) -> Result<Vec<bool>, Error>`：忽略 NULL 位图的便捷包装；实际工作委托给 `VectorizedFilterConsiderNull`（`chunk_executor.rs:431-441`）。
- `VectorizedFilterConsiderNull(...)`：当会话允许向量化且所有过滤器的 `Vectorized()` 都为真时选择 `vectorizedFilter`，否则选择 `rowBasedFilter`；完成后与调用前快照的 selection 求交（`chunk_executor.rs:444-479`）。
- `rowBasedFilter`：逐过滤器、逐行短路已经淘汰的行；整数谓词直接调用 `EvalInt`，其他求值类型包装为单元素 `CNFExprs` 后调用 `EvalBool`（`chunk_executor.rs:482-542`）。
- `vectorizedFilter`：克隆过滤表达式组成 `CNFExprs`，调用 `expression_core::VecEvalBool`；调用者没请求 NULL 位图时仍可使用空缓冲参与求值，但返回 `None`（`chunk_executor.rs:545-564`）。

## 执行流程

投影执行有两条主链。旧式入口 `VectorizedExecute` 实际按“表达式列 → iterator 的每一行”执行：`VectorizedExecute → evalOneColumn → evalOneCell → executeTo* → Expression::Eval* → Chunk::Append*`。当前 [`pkg/expression/evaluator.rs`](evaluator.rs) 的 `defaultEvaluator::run` 则先由 `NewEvaluatorSuite` 调用 `Vectorizable` 判断整组是否允许重排；允许时逐表达式选择 `evalOneVec`（会话开启且表达式自身 `Vectorized()`）或 `evalOneCell`，不允许时严格按“行 → 表达式”求值，以保持有副作用表达式的跨列顺序。

`evalOneVec` 先创建临时 `Column`，调用与 `EvalType` 对应的 `VecEval*`。BIT 将整数按字段长度转换为 binary literal；MySQL FLOAT 将 `f64` 缩窄为 `f32`；ENUM/SET 将字符串名称解析为专用对象；其他已支持类型直接保留向量化结果。全部成功后才以 `output.SetCol(colIdx, result)` 替换目标列。

过滤主链为 `VectorizedFilter → VectorizedFilterConsiderNull`。后者先保存 selection 快照，再选择：

- 向量路径：`vectorizedFilter → expression_core::VecEvalBool`，一次处理组合后的 CNF。
- 逐行路径：`rowBasedFilter` 临时清空 selection、重置 iterator，初始化全长位图；每个谓词只评估仍为 `true` 的行，SQL NULL 使该行不通过，并累积到可选 NULL 位图。

成功返回后，如果输入原先有 selection，入口把未出现在 selection 中的物理行强制置为 `false`。因此结果数组的索引是 Chunk 物理行号，而不是 selection 中的压缩位置。

## 数据与状态

- `exprs`/`filters` 持有 `Box<dyn Expression>`；执行阶段通过 trait 的 `GetType`、`Eval*`、`VecEval*`、`Vectorized` 与 `CloneExpr` 访问动态实现。
- `output: &mut chunk::Chunk` 是累积写目标。逐行执行使用 `Append*`，所以调用者必须提供列布局正确且处于预期写入位置的 Chunk；本文件不清空旧数据，也不提供事务式回滚。
- `selected: Vec<bool>` 与 `isNull: Option<Vec<bool>>` 是可复用缓冲。逐行路径会 `clear + resize` 到 `iterator.Len()`；向量路径将缓冲交给 `VecEvalBool`。`None` 明确表示调用者不需要 NULL 位图。
- SQL NULL 独立于值存储：各 `executeTo*` 在 `isNull` 时只追加 null；过滤时 NULL 同时把 `selected[index]` 变为 `false`，并在请求时把 `isNull[index]` 累积为 `true`。
- 特殊物理语义包括：无符号整数保留 `i64` 的底层位模式后以 `u64` 追加；BIT 按 `(flen + 7) >> 3` 字节编码；FLOAT 缩窄至 `f32`；逐行字符串 ENUM/SET 只填名称且数值为零。

## 依赖与调用关系

上游直接证据：

- [`pkg/expression/evaluator.rs`](evaluator.rs) 的 `NewEvaluatorSuite` 调用 `Vectorizable`；`defaultEvaluator::run` 调用 `evalOneVec` 或 `evalOneCell`，构成常规表达式投影主链。
- [`pkg/expression/lib.rs`](lib.rs) 将本文件装配为 `chunk_executor_kernel`，并重导出 `VectorizedFilter`；仓库其他 crate 应经该重导出使用过滤 API。
- [`pkg/expression/chunk_executor_test.rs`](chunk_executor_test.rs) 直接调用 `VectorizedFilterConsiderNull` 验证异常展开后的 selection 恢复；[`pkg/expression/builtin_32_aster_unit_test.rs`](builtin_32_aster_unit_test.rs) 覆盖向量化判定、selection/NULL 相交和无符号位模式。

RustCodeGraph 的精确节点证实：`VectorizedExecute → evalOneColumn`；`VectorizedFilterConsiderNull → rowBasedFilter | vectorizedFilter`，并由 `VectorizedFilter` 与 selection 恢复测试调用；`rowBasedFilter → EvalBool`；`evalOneVec → Expression::VecEvalInt/Real/String/Decimal/Time/Duration/JSON/VectorFloat32`。索引对 `vectorizedFilter → expression_core::VecEvalBool` 没有解析出静态边，因此该边以源文件 `chunk_executor.rs:555-562` 为直接证据。

下游依赖集中于 `Expression` trait、`EvalContext`、`types::FieldType`/解析函数、MySQL 类型标志、AST 函数名常量，以及 `chunk::Chunk`/`Iterator4Chunk`/`Column`。本文件不访问存储、网络或会话锁。

## 错误处理与边界

所有 `Eval*`、`VecEval*` 和 `ParseEnumValue` 错误均用 `?` 原样向上传播；未知 `EvalType` 转换为包含具体类型的 `errors::New("unsupported type ... during evaluation")`。投影写入不是原子的：逐行失败时目标列可能已有前缀，多个表达式时先前列可能已完成；调用者不得把错误后的输出当完整结果。

向量化 ENUM/SET 名称解析采用 `unwrap_or_default()`：非法名称不使整列失败，而是追加默认对象。Go 版本同样继续执行并追加解析所得零值，但会写 debug 日志；Rust 当前没有对应日志，这是可观测性差异。逐行整数 ENUM 通过 `ParseEnumValue(...)?`，非法数值会失败；逐行字符串 ENUM/SET 则保存名称并将数值置零，与 Go 保持一致。

`VectorizedFilterConsiderNull` 在内部分支返回 `Err` 时返回 `(Vec::new(), None, Some(err))`，不会保留分支可能形成的部分位图；Go 版本返回当时的 `selected/isNull` 与错误。这是接口形态和失败态内容差异，调用者只能依赖 `Some(err)`，不能依赖失败时的位图。逐行路径的 `SelectionGuard` 会在 `?` 提前返回和 panic 展开时恢复 selection；进程 abort 不执行析构，不在此保证内。

边界上，空过滤器会产生全 `true` 的逐行位图或由 `VecEvalBool` 定义的空 CNF 结果；输入 selection 中的下标默认合法，本文件直接用其索引位图。输出列数量、类型和容量也由调用者建立，本文件没有显式前置检查。

## 并发与资源生命周期

函数全部同步执行，不创建线程、异步任务、通道、锁或事务。可变借用保证同一次调用对 iterator/output 的排他访问；表达式以共享借用读取，但 `EvalContext` 和具体表达式实现可能包含内部状态，因此是否可并行不能由本文件推出。

临时列、CNF 克隆和位图均由 Rust 所有权在函数返回时释放。`rowBasedFilter` 是唯一含显式资源恢复协议的部分：保存 selection 的拥有型副本，清空原 selection 并重置 iterator，最后由栈上 `SelectionGuard` 的 `Drop` 把副本移回 Chunk。守卫使用裸指针绕过同时持有 iterator 与 Chunk 可变引用的借用冲突；其安全前提写在源码中：调用期间 iterator 被排他借用，且 iterator 保证 Chunk 存活。

## 与 Go 版本的对应关系

直接对照文件为 [`pkg/expression/chunk_executor.go`](chunk_executor.go)。Rust 保留了 Go 的函数分组、类型分派、sequence 判定、变量函数递归检查、投影物理转换、过滤路径选择、NULL 语义以及 selection 求交规则。`evaluator.rs` 对应 Go 的 [`pkg/expression/evaluator.go`](evaluator.go)，说明本文件的内部入口如何进入常规 `EvaluatorSuite` 执行链。

已核实差异：

- Go `evalOneVec` 直接使用 `output.Column(colIdx)`，特殊类型再 `SetCol`；Rust 总是在临时默认列求值，成功后统一 `SetCol`，失败时不会把临时列装入输出。
- Go 的逐类型 `evalOneColumn` 各自循环；Rust 抽取为统一循环并委托 `evalOneCell`，支持类型集合保持一致。
- Go 用 `defer input.SetSel(input.Sel())` 恢复 selection；Rust 用 `SelectionGuard::Drop`，额外由 Rust 测试验证 panic 展开路径。
- Go 非法向量 ENUM/SET 名称写 debug 日志后继续；Rust 静默追加默认值。Go 的错误返回可携带部分位图，而 Rust 过滤入口在错误时清空位图。
- Rust 用 `Option<Vec<bool>>` 区分“不请求 NULL 位图”，对应 Go 的 `nil []bool`；Rust 为动态表达式克隆构造 `CNFExprs`，对应 Go 直接传递表达式切片。

## 扩展指南

- 新增 `EvalType` 时，至少同步修改 `evalOneVec`、`evalOneColumn` 的支持集合、`evalOneCell` 分派和相应 `executeTo*`；同时核对 `Expression` trait 的标量/向量接口及 Go `chunk_executor.go`，避免只支持一条路径。测试应放在独立的 `*_test.rs` 文件，不能内嵌到生产源文件。
- 改变 MySQL 特殊类型表示时，同时检查 BIT 字节宽度、FLOAT 精度、ENUM/SET 名称和值、unsigned 位模式及 NULL；优先扩展 [`pkg/expression/builtin_32_aster_unit_test.rs`](builtin_32_aster_unit_test.rs) 或新增同目录独立测试。
- 新增有跨行副作用的函数时，应评估 `Vectorizable`、`HasGetSetVarFunc` 与 `checkSequenceFunction`；若向量化会改变调用顺序，必须加入保守拒绝规则，并补充“允许/拒绝”成对用例。
- 修改过滤逻辑时必须保持三项不变量：位图按物理行索引、SQL NULL 不通过谓词、原 selection 在任何退出路径恢复。异常恢复回归位于 [`pkg/expression/chunk_executor_test.rs`](chunk_executor_test.rs)，selection/NULL 与 Go 行为对照位于 [`pkg/expression/builtin_32_aster_unit_test.rs`](builtin_32_aster_unit_test.rs) 和 Go 的 [`pkg/expression/builtin_vectorized_test.go`](builtin_vectorized_test.go)。
- 性能风险主要来自每行动态分派、非整数过滤器反复克隆为单元素 CNF、向量特殊类型的二次分配，以及 selection 快照/全长位图。优化时不能以压缩 selection 下标替代物理下标，也不能牺牲错误路径恢复。

## 验证依据

- 源码全量阅读：[`pkg/expression/chunk_executor.rs`](chunk_executor.rs)，符号范围 `Vectorizable` 至 `vectorizedFilter`。
- crate 与装配：[`pkg/expression/Cargo.toml`](Cargo.toml)、[`pkg/expression/lib.rs`](lib.rs)、[`pkg/expression/evaluator.rs`](evaluator.rs)。`pkg/expression` 下不存在 `doc.go`，因此没有额外包契约文件。
- Go 对照：[`pkg/expression/chunk_executor.go`](chunk_executor.go)、[`pkg/expression/evaluator.go`](evaluator.go)、[`pkg/expression/builtin_vectorized_test.go`](builtin_vectorized_test.go)。
- Rust 测试：[`pkg/expression/chunk_executor_test.rs`](chunk_executor_test.rs)、[`pkg/expression/builtin_32_aster_unit_test.rs`](builtin_32_aster_unit_test.rs)、[`pkg/expression/evaluator_test.rs`](evaluator_test.rs)。它们分别证明异常恢复、向量化/selection/物理类型边界，以及 `EvaluatorSuite` 的上游接线。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；精确 `node` 查询了 `VectorizedExecute`、`VectorizedFilterConsiderNull`、`evalOneVec`、`rowBasedFilter`、`vectorizedFilter`，其调用边已在“依赖与调用关系”列出。路径过滤未返回该文件且自然语言 `explore` 命中噪声较多，故未以这些不稳定结果支持结论。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务给定命令校验目标文件存在且恰有 11 个固定二级标题，并人工复核唯一新增产物、链接目标、符号名和 Go/Rust 差异。
