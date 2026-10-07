# `pkg/expression/aggregation/aggregation.rs`

## 文件定位

本文件位于 `astersql-expression-aggregation` crate，是行式聚合的公共运行时层：它不实现某一种 SQL 聚合算法，而是定义所有具体实现共同遵守的 `Aggregation` trait、每个分组使用的 `AggEvaluateContext`、分布式阶段枚举 `AggFunctionMode`，并提供从 tipb 表达式构造具体聚合器的 `NewDistAggFunc`。crate 入口 `pkg/expression/aggregation/lib.rs` 将本模块及 `sum.rs`、`avg.rs`、`count.rs`、`max_min.rs` 等具体实现统一再导出。

在完整链路中，规划阶段使用 `CheckAggPushDown` 判断 `AggFuncDesc` 是否可下推；存储或 mock coprocessor 收到 tipb 聚合表达式后用 `NewDistAggFunc` 恢复行式聚合器；执行方为每个分组创建上下文，逐行调用 `Update`，最后读取部分结果或最终结果。Rust 规划器直接入口见 `pkg/planner/core/operator/physicalop/base_physical_agg.rs:913`；Go mock TiKV/UniStore 的对应运行入口见 `pkg/store/mockstore/mockcopr/cop_handler_dag.go:336`、`pkg/store/mockstore/unistore/cophandler/cop_handler.go:450` 和 `pkg/store/mockstore/unistore/cophandler/mpp.go:599`。

## 核心职责

1. `NewDistAggFunc` 把 `tipb::Expr` 的子表达式解码为 `ExprBox`，把 tipb 聚合类型映射为 AST 名称与具体 Rust 结构，并把 PB 中的阶段模式写回 `AggFuncDesc`。
2. `Aggregation` 统一“创建/重置分组状态、按行更新、输出部分/最终结果”的调用协议，使 `sumFunction`、`avgFunction`、`countFunction`、`concatFunction`、极值和位运算实现可被同一执行器驱动。
3. `AggEvaluateContext` 集中保存一个分组的可变中间状态，包括计数、当前值、去重器、可变长缓冲和 FIRST_ROW 标志。
4. `aggFunction` 保存公共描述符，并实现上下文初始化、复用重置和 SUM/AVG 共用的非 NULL、DISTINCT、累加逻辑。
5. `NeedCount`、`NeedValue`、`IsMaxMinCount` 和 `IsAllFirstRow` 为规划/执行接线提供聚合属性分类。
6. `CheckAggPushDown`、`checkVectorAggPushDown` 和 `CheckAggPushFlash` 组合语法、数据类型、存储能力和会话开关，形成最终下推决策。

## 主要符号

- `NewDistAggFunc(expr, field_types, ctx) -> Result<(Box<dyn Aggregation>, AggFuncDesc), Error>`：支持 `SUM`、`SUM_INT`、`COUNT`、`AVG`、`GROUP_CONCAT`、`MAX/MIN`、`MAX_COUNT/MIN_COUNT`、`FIRST_ROW` 与三种位聚合。未知 tipb 类型返回错误。MAX/MIN 根据参数类型选择 collator；MAX_COUNT/MIN_COUNT 在 `FinalMode`/`Partial2Mode` 且存在第二参数时用第二参数确定比较规则。
- `Aggregation`：对象安全的行式聚合 trait。`Update` 接收可变状态、语句上下文和一行；`GetPartialResult` 可返回多列中间值；`GetResult` 返回单一最终 `Datum`；`CreateContext`/`ResetContext` 管理分组状态生命周期。
- `AggEvaluateContext`：字段 `Ctx`、`DistinctChecker`、`Count`、`Value`、`Buffer`、`BufferInitialized`、`GotFirstRow` 分别承载表达式上下文、去重状态、计数、标量累计值、GROUP_CONCAT 字节缓冲及其空值判别、FIRST_ROW 是否已命中。
- `AggFunctionMode`：`#[repr(i32)]` 的五阶段枚举；`CompleteMode`、`FinalMode`、`Partial1Mode`、`Partial2Mode`、`DedupMode` 常量保持 Go 风格调用面，`ToString` 给出稳定名称。输入/输出语义分别是原始到最终、部分到最终、原始到部分、部分到部分、原始到去重后原始。
- `aggFunction` / `newAggFunc`：具体聚合器内嵌的公共部分；由 `AggFuncDesc::from_runtime` 建立描述符。
- `aggFunction::updateSum`：求值第一个参数，忽略 NULL，对 DISTINCT 值执行 `distinctChecker::Check`，用 `calculateSum` 更新值，并且只在实际纳入一行后增加 `Count`。`sum.rs:34` 和 `avg.rs:61` 是直接使用者。
- `NeedCount` / `NeedValue` / `IsAllFirstRow`：纯分类函数。COUNT、AVG、MAX_COUNT/MIN_COUNT 需要计数；值集合还包括 SUM、FIRST_ROW、极值、GROUP_CONCAT、位聚合和 APPROX_PERCENTILE；空切片对 `IsAllFirstRow` 返回真，符合迭代器 `all` 语义。
- `CheckAggPushDown`：总入口；`checkVectorAggPushDown` 是私有向量限制，`CheckAggPushFlash` 是 TiFlash 类型与函数白名单。

## 执行流程

分布式构造流程如下：

1. `NewDistAggFunc` 调用 `expression::PBToExprs`，使用调用方给出的字段类型和 `BuildContext` 恢复所有参数表达式；解码错误直接向上返回。
2. 按 `tipb::ExprType` 选择 AST 函数名，通过 `newAggFunc` 构造公共描述符，并用 `PBAggFuncModeToAggFuncMode` 恢复阶段模式。
3. 按相同 PB 类型装箱具体结构。涉及字符串比较的 MAX/MIN 系列从比较参数的字段类型取得 collation；二阶段 MAX_COUNT/MIN_COUNT 的值位于第二参数，因此索引选择受 mode 影响。
4. 返回 trait object 和描述符克隆；调用者以描述符了解模式/参数，以 trait object 执行聚合。

一个分组的运行流程是：`CreateContext` 初始化空状态；执行器反复调用具体实现的 `Update`；SUM/AVG 的原始数据阶段委托 `updateSum` 做表达式求值、NULL 过滤、可选去重与累计；需要中间交换时调用 `GetPartialResult`，完成时调用 `GetResult`；上下文跨分组复用时调用 `ResetContext`，它重建 DISTINCT checker 并清零所有公共字段。

下推判断按短路顺序执行：非 GROUP_CONCAT 的 `ORDER BY`、APPROX_PERCENTILE、非 TiFlash 的 APPROX_COUNT_DISTINCT 或不支持的向量聚合先拒绝；MAX_COUNT/MIN_COUNT 额外要求 TiFlash、单参数且非 Dedup；随后 TiFlash 进入白名单与类型检查，TiKV 明确拒绝 GROUP_CONCAT；最后还必须通过 `expression::IsPushDownEnabled` 的会话/配置开关。

## 数据与状态

`AggEvaluateContext` 是“每个聚合函数、每个分组”一份的状态，而不是全局状态。`Count` 同时服务 COUNT、AVG 和 MAX_COUNT/MIN_COUNT；`Value` 是可空的通用 `Datum`；`Buffer` 与 `BufferInitialized` 配合区分“没有有效输入，结果为 NULL”和“有效结果恰为空字符串”；`GotFirstRow` 避免 FIRST_ROW 被后续行覆盖。`DistinctChecker` 仅在描述符的 `HasDistinct` 为真时创建，重置时不是清空旧对象，而是基于新的 `EvalContext` 重新创建。

`AggFunctionMode` 的整数表示需与 tipb/Go 保持一致，不能重排。描述符持有表达式对象并被具体聚合器拥有；`NewDistAggFunc` 返回的是 `AggFuncDesc::Clone()`，使调用方取得独立描述符值。表达式求值上下文使用 `Arc<dyn EvalContext>` 共享只读能力，而累计字段只通过 `&mut AggEvaluateContext` 修改。

## 依赖与调用关系

crate 边界由 `pkg/expression/aggregation/Cargo.toml` 定义，包名为 `astersql-expression-aggregation`，`lib.rs` 为入口且关闭 doctest；`package.metadata.porting.go-package` 指向 `pkg/expression/aggregation`。本文件的主要直接依赖是：

- `expression`：PB 表达式解码、表达式求值、`BuildContext`/`EvalContext`、下推开关和错误构造。
- `tipb`：输入表达式类型和聚合 mode；依赖固定到 tipb Git revision `07f0ea6b6bffa9d8ac100d81ee51dbbfe4dda3bf`。
- `chunk`、`stmtctx`、`types` 门面：行输入、语句级类型上下文与中间 `Datum`。
- `collate`：MAX/MIN 系列比较器构造。
- `kv::StoreType`、`mysql`、`ast`：存储分类、字段类型和规范化聚合名称。
- crate 内 `descriptor.rs`、`util.rs` 及各具体聚合文件：分别提供 `AggFuncDesc`、mode 转换/求和/去重辅助和 trait 实现。

RustCodeGraph 对目标文件的索引报告 35 个符号，并显示该文件被 12 个文件使用。精确调用证据包括：`sum.rs:34` 与 `avg.rs:61` 调用 `updateSum`；`base_physical_agg.rs:913` 调用 `CheckAggPushDown`；`go_merge_44_test.rs:116` 调用 `NewDistAggFunc`。同路径 Go 运行链还显示 mock coprocessor 在构建 DAG/MPP 聚合执行器时创建聚合器，其执行器在 `aggregate.go:202`、`closure_exec.go:1155` 等位置按分组创建上下文。

## 错误处理与边界

`NewDistAggFunc` 会传播 PB 子表达式解码错误，并对未知 `ExprType` 返回带类型值的明确错误；`updateSum` 会传播参数表达式求值、DISTINCT 编码/检查以及数值累计错误。下推判断不返回错误，任何能力或类型不满足都保守地返回 `false`。

本文件依赖上游已构造合法描述符：MAX/MIN 构造、向量类型判断以及 TiFlash 的 SUM/AVG/GROUP_CONCAT 分支都会访问首参数；如果非法零参数描述符绕过正常构造路径，可能发生索引越界。MAX_COUNT/MIN_COUNT 的 Final/Partial2 参数布局也必须遵守 `[count, extrema value]` 约定。扩展 PB 类型时必须同时更新类型到名称和类型到实现的两个 `match`，否则前者接受而后者会落入 `unreachable!()`。

NULL 不增加 SUM/AVG 的 `Count`，DISTINCT 重复值也不增加；只有 `calculateSum` 成功后才提交新值和计数。`ResetContext` 将 `Value` 置 NULL、清空字节缓冲并重置计数/标志，防止复用时跨组泄漏。`IsAllFirstRow([])` 返回真是当前实现的边界事实，调用方若不接受空集合含义应先自行判断长度。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`Arc<dyn EvalContext>` 允许多个分组状态共享求值上下文，但 `Aggregation::Update` 需要 `&mut self` 和 `&mut AggEvaluateContext`，因此同一个聚合器/分组状态不能在没有调用方同步的情况下并发更新。

状态生命周期由执行器管理：构造聚合器通常长于单个分组；每个分组通过 `CreateContext` 获得状态；输出完成后可丢弃，或用 `ResetContext` 原地复用。重置会释放 `Buffer` 中的逻辑内容但保留其容量；DISTINCT checker 被替换，旧 checker 随赋值释放；`Arc` 在状态销毁或换上下文时自动递减引用计数。大分组的内存主要增长点是 DISTINCT 状态和 GROUP_CONCAT 缓冲，本文件不设额度，限制与截断由具体实现/上层配置负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/aggregation.go`，Rust 基本保持其公共 API、mode 数值顺序、支持的 PB 聚合类型、SUM/AVG 更新顺序以及下推白名单。Rust 用 `Box<dyn Aggregation>`/`Result`/`Arc` 表达 Go 的 interface、多返回值 error 和引用式 context；`PBToExprs` 代替 Go 中逐 child 调用 `PBToExpr` 的循环；描述符由值克隆返回，而 Go 返回指针。

状态布局有一项显式 Rust 化：Go 的 `Buffer *bytes.Buffer` 以 nil 区分未初始化，Rust 使用 `Vec<u8>` 加 `BufferInitialized`。Rust 公共 `ResetContext` 还明确清零 `Count`、缓冲和 `GotFirstRow`，而 Go 基类只替换 context/checker 并将 `Value` 设 NULL，Go 的具体聚合实现承担额外重置；因此不能仅比较基类函数行数判断语义缺失。

当前 Rust 在 MAX_COUNT/MIN_COUNT 的两阶段比较参数选择和下推约束上与 Go 同步；`go_merge_44_test.rs` 覆盖 PB 往返、Complete/Partial/Final 形态、Dedup 拒绝和上下文重置。Go 测试 `aggregation_test.go` 是行为基准，Rust 的 `aggregation_test.rs` 转发到独立的 `aggregation_aster_unit_test.rs`，符合测试与生产源码分离要求。

## 扩展指南

新增一种行式聚合时，至少需要同步以下接点：

1. 在独立生产文件中实现 `Aggregation`，不要把测试或具体算法塞入本文件；在 `lib.rs` 声明并再导出模块。
2. 若支持从存储 PB 恢复，在 `NewDistAggFunc` 的名称映射和具体装箱两个 `match` 同时添加分支；核对 mode、参数布局、collation 和空输入语义。
3. 若规划拆分需要计数或值，在 `NeedCount`/`NeedValue` 更新分类，并检查 `descriptor.rs` 的 Split、返回类型与默认值逻辑。
4. 若可下推，更新 `CheckAggPushDown`/`CheckAggPushFlash` 以及向量、JSON、Duration、ORDER BY、DISTINCT 和目标存储限制；默认应保守拒绝未知能力。
5. 在同目录独立 `*_test.rs` 中增加 PB 构造、空集、NULL、DISTINCT、Reset、所有相关 mode 和拒绝下推条件的回归；同步对照 `aggregation_test.go`，必要时增加与 Go 合并提交对应的独立用例。

兼容风险主要是 mode 整数重排、部分结果列顺序变化和下推能力高估；正确性风险集中在 NULL/DISTINCT 计数及 Reset 跨组污染；性能风险集中在每组重建 DISTINCT checker、表达式克隆以及无界去重/拼接状态。修改后应优先运行聚焦的 aggregation crate 测试，但本任务是纯文档分析，按计划不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/aggregation/aggregation.rs` 确认目标文件在索引内；`node --file ... --offset 1 --limit 420` 完整读取 387 行并报告 35 个符号及 12 个使用文件；`explore "pkg/expression/aggregation/aggregation.rs aggregation.rs AggFunc UpdateContext PartialResult"` 给出 `NewDistAggFunc -> newAggFunc`、`CheckAggPushDown -> checkVectorAggPushDown/CheckAggPushFlash` 及相关调用面。
- 已读生产与边界文件：`pkg/expression/aggregation/aggregation.rs`、`aggregation.go`、`Cargo.toml`、`lib.rs`，以及直接实现/调用搜索命中的 `sum.rs`、`avg.rs`、`pkg/planner/core/operator/physicalop/base_physical_agg.rs` 和 Go mockstore 入口。
- 已读独立测试：`pkg/expression/aggregation/aggregation_test.rs`、`aggregation_aster_unit_test.rs`、`go_merge_44_test.rs` 与 Go 基准 `aggregation_test.go`。关键覆盖包括 mode/PB 往返、SUM/AVG NULL 与 DISTINCT、COUNT FinalMode、Reset、下推分类以及 MAX_COUNT/MIN_COUNT 的完整/最终阶段。
- 人工复核：文档中的函数名、字段名、mode、支持列表和拒绝条件均可回链到上述源码；未把 RustCodeGraph 的同名 Go 符号误当作 Rust 运行调用者，也未宣称 Cargo 测试已运行。
- 结构验证使用任务文件指定命令，要求目标文档存在且恰有 11 个固定二级标题；验证结果在交付前记录。
