# `pkg/expression/vectorized.rs`

## 文件定位

本文件属于 `astersql-expression` crate（`pkg/expression/Cargo.toml` 的 `[package]` 与 `[lib]`），由 `pkg/expression/lib.rs:359-360` 以 `#[path = "vectorized.rs"] mod vectorized_kernel;` 装配成私有模块。它只定义一个公开到父模块可见范围内的函数 `genVecFromConstExpr`，负责把一次标量表达式求值结果广播成一列。

需要特别区分当前接线与同名实现：crate 根在 `pkg/expression/lib.rs:368` 再导出的是 `core_support::*`，而没有再导出 `vectorized_kernel::*`。因此 `pkg/expression/column.rs` 和 `pkg/expression/constant.rs` 中经 `use crate::*` 解析到的是 `pkg/expression/core_support.rs:722` 的同名函数；本文件函数的已确认直接使用者是 `pkg/expression/scalar_function_37_aster_unit_test.rs:294,299`。RustCodeGraph 因同名符号也把 `column.rs`、`constant.rs` 的调用列在本函数名下，阅读调用图时必须结合模块可见性消歧。

## 核心职责

`genVecFromConstExpr`（`vectorized.rs:26-148`）承担三件事：确定输出行数；依据 `types::EvalType` 选择对应的标量 `Expression::Eval*` 入口；将得到的非 NULL 值或 NULL 状态填充到 `chunk::Column` 的每一行。它覆盖整数、实数、十进制、日期时间/时间戳、时长、JSON、`VectorFloat32` 和字符串八类求值族。

该函数处理的是“值不随输入行变化”的广播，不读取任一输入行内容；传给标量求值器的始终是 `chunk::Row::default()`（例如 `vectorized.rs:47,58,69`）。`input` 只贡献行数：`Some(chunk)` 使用 `Chunk::NumRows()`，`None` 则生成一行，保持 Go 独立常量向量求值的约定。

## 主要符号

- `pub fn genVecFromConstExpr(ctx, expr, targetType, input, result) -> Result<(), errors::Error>`（`vectorized.rs:26-32`）是唯一生产符号。`ctx` 提供表达式求值上下文，`expr` 提供类型化标量求值，`targetType` 决定求值及列存储路径，`input` 决定广播长度，`result` 是原地改写的输出列。
- `n`（`vectorized.rs:34-42`）是本次输出基数：缺省为 1；有输入时等于 `NumRows()`；零行时先按目标类型 `Reset`，随即成功返回。
- `targetType` 的 `match`（`vectorized.rs:44-144`）是扩展和兼容性的中心分派点。`ETDatetime | ETTimestamp` 共享 `EvalTime` 与时间列布局，其余已支持类型各走独立分支；兜底分支返回不支持类型错误。
- 文件没有类型、trait、impl、模块级常量或条件编译项；也不持有全局状态。

## 执行流程

1. 入口先把输出行数设为 1。若 `input` 存在，则读取其逻辑行数；这也意味着带 selection 的 Chunk 按 `NumRows()` 所表示的当前逻辑行数广播，Go 的 `TestVectorizedConstant` 用 1024 行及 10 项 selection 验证了这一语义（`pkg/expression/constant_test.go:478-533`）。
2. 输入为零行时，函数调用 `result.Reset(targetType)` 清除复用列中的旧值，然后不求值表达式直接返回。Rust 回归 `constant_vectorization_repeats_values_and_resets_empty_input` 明确验证了三行广播后再以空输入清零（`pkg/expression/scalar_function_37_aster_unit_test.rs:283-302`）。
3. 固定宽度类型 `ETInt`、`ETReal`、`ETDecimal`、时间类和 `ETDuration` 先调用一次对应 `Eval*`。NULL 使用相应 `Resize*(n, true)` 一次建立全 NULL 列；非 NULL 先 `Reset` 到目标布局，再循环调用 `Append*` 广播值（`vectorized.rs:45-103`）。
4. 变长类型 `ETJson`、`ETVectorFloat32`、`ETString` 先 `Reserve*(n)`，再求值一次，并逐行选择 `AppendNull` 或类型化 `Append*`（`vectorized.rs:104-137`）。JSON、时间和向量在重复写入时克隆值；字符串按引用追加。
5. 未支持的 `EvalType` 不修改为伪造结果，而是返回 `unsupported type {targetType} during evaluation`（`vectorized.rs:138-144`）。所有成功分支最终返回 `Ok(())`。

## 数据与状态

函数自身只有局部计数和单次求值结果，不缓存跨调用状态。持久变化全部发生在调用方提供的 `result: &mut chunk::Column` 上；可变独占借用保证同一次调用期间没有其他 Rust 代码并发写该列。

列布局由 `targetType` 决定。固定宽度 NULL 路径通过 `Resize*` 同时建立长度和 NULL 位图；非 NULL 路径先重置再追加。变长路径依赖 `Reserve*` 为新一批次准备容量，然后按行追加偏移和值或 NULL。具体列操作由 `astersql-util-chunk` 提供，依赖在 `pkg/expression/Cargo.toml` 中以 `chunk-dependency = { package = "astersql-util-chunk", path = "../util/chunk" }` 声明。

输入 Chunk 的内容和行对象不进入求值；输入只是一个可选的行数载体。表达式、上下文和输入均为共享借用，函数不会取得其所有权；十进制通过引用追加，时间、时长、JSON 和向量按各自值语义克隆或复制到多行。

## 依赖与调用关系

向下依赖来自 `use crate::*`（`vectorized.rs:21`）：`EvalContext`、`Expression`、`types::EvalType`、`chunk::{Chunk, Column, Row}` 和 `errors::Error/Errorf` 均经 crate 根的内部装配或再导出可见。实际工作调用包括 `Expression::EvalInt/EvalReal/EvalDecimal/EvalTime/EvalDuration/EvalJSON/EvalVectorFloat32/EvalString`，以及 `Column` 的 `Reset`、`Resize*`、`Reserve*`、`Append*`。

向上关系必须按当前模块边界理解。`pkg/expression/lib.rs:359-360` 声明该模块；`pkg/expression/scalar_function_37_aster_unit_test.rs:23` 显式从 `crate::vectorized_kernel` 导入函数并在测试中调用。生产文件 `column.rs` 与 `constant.rs` 虽有同名调用，但 crate 根只 `pub use core_support::*`（`lib.rs:368`），故它们当前连接到 `core_support.rs:722-826` 的另一份实现，而不是本文件。RustCodeGraph 的 `query/node` 能找到三个同名定义（本文件、`core_support.rs`、Go 文件），其按裸名称给出的 caller 集合存在合并歧义。

crate 边界由 `pkg/expression/Cargo.toml` 确认：库入口是 `lib.rs`、关闭 doctest，并通过本地 path 依赖接入 chunk/types 等数据库基础类型。本文件没有 feature gate，也没有直接使用第三方 crate。

## 错误处理与边界

每个 `expr.Eval*` 使用 `?` 原样传播 `errors::Error`；求值失败后不会进入该分支的广播循环。固定宽度分支是在求值成功后才重置/填充结果；JSON、向量和字符串分支则先调用 `Reserve*` 再求值，因此出错时容量或列内部预留状态可能已经变化，但不会附加本批次结果。调用方不能把错误后的 `result` 当作有效输出。

边界行为包括：`input == None` 输出一行；零行输入清空结果且不调用表达式；NULL 被广播为恰好 `n` 个 NULL；未知类型明确报错。函数不核验 `expr` 的实际返回类型是否与 `targetType` 相符，这一契约由调用者保证。`ETDatetime` 与 `ETTimestamp` 共用求值入口但在非 NULL 时用原 `targetType` 重置列，保留二者的逻辑类型区别。

当前直接 Rust 测试只覆盖非 NULL 整数三行广播和空输入重置；NULL、`None` 输入、错误传播、所有其他类型及 unsupported 分支未在该直接测试中逐项验证。Go 测试补充了整数/字符串及 selection 行数语义，但不能替代 Rust 分支回归。

## 并发与资源生命周期

本函数不创建线程、任务、锁、通道、事务或外部资源，也没有 `unsafe`。一次调用完全同步：标量求值完成后才进行广播，返回时所有借用结束。

并发安全主要由签名约束：上下文和表达式仅共享借用，结果列要求独占可变借用。函数没有内部共享缓存；能否从多个线程同时调用取决于具体 `EvalContext`、`Expression` 实现及调用者是否为每次调用提供独立结果列，本文件不作额外保证。克隆 JSON、时间、时长及向量值使输出各行不依赖循环局部值的生命周期。

资源成本为一次标量求值加 `O(n)` 次填充；输出内存为 `O(n)`。固定宽度 Go 实现可 resize 后直接写底层 slice，而 Rust 实现采用 reset 加逐项 append，语义一致但常数开销可能不同；修改此处时应关注大批次性能。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/vectorized.go:23-128`，函数名、参数角色、缺省一行、空输入清理、类型分派、NULL 广播以及错误文本均一一对应。Rust 用 `Result` 与 `?` 代替 Go 的三返回值加显式 `if err != nil`，用 trait object 借用代替 Go interface 值。

固定宽度非 NULL 写法不同：Go 先 `Resize*(n, false)` 再取得类型 slice 批量赋值；Rust 先 `Reset` 再逐项 `Append*`。时长方面 Go 写入 `types.Duration.Duration` 字段，Rust `AppendDuration(v.clone())` 交给 Rust 列 API。JSON、向量和字符串都保持 reserve 后逐行 append；Rust 对拥有所有权或复合值的类型显式 `clone`/`Clone`。

迁移状态存在一项结构性差异：本文件是 Go 同路径函数的近直接移植，但生产 `Constant`/`CorrelatedColumn` 当前使用 `core_support.rs` 中的同名实现。后者要求非可选 `&Chunk`、错误文案也不同。因而不能宣称本文件已成为生产广播路径的唯一实现；统一两份实现属于后续接线/去重工作，不在本次纯文档任务范围内。

## 扩展指南

新增求值类型时，最可能修改 `genVecFromConstExpr` 的 `match`：选择正确的 `Expression::Eval*`，明确 NULL 列构造方式，确保旧结果被清理，并依据值所有权选择复制、借用或克隆。还要同步 Go 的 `pkg/expression/vectorized.go`，并确认 `chunk::Column::Reset` 及对应 `Reserve/Resize/Append` 已支持该 `EvalType`。

任何生产接线调整都必须先决定两份同名实现的权威来源：若让 `vectorized_kernel` 服务生产调用，需要显式再导出或使用限定路径，并处理 `Option<&Chunk>` 与 `&Chunk` 签名差异；若保留 `core_support` 为生产实现，则本文件可能只用于兼容测试。不要仅依据 RustCodeGraph 的裸名称 caller 输出替换接线。

测试应放在独立文件，不能嵌入本源文件。至少扩展 `pkg/expression/scalar_function_37_aster_unit_test.rs`，覆盖 NULL、`None`、零行不求值、求值错误、unsupported 类型以及每个类型族；若改变生产 `Constant`/`CorrelatedColumn` 路径，还应同步 `pkg/expression/constant_test.rs`、`pkg/expression/column_test.rs` 及 Go 的 `constant_test.go`/`column_test.go`。兼容风险集中在 NULL 位图、selection 后逻辑行数、时间戳布局和错误文案；性能风险集中在大 `n` 下逐项 clone/append。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/expression/vectorized.rs` 确认目标文件及 2 个符号记录；`node --file ...` 读取了完整 148 行源文件。
- RustCodeGraph `query genVecFromConstExpr` 找到 `vectorized.rs:26`、`core_support.rs:722` 与 `vectorized.go:23` 三个同名定义；`node genVecFromConstExpr` 给出的裸名称 caller 包含 `column.rs`/`constant.rs`，并已用 `lib.rs:359-368` 的模块声明与再导出规则人工消歧。
- 已阅读 Rust 路径：`pkg/expression/vectorized.rs`、`lib.rs`、`column.rs:45-134`、`constant.rs:240-343`、`core_support.rs:722-826`、`scalar_function_37_aster_unit_test.rs:283-302`。
- 已阅读 crate/Go/测试路径：`pkg/expression/Cargo.toml`、`vectorized.go`、`constant_test.go:478-533`、`column_test.go`、`column_test.rs:296-382`。包内不存在 `doc.go`，因此没有可读取的最近包契约文件。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构检查，并人工复核唯一生产物、源码链接、当前接线边界与未覆盖测试说明。
