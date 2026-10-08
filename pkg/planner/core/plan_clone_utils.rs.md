# `pkg/planner/core/plan_clone_utils.rs`

## 文件定位

本文对应真实源文件 [`plan_clone_utils.rs`](./plan_clone_utils.rs)。它属于 `astersql-planner-core` crate，集中提供三类计划复制辅助能力：访问路径的“清空分析状态后重建”、Point Get 缓存计划的快速深克隆，以及受支持逻辑算子子树的带新 ID 克隆。`pkg/planner/core/lib.rs` 以私有模块 `mod plan_clone_utils` 装配该文件，再通过 `pub use plan_clone_utils::*` 将公开类型和函数提升为 crate API；独立测试由 `#[cfg(test)] mod plan_clone_utils_test` 显式挂载。

本文件不是计划缓存容器、优化规则或计划构建入口。RustCodeGraph 与全仓精确引用搜索显示，当前 Rust 生产代码尚未调用 `freshAccessPath`、`FastClonePointGetForPlanCache` 或 `cloneLogicalSubtree`；直接调用来自同目录单元测试和计划缓存 casetest。因此这里应视为已实现、已导出、已有验证但尚未完整接入 Rust 生产主链的迁移边界。Go 对照实现则分别接入 `rule_correlate.go` 和 `plan_cache.go`。

文件包含两个公开结构体、三个公开函数和五个私有辅助函数；没有模块级常量、trait、`impl` 或条件编译项。`pkg/planner/core/Cargo.toml` 声明 crate 入口为 `lib.rs`、`autotests = false`，所以测试必须由模块入口显式注册。

## 核心职责

1. `freshAccessPath` 只保留访问路径的结构身份 `index_name`，将范围、行数估计和部分备选路径恢复为默认值，供后续统计/路径推导重新计算。
2. `FastClonePointGetForPlanCache` 把缓存中的 `PointGetPlan` 深克隆到调用者提供的目标对象，并把规划上下文替换为本次执行的 `PlannerContext`，避免不同会话共享可变计划状态。
3. `cloneLogicalSubtree` 克隆由八种受支持逻辑算子组成的整棵子树；先收集原树全部 ID，再为克隆节点分配不与原树或已分配克隆节点冲突的新 ID。任一节点类型不受支持时，整个操作返回失败。

三项职责都只操作内存值，不负责重新派生统计、重建 Point Get 参数、执行谓词下推或把克隆结果装入优化器。调用者必须根据返回状态决定是否继续优化，并在适当阶段完成被清空或需重建的运行时状态。

## 主要符号

- `pub struct AccessPath`：轻量访问路径快照。`index_name` 表示结构身份；`ranges`、`count_after_access`、`count_after_index` 是分析结果；`partial_alternative_index_paths` 保存嵌套备选路径。派生的 `Clone` 会递归复制嵌套向量。
- `pub fn freshAccessPath(source: &AccessPath) -> AccessPath`：构造只含 `source.index_name.clone()` 的新值，其余字段来自 `AccessPath::default()`。它不会修改源对象，也不会保留已有范围、估算或 Index Merge 备选。
- `pub struct PointGetPlan`：Point Get 缓存快照，包含 `ctx`、可选 `plan`、可选 `access_path`、`partition_names`、`handles` 和二维 `index_values`。这些字段均实现 `Clone`，其中向量、字符串、计划树和嵌套访问路径按 Rust 值语义递归复制。
- `pub fn FastClonePointGetForPlanCache(new_ctx, source, destination) -> PointGetPlan`：先以 `source.clone()` 覆盖 `destination`，再换绑 `ctx`，显式重新克隆访问路径及三个向量字段，最后额外返回 `destination.clone()`。当前签名要求目标对象始终存在，不像 Go 版本可接收 `nil` 目标。
- `fn supported_logical_kind(kind: &PlanKind) -> bool`：白名单包含 `DataSource`、`Join`、`Selection`、`Projection`、`Aggregation`、`Limit`、`Sort`、`TopN`。`Apply`、物理扫描/Reader、DML、CTE、Window 等其余变体均不支持。
- `pub fn cloneLogicalSubtree(plan: &PlanNode) -> (Option<PlanNode>, bool)`：公开入口。它通过 `collect_plan_ids` 建立原树 ID 集合，以最大 ID 加一作为首选候选；若最大值为 `i32::MAX`，候选回绕到 `i32::MIN`。
- `clone_logical_subtree_with_ids`：递归克隆单节点。先验证类型，再克隆全部子节点，然后复制节点的所有其他字段、覆盖 ID 和 children；因此 `estimated_rows`、代价、运行统计、字符串元数据等也从源节点复制。
- `cloneWithChildren`：按原顺序递归处理子节点。任一子节点失败即丢弃已经构造的局部向量并返回 `(Vec::new(), false)`。
- `collect_plan_ids` 与 `allocate_fresh_id`：前者深度优先收集原树 ID；后者在 `HashSet<i32>` 中跳过冲突 ID，并用 `wrapping_add` 推进候选值。

## 执行流程

访问路径刷新是单步构造：读取源路径的 `index_name`，用默认值补齐其余字段并返回新对象。源路径保持不变；返回对象需要由上游重新填充范围和统计。

Point Get 快速克隆按以下顺序执行：首先完整克隆源快照并覆盖 `destination`；然后用 `new_ctx` 替换源上下文；接着重新赋值 `access_path`、`partition_names`、`handles`、`index_values`；最后再次克隆目标并作为返回值交给调用者。因此结束时 `destination` 与返回值内容相等，但它们是两个独立拥有的值；两者也都不共享源对象中的 `Vec`、`String`、`PlanNode` 或 `AccessPath` 可变存储。

逻辑子树克隆分两阶段。入口先遍历完整原树收集 ID，并计算候选起点；递归阶段对每个节点先做算子白名单判断，再按原顺序克隆全部子树，之后以 `plan.clone()` 复制节点非 children 状态，分配新 ID 并替换 children。ID 是后序分配的，所以叶子通常先取得较小候选值，根节点最后取得 ID；API 只保证新 ID 唯一且避开原树，不保证克隆根保留原 ID 或按先序连续编号。

失败是整棵子树级别的：根节点不支持时立即失败；深层子节点不支持时，失败逐层向上传播，顶层返回 `(None, false)`。虽然递归期间局部 `used_ids` 和 `next_id` 已推进，但它们只存在于本次调用栈中，失败后不会泄露到外部状态。

## 数据与状态

`AccessPath` 将结构身份和分析结果放在同一值中。`freshAccessPath` 的不变量是只保留 `index_name`；`ranges` 与 `partial_alternative_index_paths` 为空，两个计数字段为零。同目录测试 `fresh_access_path_preserves_identity_and_clears_analysis_state` 明确覆盖这一边界。

`PointGetPlan` 的 `ctx` 表示本次规划/执行所属会话状态；`plan` 和 `access_path` 是可选的嵌套值；分区名、句柄和索引值是执行重建所需集合。函数会保留源快照的所有业务字段但替换上下文。casetest `fast_point_get_clone_does_not_share_mutable_state_with_source` 逐一修改目标的分区名、句柄、索引值、嵌套访问路径及计划扫描范围，确认源值保持不变。

`PlanNode` 定义在 `common_plans.rs`，除 `id`、`kind`、`children` 外还含存储类型、估算行数/代价、运行统计和资源信息。克隆逻辑只重建 `id` 与 `children`，其余字段沿用 `Clone` 结果。`used_ids` 同时记录原树 ID 和新分配 ID，维持“克隆 ID 不与原树冲突且克隆内部互异”的不变量。若源树自身包含重复 ID，集合会折叠重复值，但新 ID 仍不会命中该值；本文件不验证或修复源树 ID 唯一性。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashSet`。本文件从 crate 根使用 `PlanKind`、`PlanNode`、`PlannerContext`，三者实际定义在 `common_plans.rs` 并由 `lib.rs` 再导出。`Cargo.toml` 未为本文件引入专用外部 crate；它处于 `astersql-planner-core` 的普通默认功能中，不受 `nextgen` feature 条件控制。

RustCodeGraph 验证的内部调用链为 `cloneLogicalSubtree → collect_plan_ids / clone_logical_subtree_with_ids → supported_logical_kind / cloneWithChildren / allocate_fresh_id`，其中 `cloneWithChildren` 再递归回 `clone_logical_subtree_with_ids`。同目录 `plan_clone_utils_test.rs` 调用 `freshAccessPath` 和 `cloneLogicalSubtree`；`casetest/plancache/plan_cache_rebuild_test.rs` 通过 crate 公开 API 调用 `FastClonePointGetForPlanCache`。

全仓精确符号搜索未发现三个公开函数的 Rust 生产调用点。Go 生产调用边为：`plan_cache.go` 调用 `FastClonePointGetForPlanCache`，`rule_correlate.go` 调用 `cloneLogicalSubtree` 以构造 Apply 备选的独立 inner plan，并调用 `freshAccessPath` 清空待重新推导的路径状态。这些 Go 边说明移植目标，不代表 Rust 已经接入同一主链。

## 错误处理与边界

本文件没有 `Result` 或结构化错误。`cloneLogicalSubtree` 以 `(Option<PlanNode>, bool)` 表示成功或失败：成功应为 `(Some(plan), true)`，不支持算子应为 `(None, false)`。`cloneWithChildren` 中的 `expect("successful clone returns a plan")` 依赖内部契约“`ok == true` 必须伴随 `Some`”；当前递归函数满足该契约，但未来若修改返回组合而未同步这里会触发 panic。

逻辑白名单是保守边界。即使 `PlanKind` 可被普通 `Clone` 复制，只要它不在八种列表中，函数也拒绝整个子树；调用方不能使用部分克隆继续优化。同目录测试以 `Projection → Apply` 验证深层不支持节点会导致整体失败。

ID 分配使用 `checked_add(1).unwrap_or(i32::MIN)` 和后续 `wrapping_add`，可跨越有符号整数边界，并通过集合跳过已占用值。理论上如果 `i32` 全部取值都已被占用，循环不会终止；现实计划树无法合理持有如此多节点，但代码没有显式容量错误。深度优先递归也没有深度限制，异常深的树可能耗尽调用栈。

`FastClonePointGetForPlanCache` 不会失败，也不校验目标是否与源对象语义上对应。它要求不同借用，Rust 通常阻止把同一个值同时作为不可变源和可变目标。函数当前先完整克隆、后重复克隆若干字段并克隆返回值，语义安全但存在额外分配；扩展时不能仅为降低分配而退化为共享可变状态。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。全部状态都由调用栈和拥有所有权的 Rust 值构成。`freshAccessPath` 与 `cloneLogicalSubtree` 只借用输入；`FastClonePointGetForPlanCache` 对源使用共享借用、对目标使用独占可变借用，并取得新上下文的所有权，借用规则保证一次调用期间目标不会被并发修改。

`HashSet`、克隆树及临时向量在函数返回或失败展开时自动释放。逻辑克隆不共享 children 向量或字符串缓冲区，但这里没有 `Arc`、锁保护对象或外部资源句柄需要额外生命周期协议。Point Get 的 `PlannerContext` 也是值类型，不是对真实 session 的引用；换绑只替换快照，不进行事务开始、提交或回滚。

资源风险主要来自分配规模：逻辑克隆的时间与额外内存大致随节点数和节点内可克隆字段总量线性增长；Point Get 函数当前至少执行一次完整源克隆和一次完整目标返回克隆，并对若干字段重复赋值克隆。若用于高频缓存命中路径，优化分配前必须用现有深拷贝测试守住隔离语义。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/plan_clone_utils.go`。Rust 保留了三个同名能力及八种逻辑算子白名单，但数据模型与接线尚不完全等价。

- Go `FastClonePointGetForPlanCache` 操作完整的 `physicalop.PointGetPlan`：目标可为 `nil`；有选择地共享不可变元数据；按 `SafeToShareAcrossSession` 决定常量共享或克隆；复用目标切片容量；清空 `PartitionIdx`、`Handle` 等重建字段；刻意不复制 cost 等缓存。Rust 的轻量 `PointGetPlan` 没有这些字段与策略，而是深克隆全部现有字段、换绑上下文并返回第二份克隆。它验证了会话隔离，但不能据此宣称覆盖 Go 的全部性能和重建语义。
- Go `cloneLogicalSubtree` 按具体逻辑算子调用 `cloneDataSource`、`cloneJoin` 等函数，为每类节点重建基础计划、schema 和易变条件切片，并共享被认定为不可变的元数据；新 ID 来自 `NewBaseLogicalPlan` 的规划上下文。Rust 使用统一 `PlanNode` 值模型整体克隆，再替换 children 和 ID；它没有 Go 中 `PreferCorrelate = false`、DataSource access path 深克隆、schema 克隆和 stats 保留等类型专属处理。
- Go `freshAccessPath` 保留 Index、StoreType、handle/hint 标志等多项结构字段，并清空范围、条件和 Index Merge 分析字段。Rust `AccessPath` 只有 `index_name` 可作为结构身份，因此当前仅保留该字段。

Go 注释说明逻辑克隆用于相关子查询优化：谓词下推修改 Apply 备选 inner plan 时不能污染 Join 原始 inner 子树。Rust 的深拷贝和整体失败策略与此安全意图一致，但 Rust 生产规则尚无调用边；将来接线时需逐项补齐 Go 的算子专属状态，而不能只依赖统一 `Clone` 即认定语义完成。

## 扩展指南

- 新增可克隆逻辑算子时，至少同步 `supported_logical_kind`，并判断统一 `PlanNode::clone` 是否足以隔离该算子的易变字段。若未来数据模型包含共享引用，应为算子增加显式深拷贝逻辑，并在独立 `plan_clone_utils_test.rs` 增加成功、嵌套失败和源/目标隔离测试。
- 修改 ID 策略时应保持三项约束：不命中原树 ID、克隆内部互异、溢出时仍有定义。测试应覆盖 `i32::MAX`、负 ID、源树重复 ID 和多分支树；不要把测试内嵌进生产源文件。
- 扩充 `AccessPath` 字段时，先按 Go 的“结构身份”与“可重新推导分析状态”分类，再决定 `freshAccessPath` 保留或清空；同步 `fresh_access_path_preserves_identity_and_clears_analysis_state`，特别覆盖嵌套 Index Merge 字段。
- 扩充 `PointGetPlan` 字段时，必须同步 `FastClonePointGetForPlanCache` 与 `casetest/plancache/plan_cache_rebuild_test.rs`。需明确字段应换绑、深克隆、共享、清空还是留给 rebuild 填充，并评估重复全量克隆的性能成本。
- 将这些函数接入 Rust 生产链时，应分别选择计划缓存命中/重建路径和 correlate 规则入口，并新增接近 Go 场景的独立回归测试。特别要验证两个优化备选互不污染、缓存跨会话隔离，以及不支持算子会安全放弃优化而不是修改原计划。

## 验证依据

- 源文件：`pkg/planner/core/plan_clone_utils.rs`，核对两个公开结构体、三个公开函数、五个私有辅助函数及全部分支。
- crate 边界：`pkg/planner/core/Cargo.toml` 与 `pkg/planner/core/lib.rs`，核对 crate 名、`autotests = false`、模块装配、公开再导出和独立测试注册。
- 直接类型定义：`pkg/planner/core/common_plans.rs` 中 `PlanKind`、`PlanNode`、`PlannerContext`，核对算子白名单以外的变体、节点被整体克隆的字段和上下文值语义。
- Rust 测试：`pkg/planner/core/plan_clone_utils_test.rs` 覆盖访问路径清空、新 ID 唯一性和不支持子节点整体失败；`pkg/planner/core/casetest/plancache/plan_cache_rebuild_test.rs` 覆盖 Point Get 换绑上下文、字段保留和深拷贝隔离。
- Go 对照与生产调用者：`pkg/planner/core/plan_clone_utils.go`、`pkg/planner/core/plan_cache.go`、`pkg/planner/core/rule_correlate.go`，用于核对移植意图、类型专属克隆策略和真实 Go 接线。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query` 精确区分本文件与 Go 文件的同名函数；文件节点与 `explore` 验证 Rust 递归调用链，并定位同目录测试调用。精确批量 callers/callees 查询长时间无结果后已终止，再以全仓符号引用搜索补齐调用证据；未发现 Rust 生产调用点。
- 验收范围：本任务只新增说明文档，不修改 Rust、Go、Cargo 或总计划，不运行 Cargo。结构验收使用任务指定命令，要求恰好存在十一个固定二级标题；另人工复核本文明确区分当前 Rust 事实、Go 对照与尚未接线部分。
