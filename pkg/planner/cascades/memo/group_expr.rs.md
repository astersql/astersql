# [`pkg/planner/cascades/memo/group_expr.rs`](group_expr.rs)

## 文件定位

本文件属于 Cargo 包 `astersql-planner-cascades-memo`，模块入口是同目录的 `lib.rs`，并由该入口公开再导出 `GroupExpression`、`GroupExpressionRef`、`hash_logical_plan` 和 `equal_logical_plans`。`Cargo.toml` 表明它直接依赖 `cascades-base`、`logicalop`、`property` 等本地 crate；其中 `cascades-base` 提供 Go `HashEqualer` 对应的哈希接口，`logicalop` 提供逻辑算子及 `LogicalPlan` trait，`property` 提供组级逻辑属性。

它位于 Cascades Memo 的核心边界：`Memo` 把一棵逻辑计划拆成若干等价类 `Group`，本文件中的 `GroupExpression` 则表示“一项逻辑算子 + 按位置排列的子 Group”。因此它既是 Memo 去重、合组和规则探索的身份单元，也是规划器在不恢复完整计划树时读取子组 schema、统计信息和逻辑属性的适配器。直接依据是 `group_expr.rs::GroupExpression`、`memo.rs::CopyIn`、`group.rs::Insert` 和 `task/task_apply_rule.rs::ApplyRuleTask::Execute`。

## 核心职责

1. 表示 Memo 中的一条等价表达式：`LogicalPlan` 保存当前算子，`Inputs` 保存算子的子等价类，而不是保存原逻辑计划的孩子节点。
2. 建立稳定身份：`Hash64` 把逻辑算子语义哈希与有序的子 `GroupID` 组合，`Equals` 再比较算子语义及每个位置的子 Group；哈希只用于候选筛选，冲突仍由相等判断区分。
3. 保存探索生命周期状态：`mask` 记录已应用规则编号，`abandoned` 让已经被替代或移除但仍被任务引用的表达式可以被安全跳过。
4. 提供组级逻辑信息：`GetInputSchema`、`GetChildStatsAndSchema`、`GetJoinChildStatsAndSchema` 从子 Group 读取属性，`DeriveLogicalProp` 把当前算子的 schema、stats、FD、单行性质、可能排序和 TiFlash 可用性写入所属 Group。
5. 维护 Memo 图的资源关系：表达式强持有子 Group，弱引用所属 Group；`addr` 为子 Group 的父表达式表提供进程内身份键，`mergeTo` 在表达式去重时合并探索状态并解除父回边。
6. 作为 `LogicalPlan` 门面把关键方法转发给包装算子，避免经 Memo 表达式调用时落入不合适的基础实现。

## 主要符号

- `pub type GroupExpressionRef = Rc<RefCell<GroupExpression>>`：单线程共享、内部可变的表达式句柄。`Rc` 支持 Group、Memo 和任务共同持有，`RefCell` 把借用检查移到运行时。
- `pub struct GroupExpression`：公开字段 `LogicalPlan: LogicalPlanRef` 与 `Inputs: Vec<GroupRef>` 构成表达式内容；crate 内字段 `group`、`hash64`、`mask`、`abandoned` 分别保存归属弱引用、缓存哈希、规则集合和废弃标志。
- `new(plan, inputs)`：构造未归组的表达式，初始化空状态并立刻调用 `Init` 缓存非零哈希。外部构造入口是 `Memo::NewGroupExpression`。
- `GetGroup`、`String`、`GetWrappedLogicalPlan`、`InputsLen`：分别提供弱引用升级、调试文本、包装算子借用和输入数量。
- `Hash64`、`GetHash64`、`Equals`、`Init`：实现表达式身份。`Init` 使用 `cascades_base::NewHashEqualer`，并把意外的零结果规范为 `1`；当 `Memo::replaceGEChild` 改写输入后会重新调用它。
- `hash_logical_plan`、`equal_logical_plans`：按已列举的具体逻辑算子类型调用各自 `Hash64`/`Equals`；未列举类型分别回退到 `TP + HashCode + ExplainInfo + schema 列 ID` 的 FNV-1a 哈希，以及 `TP + HashCode + ExplainInfo + Schema::Equal` 比较。
- `Fnv64`：文件私有的 64 位 FNV-1a 实现，是逻辑算子语义哈希分派的稳定字节累加器；表达式外层仍由 `cascades-base` 的 hasher 组合算子哈希与子 GroupID。
- `IsExplored`、`SetExplored`：查询或写入 `BTreeSet<usize>` 规则编号；`ApplyRuleTask::Execute` 在调度前查询、结束后写入。
- `IsAbandoned`、`SetAbandoned`：表达式被移除或重写冲突后标记过期；规则任务看到该标志会直接返回。
- `GetInputSchema`、`GetChildStatsAndSchema`、`GetJoinChildStatsAndSchema`：从子 Group 的 `LogicalProperty` 返回克隆值；前两类访问器分别要求有效索引、单孩子非 Join/Apply、双孩子 Join/Apply。
- `DeriveLogicalProp`：仅在所属 Group 尚无属性时运行；要求所有孩子属性已存在，汇集孩子的 `PossibleProps` 和 `HasTiFlash`，再从包装算子取得 schema、stats、FD、`MaxOneRow` 和可能属性并写入 owner。
- `addr`、`mergeTo`：前者以 `Rc::as_ptr` 产生父引用表键；后者把 source 的探索规则并入 target，逐一调用 `Group::removeParentGEs`，最后清空 source 输入和 owner 弱引用。
- `impl LogicalPlan for GroupExpression`：转发 `base/base_mut`、解释、哈希码、谓词下推、列裁剪、键构建、TopN 下推与统计派生；`as_any` 保留动态类型识别能力。

## 执行流程

初始化主链以 `Memo::Init -> Memo::CopyIn` 开始。`CopyIn` 先通过 `derive_tree` 自底向上调用原逻辑树的 `DeriveStats(true)`，再 `TakeChildren` 拆出孩子；每个孩子递归拷入后，其 owner Group 被收集为当前表达式的 `Inputs`。`Memo::NewGroupExpression` 调用本文件的 `new`，构造时 `Init -> Hash64 -> hash_logical_plan` 计算并缓存身份哈希。

随后 `Memo::InsertGroupExpression` 先以 `GetHash64 + Equals` 在全局表达式集合中去重。新表达式由 `Group::Insert` 设置 `group` 弱回指并加入组内索引，之后 Memo 对每个输入调用 `Group::addParentGEs` 登记父表达式弱引用。只有“确实新插入且没有指定目标 Group”时，`CopyIn`/`CopyInWithGroupChildren` 才调用 `DeriveLogicalProp`：孩子已在递归中完成，所以派生无需递归；已有 owner 属性时直接返回，避免等价表达式覆盖组级结果。

探索阶段，`ApplyRuleTask::Execute` 先检查 `IsExplored(rule_id)` 与 `IsAbandoned()`；通过后绑定规则、插入变换结果，最后 `SetExplored(rule_id)`。如果规则要求替换原表达式，`Memo::RemoveOut` 从 Group 和全局集合移除它、删除所有孩子的父回边，并设置 abandoned，保证已排队的其他任务不会继续使用过时表达式。

合组或孩子替换时，`Memo::mergeGroup` 先改写父表达式的输入，然后由 `Memo::replaceGEChild` 维护旧/新 Group 的父回边并重新 `Init` 哈希。改写后若产生等价表达式，`GroupExpression::mergeTo` 把 source 的探索 mask 合并给 target，删除 source 在各孩子上的父引用，并断开 source 的输入及 owner。`Group::mergeTo` 在迁移组内表达式时也沿用这一流程。

## 数据与状态

表达式身份由两部分构成：包装算子的当前语义，以及 `Inputs` 中按顺序排列的 GroupID。所属 owner Group 不参与哈希或相等判断，因此同一表达式插入不同 Group 时可触发全局去重和合组；子输入顺序参与身份，所以交换 Join 两侧在该层仍是不同表达式，等价交换应由规则显式产生。`group_and_expr_test.rs::TestGroupExpressionHashEquals` 与 `TestGroupExpressionHashCollision` 分别验证顺序敏感和“同哈希不等价仍可共存”。

`hash64` 是缓存而不是实时视图。任何会改变 `LogicalPlan` 身份字段或 `Inputs` 的代码都必须在重新进入索引前调用 `Init`；现有直接改写入口 `Memo::replaceGEChild` 已这样做。`GetHash64` 断言缓存非零，`new` 保证初始值，`Init` 对未来可能返回零的 hasher 做兜底。

逻辑属性存放在 owner `Group`，不是表达式自身。`DeriveLogicalProp` 写入 `Schema`、可选 `Stats`、始终为 `Some` 的 FD、`MaxOneRow`、`PossibleProps` 与 `HasTiFlash`。非叶子节点的 `PossibleProps` 当前继承第一个孩子；TiFlash 信息交给包装算子的 `PreparePossibleProperties`，叶子则读取 `PreparePossiblePropertiesValue`。这是一项当前实现规则，不应泛化为所有未来算子的最终排序推导语义。

`mask` 使用有序集合，合并是集合并集；`abandoned` 只从 `false` 单向变为 `true`。两者服务任务调度，不参与表达式身份。`String` 同样只用于诊断，输出算子 `TP` 和各输入 GroupID，不应作为稳定序列化格式。

## 依赖与调用关系

上游直接调用关系如下：

- `memo.rs::Memo::NewGroupExpression -> GroupExpression::new`；`Memo::CopyIn` 与 `CopyInWithGroupChildren -> GroupExpression::DeriveLogicalProp`。
- `memo.rs::Memo::RemoveOut`、`Memo::mergeGroup -> SetAbandoned`；`Memo::mergeGroup` 和 `group.rs::Group::mergeTo -> GroupExpression::mergeTo`；`Memo::replaceGEChild -> Init`。
- `group.rs::Group::addParentGEs/removeParentGEs` 与测试通过 `GroupExpression::addr` 维护或核验父引用表。
- `task/task_apply_rule.rs::ApplyRuleTask::Execute -> IsExplored/IsAbandoned/SetExplored`，将本文件状态接入 Cascades 规则调度。
- `group_expr_test.rs` 直接调用两个孩子属性访问器验证类型边界；`group_and_expr_test.rs` 通过 Memo/Group 公共链路验证身份、父回边和属性派生。

主要下游依赖是 `logicalop::LogicalPlan` 及其具体算子类型、`property::LogicalProperty/StatsInfo`、`cascades_base::Hasher`，以及同 crate 的 `Group`。Cargo 清单没有 feature 条件，本文件也没有 `cfg` 分支；仅测试模块在 `lib.rs` 中受 `#[cfg(test)]` 控制。

RustCodeGraph 的文件节点确认本文件共 451 行，并识别出 `GroupExpression`、`DeriveLogicalProp`、`mergeTo` 等符号；但本地索引对这些 associated function 的 `callers/callees` 消歧不完整，因此上述调用边由精确符号搜索和命中文件源码复核，不把图工具的空结果解释为“无调用者”。

## 错误处理与边界

本文件自身没有返回业务错误的构造、哈希、比较或属性派生入口；它用断言/`expect` 表达 Memo 内部不变量。`GetHash64` 要求已初始化；`GetInputSchema` 要求索引存在且孩子具有 schema；`GetChildStatsAndSchema` 要求包装算子不是 Join/Apply 且输入恰为一个；`GetJoinChildStatsAndSchema` 要求是 Join/Apply 且输入恰为两个。`group_expr_test.rs` 的两个 `#[should_panic]` 用例验证算子类型用错时会失败。

`DeriveLogicalProp` 要求表达式已经插入仍存活的 owner Group，且所有子 Group 已有逻辑属性，否则 `expect` 失败。它允许孩子属性整体存在但其中 `Stats` 或 `Schema` 为 `None`，相应访问器会返回 `None`；不过 `GetInputSchema` 对缺失 schema 更严格。调用方应维持 `CopyIn` 的自底向上顺序，不应把该函数当作可处理任意半初始化图的容错 API。

`RefCell` 的嵌套可变借用冲突会在运行时 panic。当前实现通过先克隆 mask、输入或属性数据，再获取 `borrow_mut`，避免在 `mergeTo` 和 `DeriveLogicalProp` 的关键路径重叠借用。`mergeTo` 又依赖每个孩子确实登记过 source 父引用；缺失时 `Group::removeParentGEs` 断言失败，这能暴露图维护错误而非静默留下悬挂索引。

未知逻辑算子不会被拒绝，而是进入哈希/相等的回退路径。该路径是否足以区分新算子的全部语义字段必须由新增算子的实现者验证；若 `HashCode`、`ExplainInfo` 和 schema 都未编码某个影响语义的字段，可能出现错误去重。

## 并发与资源生命周期

`Rc<RefCell<_>>`、`Weak` 和非线程安全的 `LogicalPlanRef` 表明本结构设计为单线程 Memo/任务调度，不实现跨线程共享。若未来把优化任务并行化，不能直接把这些句柄发送到线程；需要重新设计所有权、锁粒度和计划 trait 的线程安全约束。

所有权图刻意避免强引用环：Group 强持有其表达式，表达式强持有子 Group，表达式到 owner Group 是 `Weak`；子 Group 的 `parentExpressions` 也保存父表达式 `Weak`。因此 owner 删除或合并后，`GetGroup` 可以返回 `None`，而弱回边不会阻止对象释放。`addr` 只在对应 `Rc` 存活期间作为内存身份使用，不能持久化、跨进程比较或在对象释放后复用。

正常生命周期为：`new` 创建未归组表达式，`Group::Insert` 建立 owner，`Memo::InsertGroupExpression` 建立孩子到父表达式的弱回边；删除时 `RemoveOut` 或合并路径先清索引/回边，再设置 abandoned 或清空输入和 owner。`Group::Clear`、`Memo::Destroy` 也会断开 owner 并释放集合。已排队任务可能仍通过 `Rc` 保持表达式对象存活，因此 abandoned 是逻辑过期标记，而不是立即析构信号。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/cascades/memo/group_expr.go`。字段语义基本逐项对应：Go 的 `base.LogicalPlan` 嵌入字段对应 Rust 的 `LogicalPlanRef + impl LogicalPlan`，`[]*Group` 对应 `Vec<GroupRef>`，bitset mask 对应 `BTreeSet<usize>`，布尔 abandoned 保持一致。Go 依赖垃圾回收处理指针图；Rust 显式采用 owner/parent 弱回边，避免 `Rc` 环，并用 `RefCell` 支持原 Go 指针对象的共享可变行为。

哈希与相等的目标语义一致：都比较算子语义和有序子 Group，owner 不参与。实现机制不同：Go 直接调用每个逻辑计划的 `Hash64`/`Equals` 并用 operand 先判型；Rust 的 trait 当前没有统一提供相同接口，所以 `hash_logical_plan`、`equal_logical_plans` 显式枚举已支持算子并为未知算子提供回退。新增算子时必须审查这一分派表，这是 Rust 独有的同步点。

属性派生流程也有结构差异。Go `DeriveLogicalProp` 在函数内收集 child stats/schema，调用 `DeriveStats`、`ExtractFD` 和 `PreparePossibleProperties`，并可通过测试 failpoint 跳过；Rust `Memo::CopyIn` 在拆孩子前先以 `derive_tree` 对完整树执行统计派生，本文件随后读取包装算子缓存的 stats/schema/FD 并组装 Group 属性。Rust 还显式写入 `MaxOneRow`。因此不能仅按 Go 函数体逐行修改 Rust，必须同时检查 `memo.rs::derive_tree/CopyIn` 的前置工作。

Go `mergeTo` 自己先从 owner Group 删除 source；Rust 调用点已在 `Group::mergeTo` 或 `Memo::mergeGroup` 中完成相应删除，本函数只合并 mask、清父回边和断开 source。Go 子属性访问器返回内部指针，Rust 为避免借用越界返回克隆的 `StatsInfo`/`Schema`。Go 后半文件还包含物理计划枚举与最佳任务逻辑；这些职责在当前 Rust 架构中不属于本文件，不能据此声称 Rust 此处已移植那些函数。

## 扩展指南

新增逻辑算子时，首先判断其全部语义字段是否已被通用回退中的 `HashCode`、`ExplainInfo` 和 schema 覆盖。若不能证明覆盖，应同时在 `hash_logical_plan` 与 `equal_logical_plans` 的同一位置增加具体类型分派，并在独立测试文件中加入“相同语义相等/不同关键字段不等/哈希一致性”用例；两张分派表遗漏任一侧都会破坏哈希表等价契约。

修改表达式身份字段或 `Inputs` 时，必须在重新加入 Group/全局索引前调用 `Init`，并像 `Memo::replaceGEChild` 一样同步维护 `Group::removeParentGEs/addParentGEs`。不要直接清理或替换输入而遗漏父回边，也不要把 owner Group 纳入身份，否则跨 Group 去重和合组语义会改变。

增加规则状态时，应保持合并语义明确：探索完成集合需要在 `mergeTo` 中并集，过期状态应与 `ApplyRuleTask` 的跳过条件共同更新。增加逻辑属性字段时，应同步检查 `DeriveLogicalProp`、`property::LogicalProperty`、叶子/非叶子的派生来源，以及 Go `GroupExpression.DeriveLogicalProp`；不能用默认值掩盖尚未移植的派生。

测试必须继续放在独立文件。身份、父回边、派生与合并的综合用例宜扩展 `group_and_expr_test.rs` 或 `memo_test.rs`；孩子访问器的类型/数量边界宜扩展 `group_expr_test.rs`。应同步参照 `group_and_expr_test.go` 的原始意图，但 Rust 特有的弱引用释放、零哈希规范化和回退分派需要 Rust 独立断言。兼容风险主要是错误去重或漏去重，性能风险主要是回退哈希调用解释文本/复制 schema 相关数据，以及属性访问器的克隆成本。

## 验证依据

- 源码与结构：`pkg/planner/cascades/memo/group_expr.rs`（451 行）、`lib.rs`、`Cargo.toml`。
- 上游和图维护：`memo.rs::{CopyIn, CopyInWithGroupChildren, InsertGroupExpression, RemoveOut, NewGroupExpression, mergeGroup, replaceGEChild}`，`group.rs::{Insert, Delete, addParentGEs, removeParentGEs, mergeTo, Clear, Check}`，`task/task_apply_rule.rs::ApplyRuleTask::Execute`。
- Go 对照：`group_expr.go::{GroupExpression, Hash64, Equals, mergeTo, GetChildStatsAndSchema, GetJoinChildStatsAndSchema, DeriveLogicalProp}`，以及 `group_and_expr_test.go`。
- Rust 测试：`group_and_expr_test.rs::{TestRawHashMap, TestGroupExpressionHashCollision, TestGroupExpressionDelete, TestGroupExpressionHashEquals, TestGroupParentGERefs, TestDeriveLogicalPropPreservesFD}`；`group_expr_test.rs::{single_child_accessor_rejects_join_by_operator_type, join_child_accessor_rejects_non_join_by_operator_type}`；`memo_test.rs::TestInsertGE`。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/planner/cascades/memo/group_expr.rs` 复核了目标文件全貌；`query GroupExpression/DeriveLogicalProp/mergeTo` 确认 Rust 与 Go 同名符号。associated function 调用边因索引消歧异常改用精确 `rg` 及命中源码复核，未把异常输出用作架构结论。
- 本任务是纯文档分析，未运行 Cargo。交付结构以任务指定命令验证，人工复核重点是：11 个固定章节齐全、所有行为结论能回指上述符号、未把 Go 独有物理枚举逻辑描述为当前 Rust 实现。
