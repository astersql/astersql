# `pkg/planner/memo/group_expr.rs`

源码：[`group_expr.rs`](group_expr.rs)

## 文件定位

`group_expr.rs` 属于 `astersql-planner-memo` crate，是旧 Cascades 优化器 Memo 数据结构中连接“逻辑算子”和“子等价类”的节点实现。crate 根 `pkg/planner/memo/lib.rs` 将本模块的公开项整体再导出；`pkg/planner/memo/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/planner/memo`。

普通逻辑计划树的边连接逻辑计划节点，而这里的 `GroupExpr.Children` 连接 `GroupRef`：每个子 Group 可以包含多个逻辑等价表达式。因此一个 `GroupExpr` 描述的是一个根逻辑算子以及其每个输入位置可选择的等价类，是 `Group` 去重、规则探索和 `ExprIter` 模式绑定的基本单元。它不同于 `pkg/planner/cascades/memo/group_expr.rs` 中的新 Cascades `GroupExpression`，本文只描述 `pkg/planner/memo` 这套旧 Memo API。

直接装配入口是 `pkg/planner/memo/group.rs::Convert2GroupExpr`：它从逻辑计划节点取走孩子，递归转换成 Group，再调用本文件的 `NewGroupExpr` 和 `SetChildren`。随后 `Convert2Group` 用原计划的 Schema 创建并插入所属 Group。旧优化器 `pkg/planner/cascades/old/optimize.rs::exploreGroup` 和规则实现 `pkg/planner/cascades/old/transformation_rules.rs` 消费这里保存的探索状态、子 Group、所属 Group 和已应用规则集合。

## 核心职责

- 用 `GroupExpr` 保存一个 `LogicalPlanRef` 根算子及有序的 `Vec<GroupRef>` 子输入，把逻辑计划树转换为 Memo 图。
- 用 `GroupExprRef = Rc<RefCell<GroupExpr>>` 提供单线程共享所有权与内部可变性，使 Group、迭代器和规则代码能共同引用、原地更新同一表达式。
- 用 `FingerPrint` 生成并缓存“子输入数量 + 子 Group 身份 + 算子哈希”的字节键，供 `Group::Insert` 去重以及 `Group::Exists`、`Group::Delete` 和 `Group::rebuild_indexes` 查找。
- 用 `ExploreMark` 按优化轮次记录表达式是否已经探索，避免同一轮重复遍历。
- 用 `appliedRuleSet: HashSet<u64>` 记录已经作用于该表达式的变换规则 ID，允许规则的 `matches` 阶段跳过重复应用。
- 通过所属 Group 的 `LogicalProperty.Schema` 提供表达式 Schema，而不在表达式节点中复制 Memo 级逻辑属性。

本文件不负责维护 Group 的等价表达式列表、执行规则、计算代价或枚举匹配；这些职责分别位于 `group.rs`、`pkg/planner/cascades/old/transformation_rules.rs`、`implementation.rs` 和 `expr_iterator.rs`。

## 主要符号

- `pub type GroupExprRef = Rc<RefCell<GroupExpr>>`：表达式的标准共享句柄。`Rc` 表示非线程安全的引用计数所有权，`RefCell` 将 Rust 借用检查移到运行时。
- `pub struct GroupExpr`：文件的唯一核心类型。
  - `ExprNode: LogicalPlanRef`：根逻辑算子，参与算子类别判断、规则匹配和 `HashCode` 计算。
  - `Children: Vec<GroupRef>`：有序子 Group；顺序参与指纹，也决定 `ExprIter` 的子模式绑定顺序。
  - `Group: Weak<RefCell<Group>>`：所属 Group 的弱引用。`Group::Insert` 成功后回填，`Group::Delete` 清空；弱引用避免 `Group -> GroupExpr -> Group` 强引用环。
  - `ExploreMark: ExploreMark`：按轮次记录探索状态；具体位图边界由 `group.rs::ExploreMark` 定义。
  - `selfFingerprint: Vec<u8>`：惰性指纹缓存，私有以约束失效方式。
  - `appliedRuleSet: HashSet<u64>`：私有规则 ID 集合。
- `pub fn NewGroupExpr(node: LogicalPlanRef) -> GroupExprRef`：保存传入算子，构造空孩子、无所属 Group、未探索、空指纹和空规则集合的表达式。
- `SetExplored(round)`、`SetUnexplored(round)`、`Explored(round) -> bool`：直接委托 `ExploreMark` 的轮次状态操作。
- `FingerPrint(&mut self) -> Vec<u8>`：首次调用时编码并缓存指纹，每次向调用者返回缓存的克隆。
- `SetChildren(&mut self, children: Vec<GroupRef>)`：整体替换子 Group，同时清空指纹缓存。
- `Schema(&self) -> Option<Schema>`：升级所属 Group 的弱引用，并克隆其逻辑属性中的 Schema；任一层缺失均返回 `None`。
- `AddAppliedRule(rule_id)`、`HasAppliedRule(rule_id) -> bool`：写入和查询规则 ID；重复写入由 `HashSet` 幂等处理。

文件没有条件编译项、trait、枚举、模块级常量或可独立失败的 `Result` API。

## 执行流程

1. `group.rs::Convert2GroupExpr` 对逻辑计划节点调用 `TakeChildren`，将每个子计划递归交给 `Convert2Group`，得到有序 `Vec<GroupRef>`。
2. 它把已经移除普通子计划的根节点传给 `NewGroupExpr`，再以 `SetChildren` 连接步骤 1 的子 Group。`SetChildren` 清空指纹，保证下一次计算看到新拓扑。
3. `group.rs::Convert2Group` 调用 `NewGroupWithSchema`；后者通过 `Group::Insert` 请求插入新表达式。
4. `Group::Insert` 可变借用表达式并调用 `FingerPrint`。首次计算先写入两字节大端 `u16` 子数量，再按 `Children` 顺序写入每个 `Group::ID()` 的八字节大端值，最后追加 `ExprNode.HashCode()`。
5. Group 用该字节串检查 `Fingerprints`。重复则返回 `false`；新表达式按 operand 聚簇插入 `Equivalents`，重建索引，并将 `GroupExpr.Group` 回填为所属 Group 的弱引用。
6. 优化探索时，`old/optimize.rs::exploreGroup` 先检查表达式的 `Explored(round)`，未探索则调用 `SetExplored(round)`，递归探索 `Children`，然后通过 `NewExprIterFromGroupElem` 枚举规则 Pattern 的绑定。
7. 规则的附加匹配可借助 `HasAppliedRule(rule_id)` 排除已经执行过的规则；变换完成后通过 `AddAppliedRule(rule_id)` 记录。规则产生的新表达式仍经 `Group::Insert` 去重。
8. 删除路径中，`Group::Delete` 用同一指纹定位表达式，移出 Group、重建索引，再清空被删表达式的所属 Group 弱引用。

`FingerPrint` 是惰性缓存：后续调用不重新访问孩子或算子，只克隆缓存。当前只有 `SetChildren` 主动使缓存失效；因此调用方必须在表达式首次参与 Group 索引前完成会影响指纹的构造。

## 数据与状态

所有状态均在内存中，无磁盘、网络或事务状态。

- 所有权图：Group 以强 `GroupExprRef` 保存等价表达式；表达式以强 `GroupRef` 保存子 Group；表达式仅以 `Weak` 回指所属 Group。这打断了最直接的父级循环，但 Memo 子图仍可能因子 Group 关系长期存活，生命周期由最外层 Group/Memo 引用决定。
- 指纹格式：`u16(children.len())` 大端编码，之后是每个子 Group 的 `u64 ID` 大端编码，最后是逻辑算子 `HashCode`。孩子的数量和顺序都影响结果；同一 Group 内相同算子哈希与相同有序孩子身份被视为同一表达式。
- 指纹缓存：空 `Vec<u8>` 同时是“尚未计算”的哨兵。如果某个逻辑计划实现返回空哈希且表达式没有孩子，编码仍至少包含两字节孩子数量，所以正常指纹不会为空。
- 探索状态：`ExploreMark` 可区分轮次。`old/optimize.rs` 在每个规则批次轮次中用它结束重复探索，并在插入新等价式时重新驱动相应 Group。
- 规则状态：规则集合只保存稳定的 `u64` ID，不持有规则对象，既避免模块依赖环，也不延长规则生命周期。
- Schema：权威值位于所属 Group 的 `Prop.Schema`。`Schema()` 返回值克隆，使调用者不能通过该返回值直接改写 Group 属性。

关键不变量是：进入 Group 的表达式，其指纹所依赖的 `Children` 和 `ExprNode.HashCode()` 语义应保持稳定。虽然 Rust API 允许之后调用 `SetChildren` 或通过公开 `ExprNode` 修改算子，但 Group 的 `Fingerprints` 不会自动同步；这会使 `Exists`、`Delete` 或去重索引与表达式当前值不一致。Go 注释将孩子视为创建后不再改变，Rust 的安全使用也应遵守该逻辑不变量。

## 依赖与调用关系

crate 内依赖：

- `crate::{ExploreMark, Group, GroupRef}` 来自 `group.rs`；提供轮次标记、所属/子 Group 类型与稳定 Group ID。
- `lib.rs` 私有声明 `mod group_expr`，再以 `pub use group_expr::*` 暴露 `GroupExpr`、`GroupExprRef` 和 `NewGroupExpr`。
- `group.rs::Convert2GroupExpr` 是正常计划转换入口；`Group::{Insert, Delete, Exists, rebuild_indexes}` 是 `FingerPrint` 的直接消费者。
- `expr_iterator.rs` 读取 `GroupExpr.Children` 和 `ExprNode`，构建并推进 Pattern 匹配树。

外部 crate 依赖：

- `astersql-planner-core-operator-logicalop::{LogicalPlan, LogicalPlanRef}` 提供逻辑计划对象，以及 `HashCode`、Schema、孩子操作等行为。对应依赖在本 crate 的 `Cargo.toml` 中声明。
- `astersql-expression::Schema` 提供 Schema 值类型；同样由 `Cargo.toml` 的 `astersql-expression` 路径依赖引入。
- 标准库 `Rc`、`Weak`、`RefCell` 和 `HashSet` 分别承担共享所有权、非拥有回指、内部可变性和规则去重。

主要上游调用者包括 `group.rs::Convert2GroupExpr`、`pkg/planner/cascades/old/transformation_rules.rs::NewGroupExprWithChildren` 及各变换规则。运行主链上，`old/optimize.rs::exploreGroup` 读取/更新探索标记与孩子，`old/transformation_rules.rs::ExprView` 将规则对象转换为 `rule_id()` 后调用 `AddAppliedRule`/`HasAppliedRule`。测试及辅助构造也会直接调用公开构造器。

## 错误处理与边界

本文件没有显式错误类型或 `Result`。构造、状态查询和集合操作均按值返回；失败/缺失主要通过以下方式表达：

- `Schema()` 在表达式尚未插入 Group、所属 Group 已释放，或 Group 尚无 Schema 时返回 `None`，不会 panic。调用方若认为 Schema 必须存在，应在自己的边界把 `None` 转为错误或带上下文的断言；本文件不代替调用方建立该前置条件。
- `SetExplored`、`SetUnexplored` 对超出 `ExploreMark` 容量的轮次遵循 `ExploreMark` 的实现；相关测试 `group_test.rs::explore_mark_ignores_rounds_outside_fixed_capacity` 证明超界轮次不会成为已探索状态。
- `Children.len()` 在指纹中转换为 `u16`。超过 `u16::MAX` 时会截断计数，但仍追加所有孩子 ID；当前逻辑计划扇出远小于该边界，API 本身没有拒绝超大输入。
- `Rc<RefCell<_>>` 的重叠可变/不可变借用会在运行时 panic。调用路径应保持借用范围短；例如 `Group::Insert` 先释放表达式借用，再借 Group，最后回填弱引用。
- `FingerPrint` 依赖 `LogicalPlan::HashCode()` 的确定性和区分能力。哈希碰撞会被 Group 当作重复表达式；本文件不执行二次结构相等检查。
- `SetChildren` 会正确清空本地缓存，但不会通知已经包含该表达式的 Group 重建 `Fingerprints`。因此它是构造阶段 API，不应作为已入组表达式的通用重写操作。

## 并发与资源生命周期

`GroupExprRef` 使用 `Rc<RefCell<_>>` 而非 `Arc<Mutex<_>>`，所以 `GroupExpr` 设计为单线程优化器状态，不能跨线程发送或共享。规则探索也没有在本文件内创建线程、异步任务、通道或锁。

资源完全依赖 RAII：最后一个 `Rc` 释放时销毁表达式；`Group` 被释放后，`GroupExpr.Group.upgrade()` 失败，`Schema()` 返回 `None`。弱回指不会阻止所属 Group 被销毁。`FingerPrint()` 每次返回 `Vec<u8>` 克隆，因此调用者持有的键不借用表达式内部缓存；代价是每次查询都有与指纹长度成正比的复制。

`RefCell` 只解决单线程内部可变性，不提供并发互斥。扩展时若引入跨线程优化，不能只把外围引用换成 `Arc`；还需一起重新评估 Group、逻辑计划 trait 对象、规则集合和迭代器的 `Send`/`Sync` 能力及借用粒度。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/memo/group_expr.go`，对应测试为 `group_expr_test.go` 和 `group_expr_test.rs`。主要语义保持一致：都保存根逻辑计划、子 Group、所属 Group、探索标记、惰性指纹和已应用规则集合；构造器都从空孩子和未探索状态开始；指纹格式都是两字节大端孩子数、每个孩子八字节身份、最后追加计划哈希。

有三项明确差异：

1. Go 用 `reflect.ValueOf(child).Pointer()` 编码子 Group 指针地址；Rust 用 `Group::ID()` 的稳定自增 `u64`。两者都区分当前 Memo 中的子 Group 身份，但 Rust 字节值不应与同次 Go 进程逐字节比较。`group_expr_test.rs::group_expr_fingerprint_encodes_child_count_child_group_id_and_plan_hash` 专门验证 Rust 编码。
2. Go 的 `Group *Group` 可为 `nil` 但 `Schema()` 直接解引用，前置条件违反时会 panic；Rust 用 `Weak<RefCell<Group>>` 并返回 `Option<Schema>`，同时避免强引用环。调用方必须处理可空返回。
3. Go 的 `SetChildren` 没有清除 `selfFingerprint`，因为 Go 注释规定孩子创建后永不改变；Rust 主动清缓存，对构造辅助更稳健，但依然不能自动修复所属 Group 的外部索引。因此这不是对已入组表达式可随意变异的授权。

规则集合的参数形态也做了 Rust 化：Go 接收任意规则对象并以反射指针为 ID，Rust 直接接收由 `Transformation::rule_id()` 产生的 `u64`。这避免保存 trait 对象和导入循环，并使测试可以直接用确定的 ID 验证。

## 扩展指南

- 新增参与等价性的字段时，必须同步修改 `FingerPrint` 的编码，并扩展独立测试 `pkg/planner/memo/group_expr_test.rs`；还应检查 `group_test.rs` 中插入、删除、哈希碰撞和去重测试。编码字段要有确定顺序和无歧义长度，避免把指纹变成进程外持久格式。
- 改变孩子更新方式时，优先保持“入组后拓扑不可变”的不变量。若确需在 Group 内改孩子，应在 `group.rs` 提供原子重索引操作，而不是只调用 `SetChildren`，并覆盖旧键删除、新键冲突、`FirstExpr`/`Fingerprints` 一致性和失败回滚。
- 新增规则状态时，应继续保存稳定标识而非规则对象，防止依赖环和生命周期耦合；对应验证放在独立测试文件，不把测试模块嵌入生产源文件。
- 调整 Schema 行为时要同时检查 `Group::Insert` 的弱引用回填、`Group::Delete` 的清理、`BuildKeyInfo` 对 `Group.Prop.Schema` 的更新，以及规则中对 `ExprView::Schema` 或 `GroupExpr::Schema` 的调用。明确缺失 Schema 是返回 `None`、传播错误还是断言，不要静默改成空 Schema。
- 扩大探索轮次数或改变轮次语义时，应修改 `ExploreMark` 的定义与测试，而不是只改本文件的委托方法；同步检查 `old/optimize.rs::onPhaseExploration/exploreGroup`。
- 性能修改应关注 `FingerPrint` 返回克隆、频繁 `RefCell` 借用以及 `HashSet<u64>` 查询。任何减少复制的 API 都必须明确借用期，避免在 Group 可变操作期间持有表达式内部缓冲区引用。
- 修改公开 API 后需核对 `lib.rs` 的再导出以及 `Cargo.toml` 的 crate 边界；Go 对齐修改还需同步阅读 `group_expr.go` 和 `group_expr_test.go`，不得为了 Rust 测试便利删减 Go 行为。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/memo` 列出目标 Rust/Go 源及独立测试，`node --file pkg/planner/memo/group_expr.rs` 展示目标文件 1--113 行及其 11 个符号。
- RustCodeGraph 符号/调用证据：`query GroupExpr`、`query NewGroupExpr`、`query FingerPrint` 定位 Rust/Go 对应符号；索引 flow 显示 `group.rs::Convert2GroupExpr -> group_expr.rs::NewGroupExpr`，并列出 `group.rs`、`expr_iterator_test.rs`、`group_expr_test.rs` 和旧 Cascades 文件等使用者。
- 已核对生产源码：`pkg/planner/memo/group_expr.rs`、`group.rs`、`expr_iterator.rs`、`lib.rs`、`pkg/planner/cascades/old/optimize.rs`、`pkg/planner/cascades/old/transformation_rules.rs`。
- 已核对 crate 配置：`pkg/planner/memo/Cargo.toml`，确认 crate 名、Go 包映射及 `astersql-expression`、`astersql-planner-core-operator-logicalop` 等直接依赖。
- 已核对 Go 对照：`pkg/planner/memo/group_expr.go`、`group_expr_test.go`，确认字段、构造、指纹布局、Schema 和规则去重语义。
- 已核对 Rust 独立测试：`pkg/planner/memo/group_expr_test.rs` 验证初始状态与指纹布局；`memo_aster_unit_test.rs::fingerprint_uses_child_group_identity_and_rule_ids` 验证子 Group 身份和规则 ID 集合；`group_test.rs` 覆盖 Group 插入/删除/去重、探索轮次和指纹边界；`expr_iterator_test.rs` 覆盖以 `Children` 进行 Pattern 枚举。
- 本任务为纯文档分析，按计划未运行 Cargo 或代码测试。交付前执行任务指定的 11 章结构验证，并人工复核本文只陈述上述源码、图索引、Cargo、Go 对照和测试可支持的事实。
