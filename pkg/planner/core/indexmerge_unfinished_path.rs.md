# `pkg/planner/core/indexmerge_unfinished_path.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 根由 `pkg/planner/core/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/planner/core/lib.rs` 通过 `pub mod indexmerge_unfinished_path` 将本模块公开。它处在 OR 型 IndexMerge 访问路径生成链中：上游 `pkg/planner/core/indexmerge_path.rs::generateOtherIndexMerge` 调用 `generateORIndexMerge`，本文件把过滤表达式转成若干候选索引的“未完成路径”，吸收顶层 AND 条件后物化为 `find_best_task::AccessPath`，并追加到 `IndexMergeDataSource.source.paths`。

这里的“未完成”不是异步状态，而是规划阶段的中间表示：单个 OR 分支可能只收集到了复合索引的部分可用列条件，尚不足以直接形成最终范围；后续顶层 AND 条件可能补齐它。文件既处理普通索引，也处理 MV（multi-valued，多值）索引。它不执行查询、不访问存储，也不负责把最终访问路径转换成物理执行算子。

## 核心职责

1. `generateORIndexMerge` 在隐式由 AND 连接的过滤列表中识别可拆成至少两个分支的 DNF 条件，并为每个顶层 OR 尝试生成一条 IndexMerge 候选路径。
2. `genUnfinishedPathFromORList` 与 `initUnfinishedPathsFromExpr` 建立“OR 分支 × 原始候选访问路径”的矩阵；矩阵槽位为 `None` 表示该索引不能处理该分支。
3. `mergeANDItemIntoUnfinishedIndexMergePath` 将 OR 外层的 AND 条件按候选索引位置补入每个分支，支持复合索引逐列收集条件。
4. `buildIntoAccessPath` 将每个分支的普通索引或 MV 索引替代方案真正构建成 `AccessPath`，按启发式选择每个分支的最优方案，检查是否确实构成 IndexMerge，并设置残余过滤与行数估计。
5. `cmpAlternatives`、`estimateCountAfterAccessForIndexMergeOR` 及三个私有辅助函数提供候选排序、粗略基数估算、索引身份判重和简化 CNF 拆分。

当前实现是可调用的 Rust 规划逻辑，但不是 Go 文件的逐字段完整复刻；尤其最终路径只保存已决定的 `partial_index_paths`，而 Go 版本还保存完整的 `PartialAlternativeIndexPaths` 供后续阶段选择。

## 主要符号

- `pub fn generateORIndexMerge(&mut IndexMergeDataSource, &[Expression]) -> Result<(), String>`：模块公开入口。它在开始时冻结 `candidate_count`，因此本轮新追加的 IndexMerge 路径不会反过来成为后续 OR 的基础候选；函数当前没有产生 `Err` 的分支，正常结束总是 `Ok(())`。
- `pub struct unfinishedAccessPath`：未完成路径状态。`path` 保存原候选；`usableFilters` 保存已收集条件；`idxColHasUsableFilter` 记录复合索引各列是否获得条件；`initedWithValidRange` 表示旧/直接路径构建已成功；`needKeepFilter` 表示最终仍须重检原 OR；`orBranches` 仅在外层容器语义下保存分支矩阵。
- `pub type unfinishedAccessPathList = Vec<Option<unfinishedAccessPath>>`：与 `candidates` 位置严格对齐的单分支候选列表。
- `genUnfinishedPathFromORList`：要求至少两个 OR 分支，且每个分支至少有一个可用候选，否则整体返回 `None`。
- `initUnfinishedPathsFromExpr`：对每个候选依次尝试普通索引直接构建、MV 过滤收集、逐索引列渐进收集三条路径；只有获得有效范围或至少一个可用索引列条件的槽位才保留为 `Some`。
- `handleTopLevelANDList`：跳过当前 OR 项，将其余顶层条件逐一初始化并合并，最后调用 `buildIntoAccessPath`。
- `mergeANDItemIntoUnfinishedIndexMergePath`：只按候选位置合并 `usableFilters`；若 AND 项对任何索引均不可用，则保持原 OR 中间态不变。
- `buildIntoAccessPath`：物化与筛选核心。普通索引调用 `accessPathsForConds`；MV 索引调用 `collectFilters4MVIndex` 和 `buildPartialPaths4MVIndexWithPath`。
- `cmpAlternatives`：优先选择全部为空范围/点范围的方案，再比较方案中最大的估算行数。
- `estimateCountAfterAccessForIndexMergeOR`：把已决定分支的非负 `count_after_access` 相加，并在表行数为正时以表行数封顶。
- 私有 `all_point_or_empty`、`max_row_count`、`index_identity`、`split_cnf`：分别支持排序、行数比较、物理索引判重及当前简化表达式模型中的 CNF 拆分。

## 执行流程

入口流程如下：

1. `generateORIndexMerge` 记录调用前的路径数，对每个过滤条件执行 `indexmerge_path::split_dnf`；不足两个分支的条件被忽略。
2. 它只克隆调用前已有的 `source.paths[..candidate_count]`，调用 `genUnfinishedPathFromORList`。任一 OR 分支完全找不到可用索引时，`?` 使该 OR 候选变为 `None`。
3. `initUnfinishedPathsFromExpr` 对普通索引先调用 `generateNormalIndexPartialPath`。成功时直接标记 `initedWithValidRange`，保存是否需要残余过滤，并停止处理该候选。
4. 若候选没有 `index` 元数据，该槽位立即为 `None`。否则表达式经 `split_cnf` 拆分；名称以 `root-only:` 开头的条件会设置 `needKeepFilter`。
5. 对 MV 索引，先用 `collectFilters4MVIndex` 识别多值 OR 或单值 MV 条件；有访问条件时记录有效范围，剩余条件会要求保留过滤。
6. 若直接路径仍未成功，则逐索引列调用 `checkAccessFilter4IdxCol`。每列最多取一个尚未收集的合格条件；存在未收集 CNF 项时保留源过滤。所有候选均无可用信息时，函数返回 `None`。
7. `handleTopLevelANDList` 对 OR 之外的每个条件重复上述初始化，并由 `mergeANDItemIntoUnfinishedIndexMergePath` 把可用过滤追加到所有位置对应的 OR 分支槽位。
8. `buildIntoAccessPath` 为每个 OR 分支构造候选方案。MV 条件可能展开成多条局部路径；普通索引方案是一条局部路径。某个分支没有任何可构建方案时，整个 IndexMerge 候选失败。
9. 每个分支用 `cmpAlternatives` 排序并取第一组，再扁平化为最终 `decided`。少于两条局部路径时失败；若完全不含 MV 路径且只涉及一个物理索引身份，也失败。
10. 最终 `AccessPath.partial_index_paths` 保存已决定路径，`table_filters` 默认保存 OR 外的顶层条件；若任何局部路径要求重检当前 OR，则把原 OR 条件也加入。`count_after_access` 使用各分支行数之和并受表行数上限约束。

## 数据与状态

核心不变量是候选位置对齐：`unfinishedAccessPathList[i]` 始终对应输入 `candidates[i]`。合并 AND 条件时只有列表长度相同才逐槽合并；长度不等的分支被跳过而不是按索引内容重新匹配。`orBranches.len()` 对应 OR 分支数，每个分支必须至少能物化一个方案，否则最终路径无效。

`unfinishedAccessPath` 有两种语义：当 `orBranches` 非空时，它是整个 OR 的容器；分支列表中的元素则表示单个候选索引的收集状态，通常持有 `path`、`usableFilters` 和列覆盖标志。代码依靠调用约定区分这两种语义，没有单独的 enum 标签。

`candidate_count` 防止同一轮新生成路径污染原始候选集合。候选和表达式通过 `clone` 复制到中间态，最终只有成功路径被追加到 `ds.source.paths`；失败尝试不会修改数据源。`index_identity` 用索引列、前缀长度以及 unique/global/multi-valued/vector 标志构造字符串；非 MV 路径必须出现至少两个不同身份才被接受。

## 依赖与调用关系

上游直接调用边由 `pkg/planner/core/indexmerge_path.rs` 给出：`generateOtherIndexMerge` 调用 `crate::indexmerge_unfinished_path::generateORIndexMerge`，再通过路径数量是否增长返回空字符串或 “No available filter or index for IndexMerge” 告警文案。`pkg/planner/core/lib.rs` 同时声明生产模块，并在 `#[cfg(test)]` 下用独立文件 `indexmerge_unfinished_path_test.rs` 挂载单元测试。

本文件的主要下游均来自同 crate：

- `find_best_task::AccessPath` 是输入候选、中间克隆和最终输出的数据载体。
- `indexmerge_path::IndexMergeDataSource` 提供源路径、列与表统计；`split_dnf`、`generateNormalIndexPartialPath`、`accessPathsForConds` 负责表达式拆分及普通索引路径生成。
- `isMVIndexPath`、`collectFilters4MVIndex`、`buildPartialPaths4MVIndexWithPath` 负责 MV 索引识别、过滤分类和局部路径展开。
- `checkAccessFilter4IdxCol` 及类型常量 `EQ_OR_IN_NON_MV_TP`、`MULTI_VALUES_OR_MV_TP`、`SINGLE_VALUE_MV_TP` 限定渐进收集允许的谓词类别。
- 标准库 `Ordering` 用于候选比较，`HashSet` 用于最终索引身份去重。

`pkg/planner/core/Cargo.toml` 表明该模块编入 `astersql-planner-core`，没有专属 feature gate；crate 的 `nextgen` feature 只转发配置依赖，本文件也没有条件编译项。源码中没有跨 crate 异步调用或 I/O 依赖。

## 错误处理与边界

多数“不适用”或“无法构建”都用 `Option::None` 表示，而不是错误：OR 少于两个分支、任一分支无候选、候选缺少索引元数据、MV 过滤为空、某分支无法物化、最终局部路径不足两条，或非 MV 方案只使用一个索引，都会静默放弃该 IndexMerge 候选。

`generateORIndexMerge` 的公开返回类型是 `Result<(), String>`，但当前函数体没有错误来源，因此只返回 `Ok(())`。更值得注意的是，`buildIntoAccessPath` 对 `buildPartialPaths4MVIndexWithPath` 使用 `let Ok(Some(...)) = ... else { continue; }`：下游 MV 构建错误与“没有结果”都会降级为跳过该候选，而不会传播到入口。扩展错误语义时不能只修改入口签名，还应决定这些候选级错误是否仍可降级。

排序中浮点数 `partial_cmp` 遇到 NaN 时按 `Ordering::Equal` 处理；行数估算先把每条负数裁为零。`orOffset` 由内部枚举产生时安全，但 `buildIntoAccessPath` 是公开函数，直接调用者必须保证它落在 `allConditions` 范围内，否则访问 `allConditions[orOffset]` 会 panic。`split_cnf` 只识别 `Expression.name` 的 `and:` 前缀并用 `|` 分隔，这是当前 Rust 简化表达式表示的约定，不等价于通用 SQL AST 的 CNF 拆分。

## 并发与资源生命周期

本文件完全同步，不创建线程、异步任务、通道、锁、事务或外部资源。所有中间容器都由函数局部 `Vec`、`Option` 和 `HashSet` 持有，函数返回或循环结束后按 Rust 所有权规则释放。

唯一持久状态变更发生在 `generateORIndexMerge`：成功构建的 `AccessPath` 被顺序追加到调用者提供的 `&mut IndexMergeDataSource`。可变借用保证同一时刻本函数独占修改数据源；候选阶段使用克隆快照，避免在遍历原路径时发生别名修改。该实现没有内部并行探测；“IndexMerge 并行探测索引”是后续执行层概念，不应解释成本文件存在并发行为。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/indexmerge_unfinished_path.go`。Rust 保留了 Go 的主要阶段与同名符号：冻结初始候选、逐 OR 分支收集、用顶层 AND 补条件、普通/MV 两路物化、点/空范围优先、非 MV 单索引拒绝，以及最终行数估算。独立 Rust 测试 `pkg/planner/core/indexmerge_unfinished_path_test.rs` 还明确验证顶层 AND 条件会合入已有效的 OR 局部路径，这与 Go 的宽松收集策略一致。

但以下差异必须视为当前迁移边界：

- Go 使用真实 `expression.Expression`、`logicalop.DataSource`、表元数据和下推上下文；Rust 使用 `task::Expression` 的简化字段及 `and:`、`or:`、`root-only:` 名称协议。
- Go 会检查表达式能否下推 TiKV，并清理/保留局部路径上的 `TableFilters`、`IndexFilters`；Rust 主要通过 `needKeepFilter` 和最终 `table_filters` 表达残余过滤，没有完整复刻这些清理规则。
- Go 最终保存所有 `PartialAlternativeIndexPaths`，这里只在规划阶段用比较器选定每个分支的一组 `partial_index_paths`，可能减少后续再选择空间。
- Go 用真实索引 ID（表路径为 `-1`）判重；Rust 用 `index_identity` 的索引属性字符串判重。
- Go 对非 MV 使用基数选择率、对 MV 使用专门选择率计算，并在估算错误时退回 `SelectionFactor`；Rust 只把分支 `count_after_access` 求和并以表行数封顶，`cmpAlternatives` 的 `_tableRows` 参数也未使用。
- Go 会因计划缓存参数决定是否保留 OR 源过滤；Rust 只根据局部路径的残余过滤需求决定是否把 OR 放入 `table_filters`。
- Go 的 MV 构建会拒绝特定 intersection 组合并记录更丰富状态；Rust 的 `buildPartialPaths4MVIndexWithPath` 接口和错误降级更简单。

因此，安全扩展应以 Go 文件作为语义目标，同时以当前 Rust 类型能力为边界；不能仅凭函数同名断言两端已经完全等价。

## 扩展指南

- 新增谓词类别时，首先同步 `indexmerge_path.rs::checkAccessFilter4IdxCol`、相关类型常量和 `initUnfinishedPathsFromExpr` 的允许集合；同时扩展独立的 `indexmerge_unfinished_path_test.rs`，不要把测试嵌入生产文件。
- 修改复合索引渐进收集时，保持候选列表按位置对齐、每列最多选取一个未收集 CNF 项，以及未覆盖条件触发残余过滤这三个不变量；应增加“部分列由 OR、其余列由顶层 AND 补齐”的回归用例。
- 调整候选选择或估算时，修改 `cmpAlternatives`、`max_row_count` 和 `estimateCountAfterAccessForIndexMergeOR`，并覆盖空范围、唯一索引点范围、带 index filter 的行数、NaN/负数及表行数封顶。若追求 Go 对齐，应先补齐选择率和替代方案数据结构，不能把当前求和估算当成等价实现。
- 修改残余过滤时，联动检查 `initUnfinishedPathsFromExpr` 的 `needKeepFilter`、MV `remaining`、普通路径的 `table_filters` 与最终 OR 重检逻辑，重点防止漏过滤导致错误结果；这是高正确性风险区域。
- 改变 `index_identity` 时要防止不同物理索引被错误合并或同一索引被误判为多个；若 `AccessPath` 后续提供稳定索引 ID，应优先与 Go 的 ID 判重方式对齐。
- 让错误可观察时，应明确区分“候选不适用”和“构建失败”，再决定是否把 `buildIntoAccessPath` 改为 `Result<Option<AccessPath>, _>` 并通过 `generateORIndexMerge` 传播；这属于兼容性变化。
- 性能上应留意当前候选快照克隆、每个过滤对每个候选/索引列的嵌套扫描以及候选列表排序。优化时必须保持候选快照隔离和比较器排序语义。

## 验证依据

本说明直接读取并核对了以下文件：生产实现 `pkg/planner/core/indexmerge_unfinished_path.rs`；crate 声明 `pkg/planner/core/Cargo.toml`；模块与测试装配 `pkg/planner/core/lib.rs`；上游入口 `pkg/planner/core/indexmerge_path.rs::generateOtherIndexMerge`；Go 对照 `pkg/planner/core/indexmerge_unfinished_path.go`；独立 Rust 测试 `pkg/planner/core/indexmerge_unfinished_path_test.rs`。在 `pkg/planner/core` 根目录没有同包 `doc.go`；检索到的 `pkg/planner/core/base/doc.go` 属于 `base` 子包，不作为本模块契约。

Rust 测试提供的具体行为证据包括：`top_level_and_uses_the_same_collection_path_as_go` 验证顶层 AND 合并；`non_mv_or_requires_more_than_one_physical_index` 验证非 MV 单索引拒绝；`unique_point_ranges_beat_lower_row_count_scans` 验证点范围优先于更低行数扫描；`final_path_keeps_top_level_and_filters_but_not_covered_or` 验证最终残余过滤；`mv_or_alternative_expands_each_json_value` 验证 MV OR 可展开多条局部路径。未发现直接引用这些同名函数的 Go `*_test.go`；Go 语义依据来自同路径生产实现，Rust 边界依据来自上述独立 Rust 测试。

RustCodeGraph `status` 显示索引可用（11,467 个文件、307,296 个节点、1,848,419 条边）。`query generateORIndexMerge --json` 和 `query buildIntoAccessPath --json` 均定位到本文件及对应 Go 符号；但 `files --filter pkg/planner/core/indexmerge_unfinished_path` 返回 “No files found”，精确 `callers/callees` 查询也未能产生可信的限定调用边（按节点 ID 查询发生错误匹配）。因此调用关系使用 `rg` 和直接源码读取复核，不把错误图结果当作证据。本文档任务不改变运行时代码，按计划不运行 Cargo；最终结构验证要求文档存在且恰有上述 11 个固定二级标题。
