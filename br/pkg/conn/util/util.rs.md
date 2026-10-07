# `br/pkg/conn/util/util.rs`

## 文件定位

本文件是 Cargo crate `astersql-br-pkg-conn-util` 的主体实现，crate 根 `br/pkg/conn/util/lib.rs` 通过 `#[path = "util.rs"] pub mod util` 将其公开，并通过同一 crate 的 `stubs::kvproto` 获得精简的 `metapb::Store` 类型。它对应 Go 包文件 `br/pkg/conn/util/util.go`，位于 BR 与 PD/TiKV 连接边界：把 PD 返回的 store 元数据筛成调用方需要的节点集合，组合 PD 时间戳，并逐个访问 TiKV status 服务的 `/config`。

这不是进程入口，也不拥有 PD、HTTP 客户端或 store 缓存；它以 `StoreMeta`、`PdClient`、`HttpClient`、`CancelContext` 四个 trait 接收外部能力。RustCodeGraph 将该文件记录为被 65 个文件引用，并确认直接业务调用覆盖 `br/pkg/restore/import_mode_switcher.rs`、`br/pkg/restore/snap_client/client.rs`、`br/pkg/restore/snap_client/stubs.rs` 和 `br/pkg/utils/storewatch/watching.rs` 等恢复、限速和 store 监听路径。迁移期的 `br/pkg/conn/conn.rs` 另有一套相近实现；因此不能假定所有 BR Rust 路径都已经统一委托到本文件。

## 核心职责

1. `GetAllTiKVStores` 根据 `StoreBehavior` 处理带 `engine=tiflash` 标签的节点，并要求 `StoreMeta::GetAllStores` 排除 tombstone；`GetAllTiKVStoresWithRetry` 在外层增加激进 PD 退避。
2. `GetCurrentTsFromPD` 将 PD 的 physical/logical 两部分组合为 TSO，`GetCurrentTsFromPDWithRetry` 增加重试、告警和最终错误日志。
3. `GetConfigFromTiKVStores` 只遍历 `StoreState::Up` 的节点，为每个节点生成 `/config` URL，并对 HTTP 请求及回调整体重试；`GetConfigBytesFromTiKVStores` 再约束 HTTP 200 并把响应体交给收集回调。
4. `HandleTiKVAddress` 规范化 TiKV status 地址；当 status 地址和服务地址的 hostname 不一致时，保留 status 端口与 URL 其余部分，但改用服务地址的 hostname。
5. `with_retry`、`StatusUrl`、TiFlash label 适配器等私有逻辑，为上述公开函数保存 Go 版本的取消、错误汇聚、URL 和标签判断语义。

## 主要符号

- `CancelContext::is_cancelled(&self) -> bool`：Go `context.Context` 的最小取消面。它不携带 deadline 或取消原因，只供重试循环轮询。
- `HttpClient`：要求实现者可并发共享（`Send + Sync`）；`Get` 返回完整的 `HttpResponse`，`CloseResponse` 表达 Go `resp.Body.Close()` 的资源动作，默认实现为空。
- `HttpResponse`：保存 `status_code`、可读状态文本、已物化的 `body` 与 `request_url`。与 Go 的流式 body 不同，Rust 边界进入本文件前已把响应体表示为字节数组。
- `StatusUrl`：保存 scheme、authority、path、query、fragment。`parse`、`hostname`、`port`、`set_host`、`join_path` 和 `Display` 支撑地址修正；字段私有，调用方不能绕开规范化方法直接改写。
- `StoreBehavior::{ErrorOnTiFlash, SkipTiFlash, TiFlashOnly}`：分别表示遇到 TiFlash 时失败、跳过 TiFlash、仅保留 TiFlash。判定依据是 store label `engine=tiflash`，由 `is_store_tiflash` 经 `astersql_util_engine::IsTiFlash` 完成。
- `StoreMeta::GetAllStores`：PD store 列举抽象；布尔参数在本文件固定传 `true`，表示排除 tombstone。注释明确结果可能随后过期，缓存和变更跟踪属于调用方责任。
- `PdClient::GetTS`：PD TSO 来源抽象，返回 `(physical, logical)`。
- `GetAllTiKVStores` / `GetAllTiKVStoresWithRetry`：公开的节点过滤入口。
- `GetCurrentTsFromPD` / `GetCurrentTsFromPDWithRetry`：公开的当前 TSO 获取入口。
- `GetConfigFromTiKVStores` / `GetConfigBytesFromTiKVStores`：公开的 TiKV 配置访问入口，后者是前者的 HTTP 状态与 body 适配层。
- `HandleTiKVAddress`：公开的 status URL 构造入口。
- `with_retry`：私有通用循环，使用 `BackoffStrategy::RemainingAttempts` 和 `NextBackoff`；失败通过 `astersql_errors::Join` 聚合。

## 执行流程

节点筛选从 `GetAllTiKVStores` 开始：先调用 `GetAllStores(ctx, true)`；随后逐个把 protobuf label 临时映射为 `EngineLabel`，交给 `IsTiFlash`。`SkipTiFlash` 直接跳过 TiFlash，`ErrorOnTiFlash` 以 `ErrPDInvalidResponse` 为根因并附上 store id/address，`TiFlashOnly` 则丢弃所有非 TiFlash，其他节点按原顺序进入结果。重试版本把整个列举与过滤动作放进 `with_retry`，成功后返回最后一次写入的结果向量。

时间戳路径先由 `PdClient::GetTS` 取得两部分，再执行 `((physical as u64) << 18).wrapping_add(logical as u64)`。使用加法而不是按位或是刻意对齐 Go `oracle.ComposeTS`；`util_test.rs::compose_timestamp_uses_go_oracle_addition_semantics` 用 `logical == 1 << 18` 固定了进位行为。重试版本每次尝试前递增计数，单次失败写 Warn；退避耗尽或取消后写 Error 并返回汇聚后的错误。

配置访问按 store 顺序串行进行。非 `Up` store 被跳过；`HandleTiKVAddress` 得到基准 URL 后，`join_path("config")` 清理重复分隔符和 `.`/`..`，同时恢复 query/fragment。每次尝试调用 `HttpClient::Get`，再调用用户 callback，随后无论 callback 成功还是失败都执行 `CloseResponse`。只有 callback 成功才结束该节点的重试；任一节点最终失败会立即终止，不再访问后续节点。字节版本先拒绝非 200 响应，再把 `resp.body` 交给 `collect`。

地址处理先为不以 `http` 开头的 status/node 地址拼接 `http_prefix`，再分别解析。默认返回 status URL；hostname 不一致时，`join_host_port` 以 node hostname 和 status port 重建 authority（IPv6 会加方括号），保留 status URL 的 scheme、path、query、fragment，并记录 Warn。

## 数据与状态

本文件不持有跨调用的全局可变状态。`TS_LOGICAL_BITS = 18` 是唯一模块常量，规定 TSO logical 位宽。节点过滤结果是新分配的 `Vec<Store>`；容量按 PD 返回数量预留，store 被移动进结果，顺序稳定。

`StatusUrl` 是解析后的拥有型快照，所有组成部分均为 `String`。`join_path` 不修改自身；它构造新字符串并通过 segment 栈清理路径。空段和 `.` 被忽略，`..` 弹出前一段，即使越过根也只会对空栈执行无效果的 `pop`。

重试循环的局部状态包括 `all_errors: Vec<Option<SharedError>>`、具体 backoff 对象以及公开包装函数捕获的最后结果。TS 重试另有 `retry` 计数；配置访问的 callback 是 `FnMut`，因此可以积累跨节点状态，但该状态由调用方拥有。`EngineLabel` 与 `EngineLabelSlice` 只是把 protobuf label 临时适配到 `astersql-util-engine` 的 `Label`/`LabelStore` trait，生命周期不会超出一次判定。

## 依赖与调用关系

Cargo 清单表明该 crate 直接依赖 `astersql-br-pkg-errors`、`astersql-br-pkg-logutil`、`astersql-br-pkg-utils`、`astersql-util-engine`、`astersql-errors` 与 `tracing`；本文件实际使用前五者以及 crate 内的 `kvproto::metapb`。`NewAggressivePDBackoffStrategy` 决定尝试次数和退避时长，`astersql-errors` 提供 `Trace`、`Annotatef`、`Errorf`、`Join` 与共享错误类型，`logutil` 提供结构化 Warn/Error 字段。

RustCodeGraph 给出的关键内部调用边为：`GetAllTiKVStoresWithRetry -> GetAllTiKVStores -> StoreMeta::GetAllStores/is_store_tiflash`，`GetCurrentTsFromPDWithRetry -> GetCurrentTsFromPD -> PdClient::GetTS/compose_ts`，`GetConfigBytesFromTiKVStores -> GetConfigFromTiKVStores -> HandleTiKVAddress/HttpClient::Get/CloseResponse`。上游方面，图索引记录 `br/pkg/restore/import_mode_switcher.rs::switchTiKVMode` 调用 `GetAllTiKVStores`，`br/pkg/restore/snap_client/client.rs::{LoadSchemaIfNeededAndInitClient,setSpeedLimitForTask}` 调用重试版，`br/pkg/utils/storewatch/watching.rs::Step` 依赖 store 列举语义；`br/pkg/conn/conn.rs` 则包含公开包装与平行逻辑，不能视为本文件的单纯下游。

## 错误处理与边界

PD store/TS 错误通过 `trace_err` 增加 Trace；TiFlash 禁止策略用 `ErrPDInvalidResponse` 作为可识别根因，并附加具体 store。status address 为空会立即返回包含 store id 的错误；URL 缺少 `://` 由 `StatusUrl::parse` 拒绝。解析器是为当前 BR 边界实现的精简解析器，不等价于通用 URL 标准库：例如它不校验 scheme/port 的完整合法性。

`with_retry` 收集每次错误；尝试失败后若上下文已取消，或退避睡眠期间观察到取消，返回已收集错误的 `Join`。因此有既有业务错误时，返回值不一定包含独立的 `RetryCancelled` 文本；`RetryCancelled` 仅是错误列表意外为空时的兜底。尝试次数一开始就是零时返回 `EmptyRetryErrors`。睡眠使用 1ms 上限的轮询片段，以换取取消响应性。

配置路径只跳过非 `Up` 节点，不自行过滤 TiFlash；调用者若只想查询 TiKV，必须先传入正确筛选过的 store。非 200 响应是字节包装层的错误；基础 `GetConfigFromTiKVStores` 把所有 HTTP 语义交给 callback。callback 失败也会被当前重试策略视为可重试失败，所以 callback 若有非幂等副作用，可能执行多次。请求失败发生在获得响应前时没有可关闭对象；获得响应后 `CloseResponse` 总在 callback 返回后执行。

TS 组合先把有符号值转换为 `u64`，再移位和 wrapping add；本文件不验证 physical/logical 为非负或 logical 是否小于 `2^18`，这是 PD 输入契约。地址 hostname 不一致且 status URL 不含端口时会生成带空端口的 `host:`；当前测试未覆盖这一输入，扩展时不应擅自声称与 `net.JoinHostPort` 的所有边界完全等价。

## 并发与资源生命周期

公开 trait 均不借用异步 runtime：调用过程是同步且阻塞的。`StoreMeta`、`PdClient`、`HttpClient` 要求 `Send + Sync`，允许实现被跨线程共享，但本文件自身不会并发发起 PD/HTTP 请求；store 逐个处理，单个 store 的重试也逐次执行。`CancelContext` 没有 `Send + Sync` 上界，当前调用期间只以共享引用同步读取。

重试等待直接使用 `std::thread::sleep`，每 1ms 左右检查取消；它占用当前工作线程，不适合直接放进要求非阻塞的异步 executor。HTTP 响应由 `HttpClient` 实现创建，`GetConfigFromTiKVStores` 在 callback 完成后调用 `CloseResponse`。`parity_test.rs::config_response_is_closed_after_callback_failure` 明确保护 callback 失败时仍关闭一次；默认 `CloseResponse` 是空实现，所以真实资源释放仍依赖具体客户端正确覆盖该方法。

store 和 URL 都按值拥有，不留下悬垂借用。PD 返回的 store 只是瞬时快照；`StoreMeta` 注释明确调用方若缓存它们，必须自行处理节点过期和变更。本文件没有锁、通道、后台任务、事务或显式 drop 顺序。

## 与 Go 版本的对应关系

Rust 的公开枚举、trait 和函数名直接镜像 `br/pkg/conn/util/util.go` 的 `StoreBehavior`、`StoreMeta`、`GetAllTiKVStores*`、`GetCurrentTsFromPD*`、`GetConfig*` 与 `HandleTiKVAddress`。store 三种策略、排除 tombstone、TSO 18 位组合、只访问 Up 节点、逐节点失败即停止、非 200 错误文本以及 hostname 修正原则均保持一致。

主要表示差异是：Go 直接依赖 `context.Context`、`pd.Client`、`*http.Client`、`*http.Response` 和 `*url.URL`，Rust 为 darwin/精简依赖边界定义本地 trait 与结构；Go 的 body 通过 `io.ReadAll` 流式读取，Rust `HttpResponse.body` 已是 `Vec<u8>`；Go 用 `defer resp.Body.Close()`，Rust 显式调用 `CloseResponse`；Go 原地压缩 store slice，Rust 构造新向量。Go `url.URL.JoinPath`/`net.JoinHostPort` 由标准库实现，Rust 是局部等价实现，已由 `util_test.rs::join_path_cleans_dot_segments_like_go_url_join_path` 和 parity 测试覆盖关键路径，但并非完整 URL 兼容层。

重试行为由两端各自的 `NewAggressivePDBackoffStrategy` 驱动。Rust 的私有 `with_retry` 会将历次错误 `Join`，并在退避期间主动轮询取消；`parity_test.rs::cancellation_during_backoff_stops_before_another_pd_call` 固定了取消后不再调用 PD。`br/pkg/conn/conn.rs` 同时保留另一份 store/地址/配置逻辑，说明移植尚有重复边界；修改本文件时必须检查该平行实现是否也需要同步，而不能默认 crate re-export 已覆盖所有调用者。

## 扩展指南

- 增加 store 分类策略时，应修改 `StoreBehavior` 与 `GetAllTiKVStores` 的单次判定，并在独立的 `br/pkg/conn/util/parity_test.rs` 增加普通、空列表、混合 label、错误根因和顺序断言；同时核对 Go `util.go` 以及 `br/pkg/conn/conn.rs` 的平行策略。不要把测试内嵌进生产文件。
- 调整 TSO 组合时，应保留 `compose_timestamp_uses_go_oracle_addition_semantics` 所证明的加法/进位语义，并补充极值、负输入契约或溢出决策；若目标是更严格校验，先确认 Go `oracle.ComposeTS` 的兼容要求。
- 扩展 URL 行为应优先修改 `StatusUrl::{parse,hostname,port,join_path}` 与 `join_host_port`，补充 IPv6、缺省端口、query/fragment、转义字符和越根 `..` 的独立测试。该类型当前不是通用 URL 解析器，新增承诺会扩大兼容面。
- 改动重试应从 `with_retry` 与 `BackoffStrategy` 的契约入手，并覆盖首次成功、耗尽、取消前/退避中取消、错误聚合以及 callback 多次执行。避免在 callback 非幂等的前提下静默扩大重试范围。
- 接入真实 HTTP 客户端时必须覆盖 `CloseResponse`，并验证成功、HTTP 状态失败、collector 失败和重试各路径的释放次数。若未来改为流式 body，需重新定义读取错误和关闭错误的优先级。
- 性能敏感点主要是串行网络访问、1ms 取消轮询、每个 store label 的临时分配以及响应体整体驻留内存；优化时必须保留逐节点错误短路、原顺序和 Go 可观察行为。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/conn/util` 确认 crate 的 `lib.rs`、`util.rs`、`util_test.rs`、`parity_test.rs`、`stubs.rs` 和 Go 对照；`node --file br/pkg/conn/util/util.rs` 读取全部 688 行并列出 58 个符号；`explore` 核对了公开入口的调用链与恢复、连接、storewatch 调用者。精确 `callers` 命令在当前环境未返回可用输出，因此调用关系采用 `explore` 结果并以局部 `rg` 复核。
- 源与边界：`br/pkg/conn/util/util.rs`、`br/pkg/conn/util/lib.rs`、`br/pkg/conn/util/Cargo.toml`；上游/平行实现证据为 `br/pkg/conn/conn.rs`、`br/pkg/restore/import_mode_switcher.rs`、`br/pkg/restore/snap_client/client.rs` 与 `br/pkg/utils/storewatch/watching.rs`。
- Go 对照：`br/pkg/conn/util/util.go`，逐项核对 store 策略、PD TSO、HTTP `/config`、response close 与 status hostname 修正。
- 独立 Rust 测试：`br/pkg/conn/util/util_test.rs` 验证路径清理与 TSO 加法；`br/pkg/conn/util/parity_test.rs` 验证 TiFlash 三策略、错误根因、HTTP 状态文本、配置 body、callback 失败后关闭、退避取消以及 path/query 在 host 改写后保留。该目录没有 `util_test.go`，Go 语义以实现文件和 Rust parity 测试交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工检查本文没有把测试桩、平行实现或未覆盖 URL 边界写成已完整支持。
