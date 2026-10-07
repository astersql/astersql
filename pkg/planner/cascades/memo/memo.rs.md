# `pkg/planner/cascades/memo/memo.rs`

## 文件定位

本文件实现 Cascades 优化器的 Memo 图总控：把逻辑计划树转换成由等价类 `Group` 和组表达式 `GroupExpression` 组成的搜索空间，维护全局表达式去重、等价 Group 合并、根节点以及逻辑计划备选枚举。源码由同 crate 的 [`lib.rs`](lib.rs) 私有声明为 `mod memo` 后整体再导出；crate 名为 `astersql-planner-cascades-memo`，其边界与直接依赖见 [`Cargo.toml`](Cargo.toml)。

应用主链入口在 [`../cascades.rs`](../cascades.rs)：外层 `cascades::Memo` 包装本文件的 `Memo`，`Optimizer::NewOptimizer` 经外层 `Memo::Init` 调用本文件 `Memo::Init`，随后以根 Group 创建优化任务；规则任务则通过 `TaskContext::CopyIn`、`CopyInWithChildren` 和 `RemoveOut` 回写 Memo。因此本文件位于“逻辑计划输入 → Memo 探索空间 → 规则变换/枚举”的中枢，而不是逻辑算子本身或任务调度器。

## 核心职责

- `Memo` 统一拥有 Group ID 生成器、根 Group、存活 Group 集合、ID 索引和全局表达式集合（`Memo` 第 23–34 行）。
- `CopyIn`/`Init` 自底向上把逻辑计划树拆成 Group 图，先派生统计信息，再移走算子孩子并用子 Group 引用替代（第 75–104、192–198、349–356 行）。
- `InsertGroupExpression` 用 `GroupExpression::GetHash64` 与 `Equals` 做全局去重；重复表达式落入另一目标 Group 时触发 `mergeGroup`（第 155–180、309–329 行）。
- `mergeGroup` 重写父表达式的孩子引用，维护正向/反向边和全局索引，并在重写产生新的表达式冲突时延后递归合并上层 Group（第 223–307 行）。
- `RemoveOut` 在规则产生替代表达式后删除旧表达式，维护子 Group 的父引用，并用 abandoned 标志让已入队任务能够识别失效对象（第 130–138 行）。
- `IteratorLP` 从根 Group 枚举每个 GroupExpression 及所有孩子备选的笛卡尔积，产出不修改原逻辑算子的 `PlanAlternative`（第 358–496 行）。

## 主要符号

- `pub struct Memo`：Memo 图的所有者。`groupIDGen` 分配从 1 开始的单调 ID；`rootGroup` 在 `Init` 后指向搜索根；`groups` 和 `groupID2Group` 是存活 Group 的顺序视图与按 ID 视图；`globalExpressions` 是跨 Group 去重表。
- `Memo::NewMemo(&[u64])` 与包级 `NewMemo`：创建空 Memo。容量参数仅为 Go API 对齐而保留，当前 Rust 容器没有使用它；`Default` 等价于空容量构造。
- `Memo::Init(LogicalPlanRef) -> logicalop::Result<GroupExpressionRef>`：只允许空 Memo 初始化一次，调用 `CopyIn(None, plan)` 后设置根 Group。
- `Memo::CopyIn`：完整逻辑树入口。它先调用 `derive_tree`，再通过 `TakeChildren` 拆下孩子，递归创建孩子 Group，最后构造并插入当前 GroupExpression。
- `Memo::CopyInWithGroupChildren`：规则输出入口。若传入计划仍有孩子或显式 `child_groups` 为空，则回退到 `CopyIn`；否则把浅逻辑算子和 Binder 已绑定的子 Group 直接组合，避免丢失规则匹配所得输入。
- `Memo::InsertGroupExpression`：返回 `(实际使用的表达式, 是否新插入)`。全局命中时返回既有句柄，必要时合并所属 Group；未命中时使用指定目标或新建 Group，并登记孩子到父表达式的反向边。
- `Memo::NewGroup`、`NewGroupExpression`：分别创建已登记 Group 和尚未插入的表达式。后者会由 `GroupExpression::new` 立即计算语义哈希。
- `Memo::mergeGroup`、`replaceGEChild`：crate 内部结构维护入口；前者执行 source → target 合并，后者替换表达式输入并重新计算哈希。
- `Memo::RemoveOut`、`Destroy`：分别清除单个旧表达式和整个 Memo 状态。
- `Memo::GetGroups`、`GetGroupID2Group`、`GetRootGroup`：返回 `Rc` 句柄的快照，而不是借出内部容器；调用者仍可通过 `RefCell` 修改对象本身。
- `Memo::ForEachGroup`：按 `groups` 当前顺序遍历，回调返回 `false` 时停止。
- `PlanAlternative`：一次具体选择，保存本节点 `Expression`、孩子备选树和自底向上的 `PlanIDsHash`；`TP`、`ID` 转发到包装的逻辑算子。
- `IteratorLP`：`NewIterator` 创建的枚举器；`Each`/`Next` 消费预计算备选，`dfs` 与 `cartesian_product` 完成展开。
- `derive_tree`：内部递归函数，对完整逻辑树后序调用 `LogicalPlan::DeriveStats(true)`，错误用 `logicalop::Result` 向上传播。
- `global_expression_count`、`set_root_group`：仅 `cfg(test)` 可见，用于验证全局表清理和独立构造枚举图，不是生产 API。

## 执行流程

1. `Optimizer::NewOptimizer` 在 [`../cascades.rs`](../cascades.rs) 创建外层 Memo，并把 `RootExpression()` 交给本文件 `Memo::Init`。
2. `Init` 断言 Memo 尚无 Group，进入 `CopyIn(None, root)`。`CopyIn` 首先用 `derive_tree` 后序派生整棵树的统计信息；随后 `TakeChildren` 把当前算子变成无孩子的包装对象。
3. 每个原孩子递归执行 `CopyIn(None, child)`。返回表达式必须已有所属 Group；这些 Group 按原输入顺序组成父表达式的 `Inputs`。指定目标时会断言目标不同时出现在孩子列表中，以阻止直接自环。
4. `NewGroupExpression` 计算“算子语义 + 子 GroupID”的哈希。`InsertGroupExpression` 先在 `globalExpressions` 中以哈希加 `Equals` 查重；未命中则创建或采用目标 Group，通过 `Group::Insert` 设置所属 Group 并登记孩子的父表达式弱引用。
5. 仅当表达式确实新插入且调用方没有指定目标 Group 时，`GroupExpression::DeriveLogicalProp` 为新等价类写入 schema、统计、FD、单行性、可能排序属性和 TiFlash 能力。规则向已有目标 Group 插入表达式时不重复派生该 Group 的逻辑属性。
6. 规则结果若已经由 Binder 给出子 Group，则 `CopyInWithGroupChildren` 直接走步骤 4；否则按完整树处理。任务侧直接调用证据见 [`../cascades.rs`](../cascades.rs) 的 `TaskContext::CopyInWithChildren`。
7. 若全局去重命中且既有表达式所属 Group 与目标不同，`mergeGroup(existing_group, target)` 先从 `groups`/`groupID2Group` 移除 source，并在必要时把根改为 target；再遍历 source 的存活父表达式，暂时从全局表及原 owner 删除，替换孩子、重算哈希并插回。
8. 父表达式重写后若与全局表达式冲突：同 owner 内删除冗余项并用 `GroupExpression::mergeTo` 合并探索 mask；不同 owner 则记录待合并对。最后 `Group::mergeTo(source, target)` 搬迁 source 表达式，再执行延后的上层递归合并，避免在父结构未稳定时递归修改它。
9. `NewIterator` 从 `rootGroup` 执行 DFS。每个 Group 的每条逻辑表达式分别递归枚举孩子，`cartesian_product` 组合多孩子备选；按孩子 `PlanIDsHash` 顺序再加当前逻辑算子 ID 计算本节点指纹。全部结果存入 `alternatives`，之后 `Next` 仅移动游标。

## 数据与状态

Memo 图采用以下不变量：每个存活 Group 同时出现在 `groups` 与 `groupID2Group`；每条已插入 GroupExpression 有一个可升级的 owner 弱引用；表达式强持有其输入 Group；输入 Group 以弱引用记录父表达式；全局表中的表达式应有 owner，且不存在另一条同时满足相同哈希和 `Equals` 的表达式。相关底层实现见 [`group.rs`](group.rs) 的 `Group::Insert/Delete/mergeTo/Check` 和 [`group_expr.rs`](group_expr.rs) 的 `GroupExpression::new/Equals/Init/mergeTo`。

`groups` 使用 `Vec` 保留创建顺序，ID 由 [`group_id_generator.rs`](group_id_generator.rs) 的 `NextGroupID` wrapping 加一生成。合并会从存活列表与 ID 索引移除 source，但不会重新编号；快照 API 克隆 `Rc`，因此旧 Group 句柄可继续存在，不过已经不再属于 Memo 的存活索引。

`globalExpressions` 当前是线性 `Vec`，`find_global` 每次线性扫描，并以预计算 hash 先筛选、`Equals` 再确认。表达式孩子被替换后必须先移出全局表，调用 `Init` 重算 hash，再重新去重；跳过这一步会破坏全局唯一性。

`PlanAlternative` 保存 GroupExpression 句柄而非克隆逻辑算子。其 `PlanIDsHash` 是结构/节点 ID 指纹，不能代替完整语义相等判断。零孩子表达式的笛卡尔积以一个空孩子组合为单位，因此叶节点仍会产生一条备选；任一孩子没有备选时，组合结果为空。

## 依赖与调用关系

上游已核对的生产调用如下：

- [`../cascades.rs`](../cascades.rs) 的外层 `Memo::NewMemo/Init/CopyIn/RemoveOut/Destroy` 转发到本文件同名能力；`Optimizer::NewOptimizer` 用初始化结果取得根 Group 并启动 `NewOptGroupTask`。
- 同文件 `TaskContext::CopyIn` 在规则任务中插入完整输出，`TaskContext::CopyInWithChildren` 调用 `CopyInWithGroupChildren` 保留 Binder 绑定的子 Group。
- [`../task/base.rs`](../task/base.rs) 的任务上下文入口调用 `CopyIn`；[`../task/task_apply_rule.rs`](../task/task_apply_rule.rs) 在规则替换后调用 `RemoveOut`。

下游关系如下：

- `Group`/`GroupExpression`/`GroupIDGenerator` 来自当前 crate，承担等价类容器、表达式哈希/属性与 ID 分配。
- `logicalop::{LogicalPlan, LogicalPlanRef}` 提供孩子拆装、统计派生、算子类型和节点 ID；`CopyIn` 的可恢复错误来自这里。
- `cascades_base::NewHashEqualer` 用于全局表达式哈希语义和备选计划指纹。
- `std::rc::Rc`、`RefCell`（通过引用别名）及 `Weak` 构成单线程共享可变图；`HashMap`/`HashSet` 分别用于 ID 索引和 DFS 路径检测。

[`Cargo.toml`](Cargo.toml) 还声明 `core-base`、`property`、`planctx`，它们主要经相邻 `group.rs`/`group_expr.rs` 参与逻辑属性与物理任务缓存；本文件直接导入的 workspace crate 是 `cascades-base` 与 `logicalop`。没有 feature 条件；只有两个测试辅助方法受 `cfg(test)` 控制。

## 错误处理与边界

`Init`/`CopyIn`/`CopyInWithGroupChildren` 返回 `logicalop::Result`。本文件明确可传播的错误路径是 `derive_tree` 中任一 `DeriveStats(true)` 失败；递归用 `?` 保留错误。`GroupExpression::DeriveLogicalProp` 在当前 Rust 版本不返回 `Result`，其前置条件不满足时通过断言或 `expect` 失败。

结构不变量采用 panic-fast：重复初始化触发 `assert!`；拷入孩子找不到 owner、全局表达式没有 owner会 `expect`；指定目标与孩子 Group 相同会断言；`Group::Insert` 意外拒绝一条已通过全局检查的表达式也会断言；枚举路径重复 GroupID 会报告 Memo 备选图成环。调用方必须把这些视为内部一致性错误，而非用户 SQL 可恢复错误。

`CopyInWithGroupChildren` 只有在逻辑算子自身无孩子且显式孩子非空时采用显式 Group。因而合法的零孩子叶算子传空列表会回退 `CopyIn`；仍带完整孩子树的计划也会忽略外部列表并按树重建。`RemoveOut` 假设表达式已登记过父边；若边表不一致，`Group::removeParentGEs` 会断言。

`NewIterator` 在未初始化根时生成空迭代器。枚举将所有备选一次性物化，组合数是各层表达式数及孩子备选数的乘积；大搜索空间可能造成显著时间和内存开销。DFS 只检测当前路径上的 GroupID，允许不同分支共享同一 Group，但拒绝环。

## 并发与资源生命周期

本实现是单线程内部可变模型：`GroupRef` 和 `GroupExpressionRef` 使用 `Rc<RefCell<_>>`，既不 `Send` 也不 `Sync`。上层 [`../cascades.rs`](../cascades.rs) 同样以 `Rc<RefCell<Memo>>` 让串行任务栈共享 Memo；不得把这些句柄跨线程传递。运行时借用冲突会由 `RefCell` panic，因此新增逻辑应缩短 `borrow`/`borrow_mut` 守卫，尤其不能在持有可变借用时递归进入可能再次借用同一对象的合并路径。

所有权方向为：Memo 强持有 Group；Group 强持有 GroupExpression；GroupExpression 强持有孩子 Group；表达式到 owner、Group 到父表达式均为弱引用。这避免 owner/表达式和孩子/父表达式的引用环。`RemoveOut` 会断开孩子反向边、清全局登记并标 abandoned；`mergeGroup` 会清 source；`Destroy` 逐 Group 调用 `Clear`，再重置 ID、根、容器和全局表。外部若仍持有 `Rc`，对象可能在 `Destroy` 后继续存活，但 owner 回指及 Memo 索引已清除，不应再作为当前优化阶段成员使用。

`IteratorLP` 创建时同步、完整地枚举，之后不再读取 Memo 图；其 alternatives 中的 `Rc` 会延长相关表达式及孩子 Group 的生命周期。因此在创建迭代器后并行修改 Memo 不受支持，且持有迭代器会推迟图对象释放。

## 与 Go 版本的对应关系

直接对照文件为 [`memo.go`](memo.go)，测试对照为 [`memo_test.go`](memo_test.go)。Rust 保留了 Go 的公开命名和主算法：`NewMemo`、`Init`、`CopyIn`、全局去重、source → target 合并、父表达式重写、延后递归合并、`RemoveOut` abandoned 语义，以及从根 Group 枚举逻辑计划组合。

已确认的实现差异如下：

- Go 使用链表保存 Group、generic hashmap 保存全局表达式并复用 Memo 内 hasher；Rust 使用 `Vec<GroupRef>`、线性 `globalExpressions`，`GetHasher` 每次新建 hasher。Rust 构造参数 `capacities` 当前未用于预分配；Go 版本则把容量交给 hashmap。
- Go `CopyIn` 通过孩子是否为 `GroupExpression` 区分既有 Memo 输入与新逻辑子树；Rust 的逻辑计划 trait 对象不直接承载 GroupExpression，改由 `CopyInWithGroupChildren` 显式接收 Binder 提供的 Group。
- Rust `CopyIn` 在拆孩子前先对整棵树执行 `derive_tree`；Go 对照的 `CopyIn` 不在此处做相同的整树预遍历，而在新 Group 插入后由 `DeriveLogicalProp` 返回错误。Rust 的逻辑属性派生当前为不可恢复断言式 API。
- Go 的 `IteratorLP::Next` 用栈逐次选择表达式，重设逻辑算子的 Children 与 `PlanIDsHash`，返回 `base.LogicalPlan`；Rust 在构造迭代器时预计算不可变选择树 `PlanAlternative`，保留表达式句柄而不回写包装的逻辑算子。这是返回类型、求值时机和副作用上的实质差异。
- Go 通过对象 ID/`Equals` 比较 Group；Rust 在合并与边维护的关键位置使用 `Rc::ptr_eq` 区分具体共享对象，同时表达式语义相等仍基于 GroupID。
- Go 的容器和裸指针式关系由 GC 管理；Rust 以 `Rc`/`Weak` 明确表达强弱所有权，并通过 `RefCell` 提供单线程运行时借用检查。

因此，“算法意图对齐”不等于所有性能、错误传播和迭代副作用完全一致；扩展或回归时应同时检查两份实现，而不能只根据同名函数推断行为。

## 扩展指南

- 新增 Memo 插入路径时优先复用 `CopyIn` 或 `CopyInWithGroupChildren`，不要直接向 `Group::logicalExpressions` 写入；必须同步维护 owner、全局唯一表及所有孩子的父表达式弱引用。
- 修改表达式的算子语义或 `Inputs` 前，先从全局表移除；修改后调用 `GroupExpression::Init` 重算哈希并重新执行全局冲突判断。可参考 `mergeGroup` + `replaceGEChild` 的顺序。
- 调整 Group 合并时必须保持 source 从两个存活索引同步移除、根重定向、父边重写、同 Group 冗余表达式合并探索 mask、跨 Group 冲突延后递归五项约束。直接递归合并正在遍历的父结构有借用冲突和半更新状态风险。
- 若优化 `globalExpressions` 为哈希表，键必须同时保持 `GetHash64` 与 `Equals` 语义，并处理孩子替换导致键哈希变化的 remove/reinsert 生命周期；还需评估 Go 的容量参数是否应恢复实际作用。
- 若把 `IteratorLP` 改为惰性枚举，应保留确定的组合顺序、无根为空、路径环检测和自底向上 hash 规则，并明确 Memo 在迭代期间是否允许修改。当前全量物化的峰值内存是最直接的性能扩展点。
- 新功能测试应放在独立的 [`memo_test.rs`](memo_test.rs)，不要内嵌到生产源文件。插入/删除/合并需同时断言 `GetGroups`、`GetGroupID2Group`、owner、父引用和 abandoned 状态；枚举需覆盖叶节点、多孩子笛卡尔积、提前停止、无根及环路防护。与 Go 迁移语义有关的改动还应同步核对 [`memo_test.go`](memo_test.go)。
- 对外层优化器接线的修改应同步检查 [`../cascades.rs`](../cascades.rs) 及任务上下文；尤其规则输出的浅算子必须继续携带 Binder 的 `child_groups`。

兼容风险主要是 Go/Rust 迭代返回值不同和统计/属性错误路径不同；正确性风险集中在全局哈希失效、父反向边漏维护和递归合并顺序；性能风险集中在线性全局查重及备选笛卡尔积全量物化。

## 验证依据

本说明基于以下直接证据完成：

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中同时存在 `memo.rs`、`memo.go` 及各自独立测试。
- RustCodeGraph 文件/符号读取：[`memo.rs`](memo.rs) 全 496 行；`CopyIn` 的已识别下游为 `derive_tree`、`NewGroupExpression`、`InsertGroupExpression`，上游为 `Init` 和 `CopyInWithGroupChildren`；`InsertGroupExpression` 下游为 `find_global`、`NewGroup`、`mergeGroup`，上游为两个 CopyIn 入口；`NewIterator` 下游为 `IteratorLP::new`；`mergeGroup` 下游包含 `remove_global`、`replaceGEChild`、`find_global`。
- 应用入口：[`../cascades.rs`](../cascades.rs) 的外层 `Memo`、`TaskContext`、`Context` 与 `Optimizer::NewOptimizer`。
- crate 与所有权实现：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`group.rs`](group.rs)、[`group_expr.rs`](group_expr.rs)、[`group_id_generator.rs`](group_id_generator.rs)。目标包及上层 `pkg/planner/cascades` 未发现 `doc.go`。
- Rust 独立测试：[`memo_test.rs`](memo_test.rs) 的 `TestMemo`、`TestInsertGE`、`TestMergeGroup`、`TestRecursiveMergeGroup`、`TestIteratorLogicalPlan`，分别验证建图/ID、去重删除、父边重写、上层递归合并和 2×2 笛卡尔积及唯一 hash。
- Go 对照：[`memo.go`](memo.go) 全文件，以及 [`memo_test.go`](memo_test.go) 对应建图、合并、递归合并与迭代场景。
- 本任务是纯文档分析，按任务约束未运行 Cargo 或代码测试；交付验证只执行任务指定的 11 章节结构检查，并人工复核上述路径与符号能够回答文件定位、运行流程和安全扩展要求。
