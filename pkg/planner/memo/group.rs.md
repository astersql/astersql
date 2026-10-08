# `pkg/planner/memo/group.rs`

## 文件定位

本文件是 `astersql-planner-memo` crate 的等价类容器实现。crate 根 `pkg/planner/memo/lib.rs` 将 `group` 模块的公开项全部再导出；`pkg/planner/memo/Cargo.toml` 通过 `package.metadata.porting.go-package = "pkg/planner/memo"` 明确它移植自同路径 Go 包。它位于旧 Cascades 优化器的核心数据通路上：`pkg/planner/cascades/old/optimize.rs::FindBestPlan` 在预处理逻辑计划后调用 `Convert2Group` 建立 Memo，探索阶段通过 `Group::Insert`、`Group::Delete`、`Group::DeleteAll` 改写等价表达式，物理实现阶段通过 `GetImpl`、`InsertImpl` 复用已求得的最优实现。

这里的 `Group` 不是普通计划树节点，而是“逻辑语义等价表达式”的集合。集合元素由相邻文件 `pkg/planner/memo/group_expr.rs::GroupExpr` 表示；每个表达式保存逻辑算子和若干子 `Group`，因此普通的 `LogicalPlan` 树经 `Convert2Group` 后变为适合规则枚举和动态规划的 Memo 图。

## 核心职责

- 用 `Group` 保存同一逻辑语义下的所有 `GroupExprRef`，并维护按表达式指纹去重的 `Fingerprints` 索引。
- 用 `FirstExpr` 记录每种 `pattern::Operand` 的首个表达式，使模式迭代器能快速定位某类算子；`Equivalents` 中同 Operand 表达式保持连续。
- 用 `ExploreMark` 的 64 位位图分别记录不同规则批次/轮次是否已经探索，支持新增表达式后重新打开某一轮探索。
- 用 `ImplMap` 按完整 `PhysicalProperty::HashCode` 缓存最优 `ImplementationRef`，供 `old/optimize.rs::implGroup` 的动态规划剪枝复用。
- 将普通逻辑计划树递归转换成 `GroupExpr -> child Group` 结构，并延迟、幂等地推导 `LogicalProperty` 中的键和 `MaxOneRow` 信息。

本文件不负责规则匹配、统计信息推导或物理计划代价计算；这些分别由表达式迭代器、`old/optimize.rs::fillGroupStats` 和实现规则完成。

## 主要符号

- `ExploreMark(pub u64)`：每一位对应一个探索轮次。方法及同名自由函数 `SetExplored`、`SetUnexplored`、`Explored` 提供置位、清位和查询；`round >= 64` 时写操作为 no-op、查询返回 `false`。
- `GroupRef = Rc<RefCell<Group>>`：单线程共享可变句柄。调用者通过 `Rc` 共享所有权，通过 `RefCell` 在运行时检查借用规则。
- `NEXT_GROUP_ID: AtomicU64`：从 1 开始分配稳定 ID。`NewGroupWithSchema` 使用 `Ordering::Relaxed` 取号；ID 只要求唯一，不承载跨线程同步顺序。
- `Group`：核心状态包括 `Equivalents`、`FirstExpr`、`Fingerprints`、`ImplMap`、逻辑属性 `Prop`、`EngineType`、自指纹缓存 `SelfFingerprint`、探索位图和私有的一次性标记 `hasBuiltKeyInfo`。
- `NewGroupWithSchema(expression, schema)`：复制输入 Schema 的 `Columns` 建立新逻辑属性，默认引擎为 `EngineTiDB`，可选插入首个表达式。它不会复制输入 Schema 的 `PKOrUK` 等派生信息。
- `Group::ID`、`Group::FingerPrint`：分别返回稳定数值 ID和其大端字节表示；后者首次访问时填充 `SelfFingerprint` 缓存。
- `Group::Insert`、`Delete`、`DeleteAll`、`Exists`、`GetFirstElem`：维护等价表达式集合及两个派生索引。`rebuild_indexes` 是增删后恢复索引不变量的私有入口。
- `Group::GetImpl`、`InsertImpl`：以物理属性哈希为键读取或覆盖最佳物理实现。
- `Convert2GroupExpr`、`Convert2Group`：把 `LogicalPlanRef` 的子计划取出并递归包装为子 Group，再用当前节点 Schema 创建父 Group。
- `BuildKeyInfo`：每个 Group 至多执行一次的自底向上属性推导。
- `inherits_max_one_row`：补充逻辑算子自身 `MaxOneRow()` 之外的子节点继承规则；它是文件内私有函数。

## 执行流程

1. `old/optimize.rs::FindBestPlan` 先完成列裁剪，再把逻辑根节点交给 `Convert2Group`。
2. `Convert2Group` 在拆树前克隆当前节点 Schema；`Convert2GroupExpr` 调用 `TakeChildren` 取得并清空普通计划节点的孩子，对每个孩子递归调用 `Convert2Group`，再通过 `GroupExpr::SetChildren` 接到新表达式上。最终 `NewGroupWithSchema` 创建等价类并插入该表达式。
3. 插入时，`Group::Insert` 先计算 `GroupExpr::FingerPrint` 和 Operand。若 `Fingerprints` 已包含该字节串则返回 `false`；否则插到同 Operand 首项之后，或在没有同类时追加到末尾，然后完整重建 `FirstExpr` 与 `Fingerprints`，最后把表达式的 `Group` 弱引用回填为当前 Group。
4. 探索阶段 `old/optimize.rs::exploreGroup/findMoreEquiv` 枚举当前快照，对规则生成的新表达式调用 `Insert`。插入成功后清除当前轮的 Group 探索位，外层循环因此会再次遍历新候选；规则要求淘汰旧表达式或全量替换时分别调用 `Delete` 或 `DeleteAll`。
5. 规则需要唯一键或最多一行信息时，经 `transformation_rules.rs::GroupHandle::BuildKeyInfo` 调入本文件。`BuildKeyInfo` 先设置一次性标志，再取第一条等价表达式，递归构建所有子 Group；单孩子时先继承其 `PKOrUK`，随后把 Schema 写回逻辑算子并调用算子自身的 `BuildKeyInfo`，最后将算子 Schema 和 `MaxOneRow` 结果写回 Group。
6. 物理实现阶段 `old/optimize.rs::implGroup` 先以所需 `PhysicalProperty` 查询 `GetImpl`；未命中时枚举实现规则、递归实现孩子、比较成本，并用 `InsertImpl` 缓存最终最佳实现。

## 数据与状态

`Equivalents` 是状态真源，另外两个集合索引必须与它一致：每个表达式指纹映射到自身下标，`FirstExpr[operand]` 映射到该 Operand 在向量中的第一个下标。新增同类表达式固定插在首项之后，所以首项保持不变、同类范围保持连续，但第二个之后的同类表达式相对插入顺序会反转。删除后 `rebuild_indexes` 以当前向量重新计算全部下标，避免 `Vec::insert/remove` 引起索引漂移。

表达式指纹由 `group_expr.rs::GroupExpr::FingerPrint` 编码：子 Group 数量、各子 Group 的稳定 ID以及逻辑算子的 `HashCode`。因此相同算子连接不同子 Group 时不是同一候选；相同指纹则被视为重复。Group 自身的指纹只由全局 ID 生成，与 Group 当前包含哪些表达式无关；`DeleteAll` 虽会清空其缓存，但下一次仍会由同一 ID 得到相同值。

`Prop` 在构造时只有复制后的列 Schema；`Stats` 由优化器探索后的 `fillGroupStats` 另行填充，`PKOrUK` 和 `MaxOneRow` 由 `BuildKeyInfo` 推导。`EngineType` 默认是 TiDB，可通过 `SetEngineType` 链式改写。`ImplMap` 与逻辑表达式索引相互独立，当前增删逻辑表达式不会自动清理已经缓存的实现，调用者必须保证只在适当的优化阶段使用缓存。

## 依赖与调用关系

直接依赖由 `pkg/planner/memo/Cargo.toml` 声明：`astersql-expression` 提供 `Schema/NewSchema`，`astersql-planner-cascades-pattern` 提供 Operand 与执行引擎分类，逻辑算子 crate 提供 `LogicalPlan`、`LogicalJoin`、`LogicalMaxOneRow` 等，property crate 提供逻辑/物理属性。`crate::{GroupExprRef, ImplementationRef, NewGroupExpr}` 来自同 crate 的相邻模块再导出。

主要上游是 `pkg/planner/cascades/old/optimize.rs`：`FindBestPlan -> Convert2Group` 建 Memo，`exploreGroup/findMoreEquiv -> Insert/Delete/DeleteAll` 改写 Memo，`implGroup -> GetImpl/InsertImpl` 使用实现缓存。`pkg/planner/cascades/old/transformation_rules.rs` 将本 crate API 包装为兼容句柄，并在需要键属性的规则中调用 `BuildKeyInfo`；大量转换规则也使用 `NewGroupWithSchema` 和 `Convert2GroupExpr` 构造局部替换图。`pkg/planner/memo/expr_iterator.rs` 则消费 `GetFirstElem` 和连续的 Operand 区间完成 Pattern 绑定。

RustCodeGraph 对目标文件给出的明确下游边包括 `Convert2GroupExpr -> group_expr.rs::NewGroupExpr`；源码中的其余直接边为 `Convert2GroupExpr -> Convert2Group`、`Convert2Group -> NewGroupWithSchema`、`NewGroupWithSchema -> Group::Insert`、`BuildKeyInfo -> LogicalPlan::BuildKeyInfo` 和递归的 `BuildKeyInfo -> BuildKeyInfo`。图查询对若干同名方法未返回稳定的精确 caller 集，因此上游调用位置又用限定在 `pkg/planner/**/*.rs` 的文本搜索核验，没有把同名的其他 `Group` 实现混入结论。

## 错误处理与边界

本文件 API 不返回 `Result`，集合层面的“失败”以布尔值或空值表达：重复插入返回 `false`，删除不存在的指纹直接返回，找不到首项或实现分别返回 `None`。`BuildKeyInfo` 对空 Group 不报错：它先把 `hasBuiltKeyInfo` 置为 `true`，随后因没有首表达式而返回，之后再插入表达式也不会自动重新推导；因此空 Group 的创建和后续填充顺序需要由调用者控制。

`ExploreMark` 显式检查轮次小于 `u64::BITS`，避免越界移位；这也意味着最多记录 64 个独立轮次。`GroupExpr::FingerPrint` 将孩子数量编码为 `u16`，本文件不会拒绝超过 `u16::MAX` 个孩子；正常逻辑算子远低于此界限，但构造异常自定义节点时不能把该编码当作无限容量。

`RefCell` 借用冲突会在运行时 panic。实现通过缩小借用块和显式 `drop(current)`，确保回填表达式归属前不持有 Group 的可变借用；扩展方法时也必须避免持有一个 `borrow_mut()` 又经表达式或孩子回到同一 Group。`BuildKeyInfo` 在开始递归前即设置完成标志，可阻断异常循环，但如果下游算子推导发生 panic，标志不会回滚。

## 并发与资源生命周期

`GroupRef`、`GroupExprRef` 使用 `Rc<RefCell<_>>`，不是 `Send`/`Sync`，设计目标是单线程优化器内部共享，不可直接跨线程传递。`NEXT_GROUP_ID` 使用原子计数仅保证即使构造路径并发也不会重复分配 ID；`Relaxed` 顺序足够，因为该原子不发布其他内存状态。

所有权方向是 Group 以强 `Rc` 持有表达式、表达式以强 `Rc` 持有子 Group，而表达式通过 `Weak<RefCell<Group>>` 指回所属 Group，避免父 Group 与成员表达式形成强引用环。`Delete` 成功时只清空调用参数表达式的弱归属；若传入的是“与已存对象同指纹但非同一 Rc”的对象，被实际移除的原对象仍保留旧弱引用，这是 Go 行为的刻意对齐，`group_test.rs::group_delete_with_equivalent_expression_preserves_removed_expression_owner` 已固定该边界。`DeleteAll` 同样不逐个清理旧表达式的弱归属。

`ImplementationRef` 也通过共享引用保存在 `ImplMap` 中，缓存被覆盖或 Group 整体释放时引用计数自然下降。本文件不创建线程、异步任务、锁、通道、事务或外部 I/O 资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/memo/group.go`。字段和主流程基本一一对应：Go 的 `container/list.List` 在 Rust 中改为 `Vec<GroupExprRef>`，指向链表元素的两个 map 改为“指纹/Operand -> 下标”，因此 Rust 每次增删后调用 `rebuild_indexes`；这一实现保留了“同 Operand 连续、首项稳定”的可观察顺序。Go 指纹键是 `string`，Rust 使用原始 `Vec<u8>`；Go Group 自指纹使用对象地址字符串，Rust 改为原子分配 ID 的大端字节，从而不依赖地址格式。

Go 的 `NewGroupWithSchema` 接受可能为 `nil` 的表达式并由 `Insert` 忽略 nil；Rust 用 `impl Into<Option<GroupExprRef>>` 显式表达可空输入。Go 的探索位图依赖机器字宽和移位语义，Rust 固定为 `u64` 并对越界轮次做 no-op。Go `BuildKeyInfo` 假定 Group 至少有首表达式并直接取 `Front().Value`，Rust 对空 Group 安全返回；非空路径仍保持先递归孩子、单孩子继承键、调用逻辑算子推导、合并 `MaxOneRow` 的语义。

Rust 将 Go 的 `logicalop.HasMaxOneRow` 行为局部实现为 `inherits_max_one_row`：Lock、Limit、Sort、Selection、Apply、Projection、Window、Aggregation 继承第一个孩子；半连接类只看左孩子；普通 Join 要求恰有两个孩子且两侧都最多一行；其他 Operand 不继承。最终值仍是 `ExprNode.MaxOneRow() || inherited`。`pkg/planner/memo/group_test.rs` 以手工计划树覆盖这些规则，因为仓库当前没有可编译的 Rust `BuildLogicalPlanForTest` 全链路；Go 测试 `group_test.go` 则通过真实 SQL 解析和计划构建验证对应场景。

## 扩展指南

- 新增集合修改操作时，应把 `Equivalents` 视为唯一真源，并在修改后同步重建或精确维护 `FirstExpr`、`Fingerprints`；必须保持同 Operand 连续，否则 `ExprIter` 的范围枚举会失效。
- 修改指纹格式时需要同时检查 `group_expr.rs::FingerPrint`、Group ID稳定性和重复消除语义，并在独立的 `pkg/planner/memo/group_test.rs` 或 `group_expr_test.rs` 增加“相同算子/相同孩子去重、不同孩子不去重”的回归测试。
- 新增会传播 `MaxOneRow` 的逻辑算子时，先对照 Go `logicalop.HasMaxOneRow`，再更新 `inherits_max_one_row` 及 `group_test.rs::build_key_info_inherits_pk_and_max_one_row_per_operand_rules`；不能仅为通过测试泛化为所有单输入算子继承。
- 改动探索轮次表示时，需要同步 `Group` 与 `GroupExpr` 共用的 `ExploreMark`，并核验 `old/optimize.rs` 中“插入成功即 `SetUnexplored(round)`”的重访协议。
- 改动实现缓存键时，必须继续使用能够区分完整物理需求的哈希；`memo_aster_unit_test.rs::implementation_cache_keys_by_full_physical_property_hash` 已验证 `ExpectedCnt` 不同不应命中同一缓存。
- 如要支持并行优化，不能只替换全局 ID 原子顺序；必须整体重做 `Rc<RefCell<_>>` 所有权、动态借用和表达式弱反向引用。当前类型边界明确限定在单线程。
- 测试逻辑应继续放在独立的 `group_test.rs`/`memo_aster_unit_test.rs`，不要内嵌到生产源文件。修改 Go 移植语义时还应同步核对 `pkg/planner/memo/group_test.go`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/memo` 确认目标目录的 Rust/Go 源与测试均在索引中。
- RustCodeGraph 源码与关系查询：读取 `pkg/planner/memo/group.rs` 全部 361 行；`explore` 确认 `Convert2GroupExpr -> NewGroupExpr`，并显示目标文件被旧 Cascades 优化器/字符串化及 Memo 测试使用；对精确 `Convert2GroupExpr`、`Convert2Group`、`NewGroupWithSchema` 节点执行了 `callers`/`callees` 查询。因同名符号消歧下部分查询未输出边，又以限定 Rust 路径的 `rg` 结果核对调用点。
- 读取的直接实现证据：`pkg/planner/memo/group_expr.rs`（指纹、子 Group、弱反向引用），`pkg/planner/memo/lib.rs`（模块装配与再导出），`pkg/planner/cascades/old/optimize.rs`（建 Memo、探索改写、实现缓存主链），`pkg/planner/cascades/old/transformation_rules.rs` 的命中位置（属性推导和新 Group 构造）。
- 读取的边界与依赖证据：`pkg/planner/memo/Cargo.toml`，Go 对照 `pkg/planner/memo/group.go`，Go 测试 `pkg/planner/memo/group_test.go`，Rust 独立测试 `pkg/planner/memo/group_test.rs` 和 `pkg/planner/memo/memo_aster_unit_test.rs`。
- 测试覆盖事实：Rust 测试覆盖新建/去重/删除幂等、同指纹不同对象删除、DeleteAll、Operand 首项、物理属性缓存、计划树拆分、键与 `MaxOneRow` 传播、重复 `BuildKeyInfo` 和越界探索轮次。本任务是纯文档分析，按计划不运行 Cargo。
