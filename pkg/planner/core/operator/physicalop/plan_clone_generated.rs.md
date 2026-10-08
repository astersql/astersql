# `pkg/planner/core/operator/physicalop/plan_clone_generated.rs`

## 文件定位

本文件说明的真实源码是 [`plan_clone_generated.rs`](plan_clone_generated.rs)。它属于 `astersql-planner-core-operator-physicalop` crate；模块入口在 `pkg/planner/core/operator/physicalop/lib.rs` 的 `pub mod plan_clone_generated`，crate 边界和依赖由同目录 `Cargo.toml` 定义。它处理“把物理计划复制为可跨执行复用的缓存快照”这一窄职责，但必须区分文件中的两层内容：第 16--592 行是注释形式保留的 Go 生成文件参考，第 594--855 行才是会参与 Rust 编译的接线代码。

该 Rust 接线目前更接近迁移中的兼容层，而不是完整替代 Go 生成实现。精确搜索 `plan_clone_generated::CloneForPlanCache`、`CachePlan` 及其变体后，仓库内只有 `plan_clone_generated_test.rs` 显式导入此 trait，未找到生产代码构造或消费 `CachePlan`。生产计划体系另有 `base::Plan::clone_for_plan_cache(new_ctx)` 动态分发接口；两者同名但签名和调用体系不同，不能视为同一调用链。

## 核心职责

可执行部分承担三件事：

1. 用公开 trait `CloneForPlanCache` 把“可缓存则返回完整副本，不可缓存则返回 `None`”统一为静态类型接口。
2. 为通用节点、三种 join、三种 DML、两个 legacy index join 和 local index lookup 提供有针对性的递归克隆或拒绝策略。
3. 用公开枚举 `CachePlan` 封闭列举 25 类与 Go 生成清单对应的计划变体，并把枚举分发转交给载荷类型的 `clone_for_plan_cache`。

它不负责计划缓存的查找、淘汰、序列化或实际执行，也不接收新的 session context。Go 方法通过 `newCtx` 重绑计划上下文，而本 trait 的签名只有 `&self`；部分 join 从原对象的 `SCtx()` 取上下文，通用 `PhysicalPlanNode` 则只是递归 `Clone`。这是当前实现能力的真实边界。

## 主要符号

- `pub trait CloneForPlanCache: Sized`：唯一方法 `fn clone_for_plan_cache(&self) -> Option<Self>`。`Sized` 和返回 `Self` 意味着它不是用于 `dyn CloneForPlanCache` 的对象安全接口，而是由静态已知类型或 `CachePlan` 枚举分发。
- `impl CloneForPlanCache for PhysicalPlanNode`：先克隆节点本体，再逐个递归克隆 `children`；任一子节点返回 `None` 时，`collect::<Option<Vec<_>>>()?` 令整棵子树失败。
- `PhysicalHashJoin` 实现：`RuntimeFilterList` 非空时拒绝；否则通过 `BasePhysicalJoin.CloneForPlanCacheWithSelf` 克隆基类，分别复制等值条件、NULL-aware 等值条件和配置字段，并令新副本的 runtime-filter 列表为空。
- `PhysicalMergeJoin` 实现：克隆 `BasePhysicalJoin`、方向标记和比较函数集合。
- `PhysicalIndexJoin` 实现：从基类取得 context，克隆基类和可选 `InnerPlan`，并复制 ranges、offset/length 向量、比较过滤器、两侧 hash key、等值条件及 decorrelated-apply 标记。`InnerPlan` 的 `Result` 经 `transpose().ok()?` 转成缓存失败。
- `Insert`、`Update`、`Delete` 实现：只要 `fk_checks` 或 `fk_cascades` 非空就拒绝。`Insert` 还递归处理可选 `select_plan`；`Update` 与 `Delete` 当前仅在门禁后执行派生 `Clone`。
- `LegacyPhysicalIndexHashJoin`、`LegacyPhysicalIndexMergeJoin`：递归克隆 `outer` 与 `inner`，其余字段由结构体 `Clone` 补齐。
- `PhysicalLocalIndexLookup`：递归克隆 `index_plan` 和 `table_plan`，其余字段由结构体 `Clone` 补齐。
- 私有 `TransposeOption<T>`：只服务于 `Insert.select_plan`，把 `Option<Option<T>>` 解释为“没有子计划 / 有且成功 / 有但失败”三态。
- `pub enum CachePlan`：列出 25 个 Go 清单对应变体；其中大量算子暂以 `PhysicalPlanNode` 承载，只有 DML、legacy index hash join 和 local index lookup 等少数变体保留具体 Rust 类型。
- `impl CloneForPlanCache for CachePlan`：对每个变体做穷尽匹配，并用 `?` 将载荷克隆失败传播为整个枚举克隆失败。

文件没有模块级常量、宏、异步函数或条件编译项。唯一条件编译在模块入口：独立测试模块由 `#[cfg(test)] mod plan_clone_generated_test` 接入。

## 执行流程

典型流程从某个静态类型的 `clone_for_plan_cache()` 开始：

1. 先检查该类型的不可缓存门禁。例如 DML 检查外键元数据，hash join 检查 runtime filter。
2. 克隆基类或整个结构体。join 会调用 `CloneForPlanCacheWithSelf` 并用当前计划的 `SCtx()`；通用节点直接调用 `Clone`。
3. 对可能跨 session 携带可变状态的子结构做显式递归或字段级复制，包括 children、inner/select/index/table plan、表达式列和 offset 向量。
4. 任一递归分支失败便通过 `?` 返回 `None`，不会留下部分成功的结果。
5. 全部字段完成后返回 `Some(cloned)`。若入口是 `CachePlan`，枚举层只负责选择上述实现并重新包回相同变体。

`Insert` 的可选子计划值得单独说明：原值为 `None` 时得到 `Some(None)`；原值存在且克隆成功时得到 `Some(Some(plan))`；原值存在但克隆失败时得到外层 `None`，从而拒绝整个 Insert 计划缓存。

## 数据与状态

本文件没有全局可变状态。克隆操作只读取 `&self`，返回拥有所有权的新值。主要状态边界如下：

- 树形状态：`PhysicalPlanNode.children`、legacy join 的 `outer/inner`、local lookup 的 `index_plan/table_plan` 必须递归成功，保证父副本不继续引用原子计划。
- session context：join 实现从原计划读取 `SCtx()` 并传给基类或 inner plan 克隆；trait 本身没有 `new_ctx` 参数，因此不能像 Go 实现一样由调用方显式切换 context。
- 表达式与集合：hash/index join 显式重建条件、hash key 和若干向量；其他字段可能通过派生 `Clone` 复制。这里的“克隆”是否深拷贝取决于各字段自己的 `Clone`/专用 `Clone` 方法契约。
- 不可缓存状态：DML 的外键检查/级联、hash join 的非空 runtime filter 被视为硬门禁。返回值不携带失败原因，只有 `None`。
- `CachePlan` 的 25 个变体是封闭类型清单，不是缓存容器；文件中没有缓存 key、容量、命中率或生命周期管理状态。

## 依赖与调用关系

直接 Rust 依赖均在本 crate 内或由同目录 `Cargo.toml` 提供：

- `physical_common_plans::{Insert, Update, Delete, PhysicalPlanNode}` 提供 DML 与通用计划节点。
- `physical_index_hash_join::LegacyPhysicalIndexHashJoin`、`physical_index_merge_join::LegacyPhysicalIndexMergeJoin`、`physical_indexlookup::PhysicalLocalIndexLookup` 提供具体复合计划载荷。
- crate 根再导出的 `PhysicalHashJoin`、`PhysicalMergeJoin`、`PhysicalIndexJoin` 提供 join 实现；`expression` 是 Cargo 中声明的 `astersql-expression` 依赖，用于列和标量函数的专用克隆。
- 下游方法包括 `CloneForPlanCacheWithSelf`、`clone_physical`、`Column::Clone`、`ScalarFunction::clone_scalar` 和 `ColWithCmpFuncManager::cloneForPlanCache`。

上游方面，`lib.rs` 公开模块但没有 `pub use plan_clone_generated::*`。仓库精确文本搜索只发现 `plan_clone_generated_test.rs` 导入本 trait；没有生产调用者引用 `CachePlan`。RustCodeGraph 将同名 `clone_for_plan_cache` 候选关联到 `physical_utils.rs::ClonePhysicalPlansForPlanCache` 和 `planbuilder_runtime.rs`，但这些位置的接收者是 `Box<dyn base::PhysicalPlan>`/`base::Plan`，调用的是 `base::Plan::clone_for_plan_cache(new_ctx)`，签名为 `(Option<Box<dyn Plan>>, bool)`，并非本文件的无参数 `Option<Self>` trait。故当前主链关系应记录为“模块已编译、测试可调用，但未验证生产接线”，而不能宣称计划构建或执行路径已使用 `CachePlan`。

## 错误处理与边界

本文件不用 `Result` 暴露诊断信息，所有不支持状态均压缩成 `None`：

- `Insert`、`Update`、`Delete` 遇到任一非空外键检查或级联集合时失败。
- `PhysicalHashJoin.RuntimeFilterList` 非空时失败；成功副本明确持有空列表。
- 任何 children、outer/inner、index/table/select 子计划克隆失败都会向上传播。
- `PhysicalIndexJoin.InnerPlan` 的错误通过 `.transpose().ok()?` 丢弃具体错误，只保留失败事实。
- `CloneForPlanCacheWithSelf` 返回 `None` 时 join 克隆直接失败。

需要注意的兼容边界是“非空”与 Go 的“非 nil”并不完全等价：Go 对 DML 的空但非 nil slice 也会拒绝，而 Rust 用 `is_empty()` 会接受空 `Vec`。当前测试只覆盖非空外键集合，没有覆盖这一表示差异。另一个边界是通用 `PhysicalPlanNode` 只保证 children 递归克隆，无法表达 Go 每个具体算子的特殊字段门禁；因此 `CachePlan::TableScan` 等通用变体不能自动等价于 Go 对应实现的全部安全检查。

## 并发与资源生命周期

代码不创建线程、异步任务、锁、通道、事务或 I/O 资源。方法仅在调用栈内构造拥有所有权的新计划，失败时由 Rust 自动丢弃已经构造的局部副本，不需要显式回滚。

并发安全不是本 trait 单独保证的：字段中若含 `Arc` 或其他共享所有权，派生 `Clone` 可能共享底层只读/同步状态；表达式专用克隆和基类克隆则决定哪些对象必须分离。文件没有 `Send`/`Sync` 约束，也没有证明生成副本可跨线程。生命周期上，返回对象不借用 `self`，但 join 取得的 context 是否共享内部 session 状态由 `ContextRef` 的实现决定。

## 与 Go 版本的对应关系

同路径 `plan_clone_generated.go` 是 `plan_clone_generator` 生成的权威对照，包含 25 个 `CloneForPlanCache(newCtx)` 方法。Rust 文件将这 25 个名称保留在注释参考和 `CachePlan` 变体中，但可执行实现并非逐类型完整复刻：

- 对齐部分：DML 外键门禁、hash join runtime-filter 门禁、子计划失败向上传播，以及 join/DML 若干字段的专用克隆意图与 Go 一致。
- 表示收敛：TableScan、IndexScan、Selection、Projection、TopN、Limit、两种 Agg、多个 Reader、PointGet、Union 等在 `CachePlan` 中都由 `PhysicalPlanNode` 承载，只执行通用 children 克隆；Go 则逐类型重建表达式、列、ranges、flattened plan、partition 信息等字段，并有额外拒绝条件。
- 上下文差异：Go 每个方法都接收 `newCtx` 并重绑基类；Rust trait 不接收 context，只有部分 join 从旧计划读取 context。
- 返回差异：Go 返回 `(base.Plan, bool)`，Rust 返回 `Option<Self>`；Rust 本文件没有不可缓存原因字符串，也不通过 `dyn base::Plan` 返回。
- 类型差异：Go 的 `PhysicalIndexHashJoin` 在克隆后重置 `Self` 自引用；Rust 这里实现的是 `LegacyPhysicalIndexHashJoin`，通过递归 outer/inner 后结构更新完成，没有对应的 `Self` 字段操作。

`cache_snapshot_contract_test.rs` 通过读取 Go 文件验证 crate 根的 `CACHE_SNAPSHOT_PLAN_CONTRACT` 恰有 25 项并保持类型映射；它证明清单完整性，不证明本文件 25 个变体都具备 Go 的字段级行为。直接行为测试 `plan_clone_generated_test.rs` 只证明三种 DML 在外键检查或级联非空时返回 `None`。

## 扩展指南

扩展时先判断目标是维护这个静态兼容层，还是生产使用的 `base::Plan::clone_for_plan_cache(new_ctx)` 主链；不要仅因同名就在两套接口间接线。

- 新增可缓存计划种类时，应同时核对 Go 生成清单、`CachePlan` 变体和枚举分发；若需要保持清单契约，还要同步 crate 根的 `CACHE_SNAPSHOT_PLAN_CONTRACT` 及 `cache_snapshot_contract_test.rs`。
- 为目前用 `PhysicalPlanNode` 占位的变体补齐语义时，应改为真实具体类型，并逐项移植 Go 方法中的基类 context 重绑、表达式/列/切片深克隆、派生 flattened-plan 重建和不可缓存门禁，不能只依赖结构体 `Clone`。
- 新增失败条件时，应在独立的 `plan_clone_generated_test.rs` 增加成功与拒绝用例；不要把测试嵌入生产源文件。若变更生产主链，还需在对应具体算子的独立测试或 `cache_snapshot_test.rs` 验证新 context、深克隆隔离和 round trip。
- 修改 `Insert` 可选子计划逻辑时要保留 `TransposeOption` 的三态不变量；若改成 `Result`，应明确是否需要向上传递具体失败原因。
- 性能风险集中在递归深克隆和向量/表达式复制；正确性风险集中在遗漏共享可变字段、保留旧 session context、未重建派生 plan 列表，以及错误接受 Go 明确拒绝的特殊状态。
- 该文件头部声明包含 Go 参考实现且 Go 原文件标注“生成，勿直接编辑”。若 Go 行为变化，应优先确认生成器和 Go 输出，再同步 Rust 可执行部分与测试，避免只修改注释副本造成漂移。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引为 855 行、48 个符号。
- RustCodeGraph `node --file pkg/planner/core/operator/physicalop/plan_clone_generated.rs`：核对了注释参考区与第 594--855 行全部可执行声明，包括 `CloneForPlanCache`、10 个载荷实现/辅助实现、`TransposeOption`、`CachePlan` 及其分发实现。
- RustCodeGraph `query CloneForPlanCache`、`query CachePlan`、`query TransposeOption` 和 `node clone_for_plan_cache`：核对同名符号、候选调用边及 `base::Plan` 同名接口；再用精确引用搜索排除了把动态主链误认成本文件 trait 的结论。
- 读取 `pkg/planner/core/operator/physicalop/Cargo.toml` 与 `lib.rs`：确认 crate 名称、`expression` 等依赖、公开模块声明以及测试模块的 `#[cfg(test)]` 接线。
- RustCodeGraph 读取 `pkg/planner/core/operator/physicalop/plan_clone_generated.go` 全部 552 行：确认 25 个生成方法、`newCtx` 参数、字段级深克隆、派生列表重建和不可缓存条件。
- RustCodeGraph 读取 `plan_clone_generated_test.rs` 全部 65 行：确认直接测试仅覆盖 Insert/Update/Delete 的外键检查与级联拒绝。
- RustCodeGraph 读取 `cache_snapshot_contract_test.rs`：确认 25 项 Go/Rust 类型清单的漂移检查；抽查 `cache_snapshot_test.rs` 的 DML round trip 与外键拒绝用例，确认它验证的是另一套 `CachedPlan` 快照主链。
- 结构验证按任务要求执行；本任务是纯文档分析，未运行 Cargo，也未修改 Rust、Go、Cargo 或只读的总计划。
