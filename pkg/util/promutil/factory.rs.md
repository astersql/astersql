# [`pkg/util/promutil/factory.rs`](factory.rs)

## 文件定位

本文件属于 `astersql-util-promutil` crate，是 AsterSQL 对 Prometheus 原生指标构造过程的最小抽象层。crate 入口 `pkg/util/promutil/lib.rs` 通过 `pub mod factory` 和 `pub use factory::*` 公开这里的 API；`pkg/util/promutil/Cargo.toml` 声明其唯一直接依赖为 `prometheus = "0.14"`，并把对应 Go 包记录为 `pkg/util/promutil`。

它不负责注册、注销或导出指标：这些职责由相邻的 `registry.rs` 及上层指标模块承担。本文件只把 Counter、Gauge、Histogram 及三种 Vec 形态的创建方式收束到可替换的 `Factory` trait 中，让上层代码可以通过 `&dyn Factory` 或 `Box<dyn Factory>` 构造指标。

## 核心职责

- `Factory` 定义六种指标构造操作，隔离调用方与 `prometheus` crate 的具体构造函数。
- 私有零字段类型 `defaultFactory` 提供默认实现，将六种操作直接委托给 `prometheus` 的 `with_opts` 或 `new` 构造器。
- `NewDefaultFactory` 以 `Box<dyn Factory>` 返回默认实现，给 `pkg/lightning/metric/metric.rs`、`pkg/metrics/ddl.rs`、`pkg/metrics/import.rs` 和 `pkg/dxf/importinto/metrics.rs` 等上层路径提供统一入口。
- 构造失败时使用 `expect` 立即 panic，保持“配置错误在指标初始化阶段暴露”的行为；本层不恢复错误，也不做注册副作用。

## 主要符号

- `pub trait Factory`（`factory.rs:26`）：公开对象安全 trait。所有方法都接收 `&self`，不修改工厂状态。
  - `NewCounter(prometheus::Opts) -> prometheus::Counter`：创建只增计数器。
  - `NewCounterVec(prometheus::Opts, Vec<String>) -> prometheus::CounterVec`：创建按标签名分区的计数器向量。
  - `NewGauge(prometheus::Opts) -> prometheus::Gauge`：创建可增减或直接赋值的仪表。
  - `NewGaugeVec(prometheus::Opts, Vec<String>) -> prometheus::GaugeVec`：创建仪表向量。
  - `NewHistogram(prometheus::HistogramOpts) -> prometheus::Histogram`：创建直方图。
  - `NewHistogramVec(prometheus::HistogramOpts, Vec<String>) -> prometheus::HistogramVec`：创建直方图向量。
- `struct defaultFactory`（`factory.rs:65`）：私有、零字段的默认实现类型；调用方不能依赖其具体类型，只能经 `Factory` 使用。
- `impl Factory for defaultFactory`（`factory.rs:67`）：六个薄适配方法。三个 Vec 方法先把拥有所有权的 `Vec<String>` 临时映射为 Prometheus API 所需的 `Vec<&str>`。
- `pub fn NewDefaultFactory() -> Box<dyn Factory>`（`factory.rs:107`）：公开构造入口，将 `defaultFactory` 封装为 trait object。返回值拥有工厂对象；是否发生实际堆分配属于 `Box` 对零大小类型的实现细节，不应作为业务语义依赖。
- `#![allow(non_snake_case)]`、`#![allow(non_camel_case_types)]`（`factory.rs:21-22`）：允许保留 Go 移植中的导出名和类型命名，降低跨语言对照成本。

## 执行流程

1. 上层先调用 `NewDefaultFactory`，得到 `Box<dyn Factory>`；例如 `pkg/metrics/ddl.rs::RegisterLightningCommonMetricsForDDL` 创建工厂后把 `factory.as_ref()` 交给 `pkg/lightning/metric/metric.rs::new_common`。
2. 上层组装 `prometheus::Opts` 或 `prometheus::HistogramOpts`，并按需要传入标签名。`new_common` 和 `new_metrics` 会为 Lightning 的 chunk、rows、progress、耗时等指标选择对应工厂方法。
3. 标量方法直接调用 `Counter::with_opts`、`Gauge::with_opts` 或 `Histogram::with_opts`。
4. Vec 方法先从 `labelNames: Vec<String>` 借出 `&str` 列表，再调用 `CounterVec::new`、`GaugeVec::new` 或 `HistogramVec::new`。Prometheus 构造器在返回前消费标签描述信息，因此临时借用不会逃逸出方法。
5. 构造成功时返回独立指标句柄；调用方随后选择更新样本、封装进指标集合或经 `Registry` 注册。本文件自身不自动注册指标。
6. 构造器返回错误时，紧随其后的 `expect` 触发 panic，并附带指标类别相关的固定上下文消息。

## 数据与状态

`defaultFactory` 没有字段，不保存命名空间、标签、注册器或已创建指标，也不存在跨调用缓存。每次方法调用的全部输入都来自参数，输出是 `prometheus` crate 自己管理的指标句柄。

标量 Counter 与 Gauge 使用同一个 Rust 类型 `prometheus::Opts`；Histogram 使用包含 buckets 等直方图配置的 `prometheus::HistogramOpts`。Vec 方法取得标签名 `Vec<String>` 的所有权，仅在调用期间创建借用视图；函数结束后原字符串和临时引用列表均被释放，返回的 Vec 指标保留的是 Prometheus 构造所得描述信息，而不是对这些局部字符串的 Rust 借用。

`NewDefaultFactory` 返回动态分派对象，便于调用方替换实现。当前仓库中的主要生产调用通常立即通过 `as_ref()` 传为 `&dyn Factory`；`pkg/metrics/import.rs::GetRegisteredImportMetrics` 则直接接收并拥有 `Box<dyn promutil::Factory>`。

## 依赖与调用关系

下游依赖只有 `prometheus` crate：

- `NewCounter` → `prometheus::Counter::with_opts`
- `NewCounterVec` → `prometheus::CounterVec::new`
- `NewGauge` → `prometheus::Gauge::with_opts`
- `NewGaugeVec` → `prometheus::GaugeVec::new`
- `NewHistogram` → `prometheus::Histogram::with_opts`
- `NewHistogramVec` → `prometheus::HistogramVec::new`
- `NewDefaultFactory` → `Box::new(defaultFactory)`

上游以指标组装代码为主。`pkg/lightning/metric/metric.rs::new_common` 和 `new_metrics` 接收 `&dyn promutil::Factory`，集中创建 Lightning 指标；`pkg/metrics/ddl.rs::RegisterLightningCommonMetricsForDDL` 用默认工厂创建并注册每个 DDL job 的通用指标；`pkg/dxf/importinto/metrics.rs` 把默认工厂交给导入指标入口。Cargo 直接依赖可在 `pkg/lightning/metric/Cargo.toml`、`pkg/metrics/Cargo.toml`、`pkg/dxf/importinto/Cargo.toml` 和 `pkg/executor/importer/Cargo.toml` 中核对。

RustCodeGraph 的文件节点把 `factory.rs` 标为被 `pkg/lightning/metric/metric_test.rs`、`pkg/lightning/metric/migration_aster_unit_test.rs` 和 `pkg/util/security_2_aster_unit_test.rs` 使用；由于图查询对同名跨语言符号存在歧义且精确 `callers`/`callees` 查询超时，生产调用边同时用 `rg` 对上述具体符号和 Cargo 依赖进行了补证。

## 错误处理与边界

六个 trait 方法都返回具体指标而非 `Result`。默认实现把 Prometheus 构造器的错误通过 `expect` 转为 panic：标量分别使用 `invalid CounterOpts`、`invalid GaugeOpts`、`invalid HistogramOpts`，Vec 版本的消息还指出 label names 也可能无效。调用者因此必须把名称、help、常量标签、变量标签和 histogram buckets 当作可信的初始化配置，而不能用该 API 处理可恢复的运行时输入错误。

`pkg/util/promutil/migration_aster_unit_test.rs::histogram_factory_panics_for_non_increasing_buckets_like_go` 用非严格递增的 `[1.0, 0.5]` 验证 `NewHistogram` panic。相同的错误传播方式也适用于 Prometheus 拒绝的其他选项；但现有独立测试没有逐项枚举空名称、非法标签名或 Vec 直方图非法桶，因此文档不宣称这些边界已经分别回归。

本文件只创建 collector，不检测重复注册。重复 descriptor 的冲突发生在相邻 Registry 层；也不校验调用指标 Vec 时提供的标签值数量，该行为由返回的 Prometheus 类型负责。

## 并发与资源生命周期

工厂本身无可变状态、锁、通道、后台任务、事务或 I/O；创建过程是同步的，生命周期止于构造函数返回。方法只借用 `&self`，多个调用之间没有本文件维护的共享状态。

返回指标的克隆、并发更新和内部同步语义由 `prometheus 0.14` 类型实现，本文件不额外包锁，也不改变这些语义。工厂 trait 未显式声明 `Send + Sync`，因此不能仅凭本文件断言任意 `dyn Factory` 实现可在线程间共享；若新的调用场景要把工厂放入跨线程容器，应先决定是否给 trait 增加线程安全约束，并验证所有实现与调用点。

`Box<dyn Factory>` 拥有默认工厂并在离开作用域时释放；工厂不拥有它创建的指标。Vec 构造中的字符串和引用只活到方法结束，不形成返回对象对局部数据的生命周期依赖。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/promutil/factory.go`。两端都有 `Factory`、私有 `defaultFactory`、六个同名方法和 `NewDefaultFactory`，覆盖的指标形态一致。Go 的 `prometheus.CounterOpts` 与 `GaugeOpts` 在 Rust 侧统一映射为 `prometheus::Opts`，Histogram 仍使用专门的 `HistogramOpts`。

Go 默认构造器直接返回 `prometheus.New*` 的接口或指针；Rust `prometheus` API 的构造器返回 `Result`，本实现以 `expect` 还原 Go 构造函数在无效配置下 panic 的外部行为。Go 的标签参数是 `[]string`，Rust 为取得明确所有权使用 `Vec<String>`，再适配成 `&[&str]`。Go `NewDefaultFactory` 返回接口值，Rust 返回 `Box<dyn Factory>`；两者都对上层隐藏具体 `defaultFactory`。

Go 文件含 `var _ Factory = defaultFactory{}` 作为显式编译期接口断言；Rust 的 `impl Factory for defaultFactory` 本身即由编译器验证完整实现，无需额外断言。Rust 文件已经带有 `// Copyright 2026 AsterSQL.`，同时保留原 PingCAP Apache License 注释。

## 扩展指南

- 新增一种指标形态时，应同时扩展 `Factory` trait 与 `defaultFactory` 实现，并核对 Go `pkg/util/promutil/factory.go` 的对应接口；不要只给默认类型增加固有方法，否则 trait 注入调用方无法使用。
- 若要支持可恢复的配置错误，需要设计新的返回 `Result` 的 API 或兼容层。直接修改现有六个方法的签名会影响所有 `&dyn Factory`/`Box<dyn Factory>` 调用方，并偏离 Go 当前 panic 语义。
- 修改标签所有权或参数类型时，重点检查 `pkg/lightning/metric/metric.rs::new_common`、`new_metrics` 以及 `pkg/metrics/import.rs::GetRegisteredImportMetrics`，避免引入无必要克隆或悬垂借用设计。
- 若增加有状态或跨线程工厂，需明确状态共享、`Send`/`Sync` 约束和对象生命周期；当前 trait 没有这些边界保证。
- 测试逻辑应继续放在独立文件 `pkg/util/promutil/migration_aster_unit_test.rs`，不要嵌入 `factory.rs`。至少同步覆盖：新指标可读写、Vec 标签行为、无效配置错误路径，以及 Go/Rust 形态对应。上层接线变化还应更新 `pkg/lightning/metric/metric_test.rs` 或 `migration_aster_unit_test.rs` 中最接近的聚合指标测试。
- 性能上，本层每次 Vec 构造会临时分配一个 `Vec<&str>`；指标通常在初始化阶段创建。若要优化，须先测量初始化热点，并保持 Prometheus API 所需的标签顺序和校验行为。

## 验证依据

- 目标源码：`pkg/util/promutil/factory.rs`，核对 1 个 trait、6 个 trait 方法、私有 `defaultFactory`、对应 `impl`、公开 `NewDefaultFactory` 及条件允许属性；文件没有条件编译项。
- crate 边界：`pkg/util/promutil/Cargo.toml` 与 `pkg/util/promutil/lib.rs`，核对 crate 名、`prometheus 0.14` 依赖、Go 包映射以及公开再导出。
- Go 对照：`pkg/util/promutil/factory.go`，核对方法集合、参数形态、默认实现与 panic 语义。
- 直接测试：`pkg/util/promutil/migration_aster_unit_test.rs::default_factory_creates_all_go_metric_shapes` 覆盖六种指标的创建及样本读写；`histogram_factory_panics_for_non_increasing_buckets_like_go` 覆盖非法桶 panic。
- 上层行为证据：`pkg/lightning/metric/metric.rs::new_common`、`new_metrics` 展示 trait 注入和六种构造方法的实际使用；`pkg/lightning/metric/migration_aster_unit_test.rs::registration_and_unregistration_cover_all_go_metrics` 验证由该工厂创建的聚合指标可整批注册/注销。
- RustCodeGraph：`status` 显示索引包含目标 Rust/Go 文件；`files --filter pkg/util/promutil` 列出 `factory.rs`、`factory.go`、crate 入口和独立测试；`node --file pkg/util/promutil/factory.rs --offset 1 --limit 260` 返回完整 109 行与文件使用者；`query NewDefaultFactory --json --limit 10` 定位本文件符号。精确 `callers`/`callees` 命令在 30 秒窗口内未返回，故用上述源码与 `rg` 结果补齐调用边，不把缺失图结果推断为无调用者。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求目标文档存在且恰有 11 个规定的二级标题。
