# `pkg/store/mockstore/unistore/metrics/metrics.rs`

## 文件定位

本文件是 Cargo 包 `astersql-store-mockstore-unistore-metrics` 的指标实现文件。包入口 `pkg/store/mockstore/unistore/metrics/lib.rs` 以 `#[path = "metrics.rs"]` 将它装入私有 `implementation` 模块，再用 `pub use implementation::*` 对外导出全部公开项。`Cargo.toml` 表明该包只有 `prometheus = "0.14"` 一个直接依赖，并通过 `package.metadata.porting.go-package` 对应 Go 包 `pkg/store/mockstore/unistore/metrics`。

它位于 mockstore 的 UniStore 存储路径，而不是 TiKV 客户端或全局 `pkg/metrics` 的通用指标定义中。其职责边界是定义并注册 UniStore Raft 写路径与锁等待相关的 Histogram；它不采集时间、不驱动 Raft，也不提供 HTTP 暴露端点。

## 核心职责

文件完成三件事：

1. 用固定的 `namespace = "unistore"` 和 `raft = "raft"` 统一构造 Prometheus 指标全名 `unistore_raft_<name>`。
2. 以十个 `LazyLock<prometheus::Histogram>` 保存 Raft writer 等待、四个写阶段、RaftDB/KVDB/锁更新、Latch 等待和批大小的直方图；首次解引用时才构造 collector。
3. 由 `RegisterMetrics()` 将十个 collector 的克隆注册到 Prometheus 默认 registry，保持 Go `prometheus.MustRegister` 的失败即 panic 语义。

当前代码只完成“定义和注册”。全仓 Rust 引用检索没有发现这些十个静态 Histogram 的生产 `observe` 调用；唯一的 Rust 外部调用是 `pkg/util/metricsutil/common.rs::initMetrics` 对 `RegisterMetrics` 的条件注册，以及独立测试对注册结果的验证。因此不能据此声称 Rust UniStore 写路径已经产生这些样本。

## 主要符号

- `namespace: &str`：固定为 `unistore`，参与所有指标名的命名空间部分。与 Go 的 `string(config.StoreTypeUniStore)` 当前结果一致，但 Rust 这里是字面量，不会随配置枚举定义自动变化。
- `raft: &str`：固定为 `raft`，作为全部 Histogram 的 subsystem。
- `new_histogram(name, start, factor, count) -> prometheus::Histogram`：内部工厂。调用 `prometheus::exponential_buckets` 生成指数桶，构造带命名空间、子系统和桶的 `HistogramOpts`，再创建 Histogram。Rust Prometheus 客户端拒绝空 help，所以统一使用 `"Unistore raft metric."`。
- `RaftWriterWait`、`WriteWaiteStepOne` 至 `WriteWaiteStepFour`：writer 总等待和四阶段等待；均采用起点 `0.001`、倍率 `1.5`、数量 `20` 的桶。
- `RaftDBUpdate`、`KVDBUpdate`：两类数据库更新耗时；桶参数同上。
- `LockUpdate`、`LatchWait`：锁存储更新和冲突键 Latch 等待耗时；桶为 `0.0001 × 2^i`，共 `15` 个。
- `RaftBatchSize`：单批 Raft 条目数；桶为 `1 × 1.5^i`，共 `20` 个。它是计数分布而非耗时分布。
- `must_register(&LazyLock<Histogram>)`：强制初始化传入的惰性静态量，克隆 Histogram handle，注册到默认 registry；注册错误通过 `expect` 转成 panic。
- `RegisterMetrics()`：唯一公开函数，按源码固定顺序注册全部十个 Histogram。函数本身不做幂等保护。

## 执行流程

应用级入口位于 `pkg/util/metricsutil/common.rs`：`RegisterMetrics()` 或 `RegisterMetricsForBR()` 最终进入内部 `registerMetrics()`，后者合并常量标签后调用 `initMetrics()`。当全局配置的 store 字符串等于 `config::StoreTypeUniStore.String()` 时，`initMetrics()` 通过 `REGISTER_UNISTORE_METRICS.call_once(unimetrics::RegisterMetrics)` 调用本文件入口。

本文件内的注册过程如下：

1. `RegisterMetrics()` 依次把十个 `LazyLock` 传给 `must_register()`。
2. 每个静态量首次解引用时运行其闭包并调用 `new_histogram()`。
3. `new_histogram()` 先校验并生成指数桶，再拼装 `HistogramOpts`，最后创建 Histogram。
4. `must_register()` 克隆共享同一底层状态的 Histogram handle，并交给 Prometheus 默认 registry。
5. 后续业务代码若持有原静态 handle 并调用 `observe`，注册表中的 collector 会呈现同一组样本；但当前 Rust 生产代码尚未出现这样的调用点。

测试流程独立于应用入口：`migration_aster_unit_test.rs::registers_all_unistore_raft_histograms_with_go_buckets` 直接调用 `RegisterMetrics()`，随后 `prometheus::gather()`，筛选 `unistore_raft_` 前缀并核对十个指标的桶数量、首桶和末桶上界。

## 数据与状态

十个公开静态量都是进程级 `LazyLock<Histogram>`。惰性初始化保证每个进程内每个静态定义只构造一次，但这与 registry 注册幂等性不同：重复直接调用 `RegisterMetrics()` 仍会尝试注册同名 collector，并因 Prometheus 重复描述符而 panic。应用级 `REGISTER_UNISTORE_METRICS: Once` 位于 `pkg/util/metricsutil/common.rs`，幂等保护属于调用方而非本文件。

Histogram 保存累计桶计数、样本数与样本和；本文件没有重置接口，也没有标签维度。十项指标共享 namespace/subsystem，但名字和桶配置各自独立。桶上界由 `start * factor^i` 形成；参数全部为正且 count 非零，符合 `prometheus::exponential_buckets` 的有效输入约束。

`Histogram::clone()` 是 collector handle 的克隆，不是样本状态的深拷贝；因此注册的克隆与公开静态量观测的是同一 collector 状态。默认 registry 是进程全局资源，注册结果也具有进程级生命周期。

## 依赖与调用关系

上游关系：

- `pkg/store/mockstore/unistore/metrics/lib.rs` 装载并再导出本文件。
- `pkg/util/metricsutil/common.rs` 以 `astersql_store_mockstore_unistore_metrics as unimetrics` 引入该 crate，在 `initMetrics()` 的 UniStore 配置分支中调用 `unimetrics::RegisterMetrics`；`Once` 保证该应用路径最多执行一次。
- `pkg/store/mockstore/unistore/metrics/migration_aster_unit_test.rs` 直接调用 `RegisterMetrics()`，验证注册集合和桶边界。
- 根 `Cargo.toml` 将本 crate 列为 workspace member，并提供 facade 依赖；`pkg/util/metricsutil/Cargo.toml` 以路径依赖接入它。`pkg/store/mockstore/unistore/tikv/Cargo.toml` 仅在 Windows target 依赖列表中声明该 crate，但当前该 TikV Rust 源码没有引用指标静态量。

下游关系仅为 `prometheus` crate：`exponential_buckets` 生成桶，`HistogramOpts` 组装描述符，`Histogram::with_opts` 创建 collector，`prometheus::register` 写入默认 registry。RustCodeGraph 对精确的 `RegisterMetrics` 和 `new_histogram` 查询没有返回静态 caller/callee 边，因此以上调用关系由索引源码和仓库引用检索共同核实。

## 错误处理与边界

本文件采用启动期快速失败策略，不返回 `Result`：

- 桶参数非法时，`new_histogram()` 在 `exponential_buckets(...).expect(...)` 处 panic。
- 指标选项非法时，`Histogram::with_opts(...).expect(...)` panic。
- 默认 registry 拒绝注册时（最常见是同名重复注册），`must_register()` panic。

现有固定参数在代码层面是有效的，但新增或修改指标时仍可能引入非法桶、重复全名或与其他 collector 描述不兼容的问题。`RegisterMetrics()` 不回滚：如果注册到中途才失败，前面已经成功注册的 collector 会留在默认 registry，随后重试可能更早地因重复注册失败。

本文件不处理观测值的单位和合法性；调用点必须保证耗时以秒传入、批大小以条目数传入，并决定是否允许负值或非有限浮点数。当前 Rust 生产路径没有观测调用，因而这些调用边界尚无 Rust 行为证据。

## 并发与资源生命周期

`LazyLock` 提供线程安全的一次构造；Prometheus `Histogram` handle 可克隆并用于并发观测。默认 registry 和十个静态 collector 都持续到进程结束，本文件没有显式注销或释放路径。

调用方 `pkg/util/metricsutil/common.rs` 使用 `std::sync::Once` 串行化并去重正常应用注册，避免多个初始化路径并发重复注册。不过绕过该入口直接并发调用本文件的 `RegisterMetrics()` 不受保护，可能一方成功、另一方 panic。

指标本身不创建线程、异步任务、锁、通道或事务。名称中的 `LatchWait` 描述被观测的业务同步等待，不表示本文件持有 UniStore latch。Go 侧真实观测点位于 `pkg/store/mockstore/unistore/tikv/region.go::AcquireLatches`：它在获取 latch 前后计时并调用 `metrics.LatchWait.Observe(dur.Seconds())`；对应 Rust `region.rs` 当前只实现 latch 同步，没有指标观测。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/metrics/metrics.go`。Rust 保留了十个导出变量名、指标 name、统一 namespace/subsystem、注册顺序和指数桶参数，独立迁移测试也逐项验证生成后的 Prometheus 全名及首末桶边界。

主要差异如下：

- Go 的 namespace 来自 `string(config.StoreTypeUniStore)`；Rust 写死 `"unistore"`。当前值对齐，但未来若配置常量改名，Rust 不会自动同步。
- Go `HistogramOpts` 未填写 Help；Rust 客户端不接受空 help，因此 Rust 统一增加固定说明。这是客户端约束适配，不改变指标名和桶。
- Go 全局变量在包初始化时立即创建 Histogram；Rust 使用 `LazyLock`，直到首次注册或访问才创建。
- Go `prometheus.MustRegister` 与 Rust 的 `register(...).expect(...)` 都在注册失败时 panic。
- Go `metricsutil.initMetrics()` 在 UniStore 配置下直接调用注册函数；Rust 同一条件外加 `Once`，使正常应用入口可重复进入而不会重复注册。
- 全仓 Go 生产引用同样只找到 `LatchWait` 的一个观测点；其他九项当前只有定义和注册。Rust 连 `LatchWait` 观测也尚未迁移，因此指标暴露后可能存在但无样本。

## 扩展指南

新增指标时，应在本文件增加独立的 `LazyLock<Histogram>`，复用 `new_histogram()` 或在指标类型/标签需求不同的情况下新增语义明确的工厂，并把它加入 `RegisterMetrics()`。同时同步 Go 对照（若任务要求保持双实现）、`migration_aster_unit_test.rs` 的期望集合与桶断言；测试必须继续放在独立测试文件，不能内嵌到 `metrics.rs`。

新增观测点时，应修改真正拥有时间区间或批大小语义的 UniStore Rust 模块，而不是让本文件猜测业务阶段。特别是迁移 `LatchWait` 时可对照 `region.go::AcquireLatches`，在 Rust `region.rs` 的对应获取流程中覆盖完整等待区间，并确认依赖在非 Windows target 下可用；当前 TikV crate 的指标依赖只出现在 Windows 条件段，不能假定所有 target 已接线。

修改既有桶、namespace、subsystem 或 name 会改变监控时间序列，可能造成看板/告警不兼容和旧新序列断裂。增加标签会提升基数与内存成本；增加桶会线性增加每条序列的计数器数量。注册顺序虽不影响 Prometheus 语义，但应保持与 Go 和测试清单一致，便于审计。

若要改变重复注册行为，应优先审视应用级 `REGISTER_UNISTORE_METRICS` 与默认 registry 生命周期，不能只吞掉本文件的注册错误；否则可能掩盖同名不同描述符的冲突。任何新行为都应添加独立回归测试，至少验证完整指标名、桶/标签、重复初始化策略以及真实观测后 gather 的样本。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；目标目录列出 `lib.rs`、`metrics.go`、`metrics.rs` 和 `migration_aster_unit_test.rs`。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/metrics/metrics.rs`：核对目标文件 97 行、常量、工厂、十个静态 Histogram、注册辅助函数及公开入口；索引报告该文件被迁移测试使用。
- RustCodeGraph 对精确 `RegisterMetrics`、`new_histogram` 的 `query/callers/callees`：确认符号候选，但没有可用静态调用边；因此未把缺失边解释为“不存在调用”。
- `pkg/store/mockstore/unistore/metrics/Cargo.toml` 与 `lib.rs`：核对 crate 名、唯一直接依赖、Go 包映射、实现模块装载和公开再导出。
- `pkg/util/metricsutil/common.rs` 及其 Go 对照 `common.go`：核对应用注册链、UniStore 配置分支和 Rust `Once` 差异。
- `pkg/store/mockstore/unistore/metrics/metrics.go`：逐项核对名称、桶参数和 MustRegister 顺序。
- `pkg/store/mockstore/unistore/metrics/migration_aster_unit_test.rs`：核对独立测试覆盖的十个全名、桶数及首末边界。
- `pkg/store/mockstore/unistore/tikv/region.go` 与 `region.rs`：核对 Go `LatchWait` 的真实观测区间及 Rust 对应流程尚未观测的事实。
- 全仓 `rg` 引用检查：Rust 生产代码只找到应用注册入口，没有十个静态量的观测调用；Go 生产代码只找到 `region.go` 对 `LatchWait` 的观测。
- 本任务按计划为纯文档分析，未运行 Cargo。最终结构检查要求本文恰好包含规定的十一个二级标题。
