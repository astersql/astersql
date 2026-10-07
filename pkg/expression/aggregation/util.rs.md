# `pkg/expression/aggregation/util.rs`

## 文件定位

该文件属于 `astersql-expression-aggregation` crate，是聚合运行时的内部公共工具层。模块入口 `pkg/expression/aggregation/lib.rs` 以 `mod util` 装配它并通过 `pub use util::*` 在 crate 内统一导出。它不负责选择聚合函数或产生最终结果，而是为多个聚合实现提供两项共享能力：DISTINCT 参数元组判重，以及 SUM/AVG 输入的数值类型提升与累加。

直接使用点位于同目录聚合实现中：`aggregation.rs` 创建、重置 DISTINCT 状态并在通用求和路径调用 `Check`/`calculateSum`；`avg.rs` 用 `calculateSum` 合并分布式 AVG 的局部和；`count.rs`、`concat.rs`、`sum_int.rs` 分别用 `Check` 过滤重复的 COUNT 参数组合、GROUP_CONCAT 参数组合和整数 SUM 值。

## 核心职责

1. `distinctChecker` 把一次聚合输入的 `Vec<types::Datum>` 按会话时区编码为字节键，用 `mvmap::MVMap` 记录已经出现的键；首次出现返回 `true`，重复出现返回 `false`。
2. `calculateSum` 实现与 Go 版 SUM/AVG 相同的累计类型规则：NULL 不改变已有和；有符号/无符号整数先转为 DECIMAL；DECIMAL 保持 DECIMAL；其余非空类型转为 DOUBLE；随后仅允许在 NULL、DOUBLE 或 DECIMAL 累计状态上继续运算。
3. 两项工具都把底层 codec、类型转换或加法错误统一映射到本 crate 的 `expression::Error` 边界，供上层 `Aggregation::Update` 用 `?` 传播。

## 主要符号

- `pub struct distinctChecker`：有状态判重器。`existing_keys: mvmap::MVMap` 保存已见编码键；`key: Vec<u8>` 是跨调用复用的编码缓冲；`ctx: Arc<dyn EvalContext>` 提供时区和错误上下文。类型名沿用 Go 命名且未公开字段，实际由 crate 根再导出供同 crate 模块使用。
- `pub fn createDistinctChecker(ctx: Arc<dyn EvalContext>) -> distinctChecker`：构造空 `MVMap`、空键缓冲并持有共享求值上下文。`aggregation.rs::aggFunction::{CreateContext, ResetContext}` 只在 `AggFuncDesc.HasDistinct` 为真时调用它。
- `distinctChecker::Check(&mut self, values: Vec<types::Datum>) -> Result<bool, crate::Error>`：编码参数元组、应用语句错误策略、查询并更新集合。`&mut self` 表明同一检查器的状态与临时缓冲不得并发修改。
- `pub fn calculateSum(ctx: types::Context, sum: types::Datum, value: types::Datum) -> Result<types::Datum, crate::Error>`：无内部状态的单步累计函数。`ctx` 只用于 Datum 到 DECIMAL/DOUBLE 的转换规则；返回新的累计 Datum。

文件没有模块级常量、trait、条件编译项或后台任务。

## 执行流程

DISTINCT 路径如下：

1. `aggFunction::CreateContext` 或 `ResetContext` 在聚合描述符带 DISTINCT 时创建新的 `distinctChecker`，因此每个聚合求值上下文拥有独立的已见集合。
2. COUNT、通用 SUM、GROUP_CONCAT 或整数 SUM 先求值并执行各自的 NULL 规则，再把需要参与判重的一个或多个 Datum 交给 `Check`。
3. `Check` 清空 `self.key`，通过 `std::mem::take` 把其缓冲交给 `codec::EncodeValue(self.ctx.Location(), ..., values)`；成功时保存返回的编码键。
4. 编码错误先交给 `self.ctx.ErrCtx().HandleError`。若错误策略返回错误，立即向上层传播；若策略将其降级为警告，则继续使用当前键（此分支中为空键）完成判重。
5. `MVMap::Get` 返回非空值列表表示键已存在，此时返回 `false`；否则以空字节值执行 `Put` 并返回 `true`。调用者看到 `false` 后跳过本行的计数、累加或拼接。

求和路径如下：

1. 按 `value.Kind()` 归一化输入：NULL 变为默认 NULL Datum，整数调用 `ToDecimal`，DECIMAL 原样保留，其他类型调用 `ToFloat64`。
2. 归一化结果为 NULL 时原样返回 `sum`；这使 NULL 输入不初始化累计状态。
3. 若 `sum` 为 NULL，直接以首个非空归一化值初始化它。
4. 若 `sum` 为 DOUBLE 或 DECIMAL，调用 crate 门面 `types::ComputePlus(sum, data)`；其他累计类型被视为内部状态错误。
5. `aggregation.rs::aggFunction::updateSum` 在成功后递增计数；`avg.rs::avgFunction::updateAvg` 则另外合并上游局部 count。计数职责不在本文件中。

## 数据与状态

`distinctChecker` 的状态粒度是单个 `AggEvaluateContext`。集合在多次 `Update` 间持续存在；`ResetContext` 会丢弃旧检查器并创建新实例，从而清空分组/批次之间的去重状态。参数顺序和 Datum 编码共同构成键，所以 `[1, 2]` 与 `[2, 1]` 是不同组合；测试还证明含 NULL 的相同组合第一次通过、第二次被过滤。

`key` 会在正常成功路径跨调用复用容量，避免为每行从零构造编码缓冲。Rust 版本没有 Go `distinctChecker.vals` 字段，查询时向 `MVMap::Get` 传入新的空 `Vec`；这是缓冲复用策略的实现差异，不改变“是否存在”的结果。

`calculateSum` 不持久化状态，所有累计状态都由调用方的 `AggEvaluateContext.Value` 持有。不变量是：通用 SUM/AVG 一旦接收非空输入，累计值应为 `KindFloat64` 或 `KindMysqlDecimal`；整数输入也转换为 DECIMAL。`types::ComputePlus` 要求左右类型匹配，因此正常调用链依靠描述符和输入类型在一个累计序列中维持一致的归一化类别。

## 依赖与调用关系

- 上游构造：`aggregation.rs::aggFunction::CreateContext` 与 `ResetContext` → `createDistinctChecker`。
- 上游判重：`aggregation.rs::aggFunction::updateSum`、`count.rs::countFunction::Update`、`concat.rs::concatFunction::Update`、`sum_int.rs::sumIntFunction::Update` → `distinctChecker::Check`。
- 上游累计：`aggregation.rs::aggFunction::updateSum`、`avg.rs::avgFunction::updateAvg` → `calculateSum`。更上层由各聚合的 `Aggregation::Update` 驱动，通用 SUM/AVG 的状态最终进入执行器聚合流程。
- 下游编码：`Check` → `codec::EncodeValue`，并从 `EvalContext::Location` 取得时区，使时间相关 Datum 的键编码遵循求值上下文。
- 下游错误策略：`Check` → `EvalContext::ErrCtx().HandleError`，决定编码错误是返回还是按语句策略降级。
- 下游存储：`Check` → `mvmap::{NewMVMap, MVMap::Get, MVMap::Put}`。
- 下游数值操作：`calculateSum` → `Datum::{ToDecimal, ToFloat64}`、`types::{NewDecimalDatum, NewFloat64Datum, ComputePlus}`。

`pkg/expression/aggregation/Cargo.toml` 证明该 crate 直接依赖 `astersql-util-codec`、`astersql-util-mvmap` 和 `astersql-expression`，并通过多个 `*-dependency` crate 组装 `lib.rs::types` 门面；`package.metadata.porting.go-package` 指向 Go 包 `pkg/expression/aggregation`。

## 错误处理与边界

- `codec::EncodeValue` 的错误不会无条件失败：它先经过 `ErrCtx().HandleError`。返回 `Some(error)` 时包装成 `expression::errors::New(error.to_string())`；返回 `None` 时代表错误已按上下文策略处理，流程继续。由于编码缓冲已被 `take`，降级分支当前会以空键判重，后续同类降级输入可能被视为重复；修改此处必须与 Go 的 `HandleError` 后续行为一并核对。
- Datum 转 DECIMAL/DOUBLE 的错误由 `?` 原样向 `calculateSum` 调用方传播；`ComputePlus` 错误被转成 crate 错误字符串。
- 累计值若不是 NULL、DOUBLE 或 DECIMAL，返回 `invalid value ... for aggregate`。这保护通用 SUM/AVG 状态不被整数或其他 Datum 直接污染；整数专用 `sum_int.rs` 有独立累计路径。
- NULL 对两项工具的含义不同：`calculateSum` 将 NULL 视为“不改变累计值”；`Check` 可以编码并记住包含 NULL 的参数元组。是否在调用 `Check` 前跳过 NULL 由具体聚合决定，例如 COUNT 与 GROUP_CONCAT 会先返回，而独立测试直接验证检查器自身能稳定判重含 NULL 的组合。
- `MVMap::Put` 在此接口中无错误返回；文件没有显式内存上限。高基数 DISTINCT 会让集合随唯一键数量增长，生命周期直到上下文重置或释放。

## 并发与资源生命周期

`EvalContext` 通过 `Arc<dyn EvalContext>` 共享，并受 trait 的线程安全约束；但 `Check` 需要 `&mut self`，`MVMap` 和键缓冲也都属于单个检查器，因此本文件没有为一个检查器的并发调用提供同步。正常模型是聚合求值上下文由一个执行路径可变持有；若未来跨线程共享聚合状态，应在外层分片或加锁，而不能仅依赖 `Arc`。

构造时分配 `MVMap`；每次首次键写入都会延长集合占用。成功编码时 `key` 缓冲可复用，输入 `values` 按值传入并在编码后释放。`ResetContext` 通过替换检查器一次性释放旧集合和键缓冲；对象析构依赖 Rust 所有权，无显式 close、事务、通道、锁、异步任务或外部 I/O。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/aggregation/util.go`，Rust 基本逐分支保留其语义：

- Go `distinctChecker.existingKeys/key/ctx` 分别对应 Rust `existing_keys/key/ctx`；构造函数都创建空 `MVMap`。
- 两版 `Check` 都按求值上下文时区调用 `codec.EncodeValue`，再经错误上下文处理，然后以 MVMap 是否已有值决定返回值，首次出现时写入空值。
- Go 额外保存 `vals [][]byte` 作为 `Get` 的复用缓冲；Rust 未保存等价字段，而是每次传 `Vec::new()`。这是潜在分配性能差异，不是可观察 SQL 语义差异。
- Go `calculateSum` 与 Rust 对 NULL、整数、DECIMAL、其他类型以及累计状态种类的分支一致。Rust 的 DECIMAL Datum 可直接移动，无需 Go 的 `v.Copy(&data)`；所有权模型不同但目标语义相同。
- Go 对转换错误先写命名返回值再统一检查；Rust 用 `?` 提前返回。Go 无效累计类型分支返回当前 `data` 加错误，Rust 只返回 `Err`；现有 Rust 调用方使用 `?` 丢弃错误值，因此成功/失败控制流一致。

`pkg/expression/aggregation/util_test.go::TestDistinct` 与 `util_test.rs::TestDistinct` 使用相同的六个序列，覆盖首次/重复的两整数元组及含 NULL 元组。Rust 当前没有在 `util_test.rs` 中直接逐分支测试 `calculateSum`；其主要行为由聚合调用链和 `main_test.rs` 中 `ComputePlus` 的 DECIMAL/混合整数契约间接覆盖，新增转换边界时应补独立测试。

## 扩展指南

- 新增需要多参数 DISTINCT 的聚合时，应复用 `AggEvaluateContext.DistinctChecker`，在具体聚合已完成其 NULL 过滤规则后，把语义上完整且顺序稳定的 Datum 元组传给 `Check`；同步扩展独立测试文件，而不要把测试写入 `util.rs`。
- 改变键编码、排序规则或时区语义时，修改点是 `distinctChecker::Check`，并需要同时核对 `codec::EncodeValue`、Go `util.go`、NULL/时间/字符串排序规则组合，以及所有四类现有调用者。编码兼容变化可能改变 DISTINCT 结果，属于高正确性风险。
- 若优化高基数 DISTINCT 的分配，可考虑复用查询结果缓冲或调整 MVMap 接口，但要保留键缓冲跨调用复用、首次写入/重复跳过的不变量，并用基准或分配数据证明收益；不可为了减少内存而丢弃已见键。
- 新增 SUM/AVG 可接受类型或混合类型规则时，应在 `calculateSum` 的 `value.Kind()` 和 `sum.Kind()` 两处成对审查，并与 Go 版本及 `types::ComputePlus` 能力同步。重点测试 NULL 首值、整数到 DECIMAL、DECIMAL 精度、DOUBLE、转换错误、溢出和非法累计状态。
- 测试应放在同目录独立文件：判重与本工具分支放 `util_test.rs`，通用聚合行为可扩展 `aggregation_test.rs`，AVG 阶段合并放 `avg_test.rs`；同时核对 Go 的 `util_test.go`/相关聚合测试，保持迁移语义。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/aggregation/util.rs` 报告目标文件含 5 个符号。
- RustCodeGraph 源码与关系查询：`node --file pkg/expression/aggregation/util.rs --offset 1 --limit 260`；并读取图中直接调用文件 `aggregation.rs`、`avg.rs`、`count.rs`、`concat.rs`、`sum_int.rs` 的相关区段。精确 `callers/callees` 对目标限定符未返回边，因此调用关系又由这些文件中的直接符号引用复核，没有把宽泛探索的无关同名结果作为依据。
- crate 与模块证据：`pkg/expression/aggregation/Cargo.toml`、`pkg/expression/aggregation/lib.rs`。
- Go 对照：`pkg/expression/aggregation/util.go`，以及直接调用点所在的同目录 Go 聚合文件。
- 测试证据：`pkg/expression/aggregation/util_test.rs`、`pkg/expression/aggregation/util_test.go`、`pkg/expression/aggregation/main_test.rs`、`pkg/expression/aggregation/avg_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证本文恰好包含 11 个固定二级章节，并人工复核符号、调用边、边界与扩展建议均有上述文件依据。
