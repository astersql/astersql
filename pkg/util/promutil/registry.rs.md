# `pkg/util/promutil/registry.rs`

## 文件定位

该文件属于独立 crate `astersql-util-promutil`。crate 入口 `pkg/util/promutil/lib.rs` 声明并重新导出 `registry` 模块，因此下游既可以从模块路径，也可以从 crate 根使用这里的公开 `Registry`、`NewNoopRegistry` 和 `NewDefaultRegistry`。`pkg/util/promutil/Cargo.toml` 表明本 crate 只有 `prometheus = "0.14"` 这一项直接依赖，并通过 `package.metadata.porting.go-package` 对应 Go 包 `pkg/util/promutil`。

它位于“指标的创建”和“指标的使用”之间：相邻的 `factory.rs` 负责构造 Counter、Gauge、Histogram 等 Collector，本文件负责抽象这些 Collector 如何登记到或移出注册表。例如 `pkg/lightning/metric/metric.rs` 的 `Common::register_to`、`Metrics::register_to` 以及对应的 `unregister_from` 都只依赖 `promutil::Registry`，不绑定具体注册表实现。

## 核心职责

- `Registry` trait 统一三种操作：可失败的单项注册、失败即 panic 的批量强制注册、返回成功与否的单项注销（`registry.rs:26-33`）。
- `noopRegistry` 提供有意不保存任何指标的实现。它适用于指标已由 factory 或其他路径自动注册、但调用方仍需要满足统一接口的场景（`registry.rs:35-48,72-78`）。
- `defaultRegistry` 包装一个新建的 `prometheus::Registry`，把操作落到真实 Prometheus 注册表（`registry.rs:50-70,80-84`）。
- 两个构造器都返回 `Box<dyn Registry>`，把具体结构隐藏在动态分派边界之后；调用方可在运行时选择“忽略登记”或“独立真实登记”，而无需改变指标容器的注册流程。

本文件不负责创建指标、采集或编码 exposition 文本，也没有暴露 `prometheus::Registry::gather`。它仅定义注册生命周期边界。

## 主要符号

- `pub trait Registry: Send + Sync`：公开的对象安全接口。`Send + Sync` 要求实现可在线程间转移并被并发共享；三个方法都只借用 `&self`，Collector 则以 `Box<dyn prometheus::core::Collector>` 转移所有权。
- `Registry::Register`：登记一个 Collector，原样返回 `prometheus::Result<()>`，允许调用方检查重复描述符等底层错误。
- `Registry::MustRegister`：接收 `Vec<Box<dyn Collector>>`。接口本身不返回结果，具体实现用“忽略”或“失败即 panic”表达强制语义。
- `Registry::Unregister`：消费一个用于匹配的 Collector，并把注销结果压缩为 `bool`。
- `struct noopRegistry`：私有、无字段、无状态的实现。`Register` 恒为 `Ok(())`，`MustRegister` 直接返回，`Unregister` 恒为 `true`。
- `struct defaultRegistry(prometheus::Registry)`：私有元组结构，唯一状态是底层注册表。
- `pub fn NewNoopRegistry() -> Box<dyn Registry>`：构造新的无状态 noop trait object。
- `pub fn NewDefaultRegistry() -> Box<dyn Registry>`：用 `prometheus::Registry::new()` 构造独立注册表并封装为 trait object；它不是 Prometheus 全局默认注册表的别名。

## 执行流程

1. 调用方先通过两个公开构造器之一取得 `Box<dyn Registry>`，或自行实现该 trait。Lightning 的指标容器接收 `&dyn promutil::Registry`，因此构造选择与指标集合解耦。
2. 调用 `Register` 时，noop 分支丢弃传入 Box 并返回成功；default 分支调用 `self.0.register(collector)`，保留底层错误。
3. 调用 `MustRegister` 时，noop 分支丢弃整个 Vec；default 分支按 Vec 顺序迭代，每个 Collector 调用 `register(...).expect("metric registration failed")`。全部成功才正常返回；某一项失败时立即 panic，后续项不再处理。
4. 调用 `Unregister` 时，noop 分支丢弃参数并返回 `true`；default 分支调用 `self.0.unregister(collector)`，用 `is_ok()` 将底层 `Result` 转成布尔值。
5. 典型上游 `pkg/lightning/metric/metric.rs` 先由 `Common::register_to` 登记公共指标，再由 `Metrics::register_to` 登记 Lightning 自身指标；注销则调用对应的 `unregister_from` 逐个请求移除。接口不负责在容器级别保证这些多步操作的原子性。

## 数据与状态

`noopRegistry` 没有字段，也不记录 Collector、错误或调用次数。因此重复注册、注销从未登记的 Collector，以及混合不同 Collector 类型，结果都不依赖历史状态。

`defaultRegistry` 的全部可变状态封装在 `prometheus::Registry` 内部。本文件不另建锁、缓存、名称索引或全局单例。每次 `NewDefaultRegistry` 都调用 `prometheus::Registry::new()`，所以不同构造结果拥有彼此独立的登记集合。

Collector 以 Box 进入接口，说明调用会消费这个 Box；需要在注册后继续持有指标句柄的调用方，应像 `pkg/lightning/metric/metric.rs` 一样注册可克隆指标的 clone，保留原句柄用于观测或再次构造注销参数。`MustRegister` 还会消费 Vec 及其全部元素。

## 依赖与调用关系

下游依赖只有 `prometheus` crate：`prometheus::core::Collector` 定义被登记对象，`prometheus::Result` 表达注册错误，`prometheus::Registry::{new,register,unregister}` 提供真实实现。此边界由 `pkg/util/promutil/Cargo.toml` 明确声明。

模块装配由 `pkg/util/promutil/lib.rs` 完成：`pub mod registry` 加载本文件，`pub use registry::*` 向 crate 根再导出公开符号；同一入口用独立的 `registry_test.rs` 作为测试模块，没有把测试嵌入生产源文件。

直接生产消费证据位于 `pkg/lightning/metric/metric.rs`：

- `Common::register_to` 和 `Metrics::register_to` 调用 `Registry::MustRegister`；
- `Common::unregister_from` 和 `Metrics::unregister_from` 调用 `Registry::Unregister`；
- 参数类型是 `&dyn promutil::Registry`，因此测试实现、noop 实现和 default 实现可以替换。

仓库内对构造器的直接 Rust 引用目前见于 `pkg/util/promutil/registry_test.rs` 和 `pkg/util/promutil/migration_aster_unit_test.rs`；生产消费者主要依赖 trait，而不是在该文件内固定选择构造器。

## 错误处理与边界

- noop 的成功结果是“操作被有意忽略”，不代表 Collector 已进入任何可采集集合。这是接口最重要的语义边界。
- default `Register` 不吞掉 `prometheus::Error`；重复注册等行为由底层 crate 判定。迁移测试 `default_registry_uses_prometheus_duplicate_and_unregister_semantics` 验证相同 Counter 第二次注册返回错误。
- default `MustRegister` 的错误处理是 panic，消息固定为 `metric registration failed`。批量循环没有事务或回滚：若第 N 项失败，前 N-1 项已经登记，剩余项未尝试。因此调用方若需要可恢复或全有全无的流程，应逐项调用 `Register` 并自行补偿，而不应使用该方法。
- default `Unregister` 只暴露 `bool`，底层错误细节会被 `is_ok()` 丢弃。迁移测试验证首次注销已登记 Counter 为 `true`，再次注销为 `false`。
- 本 trait 不提供 gather、按名称查询、清空或批量注销；需要这些能力时不能假定可从 `dyn Registry` 取得底层 `prometheus::Registry`。

## 并发与资源生命周期

`Registry: Send + Sync` 是本文件明确给出的并发契约，允许 trait object 在线程间传递和共享引用。方法使用 `&self`，本文件自身没有外部可见的可变借用，也没有显式 Mutex、线程、任务、通道或异步生命周期。default 实现的实际同步由其持有的 `prometheus::Registry` 提供；这里不额外串行化多方法序列。

两个构造器返回独占的 Box。若多个所有者需要共享同一注册表，调用方必须在 Box 之外增加 `Arc` 等所有权容器，或者提供自己的 `Registry` 实现；本文件没有克隆 trait object 的接口。

Collector 的 Box 在每次调用结束时要么交给底层注册表，要么被 noop/失败路径消费和释放。`MustRegister` panic 时，已登记项留在底层注册表中，尚未处理的 Vec 元素随栈展开释放；因此批量登记的资源生命周期不是原子的。代码没有自定义 `Drop`，销毁 `defaultRegistry` 时由 Rust 按字段顺序销毁底层注册表。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/promutil/registry.go`。两版都提供 `Registry`、无状态 `noopRegistry`、`NewNoopRegistry` 和 `NewDefaultRegistry`；Go 测试 `registry_test.go::TestNoopRegistry` 与 Rust 测试 `registry_test.rs::test_noop_registry` 都验证重复注册不报错、注销 Counter/GaugeVec 恒为真。

需要注意以下语言/API 映射：

- Go 的 `Registry` 是 `prometheus.Registerer` 的类型别名；Rust 因 crate API 与对象模型不同，自定义了同名 trait，并显式要求 `Send + Sync`。
- Go `MustRegister` 使用可变参数 `...prometheus.Collector`；Rust 使用拥有所有权的 `Vec<Box<dyn Collector>>`。
- Go 默认构造器直接返回 `prometheus.NewRegistry()`；Rust 返回包装 `prometheus::Registry::new()` 的私有 `defaultRegistry`，以适配本地 trait。
- Go 库自身提供 `MustRegister` 语义；Rust 包装层逐项调用 `register(...).expect(...)`。外部可见的失败即 panic 语义保持一致，但 Rust 实现明确表现为顺序登记、无回滚。
- Go 接口的 Register/Unregister 参数由接口值传递；Rust 显式转移 boxed trait object 的所有权。扩展调用方时不能机械照搬 Go 的复用方式。

当前 Rust 文件是可工作的直接移植，不是桩或仅装配门面；顶部 `// Copyright 2026 AsterSQL.` 表示该生产 Rust 实现已经过处理。

## 扩展指南

新增注册策略时，优先新增独立实现 `Registry` 的结构，而不是在 Lightning 等调用方分支判断具体类型。实现必须同时维持 `Send + Sync`，并明确回答 Register 错误是否保留、MustRegister 是否 panic、Unregister 的 true/false 含义。若要增加方法，需要检查所有实现者，包括 `noopRegistry`、`defaultRegistry` 以及 `pkg/lightning/metric/metric_test.rs` 中的测试实现，避免破坏 trait object 使用点。

若修改 noop 契约，应同步更新独立测试 `pkg/util/promutil/registry_test.rs` 和迁移测试 `pkg/util/promutil/migration_aster_unit_test.rs`，并与 `pkg/util/promutil/registry_test.go` 的原始意图核对。若修改 default 的重复登记、注销或 panic 行为，应扩展迁移测试；特别建议覆盖 `MustRegister` 部分成功后失败的状态，以明确是否仍接受当前“无回滚”行为。

若调用方需要 gather、批量原子性、错误可观测性或共享所有权，应先评估是否扩展 trait 会影响所有消费方。性能上，当前接口为每个 Collector 分配 Box 并使用动态分派；高频新增路径应避免把注册/注销放在请求热循环中。兼容性上应保留 Go 的 Register/MustRegister/Unregister 基本契约和现有 PascalCase 方法名，除非同步迁移全部调用者与测试。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/promutil` 确认目标 Rust/Go/测试文件均已索引。
- RustCodeGraph `node --file pkg/util/promutil/registry.rs`：核对 84 行生产源码及 15 个文件内符号，确认 trait、两种实现和两个构造器的真实签名与方法体。
- RustCodeGraph `query Registry`、`query NewNoopRegistry --json`、`query NewDefaultRegistry --json`、`query MustRegister --json`：消除同名符号歧义，并确认目标文件中的定义位置。
- RustCodeGraph 对 `callers`/`callees` 的目标查询未返回可用方法级边；因此按技能规则使用精确 `rg` 补查 trait 方法和构造器引用。结果定位到 `pkg/lightning/metric/metric.rs` 的四个生产注册/注销方法，以及两个本 crate Rust 测试文件。
- 已读取：`pkg/util/promutil/registry.rs`、`pkg/util/promutil/lib.rs`、`pkg/util/promutil/Cargo.toml`、`pkg/util/promutil/registry.go`、`pkg/util/promutil/registry_test.rs`、`pkg/util/promutil/registry_test.go`、`pkg/util/promutil/migration_aster_unit_test.rs`，以及直接生产消费者 `pkg/lightning/metric/metric.rs` 的注册/注销实现。
- 测试证据：`registry_test.rs::test_noop_registry` 与 Go `registry_test.go::TestNoopRegistry` 覆盖 noop；`migration_aster_unit_test.rs::{noop_registry_accepts_duplicates_and_always_unregisters,default_registry_uses_prometheus_duplicate_and_unregister_semantics}` 覆盖两种实现的关键边界。本任务为纯文档分析，按任务约束未运行 Cargo，也未执行这些测试。
