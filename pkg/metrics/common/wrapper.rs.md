# `pkg/metrics/common/wrapper.rs`

## 文件定位

该文件是 `astersql-metrics-common` crate 的核心实现，由同目录的 [`lib.rs`](lib.rs) 以 `pub mod wrapper` 声明并通过 `pub use wrapper::*` 重新导出。crate 在 [`Cargo.toml`](Cargo.toml) 中只直接依赖 `prometheus = "0.14"`，并用 `package.metadata.porting.go-package = "pkg/metrics/common"` 标明其 Go 对照包。

它位于业务模块与 Rust Prometheus 客户端之间：上游先通过 `SetConstLabels` 配置进程级标签，随后各模块用 `NewCounter*`、`NewGauge*`、`NewHistogram*` 等工厂创建采集器。典型上游包括 `pkg/util/metricsutil/common.rs` 的注册流程、`pkg/metrics/*` 的指标初始化，以及 `pkg/dxf/framework/dxfmetric/collector.rs` 的自定义描述符构造。文件不负责注册 collector、暴露 HTTP scrape 端点或更新指标值。

## 核心职责

1. 维护一个进程内、线程安全的包级常量标签表 `CONST_LABELS`。
2. 提供标签快照、合并和整体替换操作，并保证包级标签在键冲突时优先。
3. 在创建 Counter、Gauge、Histogram 及其向量时，把调用方选项中的常量标签**替换**为当前包级标签快照，从而统一注入集群、节点或 keyspace 等维度。
4. 为 Go 的 `SummaryVec` 提供 `HistogramVec` 兼容降级，并明确不提供客户端流式分位数。
5. 为自定义 collector 构造 `prometheus::core::Desc`，此路径会合并调用方常量标签，而不是像普通工厂那样直接替换。

这些职责分别由 `SetConstLabels`、`GetConstLabels`/`GetMergedConstLabels`、八个 `New*` 工厂以及 `NewDesc` 承担；当前实现是有真实调用者的兼容包装层，不是桩或仅装配模块。

## 主要符号

- `pub type Labels = HashMap<String, String>`：与 Prometheus 常量标签的拥有型键值映射相适配。
- `static CONST_LABELS: OnceLock<RwLock<Labels>>`：惰性创建的全局标签容器。`OnceLock` 固定锁实例，`RwLock` 保护之后的读写。
- `fn const_labels() -> &'static RwLock<Labels>`：唯一的内部存取入口；首次调用时创建空映射。
- `pub fn GetConstLabels() -> Labels`：持读锁克隆映射，返回与全局状态脱离的快照。
- `pub fn GetMergedConstLabels(input: Labels) -> Labels`：把全局快照覆盖式扩展到输入映射中；全局同名键胜出。输入为空时直接返回全局快照。
- `pub fn SetConstLabels(kv: &[String])`：接收交替排列的键和值，要求元素数为偶数；键经 `to_lowercase` 规范化后整体替换全局映射。规范化后重复的键由后出现的键值对覆盖。
- `NewCounter`、`NewGauge`：接收 `prometheus::Opts`，覆盖 `opts.const_labels` 后调用对应的 `with_opts`。
- `NewCounterVec`、`NewGaugeVec`：除注入标签外，把 `&[String]` 临时投影成 `Vec<&str>`，再构造向量 collector。
- `NewHistogram`、`NewHistogramVec`：标签位于 `HistogramOpts.common_opts.const_labels`；向量版本同样转换可变标签名。
- `NewSummaryVec(opts: HistogramOpts, labelNames: &[String]) -> HistogramVec`：签名和返回类型直接体现兼容降级；它不是 Prometheus Summary。
- `NewDesc(...) -> prometheus::Result<Desc>`：保留调用方标签并以全局标签覆盖冲突键，且把描述符校验错误交给调用者。
- `CONST_LABELS_TEST_LOCK`：仅在 `cfg(test)` 下存在，供独立测试串行化会改写全局标签的用例，不属于生产 API。

文件使用 `#![allow(non_snake_case)]` 保留 Go 风格公开名称，降低迁移调用点的命名差异。

## 执行流程

常规启动/注册链如下：

1. `pkg/util/metricsutil/common.rs::RegisterMetrics` 或 `RegisterMetricsForBR` 根据运行模式写入 `keyspace_name`；BR 路径取得 PD 元数据后还会通过 `setKeyspaceIDConstLabel` 合入 `keyspace_id`。
2. `metricsutil::setConstLabels` 把映射按键排序并展开为交替键值数组，调用本文件的 `SetConstLabels`。排序不是本文件的不变量，只让上游展开顺序稳定；最终存储仍是 `HashMap`。
3. `SetConstLabels` 校验偶数长度、统一键大小写、取得写锁，并一次性替换整张表。
4. 各指标初始化函数调用 `NewCounter*`、`NewGauge*`、`NewHistogram*`。工厂通过 `GetConstLabels` 在创建时取得快照，覆盖传入 opts 自带的常量标签，再交给 `prometheus` crate 校验和构造。
5. collector 随后由其他模块注册和更新；之后再次调用 `SetConstLabels` 不会追溯修改已经构造的 collector。

描述符路径略有不同：`pkg/dxf/framework/dxfmetric/collector.rs::descriptor` 调用 `NewDesc`，后者经 `GetMergedConstLabels` 保留例如测试生成的 `server_id`，同时让包级同名标签拥有更高优先级，最后调用 `Desc::new`。

## 数据与状态

唯一持久状态是 `CONST_LABELS` 中的 `Labels`。其初始值为空；`SetConstLabels(&[])` 会清空，而不是保持旧值。每次写入都构造一张新表后在写锁内替换，因此读者看到的是替换前或替换后的完整映射，不会看到逐项更新的中间状态。

`GetConstLabels` 总是克隆，调用者修改返回值不会改变全局状态。`GetMergedConstLabels` 消耗输入映射：输入非空时复用其分配并以全局快照扩展；输入为空时返回全局快照本身。两条路径都把结果所有权交给调用者。

工厂保存的是构造瞬间的标签副本。可变标签名则只在构造调用期间借用，Prometheus collector 完成自身描述符构造后不依赖传入的 `String` 切片生命周期。Histogram 的 bucket 配置、指标名、namespace、subsystem 和 help 均原样交给底层库；包装层只改常量标签。

## 依赖与调用关系

下游依赖只有标准库的 `HashMap`、`OnceLock`、`RwLock` 和 `prometheus` crate。RustCodeGraph 的精确被调用关系确认：`GetConstLabels -> const_labels`、`SetConstLabels -> const_labels`、`NewSummaryVec -> GetConstLabels`、`NewDesc -> GetMergedConstLabels`；源码中其余指标工厂也都直接调用 `GetConstLabels`。

主要上游关系包括：

- `pkg/util/metricsutil/common.rs::{RegisterMetrics, RegisterMetricsForBR, setConstLabels}` 写入标签，`cloneConstLabels` 读取标签；这是运行时配置与本包装层之间的主要接线。
- `pkg/metrics/{distsql,executor,server,stats,ttl,...}.rs` 通过工厂初始化大量 TiDB 指标。例如 `pkg/metrics/executor.rs` 的执行阶段耗时使用 `NewSummaryVec` 兼容入口。
- `pkg/metrics/bindinfo.rs` 提供另一层 Go 形状适配，把本文件工厂重新暴露给迁移代码；其 `SummaryOpts` 会先转换为 Histogram 配置。
- `pkg/dxf/framework/dxfmetric/collector.rs::descriptor` 使用 `NewDesc` 为自定义采集器构造描述符。
- `pkg/lightning/metric/metric.rs` 主要通过自己的 factory 抽象构造指标；它也读取本 crate 的全局常量标签语义。

RustCodeGraph 对该文件给出的直接使用文件包括 `pkg/dxf/framework/dxfmetric/collector.rs`、`pkg/lightning/metric/metric.rs`、`pkg/metrics/bindinfo.rs`、`pkg/util/metricsutil/common.rs` 及其测试；限定范围的调用点搜索还显示各 `pkg/metrics/*.rs` 通过 crate 再导出的 `metricscommon::New*` 广泛使用这些工厂。

## 错误处理与边界

- `SetConstLabels` 收到奇数个字符串时立即 panic，错误文本包含实际元素数；这与 Go 版本一致，并由 `odd_const_label_arguments_panic_like_go` 验证。
- Counter/Gauge/Histogram 工厂对底层无效选项调用 `expect`，因此重复标签、非法名称等构造错误会 panic。它们适用于静态、开发期可验证的指标定义，不适合直接承接不可信运行时定义。
- `NewDesc` 不 panic 包装底层校验，而是返回 `prometheus::Result<Desc>`；上游可传播错误或像 DXF 的 `descriptor` 一样附带领域上下文 `expect`。
- `RwLock` 中毒时，读写路径均以 `poisoned.into_inner()` 继续使用内部数据，避免一次 panic 让指标标签永久不可访问；这也意味着中毒不是对外可观察的错误。
- 普通 `New*` 工厂丢弃调用方 opts 中已有的全部常量标签，而 `NewDesc` 会合并。扩展或调用时必须区分这两个契约。
- `NewSummaryVec` 不计算 Go Summary 的流式 quantile；需要 quantile 语义的调用者不能把该兼容类型视为完全等价实现。

## 并发与资源生命周期

`OnceLock` 的初始化线程安全，`RwLock` 允许多个并发快照读取者，并把 `SetConstLabels` 串行化。锁只覆盖克隆或整表赋值，Prometheus collector 的实际构造发生在读锁释放后，因此不会在全局锁内执行较重的描述符创建。

锁和标签表均为进程生命周期静态对象，没有显式销毁。创建出的 collector 拥有自己的标签与描述符，生命周期独立于全局表。并发调用 `SetConstLabels` 和任一 `New*` 时，collector 会获得某一次完整标签快照，但代码不保证它对应哪一次竞争写入；生产流程应在初始化 collector 前完成标签配置。

测试会改写同一全局状态，所以 `wrapper_test.rs` 与 `migration_aster_unit_test.rs` 使用 `CONST_LABELS_TEST_LOCK` 串行化，并用 `ConstLabelsGuard::drop` 恢复进入测试前的快照。这个互斥锁只约束遵守该约定的测试，不参与生产同步。

## 与 Go 版本的对应关系

直接对照文件是 [`wrapper.go`](wrapper.go)，独立 Go 回归是 [`wrapper_test.go`](wrapper_test.go)。Rust 保留了 Go 的公开函数名、偶数键值对约束、键转小写、全局标签覆盖输入标签、普通工厂替换 ConstLabels 以及 `NewDesc` 合并标签的总体行为。

关键差异如下：

- Go 的 `constLabels` 是未加锁的包变量，`GetConstLabels` 可直接返回底层 map；Rust 用 `OnceLock<RwLock<_>>` 并返回 clone，提供线程安全和所有权隔离。
- Go 的空输入合并可能直接返回包级 map；Rust 始终返回拥有型快照，不存在调用方通过结果别名修改全局表的情况。
- Go Prometheus 客户端原生提供 `SummaryVec`；Rust `prometheus 0.14` 没有 Summary collector，因此 `NewSummaryVec` 接受 `HistogramOpts` 并返回 `HistogramVec`。观测、count、sum、标签和注册能力保留，客户端流式 quantile 不保留。
- Go 构造器返回接口或指针且由客户端库按自身约定处理定义错误；Rust 普通工厂将 `Result` 收敛为 panic，而 `NewDesc` 显式保留 `Result`。
- Rust 为共享全局状态额外提供测试专用 `CONST_LABELS_TEST_LOCK`；Go 测试默认串行并用 `t.Cleanup` 恢复状态。

[`wrapper_test.rs`](wrapper_test.rs) 逐项复刻 Go 的 `TestGetMergedConstLabels`。[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步覆盖 Go 对齐所需的小写化、panic 文本、各 collector 的标签注入、Histogram/Summary 观测和描述符合并。

## 扩展指南

- 新增 collector 工厂时，应沿用“先取得 `GetConstLabels` 快照、再构造底层 collector”的结构，并把测试放入独立的 `wrapper_test.rs` 或 `migration_aster_unit_test.rs`，不要把测试嵌入生产文件。
- 若新 API 接收调用方常量标签，必须先明确是普通工厂的“整体替换”还是 `NewDesc` 的“合并且全局优先”，并为冲突键增加回归用例。
- 调整标签规范化时，应修改 `SetConstLabels`，同步核对 `metricsutil::setConstLabels`、Go `SetConstLabels` 以及 keyspace 标签调用链；标签名变化会改变 Prometheus 时序，具有兼容性和基数风险。
- 若未来引入真正的 Rust Summary，实现不能静默替换当前 Histogram 时序：应评估指标类型、导出样本名、quantile、bucket、告警与 dashboard 兼容性，并同步 `pkg/metrics/executor.rs`、`pkg/metrics/bindinfo.rs` 和迁移测试。
- 若需要在指标构造后动态改变常量标签，不能只修改 `CONST_LABELS`；现有 collector 已持有快照，需要设计重新创建和重新注册的完整生命周期。
- 保持锁临界区短小。不要在持有 `CONST_LABELS` 锁时注册 collector、访问网络或调用可重入的外部回调。
- 普通工厂目前用 `expect` 表达“指标定义是静态且必须有效”的不变量；若新增运行时定义入口，应考虑返回 `Result`，并让调用方提供可诊断上下文。

## 验证依据

- 目标源码：[`wrapper.rs`](wrapper.rs)，核对了 13 个索引符号、所有公开签名、内部锁访问、标签覆盖和构造逻辑。
- crate 边界：[`lib.rs`](lib.rs) 与 [`Cargo.toml`](Cargo.toml)，确认模块再导出、独立测试装配、唯一直接外部依赖及 Go 包映射。
- Go 对照：[`wrapper.go`](wrapper.go) 与 [`wrapper_test.go`](wrapper_test.go)，确认命名、覆盖优先级、键规范化、构造器与测试意图。
- Rust 测试：[`wrapper_test.rs`](wrapper_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，确认空输入、并集、冲突覆盖、小写化、奇数参数 panic、所有 collector 注入、Summary 降级和 Desc 合并。
- 上游证据：`pkg/util/metricsutil/common.rs` 的标签设置/注册链、`pkg/dxf/framework/dxfmetric/collector.rs::descriptor`、`pkg/metrics/executor.rs` 的 Summary 调用，以及 `pkg/metrics/bindinfo.rs` 的迁移适配层。
- RustCodeGraph：运行了索引状态与文件查询、目标文件 `explore`/`node`、主要符号 `query`，并对 `GetConstLabels`、`SetConstLabels`、`NewSummaryVec`、`NewDesc` 执行 `callers`/`callees`；精确 callers 未覆盖的再导出调用点用限定目录的 `rg` 补充。
- 本任务是只读代码分析与文档新增；按计划不运行 Cargo。最终结构检查要求本文恰有十一个规定的二级标题。
