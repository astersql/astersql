# `pkg/planner/core/operator/logicalop/logical_lock.rs`

## 文件定位

本文件实现逻辑计划树中的行锁边界 `LogicalLock`，面向 `SELECT ... FOR UPDATE` 与 `SELECT ... FOR SHARE`。它属于 `astersql-planner-core-operator-logicalop` crate；模块入口在 [`lib.rs`](lib.rs) 中以 `mod logical_lock` 声明并通过 `pub use logical_lock::*` 对外导出。该 crate 的 [`Cargo.toml`](Cargo.toml) 用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/planner/core/operator/logicalop`，因此本文件的直接语义基线是同目录 [`logical_lock.go`](logical_lock.go)。

运行链上的上游入口是 `pkg/planner/core/logical_plan_builder_runtime.rs`：解析后的 `SelectLockInfo` 不是 `None` 时，构建器把 `LogicalLock` 插在 FROM/WHERE 子树之上，并复制子树的 schema、输出名和唯一子节点。下游物理化入口是 `pkg/planner/core/operator/physicalop/physical_lock.rs::ExhaustPhysicalPlans4LogicalLock`，它读取锁类型与等待时间生成物理锁候选；MPP/Flash 属性在该下游入口被拒绝。

## 核心职责

1. 用 `LogicalLock` 保存锁元数据，以及理论上供锁执行所需的“表 ID → 行句柄列”和“表 ID → 物理分区 ID 列”映射。
2. 在列裁剪时把句柄列和分区物理表 ID 列加入子节点必需列，防止普通投影需求把锁定位信息裁掉。
3. 保持锁节点自身处于计划树中，同时把上层 TopN 继续交给唯一子节点处理。
4. 将 `SelectLockType` 分为 FOR UPDATE、FOR SHARE 和不受支持三组，为构建器与裁剪逻辑提供共同判定。

当前 Rust 接线需要与职责定义区分：`logical_plan_builder_runtime.rs` 只设置了 `Lock`，由 `Default` 留下两个空映射；尚未复刻 Go `PlanBuilder.buildSelectLock` 收集 `handleHelper.tailMap()`、为分区数据源添加物理表 ID 列的接线。因此裁剪逻辑具备保列能力，但当前主构建路径没有向它提供这些列。

## 主要符号

- `pub struct LogicalLock`：逻辑锁节点。`BaseLogicalPlan` 保存上下文、计划 ID、子节点、schema、输出名和统计等通用状态；`Lock: SelectLockInfo` 保存锁类型、等待秒数和目标表；`TblID2Handle: HashMap<i64, Vec<Box<dyn HandleCols>>>` 保存每张表可能包含的一组或多组句柄列；`TblID2PhysTblIDCol: HashMap<i64, Column>` 保存分区表的隐藏物理表 ID 列。
- `impl Default for LogicalLock`：构造无上下文、无子节点、锁类型为 `None`、等待时间为 0、目标表和两张映射均为空的值。`SelectLockInfo` 同时存在 `lock_type` 与 `LockType` 字段，本文件的判定和下游物理化读取的是 `LockType`。
- `LogicalLock::Init(self, ctx) -> Self`：用 `NewBaseLogicalPlan(ctx, "Lock", 0)` 分配计划 ID并设置算子类型；它不设置 schema、输出名、子节点或句柄映射，这些由调用方完成。
- `LogicalLock::PruneColumns(&mut self, parent_used_cols) -> Result<()>`：按锁类型决定普通透传裁剪或附加锁必需列后裁剪唯一子节点。
- `LogicalLock::PushDownTopN(&mut self, top_n)`：只有同时存在 TopN 和第一个子节点时才调用子节点的 `PushDownTopN`，并在子节点返回替换计划时更新该 child。
- `impl LogicalPlan for LogicalLock`：提供向下转型、基类访问及上述两个专用优化方法的 trait 分派。trait 版 `PushDownTopN` 始终返回 `None`，含义是调用完成后锁节点本身不被返回值替换。
- `isSelectForUpdateLockType`：仅接受 `ForUpdate`、`ForUpdateNoWait`、`ForUpdateWaitN`。
- `isSelectForShareLockType`：仅接受 `ForShare`、`ForShareNoWait`。
- `IsSupportedSelectLockType`：前两类集合的并集；`None` 和两种 `SkipLocked` 当前均返回 `false`。

## 执行流程

构建阶段，`logical_plan_builder_runtime.rs` 从 SELECT AST 取得非 `None` 的锁信息。若锁属于 FOR UPDATE 系列，它先设置 `builder.isForUpdateRead = true`，递归把子树中每个 `DataSource.IsForUpdateRead` 设为 true；随后以 AST 锁信息覆盖默认 `Lock`，调用 `Init`，复制子树 schema 和输出名，并把原计划设为唯一子节点。FOR SHARE 也建立锁节点，但不走 FOR UPDATE 的数据源标记分支。

列裁剪阶段分两条路径：

1. `Lock.LockType` 不受支持时，调用 `BaseLogicalPlan::PruneColumns`，把父节点使用列原样传给第一个子节点；无子节点时正常返回 `Ok(())`。
2. 锁类型受支持时，先复制父节点列，遍历每个表的所有 `HandleCols::IterColumns()` 并追加其列，再按同一表 ID 查找并追加物理表 ID 列。最后只对第一个子节点调用裁剪并传播错误；无子节点同样返回成功。这里不去重，重复列是否消解由下游具体裁剪实现决定。

TopN 下推阶段，`None` 立即返回；存在 TopN 但没有 child 也立即返回。若两者都存在，锁节点调用第一个 child 的 trait 方法。child 返回 `Some(new_plan)` 时替换原 child，返回 `None` 时保留 child 对象内已经发生的原地变化。外层 `LogicalPlan` trait 实现始终返回 `None`，使调用方继续保留 `LogicalLock` 这一锁语义边界。

物理化阶段，`ExhaustPhysicalPlans4LogicalLock` 在 Flash/MPP 属性下告警并返回空集合；缺少规划上下文也返回空集合。其余情况把 `Lock.LockType` 映射为执行侧字符串，把 `Lock.WaitSec` 交给 `LegacyPhysicalLock`，复制逻辑 schema，并给物理节点配置子属性和按期望行数缩放的统计。

## 数据与状态

`LogicalLock` 自身不持有行数据或已获取的锁，只保存规划期元数据。实际行锁的获取发生在后续物理/执行阶段。本节点的可变状态主要是 `BaseLogicalPlan` 内的计划树状态，以及两张按表 ID 索引的映射。

`TblID2Handle` 的值是 trait object 列表，因为整型主键句柄和 common handle 可用不同 `HandleCols` 实现；列裁剪只依赖统一的 `IterColumns`。同一个表允许多个句柄描述。`TblID2PhysTblIDCol` 只在分区表需要从输出行恢复实际分区时有意义，并以相同表 ID 与句柄映射关联。

`Default` 产生的对象尚未分配上下文和计划 ID。调用 `Init` 后基类类型为 `"Lock"`、query block offset 固定为 0；schema、输出名和 child 仍必须由构建器设置。当前 Rust 主构建路径没有填充两张映射，这是影响锁定位信息保留的明确迁移缺口。

相邻生成文件 `hash64_equals_generated.rs` 为 `LogicalLock` 提供另一组固有方法：哈希包含锁类型和排序后的 `TblID2Handle` 表 ID 键，相等比较锁类型与句柄映射键集合；具体句柄值、物理表 ID 映射、等待时间和目标表不参与该实现。修改这些字段的语义时，需要同步评估 memo 等价性。

## 依赖与调用关系

上游关系：

- `pkg/planner/core/logical_plan_builder_runtime.rs` 构造并挂接 `LogicalLock`，且调用 `isSelectForUpdateLockType` 决定是否标记更新读。
- `pkg/planner/core/operator/logicalop/logical_d_aster_unit_test.rs` 直接验证三个锁类型判定函数。
- `pkg/planner/core/operator/logicalop/logical_lock_test.rs` 直接构造节点验证 TopN 的空输入边界。
- `pkg/planner/cascades/memo/group_expr.rs`、若干物理计划基座文件通过类型检查/计划装配识别该逻辑节点；RustCodeGraph 文件级关系还列出 `physical_lock.rs` 为使用方。

本文件内部/下游关系：

- `BaseLogicalPlan`/`LogicalPlan` 提供子树、schema、上下文及优化器 trait 行为；`NewBaseLogicalPlan` 完成节点身份初始化。
- `SelectLockInfo`/`SelectLockType` 从 `parser_ast` 经 crate 根重新导出。
- `Column` 来自 expression，`HandleCols` 来自 planner util，二者也是列裁剪的直接数据来源。
- `LogicalLock::PruneColumns` 调用 `IsSupportedSelectLockType`、`HandleCols::IterColumns`、子节点 `PruneColumns`。
- `LogicalLock::PushDownTopN` 调用子节点 `PushDownTopN`。
- `pkg/planner/core/operator/physicalop/physical_lock.rs::ExhaustPhysicalPlans4LogicalLock` 消费本节点的上下文、schema、统计、锁类型和等待时间。当前活跃物理化函数没有读取两张句柄映射；文件顶部保留的注释化旧方案描述过把它们传给物理锁，但不能当作现行行为。

## 错误处理与边界

本文件没有自行创建业务错误。`PruneColumns` 的 `Result<()>` 只把基类或第一个子节点裁剪错误通过 `?` 原样向上传播；`Init`、TopN 下推和类型分类均不返回错误。

需要特别保留的边界如下：

- 空 child：裁剪与 TopN 下推均静默完成，不 panic；这使默认对象可用于构造中间态和单元测试，但完整计划中的锁节点按设计应有一个 child。
- 多 child：代码只处理 `first_mut()`，没有验证恰好一个 child；逻辑锁应维持单子节点不变量，新增构建入口不能依赖其余子节点会被处理。
- 不受支持类型：`None`、`ForUpdateSkipLocked`、`ForShareSkipLocked` 不追加句柄/分区列；其中 Skip Locked 已存在于 AST 枚举，但本文件明确未把它归入支持集合。
- 锁信息双字段：`Default` 同时把 `lock_type` 与 `LockType` 设为 `None`，而本文件读取 `LockType`。调用方若只修改小写字段，不会改变这里的行为。
- 重复必需列：追加过程不检查父需求或不同 handle 间是否含有同一列，可能增加下游裁剪工作量，但不改变“不可丢失锁列”的保守目标。
- TopN 类型：本文件不检查传入计划是否真为 TopN，而把 `LogicalPlanRef` 交给 child；类型约束依赖调用协议。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、事务句柄或显式锁对象，也没有 `unsafe`。所有优化动作都通过 `&mut self` 串行修改当前计划节点。`PushDownTopN` 中的 `LogicalPlanRef` 和 child 使用所有权移动/替换；`PruneColumns` 只克隆 `Column` 元数据，不复制执行数据。

`LogicalLock` 的生命周期覆盖逻辑计划构建与优化，随后其锁类型、等待时间、schema 和统计被物理计划消费。它不负责开始、提交、回滚事务，也不负责释放行锁。两个 `HashMap` 和其中的 boxed `HandleCols` 随节点销毁而自动释放；本文件没有自定义 `Drop` 或外部资源清理逻辑。

锁元数据在 Go 版本中被约定为构建后只读；Rust 类型没有用私有字段或共享不可变指针强制这一点，字段均为 `pub`。扩展代码应把 AST 锁信息视为规划期只读值，避免哈希/等价判断与物理化结果在优化过程中失配。

## 与 Go 版本的对应关系

结构和主要算法直接对应 [`logical_lock.go`](logical_lock.go)：两端都有基类、锁信息、句柄映射、分区物理表 ID 映射；`Init` 都创建 Lock 类型的基类；裁剪都只为五种受支持锁追加句柄与分区列；TopN 都越过锁节点下推到 child；三个锁类型辅助函数的集合一致。

已确认的差异与迁移状态：

- Go `Lock` 是 `*ast.SelectLockInfo`，Rust 是按值持有并在构建时 `clone`；Rust 因此不存在空指针分支。
- Go `PruneColumns` 返回当前逻辑计划与错误，Rust 通过 `&mut self` 原地修改并只返回 `Result<()>`。
- Go 支持路径直接索引 `Children()[0]`，Rust 用 `first_mut()`，所以畸形空 child 在 Rust 中不会 panic。
- Go `PushDownTopN` 最终返回 `p.Self()`，Rust 的固有方法返回 `()`，trait 方法返回 `None` 表示保留当前锁节点；可观察的树形目标相同，但返回协议不同。
- Go `PlanBuilder.buildSelectLock` 会收集 `handleHelper.tailMap()`，并在分区表路径递归添加/记录物理表 ID 列。当前 Rust 构建器没有对应填充，两个映射保持默认空值；这不是已完成的等价移植。
- Go 物理锁会继续携带句柄与物理表 ID 映射；当前 Rust 活跃的 `ExhaustPhysicalPlans4LogicalLock` 创建 `LegacyPhysicalLock`，仅消费锁类型、等待时间、schema 和统计。注释中的完整 `PhysicalLock` 草案不构成运行时代码。
- Go 将 `ForUpdateSkipLocked`/`ForShareSkipLocked` 同样排除在支持集合之外，Rust 的分类测试明确锁定了这一点。

## 扩展指南

补齐锁构建功能时，首要接入点不是改写本文件已有裁剪算法，而是让 `logical_plan_builder_runtime.rs` 在构造 `LogicalLock` 前生成 `TblID2Handle`，并为分区数据源生成和记录 `TblID2PhysTblIDCol`；实现依据应逐项对照 Go `PlanBuilder.buildSelectLock` 与 `setExtraPhysTblIDColsOnDataSource`，不能只填一张映射。随后还需确认活跃物理锁及执行器真正消费这些映射，否则“保列”仍不会形成完整锁键链路。

增加新锁类型时，应同步修改 `isSelectForUpdateLockType` 或 `isSelectForShareLockType`、分类测试、构建器的更新读标记策略、物理锁字符串/等待策略，以及执行器兼容性。尤其不要仅让 `IsSupportedSelectLockType` 返回 true：这会触发隐藏列保留，却不保证物理执行理解该锁模式。

调整列裁剪时，应维持“父使用列 + 全部 handle 列 + 同表物理 ID 列”不变量，并覆盖整型 handle、common handle、多表 join、分区表、重复列、无 child 与子节点报错。Rust 测试必须放在独立测试文件；优先扩展同目录 `logical_lock_test.rs`，锁类型集合可继续放在现有 `logical_d_aster_unit_test.rs`。

调整 TopN 协议时，要同时检查固有方法与 `LogicalPlan` trait 适配层，确保锁节点不会因返回值约定被移除；至少保留“`None` 不访问 child”和“child 返回替换计划时更新 child”的测试。调整字段参与等价性的规则时，还要同步审查 `hash64_equals_generated.rs` 与生成器 `pkg/planner/core/generator/hash64_equals/hash64_equals_generator.rs`，避免手改生成文件后被覆盖。

兼容性风险集中在 Skip Locked 支持集合、`LockType`/`lock_type` 双字段、Go/Rust 返回协议和 MPP 禁止路径；性能风险主要来自重复追加列导致更宽的中间行，以及遗漏裁剪导致隐藏列贯穿过多算子。任何主链补齐都应另行做针对性单元/集成验证，本分析任务本身不修改运行时代码。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可由 `node --file pkg/planner/core/operator/logicalop/logical_lock.rs` 完整读取。
- RustCodeGraph 文件节点：目标文件被 `pkg/planner/cascades/memo/group_expr.rs`、`logical_d_aster_unit_test.rs`、`logical_lock_test.rs`、`base_physical_agg.rs`、`physical_lock.rs` 使用。`query LogicalLock` 同时定位 Rust/Go 结构、Go 构建入口和 Rust 物理化入口；`callees IsSupportedSelectLockType` 确认它组合两个分类函数。因 Go/Rust 同名，方法级 callers/callees 查询命中了 Go 定义，本文用精确路径检索核对 Rust 引用，没有把空查询当作无调用证据。
- 已读 Rust 生产文件：本文件、`logical_plan_builder_runtime.rs`、`base_logical_plan.rs`、`hash64_equals_generated.rs`、`physicalop/physical_lock.rs`、`parser/ast/lib.rs`、`planner/util/handle_cols.rs` 以及 crate `lib.rs`。
- 已读 crate 配置：`pkg/planner/core/operator/logicalop/Cargo.toml`，确认 crate 名、路径依赖和 Go 包映射；该文件没有 feature 声明，本目标文件也没有条件编译项。
- 已读 Go 对照：`logical_lock.go` 与 `pkg/planner/core/planbuilder.go::buildSelectLock`/`setExtraPhysTblIDColsOnDataSource`。
- 已读独立 Rust 测试：`logical_lock_test.rs::absent_top_n_does_not_visit_lock_child`；`logical_d_aster_unit_test.rs::lock_type_classification_matches_go_supported_modes`。另以文本检索确认 Go planner 测试中存在 FOR UPDATE 的 point-get、prepare、plan-cache 与 integration 覆盖，但这些是系统级旁证，并非本文件每个 Rust 分支的直接单元测试。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令检查目标文档存在且恰有 11 个固定二级章节，并人工复核以上结论均对应实际符号或直接对照文件。
