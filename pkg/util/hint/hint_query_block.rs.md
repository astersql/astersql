# [`pkg/util/hint/hint_query_block.rs`](hint_query_block.rs)

## 文件定位

本文件属于 `astersql-util-hint` crate，crate 入口 `pkg/util/hint/lib.rs` 通过 `pub mod hint_query_block` 声明模块并用 `pub use hint_query_block::*` 重导出其公开 API。`pkg/util/hint/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/util/hint`；本文件的直接 Go 对照是 `pkg/util/hint/hint_query_block.go`。

它位于“SQL 语句已解析为 AST”与“优化器根据查询块消费 hint”之间：先为 SELECT 查询块分配 offset，收集显式 `QB_NAME`，拆分视图 hint，再把 hint 按 offset 分发。Rust 中可确认的直接生产入口是 `pkg/util/hint/hint_processor.rs::ParseHintsSet`，它构造 `QBHintHandler`、调用 `Process`，然后使用 offset 查询和 `GenerateQBName` 归一化 hint。`pkg/planner/core/planbuilder_runtime.rs::PendingPlanBuilder::Init` 也接收 handler 并创建 `QBHintBuildState`，但当前 Rust 文本引用中未找到该规划器运行时直接调用本文件 `Process` / `GetCurrentStmtHints` / `MarkViewQBNameUsed` 的生产边；不应将 Go 规划器的完整接线当成 Rust 已接线事实。

## 核心职责

- `QBHintHandler::Process` 以所有权方式遍历并替换 AST：SELECT 按访问顺序从 1 编号，Update/Delete 顶层使用 offset 0，并递归处理 CTE、派生表、集合运算、Join 与表达式子查询（`Process`，源文件 124–335 行）。
- `checkQueryBlockHints` 维护显式查询块名到 SELECT offset 的首次映射，对同块多名和跨块重名发出警告（358–390 行）。
- `handleViewHints` 从当前 SELECT 的 hint 中拆出视图查询块定义及归属于视图的其他 hint，仅把未消费项留给当前 SELECT（392–470 行）。
- `GetCurrentStmtHints` 验证 hint 的块名与表块名，把有效 hint 按 offset 去重累积到单次构建状态，并返回当前块的集合（554–590 行）。
- `GenerateQBName` 在外部需要写回标准块名时生成 `sel_N`、`upd_1` 或 `del_1`（600–616 行）。

## 主要符号

- `hintWarnHandler`：警告接收 trait，提供 `SetHintWarning(String)` 和 `SetHintWarningFromError(&dyn Error)`。本文件当前只直接调用前者；后者是与 Go 警告接口对齐的公开契约（27–34 行）。
- `QBHintHandler`：长寿命元数据容器。`QBNameToSelOffset` 保存名称映射，`ViewQBNameToTable` / `ViewQBNameToHints` 保存视图 hint 元数据，`selectStmtOffset` 记录已访问 SELECT 数，`warnHandler` 是可选副作用出口（36–50 行）。
- `processExprQueries<'a>`：私有表达式 visitor；遇到 `ExprKind::Subquery` 时取出内层 AST，递归调用 `QBHintHandler::Process` 后写回（52–80 行）。
- `QBHintBuildState`：单次计划构建状态。`QBOffsetToHints` 是 offset 到已归档 hint 的映射，`ViewQBNameUsed` 记录已消费视图块名，避免将运行期状态混入可复用 handler（82–90 行）。
- `NewQBHintHandler`、`NewBuildState`、`MaxSelectStmtOffset`：分别构造 handler、按是否有视图名预分配 used set、暴露已编号 SELECT 的最大 offset（92–122 行）。
- `getBlockOffset`、`GetHintOffset`、`checkTableQBName`、`isHint4View`：是块名解析和分类的核心辅助符号（497–552 行）。
- `HandleUnusedViewHints`、`SetWarns`、`MarkViewQBNameUsed`：形成视图 hint 使用跟踪与告警回写链（472–495、518–526、592–597 行）。
- `hintQBName`、`defaultUpdateBlockName`、`defaultDeleteBlockName`、`defaultSelectBlockPrefix`：定义解析和生成时共用的规范名称（100–107 行）。

## 执行流程

1. 上游用 `NewQBHintHandler` 创建处理器。`ParseHintsSet` 在解析 SQL 且收集原始 hint 后，把语句所有权交给 `Process`（`pkg/util/hint/hint_processor.rs:1136-1137`）。
2. `Process` 先对 `ExplainStmt` / `CreateBindingStmt` 直接返回。Update/Delete 先以 offset 0 登记顶层 hint；Select 则先递增 `selectStmtOffset`、写入 `QueryBlockOffset`、拆分视图 hint，再登记普通 `QB_NAME`。
3. 处理器按 AST 的结构顺序递归：CTE 经 `processWith`，字段/WHERE/GROUP/HAVING/VALUES/window/ORDER/LIMIT 中的子查询经 `processExprQueries`，FROM/Join/派生表经 `processJoin` 和 `processTableSource`，Set operation 遍历每个分支。
4. `handleViewHints` 两遍扫描 hint：第一遍登记带表列表的 `qb_name`，并为非顶层视图的首表补 `sel_<offset>`；第二遍将显式指向该名称，或所有表共用该名称的 hint 归档到 `ViewQBNameToHints`。
5. 计划构建前可用 `NewBuildState` 创建每次独立状态。`GetCurrentStmtHints` 遍历输入 hint，跳过 `qb_name`，解析 hint 和表上的块名，对未知名称告警后丢弃，对有效项按 offset 去重缓存，最后返回当前 offset 的副本。
6. 视图计划真正消费某块时应调用 `MarkViewQBNameUsed`；构建结束后 `HandleUnusedViewHints` 生成未使用告警，`SetWarns` 再写入外部 warning handler。

## 数据与状态

`QBHintHandler` 的三张映射和 `selectStmtOffset` 是对一棵已遍历 AST 提取的元数据。其中显式名与视图名都使用 `CIStr.L` 小写字段作键，因此匹配继承 parser 的不区分大小写语义。重名不覆盖旧值，“首次出现生效”是 `checkQueryBlockHints` 和 `handleViewHints` 共同维持的不变量。

`QBHintBuildState` 必须按每次计划构建隔离。`QBOffsetToHints` 在一次调用中也会收集属于其他 offset 的 hint，以便后续块复用同一 state；去重依赖 `TableOptimizerHint` 的值相等。当上游向 `GetCurrentStmtHints` 传 `None` 时，函数使用局部临时 state，因而跨调用不保留分发结果。

offset 约定为：SELECT 从 1 开始，Update/Delete 顶层为 0，解析失败为 -1。`getBlockOffset` 优先查显式名，其次接受 `upd_1` / `del_1`，最后解析 `sel_N`；`N` 大于已遍历 SELECT 数或无法转换时返回 -1。当前实现没有显式拒绝 0 或其他可成功解析且不大于上限的负 `sel_N`；扩展验证时应先与 Go 的 `strconv.ParseInt` 行为和实际调用方预期对齐，不宜单方面收紧。

## 依赖与调用关系

下游依赖很小：`std::collections::{HashMap, HashSet}` 承载状态；`crate::ast` 提供 AST、`CIStr`、visitor 和 hint 数据结构；`crate::hint_processor::{NodeType, RestoreTableOptimizerHint}` 分别用于规范名生成和未知块告警文本；`crate::errors` 为 `GenerateQBName` 提供 crate 统一错误。这些内部名称最终由 `lib.rs` 对 parser、dbterror 等 Cargo 依赖做再导出适配。

可确认的 Rust 上游边包括：

- `pkg/util/hint/hint_processor.rs::ParseHintsSet -> NewQBHintHandler -> QBHintHandler::Process`，随后调用 `isHint4View`、`GetHintOffset`、`checkTableQBName` 和 `GenerateQBName`。
- `pkg/planner/core/planbuilder_runtime.rs::PendingPlanBuilder::Init -> QBHintHandler::MaxSelectStmtOffset / NewBuildState`，并将 handler/state 存入 `PlanBuilder`。
- `pkg/util/hint/hint.rs` 的 hint 表解析调用 `GetHintOffset`（搜索命中 `hint.rs:1407`）。
- `pkg/planner/core/casetest/join/join_test.rs` 和 `pkg/planner/core/casetest/hint/hint_test.rs` 以真实 parser AST 直接驱动 `Process -> GetCurrentStmtHints -> ParsePlanHints`，是当前最直接的集成层证据。

RustCodeGraph 将本文件标记为被 16 个文件使用，并确认 Rust `GenerateQBName` 的调用者包括 `pkg/util/hint/hint_processor.rs::ParseHintsSet` 和相关独立测试。但索引对 impl 方法的精确 `callers/callees` 消歧失败；上述方法边因此用限定在 `*.rs` 的精确名称搜索补齐。

## 错误处理与边界

- `Process` 中的 `downcast::<...>().unwrap()` 依赖紧邻 `as_any().is::<...>()` 检查，在 AST `Node` 的 RTTI 契约成立时不会失败。未匹配的节点原样返回；`ExplainStmt` 和 `CreateBindingStmt` 明确不深入处理。
- 重复 `qb_name`、视图名冲突、视图 hint 混用多个块名和未知块名都是可恢复诊断：有 `warnHandler` 时记录警告，然后保留首值或忽略无效 hint，不返回错误。没有 handler 时这些告警被静默抑制。
- `GenerateQBName` 是本文件唯一返回 `Result` 的公开函数：offset 0 只允许 Update/Delete，其他 `NodeType` 返回 `errors::Error`；非零 offset 一律格式化为 `sel_<offset>`。
- `handleViewHints` 对空 hint 列表立即返回空集；空视图 `QBName` 的定义 hint 仍被标记为已消费，但不登记映射，这与 Go 对照一致。
- `isHint4View` 在 hint 无显式 QBName 且表列表为空时，`Iterator::all` 会真值返回；Go 的初始 `allViewHints := true` 也是同一语义。
- `HandleUnusedViewHints(None, warns)` 保留原向量；有 state 时先 `clear()` 再生成本轮未使用视图告警，调用方不应期望保留传入文本。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、文件或网络资源。所有集合都是普通 `HashMap` / `HashSet`，变更需要 `&mut self` 或 `&mut QBHintBuildState`；类型也没有声明内部同步。因此一个可变 handler/state 应由单个计划构建流独占，而不是在多个构建中并发更新。

生命周期分层是安全扩展的关键：handler 在一棵 AST 遍历期间拥有名称和视图元数据；`NewBuildState` 为每次计划构建产生独立的分发/使用状态；`processExprQueries` 只在 `Accept` 调用期间临时借用 handler。`ExprKind::Subquery::Query.take()` 和 `NodeRef::take()` 将节点所有权短暂移出再写回，避免不安全内部可变性；扩展 AST 分支时必须保证每个取出的节点被替换回原位。

## 与 Go 版本的对应关系

Rust 主要数据结构、常量、告警文本、首次映射优先策略、视图 hint 两阶段拆分、块名解析、按 offset 去重分发和 `GenerateQBName` 分支，都逐项对应 `pkg/util/hint/hint_query_block.go`。`pkg/util/hint/hint_processor_1_aster_unit_test.rs` 还对重名告警、视图拆分、默认 `sel_2`、跨 offset state 复用、未知块忽略及名称生成做了 Rust 回归。

最明显的实现差异是遍历机制：Go `QBHintHandler` 实现 `ast.InPlaceVisitor::Enter/Leave`，由 `ast.Walk` 统一递归；Rust parser visitor 节点不提供同样的可变替换路径，所以 `Process` 消费 `Box<dyn ast::Node>`，针对支持的 statement/result-set/expression 分支手动递归并返回替换后 AST。这不是简化语义：当前源码显式覆盖 Update、Delete、Select、Insert、Set operation、CTE、Join、派生表及各主要表达式容器。但新增 AST 字段/节点时，Go 通用 visitor 可能自然覆盖，Rust 则需在 `Process` 或对应 `process*` 辅助函数中显式补边。

表示层差异包括：Go 保存 `*ast.TableOptimizerHint` 指针，Rust 保存值并在分发时 clone；Go map 可为 nil，Rust map 默认为空；Rust `ViewQBNameUsed` 使用 `Option<HashSet<_>>` 表示 Go nil map；Go `NewBuildState` 对 nil receiver 返回 nil，Rust 借用方法无 nil receiver 状态。这些差异不改变已验证的业务分支。

## 扩展指南

- 新增包含查询的 AST 节点或字段时，先在 `QBHintHandler::Process` 中确认语句类型分支，再将其表达式、ResultSet 或子语句接入现有 `processExpr` / `processResultSet` / `Process`。必须在独立测试文件中断言新分支的 `QueryBlockOffset` 顺序，不要把 Rust 测试内联到本源文件。
- 改动名称解析时需同步审查 `getBlockOffset`、`GetHintOffset`、`checkTableQBName`、`GenerateQBName` 以及 `hint_processor.rs::ParseHintsSet`，并与 Go 对照保持边界一致，特别是 offset 0、超出 `selectStmtOffset`、非数字后缀和显式别名优先级。
- 改动视图 hint 时需把 `handleViewHints`、`isHint4View`、`NewBuildState`、`MarkViewQBNameUsed`、`HandleUnusedViewHints` 作为一个完整生命周期检查，避免只完成拆分却不标记消费，或在 handler 中残留跨 build 状态。
- 新告警应继续走 `hintWarnHandler`，保持“有 handler 则记录，无 handler 则安静退化”，并对照 Go 的文本和触发条件。
- 首选扩展 `pkg/util/hint/hint_processor_1_aster_unit_test.rs` 的聚焦单元测试；需要真实 SQL 解析与 plan hint 绑定时，扩展 `pkg/planner/core/casetest/hint/hint_test.rs` 或 `pkg/planner/core/casetest/join/join_test.rs`。Go 对照回归在 `pkg/sessionctx/stmtctx/stmtctx_test.go::TestQBHintHandlerBuildState` 及规划器 hint 相关测试中。
- 性能风险主要来自 AST 深度、hint 数量和值 clone：`Process` 是树遍历，`handleViewHints` 两遍扫描，`GetCurrentStmtHints` 的 `Vec::contains` 为每个 offset 线性去重。除非有 profile 证据，不应为文档或风格原因更换这些与 Go `slices.Contains` 对齐的策略。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/hint` 返回 9 个 Go/Rust 文件，包含目标 Rust、Go 对照、模块入口和两个独立 Rust 测试。
- RustCodeGraph 源码证据：`node --file pkg/util/hint/hint_query_block.rs` 分段读取了全部 616 行；`node --file pkg/util/hint/hint_query_block.go --offset 220 --limit 160` 核对了 Go 的未使用视图告警、块名解析、分发和名称生成。`explore` 确认 Rust `GenerateQBName` 的 `ParseHintsSet` 与测试调用者；精确 impl 方法图查询无法消除同名符号，该部分已用限定 Rust 文本搜索补齐。
- 读取的 crate/接线路径：`pkg/util/hint/Cargo.toml`、`pkg/util/hint/lib.rs`、`pkg/util/hint/hint_processor.rs`、`pkg/planner/core/planbuilder_runtime.rs`、`pkg/planner/optimize.rs`。目标目录及父级 `pkg/util` 下未发现适用的 `doc.go`。
- 读取的对照/测试路径：`pkg/util/hint/hint_query_block.go`、`pkg/util/hint/hint_processor.go`、`pkg/util/hint/hint_processor_1_aster_unit_test.rs`、`pkg/planner/core/casetest/join/join_test.rs`、`pkg/planner/core/casetest/hint/hint_test.rs`、`pkg/sessionctx/stmtctx/stmtctx_test.go`。关键断言覆盖重名保留首值、视图 hint 拆分、`sel_2` 默认补全、未知块警告与忽略、跨 offset state 复用、Update/Select 块名生成，以及真实 SQL 下的命名查询块绑定。
- 本任务为纯文档分析，按计划不运行 Cargo。交付时使用任务指定的 11 章节结构检查，并人工核对本文不把 Go 生产调用误写为 Rust 已接线事实。
