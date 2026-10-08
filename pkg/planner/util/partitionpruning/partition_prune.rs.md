# `pkg/planner/util/partitionpruning/partition_prune.rs`

## 文件定位

本文件是 `astersql-planner-util-partitionpruning` crate 的核心实现，使用规划规则 crate 提供的精简表达式与分区元数据，在规划期把查询谓词转换为“需要访问的分区定义下标”。crate 根 `pkg/planner/util/partitionpruning/lib.rs` 公开模块并再导出全部公共项；`pkg/planner/util/partitionpruning/Cargo.toml` 将库入口设为 `lib.rs`，直接依赖只有 `astersql-planner-core-rule`。

当前 Rust 接线状态必须与算法能力分开理解：RustCodeGraph 把本文件列为 18 个符号，并显示直接使用方为 `partition_prune_test.rs` 和 `physical_insert.rs`；但进一步按符号搜索确认，`partition_pruning`、`PartitionedTable` 和 `FULL_RANGE` 的实际调用/构造只存在于 `pkg/planner/util/partitionpruning/partition_prune_test.rs`。`physical_insert.rs` 使用的是 `table::PartitionedTable` trait，并未调用本文件的同名结构或函数。根 workspace 和 `pkg/lib.rs` 暴露本 crate，`pkg/executor/Cargo.toml` 也声明依赖，但 `pkg/executor/*.rs` 尚无对该 API 的使用。因此它目前是“可独立测试、已公开、尚未接入 Rust 生产规划链”的移植实现，而不是当前 Rust SQL 请求必经路径。

## 核心职责

- `partition_pruning` 校验最小分区元数据，把多个顶层条件按 AND 语义求交，并应用显式分区名过滤与 DDL dropping-overlap 修正。
- `partitions_for_expr` 识别精简 IR 中的 `and`、`or`、`in`、`is_null`、`eq`、`lt`、`le`、`gt`、`ge`；无法安全识别的表达式保守返回全集，避免错误漏扫。
- `partitions_for_value` 按 `PartitionKind` 分派到 Hash/Key、Range/RangeColumns、List/ListColumns 三类定位算法。
- `handle_dropping_for_range` 与 `handle_dropping_for_list` 在分区 DDL 期间把正在删除的定义映射到仍有效的重叠定义，或剔除无替代定义的分区。
- 结果协议为：空向量表示没有匹配分区，普通非负值表示 `PartitionInfo.definitions` 的下标，单元素 `[FULL_RANGE]`（`-1`）表示全部定义。

这是一套基于 `rule_init.rs` 精简 `Expr`/`Value`/`PartitionInfo` 的局部算法，不等价于 Go `PartitionProcessor` 的完整表达式、类型、排序规则和执行上下文能力。

## 主要符号

- `pub const FULL_RANGE: isize = -1`：全分区哨兵，与 Go `rule.FullRange` 的返回约定对齐。
- `pub struct PartitionedTable`：包装 `PartitionInfo` 和 `overlapping_dropping: BTreeMap<usize, Option<usize>>`。映射缺项表示普通分区，`Some(replacement)` 表示 dropping 分区的替代下标，`None` 表示没有替代。
- `PartitionedTable::overlap_for`：内部查询上述三态映射；普通分区返回自身下标。
- `pub fn partition_pruning(...) -> Result<Vec<isize>, String>`：唯一公共剪枝入口；输入表视图、条件列表和可选分区名。
- `pub fn handle_dropping_for_range(...) -> Vec<usize>`：公共 Range dropping 修正入口，便于独立复用或验证。
- `handle_dropping_for_list`：List dropping 修正；映射后再按替代分区名过滤，并通过 `BTreeSet` 排序去重。
- `partitions_for_expr`：递归解释受支持的表达式形状；不支持或形状不合法时回退全集。
- `comparison_partitions`：只接受分区列与常量的二元比较；常量在左时翻转不等号；普通比较遇到 `NULL` 返回空集。
- `partitions_for_value`、`hash_partitions`、`range_partitions`、`list_partitions`：按分区类型定位候选下标。
- `full_set`：构造 `[0, count)` 的有序全集。
- `compare_value`：定义局部值序；支持同型值、`Int`/`UInt` 交叉比较以及 `NULL` 最小，其他跨类型组合返回不可比较。

本文件没有 trait、宏、条件编译项或模块级可变状态。

## 执行流程

1. `partition_pruning` 先拒绝空 `definitions` 或空 `columns`，因此后续 Hash 取模与列匹配都有有效前提。
2. 候选集合从所有定义下标开始。每个顶层 `condition` 经 `partitions_for_expr` 求出候选集合，再与当前集合求交，实现条件数组的 AND 语义。
3. 表达式递归中，`and` 求交、`or` 求并；空 `and` 保持全集，空 `or` 得到空集。无法识别的非标量表达式、函数、参数个数或非分区列条件返回全集。
4. `in` 要求首参数是分区列。常量逐项按等值定位，`NULL` 项不增加候选；出现非常量项时加入全集，整体保守退化。`is_null` 仅在唯一参数是分区列时将 `NULL` 交给分区类型算法。
5. 二元比较只处理“目标分区列与常量”。`constant op column` 会被规范化为 `column reversed_op constant`；`column op NULL` 按 SQL 普通比较未知结果处理为空集，而不是当作 `IS NULL`。
6. Hash/Key 仅对等值做单分区定位：`NULL` 为 0，整数取数值派生值，浮点取位模式，文本使用文件内固定的逐字节哈希；非等值返回全集。
7. Range/RangeColumns 使用每个定义 `less_than` 的第一个值，寻找首个上界严格大于常量的定义。等值返回落点；小于/小于等于取前缀；大于/大于等于取后缀。若常量不落在任何有限上界内，`lt`/`le` 返回全集而 `eq`/`gt`/`ge` 返回空集。`NULL` 的类型分派落到首分区，但普通比较的上层逻辑会先把 `NULL` 比较变为空集；`is_null` 才使用该规则。
8. List/ListColumns 仅对等值扫描各定义的 `in_values`，并只比较每个元组的第一个值；非等值返回全集。
9. 非 List 类型在 dropping 修正前按原始定义名做不区分 ASCII 大小写的过滤。Range 随后合并 dropping 连续区间并按需插入替代下标；List 则先映射替代下标，再按替代定义名过滤。
10. 修正后若选中数量等于定义总数，结果压缩为 `[FULL_RANGE]`；否则把有序 `usize` 下标转换为 `isize`。空集原样返回。

## 数据与状态

输入数据类型来自 `pkg/planner/core/rule/rule_init.rs`：`Expr` 只有列、常量、标量函数和 Cast 四种形态；`Value` 只有 Null、Bool、Int、UInt、Float、Text；`PartitionInfo` 只包含类型、分区列 ID 和定义数组；定义只保留 ID、名字、Range 上界及 List 值元组。因此本文件不持有完整表对象、计划上下文、SQL 类型信息、collation、时区或真实元数据生命周期。

所有候选集合均以 `BTreeSet<usize>` 表示，保证交并结果稳定排序并自然去重；dropping 映射使用 `BTreeMap`，List 修正再次经 `BTreeSet` 排序去重。`partition_pruning` 只借用输入，内部集合为调用栈内临时所有权，返回新建的下标向量，不修改表或表达式。

重要不变量包括：下标应指向 `definitions`；`FULL_RANGE` 只作为最终 `Vec<isize>` 协议出现，不进入内部 `usize` 集合；分区名匹配使用 `eq_ignore_ascii_case`；完整选择由“选中数量等于定义数量”判定。`overlapping_dropping` 的键和值由调用方保证合法，代码仅在名称读取处使用安全 `get`，Range 的替代下标在部分路径仍会参与顺序比较。

## 依赖与调用关系

下游依赖只有 `astersql_planner_core_rule::rule_init::{Expr, PartitionInfo, PartitionKind, Value}` 和标准库 `Ordering`、`BTreeMap`、`BTreeSet`。内部主调用边为：

`partition_pruning` → `full_set` / `partitions_for_expr` → `comparison_partitions` / `partitions_for_value` → `hash_partitions`、`range_partitions` 或 `list_partitions`；Range 路径还调用 `compare_value`。入口最后按类型调用 `handle_dropping_for_range` 或 `handle_dropping_for_list`，二者通过 `overlap_for` 解释 DDL 映射。

上游方面，`lib.rs` 公开并再导出符号；独立测试直接调用 `partition_pruning`。仓库搜索未找到 Rust 生产调用。Go 对照入口的真实上游包括 `pkg/planner/core/operator/physicalop/fragment.go`、`physical_index_scan.go` 和 `physical_utils.go`，以及 `pkg/planner/core/integration_test.go`；这些是理解预期应用位置的证据，但不能据此声称 Rust 入口已经接入。

## 错误处理与边界

入口只有两个显式错误，均用 `String` 返回：没有分区定义，或没有分区列。其余无法分析的情况遵循保守原则返回全集，而不是报错，包括非标量表达式、未知函数、参数形状不匹配、非目标列、Cast、列列比较、常量常量比较和不可比较的值类型。

需要特别注意以下边界：Hash/Key 非等值不剪枝；List 非等值不剪枝；多列 Range/ListColumns 目前只看首个上界/元组元素；文本比较是 Rust 字符串顺序，未注入 SQL collation；浮点 `NaN` 会使 `partial_cmp` 返回 `None`，Range 查找因而无法按正常序定位；负有符号整数 Hash 使用 `unsigned_abs`，文本 Hash 使用本地固定算法，这些都不是对 Go 表达式分区函数求值的完整复现。

独立测试 `partition_prune_test.rs` 固定了四组关键行为：严格 Range 上界、单分区全集压缩、超出有限末界不误选末分区、普通 `NULL` 比较与 `IS NULL` 的区别，以及 List dropping 替代与替代后名称过滤。测试尚未覆盖非法 dropping 下标、Hash/Text/Float、OR/IN 非常量、跨类型 Range 比较、多列分区或生产接线。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件、网络连接或其他外部资源。所有函数都是同步计算；共享输入通过不可变借用访问，局部容器由单次调用独占，因此本身没有并发写入或清理协议。若调用方跨线程共享 `PartitionedTable`，并发安全只取决于其不可变使用和字段类型，文件内没有缓存或全局状态。

计算成本主要来自表达式递归和集合运算：每个条件可能构造多个 `BTreeSet`；Range 与 List 定位为线性扫描定义；复杂 AND/OR 会重复分配集合；dropping 修正至多线性扫描并排序去重。扩展到大量分区或深表达式树时，应在保持保守语义的前提下评估分配和 `O(条件 × 分区数)` 成本。

## 与 Go 版本的对应关系

Go 同路径 `partition_prune.go` 的 `PartitionPruning` 是生产门面：Hash/Key 委托 `PartitionProcessor.PruneHashOrKeyPartition`，Range 调用 `PruneRangePartition`、`ConvertToIntSlice` 和 `handleDroppingForRange`，List 调用 `PruneListPartition`；未知类型返回 `rule.FullRange`。它接收 `PlanContext`、真实 `table.PartitionedTable`、完整 expression、分区名、列与输出名，错误类型也是 Go `error`。

Rust `partition_pruning` 保留了返回定义下标、空结果、全范围哨兵、显式分区名以及 Range dropping 合并等核心意图，但把 Go 各剪枝器内聚为基于精简 IR 的局部实现。Rust 还为 List 增加显式的 dropping 映射处理；Go 同文件的 List 分支直接委托规则层，相关行为位于其下游实现而非此门面。

`handle_dropping_for_range` 与 Go 函数的主要流程一致：无替代则跳过、普通分区保留、连续 dropping 区间归并到重叠定义、显式名称限制替代定义、最终全集压缩。差异是 Go 在函数内可将输入 `[FullRange]` 展开，并基于真实 `PartitionInfo` 查询 DDL 状态；Rust 内部从不把哨兵传入该函数，而由入口先持有完整下标集，并使用调用方预构造的 `overlapping_dropping` 映射。

因此迁移结论是“局部算法与测试已存在，但类型覆盖、SQL 语义和生产接线尚未达到 Go 门面的完整程度”。新增说明或调用时不得把 Go 生产调用边直接归属于 Rust，也不应以当前单元测试替代端到端剪枝验证。

## 扩展指南

- 增加表达式支持时，从 `partitions_for_expr` 和 `comparison_partitions` 接入；任何无法证明安全的分支必须返回全集，避免漏扫。若需要 Cast、函数分区表达式或 SQL 三值逻辑，应先扩充/复用真实表达式语义，而不是在字符串函数名上继续堆叠特例。
- 扩展分区类型或取值定位时修改 `partitions_for_value` 及对应 helper；多列 RangeColumns/ListColumns 必须按元组字典序和完整元组匹配，不能沿用当前“只看第一项”的简化。
- 调整 DDL dropping 行为时同步审查 `PartitionedTable::overlap_for`、两个 `handle_dropping_*` 函数，以及替代前后分区名过滤顺序；特别防止无效替代下标、重复下标和全集压缩误判。
- 接入生产规划链前，需要明确从真实表元数据/表达式到精简结构的无损转换，核对 `FULL_RANGE` 与空集消费者，并与 Go 的 `fragment.go`、`physical_index_scan.go`、`physical_utils.go` 调用时机逐一对齐。`pkg/executor/Cargo.toml` 中已有依赖不能视为接线完成。
- 测试必须放在独立的 `pkg/planner/util/partitionpruning/partition_prune_test.rs`，不要内嵌到源文件。优先补足 Hash/Key、AND/OR/IN、分区名大小写、Range/ListColumns 多列、dropping 异常映射和所有受支持 `Value` 类型；生产接线后还需增加规划器/执行器层回归。
- 性能改动应保持有序、去重和保守回退不变量，并用大量分区及深组合谓词评估集合分配；兼容性改动需同时对照 Go 返回下标顺序、DDL 中间态和 SQL 类型/collation 语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/util/partitionpruning` 找到 `lib.rs`、Go/Rust 对照源及 Rust 测试，目标文件含 18 个符号。
- RustCodeGraph `node --file pkg/planner/util/partitionpruning/partition_prune.rs --offset 1 --limit 500`：读取目标文件完整 373 行，核对全部常量、结构、impl、函数和内部流程。
- RustCodeGraph 对目标文件报告的文件级使用方为 `pkg/planner/core/operator/physicalop/physical_insert.rs` 与 `partition_prune_test.rs`；精确 `callers partition_pruning` 查询长时间无结果后终止。随后用符号搜索核验：前者只有 `table::PartitionedTable` trait，同名但不调用本 API；后者是当前唯一真实 Rust 调用者。此处以精确符号证据修正文件级近似关系。
- 已读 `pkg/planner/util/partitionpruning/lib.rs` 与 `Cargo.toml`，核对公开再导出、测试模块、crate 名、库入口、Go 包元数据和直接依赖；已查根 `Cargo.toml`、`pkg/lib.rs`、`pkg/executor/Cargo.toml` 的 facade/依赖声明。
- 已读 `pkg/planner/core/rule/rule_init.rs` 中 `Value`、`Expr`、`PartitionKind`、`PartitionDefinition`、`PartitionInfo`，核对本算法输入 IR 的真实字段与能力边界。
- 已读 Go 对照 `pkg/planner/util/partitionpruning/partition_prune.go` 全部 97 行，并搜索其真实调用者 `fragment.go`、`physical_index_scan.go`、`physical_utils.go` 与 Go integration test，核对生产门面和 dropping 流程。
- 已读独立 Rust 测试 `pkg/planner/util/partitionpruning/partition_prune_test.rs` 全部 160 行，核对现有回归覆盖及尚未覆盖的边界。
- 人工复核结论：本文件存在是为了提供 Rust 分区剪枝算法与 Go 迁移落点；运行时先解析谓词集合、按分区类型定位、处理 DDL 映射并编码结果；安全扩展必须保留保守全集回退、返回协议和独立测试，并在生产接线前补齐真实 SQL 语义差距。
