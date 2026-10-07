# `pkg/planner/cardinality/selectivity.rs`

## 文件定位

本文件是 `astersql-planner-cardinality` crate 的谓词选择率估算实现。crate 根 [`lib.rs`](lib.rs) 以私有模块 `mod selectivity` 装入它，再通过 `pub use selectivity::*` 向规划器公开符号；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `pkg/planner/cardinality`。选择率定义为过滤后行数与过滤前行数之比，主要入口 `Selectivity` 接收 CNF 条件、`statistics::HistColl` 和已填充的访问路径，供规划器将基础表统计缩放为过滤后的行数。

RustCodeGraph 将本文件识别为 33 个符号，并显示它被 8 个文件使用。当前可核实的生产上游包括：`pkg/planner/core/operator/logicalop/logical_datasource.rs::DeriveStats`（派生数据源统计）、`pkg/planner/core/operator/physicalop/tiflash_predicate_push_down.rs`（按选择率组织 late materialization 条件）、`pkg/planner/core/operator/physicalop/index_join_probe.rs`（反推 index join probe 各阶段行数），以及同 crate 的 `row_count_index.rs`（复用等值、越界与最后桶启发式）。本文件不是独立执行单元，不拥有 SQL、存储或统计收集入口。

## 核心职责

- `Selectivity` 把相关列、普通列直方图、主键、普通索引和多值索引候选统一成 `StatsNode`，用位掩码表示候选覆盖的谓词，再用贪心规则选取互不重叠的统计集合。
- `getMaskAndRanges`、`findPrefixOfIndexByCol` 将谓词交给 ranger 提取 access condition/构造范围，也可复用调用方已填充的 `AccessPath`；DNF 只部分下推时保留 `partCover` 信息。
- 对统计未覆盖的条件，主入口分别处理常量、DNF、字符串匹配和其它表达式：能精确或辅助估算时消费对应位，否则乘会话级默认选择率。
- `GetSelectivityByFilter` 在单列 stats v2 上用 TopN、直方图边界/桶重复数及 NULL 样本执行实际过滤表达式，提供 LIKE、ILIKE、REGEXP 等字符串匹配的辅助估算。
- `getEqualCondSelectivity`、`outOfRangeEQSelectivity`、`outOfRangeFullNDV`、`crossValidationSelectivity` 提供索引等值、统计范围外值和多列索引交叉验证，供 `row_count_index.rs` 复用。
- `CalcTotalSelectivityForMVIdxPath` 组合多值索引 partial path：交集相乘，并集使用容斥公式；每条路径的结果夹在 `[0, 1]`。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `Selectivity(ctx, coll, exprs, filledPaths) -> Result<f64, Error>` | 公共主入口。要求 `exprs` 表示 CNF 的各项；建立候选统计节点、贪心去重覆盖并处理剩余谓词。 |
| `StatsNode` | 公共迁移结构，保存范围、节点类型、统计 ID、谓词位掩码、选择率上下界、列数及 DNF 部分覆盖信息。`IndexType`、`PkType`、`ColType` 标识节点类型。 |
| `GetUsableSetsByGreedy` / `statsNodeForGreedyChoice::isBetterThan` | 对节点稳定排序并选取不重叠集合；优先级依次是非普通列统计、覆盖位数、完整 DNF 覆盖、DNF 最少 access 条件数、更少索引列、更低选择率。 |
| `getMaskAndRanges` | 对列或索引条件构造 `ranger::Ranges`，返回覆盖 mask、DNF 部分覆盖标记和 `MinAccessCondsForDNFCond`。不支持的 `RangeType` 会 panic。 |
| `findPrefixOfIndex` / `findPrefixOfIndexByCol` | 按 `UniqueID` 找连续索引前缀；存在 cached path 时改用 `EqualByExprAndID` 对齐表达式索引列。 |
| `CalcTotalSelectivityForMVIdxPath` / `getMaskAndSelectivityForMVIndex` | 计算 MV Index partial paths 的总选择率，并把抽出的 access conditions 映射回原谓词 mask。 |
| `GetSelectivityByFilter` / `prepareFilterForStatsEvaluation` | 将表达式副本中的列索引递归归一化为 0，在单列 chunk 上向量化执行 TopN、Histogram 和 NULL 样本。 |
| `findAvailableStatsForCol` | 优先选择完整加载且有效的列统计；否则选择无前缀长度的完整单列索引统计。 |
| `getEqualCondSelectivity` / `crossValidationSelectivity` | 唯一索引全列点查直接按一行估算；其它点查结合索引计数、逐列点范围和范围外启发式。 |
| `outOfRangeEQSelectivity` / `outOfRangeFullNDV` | 对 ANALYZE 后新增/删除导致的范围外值平滑估算；`outOfRangeBetweenRate`（100）作为 NDV 下限。 |
| `IsLastBucketEndValueUnderrepresented` | 当修改量、新增行规模和最后桶上界同时满足阈值时，识别最后值 Repeat 可能被陈旧统计低估。 |
| `CollectFilters4MVIndex` / `BuildPartialPaths4MVIndex` | 可变全局可选回调，保持 Go 为规避 import cycle 的注入形状；当前 Rust 仓库中初值为 `None`，未找到生产赋值。 |
| `BuildPartialPaths4MVIndexResult` | 承载 Go 多返回值的临时结构；partial path 使用 `'static` 引用，是未来接线必须审查的生命周期边界。 |

辅助常量 `unknownColumnID`、`staleLastBucketThreshold`（0.3）和 `valueAwareRowAddedThreshold`（0.5）分别服务于列/常量识别和最后桶启发式；`compareType`、`getConstantColumnID`、`isColEqCorCol` 保留 Go 的判定顺序。

## 执行流程

`Selectivity` 的主流程如下：

1. `RealtimeCount == 0` 或没有条件时返回 `1.0`。条件超过 63 个（mask 仅为 `i64`）或没有列/索引统计时调用 `pseudoSelectivity`，并记录 `TiDBOptSelectivityFactor`。
2. 先分离 `column = correlated_column`，用该列 NDV 的倒数估算；无效统计或 NDV 非正时回退 `1 / pseudoEqualRate`。这些条件不再进入 ranger。
3. 提取并按 schema `ID` 排序去重列。隐藏虚拟列仅使用索引统计，JSON 列因 ANALYZE 不建立普通列统计而跳过。普通列/handle 经 `getMaskAndRanges` 和 `GetRowCountByColumnRanges` 形成 `ColType`/`PkType` 节点；缺失的非隐藏列统计会记录 used-stats 状态。
4. 按索引 ID 稳定遍历统计。MV Index 尝试走注入回调；普通索引先找可用前缀，再复用 cached path 或调用 ranger 构造范围，最后由 `GetRowCountByIndexRanges` 生成 `IndexType` 节点及最小/最大估计。
5. `GetUsableSetsByGreedy` 仅选择 mask 与当前未覆盖位无冲突的候选。每个节点的选择率乘入结果；DNF 仅部分覆盖时另乘会话 `SelectivityFactor`。
6. 将未覆盖条件分类。非 plan-cache 过度优化的常量可直接判真、假或 NULL；假/NULL 令乘积为零。DNF 在所有相关列均有统计且可拆成多个分支时递归调用 `Selectivity`，按独立性假设使用 `a + b - a*b` 合并；递归错误被记录为 debug，并对该分支使用默认因子。
7. 启用 `EnableEvalTopNEstimationForStrMatch` 时，字符串匹配及其否定调用 `GetSelectivityByFilter`；`MATCH AGAINST` 先尝试改写为 ILIKE。辅助估算错误只追加 statement warning，不终止整个估算。
8. 仍有未覆盖位时，从普通默认因子、字符串匹配默认值、否定字符串匹配默认值中取适用的最小值乘入。最终结果下限为 `1 / RealtimeCount`，即正常非空表至少估算一行。

`GetSelectivityByFilter` 先拒绝可变副作用、相关列、多列、缺失类型、新排序规则下的非二进制字符串、非 stats v2 或无完整统计；随后分别对 TopN 值、每个直方图桶的上下界及 NULL 构造单列 chunk，调用 `VectorizedFilter`，按各部分在总计数中的权重求和。`CalcTotalSelectivityForMVIdxPath` 则根据 access conditions 是否触及 MV Index 虚拟列选择表实时行数或缩放后的索引行数作为分母。

## 数据与状态

- `HistColl` 是只读统计输入，关键字段包括 `RealtimeCount`、`ModifyCount`、`PhysicalID`、列/索引映射、`Idx2ColUniqueIDs` 和 `MVIdx2Columns`。本文件不会更新统计缓存。
- 谓词覆盖用 `i64 mask` 表示，第 `i` 位对应 `remainedExprs[i]`；这解释了主入口在超过 63 个谓词时必须回退伪统计。`StatsNode::default().mask == 0`，不会误报覆盖。
- `Ranges`、`AccessConds` 和 `TableFilters` 要么由 ranger 新建，要么从 `filledPaths` 克隆；`partCover` 表明一个 DNF 只部分成为 access condition，不能把整个表达式视为精确覆盖。
- `GetSelectivityByFilter` 使用表达式副本并递归修改该副本的 `Column.Index`，不修改调用方共享表达式。Histogram bounds 被复制进新 chunk 后执行过滤，避免向统计缓存的选择向量写入临时状态。
- 全局可变状态仅有 `outOfRangeBetweenRate` 以及两个 MV Index 回调。前三者的读取都位于 `unsafe` 块；回调也通过 `unsafe` 读取。当前实现没有本地同步原语。
- 会话级副作用包括记录相关优化变量、追加 warning，以及读取 RangeMaxSize、默认选择率、字符串选择率和向量化表达式开关。

## 依赖与调用关系

上游调用链的直接证据为：

- `logical_datasource.rs::DeriveStats -> cardinality::Selectivity -> StatsInfo::Scale`：当前 SQL 优化主链中最直接的表过滤基数派生。
- `tiflash_predicate_push_down.rs -> cardinality::Selectivity`：估算条件组，决定 late materialization 的候选、顺序与收益。
- `index_join_probe.rs -> cardinality::Selectivity`：把 table filter/index filter 的选择率用于回推 probe 扫描行数，并在错误或无效结果时局部回退。
- `row_count_index.rs -> getEqualCondSelectivity/outOfRangeFullNDV/IsLastBucketEndValueUnderrepresented`：索引范围行数估算复用本文件启发式。

下游依赖按职责分为：

- `ranger`：`ExtractAccessConditionsForColumn`、`BuildColumnRange`、`DetachCondAndBuildRangeForIndex`、DNF 合并与 `Range` 数据结构。
- 本 crate 相邻实现：`pseudo.rs::pseudoSelectivity`、`row_count_column.rs::{GetRowCountByColumnRanges,getColumnRowCount}`、`row_count_index.rs::GetRowCountByIndexRanges`、`trace.rs::recordUsedItemStatsStatus`。
- `statistics`：列/索引有效性、Histogram/TopN、实时行数缩放及查询计数。
- `expression`、`chunk`、`codec`、`collate`：表达式分类/克隆/向量执行、统计样本 chunk、TopN datum 解码和点范围排序规则。
- `planctx`/`variable`/`vardef`：通过 `lib.rs::CardinalityContext` 的 object-safe 子集读取表达式、ranger、会话上下文和优化变量。

`Cargo.toml` 以工作区路径依赖明确声明上述 crate 边界，并设置 `autotests = false`；独立测试由 `lib.rs` 中 `#[path = "selectivity_test.rs"] mod selectivity_test` 显式装入。

## 错误处理与边界

- ranger 构造、行数估算、TopN datum 解码和向量过滤通过 `Result` 向上返回。`Selectivity` 构建列/索引范围的错误会终止主入口；字符串辅助估算错误则降级为 warning。
- DNF 子项递归失败不会使整个主估算失败，而是打印 debug 并使用 `SelectivityFactor`；这是有意的可用性回退，但会降低精度。
- `GetSelectivityByFilter` 用 `(false, 0.0)` 表示“不适用”，区别于错误。它只支持安全的单列表达式和 stats v2；新排序规则的非二进制字符串明确不做样本恢复。
- `getMaskAndRanges` 在列类型缺失时返回空覆盖/空范围；未知 range type 会 panic。索引遍历中的 `expect`/`unwrap` 依赖 `HistColl` 内部映射一致，MV Index 路径也假定可取得索引元数据。
- 最终至少一行的下限只在主入口非空表路径生效；常量 false/NULL 先把结果乘为零，随后仍被提升到 `1 / RealtimeCount`，与 Go 当前行为一致。
- `outOfRangeEQSelectivity` 在没有新增行时返回 0；否则以 NDV 下限平滑并受新增行比例约束。`outOfRangeFullNDV` 的返回值是“估计行数”而非选择率，最小为 1（`modifyCount == 0` 例外）。
- `GetSelectivityByFilter` 中 `totalCnt` 来自 TopN、Histogram 非 NULL 数及 NULL 数；代码未单独防御三者总和为零，调用方应维持有效 stats v2 不变量。
- Histogram 选中向量循环中的长度保护沿用现有实现；其条件为 `selected.len() < 2 * i`，扩展时应特别复核边界索引 `2*i+1`，不要在缺少回归证据时改变 Go 对齐行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部 I/O。范围、节点、表达式和 chunk 都是一次估算调用内的栈/堆值；递归 DNF 调用同步完成。TopN/Histogram 求值使用新建 chunk 和克隆的表达式，生命周期限于函数调用，统计集合本身按共享只读引用传入。

需要特别关注两个并发边界：一是 `outOfRangeBetweenRate` 为 `static mut`，二是 `CollectFilters4MVIndex`/`BuildPartialPaths4MVIndex` 也是 `static mut Option<fn>`。文件内没有原子或锁保护；若未来在运行时写入它们，必须保证初始化先于并发规划且之后不再修改，或改造成线程安全的一次初始化机制。当前 Rust 仓库搜索不到两个回调的赋值，因此 MV Index 分支会在回调为 `None` 时安全返回“不适用”，而不是执行 Go 中由 `planner/core/core_init.go` 注入的真实逻辑。

`BuildPartialPaths4MVIndexResult.partialPaths` 使用 `Vec<&'static AccessPath>`，这不是 Go 指针生命周期的自然等价物。未来接线不应通过泄漏临时路径来满足 `'static`；应先设计清晰的所有权/借用接口，并在独立测试中覆盖结果离开构造函数后的有效性。

## 与 Go 版本的对应关系

直接对照文件是 [`selectivity.go`](selectivity.go)。Rust 保留了 Go 的主入口顺序、63 位 mask 限制、列/索引节点模型、六级贪心规则、DNF 独立性容斥、字符串 TopN 辅助估算、范围外启发式和 MV Index 回调形状。`Cargo.toml` 的 porting metadata 也明确指向同一 Go 包。

已核实的语言适配差异包括：

- Go 直接临时修改过滤表达式中列的 `Index` 并用 `defer` 恢复；Rust 的 `GetSelectivityByFilter` 接收拥有所有权的表达式副本，由 `prepareFilterForStatsEvaluation` 递归改写实际求值树，无需恢复调用方对象。
- Go 对共享 `hist.Bounds` 做浅层 chunk header copy并清空 `Sel`；Rust 逐 datum 构造新 chunk，达到不污染共享统计状态的同一目的，代价是额外复制。
- Go 的统计有效性函数接收规划上下文和对象 ID；Rust 当前接口只传统计对象及 `coll.Pseudo`。文档只确认调用形状，不把两者的告警/加载副作用视为完全等价。
- Go 在 `planner/core/core_init.go` 给 MV Index 两个函数变量赋值；当前 Rust 搜索不到相应赋值，因此普通选择率路径可用，但 MV Index access-filter/partial-path 估算尚未接线。
- Rust `BuildPartialPaths4MVIndexResult` 用结构体模拟 Go 多返回值，并引入 `'static` partial-path 引用；这是迁移边界，不是已验证的生产生命周期设计。
- Go 的 `IsLastBucketEndValueUnderrepresented` 用上下文参与 bucket 定位；Rust `Histogram::LocateBucket` 接口不需要该参数，函数仍保留 `_sctx` 形状以对齐调用层。

Go 回归面主要在 [`selectivity_test.go`](selectivity_test.go)，涵盖总体选择率、DNF、贪心、TopN 辅助估算、跨列验证、范围外值、最后桶启发式以及追加/前缀 common handle 等场景。Rust 独立测试 [`selectivity_test.rs`](selectivity_test.rs) 当前重点覆盖辅助算法、MV Index 组合公式、范围构建、表达式列索引归一化以及 common handle 路径，并非 Go 全量测试的逐项移植。

## 扩展指南

- 新增可被统计精确覆盖的谓词时，优先在 `getMaskAndRanges`/ranger 接入，并确保 mask 只标记真正完整覆盖的表达式；DNF 部分覆盖必须保留 `partCover` 和 `minAccessCondsForDNFCond`。
- 调整节点选择策略时修改 `statsNodeForGreedyChoice::isBetterThan`，同步维护确定性排序和 `selectivity_test.rs` 的优先级链、输入顺序稳定性及不相交集合用例，避免无意引发大范围执行计划变化。
- 扩展字符串/表达式样本估算时修改 `GetSelectivityByFilter`，继续拒绝副作用和相关列，分别验证 TopN、Histogram、NULL、排序规则、向量化开关、解码/求值错误；测试逻辑必须留在独立 `selectivity_test.rs`。
- 接通 MV Index 时，应在 planner core 的初始化边界为两个回调提供线程安全、确定的安装过程，消除或封装 `static mut`，并重新设计 `'static` partial path 所有权；还需补充回调缺失、构建失败、intersection/union、虚拟列参与与未参与两类分母的测试。
- 修改范围外或最后桶启发式时，必须同步 `row_count_index.rs` 的调用语义以及 Rust/Go 测试中的新增、删除、零修改、零 NDV、低 Repeat 和边界值用例；不要把返回“行数”的 `outOfRangeFullNDV` 误当作选择率。
- 新增上游调用者时，应传入 CNF 条件和与同一 `HistColl` 对应的 `filledPaths`；错误策略由调用场景决定。数据源统计派生当前传播错误，TiFlash/index join 的局部调用可选择回退。
- 任何语义改动都应继续对照 `selectivity.go`，并将 Rust 回归放在同目录独立测试文件 `selectivity_test.rs`，不把测试嵌入生产源文件。

## 验证依据

本说明基于以下直接证据编写：

- RustCodeGraph：`status` 确认索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/cardinality` 确认 Rust/Go 对照文件；`query Selectivity --kind function`、`node --file pkg/planner/cardinality/selectivity.rs`、`node selectivity.rs::Selectivity` 和 `callees selectivity.rs::GetSelectivityByFilter` 用于核对符号、源代码及下游边。精确 `callers` 查询没有返回结果，因此上游边另由 `rg` 直接核验。
- Rust 源与 crate 边界：`pkg/planner/cardinality/selectivity.rs`、`lib.rs`、`Cargo.toml`；相邻下游实现 `pseudo.rs`、`row_count_column.rs`、`row_count_index.rs`、`trace.rs`。
- Rust 上游：`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`pkg/planner/core/operator/physicalop/tiflash_predicate_push_down.rs`、`pkg/planner/core/operator/physicalop/index_join_probe.rs`。
- Go 对照：`pkg/planner/cardinality/selectivity.go`；MV Index 注入点 `pkg/planner/core/core_init.go:167-168`。Rust 全仓搜索没有发现两个同名回调的赋值。
- 独立测试：`pkg/planner/cardinality/selectivity_test.rs` 与 Go 回归 `pkg/planner/cardinality/selectivity_test.go`。Rust 测试明确覆盖类型排序、索引前缀、贪心优先级、常量列识别、范围外估算、MV Index 组合/夹紧、最后桶启发式、表达式列索引归一化、追加及前缀 common handle 范围。

本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务文件指定命令，要求目标文件存在且恰好包含上述 11 个固定二级标题；人工复核还需确认唯一新增生产物为本文件、没有把未接线 MV Index 描述成已支持，并且所有扩展测试均指向独立测试文件。
