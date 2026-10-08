# `pkg/session/metrics/metrics.rs`

## 文件定位

本文件是 `astersql-session-metrics` crate 中的会话侧指标句柄绑定层。crate 入口 `pkg/session/metrics/lib.rs` 以 `session_metrics` 模块公开本文件，同时在内嵌的 `metrics` 模块中加载 `pkg/metrics/session.rs` 定义的包级 collector，并公开 `pkg/metrics/telemetry.rs` 的遥测指标。`pkg/session/metrics/Cargo.toml` 表明该 crate 只直接依赖 `prometheus = "0.14"`，没有 feature 分支；本文件本身也没有条件编译项。

在应用初始化链上，`pkg/util/metricsutil/common.rs` 的 `RegisterMetrics`/`registerMetrics` 先建立父级 collector，再由 `initMetrics` 调用 `session_metrics::InitMetricsVars`。因此，本文件不定义指标名称、帮助文本、桶边界或注册表策略，而是把已经存在的 collector 绑定到会话运行时所需的固定标签组合，得到可直接 `inc` 或 `observe` 的句柄。

## 核心职责

本文件承担三项职责。

1. 用 `Counter = prometheus::Counter` 和 `Observer = prometheus::Histogram` 统一表达会话调用侧使用的计数器、直方图句柄。
2. 维护 51 个 `pub static mut Option<...>` 别名：3 个非事务 DML 计数器、18 个事务语句数/耗时/重试观察者、4 个解析与编译耗时观察者，以及 26 个遥测计数器。初始值全部为 `None`，初始化后指向父级指标的具体 label series。
3. 通过 `InitMetricsVars` 集中建立上述绑定，并以 `INIT_LOCK` 串行化初始化和重复初始化；`init` 只是与 Go 包初始化形态对应的显式包装函数。

它不是指标采集调用点，也不拥有 Prometheus 注册流程。当前 Rust 生产接线通过 `pkg/util/metricsutil/common.rs::initMetrics` 调用 `InitMetricsVars`；具体业务侧是否读取每一个公开静态句柄，应以相应业务模块的引用为准，不能仅因本文件定义了句柄就推断所有 Go 消费点均已迁移。

## 主要符号

- `Counter`、`Observer`（第 26、28 行）：分别是 `prometheus::Counter` 与 `prometheus::Histogram` 的公开类型别名。Go 的 `prometheus.Observer` 是接口；Rust 这里选择具体 `Histogram`，因为 `HistogramVec::with_label_values` 返回该类型。
- `INIT_LOCK: Mutex<()>`（第 31 行）：私有进程内互斥锁，只保护本文件的整批句柄重绑定。
- `NonTransactional{Delete,Insert,Update}Count`（第 34–38 行）：从 `metrics::NonTransactionalDMLCount` 按 `delete`、`insert`、`update` 绑定出的三个计数器。
- `StatementPerTransaction*`（第 41–55 行）：覆盖事务模式 `Pessimistic/Optimistic`、结果 `OK/Error`、来源 `Internal/General` 的 2×2×2 八个直方图句柄。
- `TransactionDuration*`（第 57–71 行）：覆盖事务模式、结束方式 `Commit/Abort`、来源的 2×2×2 八个耗时直方图句柄；`TransactionRetry{Internal,General}`（第 73–75 行）则只按来源拆分。
- `SessionExecute{Compile,Parse}Duration{Internal,General}`（第 78–84 行）：解析和编译阶段各自按内部/普通会话拆分的四个直方图句柄。
- `Telemetry*`（第 87–140 行）：CTE、multi-schema change、flashback cluster、十五类分区统计、账户操作、index merge 和 store batched query 的固定序列。分区别名依赖 `TelemetryMetrics::table_partition` 的 15 元数组顺序（`pkg/metrics/telemetry.rs::TelemetryMetrics`）。
- `init()`（第 144 行）：无条件调用 `InitMetricsVars`；Rust 不会像 Go 那样自动执行普通函数，必须由调用方显式调用。
- `InitMetricsVars()`（第 150 行）：唯一核心入口；获取锁、保证父级会话指标存在、依次绑定所有句柄。命名保留 Go 风格，crate 根在 `pkg/session/metrics/lib.rs` 通过 lint allow 允许非 snake case 和全局可变静态引用。

## 执行流程

`InitMetricsVars` 的流程如下。

1. 获取 `INIT_LOCK`。若锁曾因 panic 中毒，`unwrap_or_else(|poisoned| poisoned.into_inner())` 仍取回 guard，允许重新完成整批绑定。
2. 进入 `unsafe` 区域访问 51 个 `static mut` 句柄及父级 `static mut` collectors。若 `metrics::NonTransactionalDMLCount` 尚为 `None`，先调用 `pkg/metrics/session.rs::InitSessionMetrics` 创建会话父级指标；这里以该字段作为整组父级会话指标是否初始化的哨兵。
3. 对非事务 DML 使用单标签 `delete`、`insert`、`update` 建立三个 counter series。
4. 对 `StatementPerTransaction` 和 `TransactionDuration` 分别按三个标签绑定八种组合。标签顺序与父级定义一致：前者/后者均为事务模式、结果或结束方式、来源（见 `pkg/metrics/session.rs::InitSessionMetrics` 的 `LblTxnMode, LblType, LblScope`）。
5. 将 `SessionRetry`、`SessionExecuteCompileDuration`、`SessionExecuteParseDuration` 按 `Internal/General` 两类来源绑定。
6. 调用 `metrics::telemetry::init_telemetry_metrics()` 获取由 `OnceLock` 保存的 `TelemetryMetrics`，绑定 CTE 和账户操作的 label series，并克隆其余单 counter 或分区数组成员句柄。
7. guard 在函数返回时释放。函数没有返回值；成功后所有 51 个公开 `Option` 均应为 `Some`。

重复调用会重新取得同一 label values 对应的 Prometheus 句柄并覆盖各 `Option`，不会创建新的逻辑时间序列。`pkg/session/metrics/migration_aster_unit_test.rs::init_metrics_vars_matches_go_labels_aliases_and_is_idempotent` 通过先后读取计数值验证这一点。

## 数据与状态

本文件不保存业务数据，只保存指向 Prometheus 度量序列的可克隆句柄。51 个公开全局变量使用 `Option` 表达初始化前/后的状态：`None` 表示尚未绑定，`Some(Counter/Histogram)` 表示已绑定。调用侧必须处理初始化时序，不能在未执行注册/初始化链时假定为 `Some`。

父级状态分为两类：会话 collector 位于 `crate::metrics`，实际由 `pkg/metrics/session.rs` 内联进入当前 crate；遥测状态位于 `pkg/metrics/telemetry.rs` 的 `TELEMETRY_METRICS: OnceLock<TelemetryMetrics>`。`init_telemetry_metrics` 先读 `OnceLock`，必要时构造一次，然后返回 `'static` 引用。本文件克隆的 Prometheus 句柄共享底层指标，不复制样本值。

分区遥测使用数组位置表达语义，绑定顺序是总体、LIST、RANGE、HASH、RANGE COLUMNS、其列数阈值三项、LIST COLUMNS、最大分区数、创建/添加/删除 INTERVAL、COMPACT、REORGANIZE；`exchange_partition` 是独立字段而非数组成员。这个顺序是扩展时最容易发生错位的隐含契约。

## 依赖与调用关系

上游关系：

- `pkg/util/metricsutil/common.rs::RegisterMetrics` 和 `RegisterMetricsForBR` 汇入私有 `registerMetrics`；后者调用 `initMetrics`，而 `initMetrics` 在父级 collectors 初始化之后调用本文件的 `InitMetricsVars`。
- `pkg/session/metrics/metrics.rs::init` 也直接调用 `InitMetricsVars`，用于保持 Go 的 `init()` 结构对应，但当前检索到的生产全局初始化链是 `metricsutil` 路径。
- `pkg/session/metrics/migration_aster_unit_test.rs` 直接调用入口并读取全部别名；RustCodeGraph 的文件使用关系还列出 `pkg/session/runtime/normal_ddl_test.rs` 和 `pkg/session/runtime_test/typed_adapter_bridge.rs`，但源码检索没有发现它们直接引用本文件符号，因此不能把文件级依赖等同为函数调用。

下游关系：

- `crate::metrics::InitSessionMetrics` 定义于 `pkg/metrics/session.rs`，创建 `NonTransactionalDMLCount`、`StatementPerTransaction`、`TransactionDuration`、`SessionRetry`、解析/编译耗时等父级向量。
- `prometheus::{CounterVec,HistogramVec}::with_label_values` 根据固定字符串和 `metrics::Lbl*` 常量选取具体 series；返回的 `Counter`/`Histogram` 句柄被写入公开静态变量。
- `crate::metrics::telemetry::init_telemetry_metrics` 定义于 `pkg/metrics/telemetry.rs`，返回进程生命周期内共享的 `TelemetryMetrics`。

crate 边界由 `pkg/session/metrics/Cargo.toml` 定义；根 workspace 以 `facade_session_metrics` 依赖它，`pkg/session/Cargo.toml` 和 `pkg/util/metricsutil/Cargo.toml` 也通过路径依赖接入。这里没有网络、存储、SQL AST 或事务对象依赖。

## 错误处理与边界

`InitMetricsVars` 不返回 `Result`，失败通过 panic 表达。会话父级字段在执行 `InitSessionMetrics` 后仍缺失时，五处 `expect` 分别保护非事务、语句数、事务耗时、重试、编译与解析 collector；错误文本明确说明预期由父级初始化建立。遥测描述符创建失败也通过 `.expect("valid telemetry metric descriptors")` 转成 panic。标签数量或顺序不符合 vector 描述符时，Prometheus API 同样可能 panic，因此标签表是兼容性边界。

锁中毒不是永久错误：代码接受 poisoned mutex 的内部值继续运行。这能让后续调用尝试恢复，但不保证曾 panic 的那次调用没有留下部分 `Some` 状态；下一次完整成功调用才会覆盖整组句柄。

当前哨兵只检查 `NonTransactionalDMLCount`。如果外部代码制造“该字段为 `Some`、其他父级字段仍为 `None`”的部分初始化状态，本函数会跳过 `InitSessionMetrics` 并在后续 `expect` 处 panic。正常入口要求父级 collectors 作为整组初始化，不支持任意拆分或并发直接改写。

## 并发与资源生命周期

`INIT_LOCK` 让同一 crate 内并发调用 `InitMetricsVars` 时串行执行，防止多个线程同时读写本文件的 `static mut`。但是公开句柄本身仍是 `pub static mut`，类型系统无法强制所有外部读写都持有该锁；`pkg/session/metrics/lib.rs` 还显式允许 `static_mut_refs`。因此安全不变量依赖约定：应用先完成指标初始化，之后业务路径只克隆/使用底层线程安全的 Prometheus 句柄，不任意替换这些 `Option`。

锁的生命周期仅覆盖一次整批绑定，函数结束即释放；没有后台任务、通道、事务或显式清理流程。Prometheus 句柄及遥测 `OnceLock` 都具有进程级生命周期。重复初始化临时取得并丢弃句柄克隆，但底层 metric series 继续存在，测试确认样本值连续累加。

父级 `InitSessionMetrics` 自身未在本文件的 `INIT_LOCK` 外公开同步契约；本文件仅能保证经本入口触发时的串行性。新增入口若也能重建父级 `static mut` collectors，必须与现有初始化顺序协调，否则旧别名可能继续指向被替换前的 collector。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/metrics/metrics.go`。Rust 保留了 Go 的公开变量名、`init`/`InitMetricsVars` 名称以及所有标签组合：非事务 DML 三类、事务模式×结果/结束方式×来源组合、重试和解析/编译来源、CTE 字符串、账户操作字符串，以及各遥测单指标。

主要实现差异如下。

- Go 全局变量是非空接口/句柄并依赖包 `init()` 自动赋值；Rust 用 `Option` 表达初始化状态，普通 `init` 函数不会自动执行，生产接线显式调用 `InitMetricsVars`。
- Go 直接引用 `github.com/pingcap/tidb/pkg/metrics` 的全局 collectors；Rust crate 通过 `include!("../../metrics/session.rs")` 和本地 telemetry 模块拥有对应定义，并在缺失时主动调用 `InitSessionMetrics`。
- Go 没有本文件级显式锁；Rust 用 `INIT_LOCK` 支持并发和重复调用，并恢复 poisoned guard。
- Go 的分区遥测是多个具名全局 counter；Rust 的 `TelemetryMetrics` 将十五项压入数组，所以本文件必须维护位置映射。语义保持一致，但重排数组会造成静默错绑风险。
- Rust `init_telemetry_metrics` 返回 `Result`，本文件将错误提升为 panic；Go 赋值路径没有对应的显式错误返回。

迁移回归位于独立文件 `pkg/session/metrics/migration_aster_unit_test.rs`，符合测试不与生产源码混放的仓库约定。仓库中没有同目录 Go `*_test.go`；Go 语义证据来自实现本身，而 Rust 测试覆盖全部别名和幂等性。

## 扩展指南

新增会话指标别名时，应按以下接入点同步修改。

1. 先确认父级指标是否已在 `pkg/metrics/session.rs` 或 `pkg/metrics/telemetry.rs` 定义和初始化；本文件不应重复定义 collector。若是 Go 逐提交对齐，标签名、顺序、桶语义必须以对应 Go 增量为准。
2. 在本文件新增明确类型的 `pub static mut Option<Counter/Observer>`，并在 `InitMetricsVars` 的相应分组中绑定。会话父级若新增新的必需 `Option`，要检查单一 `NonTransactionalDMLCount` 哨兵是否仍能代表整组初始化完成。
3. 对 `CounterVec`/`HistogramVec` 严格遵循父级描述符的 label 次序；扩展 `table_partition` 时优先考虑消除裸数组下标，至少要同步 `TelemetryMetrics::new` 的数组构造顺序和本文件映射。
4. 在 `pkg/session/metrics/migration_aster_unit_test.rs` 增加别名共享底层 series 的断言，并保留重复 `InitMetricsVars` 后继续累计的覆盖。不要把测试写入 `metrics.rs`。
5. 若新句柄会被生产业务消费，还需在对应独立业务测试中验证实际 `inc`/`observe` 时机；仅验证绑定不能证明业务路径已接线。

兼容性风险主要是 label 名或次序变化造成时间序列不兼容，正确性风险是 `static mut` 的未同步访问和数组错位，性能风险则是高基数标签或在热路径反复初始化。扩展不应为动态业务值在这里预绑定无限序列。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点、1,848,419 条边。
- RustCodeGraph `files --filter pkg/session/metrics` 与 `node --file pkg/session/metrics/metrics.rs`：确认目标文件 317 行、模块内符号及文件使用关系；`query/node InitMetricsVars` 确认入口位于第 150 行，图中的直接调用者为同文件 `init`；`callees init` 确认 `init -> InitMetricsVars`。图工具没有解析出 `InitMetricsVars` 内宏/方法调用边，所以下游关系进一步由已索引源码节点和精确源码检索核验。
- RustCodeGraph `node InitSessionMetrics --file pkg/metrics/session.rs`：确认父级 collector 的名称、标签维度和桶定义；`node TelemetryMetrics`、`node init_telemetry_metrics --file pkg/metrics/telemetry.rs`：确认遥测字段、15 元分区数组和 `OnceLock` 式初始化。
- 读取的 crate/入口证据：`pkg/session/metrics/Cargo.toml`、`pkg/session/metrics/lib.rs`、根 `Cargo.toml`、`pkg/session/Cargo.toml`、`pkg/util/metricsutil/Cargo.toml`。
- 读取的实现与调用证据：`pkg/session/metrics/metrics.rs`、`pkg/metrics/session.rs`（通过 RustCodeGraph 节点）、`pkg/metrics/telemetry.rs`（通过 RustCodeGraph 节点）、`pkg/util/metricsutil/common.rs`。
- Go 对照与测试证据：`pkg/session/metrics/metrics.go`、`pkg/session/metrics/migration_aster_unit_test.rs`、`pkg/util/metricsutil/common_test.rs`。同目录未找到 Go `*_test.go`；`migration_aster_unit_test.rs` 对 51 个别名进行底层 series 联动检查，并验证重复初始化。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核只新增本文档、未修改 Rust/Go/Cargo/只读总计划。
