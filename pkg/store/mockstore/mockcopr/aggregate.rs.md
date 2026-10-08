# `pkg/store/mockstore/mockcopr/aggregate.rs`

## 文件定位

本文件实现 mock Coprocessor DAG 中的两个聚合执行器：`hashAggExec` 与 `streamAggExec`。它属于 Cargo crate `astersql-store-mockstore-mockcopr`；crate 入口 `pkg/store/mockstore/mockcopr/lib.rs` 以私有模块 `aggregate` 装配本文件，并在测试配置下把独立文件 `aggregate_test.rs` 纳入同一 crate。这里的实现服务于 mockstore 的存储侧下推执行，不是 SQL 层通用聚合执行器 `pkg/executor/aggregate/**`。

在请求主链中，`coprHandler::handleCopDAGRequest` 先经 `buildDAGExecutor` 和 `buildDAG` 自底向上组装执行器；`coprHandler::buildExec` 遇到 `ExecutorSpec::HashAgg` 或 `ExecutorSpec::StreamAgg` 时分别调用 `buildHashAgg`、`buildStreamAgg`，然后以 `executor::SetSrcExec` 挂接前一个算子。根执行器由 `handleCopDAGRequest` 反复调用 `Next` 排空，聚合行最终交给 `fillUpData4SelectResponse` 编码（证据：`cop_handler_dag.rs` 的 `handleCopDAGRequest`、`buildDAG`、`buildExec`、`buildHashAgg`、`buildStreamAgg`）。

## 核心职责

- `AggState` 为每个聚合调用保存运行态，覆盖 `COUNT`、`SUM`、`MIN`、`MAX`、`FIRST` 五类 `AggKind`。
- `hashAggExec` 消费完全部上游输入，以分组键为索引保存每组状态，再逐组输出；它适合输入未按分组键排序的情形，但内存占用随分组数增长。
- `streamAggExec` 假设输入已按分组键相邻排列，只保留当前组状态和一行跨组暂存值；它在遇到新键或上游结束时输出一组结果。
- 两个执行器共同负责表达式求值、SQL 空值的基础聚合规则、结果行布局、上游统计透传，以及本算子的 `ExecDetail` 采集。

本文件实现的是简化 mock 语义：聚合输入和分组表达式使用本 crate 的 `Expr`/`Datum`，没有 Go 版本完整的 session 类型上下文、collation、protobuf 解码与 partial-result 编码层。

## 主要符号

- `pub enum AggState`：聚合状态和结果的内部表示。`Count(u64)` 从零开始；其余四类以 `Option<Datum>` 区分“尚无值”和已有值。
- `AggState::new(&AggCall)`：按 `AggKind` 创建空状态。它是私有方法，仅由执行器初始化状态向量。
- `AggState::update(Datum)`：应用一条已求值输入。`COUNT` 忽略 `NULL`；`SUM`/`MIN`/`MAX` 忽略 `NULL`；`FIRST` 保留第一次输入，包括 `NULL`。
- `AggState::result()`：把状态转为输出 `Datum`。空 `COUNT` 为 `Datum::Uint(0)`，其余空状态为 `Datum::Null`。
- `pub type aggCtxsMapper = BTreeMap<Vec<Datum>, Vec<AggState>>`：HashAgg 的“分组键到聚合状态向量”映射。公开性主要是为了与同 crate 代码/迁移形状对齐；模块本身未从 `lib.rs` 再导出。
- `eval_group_key`：依序求值全部 `groupByExprs`，形成 `Vec<Datum>` 分组键；任一表达式错误立即返回。
- `update_aggregates`：将 `states` 与 `calls` 逐项配对，求值调用表达式后更新状态；无表达式时提供 `Datum::Int(1)`，用于 `COUNT(*)`。
- `output_row`：固定按“全部聚合结果在前、全部分组键在后”构造 `Row`。
- `append_details`：递归取得上游 `ExecDetails`，再追加本算子的明细，因此顺序为叶子到根。
- `pub struct hashAggExec`：HashAgg 的配置、分组映射、首次出现顺序、输出队列、执行标记、调用计数、执行明细和上游所有权。
- `hashAggExec::{new, innerNext, getGroupKey, aggregate, getContexts}`：分别完成构造、拉取并聚合一行、分组键求值、并入分组，以及只读查询已有状态。
- `pub struct streamAggExec`：StreamAgg 的配置、当前键/状态、跨组 `pending` 行、结束标记、统计和上游所有权。
- `streamAggExec::{new, getPartialResult, meetNewGroup}`：分别构造执行器、格式化当前组结果、判断一行是否相对当前键开启新组。
- 两个 `impl executor`：实现 `SetSrcExec`、`GetSrcExec`、`ResetCounts`、`Counts`、`ExecDetails` 和核心拉取接口 `Next`。

## 执行流程

HashAgg 的流程如下：

1. `coprHandler::buildExec` 创建 `hashAggExec::new`，并在它不是 DAG 叶子时通过 `SetSrcExec` 绑定上游。
2. 首次 `hashAggExec::Next` 进入 `!executed` 分支；`innerNext` 循环调用上游 `Next`，每取得一行便调用 `aggregate`。
3. `aggregate` 通过 `getGroupKey`/`eval_group_key` 计算键。新键会追加到 `groupKeys`；`aggCtxsMap.entry` 为该键惰性创建与 `aggExprs` 等长的状态向量，随后 `update_aggregates` 更新各项。
4. 上游结束后，若输入为空且没有 `GROUP BY`，代码显式建立空键和空状态，从而仍输出一行（例如空输入上的 `COUNT(*) = 0`）。有 `GROUP BY` 的空输入不产生组。
5. 每个 `groupKeys` 条目通过 `output_row` 物化到 `rows`，设置 `executed = true`；本次及后续 `Next` 从队首返回一行，直至 `None`。虽然状态映射是 `BTreeMap`，输出顺序实际由记录首次见到顺序的 `groupKeys` 决定。

StreamAgg 的流程如下：

1. `streamAggExec::Next` 若已 `finished`，直接返回 `None`；否则优先取上一轮保存的 `pending` 行，没有时才拉取上游。
2. 若尚未取得任何行就遇到 EOF：有 `GROUP BY` 时返回 `None`；无 `GROUP BY` 时建立空键并返回一行空聚合结果。下一次调用返回 `None`。
3. 取得首行后，计算其键为 `current_key`，重建 `current_states`，并以该行更新聚合状态。
4. 循环拉取后续行：同键则继续更新；不同键则将该行保存到 `pending`，输出当前组；EOF 则标记 `finished` 并输出当前组。
5. `pending` 只缓存一行，因此下次调用从该行开始新组。正确性依赖“相同分组键在输入中连续”的外部前置条件；本文件不会排序，也不会检测某个旧键在更晚位置再次出现。

两个 `Next` 都在入口记录 `Instant`、递增公开的 `count`，并在成功或失败后更新 `execDetail`；错误调用也计一次 iteration，但不增加 produced rows。

## 数据与状态

`AggCall`（定义于 `copr_handler.rs`）把 `AggKind` 与可选 `Expr` 组合起来。`Expr::eval` 对列越界返回 `CopError::ColumnOffset`，算术类型不匹配返回 `CopError::Type`；`Datum` 支持 `Null`、`Int`、`Uint`、`Real` 和 `Bytes`。分组键使用 `Vec<Datum>` 的 `Ord`/`Eq` 语义：数值族可跨有符号、无符号和浮点值比较，字节串按字节序比较，`NULL` 最小；这不是 Go 版本基于 session 类型上下文和 collator 的完整 SQL 比较体系。

`SUM` 只接受同一种数值变体连续相加：`Int+Int`、`Uint+Uint` 使用饱和加法，`Real+Real` 使用浮点加法；跨数值变体或非数值会报类型错误。`MIN`/`MAX` 使用 `Datum` 的全序。`FIRST` 的 `Option` 同时承担“未见输入”的标记，因此第一次输入即使为 `NULL` 也会固定为 `Some(Null)`。

HashAgg 持有所有组的 `aggCtxsMap`、首次见到顺序 `groupKeys` 以及已物化输出 `rows`，`executed` 保证上游只排空一次。StreamAgg 只持有 `current_key`、`current_states`、至多一行 `pending` 和 `finished`。两者的 `count` 记录本算子 `Next` 被调用次数，但 `Counts()` 只透传扫描链上的上游计数，不返回该字段；性能明细通过 `ExecDetails()` 返回。

## 依赖与调用关系

直接标准库依赖是 `BTreeMap`、`VecDeque` 和 `Instant`。crate 内依赖如下：

- `copr_handler::{AggCall, AggKind, CopError, Datum, ExecDetail, Expr, Row}` 提供协议形状、值/表达式、错误和统计类型。
- `executor::{executor, NextRow}` 提供拉取式算子接口与返回类型。
- 上游构造者是 `cop_handler_dag.rs` 的 `coprHandler::buildExec`；它经 `buildHashAgg`/`buildStreamAgg` 调用本文件的构造函数，并经 `SetSrcExec` 形成执行器链。
- 运行时直接下游调用包括 `Expr::eval`、`AggState::{new,update,result}`、上游动态分派的 `executor::{Next,ResetCounts,Counts,ExecDetails}`，以及 `ExecDetail::update`。
- 根消费方是 `coprHandler::handleCopDAGRequest`：反复调用根 `Next`，再读取 `Counts`/`ExecDetails` 并编码响应。

`Cargo.toml` 声明本 crate 的 `[lib] path = "lib.rs"`，并记录 Go 包映射 `pkg/store/mockstore/mockcopr`。其中列出的业务 crate 依赖均为 optional；本文件当前只使用 crate 内简化类型，没有直接引用这些外部 optional crate。独立测试由 `lib.rs` 的 `#[cfg(test)] #[path = "aggregate_test.rs"]` 接入。

RustCodeGraph 对目标文件报告 44 个符号；精确 `query` 能定位 `hashAggExec`、`streamAggExec`、`update_aggregates`、`cop_handler_dag.rs::buildHashAgg` 等符号。图的通用名 `executor`/`Next` 存在大量同名候选，精确 `callers`/`callees` 调用未返回可用边，因此上述局部调用边以已索引文件的 `node` 源码和构建链交叉验证，不把噪声 blast-radius 当作本模块调用关系。

## 错误处理与边界

- 两个执行器在未绑定 `src` 时调用 `Next`，分别返回包含 `hash aggregate has no source` 或 `stream aggregate has no source` 的 `CopError::InvalidRequest`，不会 panic。
- 分组或聚合表达式的错误通过 `?` 原样传播，包括列偏移越界、`Add` 类型错误等；聚合在首个错误处停止。
- `SUM` 遇到不同数值变体或非数值输入返回 `CopError::Type("sum expects one numeric type")`。已通过 `slot.take()` 取出的旧值在这个错误路径不会恢复；请求随即失败，因此当前调用链不会继续使用该部分状态，但若未来支持错误后重试，需要首先修正这一状态破坏行为。
- `states.iter_mut().zip(calls)` 会静默取较短长度。本文件自己的构造路径始终从 `aggExprs` 创建等长状态，维持不变量；若未来开放外部状态注入，应显式校验长度。
- HashAgg 的 `getContexts` 对未知键返回 `None`；正常 `Next` 只遍历已登记并已插入映射的 `groupKeys`，随后用索引 `self.aggCtxsMap[key]`，依赖这两个容器同步的不变量。
- 无 `GROUP BY` 的空输入会输出一行；带 `GROUP BY` 的空输入无输出。`COUNT` 空结果为无符号零，其余为空值。
- StreamAgg 不验证排序前置条件；非连续的相同键会被当作多个结果组。这是调用者必须保证的边界，而非本地错误。
- `MIN`/`MAX` 和分组相等性继承简化 `Datum` 规则，没有 Go 版本的字符集、collation、时区和 statement error-context 处理。

## 并发与资源生命周期

执行器 trait 要求 `Send`，但本文件内部没有线程、异步任务、锁、通道或共享可变状态；每个执行器由 `Box<dyn executor>` 独占上游，以 `&mut self` 串行拉取。因而并发安全依靠所有权隔离，而不是同步原语。

HashAgg 的资源高峰发生在首次 `Next`：它排空上游，同时保留所有分组状态，随后又把所有输出行物化进 `VecDeque`；其空间复杂度至少随分组数和输出体积线性增长，文件内没有内存配额或 spill。StreamAgg 的聚合工作集与单组状态和一行 `pending` 成正比，但上游本身仍可能缓存数据。

`ExecDetail` 在每次 `Next` 返回前更新，`Instant` 计时覆盖该次调用的全部上游拉取和本地处理。`ResetCounts` 只递归重置上游，并不清空 `count`、聚合状态、`executed`/`finished` 或 `execDetail`；因此它不是“重放执行器”的生命周期方法。执行器被 drop 时，`Box`、映射、队列和行值按 Rust 所有权自动释放，没有显式关闭协议。

## 与 Go 版本的对应关系

同路径 `aggregate.go` 定义同名 `aggCtxsMapper`、`hashAggExec`、`streamAggExec`，并实现同一类 `executor` 接口方法；Rust 保留了“HashAgg 全量分组、StreamAgg 遇新组输出、统计透传、执行耗时更新”的总体骨架。

关键差异必须在扩展时显式考虑：

- Go 的聚合函数来自 `pkg/expression/aggregation.Aggregation`，状态为 `AggEvaluateContext`，一个聚合可产生多个 partial result；Rust 固定为五种 `AggKind`，每个调用只输出一个 `Datum`。
- Go 从 protobuf/表达式上下文构造执行器，先解码相关列，并使用 session `StmtCtx`、时区、error context 和 codec 编码；Rust 的行已是 `Vec<Datum>`，直接求值并返回结构化行。
- Go HashAgg 以编码后的字节串为 map key，同时单独保存分组输出字节；Rust 以 `Vec<Datum>` 为键，并直接把分组值追加到结果行。
- Go StreamAgg 使用 `groupByCollators` 和 `Datum::Compare(TypeCtx, collator)` 判断分组；Rust 用 `Vec<Datum>` 相等性。因此字符串校对和完整类型转换语义尚未对齐。
- Go 的 `getPartialResult` 会输出 partial-result 形状并重建 aggregation context；Rust 的 `getPartialResult` 只是读取当前 `AggState`，新组状态在下一次 `Next` 开始时整体重建。
- Rust 显式覆盖了空输入且无分组时的一行结果，并以独立测试确认 StreamAgg 的执行明细；现有 Rust 独立测试未覆盖具体聚合值、Hash/Stream 分组等价性、排序前置条件或上述 Go 完整语义。

在 `pkg/store/mockstore/mockcopr` 下没有同路径 `aggregate_test.go`。当前目录的 Go 测试文件也未直接引用 `hashAggExec`/`streamAggExec`；因此 Go 行为依据主要来自 `aggregate.go` 本身，而 Rust 回归依据来自 `aggregate_test.rs`。

## 扩展指南

- 新增聚合种类时，至少同步修改 `copr_handler.rs::AggKind`、`AggState`、`AggState::new`、`update`、`result`，并检查输出是否仍是一调用一列；若需要多列 partial result，应先调整 `output_row` 和 DAG 输出偏移契约。
- 修改空值或类型语义时，优先在 `AggState::update` 建立明确规则，并与 Go `Aggregation::Update/GetPartialResult` 对照。尤其要决定跨数值类型 `SUM`、溢出、NaN、字节串和错误后的状态原子性。
- 修改分组相等性时，应从 `eval_group_key` 及 `Datum::{Eq,Ord}` 入手，并评估 Go 的 `StmtCtx`/collator 语义；HashAgg 和 StreamAgg 必须共享同一等价关系，否则同一计划在两种执行器上可能产生不同组。
- 修改 HashAgg 输出次序时，注意当前 `groupKeys` 保证首次出现顺序，`BTreeMap` 只负责确定性的存储/查询；不要误以为直接遍历 map 与现状相同。
- 修改 StreamAgg 时必须保留 `pending` 的“跨调用保存首个新组行”不变量，并清楚记录输入有序要求。若要容忍无序输入，应改用 HashAgg 或在上游显式排序，而不是在这里静默合并非连续组。
- 调整统计或重置语义时，同步检查 `append_details`、两个 `Next` 的成功/错误更新和 `executor` trait；当前独立测试专门要求空输入和上游错误都计入 iteration。
- 测试逻辑应继续放在独立的 `pkg/store/mockstore/mockcopr/aggregate_test.rs`，不要内嵌到生产文件。建议补充：五类聚合的 NULL/空输入行为、SUM 类型与溢出、表达式错误传播、多组首次出现顺序、Hash/Stream 等价结果、Stream 无序输入契约、无 source 错误，以及 `Counts`/`ExecDetails` 链顺序。
- 兼容风险集中于结果行列顺序和 partial-result 形状；性能风险集中于 HashAgg 双重物化和大分组键克隆；正确性风险集中于简化 `Datum` 与 Go session/collation 语义的差距。

## 验证依据

本说明基于以下直接证据人工复核：

- 目标实现：`pkg/store/mockstore/mockcopr/aggregate.rs`（RustCodeGraph `node --file`，完整 1–399 行）。
- crate 边界：`pkg/store/mockstore/mockcopr/Cargo.toml` 与 `pkg/store/mockstore/mockcopr/lib.rs`；前者声明 package、lib 路径、porting 元数据和依赖，后者声明模块与独立测试装配。
- 调用主链：`pkg/store/mockstore/mockcopr/cop_handler_dag.rs` 的 `handleCopDAGRequest`、`buildDAGExecutor`、`buildDAG`、`buildExec`、`buildHashAgg`、`buildStreamAgg`。
- trait 与上游协议：`pkg/store/mockstore/mockcopr/executor.rs` 的 `executor`、`NextRow`、`ExecDetail::update`；`pkg/store/mockstore/mockcopr/copr_handler.rs` 的 `CopError`、`Datum`、`Expr::eval`、`AggKind`、`AggCall`、`ExecutorSpec`、`ExecDetail`。
- Go 对照：`pkg/store/mockstore/mockcopr/aggregate.go`（RustCodeGraph `node --file`，完整 1–364 行）。
- Rust 测试：`pkg/store/mockstore/mockcopr/aggregate_test.rs`；`stream_aggregate_updates_exec_detail_on_empty_input` 验证无分组空输入与明细计数，`aggregates_update_exec_detail_when_the_source_errors` 验证 Hash/Stream 的错误调用也更新明细。
- Go 测试检索：在 `pkg/store/mockstore/mockcopr/**/*_test.go` 中检索 `hashAggExec|streamAggExec|HashAgg|StreamAgg|aggregate` 无命中，未发现本目录直接聚合回归测试。
- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列出 27 个已索引 Go/Rust 文件，目标文件报告 44 个符号。已运行 `query hashAggExec`、`query streamAggExec`、`query update_aggregates`、`query buildHashAgg`；因常见符号重名且精确 `callers`/`callees` 未产出可用结果，调用关系以索引 `node` 展示的真实构造和调用语句复核。

本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证目标文档存在且恰好包含上述 11 个固定二级标题。
