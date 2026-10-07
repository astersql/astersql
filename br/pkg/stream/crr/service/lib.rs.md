# `br/pkg/stream/crr/service/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-stream-crr-service` 的 crate root，`br/pkg/stream/crr/service/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它指定为编译入口。该 crate 对应 Go 包 `br/pkg/stream/crr/service`，直接依赖 `br/pkg/stream/crr/internal/checkpoint` 的 Rust crate 与 `prometheus` 0.14，没有声明 Cargo feature。

本文件是模块门面，不实现检查点计算、HTTP 处理、状态机或指标更新。它把实现分配到同目录的 `http.rs`、`metrics.rs`、`service.rs` 和 `status.rs`，再将对外 API 组装成单一 crate 边界。目标目录不存在 `doc.go`；Go 的包边界由各 `.go` 文件共用的 `package service` 隐式形成，因而没有与 `lib.rs` 一对一的 Go 入口文件。

## 核心职责

- 通过 `#[path = "..."]` 声明 `http`、`metrics`、`service` 和 `status` 四个子模块，将 Go 的“单包多文件”布局映射成 Rust 模块树。
- 保持 `http`、`service`、`status` 为公开模块，使调用方可以显式使用 `crate::http::*`、`crate::service::*` 或 `crate::status::*`。
- 通过 `pub use service::*` 和 `pub use status::*` 提供扁平 API；例如外部 crate 可直接导入 `Config`、`DefaultRetryInterval`、`Service`、`New`、`StatusSnapshot` 和 `GetStatusFileName`，无需增加一层 `service::service` 或 `service::status`。
- 将 `metrics` 保持为 crate 私有模块。`status.rs::observe_status_metrics` 可在 crate 内刷新指标，但外部不能依赖指标实现细节。
- 仅在 `cfg(test)` 下挂载 `parity_test.rs`、`service_test.rs` 和 `status_test.rs`，保持生产与测试源码分文件。

## 主要符号

`lib.rs` 本身不定义常量、类型、trait、函数或 `impl`，其有意义的符号都是模块声明与再导出：

- `pub mod http`：公开 HTTP 适配边界，主要提供 `HttpMux`、`HttpResponseWriter`、`HttpHandler`、`Service::Register` 和 `RegisterOrPanic`；它不被 glob 再导出，这些类型应通过 `http` 模块路径访问。
- `mod metrics`：crate 私有的 Prometheus 指标实现，由 `status.rs` 调用，并为 crate 内测试提供 `pub(crate)` 查询辅助。
- `pub mod service` 与 `pub use service::*`：声明并扁平导出核心服务 API。关键符号包括 `DefaultRetryInterval`、`Config`、`UpstreamCheckpointWaiter`、`ResumeStateStore`、`Deps`、`CalculatorConfig`、`Service`、`New` 和 `NewExistenceSyncChecker`。
- `pub mod status` 与 `pub use status::*`：声明并扁平导出状态 API。关键符号包括 `STATE_*`、`PHASE_IDLE`、`GetStatusFileName`、`StatusStatistic`、`StatusSnapshot`、`StatusStore`、`StatusObserver` 和 `encode_status_snapshot`；`new_status_store` 是 `pub(crate)`，不构成外部 API。
- `parity_test`、`service_test`、`status_test`：三个私有且只在测试构建中存在的模块。

crate 根的 `#![allow(...)]` 对整个 crate 允许 Go 风格的大写命名、暂未使用的移植符号与 Clippy 警告。这是移植兼容策略，也会降低编译器对新问题的提示强度，不应被视为新代码可忽略质量检查的通用授权。

## 执行流程

`lib.rs` 在运行时没有主动执行流程；它的作用发生在编译期，并将运行时调用链暴露给使用方：

1. Cargo 以 `lib.rs` 为 crate root，按四个 `#[path]` 纳入子模块。
2. `service::*` 和 `status::*` 在 crate 根展开，因此 `br/pkg/stream/crr/config/config.rs` 能使用 `astersql_br_pkg_stream_crr_service::{Config, DefaultRetryInterval}` 构造 CLI 默认配置。
3. 真实服务若由调用方构造，入口是 `service.rs::New(Deps, Config)`：它创建 checkpoint `Calculator`、共享状态仓和 observer。
4. `Service::Run` 先将状态设为 running，然后重复“加载/刷新 resume state → 计算下一 checkpoint → 持久化进度 → 无进展时等待 PD”；退出时 `RunStopGuard::drop` 尝试落盘 pending state 并置 stopped。
5. `StatusObserver` 将 calculator 事件写入 `StatusStore`，状态更新同时刷新私有 `metrics` 模块；`Service::Register` 注册 `/livez`、`/readyz` 和 `/status`，只读状态快照，不触发计算。

必须区分“crate 能力”与“完整应用已接线”：当前 Rust `br/pkg/task/operator/crr_checkpoint.rs` 从自身 `stubs.rs` 导入 `NewCRRService`，该函数返回的是最小 `CRRService` 桩，不是本 crate 的 `Service`。因此上述真实运行链已在本 crate 内实现并由独立测试覆盖，但从 Rust operator 的应用级创建路径尚未接入它。

## 数据与状态

`lib.rs` 自身不存储任何运行时数据。它暴露的主要状态所有权分布在子模块：

- `Service` 持有 `Mutex<Calculator>`、`StatusStore`、`StatusObserver`、watcher、可选 `ResumeStateStore`、`Config`、`Mutex<bool>` 初始化标志，以及 `Mutex<Option<PersistentState>>` pending 快照。
- `StatusStore` 使用 `Arc<RwLock<StatusStoreInner>>` 保护 `StatusSnapshot`；初始状态为 `starting/idle`，运行期在 `running` 与 `degraded` 间转换，最终进入 `stopped`。
- `StatusSnapshot` 包含 live/ready、轮次、上下游水位、按 store 进度、文件统计、最近成功/失败时间和连续失败计数；`snapshot_copy` 深拷贝 map，避免读取者保持锁或与内部共享可变别名。
- `metrics.rs` 在进程级 `OnceLock<GaugeMap>` 中惰性初始化 Prometheus gauges；state/phase 按标签以 one-hot 0/1 更新。
- resume state 的稳定相对路径是 `crr-checkpoint/resume-state.json`，由扁平导出的 `GetStatusFileName` 返回。

## 依赖与调用关系

下游依赖由 `Cargo.toml` 和子模块进口确认：

- `service.rs` 从 `astersql-br-pkg-stream-crr-internal-checkpoint` 使用 `Calculator`、`NewCalculator`、`CheckpointCalculatorConfig`、`Context`、`Error`、存储/PD trait 与 `PersistentState`。
- `status.rs` 实现 checkpoint `Observer`，消费 `CheckpointEvent`、`EventType`、`FileStatistic` 和 `PersistentState`，并调用 `crate::metrics::observe_status_metrics`。
- `metrics.rs` 使用 `prometheus::{GaugeVec, Opts}` 注册 `tidb_br_crr_*` gauges。
- `http.rs` 依赖 crate 根扁平导出的 `Service` 以及 `status::encode_status_snapshot`，把服务快照转换成探针状态码和 JSON。

已核实的上游 Rust 使用有两类：

- `br/pkg/stream/crr/config/config.rs` 是当前生产源中直接依赖本 crate 的文件，它将 `Config` 重命名为 `ServiceConfig`，并使用 `DefaultRetryInterval` 组装 CRR CLI 默认值；该 config crate 的 `Cargo.toml` 以 `path = "../service"` 声明依赖。
- crate 内 `parity_test.rs`、`service_test.rs`、`status_test.rs` 通过 `crate::*` 或子模块路径验证公开契约与内部不变量。

RustCodeGraph 将 `lib.rs` 识别为只含一个文件级符号的门面，因而不存在可对 `lib.rs` 内部函数执行 `callers/callees` 的调用边。精确的行为调用边位于子模块，例如 `Service::Run -> run_once -> Calculator::ComputeNextCheckpoint`、`run_once -> wait_checkpoint_advance -> UpstreamCheckpointWaiter::WaitGlobalCheckpointAdvance`、`StatusObserver::OnCheckpointEvent -> StatusStore::apply_event -> observe_status_metrics`。

## 错误处理与边界

`lib.rs` 不创建或转换错误；它只决定错误 API 从哪些模块可见。实际边界由所属子模块维护：

- `New` 返回 checkpoint crate 的 `Error`，并将非正 `RetryInterval` 恢复为一秒；当前 Rust `Deps` 用 `Box<dyn ...>` 表示必填 PD/watcher/upstream/sync，不存在 Go `nil` watcher 的同型表达。
- calculator 已通过 observer 上报的失败用 `RunOnceError::ObservedCalculator` 标记，其他 watch/resume 失败用 `Other` 由服务层记录，避免连续失败重复计数。
- resume load/save 错误增加 `load resume state` / `save resume state` 上下文并进入重试；关机 flush 是 best-effort，失败仅输出日志，不改写退出结果。
- `/livez` 和 `/readyz` 分别在 `Live` / `Ready` 为 false 时返回 503；`/status` 成功返回 200 与 `application/json`，编码失败返回 500。`RegisterOrPanic` 对缺失 mux 以 `service: nil mux` panic，对齐 Go 的启动期快速失败契约。
- `encode_status_snapshot` 显式处理 JSON 转义、Go 零值时间、负的有符号计数、map 键排序与 RFC3339 年份范围；年份超出 `[0,9999]` 时会返回 `Error`。

crate 根的公开边界还需注意：`metrics` 刻意不公开；三个测试模块不会进入非测试构建；`http` 模块虽公开，其成员没有被 crate 根 glob 导出。

## 并发与资源生命周期

`lib.rs` 本身不创建线程、锁、channel、网络服务或事务。它所暴露的并发契约来自子模块：

- `Service` 用 `Mutex` 串行化 calculator、resume 初始化标志和 pending state 访问；其依赖 trait 要求 `Send + Sync`，以便服务可在工作线程中运行。
- `StatusStore` 的 `Arc<RwLock<_>>` 允许 HTTP/指标读取与计算事件写入共享状态；快照离开锁前深拷贝，降低长时间持锁风险。
- HTTP handler 持有 `Arc<Service>` clone，其闭包类型是 `Send + Sync`，因而生命周期不依赖注册时的临时借用。
- Prometheus collectors 经 `OnceLock` 在进程内只初始化和注册一次。
- `Run` 的 `RunStopGuard` 提供 RAII 清理：无论正常取消还是提前返回，均先用以 `RetryInterval` 为上界的独立 context 尝试刷新 pending resume state，然后将状态标记为 stopped。
- 重试休眠每最多 5ms 检查一次 `Context`，使取消可以中断 backoff；无 checkpoint 进展时通过 watcher 阻塞，避免空转。

## 与 Go 版本的对应关系

Go 中 `http.go`、`metrics.go`、`service.go`、`status.go` 共属 `package service`，不需要显式模块门面。Rust `lib.rs` 就是对这一语言差异的结构性补偿：`pub use service::*` 和 `pub use status::*` 尽量还原 Go 包内公开符号位于同一命名空间的体验，而 HTTP 适配类型保留在显式 `http` 子模块。

主要语义对应如下：

- Rust `service.rs::Config/Deps/Service/New/Run` 对应 Go `service.go` 同名结构与方法；默认重试间隔同为一秒，外层计算、watch、错误上报与 resume 流程保持同构。
- Rust `http.rs::Service::Register` 对应 Go `http.go::(*Service).Register`，三条路由、HTTP 状态码和 JSON 换行契约一致；Rust 通过 trait 抽象 mux/writer，Go 直接使用 `net/http`。
- Rust `StatusStore/StatusObserver/StatusSnapshot` 对应 Go `statusStore/statusObserver/StatusSnapshot`；Rust 用 `Arc<RwLock<_>>` 对应 Go `sync.RWMutex`，用手写 JSON 对齐 Go `encoding/json` 的零值、排序与转义细节。
- Rust `metrics.rs` 对应 Go `metrics.go`，指标 namespace/subsystem/name、标签与状态刷新项目一致；Go 在 `init` 中注册，Rust 在首次观察时经 `OnceLock` 惰性注册。
- Rust 的三个测试模块与 `service_test.go` 的主要场景对齐，并增加 `parity_test.rs` 总契约以及 `status_test.rs` 的 Go JSON 对抗边界。

迁移状态不是“已完全替换 Go 生产路径”：Rust operator 尚在使用 `br/pkg/task/operator/stubs.rs` 内的服务桩。扩展或评估行为时，必须同时区分本 crate 已实现的行为、crate 内测试证明的契约，以及应用级尚缺的真实接线。

## 扩展指南

- 新增对外功能时，先确定它属于服务循环、状态模型、HTTP 适配还是指标内部实现。只有需要改变模块可见性、增加子模块或调整 crate-root 扁平 API 时才应修改 `lib.rs`。
- 在 `service.rs` 或 `status.rs` 增加 `pub` 符号会因 glob 导出自动扩大 crate 根 API；提交前应检查名称冲突、下游兼容性和是否真需要扁平导出。HTTP 新类型默认应保留在 `http` 命名空间，指标辅助默认保持 `pub(crate)`。
- 新增模块时使用明确 `#[path]`，并评估它是 `pub mod`、私有 `mod` 还是仅 `cfg(test)` 挂载。生产逻辑与 Rust 测试必须保持在独立文件，不应在 `lib.rs` 中内嵌 `#[cfg(test)] mod tests { ... }`。
- 修改运行行为时同步更新对应 Go 意图与 Rust 独立测试：主循环/resume/watch 改动扩展 `service_test.rs`，公开契约或 HTTP/指标改动扩展 `parity_test.rs`，JSON/时间边界改动扩展 `status_test.rs`；Go 对照测试是 `service_test.go`。
- 若要把真实 `Service` 接入完整 Rust 应用，不能只改 `lib.rs`；需要在 operator 组装层用本 crate 的 `Deps/Config/New`替代 `stubs.rs::NewCRRService`，并为 PD reader、watcher、upstream storage、sync checker 和 resume store 提供真实 trait 实现。这属于本文档任务范围外，且应用独立实施计划和集成测试控制风险。
- 保留 crate 根宽泛 `allow` 时，新代码仍应单独运行和审查适用的 lint；删除或缩小 `allow` 属性前则需评估全 crate 的 Go 风格 API 兼容成本。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件，其中 7032 个 Rust 文件；`files --filter br/pkg/stream/crr/service` 找到 13 个 Go/Rust 源文件，`node --file br/pkg/stream/crr/service/lib.rs` 确认目标文件为 51 行门面。
- RustCodeGraph 读取与调用证据：`service.rs` 中 `New`、`Service::Run/run_once`、`wait_checkpoint_advance`、resume load/save 与 `RunStopGuard`；`status.rs` 中 `StatusStore`、`StatusObserver`、`new_status_store`、`encode_status_snapshot`；`http.rs` 中 `Service::Register` 和三个 handler；`metrics.rs` 中 `GaugeMap`、`metrics` 和 `observe_status_metrics`。
- crate 边界证据：`br/pkg/stream/crr/service/Cargo.toml` 声明 `lib.rs`、Go 包元数据、checkpoint path 依赖和 `prometheus = "0.14"`；`br/pkg/stream/crr/config/Cargo.toml` 与 `config.rs` 确认当前外部 Rust 使用是 `Config`/`DefaultRetryInterval`。
- Go 对照证据：`br/pkg/stream/crr/service/service.go`、`http.go`、`status.go`、`metrics.go` 以及 `service_test.go`。它们确认服务循环、状态机、探针、resume path、指标集和主要测试场景。
- Rust 独立测试证据：`parity_test.rs` 覆盖正常 HTTP/状态、默认值与 nil mux、错误去重计数、关机落盘与指标；`service_test.rs` 覆盖成功/失败、watch 恢复、resume load/save 重试、关机 flush、HTTP、状态统计和指标注册；`status_test.rs` 覆盖 epoch 前时间、map 键排序和 Go RFC3339 年份范围。
- 应用接线限制证据：`br/pkg/task/operator/crr_checkpoint.rs::NewCRRCheckpointService` 导入并调用 `crate::stubs::NewCRRService`；`br/pkg/task/operator/stubs.rs::NewCRRService` 明确标注为最小桩，且只保留配置与 closed 标志。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前应用任务文件指定的结构检查，确认文档存在且恰好包含本文的 11 个固定二级章节。
