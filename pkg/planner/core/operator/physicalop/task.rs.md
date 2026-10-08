# [`pkg/planner/core/operator/physicalop/task.rs`](task.rs)

## 文件定位

本文件位于 `astersql-planner-core-operator-physicalop` crate。crate 根 `lib.rs` 以私有模块 `mod task;` 编译它，再以 `pub use task::*;` 导出其接口。它处在物理算子生成与执行形态选择的边界：把一棵 `dyn PhysicalPlan` 包装成 TiDB 本地执行的 `RootTask`，保存尚未组装成 reader 的 `CopTask` 两侧计划，并提供 Cop 收尾和物理树遍历工具。

这里的“任务”是优化器候选计划的执行载体，不是异步运行时任务。其公共契约来自 `pkg/planner/core/base/task_base.rs` 的 `Task` trait。`Cargo.toml` 将本 crate 映射到 Go 包 `pkg/planner/core/operator/physicalop`，本文件直接使用 `base`、`expression`、`property` 与 `kv` 等工作区 crate。

## 核心职责

- `RootTask`：持有最终在 Root/TiDB 侧继续挂接的物理计划、来源任务、优化警告、Root 残余条件，以及向父 Join 传播的 MPP 分区信息。
- `CopTask`：保存 reader 构造前的表侧/索引侧下推计划，负责将不可下推条件物化为 Root `PhysicalSelection`、在索引侧结束时迁移统计，以及辨认计划实际使用的存储类型。
- `TryExpandVirtualColumn`：为调用者提供可剪枝的深度优先物理计划树遍历框架。名称沿用 Go，但 Rust 版本并不在函数内部直接展开虚拟列；具体处理由传入的 visitor 完成。
- `AttachedTask`：只是 `RootTask` 的类型别名，用于表达“算子已挂接后的任务”，不引入新状态或行为。

## 主要符号

- `RootTask { plan, source, warnings, RootTaskConds, RootTaskSelectivity, FromDataSource, mpp_partition_type, mpp_hash_cols }`：`plan` 必须存在；构造函数 `New` 将选择率设为 `1.0`、分区类型设为 `property::AnyType`，`NewWithMpp` 再覆盖分区类型和 Hash 列。
- `RootTask::{GetPlan, GetPlanMut, SetPlan}`：分别提供只读、可变访问和整体替换。计划字段保持私有，外部应经这些入口或 `Task::{plan, plan_mut}` 访问。
- `impl Task for RootTask`：行数来自 `plan.stats_count()`；`copy` 深克隆计划、递归复制 `source` 并克隆条件/警告/分区列；`convert_to_root_task` 返回副本；`invalid` 将来源任务的失效状态向上透传；MPP getter 返回值克隆 Hash 列，setter 原子式替换这组元数据。
- `CopTask { TablePlan, IndexPlan, RootTaskConds, IndexPlanFinished }`：当前是 Go `CopTask` 的聚焦子集；`Default` 表示两侧计划为空、无 Root 条件、索引侧未结束。
- `CopTask::HandleRootTaskConds(&self, &mut RootTask, Option<f64>)`：把 Cop 侧残余条件转换为 Root 选择算子。
- `CopTask::FinishIndexPlan(&mut self)`：幂等地结束索引侧，并在双侧计划均存在时把索引统计复制到表侧，同时保留表侧 `StatsVersion`。
- `CopTask::GetStoreType(&self) -> kv::StoreType`：沿表侧单孩子链寻找叶子；分叉即按 MPP 树返回 `TiFlash`，否则优先读取 `PhysicalTableScan.StoreType` 或 `BasePhysicalPlan.StoreType()`，无法识别时退回 `TiKV`。
- `TryExpandVirtualColumn(&mut BasePhysicalPlan, &mut dyn FnMut(&mut dyn PhysicalPlan) -> bool)`：先访问父节点；visitor 返回 `true` 时剪掉该节点的整个子树，否则克隆孩子、递归访问，再通过 `set_children` 装回。

## 执行流程

1. 物理算子挂接时，crate 内的默认挂接逻辑和具体算子通过 `RootTask::New` 或 `NewWithMpp` 包装已构造的计划。`lib.rs::attach_operator_to_task` 会克隆子任务计划、挂到当前算子，并把首个子任务的 MPP 分区信息带入新的 Root 任务。
2. Cop 路径转成 Root reader 前，Go 主链会先 `FinishIndexPlan`。Rust 方法首先用 `IndexPlanFinished` 保证只执行一次；若表侧或索引侧缺失，只更新完成标志，不迁移统计。
3. 若 Cop 留有不可下推条件，`HandleRootTaskConds` 取得现有 Root 计划的上下文和 query block offset，以给定的有限选择率缩放统计；选择率缺失、NaN 或无穷时使用 `0.8`。它把原计划作为新 `PhysicalSelection` 的唯一孩子，克隆条件，并同时在选择算子和 `RootTask` 上记录 `FromDataSource`、条件及选择率。
4. `GetStoreType` 仅检查 `TablePlan`。没有表侧计划时返回 `TiKV`；遍历中遇到多个孩子时将其视为 TiFlash/MPP 分支；到叶子后再读取扫描或基础计划保存的 store type。
5. `TryExpandVirtualColumn` 采用前序深度优先遍历。每层先让 visitor 决定是否停止下钻；继续时克隆所有直接孩子，递归修改克隆体，最后整体替换父节点的孩子集合。

## 数据与状态

`RootTask` 拥有 `Box<dyn PhysicalPlan>`，因此不存在 Go 中空 `p` 的正常构造状态。`source: Option<Box<dyn Task>>` 记录它从何种任务提升而来，并决定 `invalid()` 是否透传失效；它不是执行期父指针。`warnings` 与 `RootTaskConds` 都随 `copy` 独立克隆，防止候选任务之间共享可变容器。MPP Hash 列在读写边界也按值克隆。

`RootTask::memory_usage` 当前只累计计划及 Root 条件表达式的内存估算，没有计入 `source`、`warnings`、MPP Hash 列和结构体自身开销；调用者不应把它理解为完整堆占用。`CopTask` 不实现 `Task`，其状态只服务于本文件的收尾方法。`IndexPlanFinished` 是单向幂等标志；一旦置为 `true`，后续调用不会重新同步统计。

`HandleRootTaskConds` 会把同一组条件分别克隆进 `PhysicalSelection.Conditions` 与 `RootTask.RootTaskConds`。这保留了 Go 路径所需的来源信息，但意味着两处不是同一个可变对象。`TryExpandVirtualColumn` 同样采用“克隆孩子后替换”的所有权策略；独立测试验证克隆后计划 ID 保持不变。

## 依赖与调用关系

上游方面，`lib.rs` 公开再导出本文件；`base_physical_plan.rs`、`physical_apply.rs`、`physical_projection.rs`、`physical_batch_point_get.rs`、`physical_indexlookup_reader.rs`、`nominal_sort.rs` 等构造 `RootTask`。RustCodeGraph 将 `task.rs` 标为被 15 个文件使用，并显示 `RootTask::New/NewWithMpp` 在物理算子挂接路径中有大量调用。`AttachedTask::NewWithMpp` 也用于基础物理计划的默认挂接。

下游方面，Root 的通用行为落到 `base::Task` 和 `dyn PhysicalPlan`；条件物化依赖 `PhysicalSelection`、`StatsInfo::Scale`、`ExprBox::{CloneExpr, MemoryUsage}`；存储辨认依赖 `kv::StoreType` 与 `PhysicalTableScan`；MPP 信息依赖 `property::{MPPPartitionType, MPPPartitionColumn}`。

直接搜索显示，Rust 生产代码当前没有调用 `HandleRootTaskConds`、`FinishIndexPlan`、`GetStoreType` 或 `TryExpandVirtualColumn`；这些方法的直接 Rust 证据来自 `task_test.rs`。相应 Go 方法则接入 `task_base.go` 的 Cop-to-Root 转换以及 `find_best_task.go`、`core/task.go` 的优化路径。因此这些 Rust API 已具备局部行为，但不能仅凭本文件宣称完整 Cop 主链已经迁移。

## 错误处理与边界

本文件没有返回 `Result` 的公共方法。计划克隆失败时，`RootTask::copy` 以 `expect("root task plan clone")` 终止；遍历孩子克隆失败时，`TryExpandVirtualColumn` 以 `expect("physical child clone for virtual-column traversal")` 终止。这些路径假设已接入的物理计划都实现可成功的 `clone_physical`。

`HandleRootTaskConds` 对空条件无操作；对非法或缺失选择率采用 `0.8`，不会传播统计估算错误。它用临时空 `BasePhysicalPlan` 通过 `mem::replace` 取得旧计划，随后必定安装 selection；若中间新增可失败步骤，必须避免把占位计划遗留在任务中。

`FinishIndexPlan` 接受单侧缺失并只设置完成标志；统计迁移不会检查具体计划类型。`GetStoreType` 对空表计划、未知叶子和未携带 store type 的基础计划均保守返回 `TiKV`。它只沿单孩子链观察叶子，多孩子结构直接分类为 `TiFlash`，因此新增非 MPP 的分叉计划时必须重新验证该不变量。

## 并发与资源生命周期

所有修改方法都要求 `&mut`，文件内没有锁、原子、线程、异步任务或通道。任务对象预期在物理优化期间由单一所有者构造和变换；`warnings` 的 Go 注释也明确其非并发安全语义。跨候选分支通过 `copy`/`CloneExpr`/`MPPPartitionColumn::Clone` 隔离状态，而不是通过同步共享。

`Box<dyn PhysicalPlan>` 和 `Box<dyn Task>` 遵循 Rust 所有权自动释放。替换计划时旧 Box 在离开作用域后释放；遍历函数在克隆并装回孩子后释放原孩子集合。`FinishIndexPlan` 的生命周期边界由布尔标志表达，未提供恢复到未完成状态的入口。

## 与 Go 版本的对应关系

直接对照文件为 `task.go`，而 `RootTask`/完整 `CopTask` 结构和 Cop-to-Root 主流程位于 `task_base.go`。

- `FinishIndexPlan` 与 Go 保持“幂等、索引统计覆盖表统计、保留表统计版本”的核心语义；Rust 使用 trait 方法，不依赖 Go 的 `*PhysicalTableScan` 强制类型断言。
- `GetStoreType` 保留空计划为 TiKV、分叉为 TiFlash、叶子表扫描读 `StoreType` 的规则；Rust 还允许 `BasePhysicalPlan` 携带 store type，这是额外兼容路径。
- Go `handleRootTaskConds` 在方法内调用 `cardinality.Selectivity`，失败时回退 `cost.SelectionFactor`；Rust 把选择率作为参数传入，并只校验有限性，默认值硬编码为 `0.8`。Rust 还把条件、选择率和来源标记留存在 `RootTask` 中。
- Go `tryExpandVirtualColumn` 识别 `PhysicalTableScan` 后直接调用 `ExpandVirtualColumn` 并停止；Rust 将识别和修改交给 visitor，同时用 visitor 的布尔返回值表达剪枝。二者名称相同，但 Rust 调用者必须显式实现 Go 的扫描节点处理逻辑。
- Go `RootTask::Copy` 共享物理计划指针，只复制 warning 容器；Rust `copy` 调用 `clone_physical` 深克隆计划并递归复制 `source`。Go 以空计划判断 RootTask 无效，Rust 的计划不可空，改为透传 `source.invalid()`。
- Go `CopTask` 还含成本、顺序、索引合并、分区、列和 warning 等大量字段；本文件 Rust `CopTask` 仅覆盖四个收尾字段。`task_base.rs` 另有不同的移植模型，不能把两个同名类型视为同一个 Rust 类型。

## 扩展指南

新增 Root 任务元数据时，应同时更新 `RootTask::New`、`NewWithMpp`（若与分区相关）、`Task::copy`、内存统计和对应独立测试，确保候选复制不会丢状态。若状态要通过算子挂接传播，还要检查 `lib.rs::attach_operator_to_task` 及各具体算子的 `attach_operator_to_task`。

扩展 Cop 收尾字段或行为时，应先以 Go `task_base.go`/`task.go` 的真实增量为依据，避免把完整 Go 子系统递归搬入本文件。修改 `FinishIndexPlan`、`HandleRootTaskConds` 或 `GetStoreType` 时，分别补充缺失单侧/重复调用、非法选择率/空条件、未知叶子/多孩子等边界。测试必须继续放在同目录独立文件 `task_test.rs`，不要嵌入生产源文件。

要实现完整虚拟列展开，最合适的接入点是 `TryExpandVirtualColumn` 的 visitor 或一个包装函数；需明确 visitor 返回 `true` 是“本节点已处理，停止下钻”，并验证克隆/替换不会改变计划 ID、统计、schema 或上下文。任何将这些辅助方法接入生产主链的改动都应同时核对 Go 调用点与 Rust 调用图，并评估深克隆带来的内存和规划时延。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/planner/core/operator/physicalop/task.rs --offset 1 --limit 500` 覆盖目标文件全部 291 行；`query RootTask/CopTask/MppTask` 用于消除同名类型歧义；`explore` 确认 `RootTask::New/NewWithMpp` 的物理算子调用面以及目标文件的使用范围。
- Rust 源码：`pkg/planner/core/operator/physicalop/task.rs`；trait 契约 `pkg/planner/core/base/task_base.rs`；模块声明和再导出 `pkg/planner/core/operator/physicalop/lib.rs`；直接调用样例集中在 `base_physical_plan.rs`、`physical_projection.rs`、`physical_apply.rs`、`physical_batch_point_get.rs` 与 `physical_indexlookup_reader.rs`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`，包括 `base`、`expression`、`property`、`kv` 依赖及 `package.metadata.porting.go-package`。
- Go 对照：`pkg/planner/core/operator/physicalop/task.go` 的四个对应辅助行为；`task_base.go` 的 `RootTask`、完整 `CopTask` 和 `convertToRootTaskImpl`；生产调用点还包括 `pkg/planner/core/find_best_task.go` 与 `pkg/planner/core/task.go`。
- 独立测试：`pkg/planner/core/operator/physicalop/task_test.rs` 覆盖叶子 store type、统计版本保留、Root selection 物化和可剪枝遍历；`task_base_test.rs` 属于另一个 `task_base` 模型，只作为相邻迁移证据，不能替代本文件测试。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验证要求目标文件存在且上述固定二级标题恰好出现 11 个；另以路径和符号搜索人工复核链接、调用点与“尚未接入生产调用”的限制说明。
