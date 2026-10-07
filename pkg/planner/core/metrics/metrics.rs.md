# `pkg/planner/core/metrics/metrics.rs`

## 文件定位

本文件是独立 crate `astersql-planner-core-metrics` 的实现文件，由同目录 `lib.rs` 以 `planner_core_metrics` 模块导出；`Cargo.toml` 的 `package.metadata.porting.go-package` 将其明确对应到 Go 包 `pkg/planner/core/metrics`。它位于规划器与 Prometheus 指标设施之间，负责定义规划器 core 所需的指标向量、预绑定固定 `type` 标签的句柄，以及供调用方选择句柄的公开函数。

该文件本身不执行 SQL 规划，也不决定缓存是否命中。业务路径先完成统计估计或计划缓存判断，再通过这里提供的 `Counter`、`Gauge`、`Histogram` 句柄记录结果。Rust 侧统一指标初始化链在 `pkg/util/metricsutil/common.rs::initMetrics` 中调用 `plannercore::InitMetricsVars`；当前可确认的生产计数路径是 `pkg/session/runtime/dispatch.rs` 在 prepared plan-cache 命中后调用 `GetPlanCacheHitCounter(false).inc()`。

## 核心职责

1. 用 `counter_vec` 和 `gauge_vec` 统一构造带 `tidb` namespace、指定 subsystem 和单一 `type` 标签的指标向量。
2. 定义六个底层指标向量：伪统计估计计数、计划缓存命中计数、计划缓存未命中计数、缓存计划数量/淘汰数量仪表、缓存内存仪表，以及计划缓存处理耗时直方图。
3. 将 Go 版本使用的固定标签提前绑定为 15 个 `LazyLock` 句柄，避免调用热路径重复执行标签查找。
4. 通过 `GetPlanCache*` 访问器按布尔参数选择 prepared/non-prepared 或 session/instance 句柄，并返回可克隆的 Prometheus handle。
5. 通过 `InitMetricsVars` 按 Go 版本的绑定顺序强制初始化所有具体句柄，确保统一初始化阶段就完成描述符和标签绑定。

这里“已提供指标 API”和“已接入 Rust 运行时”必须区分：所有访问器都有迁移测试，但仓库当前 Rust 生产代码只检索到统一初始化接线与 prepared 命中计数接线；例如 `pkg/planner/core/stats/stats.rs` 中伪估计递增仍位于保留的 Go 逻辑注释内，不能据此声称 Rust 统计路径已经上报该指标。

## 主要符号

- `counter_vec(subsystem, name, help) -> CounterVec`：内部构造器，固定 namespace 为 `tidb`，并声明唯一可变标签 `type`。用于 `PseudoEstimation`、`PlanCacheCounter` 和 `PlanCacheMissCounter`。
- `gauge_vec(name, help) -> GaugeVec`：内部构造器，固定 namespace/subsystem 为 `tidb/server`，同样只声明 `type` 标签。用于计划数量和内存指标。
- `PseudoEstimation`：`tidb_statistics_pseudo_estimation_total` 的向量；公开句柄 `PseudoEstimationNotAvailable`/`PseudoEstimationOutdate` 分别绑定 `nodata`/`outdate`。
- `PlanCacheCounter`：`tidb_server_plan_cache_total`，绑定 `prepared` 和 `non-prepared` 两类命中计数器。
- `PlanCacheMissCounter`：`tidb_server_plan_cache_miss_total`，绑定 `prepared`、`non-prepared` 和 `non-prepared-unsupported`。
- `PlanCacheInstancePlanNumCounter`：`tidb_server_plan_cache_instance_plan_num_total`，既承载 session/instance 缓存计划数量，也承载标签为 ` instance-plan-cache-last-evict` 的最近淘汰数量。
- `PlanCacheInstanceMemoryUsage`：`tidb_server_plan_cache_instance_memory_usage`，分别绑定 session 与 instance 级内存用量。
- `PlanCacheProcessDuration`：`tidb_server_plan_cache_process_duration_seconds`，采用 `exponential_buckets(0.001, 2.0, 28)`，即从 1 ms 起、倍率 2、共 28 个 bucket；绑定 session lookup、instance lookup 和 instance clone 三种操作。
- `init()`：薄入口，仅调用 `InitMetricsVars()`。
- `InitMetricsVars()`：依次 `LazyLock::force` 15 个公开或私有的具体句柄。该函数不重置现有值，也不执行业务计数。
- `GetPlanCacheHitCounter(bool)` / `GetPlanCacheMissCounter(bool)`：参数为 `true` 时选 non-prepared，否则选 prepared。
- `GetNonPrepPlanCacheUnsupportedCounter()`：返回 non-prepared 不支持计数器。
- `GetPlanCacheInstanceNumCounter(bool)` / `GetPlanCacheInstanceMemoryUsage(bool)`：参数为 `true` 时选 instance，否则选 session。
- `GetPlanCacheCloneDuration()`：固定返回 instance clone 直方图。
- `GetPlanCacheLookupDuration(bool)`：参数为 `true` 时选 instance lookup，否则选 session lookup。
- `GetPlanCacheInstanceEvict()`：返回绑定到计划数量向量的 last-evict Gauge。

除两个伪估计 `LazyLock<Counter>` 句柄外，预绑定的计划缓存句柄均为私有静态量；调用方应使用公开访问器，而不是重新拼装标签。

## 执行流程

初始化流程如下：

1. `pkg/util/metricsutil/common.rs::initMetrics` 在父级 collector 初始化之后调用 `plannercore::InitMetricsVars`。
2. `InitMetricsVars` 按 Go `metrics.go` 的顺序强制求值两个伪估计句柄、五个命中/未命中句柄、五个数量/内存/淘汰 Gauge 和三个耗时 Histogram。
3. 首次求值具体句柄时，会先求值相应底层向量；底层向量通过 `CounterVec::new`、`GaugeVec::new` 或 `HistogramVec::new` 构造。
4. `with_label_values` 将固定标签绑定到子指标。后续访问器只克隆这一已绑定 handle，克隆后仍操作同一底层时序状态。

运行时选择流程以命中计数为例：调用方先判断缓存命中，再把 `isNonPrepared` 传给 `GetPlanCacheHitCounter`；函数以布尔分支选取标签为 `non-prepared` 或 `prepared` 的静态 handle，克隆并返回；调用方最后执行 `inc()`。`pkg/session/runtime/dispatch.rs` 当前传入 `false`，因此计入 prepared 序列。`pkg/planner/core/tests/pointget/point_get_plan_test.rs` 读取同一 handle 的前后值，验证第二次执行 prepared statement 时计数增加。

Gauge 访问器只负责选择时间序列，不维护绝对值不变量；数量、内存与淘汰值应由业务调用方以 `set`/`add`/`sub` 等操作维护。Histogram 访问器也不计时，调用方必须以秒为单位执行 `observe`。

## 数据与状态

所有指标定义和具体句柄均为进程级静态状态，使用 `std::sync::LazyLock` 延迟创建。底层状态由 `prometheus` 0.13.4 的 `CounterVec`、`GaugeVec`、`HistogramVec` 及其子 handle 持有：

- `Counter` 适合只增不减的事件累计；伪估计、缓存命中、缓存未命中和“不支持”均使用此类型。
- `Gauge` 可增、减或设置，适合缓存计划数、内存占用和最近淘汰数量。
- `Histogram` 累计样本数、总和与 bucket 计数，适合 lookup/clone 秒级耗时。

标签是兼容性数据，不是展示文本。特别是 session/instance/evict/lookup/clone 的标签值均以一个前导空格开头，例如 `" instance-plan-cache"`；这与 Go 文件逐字一致，并由 `migration_aster_unit_test.rs` 明确验证。修改或清理这些空格会创建不同的 Prometheus 时间序列，属于兼容性变更。

访问器返回 handle 的克隆而非指标值快照。`migration_aster_unit_test.rs::plan_cache_counter_selectors_match_go_labels_and_share_real_state` 对一个克隆执行 `inc_by(2.0)`，随后重新获取 prepared handle 仍观察到增量，证明多个 handle 共享底层状态。

## 依赖与调用关系

直接依赖只有同 crate 别名导入的 `prometheus`（Cargo 包名为 `prometheus13`，实际上游 package 为 `prometheus` 0.13.4）以及标准库 `LazyLock`。`lib.rs` 将依赖重命名为 `prometheus`，并把本文件公开为 `planner_core_metrics`。

已核实的上游关系：

- `pkg/util/metricsutil/common.rs::initMetrics -> planner_core_metrics::InitMetricsVars`：统一初始化入口。
- `pkg/session/runtime/dispatch.rs -> GetPlanCacheHitCounter(false) -> Counter::inc`：prepared plan-cache 命中生产路径。
- `pkg/planner/core/tests/pointget/point_get_plan_test.rs -> GetPlanCacheHitCounter(false)`：SQL 层回归测试读取命中计数。
- `pkg/planner/core/metrics/migration_aster_unit_test.rs`：直接覆盖全部公开访问器和两个公开伪估计句柄。

Cargo 反向依赖包括 `pkg/util/metricsutil`、`pkg/session`、`pkg/domain`、`pkg/planner/core/stats` 和 point-get 测试 crate；依赖声明只证明 crate 可见性，不等价于每个 API 已在生产路径调用。RustCodeGraph 对目标文件列出的直接使用文件为 `pkg/util/metricsutil/common.rs`、本目录迁移测试、point-get 测试及会话运行时相关接线；原始检索进一步确认当前未发现其他访问器的 Rust 生产调用。

Go 侧业务接线更完整：`plan_cache.go` 使用 hit/miss/lookup/clone，`plan_cache_lru.go` 使用计划数和内存 Gauge，`plan_cacheable_checker.go` 使用 unsupported counter。它们是解释 API 设计意图和后续 Rust 接线位置的直接对照，但不是 Rust 已接线的证据。

## 错误处理与边界

本文件没有返回 `Result` 的运行时错误路径。指标描述符创建失败时，构造器通过 `expect("planner metric descriptor must be valid")` 立即 panic；直方图 bucket 参数非法时通过 `expect("valid buckets")` panic。这些参数全部是编译期固定字面量，因此 panic 表达的是开发期不变量被破坏，而不是可恢复的请求错误。

访问器不会验证调用语义：布尔值传错只会把样本写入另一条合法时间序列；Histogram 不检查调用方是否真的传入秒；Gauge 不阻止负值；Counter 的合法增量规则由 Prometheus 类型实现负责。因而调用点必须保留以下边界：

- `isNonPrepared=true` 只能代表 non-prepared plan cache。
- `instancePlanCache=true` 只能代表 instance cache。
- duration 必须使用秒，保持 `_seconds` 指标单位。
- 标签字符串（包括前导空格）、namespace、subsystem、指标名和 bucket 配置均属于可观测性兼容契约。

当前文件只创建并绑定指标，不单独调用 registry 注册函数；父级 collector/统一注册生命周期由 `pkg/util/metricsutil/common.rs` 负责。不要在访问器内重复注册或重建向量，否则可能产生重复描述符或状态分裂。

## 并发与资源生命周期

`LazyLock` 提供线程安全的一次初始化：并发首次访问时只有一个初始化结果被发布，之后所有线程复用同一静态指标对象。`InitMetricsVars` 可重复调用；`LazyLock::force` 在首次完成后不会重新创建、清零或替换指标，因此不会丢失已累计样本。

返回的 Prometheus handles 可廉价克隆，并共享底层原子指标状态；它们不是需要显式关闭的资源，也没有锁、通道、事务、后台任务或异步取消生命周期。本文件拥有的状态持续到进程退出。高频路径预绑定标签，避免每次记录时查找/创建 label child；扩展时应继续沿用这一模式，防止不受控标签基数和热路径额外开销。

## 与 Go 版本的对应关系

Rust `InitMetricsVars` 与 `pkg/planner/core/metrics/metrics.go::InitMetricsVars` 的 15 个绑定及顺序一致；8 个公开访问器的布尔分支也逐一对应。主要实现差异是：

- Go 从共享 `pkg/metrics` 取得父级向量并把具体句柄赋给包级变量；Rust 在本文件内以 `LazyLock` 定义底层向量和具体句柄。
- Go 返回 `prometheus.Observer` 接口用于耗时观测；Rust 返回具体 `Histogram` handle。
- Go 包 `init()` 自动调用初始化；Rust 提供 `init()`，同时由 `pkg/util/metricsutil/common.rs` 明确调用 `InitMetricsVars`。Rust 模块加载本身不会像 Go 包初始化那样自动执行普通函数。
- Rust handle 访问器通过 `clone()` 返回共享句柄；Go 接口值直接返回已绑定对象。两者在累计同一时间序列这一语义上由迁移测试核对。

Go 生产路径对这些 API 的使用范围大于当前 Rust 路径。尤其伪统计计数、miss、unsupported、instance/session Gauge 以及 lookup/clone Histogram 虽已实现并测试，仍应以实际 Rust 调用搜索为准判断迁移完成度。`pkg/planner/core/stats/stats.rs` 的注释保留了 Go 的伪统计递增位置，但不是可执行 Rust 逻辑。

## 扩展指南

新增指标或标签分支时，建议按以下接入点操作：

1. 在本文件增加或复用底层 `*Vec`，保持 `tidb` namespace 和与 Go 一致的 subsystem/name/help/buckets。
2. 为固定标签增加私有 `LazyLock` 预绑定句柄；只有确需业务直接访问时才公开静态量或访问器。
3. 把新句柄加入 `InitMetricsVars`，并保持与 Go 对照文件相同的初始化顺序。
4. 在 `pkg/planner/core/metrics/migration_aster_unit_test.rs` 增加标签、共享状态和可观测性断言；测试逻辑必须继续独立于生产源文件。
5. 若接入计划缓存行为，还应在最近的独立业务测试补充断言，例如 point-get 场景位于 `pkg/planner/core/tests/pointget/point_get_plan_test.rs`；若接入伪统计路径，则应在 stats 的独立测试文件覆盖真实触发条件。
6. 搜索 Go 调用点和 Rust 对应实现，分别证明 API 对齐与运行时接线，不以 Cargo 依赖或注释代替调用证据。

兼容风险集中在指标全名、标签键/值（尤其前导空格）、布尔分支方向和 histogram buckets；性能风险集中在热路径动态绑定标签、重复构造向量或增加高基数标签；正确性风险集中在 Gauge 增减不平衡、错误单位及把 miss/hit 写入错误类别。新增独立 Rust 源文件或测试时还应遵守仓库 Bazel 元数据规则，但仅修改本说明文档不触发代码构建。

## 验证依据

- 源码与模块边界：`pkg/planner/core/metrics/metrics.rs`（全部定义）、`pkg/planner/core/metrics/lib.rs`（模块导出和独立测试接线）、`pkg/planner/core/metrics/Cargo.toml`（crate 名、Prometheus 0.13.4 依赖、Go 包映射）。目标目录没有 `doc.go`。
- Go 对照：`pkg/planner/core/metrics/metrics.go`；调用意图还由 `pkg/planner/core/plan_cache.go`、`plan_cache_lru.go` 和 `plan_cacheable_checker.go` 核验。
- Rust 运行时证据：`pkg/util/metricsutil/common.rs::initMetrics` 的统一初始化调用；`pkg/session/runtime/dispatch.rs` 的 prepared hit 递增；`pkg/planner/core/stats/stats.rs` 中尚未执行的 Go 逻辑注释用于界定迁移缺口。
- 独立测试：`pkg/planner/core/metrics/migration_aster_unit_test.rs` 验证全部标签选择、前导空格、共享 Counter 状态和 Histogram 样本；`pkg/planner/core/tests/pointget/point_get_plan_test.rs` 验证 SQL prepared plan-cache 命中增量。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 4 个已索引文件；`node --file pkg/planner/core/metrics/metrics.rs` 返回完整 225 行和使用文件；`query` 定位 `InitMetricsVars`、`GetPlanCacheHitCounter`、`GetPlanCacheLookupDuration`；`explore` 给出 `init -> InitMetricsVars`、测试/point-get 调用边。精确 `callers/callees` 对同名符号存在歧义，因此调用范围又以仓库 `rg` 结果交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工检查每项运行时状态陈述均区分源码事实、Go 对照与未接线部分。
