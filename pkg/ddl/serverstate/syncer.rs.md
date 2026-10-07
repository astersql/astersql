# `pkg/ddl/serverstate/syncer.rs`

## 文件定位

本文件属于 `astersql-ddl-serverstate` crate，是 DDL 子系统感知“集群正常运行/升级中”全局状态的协议与协调存储实现。crate 入口 `pkg/ddl/serverstate/lib.rs` 同时导出本文件和 `mem_syncer.rs`；`pkg/ddl/serverstate/Cargo.toml` 声明其 Go 对照包为 `pkg/ddl/serverstate`，当前唯一常规 Rust 依赖是同级 `astersql-ddl-schemaver`，用于公共 etcd 客户端、会话和可取消上下文。

它不负责执行 DDL job、schema state transition 或 backfill。它提供的状态会被普通 DDL 调度策略读取：`pkg/session/runtime/normal_ddl_service.rs` 的 `UpgradeState::sync` 消费 watch、刷新状态并发布 owner operation，`pkg/ddl/normal_policy.rs` 的 `NormalDdlJobPolicy::runnable` 据升级快照决定暂停或恢复普通 DDL。跨 keyspace 会话创建路径则在 `pkg/session/runtime/session_factory.rs` 用 `EtcdSyncer::with_client` 同步读取状态，并把缓存通过 `JobSubmitServerState` 提供给 job submit。

## 核心职责

1. 用 `Syncer` trait 统一初始化、写入、读取、本地升级态判断、watch 获取和重新订阅六类操作。
2. 用 `StateInfo` 将协议载荷稳定为 `{"state":"..."}`，并兼容 Go `encoding/json` 的关键读取语义：字段名大小写不敏感、`null`/缺失字段得到默认空串、未知但合法的 JSON 字段被忽略、Unicode 代理对可组合。
3. 用 `EtcdSyncer` 在同一套缓存、重试和解码逻辑之下支持两种后端：`StateStore` 进程内后端用于确定性测试，`astersql_ddl_schemaver::EtcdClient` 后端用于实际协调服务接线。
4. 将远端写入通知与本地缓存刷新解耦。`update_global_state` 只写后端；`is_upgrading_state` 只读本地缓存；只有 `init` 或 `get_global_state` 会刷新缓存。调用者收到 watch 后必须再读取状态。
5. 管理 watch 和 etcd session 生命周期，使 rewatch 能替换旧订阅，析构同步器时能取消内部 watch 并关闭后端 session，而不取消调用者传入的上下文。

## 主要符号

- 常量：`KEY_OP_DEFAULT_RETRY_COUNT = 3`、`KEY_OP_DEFAULT_TIMEOUT = 1s`、`KEY_OP_RETRY_INTERVAL = 30ms` 定义键操作策略；`STATE_PROMPT` 参与会话标识；`STATE_UPGRADING = "upgrading"` 与 `STATE_NORMAL_RUNNING = ""` 定义当前协议状态值。
- `SyncError`：区分取消、超时、后端错误、非法状态 JSON、键数量异常、未初始化和 watch 关闭。其 `Display` 给上层提供稳定的人类可读错误。
- `SyncContext`：共享一个原子取消标志和 schemaver transport context，并可附加 deadline。`with_timeout` 取父 deadline 与新 deadline 的较早者；`cancel` 同时取消本地标志和 transport。
- `StateInfo`：唯一协议字段是公开的 `state: String`。`marshal` 手工产生 JSON；`unmarshal` 借助私有 `JsonCursor` 解析并跳过未知值。
- `WatchResponse`、`WatchChannel`、`Watcher`：分别表示状态键事件、共享接收端和可替换接收端容器。`WatchChannel` 支持阻塞或限时接收。
- `Syncer: Send + Sync`：供服务层以 `Arc<dyn Syncer>` 注入；公开契约包括 `init`、`update_global_state`、`get_global_state`、`is_upgrading_state`、`watch_chan` 和 `rewatch`。
- `StateStore`：公开的进程内 etcd 风格测试后端。它保存键的值列表、watcher 列表和故障注入队列；`fail_next`、`set_raw_values` 用于构造重试及多值异常。
- `EtcdSyncer`：正式同步器。`new` 连接 `StateStore`，`with_client` 连接公共 `EtcdClient`；字段分别保存路径、提示串、后端、会话建立标志、本地状态缓存、watcher 和当前内部 watch context。
- `new_etcd_syncer`：将内存后端实现装箱为 `Arc<dyn Syncer>`，主要供 `syncer_test.rs` 使用。生产接线直接使用 `EtcdSyncer::with_client`。
- `StateBackend`：私有后端枚举，封装 `create_session/get/put/watch` 差异；其 `Drop` 负责关闭公共 etcd session。

## 执行流程

初始化流程由 `EtcdSyncer::init` 驱动：先用 `StateBackend::create_session` 建立会话，成功后标记 `session_ready`；随后调用 `get_global_state`，以最多三次、每次一秒子超时读取状态键并刷新 `cluster_state`；最后 `start_watch` 创建独立子上下文、取消并替换旧 watch、将新接收端装入 `Watcher`。`session_ready` 当前只写不读，不承担接口前置条件判断。

写入流程由 `update_global_state` 驱动：先用 `StateInfo::marshal` 编码，再由 `put_key_value` 最多尝试三次。每次派生一秒子超时；失败后按 30ms 间隔重试。内存后端覆盖该键为单值并同步向匹配 watcher 发事件；公共 etcd 后端调用 `EtcdClient::Put`。成功写入不会修改 `cluster_state`。

读取流程由 `get_global_state` 驱动：`get_key_value` 在每次失败后等待 200ms，最多读取三次。零个值映射为默认正常态；恰好一个值交给 `StateInfo::unmarshal`；多个值返回 `WrongKeyCount`。只有获得有效状态后才替换 `cluster_state`，因此解析失败不会污染已有缓存。

消费流程可在 `UpgradeState::sync` 中看到：先非阻塞读取 `watch_chan`；收到事件则刷新，超时表示无变化，其他 watch 错误触发 `rewatch`；需要刷新时调用带三秒 timeout 的 `get_global_state`，再把升级态发布为 owner operation。该流程证明 watch 事件只充当“重新读取”信号，事件载荷不是调度决策的唯一事实来源。

跨 keyspace 流程由 `session_factory.rs` 展示：构造 `EtcdSyncer::with_client` 后可以在没有 `init` 的情况下直接 `get_global_state`，从而不分配 lease、不启动 watch；后续刷新闭包再次显式读取并更新缓存。

## 数据与状态

协调键由构造参数 `etcd_path` 决定；生产调用传入 `astersql_ddl_util::ServerGlobalState`。协议载荷只有 `state` 字符串，空串表示正常运行，`"upgrading"` 表示升级中。trait 并未禁止其他字符串，但 `is_upgrading_state` 只对精确的 `STATE_UPGRADING` 返回 true。

`cluster_state: RwLock<Arc<StateInfo>>` 是每个 `EtcdSyncer` 实例的只读快照缓存，初始为正常态。远端写入、watch 到达和事件载荷本身都不更新它；成功的 `get_global_state` 才更新。这个滞后是接口语义而非异常，Rust 测试 `etcd_syncer_matches_go_state_watch_and_cache_semantics` 和 Go 测试 `TestStateSyncerSimple` 都检查了读取前后的差异。

`StateStore` 在一个 `Mutex<StoreState>` 中保存 `HashMap<String, Vec<Vec<u8>>>`、活跃订阅和 FIFO 故障队列。正常 `put` 把目标键规范化为单值；`set_raw_values` 能构造多值以验证 `WrongKeyCount`。`AtomicU64` 只负责分配 watcher ID。

`StateInfo::unmarshal` 会保留最后出现的匹配字段，缺失字段默认空串。未知字段仍必须是合法 JSON；对象、数组、布尔、null 和标准 JSON 数字均可跳过，尾随垃圾和非法转义会得到 `InvalidState`。

## 依赖与调用关系

上游生产调用主要有三类：

- `pkg/session/runtime/normal_ddl_service.rs` 持有 `Arc<dyn Syncer>`，在 owner 调度轮次消费 watch、rewatch、读取全局状态并生成稳定的升级快照。
- `pkg/ddl/normal_policy.rs` 从共享缓存或调度轮次快照判断 job 是否可运行；`pkg/session/runtime/system_session.rs` 的 `JobSubmitServerState` 也把 `is_upgrading_state` 适配成 jobsubmit 所需接口。
- `pkg/session/runtime/session_factory.rs` 通过 `EtcdSyncer::with_client` 构造普通和跨 keyspace 状态同步器；跨 keyspace 路径明确支持“不 init，只按需 get”的协议。

下游依赖只有标准库并发/时间设施和 `astersql-ddl-schemaver`。公共 etcd 后端调用其 `Context`、`EtcdClient::{NewSession,Get,Put,Watch}`、`Session::Close` 和 `SessionTTL`；内存后端不访问网络。

RustCodeGraph 将 `syncer.rs` 标为被 `pkg/ddl/normal_policy.rs`、`pkg/ddl/schemaver/syncer.rs`、`pkg/ddl/serverstate/syncer_test.rs` 等 12 个文件使用。精确 trait 方法的 `callers` 查询未输出边，因此生产调用关系以上述 RustCodeGraph 文件片段和 `rg` 命中为直接证据，不扩展声称未核验的调用者。

## 错误处理与边界

上下文检查优先于后端操作。父上下文已取消时返回 `Cancelled`；本地 deadline 到期返回 `Timeout`。`with_timeout` 不延长父 deadline。需要注意，重试之间的 `thread::sleep` 本身不可被即时打断，错误只会在下一轮开始时重新检查。

读写重试都保留最后一次错误；重试次数为零时才会生成兜底 `Backend` 文本。公共 etcd 错误统一转成字符串形式的 `Backend`，因此此层不暴露传输库的具体错误类型。会话创建也最多三次，间隔 200ms。

读取零值是合法情况，等价于默认正常态；多值是不变量破坏，返回 `WrongKeyCount`。非法 UTF-8、非法 JSON、尾随内容、非法未知字段值均返回 `InvalidState`。`StateBackend::put` 还会拒绝无法转成 UTF-8 的字节，不过当前 `StateInfo::marshal` 总会生成 UTF-8。

`WatchChannel::recv_timeout` 明确区分超时与断开；生产调用把非超时错误当作需要 rewatch。内存 watch 发送失败时会移除失效订阅；公共 watch 收到 transport error 或 compact revision 后结束转发线程，交由上层重新订阅。

锁中毒使用 `unwrap`，因此持锁线程 panic 会导致后续访问 panic，而不是 `SyncError`。本文件没有恢复锁中毒的策略。`session_ready` 不被读取，故 `get_global_state`/`update_global_state` 可在 `init` 前用于公共客户端；反之，`watch_chan` 在 init/rewatch 前指向一个已断开的默认通道。

## 并发与资源生命周期

`Syncer` 要求 `Send + Sync`，实例通常放入 `Arc`。状态缓存由 `RwLock` 保护；后端数据、session 和 watch context 分别由 `Mutex` 保护；取消标志和 watcher ID 使用原子量。`WatchChannel` 的 receiver 也位于共享 `Mutex` 中，所以克隆的 channel 是竞争消费同一事件流，并非广播给每个克隆者。

`start_watch` 为每次订阅创建独立子 `SyncContext`，替换前取消旧上下文，避免 rewatch 后旧订阅继续存活。它不复用调用者的 `cancelled` 原子标志，只通过 transport child 和继承 deadline 关联父上下文，因此关闭或 rewatch 不会调用 `ctx.cancel()` 影响外层；`crossks_align_schema_protocol_public_client_preserves_init_and_watch` 专门验证 drop 后调用者 context 仍有效。

内存 `StateStore::watch` 为每个订阅创建一个清理线程，每 10ms 检查 context，结束后按 ID 移除 watcher。公共 etcd watch 创建转发线程，每 20ms 轮询 transport channel，把每个 event 转换为本地 `WatchResponse`。接收端消失、transport 断开、错误响应、compact revision 或 context 结束都会终止该线程。

`EtcdSyncer::drop` 取消当前内部 watch。字段继续析构时，`StateBackend::drop` 关闭可能存在的 etcd session。内存后端没有 session 资源；其 watcher 清理由各自线程完成。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/serverstate/syncer.go`，测试是 `pkg/ddl/serverstate/syncer_test.go`。Rust 保留了 Go 的 `Syncer` 六方法契约、状态常量、`StateInfo` JSON 载荷、三次读写重试、一秒单次超时、读失败后 200ms 等待、零值默认态、多值报错，以及“收到 watch 后显式 Get 才刷新本地缓存”的核心语义。

Rust 的 `EtcdSyncer` 比 Go `etcdSyncer` 多一层 `StateBackend`，使相同状态机可对接内存测试存储或 `astersql-ddl-schemaver::EtcdClient`。`with_client` 还支持跨 keyspace 的无 session、无 watch 直接读写；对应 Rust 测试以一个在 `NewSession`/`Watch` 时 panic 的边界客户端证明该路径。

JSON 方面，Go 直接使用 `encoding/json`；Rust 以 `JsonCursor` 实现任务所需子集，并用独立测试覆盖 Unicode 代理对、大小写字段名、null 和未知字段合法性。扩展协议字段时必须继续与 Go 解码行为对照，不能假定手写解析器天然覆盖 `encoding/json` 的所有边界。

当前差异包括：Go 在初始化、读取和写入路径记录 metrics，并用 DDL logger 记录失败/非法值；Rust 文件没有对应指标或结构化日志。Go 直接持有 `clientv3.Client`、`concurrency.Session` 与 `util.Watcher`；Rust 经 schemaver transport 抽象。Go 写入调用 `util.PutKVToEtcd`，Rust 本地明确使用 30ms 重试间隔。Go 状态缓存是原子指针，Rust 是 `RwLock<Arc<StateInfo>>`。

## 扩展指南

新增状态值时，优先保持 `StateInfo` 载荷和空串正常态兼容，并检查所有精确比较 `STATE_UPGRADING` 的调用点；若新状态也应暂停 DDL，仅新增常量而不修改 `is_upgrading_state` 和调度策略是不完整的。应同步扩展独立的 `pkg/ddl/serverstate/syncer_test.rs`，并与 `syncer_test.go` 的期望对照，不要把测试嵌入生产文件。

新增协议字段或 JSON 类型时，应修改 `StateInfo` 及 `JsonCursor`，重点验证未知字段、重复字段、null、Unicode、非法 JSON 和前后兼容。手写解析器属于高风险兼容边界，测试必须覆盖 Go `encoding/json` 的对应行为。

修改重试策略时，应同时审查 `get_key_value`、`put_key_value` 和 `StateBackend::create_session`：三者当前间隔并不相同，而且 sleep 不可取消。性能风险主要是同步阻塞和线程数量；若改成异步实现，还需保持 `Syncer: Send + Sync` 以及现有调用方的同步契约。

修改 watch 时，应保持“事件提示刷新、缓存由 get 更新”的不变量，明确 cloned `WatchChannel` 的竞争消费语义，并验证 rewatch 会终止旧订阅、drop 不取消调用者 context、compact/error 会让上层有机会重订阅。生产接线的回归点包括 `UpgradeState::sync`、`NormalDdlJobPolicy` 和跨 keyspace `session_factory`。

若补齐 Go 已有的 metrics/logger，应通过 crate 依赖正式接线；`Cargo.toml` 中位于 `cfg(any())` 的 logutil/util/metrics 依赖当前永不启用，不能把它们视为现有运行时能力。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引含 11,467 个文件；`node --file pkg/ddl/serverstate/syncer.rs` 分段读取了 1–911 行，核对全部常量、类型、trait、impl、线程和 Drop；`files --filter pkg/ddl/serverstate` 核对模块文件集合；`explore` 给出 `get_global_state`、`update_global_state` 等调用影响以及相关测试。精确 trait `callers/callees` 查询无文本输出，因此未将其当作唯一证据。
- 源码与 crate：`pkg/ddl/serverstate/syncer.rs`、`pkg/ddl/serverstate/lib.rs`、`pkg/ddl/serverstate/Cargo.toml`。
- Rust 生产调用：`pkg/session/runtime/normal_ddl_service.rs`（watch/rewatch/get 与 owner 快照）、`pkg/session/runtime/session_factory.rs`（公共客户端和跨 keyspace 直接读取）、`pkg/session/runtime/system_session.rs`（jobsubmit 缓存适配）、`pkg/ddl/normal_policy.rs`（升级期 admission）。
- 独立 Rust 测试：`pkg/ddl/serverstate/syncer_test.rs` 覆盖内存同步器、watch/缓存不变量、JSON 兼容、无 lease/watch 的跨 keyspace 读取、公共客户端初始化/rewatch/drop。
- Go 对照：`pkg/ddl/serverstate/syncer.go` 和 `pkg/ddl/serverstate/syncer_test.go`，用于核对接口、重试、缓存刷新和真实单节点 etcd watch 语义。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在，且固定的 11 个二级标题各出现一次；交付前另以该命令的退出码确认。
