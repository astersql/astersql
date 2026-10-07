# `pkg/metrics/owner.rs`

## 文件定位

本文件属于 `astersql-metrics` crate，由 `pkg/metrics/lib.rs` 通过 `pub mod owner` 公开。它不是 owner 选举算法本身，而是 owner 生命周期的 Prometheus 指标定义与初始化层：定义新建 etcd session、监听 owner key 以及竞选 owner 三类观测对象，并把构造结果发布到包级静态槽位。

包级总入口 `pkg/metrics/metrics.rs::InitMetrics` 在一次性初始化序列中调用 `owner::init_owner_metrics`；随后 `metrics.rs::RegisterMetrics` 才把三个 collector 注册到默认 Prometheus registry。初始化、注册与业务侧写入是三个独立阶段，不能仅凭本文件存在就推断 Rust owner 流程已经产生样本。

## 核心职责

1. 用 `NEW_SESSION_HISTOGRAM` 描述创建 owner 使用的 etcd session 所耗时间，并按 `type`、`result` 两个维度分组。
2. 用 `WATCH_OWNER_COUNTER` 记录 owner watcher 的终止或事件结果；本文件同时声明 `WATCHER_CLOSED`、`CANCELLED`、`DELETED`、`PUT_VALUE`、`SESSION_DONE`、`CTX_DONE` 六个 Go 对齐标签值。
3. 用 `CAMPAIGN_OWNER_COUNTER` 记录 owner 竞选结果，并以 `NO_LONGER_OWNER` 表示竞选期间发现本节点已失去 owner 身份的分支。
4. 通过 `init_owner_metrics` 一次构造并发布上述三个 collector，保持指标名称、帮助文本、变量标签和直方图桶与 `pkg/metrics/owner.go::InitOwnerMetrics` 一致。

本文件不负责发起竞选、监听 etcd、创建 session、递增计数器、记录耗时或注册 collector；这些动作分别属于 owner/etcd 调用方与 `pkg/metrics/metrics.rs`。

## 主要符号

- `NEW_SESSION_HISTOGRAM: Option<prometheus::HistogramVec>`：指标全名为 `tidb_owner_new_session_duration_seconds`，变量标签依次为 `type`、`result`。桶由 `ExponentialBuckets(0.0005, 2.0, 22)` 生成，即从 0.0005 秒开始翻倍，共 22 个桶，最后一个有限桶上界约为 1048.576 秒。
- `WATCH_OWNER_COUNTER: Option<prometheus::CounterVec>`：指标全名为 `tidb_owner_watch_owner_total`，变量标签依次为 `type`、`result`，用于区分 owner 类型/提示符与 watcher 结果。
- `CAMPAIGN_OWNER_COUNTER: Option<prometheus::CounterVec>`：指标全名为 `tidb_owner_campaign_owner_total`，同样使用 `type`、`result` 标签，表达竞选结果的累计次数。
- `WATCHER_CLOSED = "watcher_closed"`、`CANCELLED = "cancelled"`、`DELETED = "deleted"`、`PUT_VALUE = "put_value"`、`SESSION_DONE = "session_done"`、`CTX_DONE = "context_done"`：watch 流程的固定结果标签值。
- `NO_LONGER_OWNER = "no_longer_owner"`：竞选流程检测到当前节点不再是 owner 时的固定结果标签值。
- `pub unsafe fn init_owner_metrics()`：唯一函数入口；依次构造会话直方图、watch 计数器和竞选计数器，并写入三个 `static mut Option<_>`。

文件没有自定义 struct、enum、trait、impl、泛型或条件编译项。全部静态槽位和常量均为公开符号；构造函数虽然公开，但其 `unsafe` 签名把初始化时序与并发安全责任交给调用方。

## 执行流程

生产初始化与注册链如下：

1. `pkg/metrics/metrics.rs::InitMetrics` 进入 `INIT_METRICS_ONCE.call_once`，保证包级初始化序列只执行一次。
2. 初始化序列在 meta 指标之后、RawKV 指标之前调用 `crate::owner::init_owner_metrics()`。
3. `init_owner_metrics` 先用 `metricscommon::NewHistogramVec` 构造 session 耗时直方图。选项固定为 namespace `tidb`、subsystem `owner`、name `new_session_duration_seconds`，并配置 22 个指数桶。
4. 函数再用 `metricscommon::NewCounterVec` 构造 `watch_owner_total` 与 `campaign_owner_total`；两者均使用 `[LblType, LblResult]`，标签顺序属于监控接口契约。
5. 每次构造后立即把 collector 写入相应全局 `Option`，从 `None` 变为 `Some`。
6. 上层调用 `pkg/metrics/metrics.rs::RegisterMetrics` 时，`register_options!` 通过 `register_option` 依次注册三个 owner collector；若初始化未先完成，`expect("InitMetrics must run before RegisterMetrics")` 会 panic。
7. `ToggleSimplifiedMode(true)` 会注销 `CAMPAIGN_OWNER_COUNTER`，切回普通模式时重新注册它；另外两个 owner collector 不参与简化模式切换。

当前仓库的全量 Rust 引用搜索没有发现 session 创建、watch 或 campaign 业务代码读取这些静态槽位并写入样本。因此，上述已验证流程止于“构造、注册、模式切换”；Go 侧已有的业务采样接线不能视为 Rust 侧已完成。

## 数据与状态

三个 collector 均以 `pub static mut Option<_>` 保存，进程启动时为 `None`，初始化后为 `Some`。`Option` 对应 Go 包级指标指针初始化前的 nil 状态。collector 本身累计的时间序列状态由 `prometheus` crate 管理：Counter 只能累计，Histogram 保存每组标签对应的计数、总和与桶分布。

`metricscommon::NewHistogramVec` 和 `NewCounterVec` 最终经过 `pkg/metrics/common/wrapper.rs`，在构造时取得包级常量标签快照并写入 collector 选项。因此，常量标签必须在 `init_owner_metrics` 之前配置；初始化后修改包级常量标签不会重建已有 collector。

直接重复调用 `init_owner_metrics` 会用新 collector 覆盖三个槽位，原有采样状态不会转移。正常生产路径由 `InitMetrics` 的 `Once` 阻止重复初始化，但本函数本身没有锁或幂等保护。三个赋值也不是事务性的：若中途 panic，可能出现前面的槽位已更新而后面的槽位仍保留旧值或 `None`。

事件常量是无状态的 `&'static str`。调用方必须控制 `type` 和 `result` 的取值集合；任意动态错误文本作为标签值会造成高基数时间序列。Go 竞选错误分支当前会把 `err.Error()` 用作 `result`，这是既有兼容行为，也是移植时需要评估的基数风险。

## 依赖与调用关系

- 模块入口：`pkg/metrics/lib.rs` 公开 `owner` 模块；`pkg/metrics/Cargo.toml` 将该库定义为 `astersql-metrics`，入口为 `lib.rs`。
- 直接上游：RustCodeGraph 文件关系显示 `pkg/metrics/owner.rs` 被 `pkg/metrics/metrics.rs` 使用；源码确认 `InitMetrics -> owner::init_owner_metrics`、`RegisterMetrics -> 三个 owner collector`，以及 `ToggleSimplifiedMode -> CAMPAIGN_OWNER_COUNTER`。
- 构造适配：`crate::bindinfo::{compat_metricscommon, compat_prometheus}` 把 Go 风格选项和构造调用适配到 Rust；`crate::*` 提供 `LblType`、`LblResult` 等包级标签常量。
- Cargo 依赖：`pkg/metrics/Cargo.toml` 直接依赖 `prometheus = "0.14"`，并通过路径依赖 `astersql-metrics-common` 使用指标构造包装。
- Go 业务写入：`pkg/util/etcd.go` 观察 `NewSessionHistogram`；`pkg/owner/manager.go` 写入 `CampaignOwnerCounter` 和 `WatchOwnerCounter`，并使用本文件对应的事件字符串。
- Rust 业务写入现状：对三个静态名和全部事件常量的全仓 Rust 检索只命中定义、初始化、注册、简化模式和测试，未命中 `pkg/owner` 或 Rust etcd 业务侧采样。因而 Rust 当前没有与上述 Go 写入点等价的直接调用证据。

## 错误处理与边界

`init_owner_metrics` 不返回 `Result`。`metricscommon` 包装器内部调用 `prometheus::HistogramVec::new` 或 `CounterVec::new` 并使用 `expect`，所以非法指标名、帮助文本、标签或桶配置会 panic。当前参数全部为编译期固定值，现有单元测试仅覆盖 session 直方图的全名，没有直接覆盖这些 panic 边界。

初始化前读取静态槽位会得到 `None`；注册路径把这种顺序错误升级为 panic。业务调用若直接 `unwrap` 也具有相同边界。`with_label_values` 要求恰好两个标签且顺序为 `type`、`result`，数量不符会 panic；标签值语义错误则不会被本文件校验，只会产生错误或额外的时间序列。

`RegisterMetrics` 可能因重复 descriptor 返回 `prometheus::Error`，该错误由上层传播；本文件本身不参与恢复。简化模式重新注册 `CAMPAIGN_OWNER_COUNTER` 也可能返回注册错误，而注销失败被上层显式忽略。修改 owner 指标时必须同时考虑初始化、注册和简化模式列表的一致性。

## 并发与资源生命周期

collector 类型内部可支持并发观测，但承载它们的三个 `static mut Option<_>` 不提供同步保证。`init_owner_metrics` 标记为 `unsafe`，且自身不获取 `PACKAGE_INIT_LOCK`；其安全前提来自正常路径的 `INIT_METRICS_ONCE` 和“初始化先于并发业务”的启动时序。脱离总入口并发调用、读取或覆盖这些槽位会产生 Rust 数据竞争风险。

collector 通常随进程存活。注册时克隆 collector handle；若之后直接再次初始化并替换静态槽位，registry 仍可能持有旧 collector，而新写入可能落到未注册的新 collector，造成状态分裂。`ToggleSimplifiedMode` 通过独立 `MODE: Mutex<bool>` 串行化注册状态切换，但它不保护 owner 静态槽位的替换。

扩展时应优先把裸 `static mut` 收敛到 `OnceLock`、`LazyLock` 或其他线程安全且不可替换的容器；若为严格对齐现有包模式仍保留静态槽位，则所有初始化必须继续经由包级一次性入口，并避免在运行期替换 collector。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metrics/owner.go`。Rust 与 Go 均定义三个 collector、六个 watch 结果字符串和一个 campaign 结果字符串；namespace、subsystem、metric name、help、标签顺序以及 `ExponentialBuckets(0.0005, 2, 22)` 均一致。Rust 的大写下划线符号对应 Go 的导出驼峰符号，例如 `NEW_SESSION_HISTOGRAM` 对应 `NewSessionHistogram`，`PUT_VALUE` 对应 `PutValue`。

Rust 使用 `Option<Collector>` 表示 Go 指针初始化前的 nil，并把初始化函数标为 `unsafe`；Go 通过包 `init()` 自动调用 `InitMetrics`，Rust 则由显式入口和 `Once` 管理。Go `RegisterMetrics` 与 Rust `RegisterMetrics` 都注册三个 owner collector，且两侧简化模式都只切换 campaign 指标。

最重要的迁移差异在业务接线：Go `pkg/util/etcd.go` 在 session 创建结束时记录耗时和成功/失败标签；Go `pkg/owner/manager.go` 在竞选错误、失去 owner、watcher 关闭/取消、key 删除/写入、session/context 结束等分支递增计数。当前 Rust 全仓没有相应 collector 或事件常量的业务写入引用。因此文档只能确认“指标定义、初始化和注册已移植”，不能确认“owner 运行路径采样已移植”。

## 扩展指南

- 修改或新增 owner 指标时，以 `pkg/metrics/owner.go` 和实际 Go 写入点为语义基准，同步保持 metric name、help、标签名/顺序、桶和事件标签值；不要为迁就 Rust 现状删减 Go 分支。
- 若补齐业务采样，最可能接入 Rust 的 owner manager 与 etcd session 创建路径。应逐一映射 Go `pkg/owner/manager.go` 和 `pkg/util/etcd.go` 的分支，尤其区分 watcher channel 关闭、主动取消、DELETE、PUT、session done 与 context done。
- 新增 collector 后必须同步 `metrics.rs::InitMetrics`、`RegisterMetrics`，并判断是否属于 `ToggleSimplifiedMode` 的高开销集合。只定义静态量不构成完整交付。
- 测试应放在独立 Rust 测试文件，不要嵌入 `owner.rs`。可扩展 `pkg/metrics/metrics_2_aster_unit_test.rs`，或新增同目录 `owner_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 下挂载。
- 至少补测三个 descriptor 的全名/help/变量标签、22 个桶的首末边界、所有固定事件字符串、初始化后非空、注册后可 gather，以及简化模式只切换 campaign 指标。若补业务写入，还需对每个 Go 分支断言准确标签和增量。
- 兼容风险主要是监控查询/告警因重命名或标签变化失效、Go/Rust 事件分类不一致；正确性风险是初始化顺序或静态槽位替换导致 panic/状态分裂；性能风险是动态错误文本和其他无界标签值造成时间序列基数膨胀。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/metrics/owner.rs` 报告目标文件及 2 个索引符号。
- RustCodeGraph `node --file pkg/metrics/owner.rs`：核对三个静态 collector、七个事件常量和 `init_owner_metrics` 的完整定义；文件关系报告唯一使用文件为 `pkg/metrics/metrics.rs`。
- RustCodeGraph `query init_owner_metrics`：定位到 `pkg/metrics/owner.rs:52`；`explore` 报告 Rust `init_owner_metrics` 的上游为 `pkg/metrics/metrics.rs::InitMetrics`。精确 `callers`/`callees` 命令未产生可见输出，因此调用细节又由已索引的 `metrics.rs` 源码核对。
- 已读 crate/模块与下游实现：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`、`pkg/metrics/common/wrapper.rs`。
- 已读 Go 对照与业务证据：`pkg/metrics/owner.go`、`pkg/metrics/metrics.go`、`pkg/owner/manager.go`、`pkg/util/etcd.go`；全仓 Rust/Go 精确引用搜索用于确认写入点和 Rust 接线缺口。
- 已读相关独立测试：`pkg/metrics/metrics_2_aster_unit_test.rs::package_metrics_initialization_covers_go_success_boundary_and_error_paths` 直接验证初始化后 `NEW_SESSION_HISTOGRAM` 的全名；未发现 owner 专属 Rust 测试，也未发现两个计数器、桶、事件常量和业务写入分支的直接 Rust 断言。
- 本任务为纯文档分析，按计划不运行 Cargo；验证采用文档结构命令与人工源码/调用证据复核。
