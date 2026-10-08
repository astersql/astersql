# `pkg/planner/core/operator/physicalop/base_physical_join.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate；crate 根由 `Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 声明 `mod base_physical_join` 并用 `pub use base_physical_join::*` 导出这里的公开项。它位于逻辑 Join 转成具体物理 Join 与物理计划交付 executor 之间：`base_physical_plan.rs` 从 `LogicalJoin`/`LogicalApply` 建立 `BasePhysicalJoin`，`PhysicalHashJoin`、`PhysicalMergeJoin`、`PhysicalIndexJoin` 等具体节点持有这份公共状态，executor 随后读取其中的连接类型、条件和连接键。

该文件不是执行 Join 算法的实现。它负责物理 Join 的公共规划期模型、克隆、相关列发现、内存估算和输出 Schema 规则；哈希表构建、索引探测、归并推进等行为在具体算子及 executor 中实现。源码没有条件编译项；独立测试通过 `lib.rs` 中的 `#[cfg(test)] mod base_physical_join_test` 接入。

## 核心职责

- `BasePhysicalJoin` 集中保存所有物理 Join 都要共享的 `PhysicalSchemaProducer`、`JoinType`、左右/其他过滤条件、inner/outer 侧信息、普通与 null-aware 连接键、NULL 安全等值位图及外连接默认值。
- `New` 建立空条件、空键集合的基座，并将 `InnerChildIdx` 默认设为 `1`（右孩子）。调用者随后按候选算法和逻辑 Join 内容填充字段；例如 `base_physical_plan.rs` 的物理枚举路径以及 `physical_merge_join.rs::GetMergeJoin`。
- `CloneForPlanCacheWithSelf` 与 `CloneWithSelf` 将计划绑定到新 `ContextRef`，但服务于不同生命周期并具有不同的 `IsNullEQ` 处理规则。
- `ExtractCorrelatedCols` 从三组谓词中汇总外层相关列，使 Apply/去相关及父计划能识别尚未绑定的引用。
- `MemoryUsage` 估算基座、生产者、向量预留空间及向量元素的动态内存，供具体 Join 的内存统计继续累加。
- `BuildPhysicalJoinSchema` 根据 Join 类型合成两侧输出列，并清除外连接可空侧的 `NotNullFlag`；Apply attach 和 MPP HashJoin attach 都直接使用它。

## 主要符号

- `pub struct BasePhysicalJoin`：公共状态容器。`PhysicalSchemaProducer` 提供上下文、children、统计信息和当前 Schema；`LeftConditions`、`RightConditions`、`OtherConditions` 分别保存只能在左侧、只能在右侧和需跨侧求值的表达式；`LeftJoinKeys`/`RightJoinKeys` 按逻辑左右侧排列，`OuterJoinKeys`/`InnerJoinKeys` 按算法角色排列；`LeftNAJoinKeys`/`RightNAJoinKeys` 为 null-aware join 键；`IsNullEQ` 表示相应键是否采用 NULL 安全匹配；`DefaultValues` 用于外连接未匹配侧补值。
- `New(producer, join_type) -> Self`：保留传入 producer 与类型，其余集合为空，默认右侧为 inner child。
- `GetJoinType`、`GetInnerChildIdx`：只读访问连接类型和 inner child 下标。
- `PhysicalJoinImplement`：与 Go 的标记方法对应的空方法，本身不执行工作；Rust 的具体类型接线主要由 `lib.rs` 的 trait/macro 实现完成。
- `CloneForPlanCacheWithSelf(new_ctx) -> Option<Self>`：先调用 `PhysicalSchemaProducer::CloneForPlanCacheWithSelf`；生产者不可安全缓存时以 `None` 短路，否则克隆条件、键、位图和默认值。
- `CloneWithSelf(new_ctx) -> Result<Self, expression::Error>`：通过底层 `BasePhysicalPlan::CloneWithNewCtx` 重建 producer，若原 producer 有 Schema 则复制 Schema；表达式和列逐项克隆，错误向上传播。当前实现刻意将基座副本的 `IsNullEQ` 置空。
- `ExtractCorrelatedCols() -> Vec<CorrelatedColumn>`：按左条件、右条件、其他条件的顺序调用 `expression::ExtractCorColumns`，并克隆结果；不去重。
- `MemoryUsage() -> i64` 与 `EMPTY_BASE_PHYSICAL_JOIN_SIZE`：前者按 capacity 计容器预留槽位，再加表达式、列和 Datum 的动态占用；列和 Datum 的元素统计扣除已由槽位统计覆盖的内联结构大小，避免重复计数。
- 私有 `clone_exprs`：对 `ExprBox` 调用对象安全的 `CloneExpr`，避免只复制 trait object 容器。
- `BuildPhysicalJoinSchema(join_type, join) -> Schema`：本文件唯一独立公开函数，编码各 JoinType 的输出列和 nullability 规则。

## 执行流程

1. 物理计划枚举在 `base_physical_plan.rs` 中为 `LogicalApply` 或 `LogicalJoin` 建立 `PhysicalSchemaProducer`，以 `BasePhysicalJoin::New` 初始化基座，并通过 `populate_physical_join_conditions` 填入条件、键和角色信息。
2. 候选算法把基座装入具体节点。`PhysicalHashJoin`、`PhysicalMergeJoin` 和 `PhysicalIndexJoin` 直接持有它；IndexHash/IndexMerge Join 则经 `PhysicalIndexJoin` 间接复用。具体节点额外保存算法专属状态。
3. 普通候选克隆调用 `CloneWithSelf`。它先换绑 context 和 Schema，再深克隆条件/列/默认值；`PhysicalHashJoin::Clone` 在基座返回后显式恢复 `IsNullEQ`，以维持 hash key 与 NULL 安全位图对齐。Merge/Index Join 没有这一步，因此新增使用者必须明确自己是否需要该位图。
4. 计划缓存路径由 `plan_clone_generated.rs::CloneForPlanCache` 调用 `CloneForPlanCacheWithSelf`。该路径完整保留 `IsNullEQ`，且当 producer 不可缓存时将失败作为 `None` 传播；`cache_snapshot.rs::CachedBasePhysicalJoin` 也显式捕获和恢复全部公共字段。
5. attach children 后，调用者以 `BuildPhysicalJoinSchema` 重建输出：Semi/AntiSemi 仅返回左 Schema；LeftOuterSemi/AntiLeftOuterSemi 在左 Schema 后追加 Join 自身 Schema 的最后一个标记列；其他类型合并左右 Schema。Left/Right/Full Outer Join 分别清除右侧、左侧、双侧列的 `NotNullFlag`。
6. executor 构建阶段读取基座的 JoinType、左右条件、普通键和 null-aware 键，将规划结果转换成具体执行配置；本文件自身不访问数据行或存储层。

## 数据与状态

`BasePhysicalJoin` 的字段均为节点私有所有权下的可变规划状态，没有全局变量或内部缓存。三个条件向量和六个键向量保持顺序语义：条件提取按三组依次遍历，连接键与 `IsNullEQ` 等并行数据通常要求位置对齐。源码本身不校验这些长度关系，责任在构造具体 Join 的上游代码。

`InnerChildIdx` 使用 `usize`，约定二元 Join 中 `0` 为左、`1` 为右；`New` 只提供默认值，不证明 children 已存在。`DefaultValues` 按值持有 `Datum`。`PhysicalSchemaProducer` 是状态主干，包含当前 Schema 和 `BasePhysicalPlan`；克隆时必须先正确克隆它，否则其余 Join 字段不能形成可用物理节点。

`MemoryUsage` 使用向量 `capacity` 而非 `len`，因此包含尚未使用的预留空间。表达式槽位按 `ExprBox` 大小、键槽位按 `Column` 大小、默认值槽位按 `Datum` 大小计；动态元素再单独累加。`IsNullEQ` 的 `bool` 每槽按一字节计入。该值是规划期估算，不等于 allocator 的精确驻留内存。

## 依赖与调用关系

上游直接证据包括：

- `base_physical_plan.rs` 的 `LogicalApply`/`LogicalJoin` 物理枚举调用 `BasePhysicalJoin::New` 并填充条件；MPP Join attach 调用 `BuildPhysicalJoinSchema`。
- `physical_apply.rs::Attach2Task` 在挂载两个 RootTask child 后调用 `BuildPhysicalJoinSchema`，并写回基座的 producer Schema。
- `physical_merge_join.rs::GetMergeJoin` 从逻辑 Join 克隆三组条件与左右键；`PhysicalHashJoin::Clone`、`PhysicalMergeJoin::Clone`、`PhysicalIndexJoin::Clone` 调用基座克隆。
- `plan_clone_generated.rs` 的 Hash/Merge/Index Join 计划缓存实现调用 `CloneForPlanCacheWithSelf`；`cache_snapshot.rs::CachedBasePhysicalJoin` 是另一条序列化式缓存边界。
- `physical_hash_join.rs::ExtractCorrelatedCols` 先取基座相关列再追加自身等值条件的相关列；各具体 Join 的 `MemoryUsage` 将基座估算作为组成部分。
- `pkg/executor/builder.rs` 直接读取 `JoinType`、左/右条件、普通键、NA 键等字段，说明这些数据已进入 SQL 规划到执行的主链。

下游 crate 依赖由源码与 `Cargo.toml` 共同限定：`base` 提供 `ContextRef`、`JoinType`、`PhysicalPlan`；`expression` 提供表达式、列、相关列和 Schema；`types` 提供 `Datum`；`mysql` 提供 `NotNullFlag`。这些均由本 crate 的 path dependency（`base`、`expression`、`types`、`mysql`）声明，没有由本文件直接使用的 feature gate。

RustCodeGraph 对目标文件识别出 17 个符号，并报告其被 `cache_snapshot.rs`、executor 测试及其他物理计划文件使用；对精确 Rust 函数的 callers/callees 查询未返回边，因此上述 Rust 调用边又以精确 `rg` 和对应源码片段补证，未把宽泛同名搜索结果当作结论。

## 错误处理与边界

`CloneForPlanCacheWithSelf` 的失败边界是 `Option`：底层 producer 返回 `None` 时整次克隆立即返回 `None`。`CloneWithSelf` 的失败边界是 `expression::Error`，唯一显式可失败步骤是换绑 context 的底层计划克隆；其余集合克隆不返回错误。调用方必须保留这两类失败语义，不能用空结构替代失败结果。

`ExtractCorrelatedCols` 不去重，也不检查同一相关列是否跨多条条件重复出现；这是与逐表达式提取一致的行为。`MemoryUsage` 只能由有效引用调用，Rust 不存在 Go 方法对 nil receiver 返回零的分支。

`BuildPhysicalJoinSchema` 对不完整 children 采取防御性退化：缺少左孩子时使用空 Schema；需要合并但缺少右孩子时只返回左 Schema；outer-semi 找不到当前 Join Schema 的最后一列时不追加标记列。这比当前 Go 实现直接索引两个 children/最后一列、更可能 panic 的前置条件宽松。正常物理 Join 仍应满足二元 children 和有效 marker Schema，不应依赖这些退化路径掩盖构造错误。

外连接仅在列具有 `RetType` 时删除 `NotNullFlag`；`RetType == None` 的列原样保留。函数不修改 child 的 Schema，而是克隆/重新组装返回值。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、异步任务、线程、本地/远程 I/O 或显式资源句柄；方法均同步执行。`ContextRef` 的共享/线程安全契约来自 `base` crate，本文件只在克隆时换绑它。测试中的 `AtomicI32` 只是 `TestPlanContext` 的 plan id 模拟，不属于生产实现。

生命周期以物理计划节点为边界：`BasePhysicalJoin` 随具体 Join 创建，候选重写或计划缓存时产生独立副本，随计划释放而释放其表达式、列和 Datum。表达式/列通过专用 Clone 方法复制，避免新旧候选意外共用可变规划状态；计划缓存 snapshot 则将表达式/列转换为缓存表示并在新 context 中恢复。没有需要调用者显式 close 的资源。

## 与 Go 版本的对应关系

直接对照文件是 `base_physical_join.go`。结构字段、访问器、标记方法、相关列提取、两类克隆、内存估算和 Schema 构造均有同名或等价实现；Rust 用 `Vec<ExprBox>`/`Vec<Column>`/`Vec<Datum>` 的值所有权替代 Go slice 中的 interface/pointer，并以 `Option`/`Result` 表达克隆失败。

需要注意以下已验证差异：

- 当前 Go `CloneWithSelf` 会复制 `IsNullEQ`，而当前 Rust `BasePhysicalJoin::CloneWithSelf` 清空它；独立 Rust 测试 `ordinary_and_plan_cache_clone_match_go_null_eq_contract` 固定了“普通基座克隆清空、计划缓存克隆保留”的现状。`PhysicalHashJoin::Clone` 随后显式补回该位图，所以 HashJoin 普通克隆仍保留实际语义；其他新增调用方不能假定基座普通克隆会保留它。
- Go 计划缓存克隆逐个 `Datum::Clone`，Rust 使用 `Vec<Datum>::clone`；在当前 Rust Datum 的值语义下这是对应操作。
- Rust Schema 构造对缺 child/marker 采用安全退化，Go 使用直接下标。有效计划上的主要 JoinType 输出规则相同。
- Go `MemoryUsage` 的当前 capacity 表达式重复计算 `RightNAJoinKeys` 且未计 `RightJoinKeys` 槽位；Rust 对六组键各计一次。Rust 独立测试只验证所有 Rust 向量预留空间都会反映在差值中，因此这是明确的实现差异，不应描述成逐项完全一致。
- Go 的 `PhysicalJoin` 是接口标记；Rust 的 `PhysicalJoinImplement` 是公开占位方法，真正的通用物理算子 trait 接线见 `lib.rs` 的 `join_operator_core!` 及 HashJoin 专用实现。

## 扩展指南

新增公共 Join 状态时，应同时检查 `BasePhysicalJoin`、`New`、两种克隆、`MemoryUsage`、`cache_snapshot.rs::CachedBasePhysicalJoin::{capture,restore}`，以及所有具体 Join clone/plan-cache clone；若字段与键按位置对齐，还需定义并测试长度不变量。Go 对齐任务还必须同步核对 `base_physical_join.go`，不能只让 Rust 编译通过。

调整输出列规则时，修改入口应是 `BuildPhysicalJoinSchema`，并同步检查 `physical_apply.rs::Attach2Task` 和 `base_physical_plan.rs` 的 MPP attach 调用。需要覆盖 Semi、AntiSemi、两种 outer-semi、Inner、Left/Right/Full Outer，尤其验证 marker 列位置与可空侧 `NotNullFlag`。当前独立测试未直接覆盖该函数，新增回归应放在同目录独立测试文件（优先扩展 `base_physical_join_test.rs`），不要把测试内嵌进生产源文件。

调整克隆语义时，必须区分普通候选克隆、计划缓存克隆和 cache snapshot 三条路径，并复核 `PhysicalHashJoin::Clone` 对 `IsNullEQ` 的补偿是否仍必要。调整内存统计时，应以 capacity 增量和含动态负载的元素分别测试，避免容器槽位与元素内联大小重复计数。

兼容性风险主要是 Join 输出列顺序/nullability、NULL 安全匹配位图丢失、NA 键角色颠倒；这些会改变计划或查询结果。性能风险主要是克隆深度与内存估算失真，可能影响计划缓存占用判断。并发风险较低，因为本文件无内部并发状态，但不得把可变表达式/列改为跨候选共享而不审计调用方。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/planner/core/operator/physicalop/base_physical_join.rs` 确认目标已索引；`node --file ... --offset 1 --limit 1000` 读取完整 232 行及 17 个符号；对 `BasePhysicalJoin`、`BuildPhysicalJoinSchema` 等执行了 `query`，并对精确 symbol id 执行 `callers`/`callees`。精确边查询无输出后，以 `rg` 补齐索引未覆盖的 Rust 调用边。
- 生产源码：`base_physical_join.rs`；直接入口/调用方 `base_physical_plan.rs`、`physical_apply.rs`、`physical_hash_join.rs`、`physical_merge_join.rs`、`physical_index_join.rs`、`plan_clone_generated.rs`、`cache_snapshot.rs`、`lib.rs`；执行消费方 `pkg/executor/builder.rs`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`，确认 crate 名、`lib.rs` 入口、`autotests = false`、相关 path dependencies 与 `package.metadata.porting.go-package`。
- Go 对照：`pkg/planner/core/operator/physicalop/base_physical_join.go`；相关 Go 语义证据还包括 full join 测试对 `IsNullEQ` 的使用。
- 独立 Rust 测试：`base_physical_join_test.rs` 验证两类克隆当前的 `IsNullEQ` 差异以及所有向量预留容量的内存计量；`physical_hash_join_test.rs` 提供 NULL 等值/full outer join 的相邻行为证据。本文档任务未运行 Cargo，符合总计划的纯文档约束。
- 结构验收以任务指定命令检查目标文件存在且恰有十一个固定二级标题；人工复核确认本文区分了当前事实、Go 差异、错误边界和安全扩展点，没有将防御性退化或未测试行为写成额外功能保证。
