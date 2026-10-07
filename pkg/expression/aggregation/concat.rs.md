# `pkg/expression/aggregation/concat.rs`

## 文件定位

本文件实现旧式行求值聚合接口 `Aggregation` 下的 `GROUP_CONCAT` 运行时对象 `concatFunction`。它属于 Cargo crate `astersql-expression-aggregation`（`pkg/expression/aggregation/Cargo.toml`），由 `lib.rs` 的私有 `mod concat` 纳入 crate，再通过 `pub use concat::*` 向 crate 使用者导出。

常规执行路径由 `AggFuncDesc::GetAggFunc`（`descriptor.rs:297`）按 `ast::AggFuncGroupConcat` 构造对象，并从表达式上下文注入 `group_concat_max_len`；PB/下推路径由 `NewDistAggFunc`（`aggregation.rs:31`）在遇到 `tipb::ExprType::GroupConcat` 时构造对象。后续调用通过 `Box<dyn Aggregation>` 动态分派。本文件只负责按调用顺序拼接已经交给它的行；`ORDER BY` 的排序不在此文件中实现，因此这里不能单独保证输入顺序，只会保留上游传入行的顺序。

## 核心职责

- `concatFunction::Update` 对最后一个参数之外的所有表达式逐行求值；任一值为 NULL 时整行不参与聚合。
- 首个有效行直接写入结果，后续有效行先写 separator，再顺序写入本行各值；`BufferInitialized` 用来区分“尚无结果（NULL）”与“已有一个空字符串结果”。
- 当描述符启用 DISTINCT 时，通过 `AggEvaluateContext::DistinctChecker` 对一整行的值向量去重。
- 结果超过 `maxLen` 时按字节截断，并在 `concatFunction` 整个生命周期内最多向 `StatementContext` 添加一次告警。
- `GetResult`、`GetPartialResult` 暴露最终/部分结果；上下文的创建和清理复用 `aggFunction` 的公共实现。

## 主要符号

- `pub struct concatFunction`（`concat.rs:25`）：聚合器本体。`aggFunction` 保存 `AggFuncDesc`；`separator` 是首次求值得到的分隔符；`maxLen` 是字节上限；`sepInited` 和 `truncated` 是跨分组保留的生命周期哨兵。类型名沿用 Go 风格，crate 根通过 `#![allow(non_camel_case_types, non_snake_case)]` 接纳该命名。
- `writeValue(&mut AggEvaluateContext, Datum) -> Result<(), Error>`（`concat.rs:40`）：字节 Datum 原样追加；其他 Datum 先 `ToString`，再追加 UTF-8 字节。转换错误原样传播。
- `initSeparator(&mut self, &dyn EvalContext, Row) -> Result<(), Error>`（`concat.rs:52`）：求值参数列表最后一项；NULL separator 返回 `Invalid separator argument`，否则保存其字符串形式。
- `Aggregation::Update`（`concat.rs:73`）：核心状态转移入口，依次处理 separator 初始化、参数求值、NULL 过滤、DISTINCT、拼接、长度限制与告警。
- `Aggregation::GetResult`（`concat.rs:122`）：未初始化缓冲时返回 NULL Datum；否则将缓冲以 `String::from_utf8_lossy` 转为字符串，并携带描述符返回类型的 collation。
- `Aggregation::GetPartialResult`（`concat.rs:137`）：以单元素 `Vec<Datum>` 包装 `GetResult`。
- `Aggregation::CreateContext` / `ResetContext`（`concat.rs:140`、`:143`）：委托 `aggFunction` 创建或清空每组状态；不会重置聚合器自身的 `separator`、`sepInited` 或 `truncated`。

本文件没有模块级常量、枚举、独立 trait、条件编译项或额外公开函数。

## 执行流程

1. `AggFuncDesc::GetAggFunc` 将描述符克隆进 `aggFunction`，读取 `ExprContext::GetGroupConcatMaxLen`，并以空 separator、两个 false 哨兵构造 `concatFunction`。`NewDistAggFunc` 的 PB 路径则把 `maxLen` 设为 0，即不在本聚合器中实施长度截断。
2. 调用者为一个分组调用 `CreateContext`。公共实现创建空 `Buffer`、`BufferInitialized = false`，并仅在 `HasDistinct` 时创建 `DistinctChecker`（`aggregation.rs:223`）。
3. 首次 `Update` 调用 `initSeparator`，只求值最后一个参数。成功后设置 `sepInited = true`；以后即使 `ResetContext` 开始新分组，也继续使用该 separator。
4. `Update` 求值 `Args[..len-1]`。任一表达式求值失败则返回错误；任一结果为 NULL 则立即成功返回，不改变缓冲和去重器。
5. DISTINCT 模式把本行全部非 separator 值作为一个向量交给 `DistinctChecker::Check`；重复元组直接跳过。
6. 若此前已有有效行，先追加 separator；随后将本行每个值依次交给 `writeValue`。即使首值是空字符串，也会把 `BufferInitialized` 设为 true，因此下一行仍会得到前导 separator。
7. 若 `maxLen > 0` 且缓冲超过限制，则以 `min(maxLen, usize::MAX)` 为字节位置截断。首次截断使用第一个参数的 `StringWithCtx` 生成告警文字，并把 `truncated` 设为 true。
8. `GetResult` 把未初始化状态映射为 SQL NULL，把已初始化字节缓冲映射为带返回 collation 的字符串；`GetPartialResult` 返回同一值的一列部分结果。
9. `ResetContext` 清空当前分组的缓冲、初始化标记及 DISTINCT 集合，但保留聚合器级 separator 和“已告警”哨兵。

## 数据与状态

状态分为两层。`concatFunction` 保存聚合器生命周期状态：描述符、separator、长度上限、separator 是否初始化、是否已产生截断告警。`AggEvaluateContext` 保存每个分组的状态：共享求值上下文、可选去重器、`Vec<u8>` 缓冲以及 `BufferInitialized`；其 `Count`、`Value`、`GotFirstRow` 字段不是本实现的业务状态。

关键不变量如下：最后一个参数必须存在且被解释为 separator；所有更早参数组成一个待拼接元组；只有所有值都非 NULL 且 DISTINCT 检查通过时，缓冲才改变；`BufferInitialized == false` 表示结果为 NULL，而不是空字符串；`sepInited` 和 `truncated` 的生命周期长于单个 `AggEvaluateContext` 分组。`concat_test.rs:84` 和 `aggregation_aster_unit_test.rs:410` 明确验证了 Reset 后仍沿用首次 separator 的当前语义。

内存随当前组拼接结果和 DISTINCT 键集合增长。长度检查发生在完整追加一行之后，因此临时峰值可高于 `maxLen`；截断后缓冲不超过限制，但后续行仍可能再次追加后再截断。`maxLen == 0` 表示不限制，而不是产生空结果。

## 依赖与调用关系

上游构造与调用：

- `descriptor.rs::AggFuncDesc::GetAggFunc` 是普通描述符路径，设置真实会话长度上限。
- `aggregation.rs::NewDistAggFunc` 是 `tipb::ExprType::GroupConcat` 路径，返回 `Box<dyn Aggregation>`，其 `maxLen` 当前为 0。
- `Aggregation` trait 定义 `Update`、结果读取和上下文生命周期契约；实际执行器通过该 trait 调用本实现。RustCodeGraph 的文件关系还显示 `concat.rs` 被 `aggregation.rs`、`descriptor.rs` 等 crate 内文件引用。

下游依赖：

- `expression::Expression::Eval` 求值参数，`Datum::ToString`/`GetBytes` 完成表示转换。
- `distinctChecker::Check` 实施 DISTINCT 元组去重。
- `stmtctx::StatementContext::AppendWarning` 保存截断告警。
- `aggFunction::{CreateContext, ResetContext}` 管理 `AggEvaluateContext`。
- `StringerWithCtx::StringWithCtx` 生成告警中的参数描述；`RetTp::GetCollate` 决定结果 Datum 的 collation。

直接 Cargo 依赖来自 `expression`、`chunk`、`stmtctx`、`errors` 以及 crate 根的 `types` 门面；`std::sync::Arc` 用于共享只读求值上下文。文件本身不创建线程、任务、通道或事务。

## 错误处理与边界

- separator 参数求值失败、值参数求值失败、Datum 字符串转换失败、DISTINCT 检查失败都会通过 `Result` 向调用者传播；已写入缓冲的先前行不会回滚。
- NULL separator 是硬错误；任一非 separator 参数为 NULL 则静默跳过整行。空参数列表会在 `.last().unwrap()` 或 `len() - 1` 处失败，因此参数个数合法性必须由描述符构造阶段保证，本文件不做防御性校验。
- 尚无有效行时 `GetResult` 返回 NULL；首个有效值为空字符串时返回非 NULL 空串。这一差异由 `BufferInitialized` 保证，并在 `aggregation_aster_unit_test.rs:468` 覆盖。
- 截断按字节而非字符边界进行；若切开多字节 UTF-8，`GetResult` 的 `from_utf8_lossy` 会以替换字符生成 Rust 字符串。这是当前代码事实，修改时需单独评估与 Go 的原始字节字符串行为是否完全一致。
- `RetTp` 缺失时结果使用空 collation 字符串；存在时使用 `GetCollate()`。separator 在首次成功初始化后固定，Reset 不会按新分组重新求值。
- 截断告警不是错误。它最多生成一次，告警文本取第一个值参数；没有值参数同样会越界，仍依赖上游描述符约束。

## 并发与资源生命周期

`Update` 和 `ResetContext` 都要求 `&mut self`，每组状态也通过 `&mut AggEvaluateContext` 修改；本文件没有内部同步，设计上应由执行器独占驱动一个聚合器实例，不能把同一实例并发更新。`Arc<dyn EvalContext>` 只解决求值上下文的共享所有权，不使聚合状态并发安全。

缓冲和 DISTINCT 集合属于 `AggEvaluateContext`，`ResetContext` 会清空/重建它们以复用分组状态。separator 字符串和两个哨兵属于 `concatFunction`，持续到该聚合器被丢弃：这使所有分组共享首次 separator，并使 MySQL/Go 约定的“聚合器生命周期只告警一次”成立。所有资源由 Rust 所有权自动释放，没有显式 close、后台任务或外部句柄。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/aggregation/concat.go`。Rust 保留了 Go 的字段与主流程：最后一参作 separator、NULL 行跳过、DISTINCT 元组检查、非首行前插入 separator、按字节长度截断、生命周期内只告警一次、空分组返回 NULL，以及 Reset 不清除 `sepInited`/`truncated`。

表示层差异主要有三点：Go 使用可空 `*bytes.Buffer` 表示“尚无结果”，Rust 使用始终存在的 `Vec<u8>` 加 `BufferInitialized`；Go `writeValue` 对非字节 Datum 使用 `fmt.Fprintf("%v", GetValue())`，Rust 使用可能失败的 `Datum::ToString()`；Go 的字符串可保留任意字节，Rust `GetResult` 使用有损 UTF-8 转换。普通整数和合法 UTF-8 测试覆盖下二者一致，但特殊 Datum 格式和截断多字节字符需要针对性兼容测试。

Go `TestConcat`（`aggregation_test.go:503`）验证初始 NULL、两行 separator 拼接、NULL 跳过、部分结果和 DISTINCT。Rust 对应入口 `aggregation_test.rs::TestConcat` 调用 `aggregation_aster_unit_test.rs::group_concat_matches_go_separator_null_distinct_and_reset_cases`；独立的 `concat_test.rs` 进一步验证 Reset 后只告警一次及 separator 跨分组保留。当前 Rust 独立测试没有逐项覆盖 NULL separator、字节 Datum、多参数元组、求值错误和多字节截断。

## 扩展指南

- 调整逐行拼接、NULL 或 DISTINCT 语义时，修改 `concatFunction::Update`，并在独立测试文件 `concat_test.rs` 增加回归；若行为来自 Go 移植，还应同步核对 `concat.go` 与 `aggregation_test.go::TestConcat`。
- 调整 Datum 文本化或二进制行为时，修改 `writeValue`，重点测试 `KindBytes`、decimal/时间等非字节类型、非法/非 UTF-8 字节，以及结果 collation。
- 调整 separator 规则时，修改 `initSeparator` 和 `sepInited` 生命周期；必须明确是每个聚合器、每个分组还是每行求值，并相应修改 Reset 测试，不能只改 `ResetContext` 的缓冲逻辑。
- 调整长度限制或告警时，同时检查 `descriptor.rs::GetAggFunc` 的 `maxLen` 注入、`aggregation.rs::NewDistAggFunc` 的下推默认值和 `truncated` 生命周期。性能风险包括先追加后截断造成的峰值分配，以及每次超限仍重复追加/截断。
- 若要支持有序拼接，不应只在本文件中宣称支持；应先确认上游是否已按 ORDER BY 排列输入，或设计排序所需的独立部分状态、合并协议与内存管理。
- 测试逻辑必须继续放在 `concat_test.rs` 或现有独立聚合测试文件中，不应嵌入 `concat.rs`。新增生产行为还需维持 Go 版本的真实逻辑，不用简化实现替代兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标 `concat.rs` 有 11 个符号；`files --filter pkg/expression/aggregation` 确认目标、Go 对照与独立测试均被索引；`node --file` 读取了 `concat.rs`、`lib.rs`、`aggregation.rs`、`descriptor.rs`、`concat.go`、`concat_test.rs`；`query` 确认了 `concatFunction`、`writeValue`、`initSeparator`、`GetResult`、`GetPartialResult`、`CreateContext`、`ResetContext`、`GetAggFunc` 与 `NewAggFuncDesc`。`callers/callees` 查询在本地索引后端超时，因此没有把缺失的精确函数级边当作已验证事实。
- crate 与装配：`pkg/expression/aggregation/Cargo.toml`、`pkg/expression/aggregation/lib.rs`。
- 运行时契约与构造：`pkg/expression/aggregation/aggregation.rs` 的 `NewDistAggFunc`、`Aggregation`、`AggEvaluateContext`、`aggFunction::{CreateContext, ResetContext}`；`pkg/expression/aggregation/descriptor.rs::AggFuncDesc::GetAggFunc`。
- 直接实现与对照：`pkg/expression/aggregation/concat.rs`、`pkg/expression/aggregation/concat.go`。
- 测试证据：`pkg/expression/aggregation/concat_test.rs`；`pkg/expression/aggregation/aggregation_test.rs::TestConcat`；`pkg/expression/aggregation/aggregation_aster_unit_test.rs` 中 GROUP_CONCAT 共享测试与空首值测试；Go 的 `pkg/expression/aggregation/aggregation_test.go::TestConcat`。
- 人工复核结论：该文件存在的原因是为行式 `Aggregation` 接口提供 GROUP_CONCAT 的状态机；安全扩展必须同时考虑描述符构造、每组上下文与聚合器生命周期状态、Go 对照和独立测试。按任务约束未运行 Cargo。
