# `pkg/planner/core/rule/rule_partition_processor.rs`

## 文件定位

本文件属于 `astersql-planner-core-rule` crate（见同目录 `Cargo.toml`），模块由 `lib.rs` 以 `pub mod rule_partition_processor` 公开。它包含两层内容：前半部约 2200 行是用注释保存的 Go `rule_partition_processor.go` 对照文本；从 `use crate::rule_init::{...}` 开始才是可编译的 Rust 实现。后者针对 `rule_init.rs` 定义的紧凑 `Plan`/`Expr` IR，实现一个静态分区裁剪规则及分区区间集合工具。

当前生产规划器还有一条独立运行时接线：`pkg/planner/core/optimizer_runtime.rs` 的 `LogicalRule::PartitionProcessor` 位于 Go 对齐的规则序列中，并分派到 `rewrite_static_partitions`。仓库搜索和 RustCodeGraph 均未发现该运行时直接构造或调用本文件的 `PartitionProcessor`；本文件的规则结构体目前直接调用者是独立 Rust 测试。因此，应把这里视为紧凑 IR 规则实现与迁移对照，而不能把它误认为完整规划器的唯一分区裁剪入口。

## 核心职责

- `PartitionProcessor::optimize` 后序遍历紧凑计划树，仅处理携带 `PartitionInfo` 的 `PlanKind::DataSource`。
- 根据节点的全部 `predicates` 推导可能命中的分区下标集合；多个顶层谓词按 AND 语义求交，并继续与已有 `selected_partitions` 求交，保证规则重复运行不会扩大已选范围。
- 若集合非空，将其写回 `selected_partitions`；若集合为空，将该节点改写为 `PlanKind::TableDual { rows: 0 }`，清空子节点并把 `estimated_rows` 归零。
- 支持紧凑 IR 中的 Range/RangeColumns、Hash/Key、List/ListColumns 三类分区的保守裁剪；无法可靠理解的表达式返回全分区集合，优先保证不误删数据。
- 提供公开的 `PartitionRange`、`PartitionRangeOr`，用于半开区间 `[start, end)` 与区间并、交、离散集合压实。

这些职责由末段真实 Rust 符号实现；注释中的 Go 代码还描述了 UnionScan 重写、完整 TiDB 表达式求值、动态/静态分区模式及更复杂的 range/list/hash 算法，但它们并非本文件当前可执行 Rust 能力。

## 主要符号

- `PartitionRange { start, end }`：公开半开分区下标区间。字段使用 `usize`，不接受负下标。
- `PartitionRangeOr(Vec<PartitionRange>)`：公开区间并集包装；`full` 生成全集，`from_indices` 把有序离散下标压成连续区间，`indices` 展开，`intersect`/`union` 通过 `BTreeSet` 做集合运算后重新压实。
- `PartitionProcessor`：无字段规则类型，实现 `rule_init::LogicalRule`。`name()` 固定返回 `partition_processor`；`optimize(Plan)` 消费计划并返回 `(Plan, changed)` 或字符串错误。
- `prune_plan`：核心递归入口。先处理子节点，再处理当前 DataSource；汇总任一后代或当前节点是否变化。
- `validate_partition_info`：拒绝无分区定义或无分区列的元数据，分别返回 `partitioned table has no definitions`、`partitioned table has no partition columns`。
- `partitions_for_expr`：表达式分派器。识别 `and`、`or`、`in`、`is_null`、`eq/lt/le/gt/ge`；其它形状或非分区列条件返回全集。
- `comparison_partitions`：识别“分区列 op 常量”和“常量 op 分区列”；后者翻转大小比较符。
- `partitions_for_value`：按 `PartitionKind` 分派给 `range_partitions`、`hash_partitions` 或 `list_partitions`。
- `range_partitions`：依据每个定义的首个 `less_than` 值定位分区；类型不可比时保守返回全集。
- `hash_partitions`：仅裁剪等值；整数/无符号整数/布尔/浮点/文本各自映射为 `u64` 后对分区数取模，文本使用文件内固定的 FNV-1a 风格折叠。
- `list_partitions`：仅裁剪等值，且只比较每个 `in_values` 元组的第一个值。
- `full_set`、`compare_value`：分别生成下标全集，以及实现 Null、同型值和有符号/无符号整数间的有限比较。

文件没有条件编译项、模块级常量、异步函数或自定义 trait；可执行部分只有上述两个公开数据类型、一个公开规则类型及内部辅助函数。

## 执行流程

1. 调用者通过 `LogicalRule::optimize` 把紧凑 `Plan` 交给 `PartitionProcessor`。
2. `prune_plan` 先递归所有 `children`，所以修改顺序是后序；子树错误用 `?` 立即上抛，子树的 `changed` 以逻辑 OR 汇总。
3. 非 DataSource 节点以及 `partition: None` 的普通 DataSource 原样返回。
4. 对分区 DataSource，先克隆 `PartitionInfo` 和旧 `selected_partitions`，再校验至少有一个定义和一个分区列。
5. 选择集合从 `[0, definitions.len())` 全集开始；每个顶层谓词经 `partitions_for_expr` 转成候选集合并逐次求交。表达式内 `and` 求交、`or` 求并，`in` 把各常量的等值结果求并。
6. 若节点已有显式分区集合，再与推导结果求交。这是不扩大先前约束的关键不变量。
7. 空结果改写为零行 `TableDual`；非空且与旧集合不同则回写并置 `changed = true`；完全相同则保持幂等并返回原变化标志。
8. Range 定位选择首个“常量严格小于首个上界”的定义；Null 固定选择第 0 个分区。由于紧凑 IR 没有完整开闭界信息，`lt/le` 都保守保留从 0 到定位分区，`gt/ge` 都保守保留从定位分区到末尾。
9. Hash/Key 和 List/ListColumns 只有等值能缩小范围；其它比较返回全集。

## 数据与状态

规则没有跨调用状态：`PartitionProcessor` 是零大小类型，所有工作数据都在栈上或由 `Plan` 所有。主要状态是 `PlanKind::DataSource.partition`（只读元数据）、`selected_partitions: Option<BTreeSet<usize>>`（可写选择结果）、`predicates` 与 `children`。

`BTreeSet` 同时承担去重和确定性排序，因此输出分区下标稳定；`PartitionRangeOr::from_indices` 依赖这一升序迭代性质压实连续下标。区间方法当前通过“展开为集合—运算—重新压实”实现，语义直接但时间和空间开销与区间覆盖的分区数相关，而非只与区间个数相关。

`prune_plan` 为避开同时借用 `plan.kind` 与后续可变写回，先克隆分区元数据和旧选择集合。改写空结果时会丢弃当前 DataSource 的全部子节点，并把估算行数设为 0；其它 schema、predicates、keys、used_stats 字段留在节点上，但节点种类已变为 `TableDual`。

## 依赖与调用关系

可执行 Rust 代码的直接依赖很小：标准库 `std::cmp::Ordering`、`std::collections::BTreeSet`，以及同 crate 的 `rule_init::{Expr, LogicalRule, PartitionInfo, PartitionKind, Plan, PlanKind, Value}`。`Cargo.toml` 将本模块归入 `astersql-planner-core-rule`；crate 另有三个非条件依赖和一组 Windows 条件依赖，但本文件末段没有直接引用这些外部 crate。

RustCodeGraph 的关键边为：`rule_partition_processor.rs::optimize -> prune_plan`，`prune_plan -> validate_partition_info / partitions_for_expr / full_set`，`partitions_for_expr -> comparison_partitions / partitions_for_value / full_set`，并通过递归边再次调用自身。图工具还将部分常见标准库方法误解析到同名无关符号（例如 `clear`、`len`），这些不作为架构依据。

上游可见性来自 `pkg/planner/core/rule/lib.rs` 的公开模块声明。仓库文本搜索只找到 `pkg/planner/core/rule/rule_partition_processor_test.rs` 与 `rule_partition_pruning_test.rs` 直接实例化本文件的规则或区间类型。完整规划链由 `logical_plan_builder_runtime.rs` 设置 `FLAG_PARTITION_PROCESSOR`，`optimizer_runtime.rs` 按 `LOGICAL_RULES`/`LOGICAL_RULE_FLAGS` 运行规则，但该链的 PartitionProcessor 分支调用的是同文件中的 `rewrite_static_partitions`，不是这里的 `PartitionProcessor::optimize`。

## 错误处理与边界

- 唯一显式错误来自分区元数据校验；错误沿递归和 `optimize` 的 `Result` 原样传播，不做包装。
- 未识别函数、参数个数不符、比较两侧不是目标分区列与常量、谓词引用非分区列，以及 Range 边界与常量类型不可比时，均返回全集。这是防止错误裁剪的保守降级。
- `in` 中只要某个候选不是常量，就把全集并入结果，使整个 `in` 不再裁剪；空 `or` 会得到空集，空 `and` 保持全集。
- Range/RangeColumns 当前只读取 `less_than.first()`；List/ListColumns 只读取元组首值，因此多列分区只具有有限、保守或不完整的紧凑 IR 语义。
- Range 的 Null 固定映射到首分区；List 中 Null 只有显式出现在首值位置才匹配；Hash/Key 的 Null 哈希为 0。
- Hash/Key 的字符串、浮点映射是本文件自定义的紧凑 IR 行为，不能据此推断与 TiDB Go 表达式求值、collation 或 Key 分区编码完全一致。
- `validate_partition_info` 保证 Hash/Key 取模时分区数非零。`PartitionRangeOr` 本身不校验 `start <= end` 或下标上界，调用者必须提供有效区间。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。规则消费一个 `Plan`，在单次调用内递归可变处理，然后把所有权交还调用者；没有全局可变状态，因此不同计划使用独立 `PartitionProcessor` 值调用时不存在本文件内部的共享状态竞争。

递归深度与计划树高度一致；极深计划可能带来调用栈压力。集合和元数据克隆在每个分区 DataSource 上产生临时分配，离开函数即释放。空结果改写时，原 DataSource 的子计划随 `children.clear()` 释放；没有额外清理协议。

## 与 Go 版本的对应关系

`pkg/planner/core/rule/rule_partition_processor.go` 是语义来源。名称层面，Rust 保留了 `PartitionProcessor`、`PartitionRange` 以及区间 OR 概念；Rust 的 `PartitionRangeOr::{intersect, union}` 对应 Go 的 `PartitionRangeOR.Intersection/Union`，相邻或重叠区间都会压实。`rule_partition_pruning_test.rs` 复用了 Go `TestPartitionRangeOperation` 的典型并交案例。

核心目标也一致：在谓词下安全缩小分区范围，不能确认时保留全集，完全不命中时产生零行计划。独立测试 `rule_partition_processor_test.rs::incomparable_range_constant_aborts_pruning_like_go` 专门验证 Range 常量类型不可比时保留全部分区；`rule_partition_pruning_test.rs::range_partition_predicate_selects_canonical_partition` 验证 `col = 15` 在上界 10/20/30 中选择第二分区。

但当前 Rust 不是 Go 文件的等量移植。Go 实现操作真实 `base.LogicalPlan`、`logicalop.DataSource`、表元数据与 TiDB expression/ranger，覆盖 UnionScan/CTE、显式分区名、分区重组、range columns、单调函数、collation、删除中分区、plan-cache 标记等大量分支；Rust 末段只处理紧凑 IR 的少数表达式形状。文件顶部的大段注释仅是移植参考，不能作为“Rust 已接线”的证据。完整 Rust 规划器的实际静态分区改写位于 `optimizer_runtime.rs::rewrite_static_partitions`，后续对齐必须同时判断该运行时实现与本紧凑实现各自的职责，避免形成第三套分叉逻辑。

## 扩展指南

- 新增谓词形状时，从 `partitions_for_expr` 接入；必须让未知或求值失败路径保守返回全集，并在独立测试文件增加 AND/OR 嵌套、左右操作数翻转和非分区列用例。
- 改善某类分区算法时，分别修改 `range_partitions`、`hash_partitions` 或 `list_partitions`。多列 Range/List 需要先扩充 `Expr`/`PartitionInfo` 所能表达的边界与元组语义，不能仅用首值近似后宣称与 Go 等价。
- 若要使本规则进入完整规划器主链，应先厘清并复用 `optimizer_runtime.rs::rewrite_static_partitions`，而不是仅在规则列表中再构造本结构体；同时核对 `FLAG_PARTITION_PROCESSOR` 的顺序（当前在谓词下推之后），以免改变裁剪前提。
- 改区间运算应同步 `rule_partition_pruning_test.rs`；若从集合展开算法换成双指针区间算法，要保持半开区间、稳定升序、相邻区间合并和空区间排除不变量。
- 改规则行为应同步 `rule_partition_processor_test.rs` 或 `rule_partition_pruning_test.rs`，不要把测试嵌回生产源文件。完整 Go 语义对齐还应参照 `rule_partition_processor.go` 与 `rule_partition_pruning_test.go`，特别关注类型转换、collation、Null、显式分区名和错误降级。
- 性能风险集中在 `PartitionRangeOr` 展开大区间、每个 DataSource 克隆元数据，以及复杂谓词反复构造集合；兼容风险集中在哈希算法、开闭界、类型比较和多列分区语义。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标目录和目标 Rust/Go/测试文件均已索引。
- RustCodeGraph 查询：`query PartitionProcessor`、`query prune_plan`、`query partitions_for_expr`、`query PartitionRangeOr`，以及对 `prune_plan`、`partitions_for_expr` 的 `callers`/`callees`；确认了 `optimize -> prune_plan -> validate_partition_info/partitions_for_expr/full_set` 等直接边，并发现本文件结构体无生产调用者证据。
- 已读生产与装配文件：`pkg/planner/core/rule/rule_partition_processor.rs`、`rule_init.rs`、`lib.rs`、`Cargo.toml`，以及运行时入口 `pkg/planner/core/optimizer_runtime.rs` 和 flag 设置位置 `logical_plan_builder_runtime.rs`。
- 已读 Go 对照：`pkg/planner/core/rule/rule_partition_processor.go`；已检索并核对 `pkg/planner/core/rule/rule_partition_pruning_test.go` 的表达式裁剪和 `TestPartitionRangeOperation` 案例。
- 已读独立 Rust 测试：`pkg/planner/core/rule/rule_partition_processor_test.rs`、`pkg/planner/core/rule/rule_partition_pruning_test.rs`。它们分别覆盖类型不可比时的保守回退、Range 等值定位与区间并交压实。
- 本任务是只新增说明文档的分析任务，按计划不运行 Cargo；文档结构以任务指定的 11 个固定二级标题检查。
