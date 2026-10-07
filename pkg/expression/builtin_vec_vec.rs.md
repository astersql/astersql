# `pkg/expression/builtin_vec_vec.rs`

## 文件定位

本文件是 `astersql-expression` crate 的向量（`VectorFloat32`）内置函数列式求值实现。crate 根在 [`pkg/expression/lib.rs`](lib.rs) 中用 `#[path = "builtin_vec_vec.rs"] mod builtin_vec_vec_kernel;` 私有挂载它，并在 `builtin_vec` 门面中再导出其实现；相邻的 [`pkg/expression/builtin_vec.rs`](builtin_vec.rs) 定义同一组签名类型、函数类和标量求值路径。它处于“表达式子节点批量求值 → 本文件逐行执行向量运算 → 写入结果 `chunk::Column`”这条执行链中，不负责 SQL 名称注册、参数类型推断或标量实现。

[`pkg/expression/Cargo.toml`](Cargo.toml) 将该目录声明为 `astersql-expression` 库（入口为 `lib.rs`，`autotests = false`）。本文件直接使用 crate 内的表达式抽象，并通过 `chunk-dependency`、`contextutil-dependency` 和 `types-dependency` 的根模块再导出访问列式批、共享错误和向量类型。

## 核心职责

- 为 `VEC_DIMS` 批量返回每行向量维度。
- 通过统一的 `vectorizedDistance` 模板执行 `VEC_L1_DISTANCE`、`VEC_L2_DISTANCE`、`VEC_NEGATIVE_INNER_PRODUCT` 和 `VEC_COSINE_DISTANCE`，避免四份相同的参数求值、NULL 传播和结果写入代码。
- 为 `VEC_L2_NORM` 批量计算单向量的二范数。
- 为 `VEC_FROM_TEXT` 完成字符串列到 `VectorFloat32` 列的解析与返回列维度约束检查；为 `VEC_AS_TEXT` 完成反向格式化。
- 保持 SQL NULL 语义：输入 NULL 逐行产生 NULL；距离或范数结果为 NaN 时也写成 NULL。该行为由 [`pkg/expression/builtin_vec_vec_test.rs`](builtin_vec_vec_test.rs) 和 [`pkg/expression/builtin_vec_vec_31_aster_unit_test.rs`](builtin_vec_vec_31_aster_unit_test.rs) 覆盖。

本文件没有 `vectorized() -> bool` 判定；是否走列式路径由表达式框架和签名分派决定。它只提供签名类型上的 `vecEval*` 方法。

## 主要符号

- `evaluateVectorArgument(argument, ctx, input) -> Result<chunk::Column, Error>`：分配一个临时列，调用子表达式的 `VecEvalVectorFloat32`，成功后返回整列。距离、维度、范数和向量转文本共享这个入口。
- `vectorizedDistance(base, ctx, input, result, distance)`：双向量距离的内部骨架。它先完整求值左右子表达式，再把结果列重置为 `Float64`，然后按 `input.NumRows()` 逐行传播 NULL、调用传入的距离闭包，并把 NaN 转成 NULL。
- `vectorized_distance!($signature, $method)`：为签名类型生成 `vecEvalReal`。文件内四次展开分别绑定 `L1Distance`、`L2Distance`、`NegativeInnerProduct`、`CosineDistance`。
- `builtinVecDimsSig::vecEvalInt`：将 `VectorFloat32::Len()` 转为 `i64` 写入整数结果列。
- `builtinVecL2NormSig::vecEvalReal`：调用 `VectorFloat32::L2Norm()`，并处理 NULL/NaN。
- `builtinVecFromTextSig::vecEvalVectorFloat32`：调用子表达式 `VecEvalString`、`types::ParseVectorFloat32` 和 `CheckDimsFitColumn(return_type.GetFlen())`，再追加向量结果。
- `builtinVecAsTextSig::vecEvalString`：调用 `VectorFloat32::String()`，把非 NULL 向量追加到字符串结果列。

这些方法是对应签名类型的固有方法而不是新 trait 实现；签名类型和 `baseBuiltinFunc` 字段来自 `builtin_vec` 标量内核。

## 执行流程

距离函数的主流程如下：

1. `vectorized_distance!` 生成的 `vecEvalReal` 把签名的 `baseBuiltinFunc`、上下文、输入批、结果列和具体向量算法传给 `vectorizedDistance`。
2. `vectorizedDistance` 用 `evaluateVectorArgument` 依次批量求值第 0、1 个参数。任一子表达式返回错误时立即以 `?` 退出，此时不会进入逐行距离计算。
3. `ResizeFloat64(0, false)` 清空并设置结果列类型；循环边界由输入批的 `NumRows()` 决定。
4. 任一参数列当前行是 NULL 时追加 NULL；否则取得两个 `VectorFloat32` 引用并执行具体距离方法。
5. 距离方法的错误（例如维度不匹配）立即传播；成功结果若为 NaN（典型例子是零向量余弦距离）则追加 NULL，否则追加 `f64`。

其余函数采用同样的“先整列求子表达式、再按输入行数处理”模式。`vecEvalInt` 写入维度；`vecEvalReal` 写入范数；`vecEvalVectorFloat32` 解析并校验每个字符串；`vecEvalString` 格式化每个向量。常量表达式由子表达式的向量化接口广播到输入的虚拟行数，测试以三行虚拟 `Chunk` 验证这一点。

## 数据与状态

输入状态由只读的 `EvalContext` 和 `chunk::Chunk` 提供；签名对象只读取 `baseBuiltinFunc.args` 与 `baseBuiltinFunc.return_type`。输出状态全部写入调用方提供的可变 `chunk::Column`：

- 维度结果用 `ResizeInt64(0, false)` 初始化后，通过 `AppendInt64`/`AppendNull` 逐行追加。
- 距离和范数用 `ResizeFloat64(0, false)` 初始化后，通过 `AppendFloat64`/`AppendNull` 追加。
- 文本转向量用 `ReserveVectorFloat32(input.NumRows())` 预留容量；向量转文本用 `ReserveString(input.NumRows())` 预留容量。
- 临时参数列是函数栈上的局部所有权值，函数返回时释放；本文件没有全局缓存、静态可变状态或跨批次状态。

结果列每个输入行恰好追加一个值或 NULL。该“输出行数等于 `input.NumRows()`”不变量依赖子表达式生成同样行数的临时列；本文件不另行检查临时列长度。

## 依赖与调用关系

上游方面，`lib.rs` 挂载本模块；表达式执行框架通过具体签名的 `vecEvalInt`、`vecEvalReal`、`vecEvalVectorFloat32` 或 `vecEvalString` 方法完成批量分派。RustCodeGraph 的调用流还确认 `ScalarFunction::EvalVectorFloat32` 经表达式内核的 `evalVectorFloat32`/`vecEvalVectorFloat32` 进入具体签名实现。由于多个签名共享 `vecEval*` 名称，索引对重载方法的通用 callers 查询存在歧义；本文只把文件内精确符号和已核对的框架链作为证据，不把同名搜索结果当成独占调用者。

下游方面：

- `evaluateVectorArgument` 调用 `ExprBox::VecEvalVectorFloat32`。
- 距离模板调用 `VectorFloat32::{L1Distance,L2Distance,NegativeInnerProduct,CosineDistance}`。
- 范数调用 `VectorFloat32::L2Norm`，维度调用 `VectorFloat32::Len`。
- 文本入口调用 `ExprBox::VecEvalString`、`types::ParseVectorFloat32`、`VectorFloat32::CheckDimsFitColumn`；文本出口调用 `VectorFloat32::String`。
- 所有实现依赖 `chunk::Column` 的类型化重置、预留、NULL 检查、读取和追加 API。

RustCodeGraph 对 `vectorizedDistance` 的精确 callee 查询给出到 `evaluateVectorArgument` 的调用边；文件级索引还显示目标实现被向量相关独立测试使用。

## 错误处理与边界

所有可失败步骤均返回 `Result<_, Error>` 并用 `?` 原样向上传播：子表达式批量求值、距离算法、文本解析和维度约束检查。没有吞错、告警降级或部分错误恢复。错误可能发生在已经向结果列追加若干行之后，因此调用者不应在 `Err` 后读取结果列为完整批次；框架应把整次求值视为失败。

主要边界如下：

- SQL NULL 不调用底层数值/解析/格式化逻辑，直接生成 NULL。
- NaN 只在距离与范数路径被规范化为 SQL NULL；正常有限值（包括负内积结果）直接写出。
- 双向量维度不一致由底层距离方法报错；本文件不预先比较维度。
- `VEC_FROM_TEXT` 先解析文本，再用返回类型的 `flen` 校验向量是否适合目标列。非法文本或维度不符都会终止整个批次。
- 空输入批会生成空结果，不进入循环。参数数量和参数/返回类型正确性由 `builtin_vec` 的函数类构造阶段负责，不在本文件重复验证。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。每次调用只操作传入的只读上下文/输入和独占可变结果列，因此没有本文件内部的共享可变状态。

与 Go 版使用 `bufAllocator.get()`/`put()` 复用临时列不同，Rust 的 `evaluateVectorArgument` 每次创建 `chunk::Column::default()` 并按所有权返回，函数结束后由 Rust 生命周期自动释放。距离路径同时持有左右两个临时列；单参数路径持有一个。扩展或优化临时列复用时，需要确认不会让同一可变列在嵌套子表达式求值间别名，并重新评估每批分配成本。

## 与 Go 版本的对应关系

[`pkg/expression/builtin_vec_vec.go`](builtin_vec_vec.go) 是由 generator 生成的直接对照。Rust 与 Go 均覆盖 `VecDims`、四个双向量度量、`VecL2Norm`、`VecFromText`、`VecAsText`，并保持以下语义一致：先批量求参数、逐行传播 NULL、底层错误立即返回、NaN 距离/范数变为 NULL，以及文本解析后的目标列维度检查。

实现层面的差异是：

- Go 为每个距离签名单独生成循环；Rust 用 `vectorizedDistance` 加宏复用公共控制流。
- Go 通过 `bufAllocator` 租借临时列并用 `defer put` 归还；Rust 创建拥有所有权的局部 `Column`。
- Go 通常先 `Resize*(n, false)`、`MergeNulls`，再取得底层切片按下标写入；Rust 以零长度重置或预留容量后逐行 `Append*`。
- Go 每个签名在生成文件中显式提供 `vectorized() == true`；Rust 本文件只提供 `vecEval*`，向量化能力由 Rust 表达式框架的签名分派体现。

Go 的 [`pkg/expression/builtin_vec_vec_test.go`](builtin_vec_vec_test.go) 用 `vecBuiltinVecCases` 表驱动覆盖八个函数族、正常值、NULL 和余弦 NaN。Rust 的 `builtin_vec_vec_test.rs` 锁定相同的八项清单，并调用 `builtin_vec_vec_31_aster_unit_test.rs` 中的完整回归集合；后者额外明确验证数值、维度不匹配、非法文本、常量广播、NULL/NaN 和函数类下推编码。

## 扩展指南

新增向量度量时，如果签名是“两列 `VectorFloat32` 输入、`f64` 输出，NULL/NaN 规则相同”，优先在 `builtin_vec.rs` 定义签名/标量语义后，在本文件用 `vectorized_distance!` 绑定底层 `VectorFloat32` 方法。若错误、NULL 或输出类型语义不同，不应强套该宏，应写独立方法并明确结果列初始化与每行追加规则。

修改文本互转时，应同步检查 `ParseVectorFloat32`、`String` 和 `CheckDimsFitColumn` 的契约，尤其是 `return_type.GetFlen()` 的含义、非法文本错误以及格式稳定性。任何新路径都必须保证成功时输出行数与输入批一致，并在失败时不把部分结果误当成完整结果。

测试应继续放在独立文件，不嵌入生产源：

- 在 `builtin_vec_vec_31_aster_unit_test.rs` 增加具体 Rust 数值、NULL、NaN、错误和常量广播回归。
- 在 `builtin_vec_vec_test.rs` 同步函数族清单和 Go 对齐入口。
- 若 Go 对照语义发生变化，同步核对 `builtin_vec_vec.go`（生成来源）及 `builtin_vec_vec_test.go`，不要只让 Rust 测试通过。

兼容风险主要是 SQL NULL/错误语义、文本格式和下推签名一致性；性能风险主要是每批临时列分配、逐行追加以及重复解析/格式化。改动后应以独立单元测试比较标量与向量化结果，而不能只验证编译。

## 验证依据

- RustCodeGraph `status`：索引可用，覆盖本仓库 Rust 与 Go 文件。
- RustCodeGraph `node --file pkg/expression/builtin_vec_vec.rs --offset 1 --limit 400`：读取目标文件 181 行全貌，确认两个内部辅助、一个宏、四次距离宏展开和四个显式签名实现。
- RustCodeGraph `query vectorizedDistance`、`query evaluateVectorArgument` 与 `callees vectorizedDistance`：确认精确符号位置以及 `vectorizedDistance → evaluateVectorArgument` 调用边。针对重载 `vecEval*` 的查询返回大量同名候选，因此未把含歧义结果写成精确调用边。
- RustCodeGraph `node` 读取 `pkg/expression/lib.rs`：确认 crate 根、模块挂载、测试模块挂载及 `builtin_vec` 再导出关系。
- RustCodeGraph `node` 读取 `pkg/expression/builtin_vec_vec_test.rs` 与 `pkg/expression/builtin_vec_vec_31_aster_unit_test.rs`：确认八个函数族，以及数值、NULL、NaN、维度错误、非法文本、广播和下推编码测试。
- 直接读取未由调用图完整表达的 `pkg/expression/Cargo.toml`、Go 对照 `pkg/expression/builtin_vec_vec.go` 和 Go 测试 `pkg/expression/builtin_vec_vec_test.go`：确认 crate 边界、依赖再导出来源和 Go/Rust 语义对应。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定的 11 章节结构命令和人工事实复核验收。
