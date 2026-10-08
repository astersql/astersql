# `pkg/statistics/table.rs`

## 文件定位

`table.rs` 是 `astersql-statistics` crate 的表级统计容器层：它把列统计 `Column`、索引统计 `Index`、实时行数/修改量、加载状态与表级版本组合成 `HistColl` 和 `Table`。模块由 `pkg/statistics/lib.rs` 以私有 `mod table` 声明，再通过 `pub use table::*` 重导出，因而规划器、session 规划接线、统计缓存和同步加载器均通过 crate 根 API 使用它（`pkg/statistics/lib.rs`；`Table`、`HistColl`）。

`pkg/statistics/Cargo.toml` 将该 crate 声明为 `astersql-statistics`，并用 `package.metadata.porting.go-package = "pkg/statistics"` 指明 Go 对照包。本文件没有条件编译项；独立 Rust 测试由 `lib.rs` 的 `#[cfg(test)] #[path = "table_test.rs"] mod table_test` 接入，测试未内嵌在生产源文件中。

## 核心职责

- 维护表统计快照。`Table` 在 `HistColl` 之上增加存在图、分析/直方图/表信息版本和主键句柄标记，并通过 `Deref`/`DerefMut` 让调用方直接访问 `HistColl` 字段和方法。
- 维护“元数据中存在”与“已 ANALYZE”两个独立维度。`ColAndIdxExistenceMap` 的 bool 值表示是否已分析，key 是否存在则表示是否有该列/索引条目；这是避免统计加载器发出无效请求的元数据。
- 为优化器准备查询维度的统计。`ID2UniqueID` 和 `GenerateHistCollFromColumnInfo` 将元数据列 ID 重映射为查询内 unique ID，建立索引到列以及首列到索引的反向映射。
- 提供选择率与维护决策所需的派生量：MV 索引缩放计数、分析时行数、过期判定、健康度、自动 ANALYZE 资格、加载需求和内存占用。
- 在没有真实统计时提供 `PseudoHistColl`/`PseudoTable`，以 `PseudoRowCount = 10_000` 作为估算起点，并显式标记是否允许触发统计加载。

## 主要符号

- 常量与全局配置：`PseudoVersion` 为 0，`PseudoRowCount` 为 10,000，`AutoAnalyzeMinCnt` 默认为 1,000。`RATIO_OF_PSEUDO_ESTIMATE: AtomicU64` 以 `f64::to_bits` 保存默认 0.7，由 `RatioOfPseudoEstimate`/`SetRatioOfPseudoEstimate` 读写。`AUTO_ANALYZE_MIN_CNT_OVERRIDE: AtomicI64` 以 -1 表示未覆盖，由 `EffectiveAutoAnalyzeMinCnt`、`SetAutoAnalyzeMinCnt`、`ResetAutoAnalyzeMinCnt` 管理。
- `CopyIntent` 枚举有 `MetaOnly`、`ColumnMapWritable`、`IndexMapWritable`、`BothMapsWritable`、`AllDataWritable` 五种意图。当前 Rust `Table::Copy` 对五种值都执行 `self.clone()`，意图名尚未影响拷贝策略。
- `Table` 是对外表级快照：`ColAndIdxExistenceMap`、内嵌语义的 `HistColl`、`Version`、`LastAnalyzeVersion`、`LastStatsHistVersion`、`TblInfoUpdateTS` 和 `IsPkIsHandle`。`Table::New`、`DelCol`/`DelIdx`、`CopyAs`、`GetStatsHealthy`、`ColumnIsLoadNeeded`、`IndexIsLoadNeeded` 是主要生命周期/决策 API。
- `ColAndIdxExistenceMap` 分别用 `HashMap<i64, bool>` 保存列与索引状态。`Has` 只看 key，`HasAnalyzed` 还要求值为 true；`Insert*`、`Delete*`、`CloneMap` 提供变更与隔离。
- `HistColl` 保存 `Columns`/`Indices`、`PhysicalID`、`RealtimeCount`、`ModifyCount`、`StatsVer`、`Pseudo`、`CanNotTriggerLoad`，以及 `Idx2ColUniqueIDs`、`ColUniqueID2IdxIDs`、`UniqueID2colInfoID`、`MVIdx2Columns` 四组查询期映射。创建入口是 `NewHistColl` 和 `NewHistCollWithColsAndIdxs`。
- `HistColl` 的计算 API 包括 `GetScaledRealtimeAndModifyCnt`、`IsOutdated`、`MemoryUsage`、`CalcPreScalar`、`DropEvicted`、`GetAnalyzeRowCount`、`ID2UniqueID` 和 `GenerateHistCollFromColumnInfo`。`StableOrderColSlice`/`StableOrderIdxSlice` 在需要确定性选取时按 HashMap key 排序。
- `TableMemoryUsage`、`ColumnMemUsage`、`IndexMemUsage` 提供总量和跟踪量分解；`TrackingMemUsage` 只计 Histogram、CMSketch 和 TopN，不计 FMSketch。
- 边界构造/检查函数为 `PseudoHistColl`、`PseudoTable`、`AnalyzeVersionMatchesForTableStats` 和 `FullLoadStatusForPseudo`。

## 执行流程

1. 常规快照由 `Table::New(physical_id, realtime_count, modify_count)` 建立：分配空存在图和空 `HistColl`，版本及时间戳归零，之后由加载/缓存代码填充列、索引及版本。无真实统计时，`PseudoTable` 转而建立 `Pseudo = true`、行数 10,000 的轻量快照。
2. 查询规划阶段，`pkg/session/runtime/planning.rs` 组织 info ID 到 unique ID 及索引列 ID 映射，调用 `GenerateHistCollFromColumnInfo`。该方法先用 `ID2UniqueID` 筛选并重键列统计，再仅为能映射到至少一个 unique ID 且存在统计的索引建立 `Indices`、`Idx2ColUniqueIDs` 和 `ColUniqueID2IdxIDs`，最后对反向索引 ID 列表排序。
3. 基数估算时，`pkg/planner/cardinality/row_count_index.rs` 和 `selectivity.rs` 调用 `GetScaledRealtimeAndModifyCnt`。非 MV 索引或未全量加载直接返回表计数；MV 索引则以 `index.TotalRowCount / GetAnalyzeRowCount` 同比缩放实时行数和修改量，防止一行多索引项导致增长因子失真。
4. 统计同步/异步加载前，`ColumnIsLoadNeeded` 和 `IndexIsLoadNeeded` 联合内存对象、存在图与加载状态判断是否需要 I/O。真实调用点包括 `pkg/statistics/handle/syncload/stats_syncload.rs`；列 API 还返回“是否已分析”以区分缺失、未分析和待加载三种状态。
5. 缓存维护代码在 `pkg/statistics/handle/cache/statscache.rs` 使用 `CopyAs(MetaOnly)` 发布新快照，用 `MeetAutoAnalyzeMinCnt` 和 `GetStatsHealthy` 生成健康度信息；LFU 缓存在 `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs` 使用 `CopyAs(AllDataWritable)` 准备可裁剪副本。
6. `CalcPreScalar` 把桶计数累加成前缀计数后调用直方图标量预计算；它是原地、非幂等的转换，调用方必须保证每份桶数据只转换一次。`DropEvicted` 则对仍标记需加载的列/索引丢弃非必要数据。

## 数据与状态

`RealtimeCount` 是应用统计 delta 后的当前表行数，`ModifyCount` 是从分析基准后的修改量；`GetAnalyzeRowCount` 优先按 key 稳定顺序选第一个全量加载列，其次选第一个全量加载且非 MV 的索引，都不存在时返回 -1。`IsOutdated` 优先使用这个分析行数，仅在其小于 0 时退回 `RealtimeCount`，并以严格大于 `RatioOfPseudoEstimate()` 为过期条件。

`LastAnalyzeVersion > 0` 是 `Table::IsAnalyzed` 的唯一判据，不是 `StatsVer`。`GetStatsHealthy` 在伪统计上返回 `(0, false)`，未分析的真实快照返回 `(0, true)`，已分析时计算 `floor((1 - ModifyCount/count) * 100)`，修改量不小于基准则通常为 0，但 `count == 0 && ModifyCount == 0` 特别返回 100。

`Idx2ColUniqueIDs`、`ColUniqueID2IdxIDs`、`UniqueID2colInfoID` 是查询规划期的派生映射，不是统计存储本体。`GenerateHistCollFromColumnInfo` 对某索引的列 ID 使用 `map_while`：遇到第一个不在查询列映射中的 ID 即停止收集；只要已收集前缀非空，就保留该索引与这个前缀。

## 依赖与调用关系

文件内部直接依赖标准库 `HashMap` 和 `AtomicI64`/`AtomicU64`，以及同 crate 重导出的 `Column`、`Index`、`IsAnalyzed`、`NewStatsFullLoadStatus`、`StatsLoadedStatus`、`Version2`。其中 `Column`/`Index` 继续下调直方图、CMSketch、TopN、FMSketch 的行数、加载状态、内存与字符串 API。

RustCodeGraph 将 `pkg/statistics/table.rs` 识别为被 38 个文件使用，列出的上游包括 `pkg/domain/domain.rs`、`pkg/infoschema/tables.rs`、`pkg/planner/cardinality/cross_estimation.rs`、`row_count_column.rs` 和 `row_count_index.rs`。精确接线由局部搜索进一步核实：

- `pkg/session/runtime/planning.rs` 调用 `GenerateHistCollFromColumnInfo` 并读取 `RatioOfPseudoEstimate`；
- `pkg/planner/cardinality/row_count_index.rs` 和 `selectivity.rs` 调用 `GetScaledRealtimeAndModifyCnt`；
- `pkg/statistics/handle/syncload/stats_syncload.rs` 调用 `ColumnIsLoadNeeded`/`IndexIsLoadNeeded`；
- `pkg/statistics/handle/cache/statscache.rs` 调用 `CopyAs`、`MeetAutoAnalyzeMinCnt`、`IsAnalyzed`、`GetStatsHealthy`；
- `pkg/planner/core/operator/logicalop/logical_datasource.rs`、`logical_mem_table.rs` 和 `physicalop/base_physical_plan.rs` 构造伪统计；
- `pkg/domain/domain.rs` 和 statistics handle 代码使用 `EffectiveAutoAnalyzeMinCnt`。

RustCodeGraph 的精确 impl 方法 `callers`/`callees` 查询本次没有返回边，因此上述具体方法调用点以 `rg` 的 Rust 语法名称搜索为直接证据，不将未观察到的动态调用推断为事实。

## 错误处理与边界

该文件没有 `Result` 或自定义错误路径；查找缺失通过 `Option` 或布尔组合返回。`GetCol`/`GetIdx`/`RemoveCol`/`RemoveIdx` 与 `GetStatsInfo` 都在 ID 缺失时返回 `None`。`ColumnByName` 和 `IndexStartWithColumn` 先检查 `Info` 及索引首列，因此空元数据和零列索引不会触发解引用 panic。

`AnalyzeVersionMatchesForTableStats` 是本文件唯一显式断言边界：它要求 `requested_version == Version2`，否则 `assert_eq!` panic。表不存在、为伪统计或当前 `StatsVer` 未分析时均视为匹配；仅已分析且版本不等于 2 时返回 false。

`GetScaledRealtimeAndModifyCnt` 对非 MV、未全加载、分析行数非正或索引行数非正都退回未缩放计数，避免除零。浮点缩放后使用 Rust `as i64` 转换，存在截断小数的精度边界。`IsOutdated` 在行数为 0 时不过期；比例恰好等于阈值时也不过期。

## 并发与资源生命周期

`Table`、`HistColl` 和存在图本身不包含锁、任务、通道或事务；所有可变操作都要求 `&mut self`，并发发布与快照替换由上层缓存负责。`Column`/`Index` 以 `Box` 拥有，`HashMap` 拥有容器；`Remove*` 转移出 Box，`Del*` 则丢弃对象并同步删除存在图条目。

两个可调全局量使用顺序一致 `Ordering::SeqCst`：伪统计过期阈值把 `f64` bit pattern 存在 `AtomicU64` 中，自动 ANALYZE 最小行数覆盖存在 `AtomicI64` 中。这使并发读写无数据竞争，但多个测试同时更改这些进程全局值仍可产生逻辑干扰；调用者应在测试结束时恢复默认值。

`Clone` 决定快照资源隔离。当前 Rust `Table::Copy`/`CopyAs` 对所有意图都深入 clone `HashMap` 和 Box 内容，因此变更副本不会变更原存在图；这保守但可能增加大表统计的 CPU/内存压力。

## 与 Go 版本的对应关系

Go 主对照文件是 `pkg/statistics/table.go`，Go 回归是 `pkg/statistics/table_test.go`。常量、`Table`/`HistColl`/存在图的主体字段，加载判定、分析行数、MV 索引缩放、健康度、过期判定和版本匹配的主要分支与 Go 保持对应。`pkg/statistics/table_test.rs` 实际覆盖存在/已分析三态、伪行数、自动分析阈值、加载需求、分析行数过期、内存跟踪不计 FM sketch、版本匹配和健康度取整。

已核对的迁移差异与限制如下：

- Go `CopyAs` 根据 intent 共享 map、只 clone 容器或深拷贝元素；Rust 五个 intent 都是完整 `clone`。Rust 测试只验证了各 intent 的副本独立，而 Go `TestCopyAs` 明确验证 `MetaOnly` 共享两个 map，其他意图也有不同共享策略。这是性能/别名语义差异，不应在新调用点中假设 Rust 已有 Go 的共享行为。
- Go `PseudoTable` 接收完整 `TableInfo`、`allowTriggerLoading` 和 `allowFillHistMeta`，并为 public 非隐藏列/public 索引填存在图及可选伪直方图；Rust `PseudoTable(physical_id)` 只保存 ID，固定允许触发加载，存在图与列/索引为空。部分规划器代码因此在 `logical_datasource.rs` 自行补伪列。
- Go `GenerateHistCollFromColumnInfo` 由 `TableInfo` 和 expression columns 构建全部映射，并通过 `PrepareCols4MVIndex` 保存 MV 索引表达式列；Rust 只接收两张 ID map，`MVIdx2Columns` 的值也只是 `Vec<i64>`，且该方法当前不填它。
- Go `GetStatsInfo` 有 `needCopy` 参数可返回保护性副本；Rust 只返回借用引用，借用规则防止通过该 API 修改缓存，但没有对应的可拥有副本模式。
- Go `TableMemoryUsage` 保存 `TableID` 与 trait-like `CacheItemMemoryUsage` 明细；Rust 仅保存 ID 到 `i64` 的总占用映射。Go `IndexMemUsage` 没有 `FMSketchMemUsage`，Rust 结构有该字段但跟踪量同样不计它。
- Go `DropEvicted` 跳过未初始化或已 `AllEvicted` 项；Rust 以 `StatsLoadedStatus.IsLoadNeeded()` 为执行条件。两者状态语义是否在所有组合上等价，本文件与直接测试不足以证明，因此标记为未验证。

## 扩展指南

新增表级统计字段时，应同时检查 `Table::New`、`PseudoTable`、`Table::Copy`/`CopyAs` 和对应的 Go `Table`/`CopyAs`，避免新字段在伪快照或副本中丢失。新增 `HistColl` 查询期映射时，必须在 `NewHistColl`、`ID2UniqueID`、`GenerateHistCollFromColumnInfo` 中定义初始值和重映射语义，并判断它是否应进入缓存快照。

改动加载决策时，必须保留 `Has` 和 `HasAnalyzed` 区分的三态，并同步检查 `pkg/statistics/handle/syncload/stats_syncload.rs` 的解构与任务构建。改动行数/过期/健康度时，要检查分析行数优先级、MV 索引缩放、零行与阈值等号边界，以及 `pkg/planner/cardinality` 和 statscache 调用方。

任何生产修改都应继续在独立 `pkg/statistics/table_test.rs` 中增加 Rust 回归，并与 `pkg/statistics/table_test.go` 及 `table.go` 相同分支对照；不应把测试写回 `table.rs`。如要对齐 `CopyIntent` 性能语义，需要先明确 Rust 所有权容器（例如 `Arc` 或 copy-on-write）的快照设计，不能只把 clone 删掉；同时必须验证 `statscache.rs` 的发布后不可变假设和 LFU 裁剪的独立性。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/statistics/table.rs` 完整读取了 898 行源文件，并报告 38 个使用文件。
- RustCodeGraph 符号查询：查询了 `PseudoTable`、`AnalyzeVersionMatchesForTableStats`、`GetStatsHealthy`、`ColumnIsLoadNeeded`、`GenerateHistCollFromColumnInfo`、`IsOutdated`、`GetScaledRealtimeAndModifyCnt` 和 `CopyAs`；同名 Go/Rust 符号均被定位。精确 impl 方法 callers/callees 未返回边，其缺口由限定 `pkg/**/*.rs` 的同名调用搜索补足。
- 已读生产与装配证据：`pkg/statistics/table.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/Cargo.toml`。该目录下无 `doc.go`，crate 顶层职责由 `lib.rs` 的模块注释和声明核对。
- 已读 Go 对照与测试证据：`pkg/statistics/table.go`、`pkg/statistics/table_test.go`、`pkg/statistics/table_test.rs`。Rust 独立测试明确验证存在图、伪统计、加载分支、过期、健康度、版本匹配、内存跟踪和当前 Rust 拷贝独立性。
- 已核对直接调用证据：`pkg/session/runtime/planning.rs`、`pkg/planner/cardinality/row_count_index.rs`、`pkg/planner/cardinality/selectivity.rs`、`pkg/statistics/handle/syncload/stats_syncload.rs`、`pkg/statistics/handle/cache/statscache.rs`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`logical_mem_table.rs` 与 `pkg/planner/core/operator/physicalop/base_physical_plan.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在，且上述十一个固定二级标题各出现一次。
