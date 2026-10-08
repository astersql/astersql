# `pkg/util/ranger/detacher.rs`

源文件：[`detacher.rs`](./detacher.rs)；Go 对照：[`detacher.go`](./detacher.go)。

## 文件定位

该文件属于 Cargo crate `astersql-util-ranger`（见 [`Cargo.toml`](./Cargo.toml)），由 [`lib.rs`](./lib.rs) 以 `detacher_impl` 私有模块装入并整体再导出。它位于优化器表达式与底层 Range 构造之间：接收已经解析、类型化的 `expression::ExprBox` 谓词和索引列定义，把谓词划分为可参与键区间构造的 `AccessConds` 与仍需精确求值的 `RemainedConds`，并调用 `points.rs`、`ranger.rs` 将端点组合成 `Ranges`。

主入口 `DetachCondAndBuildRangeForIndex` 已接入逻辑数据源访问路径生成（`pkg/planner/core/operator/logicalop/logical_datasource.rs`）、物理计划和索引 Join（`base_physical_plan.rs`、`physical_index_scan.rs`、`index_join_probe.rs`），也被 `pkg/planner/cardinality/selectivity.rs` 用于选择率估算。`DetachCondAndBuildRangeForPartition` 的实现存在且有独立 Rust 单测，但 `pkg/planner/core/rule/rule_partition_processor.rs` 中当前找到的调用仍是注释，因此不能据此声称 Rust 分区规划主链已完整接线。

## 核心职责

1. 识别单列或复合索引前缀上的 EQ、NullEQ、IN、IS NULL 和范围谓词，合并同一列的多个约束，并保持“索引列必须从第一列连续命中”的不变量（`getPotentialEqOrInColOffset`、`ExtractEqAndInCondition`）。
2. 分别处理 CNF 与 DNF：CNF 先取连续 EQ/IN 前缀，再为下一列附加范围；DNF 必须逐个 OR 分支都能得到 access range，否则退化为全范围并保留整体 filter（`detachCNFCondAndBuildRangeForIndex`、`detachDNFCondAndBuildRangeForIndex`）。
3. 在多个 CNF 子项、点范围与非点范围之间选择更有选择性的结果；修复开关 `Fix54337` 控制是否尝试 subset/intersection，`Fix44389` 控制特定非点多列结果的选择（`extractBestCNFItemRanges`、`mergeTwoCNFRanges`、`chooseBetweenRangeAndPoint`）。
4. 遵守 `rangeMaxSize` 的内存上限。范围爆炸时记录 fallback，并把未安全用于 range 的条件送回残留条件，保证正确性不依赖优化是否成功。
5. 为 `index(tidb_shard(a), a, ...)` 补生成列前缀条件；EQ 计算一个 shard 值，IN 展开为若干 `(tidb_shard(a)=x AND a=v)` 的析取（`AddExpr4EqAndInCondition` 及 `AddGcColumn*`）。
6. 提供选择率估算和规则优化使用的轻量辅助：按单列提取条件、按列合并 DNF item、去重追加条件（`ExtractAccessConditionsForColumn`、`DetachCondsForColumn`、`MergeDNFItems4Col`、`AppendConditionsIfNotExist`）。

## 主要符号

- `DetachRangeResult`：公开结果对象。`Ranges` 是扫描区间；`AccessConds` 是实际用于构造这些区间的条件；`RemainedConds` 必须在扫描后重检；`ColumnValues` 记录各索引列已知常量；`EqCondCount`/`EqOrInCount` 描述连续等值前缀；`IsDNFCond` 和 `MinAccessCondsForDNFCond` 为规划与选择率逻辑提供 DNF 信息。
- `valueInfo`：公开类型但字段私有，记录常量 `Datum` 及其是否来自参数标记或 deferred expression。可变常量不被当作稳定的同值证据，以免错误影响计划缓存。
- `rangeDetacher<'a, 'ctx>`：一次拆分过程的内部状态，持有 `RangerContext`、全部条件、列、前缀长度、复制后的列类型，以及 `mergeConsecutive`、`convertToSortKey`、`rangeMaxSize` 三个策略参数。
- `DetachCondAndBuildRangeForIndex`：常规索引公开入口，设置 `convertToSortKey=true`、`mergeConsecutive=true`。
- `DetachCondAndBuildRangeForPartition`：分区入口，设置上述两项为 `false`，保持原始比较语义及不合并的离散点。
- `DetachSimpleCondAndBuildRangeForIndex`：只走 CNF 路径且不启用 DNF/CNF 子项优选，返回 `(Ranges, AccessConds, RemainedConds)`。
- `detachColumnCNFConditions` / `detachColumnDNFConditions`：借助 `conditionChecker` 提取单列 access 条件。DNF 任一分支无可用 access 条件时整棵 DNF 放弃；前缀索引或 collation 近似导致 `shouldReserve` 时仍保留 filter。
- `ExtractEqAndInCondition`：按索引列归位 EQ/IN 类条件，以端点交集合并同列多条件，产生空交集时提前返回空 range，并在参数可能被覆盖时标记跳过计划缓存。
- `extractBestCNFItemRanges` / `cnfItemRangeResult`：递归构造每个 CNF item 的 range，以是否为等长点范围、最小/最大覆盖列数比较候选。
- `excludeToIncludeForIntPoint` / `allSinglePoints`：将整数开边界等价移动为闭边界，并判断一组区间是否全为单点；覆盖 `i64`/`u64` 边界及不可满足区间。
- `MergeDNFItems4Col`：按 `Column.UniqueID` 合并同列、可建 range 的 DNF item；跳过多列条件和 `_tidb_rowid`，避免选择率递归失控。
- `AddExpr4EqAndInCondition`、`NeedAddGcColumn4ShardIndex`、`IsValidShardIndex`：分片索引识别、准入检查与条件重写的公开 API。

## 执行流程

常规索引路径从 `DetachCondAndBuildRangeForIndex` 进入 `detachCondAndBuildRange`：先按列复制 `FieldType`，再构造 `rangeDetacher` 并调用 `detachCondAndBuildRangeForCols`。

若顶层恰为单个 OR，`detachDNFCondAndBuildRangeForIndex` 展平 DNF。AND 分支递归走完整 CNF 拆分；单条件分支用第一列 `conditionChecker` 与 `builder` 直接建端点。所有有效分支的 ranges 汇总后 `UnionRanges`，同时求各分支共同保持的 `ColumnValues` 和最小 access 条件数。任一可满足分支完全不能用于索引、或累计内存超限时，结果回退为 `FullRange`；若只是某分支仍有残留条件，则输出 ranges，但把原始整棵 DNF 放入 `RemainedConds`。

其余情况走 CNF。`ExtractEqAndInCondition` 先寻找连续 EQ/IN 前缀并对同列条件求交；随后 `ranger.rs::buildRangeOnColsByCNFCond` 把第一列端点变成 ranges，再逐列用 `appendPoints2Ranges` 追加，非等值尾列则先求端点交集。前缀索引且允许合并时，代码额外保留未合并的 `pointRanges`，防止 `UnionRanges` 把点变成连续区间后无法继续追加后缀列。

启用 `considerDNF` 时，代码还会递归评估各 CNF item，选择覆盖列更多的点范围或更有选择性的交集。已有 EQ/IN 前缀后，剩余列通过 `detachCondAndBuildRange` 递归构造并追加到点范围；追加产生 range-size fallback 时，尾部 access 条件重新归入 residual。没有等值前缀时，则对第一列运行 `detachColumnCNFConditions` 和 `buildCNFIndexRange`。简化入口将 `considerDNF=false`，只逐项检查下一索引列并构造 CNF range。

分片索引重写是独立流程。`AddExpr4EqAndInCondition` 将条件映射到索引列，拒绝同列重复条件或非 EQ/IN 表达式；`NeedAddGcColumn4ShardIndex` 验证索引首列是 `tidb_shard` 生成列且第二列正是其参数。EQ 要求除首列外所有索引列均有稳定常量，然后求值生成列；IN 则对每个常量求 shard 值并重建 OR-of-AND 表达式。

## 数据与状态

输入表达式和值均由所有权容器 `Vec<ExprBox>` 传递；为在 access、filter 和递归分支间复用，代码显式克隆表达式、列、range 和 datum。核心中间状态包括：按索引顺序排列的 `accesses`、每列端点 `points`、同列合并标记 `mergedAccesses`、未被消费的 `newConditions`、已知常量 `columnValues`，以及 DNF 汇总使用的 `totalRangesMemUsage`。

`EqCondCount` 只计连续的纯 EQ；`EqOrInCount` 计连续 EQ/IN 前缀。该计数决定后缀列从哪里开始追加，不能简单按 `AccessConds.len()` 重算，尤其是 DNF 点范围之后追加尾列的场景。`ColumnValues` 在 CNF 候选间做“左侧已有值优先”的并集，在 DNF 分支间只保留所有分支相同且不可变的值。

字符串 range 受列 collation、表达式 collation、前缀长度和 `convertToSortKey` 共同控制。常规索引会把字符串 FieldType 转为 binary collation 的 sort-key 比较类型；分区入口不做该转换。前缀索引产生近似 range 时原条件必须保留在 `RemainedConds`。

## 依赖与调用关系

上游主要调用链为：

- `logical_datasource.rs` 构造索引列与长度，调用 `DetachCondAndBuildRangeForIndex`，再把 `RemainedConds` 分为 index filter 与 table filter。
- `physical_index_scan.rs`、`index_join_probe.rs` 和 `base_physical_plan.rs` 在物理访问路径及 range 配额评估中复用同一入口；后者会比较无限额和有限额结果。
- `selectivity.rs` 使用 `MergeDNFItems4Col` 防止 DNF 选择率递归，并分别用 `ExtractAccessConditionsForColumn` 或 `DetachCondAndBuildRangeForIndex` 计算列/索引 ranges。
- `cross_estimation.rs`、`transformation_rules.rs` 和 `logical_datasource.rs` 使用 `DetachCondsForColumn` 做单列 access/filter 划分。

下游依赖分三层：`checker.rs::conditionChecker` 判断表达式能否安全成为 access 条件；`points.rs::builder` 将表达式变为排序端点；`ranger.rs` 的 `points2Ranges`、`appendPoints2Ranges`、`buildRangeOnColsByCNFCond`、`buildCNFIndexRange`、`UnionRanges` 和 `AppendRanges2PointRanges` 完成区间物化与合并。表达式组合、相等比较、列提取和求值来自 `astersql-expression`；修复开关来自 `astersql-planner-util-fixcontrol`；上下文与 fallback 记录来自 `astersql-util-ranger-context`；元数据常量来自 `astersql-meta-model`。

`Cargo.toml` 将 Go 包映射声明为 `pkg/util/ranger`，且 `autotests=false`；因此 Rust 测试不是 Cargo 自动发现的同文件测试，而由 `lib.rs` 显式挂载 `ranger_test.rs`、`migration_aster_unit_test.rs`、`bench_test.rs` 等独立模块。

## 错误处理与边界

可恢复的构造/求值错误通过 `GoResult<T> = Result<T, errors::Error>` 向上传递，并在 `UnionRanges`、表达式求值或端点构造边界使用 `errors::Trace` 保留错误链。range intersection 的接口以 `Option` 表示失败；失败时不返回不可信交集，而是退回候选选择启发式。内存超限不是函数错误：上下文记录 fallback，函数返回更宽的 range 并保留额外 filter。

重要正确性边界包括：DNF 任一可满足分支无 access 条件时不能只扫描其它分支；nullable unique index 的 `col <=> NULL` 不进入 point-get 等值路径；不同 collation 的字符串条件不能直接视为等值前缀；非整数严格不等式不参与“边界合成等值”路径；前缀索引条件通常必须重检；参数或 deferred 常量不作为稳定的相同值依据。

代码中若干 `unwrap` 依赖上游不变量，例如 `cols`/`lengths`/`newTpSlice` 等长且非空、`accessConds[0..EqOrInCount]` 都是标量函数、调用 `AddGcColumn4InCond` 前已经由 `NeedAddColumn4InCond` 验证 IN 的首参数为列且其余为常量。新增调用者必须先满足公开准入函数隐含的形状约束；尤其不要绕过 `AddGcColumnCond` 直接传畸形数组。`AddGcColumn4EqCond` 还假定所有需要求值的 `valueInfo.value` 均存在，该条件由 `NeedAddColumn4EqCond` 保证。

源码保留了明确的未完成边界：binary-cast literal 与非 binary 字符串列的 collation 检查仍可能把可用的复合索引后缀等值降级为 filter；DNF 单项建 range 的局部内存限制有 TODO；前缀索引进一步减少 residual filter 需同时处理计划缓存依赖。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道或事务。`rangeDetacher` 只借用一次调用的 `RangerContext`，其余工作状态归当前栈帧或拥有的 `Vec`；函数返回后中间表达式、端点和候选 range 由 Rust 所有权自动释放。因此单次调用内部没有共享可变状态。

上下文仍有可观察副作用：`RecordRangeFallback` 记录范围内存回退，`SetSkipPlanCache` 在参数条件合并可能改变结果时标记跳过计划缓存。这些操作的并发安全由 `RangerContext` 实现负责，本文件不增加同步。Go 版 `ExtractEqAndInCondition` 用 `defer PutExpressionSlices` 归还表达式 slice 池；Rust 版只保留说明性注释，依赖普通 `Vec` 生命周期，没有对应池化资源需要显式归还。

## 与 Go 版本的对应关系

Rust 文件按同路径 [`detacher.go`](./detacher.go) 的顺序移植：CNF/DNF 单列拆分、EQ/IN 合并、CNF 候选比较、`rangeDetacher` 两条主路径、`DetachRangeResult` 字段，以及 shard-index 辅助函数均能找到同名 Go 符号。主要语言映射是 Go 的指针/nil 转为 `Option`，多返回值转为 tuple，`error` 转为 `Result`，slice 转为 `Vec`/借用切片；公开命名保留 Go 风格以便逐符号核对。

行为策略也保持一致：索引入口转换 sort key 并合并连续范围，分区入口两者都关闭；Fix44389/Fix54337 的选择分支、range-size fallback、前缀索引 residual、DNF 全分支安全性和 shard-index EQ/IN 重写均保留。Rust 的 `valueInfo` 虽为 `pub struct`，字段保持私有，对应 Go 未导出类型的封装意图。

测试覆盖程度并不等价。Go 的 `ranger_test.go` 包含大量 SQL 级前缀索引、shard index、fallback、DNF 最小 access 数和 collation 场景；Rust 的 `ranger_test.rs` 与 `migration_aster_unit_test.rs` 已覆盖核心 EQ/DNF、前缀截断与 residual、有限配额回退、分区不合并、简化入口、binary collation，以及单列拆分，但当前 shard-index Rust 测试只证明普通非分片 EQ/IN 不被改写，没有覆盖合法 `tidb_shard` EQ/IN 的正向展开。Go 仍是这部分完整边界矩阵的直接对照证据。

## 扩展指南

- 新增可建 range 的谓词类型时，先同步修改 `checker.rs` 的可访问性判断、`getPotentialEqOrInColOffset`/`allEqOrIn` 的快速路径和 `points.rs::builder` 的端点语义；只改其中一处会造成条件误提取或无法物化。测试应放在独立的 `ranger_test.rs` 或新的独立 `*_test.rs`，不要嵌入生产文件。
- 改动 CNF/DNF 选择策略时，重点维护三个不变量：不能丢失 residual filter；只能向点范围追加后缀列；range 配额回退必须仍产生正确的宽范围。同步覆盖 `Fix44389`、`Fix54337` 开关两侧以及 subset、intersection 失败和长 IN 列表。
- 优化前缀索引时，需同时验证字符/字节前缀、collation、计划缓存中的 mutable constant，并确保 `shouldReserve` 只在精确等价得到证明时取消。
- 扩展 shard index 形态时，从 `IsValidShardIndex` 和 `NeedAddGcColumn4ShardIndex` 接入，不要直接放宽 `AddGcColumn*` 的 `unwrap` 前提。至少增加合法 EQ、合法 IN、多列全等值、重复同列条件、错误虚拟表达式、求值错误与空值的 Rust 独立测试，并与 `ranger_test.go::TestShardIndexFuncSuites` 对照。
- 若接通分区规划器主链，应同步检查 `rule_partition_processor.rs` 的真实调用、分区 collation 语义，以及 `test_partition_path_does_not_merge_distinct_points` 之外的规划级测试；当前仅有函数级证据不足以证明端到端接线。
- 性能改动应保留 `bench_test.rs::test_bench_daily` 的长 DNF 压力面，并观察表达式克隆、递归 CNF item 构建和 `Ranges::MemUsage`。不能用合并连续范围换取更小输出而破坏后缀列追加能力。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件共 2,047 行，并被索引报告为 8 个文件使用。
- RustCodeGraph `node`：`DetachCondAndBuildRangeForIndex` 调用 `detachCondAndBuildRange`，并由 `base_physical_plan.rs` 及 `ranger_test.rs`/`bench_test.rs` 调用；分区入口和简化入口分别由对应 Rust 回归测试调用；`AddExpr4EqAndInCondition` 调用列定位、值提取、条件移除和 GC 条件生成链；`ranger.rs::buildRangeOnColsByCNFCond` 调用 `points2Ranges`/`appendPoints2Ranges`，并在 fallback 时划分已消费与残留条件。
- 直接读取的生产证据：`pkg/util/ranger/detacher.rs`、`detacher.go`、`Cargo.toml`、`lib.rs`、`ranger.rs`，以及规划调用点 `logical_datasource.rs`、`base_physical_plan.rs`、`physical_index_scan.rs`、`index_join_probe.rs`、`selectivity.rs`、`cross_estimation.rs`、`transformation_rules.rs`、`rule_partition_processor.rs`。
- 独立测试证据：`pkg/util/ranger/ranger_test.rs`、`migration_aster_unit_test.rs`、`bench_test.rs`；Go 对照测试为 `ranger_test.go` 和 `bench_test.go`。Rust 测试证明了 EQ/DNF 点范围、单列拆分、前缀索引 residual、配额回退、分区离散点及简化 CNF；Go 测试补充完整 shard-index 与更广泛 SQL 边界事实。
- 本任务是只读行为分析与文档新增，按计划不运行 Cargo；完成判定使用任务指定的 11 个固定章节结构检查，并人工核对所有“已接线/已覆盖”陈述均有上述源码、调用边或测试依据。
