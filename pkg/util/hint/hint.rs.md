# `pkg/util/hint/hint.rs`

## 文件定位

`hint.rs` 是 `astersql-util-hint` crate 的核心实现文件，由 [`lib.rs`](lib.rs) 通过 `include!("hint.rs")` 纳入 crate 根并公开其符号。它位于 parser 产出的 `ast::TableOptimizerHint` 与 planner/session 消费的语句级、计划级 Hint 状态之间：先把 Hint AST 分类为 `StmtHints` 或 `PlanHints`，再提供表/索引匹配、恢复文本和未命中告警。Hint 只约束优化与执行策略，不改变 SQL 的语义结果。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，直接依赖 parser AST、`dbterror`/planner error、MySQL errno、元数据索引模型和类型常量；无 feature 条件或条件编译逻辑。查询块名字到 select offset 的解析由同 crate 的 [`hint_query_block.rs`](hint_query_block.rs) 提供，AST Hint 的收集与绑定处理则在 [`hint_processor.rs`](hint_processor.rs)。

## 核心职责

1. 定义与 Go 兼容的 Hint 名称、子查询位图和 `Prefer*` 位图。字符串会进入 parser、binding 和 warning 文本，位值会被 planner 直接解释，均属于稳定兼容契约。
2. `ParseStmtHints` 解析作用于整条语句的 Hint，包括内存配额、最大执行时间、只读副本、`SET_VAR`、假想索引、计划缓存、资源组和慢日志开关；同时返回真正生效的输入 offset 与非致命 warning。
3. `ParsePlanHints` 把 join、index、存储引擎、聚合、CTE、limit、leading 和子查询 Hint 路由到 `PlanHints`，通过 `QBHintHandler` 将 query-block 名换算为 select offset。
4. `HintedTable`/`HintedIndex` 记录目标对象及 `Matched` 状态；`IfPrefer*`、`Match` 和 `ShouldPushDownIndexLookUp` 供计划构建时消费并回写匹配状态。
5. `Restore2*`、`RemoveDuplicatedHints` 与 `CollectUnmatchedHintWarnings` 负责稳定文本恢复、按恢复文本去重，以及计划构建结束后的未命中诊断。
6. `RegisterRestrictedHintChecker`/`filterRestrictedHints` 接入外部安全策略，并在 statement/plan 两条解析路径之间划分 warning 归属，避免重复告警。

## 主要符号

- `StmtHints`：语句范围结果。`QueryHasHints` 反映原输入是否非空；值字段与对应 `Has*` 字段区分“未指定”和“指定为零值”；`SetVars` 保存首个合法同名赋值；`HintedHypoIndexes` 是 `db -> table -> index -> IndexInfo` 三层映射；`OriginalTableHints` 保存过滤后的原始 Hint。
- `StmtHints::TaskMapNeedBackUp`：仅以 `ForceNthPlan != -1` 判断物理优化是否需要备份 task map。`Clone` 复制普通值、`SetVars` 和原 Hint，但有意清空 `HintedHypoIndexes`，与 Go 的手写克隆语义一致。
- `ParseStmtHints<SetVarChecker, HypoIndexChecker>`：通过两个回调隔离系统变量准入与目录索引查询，使本文件不依赖 session/catalog 实现；返回 `(StmtHints, Vec<i32>, Vec<errors::Error>)`。
- `RestrictedHintChecker`、`RESTRICTED_HINT_CHECKER`、`RegisterRestrictedHintChecker`、`filterRestrictedHints`：进程级受限 Hint 策略。检查器类型是函数指针，保存在 `RwLock<Option<_>>` 中；解析时复制函数指针后释放读锁。
- `PlanHints`：按 Hint 家族保存 join 表列表、index/index-merge 列表、TiKV/TiFlash 表列表、leading/HJ build/probe、聚合位图及无表参数布尔值。`IndexJoinHints` 将 INLJ、INLHJ、INLMJ 三类表集合成组。
- `HintedTable`：由大小写不敏感的库表名、分区、`SelectOffset` 和 `Matched` 组成。`Match` 要求表名与 offset 一致，并允许任一侧数据库名为 `*`。
- `HintedIndex`：保存目标库表/分区、parser `IndexHint`、lookup pushdown 标志和 `Matched`。`Match` 允许 Hint 侧数据库 `*`；`HintTypeString`/`IndexString` 为 warning 还原类型及对象文本。
- `ParsePlanHints`：返回 `Result<(PlanHints, u64), errors::Error>`；`u64` 使用 `HintFlagSemiJoinRewrite` 与 `HintFlagNoDecorrelate` 表达子查询局部选择。
- `tableNames2HintTableInfo`：补默认库名，调用 `QBHintHandler::GetHintOffset` 解析 query block，并拒绝在不支持的 join/leading Hint 上指定分区。
- `Restore2JoinHint`、`Restore2IndexHint`、`Restore2StorageHint`：生成告警与去重使用的规范 Hint 文本；内部共享 `restore2TableHint`。
- `CollectUnmatchedHintWarnings`：按固定次序聚合 index、index merge、各 join 家族及 storage 的未命中警告；其三个 `collectUnmatched*` 辅助函数只报告 `Matched == false` 的对象。

## 执行流程

语句级路径如下：

1. parser/hint processor 交付 `Vec<TableOptimizerHint>`；`ParseStmtHints` 立即记录 `QueryHasHints`，然后调用 `filterRestrictedHints`。即使全部被过滤，`QueryHasHints` 仍保留“原语句有 Hint”的事实。
2. 第一遍按 Hint 名计数并记录最后一个 offset。`HYPO_INDEX` 校验至少包含表、索引、列，逐列调用 `HypoIndexChecker` 后构造 `IndexInfo { StatePublic, IndexType::Hypo }`；`SET_VAR` 先调用准入回调，同名变量只保留第一次，后续项产生冲突 warning。
3. 第二阶段集中处理可重复 Hint：`MEMORY_QUOTA`、`USE_TOJA`、`USE_CASCADES`、`MAX_EXECUTION_TIME`、`RESOURCE_GROUP` 等采用最后一次定义并告警；负内存配额被忽略，零配额保留但提示“无限制”；`NTH_PLAN < 1` 归一为禁用值 `-1`。
4. 合并各类生效 offset 并排序，返回结构、offset 和 warnings。`IgnorePlanCache`、`UsePlanCache`、`WriteSlowLog` 是直接置位项，不进入 offset map。

计划级路径如下：

1. `ParsePlanHints` 先过滤受限 Hint；statement 路径负责的名称不在此重复告警，其余受限名称通过 `hintWarnHandler` 报告。
2. 对必须携带表名的 Hint 做统一前置检查。随后按名称分派：join 表经 `tableNames2HintTableInfo` 进入相应集合；index Hint 构造 parser `IndexHint`；storage Hint 按载荷分到 TiKV/TiFlash；聚合、limit、CTE 和 straight-join 设置位或布尔值。
3. `SEMI_JOIN_REWRITE` 与 `NO_DECORRELATE` 先验证当前子查询上下文，再设置返回位图；上下文不适用时只告警、不生效。
4. `LEADING` 只接受一条，并与外部传入的 straight-join 状态互斥。多条 leading 或二者并存时清空 `LeadingJoinOrder` 并告警。
5. 计划构建过程中，`IfPrefer*`/`Match` 将成功消费的条目标为 `Matched`。构建结束后 `CollectUnmatchedHintWarnings` 根据残留的 false 标志输出 index、join 和 storage 告警。

## 数据与状态

- 常量数据分三组：SQL Hint 名称字符串；子查询 `u64` flags；join/aggregate/store 的 `u32` `Prefer*` 位。位值按 Go 的 `iota` 顺序固定，新增值不能插入既有序列中间。
- `StmtHints` 与 `PlanHints` 是每条语句/每个计划的拥有型值，内部 `Vec`、`String`、`HashMap` 均随结果生命周期释放；本文件不保存跨语句的解析结果。
- `Matched` 是计划构建期间的可变状态：解析时为 false，匹配方法命中后变 true，未匹配告警依赖最终状态。因此同一 `PlanHints` 不应在无重置的情况下作为多个独立计划构建的干净输入复用。
- `offs` 是原 Hint 向量中的零基 offset，排序后便于调用者稳定比较。普通重复 Hint 记录最后一次；重复 `SET_VAR` 记录第一次通过校验且未冲突的项。
- 唯一进程级状态是 `RESTRICTED_HINT_CHECKER`。它没有注销 API；session 的 `InitializeHintRuntime` 用 `Once` 注册安全策略，测试则必须串行修改并恢复该全局检查器。

## 依赖与调用关系

上游主链由 RustCodeGraph 索引确认：

- [`pkg/session/hint_runtime.rs`](../../session/hint_runtime.rs) 的 `parse_statement_hints` 调用 `ParseStmtHints`，提供真实系统变量准入回调；`StartStatementHints` 在文本 SQL/绑定生命周期中使用结果，并以 RAII guard 在语句结束时恢复 `SET_VAR`。同文件的 `InitializeHintRuntime` 通过 `RegisterRestrictedHintChecker` 接入 `sem/v2` 安全策略。
- [`pkg/planner/optimize.rs`](../../planner/optimize.rs) 的 `parseStatementHints` 也调用 `ParseStmtHints`，优化成功后把 statement Hint 效果安装到规划上下文。
- [`pkg/planner/core/logical_plan_builder_runtime.rs`](../../planner/core/logical_plan_builder_runtime.rs) 与 [`pkg/planner/core/planbuilder_runtime.rs`](../../planner/core/planbuilder_runtime.rs) 消费 `ParsePlanHints`/`PlanHints`，使用 `IfPrefer*` 选择 join 或存储策略，并在构建后调用 `CollectUnmatchedHintWarnings`。
- `ParsePlanHints -> tableNames2HintTableInfo -> QBHintHandler::GetHintOffset` 把 AST 的 query-block 名落到整数 offset；`ParsePlanHints -> filterRestrictedHints` 与 `ParseStmtHints -> filterRestrictedHints` 共用安全过滤器。
- `CollectUnmatchedHintWarnings -> collectUnmatchedIndexHintWarning/collectUnmatchedJoinHintWarning/collectUnmatchedStorageHintWarning -> Restore2*` 是诊断链；恢复文本也被 `RemoveDuplicatedHints` 用作等价键。

下游 crate 依赖由 `Cargo.toml` 证明：`parser` 提供 AST 与 Hint 恢复入口，`meta-model`/`types` 支撑假想索引，`dbterror`/`errno`/`plannererrors` 生成兼容 warning 与错误。`plannererrors` 在本文件的导入边界中可用，但核心冲突 warning 直接以 `dbterror::ClassOptimizer + ErrWarnConflictingHint` 构造。

## 错误处理与边界

- Hint 的不适用、重复、冲突和受限策略通常是 warning，而不是阻断优化的错误：分别累积到 `Vec<errors::Error>` 或经 `hintWarnHandler` 写入语句上下文。
- `ParsePlanHints` 保留 `Result` 签名以对齐错误传播边界；当前分派主体大多将问题降级为 warning 并最终返回 `Ok`。调用者仍必须传播未来恢复/查询块处理可能增加的错误。
- `hintDataSetVar/I64/U64/Bool/String/CIStr/TimeRange` 对错误的 AST 载荷类型直接 `panic!`。这是 parser 到 util-hint 的内部类型不变量，不是用户 SQL 的普通错误路径；扩展 parser Hint 时必须同步载荷构造与这里的解包分支。
- `RESTRICTED_HINT_CHECKER` 的读写锁中毒会 `expect` panic。注册应在初始化阶段完成；运行期频繁替换会扩大并发行为的不确定性。
- 无表名的 join/index/leading Hint、带 index 的 `NO_INDEX_LOOKUP_PUSH_DOWN`、不带 index 的 `INDEX_LOOKUP_PUSH_DOWN`、带表名的 `MERGE`、分区限定的部分 join Hint均被拒绝并告警。
- `HintINLMJ` 已弃用；携带表参数时告警并跳过。未知或尚未实现的计划 Hint在默认分支静默忽略，文档不能据常量存在推断其已完整接线。
- `HintedTable::Match` 还要求 `SelectOffset` 相同，避免同名表跨 query block 误命中；告警会建议使用表别名。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或外部 I/O。绝大多数状态由调用者独占的 `&mut PlanHints`、`&mut QBHintHandler` 和 `&mut dyn hintWarnHandler` 传递，Rust 借用保证单次匹配/告警写入互斥。

全局 `RESTRICTED_HINT_CHECKER` 使用标准库 `RwLock`：注册持写锁，解析仅在复制可 `Copy` 的函数指针时持读锁，实际 checker 调用发生在锁外，因此不会把任意策略逻辑放在临界区内。session 侧以 `Once` 保证生产注册一次；测试 [`pkg/planner/core/hint_test.rs`](../../planner/core/hint_test.rs) 使用静态 `Mutex` 串行化注册与恢复，防止测试间污染。

集合与恢复函数通过拥有型参数或 clone 生成诊断文本，没有悬挂引用风险；代价是 `CollectUnmatchedHintWarnings` 会克隆多个 Hint 列表。该路径通常每次规划仅执行一次，若未来列表规模显著增大，可在保持输出顺序与 API 兼容的前提下评估借用切片。

## 与 Go 版本的对应关系

直接对照文件是 [`hint.go`](hint.go)。Rust 保留了 Go 的公开名称、字段组织、位图顺序、重复 Hint 规则、warning 文本、query-block offset、匹配后置 `Matched` 标志以及未匹配告警顺序。`StmtHints::Clone` 明确保留 Go“不复制 `HintedHypoIndexes`”的特殊语义。

主要语言映射是：Go slice/map 对应 `Vec`/`HashMap`，nil 指针对应 `Option`，Go interface 载荷断言对应 `ast::HintData` 枚举匹配，Go 包级 checker 变量加锁对应 `RwLock<Option<fn>>`，Go error 返回对应 `errors::Error`/`Result`。Rust 的载荷枚举比 Go 的运行时 type assertion 更显式，但类型错配仍选择 panic，以保持内部不变量失败的性质。

需要留意两类表示差异。其一，Go `nil` slice 与空 slice 可在个别分支表达不同意图，Rust 统一为 `Vec::is_empty`；例如弃用 `HintINLMJ` 和 `MERGE` 的判断以空集合近似 Go 的 nil 判断，新增 parser 表示时需做兼容回归。其二，Rust `ParsePlanHints` 直接原地构造 `PlanHints`，Go 先收集局部变量再统一构造返回值；最终字段与行为应保持一致，而不要求控制流逐行相同。

Go 同目录没有独立 `*_test.go`；Rust 的最近单测 [`hint_2_aster_unit_test.rs`](hint_2_aster_unit_test.rs) 专门覆盖本文件。更高层 Go/Rust 对照由 planner casetest 和 Go 原测试/黄金数据提供。

## 扩展指南

新增 statement-level Hint 时，应同步完成：parser 的 `HintData` 类型与构造；名称常量；`ParseStmtHints` 第一遍分类和需要的重复/边界处理；`isStmtHint`（若由 hint processor 分流）；`shouldWarnRestrictedHintInParseStmtHints` 的 warning 归属；`StmtHints` 字段/克隆语义；独立 Rust 测试及 Go 对照测试。若返回 offset，明确重复项究竟采用首个还是最后一个。

新增 plan-level Hint 时，应在 `PlanHints` 建模，在 `ParsePlanHints` 的“必须有表名”预检和分派中接线；表级 Hint通常走 `tableNames2HintTableInfo` 以继承默认库、query-block offset 与分区校验。若计划构建需要确认 Hint 是否真正生效，应使用 `Matched` 并把新集合加入 `CollectUnmatchedHintWarnings`，同时增加对应 `Restore2*` 文本与顺序测试。

修改稳定字符串或位图前必须同时检查 Go `hint.go`、parser、binding 恢复文本及 planner 消费者；这些变化有 SQL 兼容、绑定命中和 warning 黄金文件风险。不要把测试嵌入 `hint.rs`：本仓库要求继续放在独立的 [`hint_2_aster_unit_test.rs`](hint_2_aster_unit_test.rs) 或相应 planner 测试文件中。

推荐最小回归面：

- 语句解析、匹配、恢复、未命中顺序：`pkg/util/hint/hint_2_aster_unit_test.rs`。
- `SET_VAR`、`write_slow_log` 与受限 Hint：`pkg/planner/core/hint_test.rs`。
- 真实 SQL 到 `StmtHints`/`PlanHints`：`pkg/planner/core/casetest/hint/hint_test.rs`。
- 子查询 flags：`pkg/planner/core/casetest/correlated/correlated_test.rs`；JOIN/QB 名和 build/probe：`pkg/planner/core/casetest/join/join_test.rs`。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/hint` 列出本 crate 的 9 个 Go/Rust 源文件，`hint.rs` 含 60 个符号。
- RustCodeGraph `query`/`node`：确认 `ParseStmtHints` 位于 `hint.rs:373`、`PlanHints` 位于 `hint.rs:752`、`ParsePlanHints` 位于 `hint.rs:1008`、`CollectUnmatchedHintWarnings` 位于 `hint.rs:1546`，并逐段读取了完整 1,689 行目标源码。
- RustCodeGraph `explore`：确认 `ParseStmtHints` 的 Rust 上游为 planner `parseStatementHints` 与 session `parse_statement_hints`；`ParsePlanHints` 被 planner 的 JOIN casetest及运行时构建链使用；`CollectUnmatchedHintWarnings` 由 logical-plan builder 运行时消费。精确 `callers` 子命令两次运行超过 90 秒无返回后中止，未据此补充未经 `explore`/源码确认的边。
- 已读边界与装配文件：`pkg/util/hint/Cargo.toml`、`pkg/util/hint/lib.rs`、`pkg/util/hint/hint_query_block.rs`/`hint_processor.rs` 的索引关系，以及 `pkg/session/hint_runtime.rs`、`pkg/planner/optimize.rs`、planner core 运行时入口。
- 已读 Go 对照：`pkg/util/hint/hint.go` 的 `StmtHints`、`ParseStmtHints`、`PlanHints`、`ParsePlanHints`、恢复与未匹配告警实现。
- 已读测试：`pkg/util/hint/hint_2_aster_unit_test.rs`、`pkg/planner/core/hint_test.rs`、`pkg/planner/core/casetest/hint/hint_test.rs`、`pkg/planner/core/casetest/correlated/correlated_test.rs`、`pkg/planner/core/casetest/join/join_test.rs`；`rg` 还确认同目录不存在 Go 测试文件。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章结构命令，并人工检查唯一生产物、源码链接、事实边界及无测试内嵌建议。
