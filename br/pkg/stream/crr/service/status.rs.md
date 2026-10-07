# `br/pkg/stream/crr/service/status.rs`

## 文件定位

本文件是 `astersql-br-pkg-stream-crr-service` crate 的运行状态层。crate 入口 `br/pkg/stream/crr/service/lib.rs` 以 `pub mod status` 挂载并再导出本模块；`Cargo.toml` 声明它是对应 Go 包 `br/pkg/stream/crr/service` 的 library crate，并直接依赖 `astersql-br-pkg-stream-crr-internal-checkpoint`。它位于 checkpoint calculator 与服务可观测面之间：向下接收 `CheckpointEvent`/`PersistentState`，向上为 `Service::Status`、`/livez`、`/readyz`、`/status` 和 Prometheus 指标提供一致快照。

该文件不是单纯数据定义。它同时承担状态机、线程安全快照、calculator `Observer` 适配、Go 兼容 JSON 编码及 RFC3339Nano 时间编码。服务循环本身在 `service.rs`，HTTP 路由在 `http.rs`，指标 collector 在 `metrics.rs`；本文件只维护这些消费者共同读取的状态事实。

## 核心职责

1. `StatusStore` 把服务生命周期、计算轮次、checkpoint 水位、按 store 同步进度、文件统计和最近错误聚合到一个 `StatusSnapshot`。
2. `StatusObserver` 实现 checkpoint crate 的 `Observer` trait，把 calculator 发出的 `CheckpointEvent` 转交给 `StatusStore::apply_event`；服务层自身的 watcher/resume 错误也经同一入口记录。
3. 每次构造或写状态后调用 `observe_status_metrics`，使指标与锁内快照来自同一次状态变更。
4. `snapshot_copy` 返回包含三个 map 的深拷贝，HTTP 或调用方修改副本不会反向污染状态仓。
5. `encode_status_snapshot` 手写稳定的 Go 兼容 JSON：固定字段顺序、按 `omitempty` 省略空 map/空错误、字典键排序、Go 零值时间、HTML 安全字符串转义和 RFC3339Nano 时间格式。
6. `GetStatusFileName` 固定返回 `crr-checkpoint/resume-state.json`，这是 resume state 的外部路径契约。

## 主要符号

- `STATE_STARTING`、`STATE_RUNNING`、`STATE_DEGRADED`、`STATE_STOPPED` 与 `PHASE_IDLE`：状态机和初始阶段的稳定字符串。`State` 描述服务健康，`Phase` 描述最近 calculator 事件类型。
- `StatusStatistic`：从 checkpoint `FileStatistic` 投影出的文件工作量，包括读取、跳过、估算、下游检查次数和两组后缀计数。
- `StatusSnapshot`：对外快照。除健康状态外，还保存轮次/迭代、水位、store 进度、pending 数、成功/错误时间和连续失败数。
- `StatusStoreInner` 与 `StatusStore`：前者持有快照，后者以 `Arc<RwLock<_>>` 共享并同步访问；`StatusStore: Clone` 只克隆 `Arc`，不会复制出相互独立的状态。
- `StatusStore::{start, stop, set_persistent_state, clear_failure, begin_round, apply_event, snapshot_copy}`：完整写入和读取 API。`begin_round` 是内部方法，由 observer 的公开入口调用。
- `StatusObserver::{BeginCalculationRound, OnCheckpointEvent}` 及其 `Observer` trait 实现：服务层和 calculator 共用的事件桥接器。
- `new_status_store`：构造 `starting/idle` 快照，并返回共享同一 store 的 `(StatusStore, StatusObserver)`。
- `new_status_statistic`：复制 calculator 统计，隔离 map 所有权。
- `encode_status_snapshot`：公开 JSON 编码入口；`append_json_*`、`escape_json_string`、`humantime_rfc3339`、`time_format_rfc3339`、`civil_from_days` 是其内部编码流水线。

## 执行流程

1. `Service::New` 调用 `new_status_store(task_name)`。初始 `Live=false`、`Ready=false`、`State=starting`、`Phase=idle`，并立即上报一次指标；随后把克隆的 `StatusObserver` 注入 `NewCalculator`。
2. `Service::Run` 首先调用 `StatusStore::start`，设置 `Live/Ready=true`、`State=running`、`Phase=idle`。每轮 `run_once` 通过 `BeginCalculationRound` 递增 `CurrentRound`，清零迭代号和 pending 数。
3. calculator 经 `Observer::OnCheckpointEvent` 发出阶段事件；服务自身的非 calculator 错误由 `record_service_failure` 构造 `EventCalculationFailed` 并调用固有方法 `StatusObserver::OnCheckpointEvent`。两条路径最终都进入 `apply_event`。
4. `apply_event` 先更新事件时间、阶段和迭代号；非零上游 checkpoint/同步 TS 才覆盖旧值。`SyncedByStoreSet=true` 或 map 非空表示事件确实携带了 store 快照，因此显式空 map 能清除已移除 store。只有 `EventRoundPlanned` 和 `EventCheckpointAdvanced` 更新 `AliveStoreCount`。
5. `EventCheckpointAdvanced` 恢复 `running/ready`，写最近成功时间并清除错误与连续失败；`EventCalculationFailed` 转为 `degraded/not ready`、保存错误和时间并递增失败数；其他阶段事件仅在当前不为 `degraded` 时保持 `running`，防止中间事件掩盖失败。
6. resume state 加载或保存成功后，`set_persistent_state` 更新 `SafeCheckpoint`、`SyncedTS`、`SyncedByStore`，随后 `clear_failure` 恢复健康；若状态已是 `stopped`，`clear_failure` 不再复活服务。
7. `Service::Status` 调用 `snapshot_copy`；探针读取 `Live/Ready`，`/status` 调用 `encode_status_snapshot` 并追加 Go `json.Encoder.Encode` 风格的换行。`RunStopGuard::drop` 最后调用 `stop`，进入不可就绪、不可存活的终态。

## 数据与状态

状态机主路径是 `starting -> running -> degraded -> running`，退出时转为 `stopped`。`degraded` 可由 checkpoint 计算失败、watcher 错误或 resume 读写错误触发；checkpoint 成功推进或 resume 操作成功会清除失败。`stopped` 是保护性终态：`clear_failure` 在此状态直接返回。

`CurrentRound` 在每次计算前单调递增；`LastLoopIteration` 和 `PendingFileCount` 在新轮开始时归零。`LastUpstreamCheckpoint`、`SyncedTS` 对事件中的零值采用“未携带”语义，不覆盖已有水位。`SafeCheckpoint` 只随已加载/已成功持久化的 `PersistentState` 更新，因此与观察到的上游进度含义不同。

`SyncedByStore` 的 Go 原型用 nil/非 nil map 区分“未携带”和“携带空集合”。Rust 用 `CheckpointEvent::SyncedByStoreSet` 恢复这一区别：空 map 且标志为 true 必须清空旧进度。`AliveStoreCount` 只接受规划/推进事件，失败事件保留最后一次有效规划值。`Statistic` 仅在事件携带 `Some(FileStatistic)` 时整体替换。

三个时间字段使用 `Option<SystemTime>` 表示内部零值；JSON 中 `None` 不写 `null`，而写 Go `time.Time{}` 的 `"0001-01-01T00:00:00Z"`。非零纳秒会去掉末尾零，epoch 前时间按欧几里得除法正确归一化。

## 依赖与调用关系

上游调用关系由 `service.rs` 明确给出：`Service::New -> new_status_store`；`Service::Run -> StatusStore::start`；`run_once -> StatusObserver::BeginCalculationRound`；calculator 的 observer 回调以及 `record_service_failure -> StatusObserver::OnCheckpointEvent -> StatusStore::apply_event`；resume 初始化/flush 分别调用 `set_persistent_state` 和 `clear_failure`；`RunStopGuard::drop -> StatusStore::stop`。

下游依赖来自 checkpoint crate 的 `CheckpointEvent`、`EventType`、`FileStatistic`、`PersistentState`、`Error` 和 `Observer`。`calculator.rs` 规定 `Observer: Send + Sync`，事件字符串由 `EventType::as_str` 稳定映射为 `waiting_upstream`、`upstream_advanced`、`round_planned`、`waiting_downstream`、`checkpoint_advanced`、`calculation_failed`。

读取侧为 `Service::Status -> snapshot_copy`。`http.rs` 的 liveness/readiness handler 分别读取 `Live` 和 `Ready`，status handler 调用 `encode_status_snapshot`。`metrics.rs` 的 `observe_status_metrics` 消费 `StatusSnapshot` 并更新 `tidb_br_crr_*` gauges。本文件每条写路径都在持写锁期间调用它，构造函数也报告初始状态。

crate 边界由 `br/pkg/stream/crr/service/Cargo.toml` 确认：除标准库外，状态事件来自内部 checkpoint crate，指标实现使用 `prometheus`；本文件本身不直接依赖 HTTP 框架或序列化库。

## 错误处理与边界

- 所有锁通过 `expect("status store lock")` 获取；线程持锁 panic 会毒化锁，后续访问也会 panic，而不是静默返回旧快照。
- `EventCalculationFailed` 只有在 `Err=Some` 时覆盖 `LastError`；无错误对象的失败仍更新时间、累加计数并降级，但会保留旧错误文本。这与当前源码行为一致，扩展时不可假定错误文本一定对应最后一次失败。
- `apply_event` 的水位零值不会覆盖旧值；若业务需要表达“明确归零”，必须扩展事件协议，不能仅传 0。store map 已通过 `SyncedByStoreSet` 单独解决该歧义。
- JSON 字符串会转义引号、反斜杠、控制字符以及 `<`、`>`、`&`、U+2028/U+2029，对齐 Go `encoding/json` 的 HTML 安全行为。map 键排序保证 HashMap 遍历随机性不影响输出。
- `encode_status_snapshot` 当前唯一可返回的错误来自时间年份不在 `[0,9999]`；`http.rs` 将此错误映射为 HTTP 500。正常字段追加写入 `String` 不会失败。
- 数量字段保持 `i32`，避免负边界被转换成巨大 `u64`。时间算法支持 Unix epoch 前时间，并在越界年份返回带 Go 风格文案的 `Error`。
- `STATUS_FILE_NAME` 是持久化兼容契约；修改会影响恢复文件发现、测试及运维脚本。

## 并发与资源生命周期

`StatusStore` 使用 `Arc<RwLock<StatusStoreInner>>`。构造返回的 store 与 observer 共享同一个 `Arc`；服务、calculator 回调和 HTTP 查询可以跨线程使用。写方法一次持有写锁完成字段变更和指标上报，读方法只在克隆快照期间持读锁，返回后立即释放。

`snapshot_copy` 深拷贝 `SyncedByStore` 和两组统计 map，避免调用方修改副本；`new_status_statistic` 与 `set_persistent_state` 也 clone map，避免 calculator/resume 数据与状态仓形成共享可变别名。Rust 的所有权已经使普通 `HashMap` clone 安全，显式拷贝同时记录 Go `maps.Clone` 的兼容意图。

本文件不创建线程、异步任务或通道。生命周期由 `Service::Run` 控制：start 后持续接收事件；退出路径的 `RunStopGuard` 先尽力 flush pending resume state，再调用 stop。需要注意 `observe_status_metrics` 在写锁内执行，因此新增指标逻辑必须保持短小且不可回调状态仓，否则会延长写锁或造成重入死锁风险。

## 与 Go 版本的对应关系

直接对照文件为 `br/pkg/stream/crr/service/status.go`。Rust 的 `StatusStatistic`/`StatusSnapshot`、四态常量、`start`、`stop`、`set_persistent_state`、`clear_failure`、`begin_round`、`apply_event`、`snapshot_copy`、observer 和统计投影逐项对应 Go 实现；Rust 保留了 Go 风格公开命名以维持迁移 API。

主要语言差异有三项。第一，Go 的 `*statusStore` 加 `sync.RWMutex` 被映射为可克隆的 `Arc<RwLock<StatusStoreInner>>`，并将 observer 与 store 一次性成对构造。第二，Go `map == nil` 的事件语义由 Rust 的 `SyncedByStoreSet` 补偿。第三，Go 依赖 `encoding/json` 和 `time.Time.MarshalJSON`，Rust 用手写编码器复现字段标签、`omitempty`、排序、HTML 转义、零时间、epoch 前时间和年份范围。

Go 的 `newStatusObserver` 是单独构造步骤；Rust `new_status_store` 直接返回二元组。Go 失败日志使用结构化 `log.Error`/`zap.Error`，Rust 当前使用 `eprintln!`，日志后端并不等价，但状态字段与迁移测试验证的行为一致。Rust 额外公开 `encode_status_snapshot`，供无 serde 依赖的 HTTP 层复用。

Go 测试 `service_test.go` 验证端点、统计深拷贝、失败时保留 alive store、显式规划 0 store 和稳定状态文件名；Rust 的 `service_test.rs`/`parity_test.rs` 对应这些行为，并额外覆盖空 store 进度清理、签名整数、指标注册。`status_test.rs` 专门覆盖 map 排序、epoch 前时间和超出 Go RFC3339 年份范围的错误。

## 扩展指南

- 新增快照字段时，应同时修改 `StatusSnapshot`、产生该值的 `apply_event`/生命周期方法、`encode_status_snapshot` 的字段名与顺序、`observe_status_metrics`（若需指标）、Go `StatusSnapshot`，并在独立的 `status_test.rs` 或 `service_test.rs` 增加正常值、零值和错误边界测试。
- 新增 calculator 事件类型时，应先在 checkpoint crate 的 `EventType`/`CheckpointEvent` 定义稳定字符串和载荷，再明确它是否能恢复 ready、是否更新 alive store、水位或统计；最后扩展 `apply_event` 的 match 和 parity 测试。不要让普通中间事件自动清除 `degraded`。
- 新增 map 字段要明确区分“未携带”和“显式空集合”。若 Go 使用 nil 语义，Rust 事件载荷应提供与 `SyncedByStoreSet` 等价的存在标记，并测试清空旧值。
- 调整 JSON 时必须与 Go tag、`omitempty`、map 键排序、HTML 转义和 `json.Encoder` 换行保持一致；时间逻辑的改变应补充 epoch 前、纳秒裁剪、零时间和年份越界用例。
- 修改并发策略时必须保持快照原子性和短锁周期。指标更新若迁出写锁，需要设计版本或快照机制，避免状态与 gauge 短暂不一致。
- 测试逻辑应继续放在独立文件：JSON 边界放 `status_test.rs`，服务/端点状态流放 `service_test.rs`，Go/Rust 公共契约放 `parity_test.rs`；不要把测试嵌入 `status.rs`。

兼容性风险主要是外部 JSON/指标/状态路径契约；正确性风险集中于失败恢复和零值携带语义；性能风险集中于高频事件下的写锁、整份 map clone 和锁内指标更新。扩展前应先判断字段是否真的需要每个事件刷新。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`files --filter br/pkg/stream/crr/service/status.rs` 找到目标文件，`node --file ...` 读取了完整 653 行源码。
- RustCodeGraph 符号证据：`query StatusStore` 定位 Rust `StatusStore`/`StatusStoreInner`/`StatusObserver` 和 Go `statusStore`；`query CheckpointEvent`、`query Observer --kind trait`、`query FileStatistic` 定位 `br/pkg/stream/crr/internal/checkpoint/calculator.rs` 的事件、trait 和统计定义。精确 `callers encode_status_snapshot` 查询在本地挂起，调用边改由局部源码搜索核验，未据此推断未知调用者。
- 已读生产文件：`br/pkg/stream/crr/service/status.rs`、`lib.rs`、`service.rs`、`http.rs`、`metrics.rs`、`Cargo.toml`，以及直接依赖 `br/pkg/stream/crr/internal/checkpoint/calculator.rs`。
- 已读 Go 对照：`br/pkg/stream/crr/service/status.go`、`service.go`、`http.go` 和 `service_test.go` 的相关段落。
- 已读 Rust 测试：`br/pkg/stream/crr/service/status_test.rs`、`service_test.rs`、`parity_test.rs` 的状态与 JSON 相关用例。
- 关键调用边以源码核验：`Service::New -> new_status_store`；`Service::Run -> start`；`run_once -> BeginCalculationRound`；calculator/服务错误 `-> OnCheckpointEvent -> apply_event`；`Service::Status -> snapshot_copy`；HTTP status handler `-> encode_status_snapshot`；`RunStopGuard::drop -> stop`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构验证，并检查 Git 提交只包含本文件。
