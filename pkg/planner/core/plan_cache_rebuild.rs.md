# `pkg/planner/core/plan_cache_rebuild.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 将该 crate 定义为库 `astersql-planner-core`（入口为 `lib.rs`，并关闭自动测试发现），`pkg/planner/core/lib.rs:168` 以私有模块 `plan_cache_rebuild` 装入本文件，再由 `lib.rs:231` 将其中公开项重新导出。

它位于计划缓存命中后的“按当前参数重新检查访问范围”边界，但当前 Rust 版本只操作 `common_plans.rs` 中的轻量 `PlanNode`/`PlanKind`，还没有接入 Rust 侧真实缓存命中主链。仓库内 Rust 引用搜索只找到模块装配和 `plan_cache_rebuild_test.rs` 的直接单元测试；实际应用调用仍是 Go 的 `plan_cache.go:adjustCachedPlan` 调用 Go `RebuildPlan4CachedPlan`。因此，本文件应视为已公开、可单测的迁移实现，而不能描述成已替代 Go 生产路径。

## 核心职责

本文件承担三项聚焦职责：

1. `RebuildPlan4CachedPlan` 把递归检查的 `Result` 压缩成计划缓存调用方需要的成功/失败布尔值。
2. `rebuildRange` 深度优先遍历轻量计划树，额外进入 `IndexMergeReader.partial_plans`，并对 `IndexScan` 范围应用安全门禁。
3. `isSafeRange` 保存从 Go 同名函数移植的四类不安全判定：出现残留条件、访问条件数不一致、结果范围为空，以及有访问条件时从非全范围退化为全范围。

需要特别注意当前实现边界：`rebuildRange` 没有根据新参数重新计算范围。它把 `IndexScan.ranges` 克隆进 `RangeRebuildResult`，同时把访问条件和残留条件都设为空，再调用 `isSafeRange`。所以当前实际可触发的扫描失败主要是空 `ranges`；“条件丢失”和“非 full 到 full”规则由公开的 `isSafeRange` 单独实现、由测试直接覆盖，但尚未通过真实 ranger 结果接入递归入口。

## 主要符号

- `pub fn RebuildPlan4CachedPlan(plan: &mut PlanNode) -> bool`：公开兼容入口，调用 `rebuildRange(plan)`，仅以 `is_ok()` 暴露结果。它不返回失败原因，也不记录 warning。
- `pub fn rebuildRange(plan: &mut PlanNode) -> Result<(), String>`：公开递归实现。先处理 `plan.children`，随后按当前节点的 `PlanKind` 分派；任一递归或安全检查失败都会通过 `?`/`Err` 立即终止。
- `pub struct RangeRebuildResult`：轻量 ranger 拆分结果，包含 `ranges`、`access_conditions`、`remained_conditions` 三个 `Vec<String>`。派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`，便于构造和断言；本文件没有为它维护外部资源或缓存。
- `fn has_full_range(ranges: &[String], unsigned_int_handle: bool) -> bool`：私有辅助函数。只有恰好一个元素且忽略 ASCII 大小写等于 `"full"` 或 `"full unsigned"` 才视为全范围。
- `pub fn isSafeRange(...) -> bool`：公开安全判定。参数分别是原访问条件、重建结果、是否为无符号整数 handle，以及可选原范围；该函数只读输入，不修改计划。

文件中没有常量、trait、`impl`、条件编译项或异步函数。公开函数沿用 Go 风格名称而非 Rust snake_case，这是当前兼容 API 的既有形态。

## 执行流程

`RebuildPlan4CachedPlan` 的当前流程如下：

1. 接收可变 `PlanNode`，调用 `rebuildRange`。
2. `rebuildRange` 先依次递归所有普通 `children`，因此采用后序式检查：子节点失败时父节点不再处理。
3. 若当前节点是 `PlanKind::IndexScan`，复制其 `ranges`，构造空访问条件/空残留条件的 `RangeRebuildResult`，调用 `isSafeRange(&[], ..., false, Some(ranges))`。不安全时返回固定错误 `"rebuild to get an unsafe range"`。
4. 若当前节点是 `PlanKind::IndexMergeReader`，再依次递归 `partial_plans`。`table_plan` 不在此分支遍历；这与 Go 实现对 IndexMerge table plan“不含需要重建的 range”的注释方向一致，但 Rust 的轻量模型没有在此验证该不变量。
5. 其他 `PlanKind` 除普通 `children` 外没有专门处理，最终返回 `Ok(())`。
6. 顶层把 `Ok` 映射为 `true`，把任何 `Err` 映射为 `false`。

`isSafeRange` 先执行三个直接拒绝条件：`remained_conditions` 非空、重建使用的访问条件数量与原输入不同、或 `ranges` 为空。通过后，再检查退化条件：只有原访问条件非空、重建结果是全范围、调用方提供了原范围、且原范围不是全范围时才拒绝。除此之外返回 `true`。

## 数据与状态

输入计划使用 `common_plans.rs:PlanNode`：节点含 `kind` 与 `children`，`PlanKind::IndexScan` 自带 `Vec<String>` 范围，`PlanKind::IndexMergeReader` 另有 `partial_plans: Vec<PlanNode>` 和 `table_plan: Box<PlanNode>`。本文件只观察这些字段；尽管入口接收 `&mut PlanNode`，当前代码没有把任何新范围写回计划。

`RangeRebuildResult` 是调用期间的临时值。`IndexScan` 分支会克隆现有范围，因此成本与字符串范围总大小线性相关；安全判定本身除切片长度和单个字符串比较外不分配内存。`original_ranges: Option<&[String]>` 用 `None` 表示调用方没有可比较的旧范围，此时“非 full 到 full”保护不会触发。

全范围表示是协议性字符串：有符号路径用 `"full"`，无符号 handle 路径用 `"full unsigned"`，比较忽略 ASCII 大小写，但不接受多元素、额外空白或其他 ranger 表示。该约定是轻量 Rust 模型的局部表示，不等同于 Go `ranger.Ranges` 的结构化语义。

## 依赖与调用关系

直接 Rust 依赖只有 crate 根重新导出的 `PlanNode` 和 `PlanKind`（定义在 `pkg/planner/core/common_plans.rs`）；本文件没有直接使用 `Cargo.toml` 中的外部 crate。模块关系是 `lib.rs -> mod plan_cache_rebuild -> pub use plan_cache_rebuild::*`。

RustCodeGraph 对精确符号的查询确认：

- `RebuildPlan4CachedPlan` 位于本文件第 11 行，并调用本文件的 `rebuildRange`。
- `rebuildRange` 与 `isSafeRange` 同时存在 Go/Rust 两个定义；Rust 的 `rebuildRange` 调用自身完成递归，并调用 `isSafeRange`。
- `PlanNode`/`PlanKind` 在仓库内有多个同名类型；本文件通过 crate 根导入，实际对应 `common_plans.rs:222` 和 `common_plans.rs:55`，而不是 `task.rs` 或 `joinorder/util.rs` 的同名类型。

Rust 上游目前仅有 `pkg/planner/core/plan_cache_rebuild_test.rs` 直接调用入口和安全函数。Go 生产主链为 `plan_cache.go:adjustCachedPlan -> plan_cache_rebuild.go:RebuildPlan4CachedPlan -> rebuildRange -> 各类 range builder/isSafeRange`；Rust 尚无对应 `adjustCachedPlan` 调用边。

## 错误处理与边界

`rebuildRange` 只有一种自建错误文本：`"rebuild to get an unsafe range"`。递归使用 `?` 原样传播第一个错误；顶层 `RebuildPlan4CachedPlan` 丢弃错误文本，仅返回 `false`。与 Go 入口不同，Rust 入口不检查 `StmtCtx.UseCache()`，不在失败时追加 “skip plan-cache” warning，也不在重建后复查缓存开关。

安全边界包括：

- 空重建范围必定不安全，即使没有访问条件。
- 任意残留条件都不安全；访问条件数量减少或增加也不安全，但当前只比较数量，不比较条件内容。
- 原本非全范围、重建后成为全范围时，只有存在访问条件且提供了原范围才拒绝。
- `unsigned_int_handle` 仅改变识别全范围时使用的字符串；它不解析数值或 handle。
- `IndexMergeReader.table_plan` 不被专门递归；其他未列举的计划种类只遍历普通 `children`。
- 普通 children 在当前节点前处理，而 IndexMerge partial plans 在普通 children 之后处理；发生错误时后续分支不会执行。

这些边界意味着该文件目前不能保证 Go 实现覆盖的表扫描、点查、批量点查、IndexJoin、DML SelectPlan、类型转换、分区 fix-control 或 grouped ranges 行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有临时 `RangeRebuildResult` 都受单次栈帧所有权管理，函数返回后释放；递归期间对计划使用独占 `&mut` 借用，因此同一棵计划树不能由安全 Rust 代码同时进入本函数进行并发修改。

虽然接口可变，当前实现只克隆和读取 range，没有原地更新。若未来接入真实重建并写回范围，应保持“先完整构造并验证，再替换旧范围”的原子性边界，避免递归中途失败后留下部分节点已更新、部分节点未更新的计划。还需评估深计划树递归带来的栈深风险，以及实例级计划缓存是否在调用前完成深克隆；Go 的 `clonePlanForInstancePlanCache` 明确通过克隆隔离并发使用，但该隔离尚不是本 Rust 文件能够自行保证的。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/plan_cache_rebuild.go`。名称和总体意图对应，但当前实现覆盖面差异显著：

- Go `RebuildPlan4CachedPlan` 在前后检查 `StmtCtx.UseCache()`，失败时追加 warning；Rust 只把 `Result` 转成 `bool`。
- Go `rebuildRange` 针对 IndexJoin、Table/Index Scan、Reader、PointGet、BatchPointGet、IndexMerge 和带 SelectPlan 的 DML 分派，并调用 ranger/类型转换逻辑真正重建和写回范围；Rust 只识别轻量 `IndexScan` 与 `IndexMergeReader`，且不改变范围。
- Go 对 IndexMerge 只递归每组 partial plan 的首节点，并明确跳过 table plans；Rust 递归全部 `partial_plans` 元素，同时也不处理 `table_plan`。两侧数据模型并非一一同构。
- Go `isSafeRange` 检查 `ranger.DetachRangeResult` 与结构化 `ranger.Ranges`；Rust 保留相同的四类判定，但以字符串数组近似范围，并以元素计数近似访问条件完整性。
- Go 重建刻意不施加 range 内存上限，避免参数变长导致 fallback 后读取额外行；`integration_test.go:TestPlanCacheForIndexRangeFallback` 和 `TestPlanCacheForIndexJoinRangeFallback` 验证这一点。Rust 文件没有 ranger 或内存限制参数，当前无法表达或验证该语义。

因此扩展时应以 Go 文件的实际分支和测试意图为迁移基准，不应把现有 Rust 代码当作完整等价实现。

## 扩展指南

若要把本模块接入真实 Rust 计划缓存路径，最可能需要修改 `RebuildPlan4CachedPlan` 和 `rebuildRange`，并先明确计划上下文、参数值、warning/缓存开关的承载类型。不要只为测试增加字符串分支；应逐项对照 Go `rebuildRange` 的计划类型分派，复用 Rust 已有 ranger、扫描算子和点查模型，并在验证通过后写回范围。

扩展 `isSafeRange` 时，应保持四个既有安全不变量，并考虑把“条件数相等”升级为能证明条件语义未丢失的结构化结果。新增全范围表示时应同步 `has_full_range`，同时避免将展示字符串作为长期协议。新增 `PlanKind` 专门处理前，确认该算子是在普通 `children`、专用字段还是两者中持有子计划，防止漏遍历或重复遍历。

测试应继续放在独立文件，不能内嵌到生产源码：

- 直接函数边界扩展 `pkg/planner/core/plan_cache_rebuild_test.rs`，覆盖空范围、残留条件、访问条件数不匹配、有符号/无符号 full、原范围缺失，以及普通 children/IndexMerge partial plan 的失败传播。
- 真实缓存执行语义应扩展独立 casetest 或对应集成测试；现有 `pkg/planner/core/casetest/plancache/plan_cache_rebuild_test.rs` 当前测试的是 `FastClonePointGetForPlanCache` 深克隆，并不覆盖本文件的 range 重建。
- Go 行为对齐时同步核对 `pkg/planner/core/integration_test.go` 中两个 range fallback 测试和 `pkg/planner/core/plan_cache_rebuild.go` 的全部计划类型分支。

主要风险是错误接受退化范围造成结果错误（正确性），缓存开关/warning/分区与类型转换差异造成 Go/Rust 行为不兼容（兼容性），以及对每次命中重新构造大范围、克隆字符串或重复遍历造成延迟和内存增加（性能）。

## 验证依据

本说明基于以下直接证据：

- 生产源码：`pkg/planner/core/plan_cache_rebuild.rs` 全文；类型来源 `pkg/planner/core/common_plans.rs`；模块装配与测试挂载 `pkg/planner/core/lib.rs`。
- crate 边界：`pkg/planner/core/Cargo.toml`，包括库入口、`autotests = false`、feature 与 `go-package = "pkg/planner/core"` 移植元数据。
- Rust 独立测试：`pkg/planner/core/plan_cache_rebuild_test.rs`，验证空 range 失败、Limit 不参与范围算术、IndexMerge partial plan 递归，以及 `isSafeRange` 的残留条件/条件丢失/full 退化门禁。另读 `pkg/planner/core/casetest/plancache/plan_cache_rebuild_test.rs` 并确认它覆盖的是点查计划克隆，不是本文件入口。
- Go 对照与调用链：`pkg/planner/core/plan_cache_rebuild.go`、`pkg/planner/core/plan_cache.go:adjustCachedPlan`；相关 Go 测试为 `pkg/planner/core/integration_test.go:TestPlanCacheForIndexRangeFallback` 与 `TestPlanCacheForIndexJoinRangeFallback`。
- RustCodeGraph：`status` 显示索引可用；`query RebuildPlan4CachedPlan/rebuildRange/isSafeRange` 定位 Go/Rust 同名定义；`node RebuildPlan4CachedPlan` 确认 Rust 入口到 `rebuildRange` 的调用边；`query/node PlanNode/PlanKind` 消除了多个同名类型的歧义。`files --filter pkg/planner/core/plan_cache_rebuild` 未返回路径，且部分 callers/callees 查询没有给出额外边，因此用 `rg` 的全仓 Rust 引用结果补充确认当前无生产调用者。
- 人工复核结论：文件存在是为了承载缓存计划范围重建的 Rust 迁移边界；当前运行方式是对轻量树递归做安全检查；安全扩展必须接入真实参数/ranger/计划类型并保持独立测试与 Go 安全不变量，而不能宣称当前已具备完整 Go 行为。

按任务约束，本次是纯文档分析，未运行 Cargo 或代码测试；最终结构验证单独执行并记录退出状态。
