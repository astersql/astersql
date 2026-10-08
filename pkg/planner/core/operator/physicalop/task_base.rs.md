# `pkg/planner/core/operator/physicalop/task_base.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate（`pkg/planner/core/operator/physicalop/Cargo.toml`），定义一套用值类型 `PhysicalPlanNode` 表达的物理优化任务模型：TiDB 层的 `RootTask`、TiFlash MPP 层的 `MppTask` 和存储层的 `CopTask`。`lib.rs` 以 `pub mod task_base` 暴露它，因此外部只能通过 `physicalop::task_base::*` 明确选择这套 API。

必须区分本文件与同目录 `task.rs`：crate 根通过 `pub use task::*` 导出、并实现 `base::Task` trait 的主链类型来自 `task.rs`；本文件中的三个任务类型没有实现 `base::Task`，仓库内可确认的直接 Rust 使用者是 `task_base_test.rs`。所以它是可编译的、便于逐步核对 Go 语义的独立迁移模型，而不是当前规划器动态分发主链的完整替代品。文件第 31—650 行还以注释保留了 Go 来源实现，真正参与编译的 Rust 实现从 `use crate::physical_common_plans` 开始。

## 核心职责

- `SimpleWarnings` 保存任务构造期间的非致命 Warning/Note，并在任务复制或合并时隔离 `Vec` 容器。
- `RootTask` 表示已经汇聚到 TiDB 层的计划，提供有效性、估算行数、内存估算及幂等的 Root 转换。
- `MppTask` 保存 MPP 分区、哈希列、Root 残余条件及简化直方图，并把 MPP 片段包装为 `ExchangeSender -> ExchangeReceiver -> TableReader`；有残余条件时再在 TiDB 侧加 `Selection`。
- `CopTask` 在 index/table 两侧计划间维护阶段状态，把下推片段收尾为 `IndexLookUpReader`、`PartitionIndexLookUpReader`、`IndexReader` 或 `TableReader`，必要时补 `Projection` 与 Root `Selection`。
- `IndexJoinInfo` 把构造范围所需的列 ID 与相关列 ID 从 Cop 层向 Root 层传递。

这些职责由 `task_base.rs:656-981` 的可编译实现承担；前半段注释中的 Go 全量字段（如 Index Merge、真实直方图、虚拟列与分区剪枝对象）不等于 Rust 结构已经支持。

## 主要符号

- `WarningLevel::{Warning, Note}` 与 `SqlWarning { level, message }`：把 Go 的 `context.SQLWarn` 简化为枚举和字符串消息。
- `SimpleWarnings`：私有 `warnings: Vec<SqlWarning>`；`warning_count` 查询数量，`append_warning`/`append_note` 经过私有 `append` 执行 `u16::MAX` 上限，`copy_from` 合并多个源，`get_warnings` 返回克隆副本。
- `IndexJoinInfo { range_columns, correlated_columns }`：仅保存两组 `i64` 标识，没有 Go `IndexJoinInfo` 的完整表达式/范围对象。
- `RootTask { plan, index_join_info, warnings }`：`invalid` 以 `plan.is_none()` 判无效；`count` 从 `Stats.row_count` 取值；`convert_to_root_task` 克隆自身。
- `MppTask { plan, partition_type, hash_columns, root_conditions, table_column_histograms, warnings }`：`new` 要求一个有效计划并合并警告；`convert_to_root_task` 构造跨层 reader 链。
- `CopTask { table_plan, index_plan, index_plan_finished, keep_order, need_extra_projection, root_conditions, root_condition_selectivity, partition_info, index_join_info, warnings }`：`finish_index_plan` 完成 index 阶段并同步统计；`convert_to_root_task` 选择 reader 形态并挂接补偿算子。

所有结构都派生 `Clone`；`RootTask`、`CopTask` 还派生 `Default`，默认值即无计划的无效任务。`MppTask` 不提供 `Default`，构造入口是 `MppTask::new`。

## 执行流程

警告流程如下：追加入口先检查当前长度，小于 `u16::MAX` 才压入；`copy_from` 先清空目标、按各源总长度预留容量，再逐项克隆。它刻意不重新执行追加上限，因此两个各含 65,535 条警告的源能合并为 131,070 条，`task_base_test.rs::copying_warning_sources_does_not_apply_the_append_limit_again` 固化了该语义。

`MppTask::convert_to_root_task` 的步骤是：若 `plan` 为空则返回默认无效 `RootTask`；否则克隆原计划及 schema/stats；依次新建 Single 分区的 `ExchangeSender`、`ExchangeReceiver` 和名为 `TableReader` 的 `PhysicalKind::Other`；若 `root_conditions` 非空，再新增 `Selection`，把行数固定乘以 `0.8`；最后克隆警告进入 `RootTask`。节点 ID 逐层以子节点 ID 加一生成。

`CopTask::convert_to_root_task` 先拒绝 index/table 都为空的任务，然后克隆 `self`，保证调用者不被改变；克隆体调用 `finish_index_plan`。reader 类型按计划组合决定：两侧都有计划时依据 `partition_info` 选择普通或分区 IndexLookUp；仅 index 时选 IndexReader；仅 table 时选 TableReader。reader 的孩子顺序是 index 在前、table 在后，schema/stats 取最后一个孩子；Root 属性被写入 `required_properties`。随后按需加与当前 schema 等长的列投影，再按 `root_condition_selectivity` 加 Selection；最后把 `index_join_info` 和警告克隆到 Root。

## 数据与状态

计划与统计的核心值来自 `physical_common_plans.rs`：`PhysicalPlanNode` 拥有 `id`、`PhysicalKind`、`schema: Vec<i64>`、孩子、`Stats { row_count, version }` 和所需属性。任务转换通过克隆形成新树，不共享可变计划节点。

`CopTask::index_plan_finished` 是阶段位。`count` 在阶段完成前读取 index 计划行数，完成后读取 table 计划行数；缺少相应计划时返回 `0.0`。`finish_index_plan` 可重复调用：第一次先置位；两侧都存在时把 index 的整份 stats 克隆给 table，但恢复 table 原来的 `version`。`task_base_test.rs` 分别验证活动计划计数切换、行数同步和版本保留。

`root_condition_selectivity` 只在 Cop 转 Root 时使用：有限值原样采用，`None`、NaN 或无穷值回退到 `0.8`；代码未夹取负数或大于 1 的有限值。MPP 路径没有该字段，始终使用 `0.8`。`table_column_histograms`、`partition_type`、`hash_columns` 和 `keep_order` 当前只被存储或构造，转换逻辑并未消费全部字段，这是现有迁移完成度的边界。

## 依赖与调用关系

直接 Rust 依赖仅来自同 crate 的 `physical_common_plans`：`PartitionType`、`PhysicalExpr`、`PhysicalKind`、`PhysicalPlanNode`、`PhysicalProperty`、`Stats`、`TaskType`。这些类型再映射到 `Cargo.toml` 所列的 planner/property、expression、statistics、kv、tipb 等整体 crate 依赖，但本文件本身没有直接导入外部 crate。

RustCodeGraph 对文件给出的直接使用文件为 `base_physical_plan.rs`、`task.rs`、`task_base_test.rs` 与 `find_best_task_test.rs`；精确文本核验表明前三个生产/主链文件主要是同名概念或图的粗粒度关联，只有 `task_base_test.rs` 使用 `crate::task_base::{CopTask, SimpleWarnings}`。`lib.rs:175` 暴露模块，而 `lib.rs:224` 导出的是 `task::*`，这进一步限定了当前上游入口。

下游调用边中，RustCodeGraph 确认 `MppTask::convert_to_root_task` 实例化 `Stats`、`PhysicalPlanNode`、`RootTask` 以及 `ExchangeSender`、`Selection`、`Other` 变体；`CopTask::convert_to_root_task` 还调用本文件的 `invalid`、`finish_index_plan`，并实例化 `Projection`。未发现这些同名转换方法的可靠外部调用边，因此不能声称它们已经接入 `find_best_task` 主链。

## 错误处理与边界

本文件没有 `Result` 返回值。缺计划通过默认无效 `RootTask` 表达；不合法选择率用 `0.8` 回退；警告超出单源追加上限时静默丢弃。`CopTask::convert_to_root_task` 在通过 `invalid` 检查后使用 `children.last().expect("non-empty")`，其不变量是至少一个 index/table 计划存在；match 的 `(None, None)` 分支因此标为 `unreachable!()`。

与 Go 相比，Rust 的 `count` 对缺计划返回 `0.0`，避免 nil 解引用，但也可能掩盖构造错误。转换不验证 schema、分区信息或条件是否匹配底层扫描；有限但越界的选择率会直接产生负行数或放大行数。节点 ID 用加法生成，代码没有处理溢出或全局唯一性冲突。

内存估算也只是近似：三个 `memory_usage` 都以 `size_of::<Self>()` 加计划树递归大小为主，没有另计警告字符串、条件、哈希列、直方图、`IndexJoinInfo` 等堆内存；不可将其视为真实 allocator 占用。

## 并发与资源生命周期

类型没有锁、原子量、异步任务、通道或显式事务资源。所有修改都要求 `&mut self`，转换方法接收 `&self` 并克隆计划、条件、警告和 Index Join 信息，因此转换期间不会修改原任务；`task_base_test.rs::cop_conversion_finishes_index_stats_before_building_the_reader` 明确断言原任务的 `index_plan_finished` 仍为 `false`。

这种值语义与 Go 注释中的“警告 slice 非并发安全”不同：Rust 所有权规则阻止无同步的同一实例并发可变访问，但克隆大型计划树、表达式和警告消息会产生真实复制成本。生命周期由普通 `Vec`、`String` 和 `Option` 的 RAII 管理；文件没有需要手工关闭的网络连接、worker 或存储句柄。Go 注释提到的 Cop worker 并行摊销仅是来源设计背景，Rust 可编译实现没有对应并发或成本计算。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/task_base.go`，辅助逻辑在 `task.go`。Rust 保留的主要语义包括：三类任务分层；警告单次追加上限与跨源合并；Root 转 Root 使用副本；Cop 在 reader 构造前 FinishIndexPlan；统计从 index 同步到 table 时保留 table 的统计版本；Root 残余条件形成 Selection；Index Join 信息与警告向 Root 传播。

当前 Rust 是明显的局部模型，不能等同 Go 全量实现：

- Go 的任务实现 `base.Task` 并使用动态 `base.PhysicalPlan`；本文件使用 `PhysicalPlanNode` 值树且没有实现该 trait。
- Go MPP 转换展开虚拟列、收集分区信息、校验原计划必须是 TableScan/Selection、用 `cardinality.Selectivity` 估算并在失败时记录日志；Rust 固定包装简化节点并以 `0.8` 缩放。
- Go Cop 支持 Index Merge、MV Index、公共句柄、额外 handle 列、原始 schema、直方图、期望行数、部分有序匹配、真实分区信息和聚合下推投影例外；Rust `CopTask` 没有这些字段或分支。
- Go 警告保存 `error` 与 SQLWarn 指针，最终返回值副本；Rust 保存字符串并对整个 `SqlWarning` 克隆。
- Go 的 reader 类型是具体物理算子；Rust 主要以 `PhysicalKind::Other(String)` 标记 reader 名称。

仓库中没有同路径 Go 单元测试文件；Go 语义依据来自 `task_base.go`、`task.go` 及其调用点，Rust 的直接回归证据来自独立的 `task_base_test.rs`。同目录 `task_test.rs` 验证的是 crate 根导出的另一套 `task.rs` 实现，可作为“FinishIndexPlan 保留版本、Root 条件生成 Selection”等共同语义的旁证，不能冒充本模块的直接测试。

## 扩展指南

若要扩展这套独立模型，应先确认目标是继续完善 `task_base`，还是修改真正实现 `base::Task` 的 `task.rs`；若需要进入规划器主链，不能只改本文件。新增行为至少同步更新 `task_base_test.rs`，测试必须保持为独立文件。

- 增加新的 reader/转换分支：修改 `CopTask::convert_to_root_task`，同时明确孩子顺序、schema/stats 来源、Root 属性及 `partition_info` 语义；若替换字符串 reader，应在 `PhysicalKind` 增加明确变体并检查序列化/成本消费者。
- 完善 MPP 回退：修改 `MppTask::convert_to_root_task`，优先移植 Go 的虚拟列展开、计划形状校验、分区信息传递和真实选择率计算；错误路径需要设计显式返回值，不能静默伪造有效 Root。
- 增加任务字段：检查 `Clone`、`Default`、`memory_usage`、转换传播和警告/Index Join 上行是否都需要同步，避免字段只存不消费。
- 修改警告：保持单次追加上限与 `copy_from` 不重新限流的不变量，补充边界测试；若引入共享存储，必须重新说明线程安全与复制隔离。
- 修改 Cop 阶段机：保持 `finish_index_plan` 幂等、原 table stats version 不变、`convert_to_root_task` 不修改调用者；覆盖 index-only、table-only、双读、分区双读、非法空任务及非有限选择率。

兼容风险主要是与 Go 计划形状和统计传播偏离；性能风险主要是深计划树与大警告集合的全量克隆，以及未计入 `memory_usage` 的堆对象。若将本模块接入主链，还需评估与 `task.rs` 同名类型造成的 API 混淆，并统一 crate 根导出策略。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件已索引。
- RustCodeGraph `node --file pkg/planner/core/operator/physicalop/task_base.rs`：逐段读取 981 行源码，确认第 31—650 行是 Go 注释参考，第 652—981 行是可编译 Rust 实现。
- RustCodeGraph `query SimpleWarnings/MppTask/CopTask/convert_to_root_task/finish_index_plan`：核对类型与方法定义位置；`callees convert_to_root_task`、`callees finish_index_plan` 核对内部构造与调用边。`callers` 未返回可靠的外部方法调用，故本文按“未接入主链”保守表述。
- 已读 Rust 路径：`task_base.rs`、`task_base_test.rs`、`task.rs`、`task_test.rs`、`base/task_base.rs`、`base_physical_plan.rs`、`lib.rs`；目标目录没有 `doc.go`。
- 已读边界/对照路径：`physicalop/Cargo.toml`、`task_base.go`、`task.go`。Cargo 的 `[package.metadata.porting]` 指向同一 Go package，且 `autotests = false`，测试由 `lib.rs` 的 `#[cfg(test)] mod task_base_test` 显式接入。
- 直接测试覆盖：活动计划计数切换；跨两个满额警告源合并不二次限流；Cop 转 Root 先同步 index 行数、保留 table 版本且不修改原任务。
- 本任务是纯文档分析，按计划未运行 Cargo；最终仅执行任务指定的 11 章节结构检查，并人工复核文档没有把 Go 注释中的未移植能力写成已支持。
