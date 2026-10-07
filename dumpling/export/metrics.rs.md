# `dumpling/export/metrics.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-dumpling-export`（`dumpling/export/Cargo.toml`），不是独立 Rust 模块：crate 根 `dumpling/export/lib.rs:37-48` 先导入并公开 `stubs.rs`，再用 `include!("metrics.rs")` 将本文件拼入 crate 根作用域。因此这里直接使用的 `Factory`、`Registry`、`Labels`、`CounterVec` 和 `GaugeVec` 均来自 `dumpling/export/stubs.rs:885-1127`，`Arc`、`AtomicBool`、`AtomicI64` 与 `Ordering` 则由 `lib.rs:25-31` 引入同一作用域。

它位于 Dumpling 导出运行时的观测边界：`Dumper` 持有 `Arc<metrics>`（`dumpling/export/dump.rs:14-34`），构造导出会话时调用 `newMetrics`，执行时注册指标，关闭时注销；写入器和状态采样代码通过本文件的包装函数更新或读取这些指标。该文件当前接入的是 crate 内本地指标桩，不是完整 Prometheus client。

## 核心职责

1. 用 `metrics` 聚合六个导出指标和三个 chunk 进度原子状态，供一次 `Dumper` 会话共享。
2. 用 `newMetrics` 从抽象 `Factory` 创建指标并将所有进度原子量初始化为零/未就绪。
3. 用 `registerTo`/`unregisterFrom` 管理六个指标在抽象 `Registry` 中的生命周期，并为可 gather 的默认注册表提供 Prometheus 文本样本闭包。
4. 用 `Read*`、`Add*`、`SubGauge`、`Inc*`、`DecGauge` 隔离具体计数器实现，同时允许 `Option::None` 成为读返回 `NaN`、写无操作的测试/未接线语义。

本文件不负责决定何时产生导出任务、何时完成文件写入，也不负责计算状态快照；这些职责分别位于 `dump.rs`、`writer_util.rs` 和 `status.rs`。

## 主要符号

- `pub struct metrics`（`metrics.rs:12-26`）：虽为公开符号，但保留 Go 风格小写类型名。`#[derive(Clone)]` 会克隆各指标内部的 `Arc` 状态以及三个显式 `Arc<Atomic*>`，所以克隆值观察同一批运行时计数，而非获得快照。
- `finishedSizeGauge`、`finishedRowsGauge`：可增可减的累计字节数和行数。写入失败时 `writer_util.rs:191-195,338-342` 会回滚此前的增量。
- `finishedTablesCounter`、`estimateTotalRowsCounter`：只增计数器，分别表示完成表数和估算总行数。前者在表数据任务完成回调中递增（`dump.rs:191-201`），后者在自定义 SQL 路径写入估算值（`dump.rs:334-364`）。
- `errorCount`、`taskChannelCapacity`：分别注册为 `error_count` counter 和 `channel_capacity` gauge。对 Rust 非测试生产文件的直接搜索只发现构造/注册，没有更新调用；文档不能据此宣称 Rust 已实际上报错误数或队列容量。
- `totalChunks`、`completedChunks`、`progressReady`：不注册到 registry，而由 `dump.rs:175-201,367-375` 和 `status.rs:164-190` 直接以 `SeqCst` 访问，用于状态中的 chunk 进度。
- `pub fn newMetrics(f: &dyn Factory, _const_labels: &Labels) -> metrics`（`metrics.rs:28-41`）：创建六个互相独立的指标和三个原子状态。参数名前导下划线表示 Rust 当前不使用常量标签。
- `metrics::registerTo` / `metrics::unregisterFrom`（`metrics.rs:43-93`）：成对注册/注销六个固定名称。注册闭包读取指标共享的原子 bit 值，并输出 `dumpling_dump_<name>` 的 Prometheus 文本行。
- `ReadCounter`、`AddCounter`、`IncCounter`（`metrics.rs:95-112`）以及 `ReadGauge`、`AddGauge`、`SubGauge`、`IncGauge`、`DecGauge`（`metrics.rs:113-139`）：对 `CounterVec::With(None)` / `GaugeVec::With(None)` 的薄包装。
- `_ArcMetrics = Arc<metrics>`（`metrics.rs:141-143`）：仅用于保留迁移期类型/导入形状；生产代码直接书写 `Arc<metrics>`，该别名没有业务分支。

## 执行流程

一次典型导出中的指标流程如下：

1. `NewDumper` 复制配置中的 `PromFactory` 和 `Labels`，调用 `newMetrics(factory.as_ref(), &labels)`，再放入 `Arc`（`dump.rs:36-59`）。构造函数为六个指标创建零值存储，并令 `totalChunks=0`、`completedChunks=0`、`progressReady=false`。
2. `Dumper::Dump` 开始时调用 `registerTo`（`dump.rs:106-114`）。六次 `RegisterMetric` 保存固定名称及采样闭包；默认 registry gather 时才执行闭包并读取最新值（`stubs.rs:1091-1127`）。
3. 创建每个表数据任务时，`newTaskTableData` 对 `totalChunks` 加一（`dump.rs:367-375`）。任务生产结束后才将 `progressReady` 设为 true（`dump.rs:175-176`），避免状态层在总数尚未稳定时展示比例。
4. SQL/CSV 写入路径周期性调用 `AddGauge` 更新完成行数和字节数；失败路径用 `SubGauge` 回滚该次写入已经公布的增量（`writer_util.rs:135-197,300-344`）。Parquet 成功关闭后一次性公布最终字节和行数（`writer_util.rs:688-700`）。
5. 每个表数据任务完成时增加 `completedChunks`；表的完成回调增加 `finishedTablesCounter`（`dump.rs:191-205`）。自定义 SQL 导出还通过 `AddCounter` 写入估算行数（`dump.rs:334-364`）。
6. `RefreshStatus` 用 `ReadCounter`/`ReadGauge` 生成状态快照；仅当 `progressReady` 为 true 时读取 chunk 原子量并计算比例（`status.rs:151-190`）。周期日志也直接读取 `finishedSizeGauge`（`status.rs:69-143`）。
7. `Dumper::Close` 调用 `unregisterFrom`（`dump.rs:85-103`），删除六个名称及其样本闭包，避免同一 registry 中遗留旧会话状态。

## 数据与状态

六个 `*Vec` 在当前桩实现中都忽略 labels：`CounterVec::With` 和 `GaugeVec::With` 永远返回唯一 `inner`（`stubs.rs:1000-1033`）。`newMetrics` 传给 Factory 的字符串仅用于保持指标名称形状；`DefaultFactory` 本身创建独立的零值 inner，并不保存 opts、help 或 const labels（`stubs.rs:1042-1069`）。

`Counter` 和 `Gauge` 都把 `f64::to_bits()` 存在 `AtomicU64` 中。Counter 的 `Add` 使用 `compare_exchange` 循环，因此并发加法不会丢失；Gauge 的 `Add`/`Sub` 是一次 load 加一次 store，不是原子 read-modify-write，多写者并发更新可能覆盖彼此（`stubs.rs:892-967`）。本文件注册的采样闭包克隆内部 `Arc<AtomicU64>`，用 `SeqCst` load 获取实时值。

三个 chunk 字段使用显式 `Arc<AtomicI64/AtomicBool>` 且生产读写均为 `SeqCst`。不变量是：任务创建增加 `totalChunks`，任务完成增加 `completedChunks`，生产完任务后才发布 `progressReady`。状态层对 `totalChunks == 0` 视为 100%，对 `completedChunks > totalChunks` 钳制为 100% 并告警（`status.rs:164-190`）。

## 依赖与调用关系

- crate 边界：`dumpling/export/Cargo.toml` 声明 library 入口为 `lib.rs`，porting 元数据对应 Go 包 `dumpling/export`；Cargo 依赖中没有 Prometheus crate，印证本文件依赖的是本地 `stubs.rs` 指标抽象。
- 上游构造者：RustCodeGraph 将 `dump.rs::NewDumper`、`dump_test.rs::make_dumper`、`prepare_test.rs::test_validate_resolve_auto_consistency` 列为 Rust `newMetrics` 调用者；直接主链证据见 `dump.rs:36-59`。
- 生命周期上游：`Dumper::Dump → metrics::registerTo`，`Dumper::Close → metrics::unregisterFrom`（`dump.rs:85-114`）。RustCodeGraph 对 include 后的这两个方法只识别到自身边，故此处以直接源码调用为准。
- 读路径：RustCodeGraph 与源码均显示 `status.rs::RefreshStatus → ReadCounter/ReadGauge`；`runLogProgressWithTicks → ReadGauge`（`status.rs:69-157`）。
- 写路径：`dump.rs::dumpSQL → AddCounter`，表完成回调调用 `IncCounter`；`writer_util.rs` 的 SQL、CSV、Parquet 输出路径调用 `AddGauge`，SQL/CSV 错误回滚调用 `SubGauge`。
- 下游实现：所有包装函数最终调用 `stubs.rs` 中 `CounterVec/GaugeVec::With(None)` 返回的 inner；注册操作调用 `Registry::RegisterMetric`，注销操作调用 `Registry::Unregister`。
- 未接线包装：在 Rust 非测试生产文件中未找到 `IncGauge`、`DecGauge` 的调用，且未找到对 `errorCount`、`taskChannelCapacity` 的更新；它们目前是为 Go 对齐保留的 API/字段，而不是已验证的运行主链。

## 错误处理与边界

本文件所有 API 都不返回 `Result`。读包装在收到 `None` 时返回 `f64::NAN`；写包装在收到 `None` 时静默 no-op。这与 Go 的 nil 指标兼容语义一致，但调用者若把 `NaN` 当普通数参与计算，结果也会传播为 `NaN`。

当前 Rust `ReadCounter`/`ReadGauge` 直接读取桩的内存值，没有 Go 版本 `metric.Write(&dto.Metric)` 可能失败的分支。因此 Rust 的 `NaN` 只代表 `None`，不能代表底层采集写出失败。Factory 创建和 Registry 注册也无法返回错误；默认 registry 允许同名重复追加到 `names`，而 `samples` map 会用新闭包覆盖旧闭包（`stubs.rs:1084-1127`），语义弱于 Go `MustRegister` 对重复 collector 的失败行为。

`unregisterFrom` 忽略每次 `Unregister` 的 bool 结果，所以注销不存在的名称不会上报错误。注册文本固定使用 `# TYPE` 和单个无标签样本；它不输出 help、const labels 或 label 维度。Counter 包装没有阻止负数 `AddCounter`，桩也不实施 Prometheus counter 单调性校验；调用者必须自行维持只增约束。

## 并发与资源生命周期

`metrics: Clone` 与 `Arc<metrics>` 让 Dumper、writer 回调和后台状态线程共享同一存储。Counter 的 CAS 更新、chunk 原子量以及 registry 采样闭包的 `SeqCst` load 可在线程间可见；不过 Gauge 的复合更新不是 CAS，在多个 writer 同时对同一 gauge 加减时存在丢失更新风险，这是当前桩实现的并发限制，而非 Prometheus Gauge 的完整保证。

Registry 对名称和样本分别用 `Mutex` 保护；`Gather` 先克隆样本 map、按名称排序，再在锁外调用采样闭包（`stubs.rs:1091-1103`），避免采样时长期持有 registry 锁。注册闭包持有指标原子值的 `Arc`，所以即使外层 `metrics` 被释放，只要尚未注销，registry 仍会延长样本状态生命周期。正常资源顺序是 `Dump` 注册、运行期更新/采样、`Close` 注销；若调用方在未执行 `Close` 时丢弃 Dumper，当前 Rust 类型没有 `Drop` 自动注销。

进度后台线程由 `status.rs::LogProgressGuard` 管理，而非本文件管理；它克隆 `Arc<metrics>`，取消后 join。三个 chunk 原子量不需要单独释放，最后一个 `Arc` 释放时自动回收。

## 与 Go 版本的对应关系

`dumpling/export/metrics.go` 是直接语义对照：字段集合、构造函数、注册/注销函数以及八个读写包装函数一一对应；`dumpling/export/metrics_test.go:11-16` 只验证注册与注销能走通，Rust 的 `metrics_test.rs:9-36` 除复刻该测试外，还固定了六个注册名称及注销后的空列表。

主要差异如下：

- Go `newMetrics` 为每项提供 `Namespace=dumpling`、`Subsystem=dump`、Name、Help、ConstLabels；Rust Factory 只接收短名称，且 `_const_labels` 完全忽略。Rust 在 `registerTo` 输出时才硬编码完整样本名 `dumpling_dump_<name>`。
- Go 使用真实 `prometheus.CounterVec/GaugeVec` 与 `promutil.Factory/Registry`；Rust 使用 `stubs.rs` 的单样本原子存储，忽略所有 label 维度，Cargo 也未声明 Prometheus client。
- Go `registerTo` 调用 `MustRegister` 并传 collector；Rust 注册名称和文本采样闭包。Go 构造阶段即在 `NewDumper` 注册，并在构造失败时注销（`dump.go:96-149`）；Rust 在 `Dumper::Dump` 才注册，在 `Close` 注销（`dump.rs:36-114`），生命周期时点并不完全一致。
- Go 读取指标需写入 DTO，写失败返回 `NaN`；Rust 直接原子读取，仅 `Option::None` 返回 `NaN`。
- Go metrics 中三个 chunk 字段是非指针 atomic；Rust 使用 `Arc<Atomic*>` 以支持 clone 后共享。

因此当前 Rust 保留了导出业务需要的计数与文本暴露基本行为，但不是 Go Prometheus 行为的完整替代；尤其 const labels、help、collector 重复注册规则、counter 校验和 gauge 并发保证不能视为已经对齐。

## 扩展指南

- 新增导出指标时，应同时修改 `metrics` 字段、`newMetrics` 构造、`registerTo`/`unregisterFrom` 的名称集合，并扩展独立测试 `dumpling/export/metrics_test.rs`；若要求 Go 对齐，还应核对 `dumpling/export/metrics.go` 与 `metrics_test.go` 的 opts、类型和生命周期。
- 接入 `errorCount` 或 `taskChannelCapacity` 时，应在真实失败/队列边界处调用已有包装函数，并增加对应业务测试，不能仅以“已注册”代替“已上报”。队列容量是可升可降的瞬时量，应使用 Gauge；累计错误数应使用只增 Counter。
- 若增加 labels、help、直方图或多时间序列，最可能需要先扩展 `stubs.rs` 的 `Factory`、`*Vec::With` 和 `Registry`，再修改本文件。必须评估 HTTP gather 文本兼容性和既有监控面板中的 `dumpling_dump_*` 名称。
- 若并行 writer 会并发更新 gauge，应优先修复 `Gauge::Add/Sub` 的 CAS 语义或切换到完整指标后端，并用独立并发测试证明不会丢增量。
- 调整 chunk 进度时必须保持“先累计所有 total、后发布 ready”的顺序，并同步 `dumpling/export/status_test.rs`，覆盖零 chunk、完成数越界以及状态刷新线程。
- 本仓库要求 Rust 测试与源文件分离；新增回归应放在 `metrics_test.rs`，跨模块行为则放在相应的 `status_test.rs`、`writer_util_test.rs`、`dump_test.rs` 或 `http_handler_test.rs`，不要把 `#[cfg(test)]` 测试嵌入本文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter dumpling/export/metrics.rs` 确认目标文件含 14 个符号；`query` 精确定位 Rust `newMetrics`、`ReadCounter`、`AddGauge`；`explore` 给出 Rust `NewDumper → newMetrics`、`RefreshStatus → ReadCounter/ReadGauge`、`dumpSQL → AddCounter` 等调用者。图对 include 后 impl 方法的解析不完整，注册/注销边以直接源码核验补足。
- 目标与 crate：`dumpling/export/metrics.rs`、`dumpling/export/lib.rs:1-150`、`dumpling/export/Cargo.toml`。
- 指标底座：`dumpling/export/stubs.rs:885-1127`，核对 Factory、Vec、原子值、Registry、Gather 和 Unregister 的实际语义。
- Rust 调用链：`dumpling/export/dump.rs:14-114,175-205,334-375`、`dumpling/export/status.rs:1-190`、`dumpling/export/writer_util.rs:135-197,300-344,680-700`。
- Go 对照：`dumpling/export/metrics.go:1-170`、`dumpling/export/dump.go:96-149`。
- 独立测试：`dumpling/export/metrics_test.rs:1-36`、`dumpling/export/metrics_test.go:1-16`；相关跨模块测试入口还包括 `status_test.rs`、`writer_util_test.rs`、`dump_test.rs` 和 `http_handler_test.rs`。
- 人工复核结论：本文件存在于导出会话的共享观测层；运行路径是构造、注册、业务更新、状态读取、注销；安全扩展必须同步容器、构造、注册生命周期、调用点与独立测试，并正视本地桩相对 Go Prometheus 的语义缺口。
