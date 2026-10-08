# `pkg/planner/core/stats.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate（见 `pkg/planner/core/Cargo.toml`），由 crate 根 `pkg/planner/core/lib.rs:197` 以私有模块 `mod stats` 挂载，并在 `pkg/planner/core/lib.rs:244` 通过 `pub use stats::*` 将其中的公开类型和函数重导出到 crate 根。它依赖同一 crate 的轻量计划树模型 `PlanKind` 与 `PlanNode`（定义于 `pkg/planner/core/common_plans.rs:55`、`:222`），提供一组统计摘要结构和确定性的简化推导函数。

从当前调用证据看，它不是 Rust 规划器完整生产统计链路的实现：RustCodeGraph 将该文件的直接使用者仅定位到 `pkg/planner/core/stats_test.rs`，仓库文本检索也只发现这些 Rust API 在该测试文件中被调用。对应的 Go 文件 `pkg/planner/core/stats.go` 则接入真实逻辑计划、直方图、基数估计、range 构造和访问路径裁剪。因此，本文件当前更准确的角色是统计语义的轻量兼容模型与测试辅助层，而非 Go 实现的等价全量移植。

## 核心职责

- 用 `StatsInfo` 表示计划算子的估算行数、逐列 NDV 和列组 NDV（`stats.rs:26-33`）。NDV 是不同值数量估计；本文件的计算函数目前只填充 `row_count`，不会推导两个 NDV 字段。
- 用 `StatsAccessPath` 抽象访问路径在 range 访问后、普通索引过滤后的行数，以及表路径、IndexMerge、强制 hint 三类路径属性（`stats.rs:37-48`）。
- 用 `DataSourceStats` 保存轻量数据源的表行数、选择率、候选路径和派生摘要（`stats.rs:52-61`）。
- 对 `PlanNode` 子树执行自底向上的固定公式行数推导，并把结果写回每个节点（`RecursiveDeriveStats4Test`，`stats.rs:64-107`）。
- 提供 range 展示截断、路径最小选择率汇总和按过滤条件数估算数据源行数三个小型辅助函数（`stats.rs:109-142`）。

## 主要符号

`StatsInfo`（`stats.rs:26`）是可克隆、可比较且可默认构造的统计值对象。`row_count: f64` 是估算输出行数；`column_ndv: Vec<f64>` 按输出列保存 NDV；`group_ndv: Vec<(Vec<usize>, f64)>` 以列下标集合和 NDV 表示列组统计。默认值使行数为 `0.0`、两个向量为空。

`StatsAccessPath`（`stats.rs:37`）描述选择率汇总所需的最小路径信息。`table_path` 决定是否采用 `count_after_access`；`partial_index_paths` 非空代表 IndexMerge，也采用 `count_after_access`；普通索引路径采用 `count_after_index`。`forced` 记录路径是否被 hint 强制保留。该类型递归包含自身，但递归发生在 `Vec` 堆分配中，因此类型大小有限。

`DataSourceStats`（`stats.rs:52`）聚合 `table_rows`、`selectivity`、`paths` 和 `stats`。当前 `deriveStatsByFilter` 只读写前三者中的 `table_rows`、`selectivity` 以及 `stats.row_count`；`paths` 由调用者维护，不在该函数内消费。

`RecursiveDeriveStats4Test(&mut PlanNode) -> (StatsInfo, bool)`（`stats.rs:64`）递归更新整棵轻量计划树，返回当前节点的新统计和“当前节点估算是否改变”。函数名以及 Go 对照 `pkg/planner/core/stats.go:47-50` 都明确它是测试导出入口。

`pruneEstimateRange(&[Vec<String>], usize) -> Vec<Vec<String>>`（`stats.rs:110`）复制每个 range 的至多前 `keep` 个字符串元素，不修改输入。

`getGeneralAttributesFromPaths(&[StatsAccessPath], f64) -> (f64, bool)`（`stats.rs:118`）返回所有路径中的最小选择率与是否存在强制路径。

`deriveStatsByFilter(&mut DataSourceStats, usize) -> StatsInfo`（`stats.rs:138`）按条件数量使用 `0.8^conditions`，更新数据源选择率和行数，并返回更新后的统计副本。

## 执行流程

`RecursiveDeriveStats4Test` 首先以 `iter_mut` 遍历全部子节点，递归推导并收集每个子节点返回的 `StatsInfo`（`stats.rs:65-70`），因此执行顺序是后序、自底向上。随后按当前 `PlanKind` 选择公式（`stats.rs:72-97`）：

1. `TableScan` 与 `DataSource` 使用节点已有 `estimated_rows`，但最低钳制为 `1.0`。
2. `Selection` 取第一个子节点行数，并为每个条件乘一次默认选择率 `0.8`；缺少子节点时基数从 `1.0` 起算。
3. `Limit` 先扣除 `offset`，下限钳制为零，再以 `count` 为上限；缺少子节点时得到零。
4. `Join` 与 `HashJoin` 将所有子节点行数相乘，乘积先至少钳制为 `1.0`，再乘 `0.1`。所以空子节点列表的乘积按 Rust 迭代器规则为 `1.0`，结果为 `0.1`；函数没有强制二叉 Join 形状。
5. `Aggregation`、`HashAgg`、`StreamAgg` 取第一个子节点行数的平方根并至少钳制为 `1.0`；无子节点时为 `1.0`。
6. 其他算子透传第一个子节点的行数；若没有子节点，则保留当前节点原有 `estimated_rows`。

得到 `rows` 后，函数以与旧值之差是否大于 `f64::EPSILON` 计算当前节点的 `changed`，再写回 `plan.estimated_rows`，返回仅填充 `row_count` 的新 `StatsInfo`（`stats.rs:98-106`）。`changed` 不聚合子树的变更标志：子节点发生变化而当前节点最终数值不变时，当前返回值仍可为 `false`。

`getGeneralAttributesFromPaths` 从 `(1.0, false)` 开始扫描路径（`stats.rs:119-134`）。仅当 `total_rows > 0.0` 时计算比值；表路径和 `partial_index_paths` 非空的 IndexMerge 路径使用 `count_after_access / total_rows`，普通索引使用 `count_after_index / total_rows`，并持续取最小值。`forced` 的汇总与行数是否合法无关，只要任一路径为真就返回真。

`deriveStatsByFilter` 先计算 `0.8_f64.powi(conditions as i32)`，再令 `stats.row_count = table_rows * selectivity`，最后克隆整个 `StatsInfo` 返回（`stats.rs:138-142`）。

## 数据与状态

所有状态都由调用者拥有并通过普通 Rust 值传递；文件中没有全局变量、静态缓存或内部单例。`RecursiveDeriveStats4Test` 原地修改 `PlanNode.estimated_rows`，并通过临时 `Vec<StatsInfo>` 保存直接子节点的推导结果。它不会修改节点的代价、运行时行数或其他 `PlanNode` 字段（字段全集见 `pkg/planner/core/common_plans.rs:222-239`）。

`deriveStatsByFilter` 原地修改 `DataSourceStats.selectivity` 与 `DataSourceStats.stats.row_count`，保留既有 `column_ndv`、`group_ndv` 和 `paths`。返回值是 `source.stats.clone()`，后续修改返回值不会反向修改数据源。

所有数量均为 `f64`，代码没有对 `NaN`、无穷值、负的输入行数或负的路径计数做显式校验。`conditions` 和 `keep` 是 `usize`；前者在调用 `powi` 前以 `as i32` 转换，极端大值存在截断语义。以上是类型和实现边界，不应被解释为对异常统计输入的业务保证。

## 依赖与调用关系

上游装配关系为 `pkg/planner/core/lib.rs` → `mod stats` → `pub use stats::*`。因此外部 crate 可以从 `astersql_planner_core` 根访问公开符号，尽管模块本身不是 `pub mod`。当前仓库内可验证的 Rust 调用边是：

- `pkg/planner/core/stats_test.rs:52,58` → `RecursiveDeriveStats4Test`；函数内部在 `stats.rs:68` 递归调用自身。
- `pkg/planner/core/stats_test.rs:69` → `deriveStatsByFilter`。
- `pkg/planner/core/stats_test.rs:87-98` → `getGeneralAttributesFromPaths`。
- `pkg/planner/core/stats_test.rs:100-103` → `pruneEstimateRange`。

下游源码依赖仅为 `crate::{PlanKind, PlanNode}`（`stats.rs:22`），实际定义来自被 crate 根重导出的 `common_plans.rs`。本文件不直接使用 `Cargo.toml` 中的外部依赖，也不受 `nextgen` feature 或条件编译控制；`Cargo.toml` 的 `autotests = false` 表明测试通过 `lib.rs:427-429` 的显式 `#[cfg(test)]` 模块挂载，而不是 Cargo 自动发现。

Go 生产调用链明显更宽：`pkg/planner/core/stats.go:111-165` 的数据源推导会初始化真实表统计、填充/裁剪访问路径、生成 IndexMerge，再调用 Go 版 `getGeneralAttributesFromPaths`；`deriveStatsByFilter` 也被逻辑表扫描、逻辑索引扫描和数据源推导调用（`stats.go:60,84,144`）。这些边不能外推为当前 Rust 文件已经接入同一生产主链。

## 错误处理与边界

本文件所有函数都返回普通值，没有 `Result`/`Option` 错误通道，也没有日志或回退错误记录。边界行为由默认值和浮点运算决定：空路径返回 `(1.0, false)`；`total_rows <= 0.0` 时不计算任何选择率但仍汇总 `forced`；`keep == 0` 时每个输出 range 都成为空向量；`keep` 大于 range 长度时完整复制该 range。

递归推导只读取第一个子节点来处理 Selection、Limit、Aggregation 以及默认透传分支；它不验证算子元数。扫描节点把非正、`NaN` 之外可比较的估算值至少钳制为 `1.0`，但 `f64::max` 和后续公式对特殊浮点值的行为未由测试建立契约。深度极大的计划树使用同步递归，源码没有显式深度限制或栈保护。

与 Go 版相比，Rust 的 `pruneEstimateRange` 只截断字符串向量。Go 版还会修正被截断边界的开闭属性，并调用 `ranger.UnionRanges` 合并相同前缀，且可能返回错误（`pkg/planner/core/stats.go:511-535`）；因此 Rust 函数不能用于替代 Go 的真实估算 range 语义。Rust 的 `deriveStatsByFilter` 也不会调用基数估计器，因而没有 Go 版在估计失败时记录调试日志并退回默认选择率的错误分支（`stats.go:644-655`）。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道、事务或外部资源。所有借用仅持续到函数返回；临时子统计向量和 range 克隆由 Rust 所有权自动释放。`&mut PlanNode` 与 `&mut DataSourceStats` 保证单次调用期间对目标值的独占访问，但这不是跨线程同步机制。

递归函数会在父节点计算前完成对子节点的可变借用；`StatsAccessPath.partial_index_paths` 和 `PlanNode.children` 都拥有其子值，没有共享引用生命周期。若上层希望并行推导，必须在本文件之外对计划子树划分所有权并处理结果合并；当前 API 和实现均为串行。

## 与 Go 版本的对应关系

`RecursiveDeriveStats4Test` 与 Go 同名入口只有“供测试触发递归推导”的意图一致。Go 版直接调用逻辑计划对象的 `RecursiveDeriveStats(nil)`，返回 `(*property.StatsInfo, bool, error)`（`pkg/planner/core/stats.go:47-50`）；Rust 版作用于轻量 `PlanNode`，采用本文件固定公式，且不返回错误。Go 的 `pkg/planner/core/casetest/stats_test.go:35-124` 使用该入口验证真实优化计划的 GroupNDV 传播；Rust 的 `pkg/planner/core/stats_test.rs:26-60` 只验证 Selection、Limit 和聚合的数值链路，测试名虽包含 Join，但当前测试体没有构造 Join。

`getGeneralAttributesFromPaths` 是最接近逐分支移植的函数：Rust 与 Go 都在总行数为正时区分表/IndexMerge 路径和普通索引路径，取最小选择率，并独立汇总 `forced`（Rust `stats.rs:118-135`；Go `stats.go:537-555`）。Rust 用轻量 `StatsAccessPath` 替代 Go `util.AccessPath`。

`deriveStatsByFilter` 仅保留“根据过滤条件更新数据源统计”的外形。Go 版通过 `cardinality.Selectivity` 结合会话、直方图、表达式和已填充路径估计选择率，失败时采用 `cost.SelectionFactor`，再缩放完整表统计（`stats.go:644-655`）；Rust 版只按条件个数应用固定 `0.8`。Rust 测试 `stats_test.rs:64-71` 固化了三条件得到 `0.512`、10000 行得到 5120 行的当前行为。

`pruneEstimateRange` 的名称和“保留前缀列”目的对应，但数据模型和正确性处理并不等价：Go 版处理 `ranger.Range`、边界包含性、collator、range 合并和错误；Rust 版只对 `Vec<String>` 做 `take(keep)`。Rust 测试 `stats_test.rs:100-103` 仅覆盖两个字符串 range 保留一个元素。

`StatsInfo`、`StatsAccessPath`、`DataSourceStats` 也是轻量本地模型，不等同于 Go 的 `property.StatsInfo`、`util.AccessPath`、`logicalop.DataSource`。特别是 Go 统计包含直方图、版本、列唯一 ID 映射等生产信息（例如 `stats.go:606-635`），当前 Rust 结构未承载这些数据。迁移状态应描述为“局部语义覆盖”，不能声称 Go/Rust 完整对齐。

## 扩展指南

若只是增加轻量计划算子的启发式公式，应在 `RecursiveDeriveStats4Test` 的 `match` 中加入明确分支，并在独立文件 `pkg/planner/core/stats_test.rs` 添加对应算子、空子节点、多子节点和 `changed` 语义用例；不要把测试内嵌进 `stats.rs`。新增分支前还应检查 `PlanKind` 的真实形状与调用方是否确实使用轻量 `PlanNode`。

若扩展路径属性，应同步检查 `StatsAccessPath`、`getGeneralAttributesFromPaths` 和测试中的三类路径：表路径、普通索引、`partial_index_paths` 非空的 IndexMerge。涉及零/负总行数、负计数、比值大于 1 或 `NaN` 时，必须先明确兼容契约，避免无意改变当前 `min` 和跳过分支的浮点行为。

若目标是继续移植 Go 生产统计逻辑，不应在现有固定公式上直接宣称对齐。应先接入真实的逻辑计划、`property::StatsInfo`、基数估计、统计表和 ranger 类型，再逐段对照 `pkg/planner/core/stats.go` 的错误传播和不变量；尤其要保留 `TableStats.RowCount >= stats.RowCount >= CountAfterAccess` 的 Go 数据源语义说明（`stats.go:100-110`），以及 `pruneEstimateRange` 对开闭边界和重复前缀合并的处理。此类改动会影响规划正确性和性能，需要对应的独立 Rust 回归测试，并与 Go `pkg/planner/core/casetest/stats_test.go` 的真实计划测试意图对齐。

性能方面，当前递归每个节点都会分配一个直接子统计 `Vec`，range 截断会克隆字符串，`deriveStatsByFilter` 会克隆统计向量。若计划树或 range 很大，可在保持行为与测试的前提下评估减少分配；不能通过删除 NDV 数据或跳过子节点推导来换取表面性能。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11467 个文件、307296 个节点和 1848419 条边；`files --filter pkg/planner/core/stats` 将目标识别为含 17 个符号的 Rust 文件。
- RustCodeGraph 源码/符号查询：读取 `pkg/planner/core/stats.rs:1-142` 全文；查询到 `StatsInfo`、`StatsAccessPath`、`DataSourceStats`、`RecursiveDeriveStats4Test`、`pruneEstimateRange`、`getGeneralAttributesFromPaths`、`deriveStatsByFilter` 均定义于该文件；目标文件的索引使用者为 `pkg/planner/core/stats_test.rs`。
- 装配与类型证据：读取 `pkg/planner/core/lib.rs:1-476`，确认 `mod stats`、`pub use stats::*` 和显式 `stats_test` 挂载；读取 `pkg/planner/core/common_plans.rs:45-304`，确认 `PlanKind` 变体与 `PlanNode` 字段、构造器。
- crate 证据：读取 `pkg/planner/core/Cargo.toml`，确认包名、`autotests = false`、默认/`nextgen` features、依赖和 `go-package = "pkg/planner/core"` 的移植元数据；目标文件本身只引用 crate 内类型。
- Rust 测试证据：读取 `pkg/planner/core/stats_test.rs:1-104`，确认递归推导、三条件过滤、普通/表/IndexMerge 路径、空路径、零总行数以及字符串 range 截断的断言。
- Go 对照证据：读取 `pkg/planner/core/stats.go:1-180,480-699`，核对同名函数的生产调用、range 语义、路径汇总和真实选择率估计；读取 `pkg/planner/core/casetest/stats_test.go:1-130`，核对 Go 测试入口和 GroupNDV 验证。`pkg/planner/core/stats_test.go:1-240` 主要覆盖索引路径裁剪，不直接调用本文件对应的轻量 Rust API。
- 调用边补充：RustCodeGraph 的精确 `callers` 查询在本次会话中未返回结果，故使用仓库范围的 `rg` 对四个函数名复核；结果显示 Rust 直接调用仅位于 `pkg/planner/core/stats_test.rs`，Go 同名调用位于 `pkg/planner/core/stats.go` 及 `pkg/planner/core/casetest/stats_test.go`。文档没有将 Go 调用边归属于 Rust 实现。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核本文件的定位、流程、边界、迁移差异和安全扩展入口均有上述源码依据。
