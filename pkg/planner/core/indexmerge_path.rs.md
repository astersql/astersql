# `pkg/planner/core/indexmerge_path.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate，是 Rust 侧 IndexMerge 候选访问路径的生成与清理模块。crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod indexmerge_path` 暴露该模块；[`Cargo.toml`](Cargo.toml) 的 `[package.metadata.porting]` 将整个 crate 对应到 Go `pkg/planner/core`。

它位于“已有普通表/索引 `AccessPath`”与“后续从候选中选择物理任务”之间：输入是 [`find_best_task.rs`](find_best_task.rs) 定义的简化 `DataSource`、`AccessPath`、`IndexInfo` 和 `Range`，输出仍写回 `DataSource.paths`。文件既处理普通索引，也处理多值索引（MV Index）的 `MEMBER OF`、`JSON_CONTAINS`、`JSON_OVERLAPS` 模型，并对 Hint 和全文索引（FTS）约束做末端裁剪。

当前接线状态必须区别于 Go：Rust 的公开入口 `generateIndexMergePath` 在仓库 Rust 生产代码中没有调用点（对 `generateIndexMergePath(` 和 `IndexMergeDataSource` 的 Rust 搜索只命中定义、测试及 `indexmerge_unfinished_path.rs` 的辅助调用）；[`operator/logicalop/logical_datasource.rs`](operator/logicalop/logical_datasource.rs) 约第 1117 行另有一段直接生成顶层 DNF union 候选的逻辑。因此，本文件是可测试、被相邻 OR 模块复用的移植实现，但尚不能等同于 Go 规划主链已经整体接入。

## 核心职责

- `generateIndexMergePath` 统一编排 OR、单 MV 索引 AND、MV/普通索引组合 AND 三类候选生成，并在 Hint 生效时裁掉普通候选。
- `generateNormalIndexPartialPath` 与 `accessPathsForConds` 把可下推条件变成普通索引局部路径；向量索引、空条件或无法形成范围的条件不会产生候选。
- `generateMVIndexMergePartialPaths4And`、`generateANDIndexMerge4MVIndex` 和 `generateANDIndexMerge4ComposedIndex` 从 MV 过滤提取值，构造一个或多个局部点范围，再组装 union/intersection 路径。
- `collectFilters4MVIndex`、`checkAccessFilter4IdxCol`、`jsonArrayExpr2Exprs` 等函数负责识别和转换简化表达式协议。
- `cleanAccessPathForMVIndexHint` 与 `cleanAccessPathForFTS` 在生成后执行约束裁剪；前者只在至少找到一个匹配的合并路径时替换候选集合，后者要求 FTS 最终有 TiFlash 路径。
- `indexmerge_unfinished_path.rs` 通过本文件的普通/MV 局部路径辅助函数物化 OR 分支，本文件则通过 `generateOtherIndexMerge` 回调其 `generateORIndexMerge`，两者共同组成 OR 路径流程。

## 主要符号

- `pub type Result<T> = std::result::Result<T, String>`：本模块的简化错误边界；目前只有字符串错误，没有结构化错误类型。
- `UNSPECIFIED_FILTER_TP`、`EQ_OR_IN_NON_MV_TP`、`MULTI_VALUES_OR_MV_TP`、`MULTI_VALUES_AND_MV_TP`、`SINGLE_VALUE_MV_TP`：过滤分类协议。`checkAccessFilter4IdxCol`、`collectFilters4MVIndex`、MV 路径构造及 OR 未完成路径模块共享这些值。
- `IndexMergeHint { indexes: Vec<usize> }`：以数字下标表达允许参与合并的索引；空列表表示不限定索引。
- `IndexMergeDataSource`：在 `DataSource` 外增加 IndexMerge 开关、Hint、临时表标志、告警列表与 FTS 索引集合。它拥有 `source`，所有生成和清理都原地修改 `source.paths`、统计信息或告警状态。
- `generateIndexMergePath(&mut IndexMergeDataSource) -> Result<()>`：模块总入口。
- 普通索引族：`generateNormalIndexPartialPath`、`accessPathsForConds`、`generateNormalIndexPartialPath4And`、`generateANDIndexMerge4NormalIndex`。
- MV/组合族：`generateMVIndexMergePartialPaths4And`、`generateANDIndexMerge4MVIndex`、`generateANDIndexMerge4ComposedIndex`、`buildPartialPathUp4MVIndex`、`buildPartialPaths4MVIndexWithPath`、`buildPartialPaths4MVIndex`、`buildPartialPath4MVIndex`。
- 识别与转换族：`PrepareIdxColsAndUnwrapArrayType`、`collectFilters4MVIndex`、`CollectFilters4MVIndexMutations`、`checkAccessFilter4IdxCol`、`jsonArrayExpr2Exprs`、`jsonValue2Expr`、`unwrapJSONCast`、`isSafeTypeConversion4MVIndexRange`、`isMVIndexPath`。
- 清理族：`cleanAccessPathForMVIndexHint`、递归的 `indexMergeContainSpecificIndex`、`cleanAccessPathForFTS`。
- 私有辅助：`merge_path`、`uncovered`、`expr_hash`、`is_pushable`、`split_cnf`、`filter_values`、`range_from_filter`；`split_dnf` 为 `pub(crate)`，供 OR 模块使用。

文件没有 trait、`impl`、条件编译项或模块级可变静态状态。命名保留了较多 Go 风格大小写，crate 根的 `#![allow(non_snake_case)]` 允许这些移植名称。

## 执行流程

1. `generateIndexMergePath` 先检查总开关：既未启用又无 Hint、存在 `no_index_merge_hint`，或数据源是临时表时，不生成路径。若存在 Hint，退出前会清空无效 Hint 并把原因加入 `warnings`。
2. 入口把调用前 `source.paths.len()` 记为 `regular`，并克隆 `source.conditions`，确保后续追加候选时只遍历原始普通路径。
3. `generateOtherIndexMerge` 调用 [`indexmerge_unfinished_path.rs`](indexmerge_unfinished_path.rs) 的 `generateORIndexMerge`。后者拆分 `or:` 表达式，逐分支寻找普通/MV 局部路径，合并顶层其余 AND 条件，再为各分支选择候选并追加 union IndexMerge。
4. `generateANDIndexMerge4MVIndex` 遍历原始 MV 路径。它用 `PrepareIdxColsAndUnwrapArrayType` 验证索引列，用 `collectFilters4MVIndex` 分类可访问与剩余过滤，再由 `buildPartialPaths4MVIndexWithPath` 为过滤值生成点查 `Range`；`JSON_CONTAINS` 类使用 intersection，其他类别按 union 组装。
5. `generateANDIndexMerge4ComposedIndex` 先收集 MV 局部路径和已用过滤，再从显式 Hint 指定的普通索引收集非全范围路径。总局部路径超过一条时，使用 `uncovered` 保存未覆盖表过滤，并追加组合 IndexMerge。
6. 如果 IndexMerge 使用了比 `pushed_down_conditions` 更多的条件，入口取新增候选中最大的 `count_after_access`，仅在原 `stats.row_count` 更大时把它下调，避免上层估算比合并路径还大。
7. 有 Hint 且生成成功时，入口优先保留新生成的组合路径，否则保留所有本轮新增 IndexMerge 路径；随后依次执行 MV Hint 与 FTS 清理。

局部 MV 构造的内部顺序是：`filter_values` 从 `name:values` 文本取值，`buildPartialPaths4MVIndex` 为每个值创建 `eq:<value>` 表达式，`range_from_filter` 把可解析整数写成 `Datum::Int`、其他值写成 `Datum::Bytes`，最后克隆基路径并替换访问条件、范围及行数估算。`SINGLE_VALUE_MV_TP` 在处理第一条访问过滤后停止。

## 数据与状态

主要可变状态集中在 `IndexMergeDataSource`：

- `source.paths` 同时容纳输入普通路径和追加的 IndexMerge 路径；`partial_index_paths` 非空是 [`find_best_task.rs`](find_best_task.rs) 中 `AccessPath::is_index_merge` 的判据。
- `source.conditions` 是本模块考虑的完整过滤集合，`source.pushed_down_conditions` 只用于判断是否需要修正统计行数。
- `index_merge_hints` 既控制候选可见性，也承担“Hint 无法应用”后的清空语义；`warnings` 保存相应诊断。
- `fts_indexes` 在本实现中只作为“启用 FTS 清理”的非空标志，集合元素不参与具体路径匹配；实际保留条件是 `AccessPath.store == Some(StoreType::TiFlash)`。
- `used: HashMap<String, Expression>` 用 `expr_hash` 的“小写名称 + 列下标”作为去重键，决定哪些过滤已被局部路径消费以及哪些应留在 `table_filters`。
- `merge_path` 在 intersection 时将行数估算设为各局部路径最小值，在 union/非 intersection 时求和；它没有单独字段记录 union/intersection，语义只能由调用处和过滤分类推断。

这里的表达式和类型均是 Rust 迁移层的简化模型：[`task.rs`](task.rs) 的 `Expression` 主要以 `name` 字符串编码函数及值，`FieldType` 只有有限 `TypeCode`；`Range` 和 `AccessPath` 也比 Go 对象少大量元数据。因此字符串前缀及分隔符是当前实现的不变量，改变它们会同时影响识别、范围生成和测试。

## 依赖与调用关系

上游与入口关系：

- crate 根 [`lib.rs`](lib.rs) 公开模块，并在 `#[cfg(test)]` 下以独立文件挂载 [`indexmerge_path_test.rs`](indexmerge_path_test.rs)。
- RustCodeGraph 的文件节点显示本文件被 `indexmerge_unfinished_path.rs` 及相关测试引用；源码核对确认 OR 模块调用 `generateNormalIndexPartialPath`、`accessPathsForConds`、`collectFilters4MVIndex`、`buildPartialPaths4MVIndexWithPath`、`checkAccessFilter4IdxCol`、`isMVIndexPath` 和 `split_dnf`。
- 当前没有 Rust 生产调用者调用总入口 `generateIndexMergePath`。Rust 规划数据源中的现有 DNF union 接线位于 [`operator/logicalop/logical_datasource.rs`](operator/logicalop/logical_datasource.rs)，没有使用 `IndexMergeDataSource`。
- Go 主链则由 [`stats.go`](stats.go) 约第 152 行调用 Go `generateIndexMergePath`，随后约第 155 行单独调用 Go `cleanAccessPathForFTS`。

下游依赖：

- [`find_best_task.rs`](find_best_task.rs)：`DataSource`、`AccessPath`、`IndexInfo`、`Range`、`Datum`。
- [`task.rs`](task.rs)：`Expression`、`FieldType`、`TypeCode`、`StoreType` 与 `StatsInfo`。
- [`indexmerge_unfinished_path.rs`](indexmerge_unfinished_path.rs)：OR IndexMerge 的未完成路径建立、AND 条件合并及候选选择。
- 标准库 `HashMap`/`HashSet`：过滤去重、指定索引集合和 FTS 标记。

虽然 [`Cargo.toml`](Cargo.toml) 声明了 expression、statistics、logicalop、planner-util 等完整规划器依赖，本文件源码当前只直接使用同 crate 简化类型和标准集合；不能仅凭 crate 依赖声明推断本文件已使用完整 Go 等价组件。

## 错误处理与边界

- 禁用、`NO_INDEX_MERGE` 或临时表不是硬错误；有 Hint 时转换为 warning 并清空 Hint，无 Hint 时只返回 `Ok(())`。
- OR 生成的错误、MV 局部路径构造错误和 FTS 清理错误使用 `?` 向上传播。当前 MV 构造大多用 `Ok(None)` 表示不适用，真正明确的硬错误主要来自 `cleanAccessPathForFTS`：FTS 激活却没有 TiFlash 路径。
- `accessPathsForConds` 对空条件、Hint 排除、无法解析出任何范围返回 `None`；候选索引为 vector 时 `generateNormalIndexPartialPath` 直接拒绝。
- `PrepareIdxColsAndUnwrapArrayType` 在要求 MV 但索引不是 MV、或索引列与表列无交集时返回 `None`。与函数名相比，它并未操作真实数组类型，只过滤数字列下标。
- `checkAccessFilter4IdxCol` 要求列匹配，并只识别 `member-of:`、`json-contains:`、`json-overlaps:`、`eq:`、`in:`。空 JSON 值列表不可访问；单元素 contains/overlaps 会降为 `SINGLE_VALUE_MV_TP`。
- `isSafeTypeConversion4MVIndexRange` 只比较本地 `eval_type` 家族：Int/UInt、String/Bytes 分别兼容，而 Float 与 Decimal 属于不同家族；长度、小数位和 unsigned 标志不参与判断。
- `jsonValue2Expr` 不接受 `Null`、`Vector`，数值解析失败返回 `None`；`jsonArrayExpr2Exprs` 把任一元素转换失败提升为带原值的字符串错误。
- MV Hint 清理有意保守：若没有任何匹配指定索引的合并路径，则保留原路径集合，避免 Hint 清理把全部候选删除；该不变量由独立单元测试覆盖。
- `filter_values` 是逗号分隔的简化解析器，不是 JSON parser；包含嵌套、转义逗号等复杂值的真实 JSON 语义并未由此实现。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁、事务或 I/O。调用者以独占 `&mut IndexMergeDataSource` 驱动入口，Rust 借用规则保证同一调用期间不会并发修改候选集合。

资源生命周期完全是内存内、单次规划调用级别：入口克隆条件和原始 MV 路径以避免在向 `source.paths` 追加时发生迭代失效；生成的局部路径通过克隆拥有自己的表达式、范围和索引元数据。Hint 清理使用 `drain` 或整体替换 `Vec`，被裁路径随即释放。错误返回不会回滚此前已经追加或裁剪的路径；目前 FTS 清理会先 `retain` 再在空集合时返回错误，因此调用者若继续观察对象，会看到已被清空的路径集合。

## 与 Go 版本的对应关系

对应文件是 [`indexmerge_path.go`](indexmerge_path.go)。Rust 保留了主要函数族和大体顺序：总入口生成其他/单 MV/组合 AND 路径，必要时修正统计，Hint 成功后裁剪；MV 路径仍围绕过滤分类、值展开、安全类型转换、局部路径及剩余过滤展开。Go 独立测试 [`casetest/indexmerge/indexmerge_path_test.go`](casetest/indexmerge/indexmerge_path_test.go) 验证真实 SQL 规划、过滤 mutation、随机 MV 数据、Hint 与 prepared statement；Rust 对应测试文件保留了其中一部分稳定核心。

关键差异如下：

- Go 使用真实 `logicalop.DataSource`、表达式树、表/索引元数据、ranger、直方图选择率和 session/statement context；Rust 本文件使用 `IndexMergeDataSource` 与字符串编码表达式，范围和行数估算是简化算法。
- Go Hint 按索引名且不区分大小写；Rust Hint 按 `source.paths` 数字下标，`indexMergeContainSpecificIndex` 又用局部路径首列下标和 Hint 数字集合匹配。这是本地模型约定，不应描述为完整索引名语义。
- Go `accessPathsForConds` 同时支持 table path、common/int handle、真实统计派生与 full-range 判断；Rust 通过候选克隆和简易点范围完成。
- Go `collectFilters4MVIndex` 按复合索引列序逐列选择访问条件，导出的 mutation API 会枚举同一 MV 列的多条件组合；Rust `collectFilters4MVIndex` 只围绕第一列及首个确定分类分组，`CollectFilters4MVIndexMutations` 仅把该结果追加到输出，并未返回 Go 的 MV 列偏移和 mutation 集合。
- Go MV 构造检查虚拟数组列、JSON path 对应关系、空数组正确性、类型转换、索引过滤与直方图选择率；Rust 以表达式名称拆值并用固定除法/求和/最小值估算。
- Go 总入口不调用 FTS 清理，而是由 `stats.go` 紧接着调用；Rust 总入口内部调用 `cleanAccessPathForFTS`。
- Go 规划主链已调用入口；Rust 总入口尚无生产调用点，且 `logical_datasource.rs` 的现有 DNF union 逻辑只覆盖部分能力。

因此，本文件应理解为按 Go 结构移植的、可独立验证的简化实现，而不是 Go 行为逐项等价或已完整替换当前 Rust 规划路径。

## 扩展指南

- 若接入 Rust 生产主链，优先确定 `operator/logicalop::DataSource` 与本文件简化 `find_best_task::DataSource` 的唯一数据模型，避免并存两套 IndexMerge 生成器；接线点应在普通候选范围和统计已经形成、物理路径枚举之前。同步扩展独立测试，而不是把测试写进本文件。
- 新增过滤函数或改变表达式编码时，应同步检查 `checkAccessFilter4IdxCol`、`filter_values`、`split_cnf`/`split_dnf`、`range_from_filter`、`collectFilters4MVIndex` 与 `indexmerge_unfinished_path.rs`，并在 [`indexmerge_path_test.rs`](indexmerge_path_test.rs) 增加分类/边界回归，在 [`casetest/indexmerge/indexmerge_path_test.rs`](casetest/indexmerge/indexmerge_path_test.rs) 增加 SQL 级结果对照。
- 扩展真实复合 MV 索引时，不能只修改 `PrepareIdxColsAndUnwrapArrayType`；还需补齐按列序匹配、唯一数组虚拟列约束、JSON path、空数组、mutation 和剩余过滤语义，并以 Go 同名函数为行为基准。
- 调整 Hint 时要保持三个层次一致：候选是否允许、组合普通路径是否显式指定、生成后递归检查是否包含指定索引。尤其应先定义数字是“路径下标、索引 ID 还是列 ID”，消除当前不同函数中的隐含解释。
- 调整行数估算或 union/intersection 表示时，应给 `AccessPath` 增加明确语义字段，并同步下游成本选择；当前仅靠最小值/求和不足以承载 Go 的直方图选择率。
- 修改 FTS 清理应覆盖“混合 TiKV/TiFlash”“只有 TiKV”“无 FTS 标记”三种情况，并考虑错误前是否允许破坏原候选集合。
- 性能风险集中在条件/路径的多次克隆、MV 值展开造成的局部路径乘积、递归索引检查以及组合候选膨胀；正确性风险集中在未覆盖过滤丢失、union/intersection 混淆、空数组和不安全类型转换。

## 验证依据

本说明基于以下直接证据编写：

- 目标源码 [`indexmerge_path.rs`](indexmerge_path.rs) 全部 667 行；RustCodeGraph `node --file` 分段读取确认常量、结构体、公开/私有函数及内部调用。
- RustCodeGraph `status`：索引存在，包含 11,467 个文件、307,296 个节点、1,848,419 条边；`query generateIndexMergePath --json` 同时定位 Go 第 47 行与 Rust 第 57 行；对组合生成、MV 构造、过滤收集和清理函数的查询确认两种语言中的对应符号。
- RustCodeGraph 文件关系显示目标文件与 `indexmerge_unfinished_path.rs`、`indexmerge_path_test.rs`、`indexmerge_unfinished_path_test.rs` 相互关联；随后以源码节点核对了 OR 模块对本文件辅助函数的直接调用。通用名称的 `callers`/`callees` 输出出现跨仓库同名污染，因此没有把其模糊结果当作调用事实，而以限定文件节点和精确源码引用复核。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的包名、库根、feature、依赖及 Go 包映射；[`lib.rs`](lib.rs) 的模块声明和独立测试挂载。
- 数据模型：[`find_best_task.rs`](find_best_task.rs) 的 `Datum`、`Range`、`IndexInfo`、`AccessPath`、`DataSource`；[`task.rs`](task.rs) 的 `StoreType`、`TypeCode`、`FieldType`、`Expression`、`StatsInfo`。
- Go 对照：[`indexmerge_path.go`](indexmerge_path.go) 的入口、普通/MV/组合路径、过滤、Hint 与 FTS 清理；[`stats.go`](stats.go) 的实际入口调用顺序。
- Rust 测试：[`indexmerge_path_test.rs`](indexmerge_path_test.rs) 覆盖单元素/空 MV 数组分类、类型家族、嵌套路径递归、MV Hint 保守清理及 FTS；[`casetest/indexmerge/indexmerge_path_test.rs`](casetest/indexmerge/indexmerge_path_test.rs) 覆盖过滤分类/收集、多个 MV 值展开和 SQL 结果骨架。Go 对照测试为 [`casetest/indexmerge/indexmerge_path_test.go`](casetest/indexmerge/indexmerge_path_test.go)。
- 仓库搜索确认 Rust 总入口无生产调用，并定位 `logical_datasource.rs` 的独立 DNF union 实现；这构成“尚未接线”结论的直接证据。

本任务是纯文档分析，未运行 Cargo 或代码测试。结构验收使用任务指定命令，要求文件存在且恰有上述 11 个固定二级标题。
