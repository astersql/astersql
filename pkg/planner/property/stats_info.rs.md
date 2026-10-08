# `pkg/planner/property/stats_info.rs`

## 文件定位

本文件属于 `astersql-planner-property` crate。crate 根 `pkg/planner/property/lib.rs` 以私有模块 `stats_info` 装入它，再通过 `pub use stats_info::*` 暴露其 API；因此规划器其他 crate 通常以 `property::StatsInfo`、`property::DeriveLimitStats` 等名称使用这些定义。`pkg/planner/property/Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/planner/property`，直接的 Go 对照是同目录的 `stats_info.go`。

它位于“统计收集/基数估算”和“逻辑、物理计划代价决策”之间：自身不读取表统计，也不计算选择率，而是承载计划节点输出的行数与 NDV 摘要，并提供缩放、列组 NDV 查询和 Limit/TopN 截断操作。`pkg/planner/cardinality/ndv.rs::init` 向本文件注册真正的 NDV 缩放算法；逻辑计划的 `LogicalLimit::DeriveStats`、`LogicalTopN::DeriveStats`，以及多个物理计划生成路径消费这里的结果。

## 核心职责

- 用 `StatsInfo` 保存计划输出行数、逐列 NDV、列组 NDV、统计版本和可选直方图集合引用，作为规划阶段跨算子传递的轻量统计摘要。
- 用 `Scale`/`ScaleByExpectCnt` 在选择率或父节点期望行数降低时生成新的统计值，同时保留直方图引用和统计版本。
- 用 `GetGroupNDV4Cols` 以列 `UniqueID` 的无序输入查找精确列组 NDV，供 `pkg/planner/cardinality/ndv.rs::EstimateColsNDVWithSessionVars` 和聚合统计推导优先采用精确估计。
- 用 `DeriveLimitStats` 表达 Limit/TopN 的统计不变量：输出行数不超过子节点和 limit，任何列 NDV 不超过输出行数，列组 NDV 被清空。
- 通过 `ScaleNDVFunc` 注入 cardinality crate 的算法，避免 property crate 反向依赖 cardinality 而形成 Cargo 循环依赖。

本文件不实现直方图行为、统计持久化或代价公式；`HistCollRef` 明确把真实对象保持为不透明共享值。

## 主要符号

- `ScaleNDVCallback = fn(&SessionVars, f64, f64, f64) -> f64`：回调参数依次是会话变量、原 NDV、原行数和选中行数，返回缩放后的 NDV。
- `ScaleNDVFunc: RwLock<Option<ScaleNDVCallback>>`：进程级回调槽。`SetScaleNDVFunc` 获取写锁后安装或清空回调；`StatsInfo::Scale` 获取读锁并要求回调已经安装。
- `GroupNDV { Cols, NDV }`：`Cols` 是列 `UniqueID` 集合的规范序列，`NDV` 是这些列联合取值的估计基数。它可克隆、调试和比较。
- `ToString(&[GroupNDV])`：把列组渲染成稳定的 `[{[id ...] ndv} ...]` 文本，主要供测试/诊断使用。
- `HistCollRef = Arc<dyn Any + Send + Sync>`：在 `pkg/statistics` 尚未成为本 crate 可直接依赖的 crate 时保存真实直方图集合；使用方负责按真实类型 `downcast_ref`，例如 `pkg/planner/cascades/old/implementation_rules.rs::histograms`。
- `StatsInfo`：公开字段为 `RowCount`、`ColNDVs`、`HistColl`、`StatsVersion` 和 `GroupNDVs`。`Clone` 复制值字段并克隆 `Arc`，`Default` 产生零行数、空映射/列组、无直方图和版本 0。
- `Debug`：不展开不透明直方图，只以是否存在的 `"HistColl"` 标记显示。`Display`/`String`：按列 ID 排序后输出行数和逐列 NDV，避免 `HashMap` 迭代顺序造成不稳定文本。
- `Count`：用 Rust 的 `as i64` 把 `RowCount` 转为整数，对齐 Go 的 `int64(s.RowCount)` 使用方式。
- `Scale`：对 `RowCount` 乘 `factor`，分别通过回调缩放每个列 NDV 和列组 NDV；共享 `HistColl`，保留 `StatsVersion`，不修改原对象。
- `ScaleByExpectCnt`：仅当 `expect_count < RowCount` 且 `RowCount > 1` 时按二者比值调用 `Scale`，否则克隆原统计。
- `GetGroupNDV4Cols`：提取输入列的 `UniqueID`、排序，然后与 `GroupNDV.Cols` 做完整切片相等比较；空输入或空列组集合返回 `None`。
- `DeriveLimitStats`：构造新统计，将行数和每个列 NDV分别钳制到 limit 后行数，克隆直方图引用，将版本置 0 并清空列组 NDV。

文件中没有 trait、条件编译项或私有辅助函数；除两个格式化 trait 实现外，业务 API 均由 crate 根再导出。

## 执行流程

典型缩放链如下：

1. 规划器初始化路径调用 `pkg/planner/cardinality/ndv.rs::init`（另有 `pkg/planner/core/optimizer_runtime.rs` 的接线）并通过 `SetScaleNDVFunc` 安装 `ScaleNDV` 包装回调。
2. 上游根据选择率直接调用 `StatsInfo::Scale`，或物理实现根据 `PhysicalProperty.ExpectedCnt` 调用 `ScaleByExpectCnt`；已核对的后者调用点包括 `pkg/planner/cascades/old/implementation_rules.rs::scaled_stats`、`physical_lock.rs` 和 `physical_projection.rs`。
3. `Scale` 在读锁保护下复制出函数指针，计算 `new_row_count = old_row_count * factor`，再将相同的四元输入传给每个单列和列组 NDV。
4. 新值保留原直方图 `Arc` 与 `StatsVersion`，调用方得到独立的 `HashMap`、`Vec<GroupNDV>` 和列 ID 向量。

精确列组估算链为：调用方传入表达式列；`GetGroupNDV4Cols` 按 `UniqueID` 排序并精确匹配已规范化的 `GroupNDV.Cols`；`EstimateColsNDVWithSessionVars` 命中后将 NDV 下限钳制为 1 并返回匹配列数，未命中才走单列/指数退避估算。`logical_aggregation.rs::DeriveStats` 也会把命中的列组复制到聚合输出统计中。

Limit/TopN 链为：`LogicalLimit::DeriveStats`、`LogicalTopN::DeriveStats` 及物理 TopN 候选调用 `DeriveLimitStats`；函数先取 `min(limit_count, child.RowCount)`，再逐列取 `min(column_ndv, row_count)`。直方图作为一种样本描述继续共享，但旧的列组统计和统计版本不被声明为该派生结果的有效属性。

## 数据与状态

`StatsInfo` 是可克隆的值对象。`ColNDVs` 的键必须是表达式列的稳定 `UniqueID`；`GroupNDV.Cols` 必须按与查询匹配一致的升序保存，否则 `GetGroupNDV4Cols` 即使面对同一列集合也不会命中，因为函数只规范化查询侧。代码不在构造时强制这一不变量，生产者（如 `logical_datasource.rs` 和各逻辑算子的 GroupNDV 传播代码）需要负责。

`RowCount`、NDV、factor 和 limit 都是 `f64`。本文件只执行乘法与 `f64::min`，不验证有限性、非负性或 NDV 不超过行数；正常统计域由上游保证，实际 NDV 缩放边界由注入回调处理。`Count` 是面向需要整数行数的便利转换，不是四舍五入。

`HistColl` 的 `Arc` 克隆只增加共享所有权，不深拷贝直方图。`Any + Send + Sync` 保证类型擦除对象可以跨线程共享，但本模块不知道其具体内容。`StatsVersion` 在一般缩放中原样传播，在 `DeriveLimitStats` 中重置为 0；这与 Go 构造新 `StatsInfo` 时未赋该字段所得零值一致。

唯一的可变全局状态是 `ScaleNDVFunc`。它初始为 `None`，安装后可被并发读取，也可由测试或初始化代码替换/清空。

## 依赖与调用关系

crate 内直接依赖只有标准库、`crate::expression` 和 `crate::variable`。后二者由 `lib.rs` 分别从 `astersql-expression`、`astersql-sessionctx-variable` 再导出；`Cargo.toml` 没有 statistics 依赖，这正是 `HistCollRef` 类型擦除和回调注入存在的边界原因。

RustCodeGraph 索引显示本文件包含 13 个符号，并给出这些关键边：

- `StatsInfo::ScaleByExpectCnt -> StatsInfo::Scale`。
- `pkg/planner/cardinality/ndv.rs::init -> SetScaleNDVFunc`；`EstimateColsNDVWithSessionVars -> GetGroupNDV4Cols`。
- `LogicalLimit::DeriveStats`、`LogicalTopN::DeriveStats`、物理 TopN 构造路径 `-> DeriveLimitStats`。
- `physical_lock.rs`、`physical_projection.rs`、`physical_indexmerge_reader.rs` 及 cascades old implementation rules `-> ScaleByExpectCnt`。
- `logical_datasource.rs -> Scale`，选择率由上层统计推导提供。
- `logical_aggregation.rs -> GetGroupNDV4Cols`，精确列组信息随聚合统计传播。

`StatsInfo` 还被计划基类保存为节点统计，并由连接、投影、窗口、Apply、数据源和物理算子广泛构造/传播。图查询对常见符号名存在噪声，因此上述调用边又用对应 Rust 源位置作了交叉核对，没有把同名 Go/其他模块符号计入 Rust 调用链。

## 错误处理与边界

- `Scale` 在回调仍为 `None` 时以明确消息 panic：`property::ScaleNDVFunc must be installed before StatsInfo::Scale`。这是一项初始化顺序契约，不是可恢复业务错误。
- `RwLock` 被 poison 时，读写都用 `PoisonError::into_inner` 继续访问槽位；这避免一次持锁 panic 永久阻断规划器，但不会撤销导致 poison 前可能写入的值。
- `ScaleByExpectCnt` 对期望行数不小于当前行数的情况不放大统计；对当前行数 `<= 1` 也不缩放，沿用 Go 中避免极小分母导致溢出/失真的保护。Rust 返回 clone 而非原对象引用。
- `GetGroupNDV4Cols` 对空列和无列组统计返回 `None`。Rust 的 `&self` 不允许 Go 版本的 nil receiver，所以没有 `s == nil` 分支；调用者必须用 `Option<StatsInfo>` 表达缺失。
- 列组匹配是完整且顺序规范后的相等比较，不做子集匹配；重复列 ID 也不会被去重。
- `DeriveLimitStats` 对普通非负有限输入维持 `ColNDV <= RowCount`。对负数或 NaN 没有显式拒绝，行为遵循 Rust `f64::min`；这些值不属于本文件声明的正常统计输入域。
- `HistCollRef` downcast 失败不在本文件处理；实际消费者需自行决定错误策略，`implementation_rules.rs::histograms` 会返回 `PlannerError`。

## 并发与资源生命周期

`ScaleNDVFunc` 的安装和读取分别由全局 `RwLock` 的写锁、读锁串行化。`Scale` 只在取得函数指针时持读锁；函数指针是 `Copy`，锁守卫随后即可释放，回调执行不会长期占用锁，也不会因回调重入同一锁而直接死锁。回调类型是普通函数指针而不是捕获闭包，不能携带非全局环境状态。

`StatsInfo` 自身没有内部可变性。克隆 `HistColl` 只克隆 `Arc`，最后一个持有者释放时真实对象才销毁；其 `Send + Sync` 边界允许计划数据在线程间安全共享。列 NDV 映射、列组向量和其中的 `Cols` 则在 `Scale` 中重新分配，修改派生结果不会影响来源。

本文件不创建线程、异步任务、通道、文件、网络连接或事务，因此没有额外清理流程。若未来把回调改为可捕获对象，需要重新审视锁持有时间、`Send + Sync` 约束和初始化/替换竞态。

## 与 Go 版本的对应关系

Rust 基本逐项移植 `pkg/planner/property/stats_info.go`：字段、`GroupNDV`、字符串化、`Count`、两种缩放、列组查找和 Limit 推导的数学意图一致。`pkg/planner/property/physical_property_test.rs::statistics_scaling_group_lookup_and_limit_match_go` 用线性回调验证 100 行按 0.5 缩放为 50、列/列组 NDV 同步缩放、版本保留、乱序输入列仍能命中，以及 Limit 后 NDV 截断和列组清空。

需要注意的语言与迁移差异：

- Go 直接保存 `*statistics.HistColl`；Rust 因 crate 边界保存 `Arc<dyn Any + Send + Sync>`，消费者需要 downcast。
- Go 的 `ScaleNDVFunc` 是未同步的包级函数变量；Rust 用 `RwLock<Option<fn>>` 明确支持并发安装/读取，并在未安装时给出确定 panic。
- Go 方法返回 `*StatsInfo`，无需缩放时可返回原指针；Rust 返回拥有所有权的 `StatsInfo`，无需缩放时克隆，避免别名可变性。
- Go `GetGroupNDV4Cols` 接受指针切片并处理 nil receiver；Rust 接受值列切片和非空 `&self`，用 `Option<&GroupNDV>` 表示未命中。
- Go 的 `fmt` 直接打印 map，Rust `Display` 对列 ID 排序以获得确定输出；信息内容保持相同，但精确文本顺序不承诺与 Go map 展示一致。
- Go `DeriveLimitStats` 依赖结构体零值使 `StatsVersion == 0`、`GroupNDVs == nil`；Rust 显式写 0 和空向量，语义等价但空容器表示不同。

Go 侧 `pkg/planner/cardinality/ndv_test.go` 覆盖精确 GroupNDV 优先等下游语义；Rust 对应 `pkg/planner/cardinality/ndv_test.rs`。Go 的 `pkg/planner/core/casetest/stats_test.go` 使用 `property.ToString` 输出列组，Rust 对应 casetest 文件当前也通过同一 API 验证格式化逻辑属性。

## 扩展指南

- 新增 `StatsInfo` 字段时，应同时更新 `Default` 语义、`Debug`、必要的 `Display`、`Scale`、`DeriveLimitStats`，以及所有结构体字面量；先明确该字段在选择率缩放和 Limit/TopN 后是“传播、缩放、清空还是重算”。同步修改 Go 对照仅在对应迁移任务要求时进行，但必须记录语义差异。
- 改变 NDV 缩放规则时，算法应留在 `pkg/planner/cardinality/ndv.rs::ScaleNDV`，通过现有回调边界接入，避免 property -> cardinality 循环依赖。若回调可能未初始化，应在规划器统一入口接线，而不是在 `Scale` 内加入静默降级。
- 新增列组匹配方式时，首先定义 `GroupNDV.Cols` 的规范形式、重复列和子集语义；保持查询侧与生产侧规范化一致，并关注每次查询排序与线性扫描的成本。列组数量显著增长时可考虑规范键索引，但需衡量克隆和内存开销。
- 改动直方图类型时，应同步所有 `HistCollRef` 构造与 downcast 点；类型擦除错误只能在运行时发现，建议把转换集中在有明确错误返回的边界。
- 测试逻辑应继续放在独立文件，不能内嵌到 `stats_info.rs`。直接行为优先扩展 `pkg/planner/property/physical_property_test.rs`；NDV 匹配/缩放算法联动扩展 `pkg/planner/cardinality/ndv_test.rs`；逻辑 Limit/TopN 或聚合传播则在相应 `logical_*_test.rs` 中补回归。
- 兼容风险主要是 Go/Rust 浮点边界、零/NaN 输入、字符串稳定性及 `StatsVersion`/GroupNDV 传播差异；性能风险主要来自逐列/逐组重新分配、列排序和线性扫描。变更这些路径时应同时覆盖正常值、空集合、`RowCount <= 1`、期望值不小于原值、未命中和乱序列输入。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件且目标目录已索引；`files --filter pkg/planner/property` 确认 Rust/Go 对照与独立测试；`node --file pkg/planner/property/stats_info.rs --offset 1 --limit 260` 读取完整 196 行源文件；`query` 核对 `StatsInfo`、`SetScaleNDVFunc`、`ScaleByExpectCnt`、`GetGroupNDV4Cols`、`DeriveLimitStats` 符号；`explore`/调用查询确认关键调用边，并对同名符号噪声作源码复核。
- 源与 crate 边界：`pkg/planner/property/stats_info.rs`、`pkg/planner/property/lib.rs`、`pkg/planner/property/Cargo.toml`。
- Go 对照：`pkg/planner/property/stats_info.go`；下游 Go 测试证据包括 `pkg/planner/cardinality/ndv_test.go` 和 `pkg/planner/core/casetest/stats_test.go`。
- Rust 调用与测试：`pkg/planner/cardinality/ndv.rs`、`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/operator/logicalop/logical_limit.rs`、`logical_top_n.rs`、`logical_aggregation.rs`、`pkg/planner/core/operator/physicalop/physical_lock.rs`、`physical_projection.rs`、`pkg/planner/property/physical_property_test.rs`、`pkg/planner/cardinality/ndv_test.rs`。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前使用任务规定命令验证目标文件存在且恰有 11 个固定二级章节，并人工检查没有把类型擦除、回调初始化或 Go 指针语义误写成 Rust 已提供的行为。
