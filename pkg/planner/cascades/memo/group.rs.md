# `pkg/planner/cascades/memo/group.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-memo` crate，是 Cascades Memo 图中“逻辑等价类”的容器实现。crate 边界由 [`Cargo.toml`](./Cargo.toml) 声明，入口 [`lib.rs`](./lib.rs) 将私有 `group` 模块的公开项重新导出；直接依赖包括 `cascades-base` 的哈希器契约、`core-base` 的物理任务接口和 `property` 的逻辑/物理属性类型。

在完整链路中，[`memo.rs`](./memo.rs) 负责创建、登记、合并 Group 以及维护全局表达式表，本文件负责单个 Group 内部的表达式集合、operand 首项索引、父表达式回边、逻辑属性、探索状态和最优物理任务缓存。[`group_expr.rs`](./group_expr.rs) 则表示“逻辑算子 + 子 Group”，并负责表达式自己的哈希、探索 mask、属性派生和合并清理。三者共同构成 Memo 图；本文件不是 SQL 入口，也不直接执行优化规则。

## 核心职责

- 用 `Group` 表示一组逻辑语义等价的 `GroupExpression`，并以 `GroupID` 作为组级哈希和相等判断的唯一依据（`Hash64`、`Equals`）。
- 用 `Insert`、`Delete` 和 `rebuild_operand_index` 维护表达式集合、表达式到所属 Group 的弱回指，以及 `Operand2FirstExpr` 的“每种算子首项”索引。
- 用 `parentExpressions` 保存“哪些父表达式把本组作为输入”的反向弱引用；`addParentGEs`、`removeParentGEs` 和 `Check` 维护、检查这条反向边。
- 暴露逻辑属性和探索状态的读写接口，供 [`group_expr.rs`](./group_expr.rs) 的属性派生与 [`task/task_opt_group.rs`](../task/task_opt_group.rs) 的组探索调度使用。
- 在 `mergeTo` 中把 source 的表达式和父引用迁入 target；目标已有等价表达式时，委托 `GroupExpression::mergeTo` 合并表达式探索状态并拆除旧子回边。
- 以物理属性 `HashCode()` 的字节序列为键缓存最优 `Task`。当前 Rust Cascades 源码搜索只找到本文件中的 `SetBestTask`/`GetBestTask` 定义，未找到生产调用者，因此这是已实现但尚无当前 Rust 主链接线证据的接口。

## 主要符号

- `pub type GroupRef = Rc<RefCell<Group>>`：单线程共享可变句柄。`Rc` 允许 Memo、父表达式和任务共享 Group，`RefCell` 把借用检查移到运行期。
- `pub struct Group`：核心状态载体。`groupID`、`logicalExpressions`、`parentExpressions` 和 `logicalProp` 仅 crate 内可见；`Operand2FirstExpr` 公开；`explored` 与 `bestPhysicalMap` 私有。
- `Group::NewGroup(Option<LogicalProperty>) -> GroupRef` 与包级 `NewGroup(...)`：创建 ID 为 `0`、表达式和索引为空、未探索且无最优任务的 Group。真正进入 Memo 时，`Memo::NewGroup` 随后分配非零 ID 并登记到 `groups`/`groupID2Group`。
- `Hash64(&mut dyn Hasher)`、`Equals(&Group)`：只读 `groupID`。它们是固有方法，不是本文件中的 trait impl；调用方不能用表达式内容替代组 ID 的身份语义。
- `Insert(&GroupRef, GroupExpressionRef) -> bool`：先用 `GroupExpression::Equals` 线性去重；新表达式插到同 operand 首项之后，设置表达式的 `group` 弱引用，再重建索引。重复时返回 `false` 且不改状态。
- `Delete(&GroupRef, &GroupExpressionRef)`：按指针相同或语义 `Equals` 查找，未找到即静默返回；找到后移除、清空其所属组弱引用并重建索引。
- `GetGroupID`、`GetLogicalExpressions`、`GetFirstElem`：分别返回 ID、克隆后的表达式句柄快照、以及全组或指定 operand 的首项。返回句柄快照不会复制表达式对象。
- `HasLogicalProperty`、`GetLogicalProperty`、`SetLogicalProperty`：管理组级 `LogicalProperty`。getter 返回 `Option`，调用方必须处理尚未派生的状态。
- `IsExplored`、`SetExplored`：组级布尔探索标志；表达式级按规则 mask 位于 `GroupExpression`，二者不可混用。
- `ForEachGE`：先克隆 `Rc` 句柄列表，再依次回调；回调返回 `false` 提前停止。快照保证回调迁移或删除当前项时不会破坏遍历游标。
- `addParentGEs`、`removeParentGEs`：以 `GroupExpression::addr` 的 `Rc` 地址为键登记/删除父表达式弱引用；重复登记或删除缺失项都会 panic。
- `mergeTo`、`Clear`、`Check`：分别负责组内状态迁移、局部容器清理和不变量断言。`parent_count` 仅在 `cfg(test)` 下编译。
- `SetBestTask`、`GetBestTask`：用 `PhysicalProperty::HashCode()` 查询或覆盖 `Box<dyn Task>`；getter 只借用 trait object，不转移缓存所有权。
- `rebuild_operand_index`：私有线性扫描辅助函数，为每个 `LogicalPlan.TP()` 仅记录第一次出现的下标。

文件没有模块级常量、枚举、自定义错误、异步函数或 feature 条件；唯一条件编译项是测试辅助方法 `parent_count`。

## 执行流程

典型建图路径如下：

1. `Memo::Init`/`CopyIn` 自底向上把逻辑计划转换成 `GroupExpression`；`Memo::NewGroup` 调用 `Group::NewGroup(None)`，再由 `GroupIDGenerator` 分配非零 ID。
2. `Memo::InsertGroupExpression` 先在全局表达式表中去重。未命中时选择现有 target 或新建 Group，调用 `Group::Insert`，随后把表达式登记到全局表，并对每个子 Group 调用 `addParentGEs` 建立反向边。
3. `Insert` 比较现有表达式的 `Equals`。通过去重后，根据 `LogicalPlan.TP()` 选择插入位置：已有同类型时插在其首项之后，否则追加到尾部；然后设置 owner 弱引用并重建 operand 索引。
4. `GroupExpression::DeriveLogicalProperty` 从子 Group 读取属性和当前逻辑算子状态，构造 `LogicalProperty`，最终通过 `SetLogicalProperty` 写回 owner Group。读取子属性时要求其已经派生，否则会在 `expect` 处 panic。
5. [`task/task_opt_group.rs`](../task/task_opt_group.rs) 的 `OptGroupTask::execute` 先检查 `IsExplored`，为当前表达式快照创建优化任务，最后调用 `SetExplored`，避免重复调度整个 Group。

组合并由 `Memo::mergeGroup` 统筹：它先从 Memo 的组索引移除 source，必要时替换根组，重写所有父表达式的孩子指针并处理由此产生的全局表达式冲突；然后调用本文件 `Group::mergeTo(source, target)`。`mergeTo` 先复制父引用，再逐项处理 source 表达式：目标存在等价项时从 source 删除并调用 `GroupExpression::mergeTo` 合并 mask、移除子组父回边；否则从 source 删除后插入 target。最后 `Clear` 清空 source 的表达式、operand 索引、父引用和逻辑属性。

## 数据与状态

`Group` 的关键不变量是：

- 已登记到 `Memo` 的 Group 必须有大于零且在该 Memo 内唯一的 `groupID`；`Group::NewGroup` 返回的临时值仍为 `0`，所以只能由 `Memo::NewGroup` 完成登记后再通过 `Check`。
- `logicalExpressions` 中不应存在按 `GroupExpression::Equals` 判定相等的两项。Rust 实现不保留 Go 的 `hash2GroupExpr`，而是在插入时线性精确比较，因此哈希碰撞不会错误去重，但组变大时插入成本为线性扫描。
- 每个已插入表达式的 `group` 弱引用应指向当前 owner；删除、清理或合并掉的表达式必须解除该弱引用。
- `Operand2FirstExpr[operand]` 必须是当前向量中该 `LogicalPlan.TP()` 第一次出现的下标。插入和删除都完整重建该索引，避免下标移动后悬空。
- `parentExpressions` 的每个可升级弱引用都应指向一个 `Inputs` 中含当前 `groupID` 的父表达式。以地址作为键允许语义相等但对象不同的父表达式同时存在。
- `logicalProp` 为 `None` 表示尚未派生或已清理；它与表达式集合没有自动同步。调用方改变会影响属性的逻辑结构时，需要按 Memo/属性派生流程保证不会复用陈旧属性。
- `bestPhysicalMap` 的键是物理属性哈希码而不是属性对象；正确性依赖 `PhysicalProperty::HashCode` 对任务选择语义足够稳定。相同键再次设置会释放并替换旧 `Box<dyn Task>`。

`Clear` 特意不重置 `groupID`、`explored` 或 `bestPhysicalMap`。这与 Go 的清理范围一致，但意味着清空后的 source 不是一个完全恢复初始状态的新 Group；当前用途是合并后废弃，不能把它当作可安全复用的构造结果。

## 依赖与调用关系

直接下游依赖：

- `cascades_base::Hasher`：`Hash64` 向其写入 `groupID`。
- [`group_expr.rs`](./group_expr.rs) 的 `GroupExpression`、`GroupExpressionRef`：去重、owner 弱引用、operand 名称、父引用地址与表达式状态合并都依赖它们。
- `property::LogicalProperty`、`property::PhysicalProperty`：分别承载组级逻辑属性和最优任务缓存键。
- `core_base::Task`：`bestPhysicalMap` 中的动态任务接口。
- 标准库 `Rc`/`Weak`/`RefCell`/`HashMap`：组成 Memo 的单线程共享图与索引。

已核验的直接上游：

- [`memo.rs`](./memo.rs) 的 `RemoveOut` 调用 `Delete` 并移除孩子回边；`InsertGroupExpression` 调用 `Insert`/`addParentGEs`；`NewGroup` 调用构造器并设置 ID；`mergeGroup` 调用 `Delete`、`Insert`、`mergeTo`；`replaceGEChild` 调用 `removeParentGEs`/`addParentGEs`。
- [`group_expr.rs`](./group_expr.rs) 的属性派生读取/写入 `logicalProp`；表达式合并调用 `removeParentGEs`。
- [`task/task_opt_group.rs`](../task/task_opt_group.rs) 使用 `IsExplored`/`SetExplored` 和逻辑表达式快照驱动组优化。
- [`group_and_expr_test.rs`](./group_and_expr_test.rs) 与 [`memo_test.rs`](./memo_test.rs) 直接覆盖插入、删除、哈希/相等、父回边、属性和合并不变量；[`cascades_test.rs`](../cascades_test.rs) 从优化器层检查根 Group 探索状态。

RustCodeGraph 的目标文件 `explore` 能识别 `Insert`/`Delete`/`GetLogicalExpressions`/`IsExplored` 等测试调用点，但精确 `callers`/`callees` 查询未返回边；因此这里的调用关系以图查询定位候选，再以相邻源码和 `rg` 直接核验。尤其不能把 `SetBestTask`/`GetBestTask` 的空搜索结果解释为未来永远不会使用，只能说明当前已检查 Rust 源中没有接线证据。

## 错误处理与边界

本文件不返回 `Result`，边界失败分为“可接受的无操作/布尔结果”和“不变量 panic”：

- 插入重复表达式返回 `false`；删除不存在的表达式直接返回。这两种情况不会报错。
- `GetFirstElem`、`GetLogicalProperty`、`GetBestTask` 用 `Option` 表示不存在；调用方负责分支处理。
- `removeParentGEs` 删除缺失父引用、`addParentGEs` 重复登记、`mergeTo` 迁移一项却无法重新插入，以及 `Check` 发现 ID、operand 或父边不一致时都会断言失败。它们表达内部图已损坏，不是面向 SQL 用户的可恢复错误。
- `Check` 对父弱引用调用 `upgrade().expect(...)`，所以父表达式若已释放但回边未清除会 panic。这是生命周期一致性检查的一部分。
- `RefCell` 的运行时借用规则是额外 panic 边界：持有同一 Group 的不可变借用时再尝试可变借用，或反之，会触发借用冲突。`Insert`/`Delete` 通过短作用域借用避免自身重叠，扩展代码也必须维持这一点。
- `Insert` 的参数类型保证表达式非空，Rust 无 Go `nil` 输入分支。`GroupID` 初值为 `0` 只适合尚未登记的 Group；直接对它调用 `Check` 会失败。

## 并发与资源生命周期

`Rc<RefCell<_>>`、`Weak` 明确限定此图为单线程结构；它没有 `Arc`、锁、原子类型、通道或异步任务，也不承诺 `Send`/`Sync`。Cascades 调度若未来跨线程共享 Memo，必须重新设计句柄和内部可变性，不能直接传递当前 `GroupRef`。

所有权图用弱边打破环：表达式集合以强 `Rc` 持有组内表达式；表达式对 owner Group 使用 `Weak`；Group 对父表达式也使用 `Weak`；表达式的 `Inputs` 则强持有子 Group。删除表达式会清 owner 弱引用，完整的 `Memo::RemoveOut` 还会从子 Group 删除父回边；只调用 `Group::Delete` 不会自动清理孩子回边，因此生产调用方需要由 Memo 层统筹。

`ForEachGE` 的句柄快照会把表达式生命周期延长到本次遍历结束，即便回调从 Group 删除该项，对象也不会在当前迭代中释放。`parentExpressions` 的弱引用不会延长父表达式生命周期；`Check` 要求表内弱引用仍可升级，正常删除/合并路径必须先移除它。`bestPhysicalMap` 独占任务的 `Box`，覆盖条目或销毁 Group 时自动释放；本文件没有手工资源、事务或网络连接需要清理。

## 与 Go 版本的对应关系

直接对照文件是 [`group.go`](./group.go)，`Cargo.toml` 的 `package.metadata.porting.go-package` 也指向 `pkg/planner/cascades/memo`。主要语义保持一致：Group 持有等价表达式、operand 首项、父表达式反向引用、逻辑属性、探索标志和按物理属性缓存的最佳任务；哈希/相等只看 ID；插入去重、合并迁移和属性接口的角色相同。

已确认的实现差异如下：

- Go 用 `container/list` 加 `hash2GroupExpr` 泛型哈希表，平均常数时间定位重复项和删除项；Rust 用 `Vec` 并以 `Equals` 线性扫描，插入/删除后重建索引。Rust 保留了哈希冲突必须继续精确比较的行为，但性能特征不同。
- Go 的 operand 键是 `pattern.Operand`，Rust 以 `LogicalPlan.TP()` 返回的 `String` 为键；扩展或重命名计划类型时必须保持该字符串与规则/匹配层的约定一致。
- Go 的 parent 表强持有 `*GroupExpression`，Rust 值是 `Weak<RefCell<GroupExpression>>`，可避免由反向边延长父对象生命周期，但需要显式清除失效键并在检查时处理升级失败。
- Go `GetLogicalProperty` 在测试断言模式下要求非空并直接返回指针；Rust 返回 `Option<&LogicalProperty>`，把未派生状态暴露给调用方。属性派生处仍以 `expect` 保留必要的先后顺序约束。
- Go `ForEachGE` 沿链表实时遍历；Rust 先克隆 `Rc` 列表，使回调能够迁移/删除当前表达式而不使游标失效。
- Go `Equals(any)` 还处理错误动态类型与 nil；Rust 签名限定为 `&Group`，这些分支由静态类型和非空引用消除。
- Go 文件还声明了 `GroupPair` 和空实现 `ReplaceBestExpression`；当前 Rust 文件没有对应符号。由于 Go 的 `ReplaceBestExpression` 本身是 TODO/空操作，不能据此声称 Rust 缺少可观察行为。
- Go `impl/impl_and_cost.go` 明确调用 `GetBestTask`/`SetBestTask`；当前 Rust `pkg/planner/cascades/**/*.rs` 搜索没有同类调用，故物理任务缓存的 Rust 接线状态仍未验证。

测试对照以 [`group_and_expr_test.go`](./group_and_expr_test.go) 和独立 Rust [`group_and_expr_test.rs`](./group_and_expr_test.rs) 为主。Rust 测试保留了强制哈希碰撞仍按 `Equals` 区分、删除缺失/已有项、Group ID 哈希相等、父回边和逻辑属性等意图；[`memo_test.rs`](./memo_test.rs) 进一步覆盖普通及递归 Group 合并。

## 扩展指南

新增或修改 Group 行为时，优先选择最窄的接入点：

1. 新增组级元数据时，在 `Group::NewGroup` 明确初值，并决定 `Clear`、`mergeTo` 和 `Check` 应如何处理；不能只加字段而遗漏合并后 source 的生命周期语义。
2. 修改表达式身份或 operand 分类时，同步审查 `Insert`、`Delete`、`rebuild_operand_index`、`GetFirstElem`，以及 `GroupExpression::Equals/Init` 和 Memo 全局去重；保持“相等必同哈希、碰撞仍精确比较”的约束。
3. 修改父回边时必须同时检查 `Memo::InsertGroupExpression`、`RemoveOut`、`replaceGEChild`、`mergeGroup` 和 `GroupExpression::mergeTo`。强弱引用方向或清理顺序错误会导致失效弱引用、遗漏递归合并或对象泄漏。
4. 接通 Rust 物理实现/代价阶段时，可复用 `SetBestTask`/`GetBestTask`，但应先验证 `PhysicalProperty::HashCode` 的碰撞与稳定性约定，并为不同属性、覆盖缓存和未命中新增测试；不要仅照搬 Go 调用而忽略 Rust `Box<dyn Task>` 的所有权。
5. 若要并行化优化器，当前 `Rc<RefCell<_>>` 是架构边界，需要整体评估 Memo、GroupExpression、Task 和 LogicalPlan 的线程安全，不能局部替换一个别名就宣称可并发。

Rust 单元测试必须放在独立文件，不能嵌入 `group.rs`。Group 容器级行为扩展 [`group_and_expr_test.rs`](./group_and_expr_test.rs)，Memo 合并/建图行为扩展 [`memo_test.rs`](./memo_test.rs)，优化调度和探索状态可扩展 [`task/task_test.rs`](../task/task_test.rs) 或 [`cascades_test.rs`](../cascades_test.rs)。兼容性风险集中在 Go 语义漂移和公开再导出 API；正确性风险集中在去重、双向边与属性缓存失效；性能风险集中在 `Vec` 线性去重/删除和每次完整重建 operand 索引。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/planner/cascades/memo` 确认目标 Rust/Go 文件、crate 入口和独立测试均被索引。
- RustCodeGraph `node --file pkg/planner/cascades/memo/group.rs`：完整读取 294 行目标源码，确认 27 个符号、全部字段、唯一 `cfg(test)` 项及无其他条件编译分支。
- RustCodeGraph `explore "pkg/planner/cascades/memo/group.rs ..."` 与 `query`：确认 `NewGroup`、`Insert`、`Delete`、`mergeTo`、`GetLogicalExpressions`、`IsExplored` 等候选调用点及 Go/Rust 同名符号；精确 `callers`/`callees` 未返回可用边，未据此推断“无调用者”。
- 直接调用链：读取 [`memo.rs`](./memo.rs) 的 `RemoveOut`、`InsertGroupExpression`、`NewGroup`、`mergeGroup`、`replaceGEChild`，读取 [`group_expr.rs`](./group_expr.rs) 的属性派生、`addr` 与表达式 `mergeTo`，并用源码搜索核验 [`task/task_opt_group.rs`](../task/task_opt_group.rs) 的探索状态调用。
- crate 与 Go 对照：读取 [`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs) 和完整 [`group.go`](./group.go)；源码搜索确认 Go `impl/impl_and_cost.go` 使用最佳任务缓存，而当前 Rust Cascades 源中未找到对应调用。
- 测试证据：读取完整 [`group_and_expr_test.rs`](./group_and_expr_test.rs)、[`memo_test.rs`](./memo_test.rs) 及 Go [`group_and_expr_test.go`](./group_and_expr_test.go)，核对去重、哈希碰撞、删除、父回边、逻辑属性、普通合并和递归合并边界。
- 本任务只生成说明文档，按总计划不运行 Cargo。交付验证仅执行任务指定的 11 章节结构检查，并人工复核链接、符号、当前接线状态和扩展边界。
