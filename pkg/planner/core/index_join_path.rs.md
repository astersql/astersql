# `pkg/planner/core/index_join_path.rs`

## 文件定位

[`index_join_path.rs`](index_join_path.rs) 属于 `astersql-planner-core` crate，crate 边界由同目录 [`Cargo.toml`](Cargo.toml) 声明：库入口是 `lib.rs`，自动测试发现关闭，默认 feature 为空，`nextgen` feature 不改变本文件的编译条件。[`lib.rs`](lib.rs) 通过 `pub mod index_join_path` 公开模块，并在 `#[cfg(test)]` 下把独立的 [`index_join_path_test.rs`](index_join_path_test.rs) 装入同一 crate。本文件自身没有条件编译项，也没有内嵌测试。

本文件实现 Index Join 内侧访问路径的轻量 Rust 模型：把外侧运行时连接键、内侧静态 EQ/IN 或区间谓词按索引列顺序合成为模板 `Range`，记录已用/剩余谓词、索引偏移与连接键偏移的映射，再在多条候选路径之间择优。它还实现 range 内存上限回退、计划缓存范围重建、唯一索引至多一行判定和整数主键快捷候选。

当前接线边界必须明确：仓库级 Rust 符号检索只发现 [`index_join_path_test.rs`](index_join_path_test.rs) 和 [`exhaust_physical_plans_test.rs`](exhaust_physical_plans_test.rs) 直接调用本文件 API，没有发现生产 Rust 文件调用 `indexJoinPathBuild`、`getBestIndexJoinPathResultByProp` 或 `getBestIndexJoinInnerTaskByProp`。因此本文件是已公开、已有行为测试的迁移实现，但不能据此声称已进入 Rust SQL 规划主链。Go 对照实现则已由 `exhaust_physical_plans.go` 和 `find_best_task.go` 调用。

## 核心职责

1. `indexJoinPathBuild` 沿索引列构造连续 lookup 前缀：运行时 join key 用 `Datum::Null` 作模板占位，静态 EQ/IN 枚举点值笛卡尔积，下一列可追加静态区间或可下推的相关不等式。
2. 严守索引连续前缀：第一个未覆盖列之后的 join key 和谓词不能跨缺口继续参与范围；前缀索引上的静态谓词被保留为剩余条件供回表重检。
3. 每次扩展范围时用 `ranges_mem_usage` 的轻量估算检查 `IndexJoinContext.rangeMaxSize`；超限时保留此前有效前缀、记录 fallback 标志和告警，而不是让整个构建失败。
4. 用 `indexJoinPathResult` 汇总访问路径、候选评分、范围、访问/剩余条件、使用列数、等值前缀 NDV、偏移映射和动态尾列管理器；再通过 `indexJoinPathCompare` 选出最佳候选。
5. 在计划缓存启用且相关表达式参数化时生成 `mutableIndexJoinRange`，复用计划前以无内存上限的 rebuild 模式重建范围，并拒绝空范围或形状变化。
6. 提供整数主键候选、range 信息文本以及唯一索引最多返回一行的判定，供上层计划展示和属性推导使用。

## 主要符号

- `pub type Result<T> = std::result::Result<T, String>`：本模块的字符串错误边界，目前主要由可变范围重建使用；主构建流程的内部帮助函数没有可传播错误。
- `mutableIndexJoinRange` 及 `CloneForPlanCache`、`Range`、`Rebuild`：保存当前范围、展示文本、只读式构建输入副本和路径副本。Rust `CloneForPlanCache` 是结构体深克隆；`Range` 返回范围向量副本；`Rebuild` 用当前上下文重新计算。
- `indexJoinPathResult`：一次成功构建的完整结果。`lastColIsRange` 表示最后一列是非等值范围，计算 `eqUsedColsNDV` 时必须排除；`idxOff2KeyOff` 的 `-1` 表示对应索引列不由运行时 join key 填充。
- `indexJoinPathInfo`：构建输入，包含 join 其他条件、内外连接键、内表 schema、下推条件、可选表统计和按列存放的 NDV。当前主流程实际读取除 `innerSchema` 外的字段；`innerSchema` 由 Go 形状保留并被旧式帮助函数语义使用。
- `IndexJoinContext`：范围上限、计划缓存开关、期望行数以及内部可变的 fallback 状态。`expectedCount` 在本文件当前逻辑中未读取。`RecordRangeFallback` 私有；`HasRangeFallback`、`RangeFallbackWarnings`、`ResetRangeFallback` 提供观察和复位。
- `ColWithCmpFuncManager`：描述下一索引列的相关不等式，包括目标列、受影响列、条件和方向；它保存构建动态尾部范围所需的元数据，本文件本身不在运行期求值这些条件。
- `indexJoinPathTmp`、`indexJoinTmpRange`：私有中间态。前者保存候选 join key、未用索引列/前缀长度和偏移映射；后者保存临时 ranges、空范围及已纳入的键/谓词计数。
- `indexJoinPathBuild`：核心公开构建入口，返回 `(Option<indexJoinPathResult>, bool)`；第二项专门表示恒假条件导致的空范围，`None + false` 则表示该路径不适用。
- `append_point_column`、`append_interval_column`、`build_column_interval`：分别做点值笛卡尔扩展、区间尾列拼接和单列上下界构造；`truncate_datum_to_prefix` 对 UTF-8 字符串按字符截断，对非 UTF-8 字节按字节截断。
- `indexJoinPathCompare`、`indexJoinPathCmp4UnComparableOnes`、`isNDVClose`：先委托 `compareCandidates` 比较 skyline 候选；不可比较时依次按非接近 NDV、更长使用前缀、更多 join key 覆盖及最终 NDV 决胜。
- `indexJoinPathCountAfterAccess4Compare`：仅在一个完整长度运行时 join key、统计有效且形状稳定时用该列 NDV 调整 `count_after_access`；前缀索引、多 join key、缺失统计或非正 NDV 都保守返回原值并标记不可用。
- `indexJoinPathConstructResult`：组装结果，以已用等值列（排除范围尾列）的最大列 NDV作为轻量 `eqUsedColsNDV`，并调用 `getIndexCandidateForIndexJoin` 产生候选评分对象。
- `indexJoinIntPKRangeInfo`、`indexJoinPathRangeInfo`、`indexJoinPathGetRangeInfoAndMaxOneRow`：生成范围说明；只有唯一索引被完整覆盖，且最后访问条件不存在或为 `eq:` 时才判定每次 probe 最多一行。
- `getIndexJoinIntPKPathInfo`、`getBestIndexJoinPathResultByProp`、`getBestIndexJoinInnerTaskByProp`：构造整数 handle 快捷候选、遍历候选路径并返回最佳完整结果、或只投影出最佳 `AccessPath`。
- `indexJoinPathUpdateTmpRange`、`indexJoinPathBuildTmpRange`、`indexJoinPathFindUsefulEQIn`、`indexJoinPathBuildColManager`、`indexJoinPathRemoveUselessEQIn`：保留与 Go 分步算法对应的私有帮助函数；当前 `indexJoinPathBuild` 已内联/重写相应步骤，仓库检索未发现这些帮助函数的调用者，扩展时不能假设修改它们会改变主流程。
- `appendTailTemplateRange`：公开的尾部占位帮助函数，先估算扩展后内存，超限则原样返回并标记 fallback，否则给所有 range 的 low/high 各追加一个 `Null`。

## 执行流程

`getBestIndexJoinPathResultByProp` 先尝试 `getIndexJoinIntPKPathInfo`，再遍历 `DataSource.paths` 中所有非整数 handle 路径。每条路径调用 `indexJoinPathBuild`；遇到空范围立即返回该空结果，普通不适用路径继续，成功结果用 `indexJoinPathCompare` 与当前最佳项比较。`getBestIndexJoinInnerTaskByProp` 只是把最终结果投影为 `chosenPath`，并没有构造完整执行 task；目前两者均未发现生产 Rust 调用者。

`indexJoinPathBuild` 的主流程如下：

1. 若任一内侧下推条件名字恰为 `false`，返回 `(None, true)`。随后从 `path.index_columns` 或 `path.index.columns` 取得索引列；没有列时返回 `(None, false)`。
2. rebuild 模式把范围上限设为零，表示不限制；普通模式使用 `context.rangeMaxSize`。`indexJoinPathTmpInit` 建立索引列到 inner join key 偏移的映射。
3. 先收集每列首个 EQ/IN，再按索引顺序确定连续前缀。join key 可直接占一列；非 join key 必须有静态 EQ/IN 才能继续。前缀索引上的静态条件允许接上后续动态 join key，但会阻止更多静态条件继续扩展。首个缺口之后的 join key 映射全部清为 `-1`。
4. 从一个空 `Range` 开始逐列扩展：join key 追加 `Null` 模板；EQ/IN 从表达式名字解析值并做笛卡尔积；前缀索引值先截断。每次扩展独立检查内存，超限则停止并保留上一步有效范围。若 EQ/IN 没有值则是空范围；若未使用任何 join key 或范围宽度为零则路径不适用。
5. 若没有 fallback，检查紧邻的下一索引列。优先把可下推、无 `cast` 的相关不等式变成占位尾列及 `ColWithCmpFuncManager`；否则把静态 `gt/ge/lt/le` 合成一个区间。二者成功时都标记 `lastColIsRange`。特殊的 join-key 缺口在没有静态 access 时追加一个开放尾部 `Null`，对应当前简化 Range 模型。
6. 根据已使用谓词键拆分 `chosenAccess` 与 `chosenRemained`。前缀索引上的 EQ/IN 或不等式即便参与范围，也仍放入 remained 以便精确重检。
7. 若计划缓存开启且 access/other 条件中含 `?` 或 `param:`，包装 `mutableIndexJoinRange`。最后由 `indexJoinPathConstructResult` 计算 NDV、比较用行数和候选对象，返回完整结果。

计划缓存复用时，`mutableIndexJoinRange::Rebuild` 再次调用上述流程但传入 `rebuildMode = true`。它要求新结果非空、存在，且 range 数量和首个 range 的宽度与旧结果相同；满足后更新 `rangeInfo` 和 ranges，否则要求上层重新优化而不是冒险复用旧计划。

## 数据与状态

范围使用 `Vec<Range>`，每个 `Range` 以 `low`/`high` 两个 `Vec<Datum>` 表示复合索引边界，并带上下界排除标志。运行时 join key 的值尚未知，因此模板位置使用 `Datum::Null`；这既是占位约定，也被简化模型用于某些开放边界，消费者不能把这里的 `Null` 一概解释为 SQL NULL 等值查找。

表达式是 [`task.rs`](task.rs) 中的轻量 `Expression`。本文件通过 `name` 前缀识别 `eq:`、`in:`、`gt:`、`ge:`、`lt:`、`le:`、`param:` 和 `false`，并通过可选 `column` 关联列；`condition_values` 把冒号后的逗号列表解析为 `i64`，解析失败则保留为字节串。这不是完整 SQL 表达式求值器，函数名、常量编码和去重键 `(name, column)` 都是当前迁移契约的一部分。

关键不变量包括：范围只能覆盖连续索引前缀；`idxOff2KeyOff.len()` 与索引列数一致；进入结果的未使用/越过缺口映射为 `-1`；range 尾列不计入等值前缀 NDV；前缀索引条件必须重检；fallback 只能缩短 lookup 前缀，不能返回超过上限后构造出的新范围；rebuild 不允许改变范围数量或宽度。

`IndexJoinContext` 的 fallback 状态通过 `Cell<bool>` 和 `RefCell<Vec<String>>` 在共享借用下更新，因此一次构建可记录告警而无需 `&mut Context`。调用方可显式 `ResetRangeFallback`，否则状态会跨同一 context 的后续构建保留。`indexJoinPathInfo`、路径和结果普遍派生 `Clone`，可变范围包装器持有的是独立副本，不借用原调用栈数据。

## 依赖与调用关系

直接内部依赖来自 [`find_best_task.rs`](find_best_task.rs) 和 [`task.rs`](task.rs)。前者提供 `AccessPath`、`DataSource`、`Datum`、`Range`、`PhysicalProperty`、`candidatePath`、`compareCandidates` 和 `getIndexCandidateForIndexJoin`；后者提供轻量 `Expression` 与 `StatsInfo`。标准库依赖只有 `Cell`、`RefCell`、`HashMap` 和 `HashSet`。本文件没有直接使用 `Cargo.toml` 中的外部 crate，所需 planner 类型都通过同 crate 模块取得。

内部主调用链为 `getBestIndexJoinInnerTaskByProp -> getBestIndexJoinPathResultByProp -> {getIndexJoinIntPKPathInfo, indexJoinPathBuild, indexJoinPathCompare}`。构建链进一步进入 `indexJoinPathTmpInit`、点/区间追加、前缀截断、可变范围包装和 `indexJoinPathConstructResult`；结果组装再调用 `indexJoinPathCountAfterAccess4Compare` 与 `getIndexCandidateForIndexJoin`。`mutableIndexJoinRange::Rebuild` 形成一条回到 `indexJoinPathBuild(rebuildMode=true)` 的重建路径。

RustCodeGraph 的文件节点把目标文件标记为由 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 使用，但对目标符号的精确 `callers/callees` 查询在本次环境中未于 30 秒内返回；仓库级 `rg` 又没有在该文件或其他生产 Rust 文件中找到目标 API 调用。因此这里不把文件级索引关系解释为已建立业务调用边。可确认的直接 Rust 调用者是两个独立测试：[`index_join_path_test.rs`](index_join_path_test.rs) 覆盖比较、唯一性、NDV 和尾部范围，[`exhaust_physical_plans_test.rs`](exhaust_physical_plans_test.rs) 覆盖完整 lookup 构建与 fallback。

Go 生产调用边则明确存在：`exhaust_physical_plans.go` 两处调用 `getBestIndexJoinPathResultByProp`，`find_best_task.go` 调用 `getBestIndexJoinInnerTaskByProp`。这些边证明 Go 设计位置，但不证明 Rust 已接线。

## 错误处理与边界

返回值必须区分三类结果：`Ok((Some(result), false))` 是可用路径；`Ok((None, false))` 是普通“不适用”，例如无索引列、没有可用前导 join key 或 fallback 后前缀为空；`Ok((None, true))` 是恒假/空范围。候选遍历把空范围视为可立即结束的语义结果，而不是继续寻找另一路径。

`Rebuild` 是当前唯一显式产生 `Err(String)` 的路径：空范围、缺失构建结果、range 数量变化或首个 range 宽度变化都会失败。这是计划缓存的 fail-closed 边界。`indexJoinPathBuild` 的签名虽然返回 `Result`，当前主体没有产生 `Err` 的分支；表达式解析失败会退化成 `Datum::Bytes`，不是错误。

范围内存是近似值：`条数 × 首个 range 宽度 × 16`，`appendTailTemplateRange` 也使用相同量级的简化估计；它不等于 Go `ranger.Ranges.MemUsage()` 的精确对象占用。`rangeMaxSize == 0` 表示无限制。fallback 告警文本记录上限，但本文件不把它转换为 SQL warning code。

静态区间只选择首个 lower 和首个 upper 条件；缺失下界用 `Null`，缺失上界用单字节 `0xff`。若存在前缀长度，上界即使来自严格 `<` 也不标记 exclusive，因为截断后必须保守包含并重检。相关不等式只按表达式名字是否含 `cast` 判定可下推，远窄于 Go 的真实表达式/inner-schema 分析。

唯一索引的 `max_one` 判定只检查 unique、使用列数与最后 access 名称是否 `eq:`；整数 handle 候选固定使用 `Datum::Int(0)` 模板。两者都是轻量模型契约，不可外推为完整 SQL 类型、无符号 handle、collation、NULL 语义或多值/向量索引安全性。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、文件句柄或网络资源；所有构建和比较均同步完成。`IndexJoinContext` 使用 `Cell`/`RefCell`，因此不是 `Sync`，不能在多个线程间无保护共享同一个实例。单线程内的共享引用可以记录 fallback，但 `RefCell` 的运行时借用规则仍意味着未来若在持有告警借用时重入记录逻辑会 panic。

范围扩展会克隆既有 `Range`，EQ/IN 通过笛卡尔积放大条数，时间和内存约为已有 range 数乘当前值数；`rangeMaxSize` 是阻止继续扩张的主要保护。前缀截断和区间追加按 range 宽度线性处理。`CloneForPlanCache`、`Range` 和可变包装器构造都会复制 ranges、表达式、路径及统计映射，热路径上不应无必要反复调用。

`mutableIndexJoinRange` 拥有重建所需的 `indexJoinPathInfo` 和 `AccessPath` 副本，没有悬垂引用；重建成功后以新向量替换旧范围。失败时在赋值之前返回，因此旧 ranges 与 `rangeInfo` 保持不变。context 的 fallback 标志则可能已被本次尝试修改，调用方需要按语句生命周期读取并复位。

## 与 Go 版本的对应关系

直接对照文件是 [`index_join_path.go`](index_join_path.go)，直接 Go 单元测试是 [`exhaust_physical_plans_test.go`](exhaust_physical_plans_test.go)，计划缓存的用户可见回归还见 [`integration_test.go`](integration_test.go) 的 `TestPlanCacheForIndexJoinRangeFallback`。Rust [`exhaust_physical_plans_test.rs`](exhaust_physical_plans_test.rs) 复刻了 Go lookup-filter 夹具意图：连续/非连续 join key、EQ/IN 补齐、相关尾列、cast 排除、前缀重检、多字节字符截断、IN 笛卡尔积以及逐级 fallback。

Rust 与 Go 的结构对应清晰：`mutableIndexJoinRange`、两类构建输入/结果、临时映射、范围帮助函数、NDV 比较、range info、整数主键快捷路径和最佳路径遍历均有同名或同职责实现。`Rebuild` 同样在重用缓存计划前忽略普通范围内存上限，并拒绝空范围及形状变化；fallback 同样保留可用前缀而不是丢弃全部访问能力。

但 Rust 目前是有意简化的迁移模型。Go 使用真实 `expression.Expression`、`ranger.BuildColumnRange`、collation/type 检查、statement memory tracker、统计直方图与 `cardinality.EstimateColsNDVWithMatchedLen`；Rust 用字符串表达式、简化 `Datum/Range`、固定内存估算及按列最大 NDV。Go `indexJoinPathTmpInit` 会拒绝 collation 不兼容的连接键，Rust 未实现该检查。Go 相关条件会识别左右操作数、对称操作符、外部列以及 inner schema，Rust 主要按目标列和名字过滤。Go range info 会按日志脱敏规则打印真实表达式，Rust 输出列编号和表达式名。

另一个关键差异是接线状态：Go 最佳路径函数已进入物理计划枚举和 `findBestTask`；Rust 函数当前仅在测试中直接使用。新增文档或测试不能把这种差异描述为完成迁移，真正接线需要在 Rust 物理计划/任务选择入口建立调用边并验证端到端计划行为。

## 扩展指南

- 修改 lookup 前缀、EQ/IN、区间或 fallback 逻辑时，首要入口是 `indexJoinPathBuild` 及 `append_point_column`/`append_interval_column`。同步更新独立的 [`exhaust_physical_plans_test.rs`](exhaust_physical_plans_test.rs)，至少覆盖缺口、前缀索引重检、IN 组合、相关尾列和各级内存阈值；不要把测试写进生产源文件。
- 修改候选排序或统计折算时，检查 `indexJoinPathCompare`、`indexJoinPathCmp4UnComparableOnes`、`isNDVClose`、`indexJoinPathCountAfterAccess4Compare` 和 `indexJoinPathConstructResult`，并更新 [`index_join_path_test.rs`](index_join_path_test.rs) 中 NDV 阈值、稳定/不稳定 key 形状和范围尾列用例。
- 扩展计划缓存行为时，保持 `Rebuild` 的形状守卫和失败不覆盖旧状态；增加参数导致空范围、宽度变化、范围条数变化及 fallback 告警生命周期测试，并与 Go `TestPlanCacheForIndexJoinRangeFallback` 的意图核对。
- 若把 Rust 模块接入生产主链，应从现有物理计划枚举和任务选择边界调用 `getBestIndexJoinPathResultByProp`/`getBestIndexJoinInnerTaskByProp`，同时补端到端 Rust 计划测试。不能仅因 `lib.rs` 已公开模块便视为完成接线。
- 若继续对齐 Go，应优先替换字符串表达式解析和简化范围模型，补齐类型/collation、真实 ranger、精确内存、统计版本/直方图、日志脱敏及 unsigned handle 语义。迁移时要保留三态返回与逐级 fallback，不可用“编译通过”的桩替代。
- 当前若修改 `indexJoinPathUpdateTmpRange` 等未调用的分步帮助函数，不会影响主构建路径。应先决定删除重复实现还是让主流程重新复用，并以调用边和回归测试证明；不能只改死代码后宣称行为变化。

正确性风险集中在索引前缀连续性、前缀谓词重检、动态/静态尾列的互斥顺序、计划缓存形状稳定性和 `None/empty` 语义。兼容风险集中在 Go 的表达式、collation、range info 与统计语义差异。性能风险集中在 IN 笛卡尔积、范围深拷贝和过于粗糙的内存估算。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust/Go 文件均在索引覆盖范围内。
- RustCodeGraph `node --file pkg/planner/core/index_join_path.rs --offset ... --limit ...`：分四段读取完整 1,090 行，核对所有类型、函数、分支、注释和文件级使用关系。
- RustCodeGraph `query indexJoinPathBuild` 与 `query getBestIndexJoinPathResultByProp --json`：确认同名 Go/Rust 定义及其精确文件位置。带 `--file` 的 `callers/callees` 查询在 30 秒内未返回，因此没有把缺失图输出当作调用证据。
- 仓库级 `rg` 精确检索所有公开入口和核心函数：Rust 直接调用只见 `index_join_path_test.rs` 与 `exhaust_physical_plans_test.rs`；Go 生产调用见 `exhaust_physical_plans.go` 和 `find_best_task.go`。
- [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)：核对 crate 名称、`lib.rs` 入口、feature、Go 包迁移元数据、公开模块和独立测试装配；目标包根目录没有 `doc.go`，因此无额外包契约可读。
- [`index_join_path_test.rs`](index_join_path_test.rs)：核对 NDV 接近阈值、tail fallback 原样返回、不可比较候选排序、唯一索引最多一行、范围尾列不计 NDV、单 join key 行数折算及不稳定 key 形状拒绝。
- [`exhaust_physical_plans_test.rs`](exhaust_physical_plans_test.rs)：核对连续前缀、相关条件、cast 排除、前缀重检、ASCII/多字节截断、IN 笛卡尔积和逐级 range fallback；对应 Go 夹具来自 [`exhaust_physical_plans_test.go`](exhaust_physical_plans_test.go)。
- [`index_join_path.go`](index_join_path.go) 与 [`integration_test.go`](integration_test.go)：核对真实 Go 算法、生产接线、计划缓存重建和用户可见 warning 行为，并据此明确 Rust 的简化点与未接线边界。
- 本任务是纯文档分析，未运行 Cargo；交付验证仅执行任务文件规定的 11 章节结构检查，并人工复核“为何存在、如何运行、如何安全扩展”三项问题均可由上述直接证据回答。
