# `pkg/planner/core/operator/physicalop/tiflash_predicate_push_down.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`，模块由 [`lib.rs`](lib.rs) 以 `pub mod tiflash_predicate_push_down` 暴露。它位于 TiFlash 物理表扫描生成 Selection 的路径上，负责在规划期根据统计信息挑选值得提前执行的过滤条件；它不执行 SQL，也不访问 TiFlash 数据。

当前 Rust 文件包含两个层次。生产主链使用 `handle_physical_tiflash_late_materialization(&mut crate::PhysicalTableScan)`，其调用点在 [`base_physical_plan.rs`](base_physical_plan.rs) 的表扫描 Selection 构造流程中。文件后半部分的 `TiFlashTableScan`、`ColumnarIndex`、`IndexHint` 以及 `handle_tiflash_predicate_push_down` 是可独立测试的轻量兼容模型，覆盖 Go 的倒排索引提示与晚期物化决策，但仓库搜索未发现生产代码调用这个模型入口；不能把它描述成真实扫描节点已完整接入的倒排索引实现。

## 核心职责

1. `handle_physical_tiflash_late_materialization` 在真实 `PhysicalTableScan` 上实施晚期物化选择：检查 TiFlash、无序扫描、非空过滤、会话开关、真实直方图及表规模门槛，然后按所用列集合分组、估计选择率并贪心选择收益较高的谓词。
2. `CardinalityContextAdapter` 把 `base::PlanContext` 的会话、表达式和 ranger 上下文适配给 `cardinality::CardinalityContext`，使本模块能调用 `cardinality::Selectivity`。
3. `with_heavy_cost_function_expression` / `with_heavy_cost_function` 识别 JSON、日期时间、正则等高代价函数，避免把它们作为晚期物化的首批过滤条件。
4. 轻量模型入口 `handle_tiflash_predicate_push_down` 复刻 Go 的决策轮廓：解释 `USE`/`IGNORE`/`FORCE` 提示、选择单列公开倒排索引、筛选简单比较谓词、为剩余条件尝试晚期物化，并缩放行数估计。

三个共享启发式常量分别是 `SELECTIVITY_THRESHOLD = 0.6`、`TIFLASH_DATA_PACK_SIZE = 8192.0` 和 `COLUMN_COUNT_THRESHOLD = 3`。选择率越小越好；晚期物化收益按“预计过滤行数 × 未读取列数”衡量。

## 主要符号

- `CardinalityContextAdapter<'a>(&'a dyn base::PlanContext)`：私有借用适配器；三个 trait 方法直接转发 `GetSessionVars`、`GetExprCtx`、`GetRangerCtx`，不拥有上下文。
- `handle_physical_tiflash_late_materialization`：真实扫描节点的公开入口。输入为独占可变借用，成功时写入 `LateMaterializationFilterCondition`、`LateMaterializationSelectivity`，并按该选择率缩放 `StatsInfo.RowCount`。
- `with_heavy_cost_function_expression`：真实 `expression::Expression` 版本的高代价函数检查。`and`/`or` 仅在两个子项都重时才判重，`not` 递归检查唯一子项，这与 Go 保持一致。
- `HintType`、`IndexHint`、`ColumnarIndexKind`、`ColumnarIndex`、`TiFlashTableScan`：公开的轻量规划模型类型；真实生产扫描使用的是 crate 中的 `PhysicalTableScan`，两套类型不可混同。
- `transform_columns_to_code`：将有效非负列 ID 编成固定宽度 `0/1` 字符串；空集合特判返回 `"0"`。它只被轻量模型分组逻辑调用。
- `estimate_selectivity`：轻量模型从 `expression_selectivity` 按表达式 Debug 字符串取值，缺省为 `0.8`，将各值相乘并夹到 `[0, 1]`。这是测试替身，不是生产统计估算器。
- `group_by_columns_sort_by_selectivity`、`predicate_push_down_to_table_scan`：轻量模型内部的分组、过滤、排序和收益选择函数。
- `with_heavy_cost_function`、`is_predicate_simple_compare`：公开的轻量表达式分类函数；简单比较仅接受 `eq/ge/le/lt/gt/in` 及结构合法的 `and/or/not` 组合。
- `handle_tiflash_predicate_push_down`：轻量模型公开入口，返回 `Result<(), String>`；向量索引已在使用且存在谓词时返回错误。

文件没有条件编译项；测试模块的 `#[cfg(test)]` 声明位于相邻 `lib.rs`，测试逻辑保持在独立的 `tiflash_predicate_push_down_test.rs` 中。

## 执行流程

真实生产路径如下：

1. `base_physical_plan.rs` 在 TiFlash 表扫描构造下推 Selection 时，将子计划向下转型为 `PhysicalTableScan` 并调用 `handle_physical_tiflash_late_materialization`。
2. 入口先拒绝非 TiFlash、要求顺序、无过滤条件或未开启 `EnableLateMaterialization` 的扫描。
3. 它优先从 `TblColHists` 取得 `statistics::HistColl`，否则尝试 `stats_info().HistColl`；类型擦除对象无法下转型或 `RealtimeCount <= 8192` 时直接返回。
4. 每个非高代价条件通过 `ExtractColumnsMapFromExpressions` 提取列 ID；列 ID 排序后作为 `BTreeMap<Vec<i64>, ...>` 的确定性分组键，涉及列数超过 3 的条件被跳过。
5. 每组调用 `cardinality::Selectivity`。估算失败的组经 `.ok()` 被跳过；选择率大于 0.6 的组也被丢弃。候选先按选择率升序，再按组大小和逆序表达式哈希打破平局。
6. 贪心循环把候选组与已选条件合并并重新估算。若列数不增加，或预计过滤超过一个 pack 且调整后收益提高，则接受；若预计过滤行数小于一个 pack，则提前停止。
7. 有选中条件时写回晚期物化字段，将扫描行数乘以最终选择率。随后调用方读取这些条件，避免把同一条件重复留在上层残余 Selection 中。

轻量 `handle_tiflash_predicate_push_down` 的额外流程是：先应用扫描范围提示分数；只保留公开、单列、倒排索引；挑出选择率不高于 0.6 且所有列都有索引的简单比较；`FORCE` 可令未命中谓词的候选仍进入 `used_indexes`；对未被索引选中的剩余谓词运行晚期物化；最后缩放 `row_count` 并按索引 ID 排序，使解释结果稳定。

## 数据与状态

真实入口读取 `PhysicalTableScan` 的 `StoreType`、`KeepOrder`、`FilterCondition`、`TblColHists`、`Columns`、规划上下文和统计信息，写入两个晚期物化字段与统计行数。表达式均通过 `CloneExpr` 克隆；原始 `FilterCondition` 在本函数内不被移除，去重和残余条件分割由调用方完成。

分组使用有序容器和已排序列 ID，以避免哈希遍历的不确定性。排序平局继续比较表达式数量和 `HashCode`，保证相同输入得到稳定候选顺序。收益计算中的 `saturating_sub` 防止“涉及列数超过扫描列数”产生无符号下溢。

轻量模型把选择率存在 `BTreeMap<String, f64>` 中，把索引和过滤条件作为拥有所有权的值保存；它会原地追加 `used_indexes`，因此与 Go 一样可能保留重复索引。相同列存在多个合格索引时，`collect::<BTreeMap<_, _>>()` 让后出现者覆盖先出现者，这一行为由独立测试固定。

## 依赖与调用关系

上游生产调用边是 `base_physical_plan.rs` 的表扫描 Selection 构造逻辑 → `handle_physical_tiflash_late_materialization`。该调用之后读取 `LateMaterializationFilterCondition`，从残余条件中排除相等表达式，并把晚期过滤条件合并到扫描过滤集合。`physical_table_scan.rs` 还负责克隆、解释输出、索引解析和编码这些字段；`cache_snapshot.rs` 负责缓存快照保存/恢复。

真实入口的主要下游是：`base::PlanContext`、`planctx`/`rangerctx` 上下文接口，`statistics::HistColl`，`cardinality::Selectivity`，`expression::ExtractColumnsMapFromExpressions`、`Expression::CloneExpr`/`HashCode`，以及 `kv::StoreType::TiFlash`。`Cargo.toml` 将这些依赖分别声明为本地 workspace 包 `base`、`planctx`、`rangerctx`、`statistics`、`cardinality`、`expression`、`kv`；没有与本文件相关的 Cargo feature 开关。

RustCodeGraph 对真实入口识别到 `CloneExpr`、`HashCode`、三个阈值常量、`CardinalityContextAdapter` 和高代价函数检查等下游边；对轻量入口识别到 `is_predicate_simple_compare`、`predicate_push_down_to_table_scan`、`estimate_selectivity` 和阈值常量。图查询未正确给出生产上游，因此上游接线以 `rg` 定位并读取 `base_physical_plan.rs` 作为直接证据。

## 错误处理与边界

真实入口不返回错误：不适用场景、缺少可下转型的直方图、小表及无合格候选均为安全的“不做优化”。`cardinality::Selectivity` 的组级或合并级错误会跳过对应候选，不会阻断规划；这与 Go 记录 warning 后继续的控制流相近，但 Rust 当前入口不记录 warning。`partial_cmp` 遇到不可比较的浮点值时按相等处理。

真实入口只做晚期物化，未解释索引提示，也没有 Go 入口对“已用向量索引与谓词共存”的 panic 检查。轻量入口则把该情况转为 `Err("TiFlash does not support vector index with pedicates")`，保留了原字符串中的拼写；调用者必须处理错误。轻量选择率的非有限最终值回退到 0.6，但中间 `clamp` 和浮点比较仍应在扩展时谨慎处理 NaN。

列集合为空仍可形成候选组；真实入口以空 `Vec<i64>` 为键，轻量编码函数则返回 `"0"`。高代价函数的 `and`/`or` 使用“两个分支都重”才跳过的规则是 Go 既有语义，不应擅自改成任一分支为重即跳过。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部资源。所有决策在一次规划调用的当前线程内同步完成。`CardinalityContextAdapter` 的生命周期受借用的 `PlanContext` 限制；真实入口持有扫描节点的独占可变借用，因此写回过程中不能并发访问同一扫描对象。

候选表达式通过克隆取得独立拥有权，局部 `BTreeMap`、`BTreeSet` 和 `Vec` 在函数返回时释放。函数不持久化上下文引用，也不启动后台工作。主要资源风险是选择率的重复计算和表达式克隆成本，而非并发安全。

## 与 Go 版本的对应关系

直接对照文件是 [`tiflash_predicate_push_down.go`](tiflash_predicate_push_down.go)。常量、按列分组、0.6 选择率门槛、8192 行 pack 门槛、高代价函数清单、贪心收益公式、简单比较递归规则和稳定排序意图均来自 Go 实现。

Go 的 `handleTiFlashPredicatePushDown` 由 `PhysicalTableScan.BuildPushedDownSelection` 直接调用，完整处理倒排索引提示、晚期物化、统计缩放和索引排序。Rust 当前生产调用点位于 `base_physical_plan.rs`，只调用真实节点入口 `handle_physical_tiflash_late_materialization`；完整提示/倒排索引算法只在轻量 `TiFlashTableScan` 入口中复刻并被测试。因此，迁移状态是“真实晚期物化已局部接线，完整 Go 入口尚未以真实 Rust 扫描类型接线”。

另有两点可观察差异：Go 对选择率错误写日志，真实 Rust 入口静默跳过；Go 主入口遇到向量索引加谓词会 panic，轻量 Rust 入口返回 `Result::Err`。独立集成对照位于 `pkg/planner/core/casetest/tiflash_predicate_push_down_test.go` 和同目录 Rust 测试，覆盖 `TestTiFlashLateMaterialization`、`TestInvertedIndex` 的标准/Cascades 规范化计划；这些测试验证整体计划表现，不等于证明轻量模型已进入生产路径。

## 扩展指南

- 扩展真实晚期物化规则时，优先修改 `handle_physical_tiflash_late_materialization` 及 `with_heavy_cost_function_expression`，并同步独立的 `tiflash_predicate_push_down_test.rs`；若影响整体计划，还应同步 casetest 的 Go/Rust golden 对照。
- 若把倒排索引提示迁入真实生产路径，应以真实 `PhysicalTableScan`、真实 `ast::IndexHint`/索引元数据和 `cardinality::Selectivity` 为基础接线，而不是直接把轻量模型当生产数据结构；同时核对 `physical_table_scan.rs` 的构造、克隆、解释、PB 编码及计划缓存快照生命周期。
- 修改阈值、收益公式或排序平局规则会改变计划稳定性与性能，应覆盖 pack 边界、0.6 边界、相同选择率/组大小、估算失败、重复/同列索引以及列数超过 3 的用例。
- 增减高代价函数必须同步真实表达式与 `PhysicalExpr` 两个检查器，并与 Go `withHeavyCostFunctionForTiFlashPrefetch` 核对；不要把 Rust 测试内嵌进生产文件。
- 改动索引提示时需保持大小写归一化、`FORCE` 优先级、仅扫描作用域、公开单列倒排索引和同列后者覆盖等兼容规则，并评估重复索引是否仍需保留。

## 验证依据

- RustCodeGraph 状态：本仓库索引包含目标文件；`node --file ... --offset 420 --limit 700` 核对了全部实际 Rust 定义，`query` 定位两个公开入口与私有辅助函数，`callees` 核对了上述主要下游。调用者查询未返回有效上游边，故用源码搜索补证。
- 已读生产与装配文件：`pkg/planner/core/operator/physicalop/tiflash_predicate_push_down.rs`、`Cargo.toml`、`lib.rs`、`base_physical_plan.rs` 的直接调用点，以及 `physical_table_scan.rs`/`cache_snapshot.rs` 对晚期物化字段的消费者。该目录不存在 `doc.go`。
- 已读 Go 对照：`pkg/planner/core/operator/physicalop/tiflash_predicate_push_down.go` 和 `physical_table_scan.go` 的 `BuildPushedDownSelection` 调用点。
- 已读测试：`pkg/planner/core/operator/physicalop/tiflash_predicate_push_down_test.rs` 验证大表选择、小表/有序/无过滤门槛、高代价函数集合、同列索引覆盖和重复追加；`pkg/planner/core/casetest/tiflash_predicate_push_down_test.go` 与 `.rs` 验证晚期物化及倒排索引的规范化计划。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核生产入口与轻量模型没有混写。
