# `pkg/store/copr/mpp_probe.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate。crate 入口 `pkg/store/copr/lib.rs` 通过 `pub mod mpp_probe` 声明模块，并以 `pub use mpp_probe::*` 向 crate 使用者重新导出其公开项；`pkg/store/copr/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/store/copr`。文件实现两组进程内设施：失败 MPP/TiFlash store 的恢复探测状态机，以及按地址保存 MPP server 信息的有界 LRU 缓存。

当前 Rust 仓库中的直接使用范围需要与 Go 主链区分：RustCodeGraph 对 `global_mpp_failed_store_prober`、`global_mpp_server_info_manager` 的查询没有找到模块外生产调用者，按符号名补充检索也只找到 `pkg/store/copr/mpp_probe_test.rs` 对具体类型和函数的直接使用。因此这些 Rust API 已实现并被 crate 导出，但尚不能据此声称已经接入 Rust MPP 请求主链。Go 对照文件 `pkg/store/copr/mpp_probe.go` 则由 `batch_coprocessor.go`、planner optimizer 和进程启动/停止流程实际使用。

## 核心职责

1. `MppStoreState` 为一个已判定失败的地址保存探活客户端和三个时间点，并将探测结果转换为“持续恢复达到 TTL”这一业务判断。
2. `MppFailedStoreProber` 管理地址到状态的映射，可手工执行一轮 `scan`，也可用唯一后台 worker 周期执行；扫描同时回收已恢复过久或失败后长期无人查询的条目。
3. `MppServerInfoManager` 保存 `MppServerInfo`，在每次读取/写入时更新最近使用顺序，并将条目数限制在给定容量内。
4. 两个 `OnceLock` 访问器提供惰性初始化的进程级实例；Go 风格常量和类型别名降低同路径移植代码的命名差异。

本文件只把一次探活抽象为同步布尔结果，不负责构造 TiFlash `CmdMPPAlive` RPC、记录指标或输出日志；这些能力存在于 Go 对照实现，但没有出现在当前 Rust 文件中。

## 主要符号

- 常量 `DETECT_PERIOD`（3 秒）、`DETECT_TIMEOUT_LIMIT`（2 秒）、`MAX_RECOVERY_TIME_LIMIT`（15 分钟）、`MAX_OBSOLETE_TIME_LIMIT`（1 小时）和 `MPP_SERVER_INFO_MANAGER_CACHE_SIZE`（1000）定义默认节奏与内存上限。`DetectPeriod` 等四个别名保留 Go 风格名称，其中 `MaxObsoletTimeLimit` 也保留 Go 原有拼写。
- trait `MppAliveClient: Send + Sync + 'static` 是探活边界，唯一方法 `is_alive(&self, address, timeout) -> bool` 将传输错误、不可用响应等统一压缩为存活布尔值；`Arc<dyn MppAliveClient>` 允许状态与后台线程安全共享客户端。
- 私有 `StoreTiming` 保存 `recovery_time: Option<Instant>`、`last_lookup_time: Instant` 和 `last_detect_time: Option<Instant>`。`None` 的恢复时间表示当前仍失败。
- `MppStoreState` 保存公开地址、客户端和受 `Mutex` 保护的时间状态。`new` 仅由 `MppFailedStoreProber::add` 使用；`detect` 限流并更新探测结果；`is_recovered` 刷新查询时间并判断恢复持续时间是否严格大于调用方 TTL。
- `MppServerInfo` 是可克隆的值对象，字段为地址、逻辑 CPU 数和启动时间戳。私有 `ServerInfoState` 以 `HashMap` 保存值、以 `VecDeque` 保存从旧到新的访问顺序。
- `MppServerInfoManager::{new, add, delete, get}` 实现有界 LRU。`Default` 使用容量 1000；`get` 返回克隆值并把命中地址移到队尾。
- `MppFailedStoreProber::{add, is_recovered, delete, scan, run, stop}` 是公开控制面；私有 `scan_shared` 同时服务手工扫描和后台线程。其 `Drop` 实现调用 `stop`，避免由普通局部实例启动的线程遗留。
- `global_mpp_failed_store_prober` 与 `global_mpp_server_info_manager` 分别惰性初始化两个全局对象；`detect_mpp_store` 是对 trait 调用的一次性薄封装。
- `MPPStoreState`、`MPPFailedStoreProber`、`MPPServerInfo` 是对应 Rust 类型的 Go 风格公开别名，不创建新类型或新状态。

## 执行流程

失败 store 的典型状态流程如下：

1. 调用方在同步请求失败后调用 `MppFailedStoreProber::add(address, client)`。同地址再次加入会替换旧的 `Arc<MppStoreState>`，并将恢复、查询和探测时间全部重新初始化。
2. `scan` 或 `run` 启动的 worker 调用 `scan_shared`。函数先在映射锁内克隆所有状态的 `Arc`，随即释放映射锁，避免在实际探活期间长期阻塞增删查。
3. 对每个快照项调用 `MppStoreState::detect`。若状态锁正被占用或距上次完成探测不足 `detect_period`，本轮跳过；否则在持有状态锁时调用客户端，并在 RPC/替身返回后记录 `last_detect_time`。
4. 探活成功时，仅在首次连续成功时设置 `recovery_time`；后续成功保留原时间，从而累积稳定恢复时长。任何失败都会把它重置为 `None`，重新开始恢复窗口。
5. 探测后，`scan_shared` 再尝试读取时间状态。恢复时间超过 `max_recovery_time_limit`，或者仍失败且距最后业务查询超过 `max_obsolete_time_limit`，都会把地址放入待删除列表；最后一次性重新获取映射锁并删除。
6. 业务侧调用 `is_recovered(address, ttl)`：地址不存在直接视为恢复；地址存在则尝试状态锁，成功后刷新 `last_lookup_time`，只有连续恢复时间严格大于 `ttl` 才返回 `true`。

后台路径中，`run` 用 `running.compare_exchange(false, true, ...)` 保证只有一个 worker，复制配置后生成线程。线程循环每轮扫描一次，再分十次各睡眠 100 毫秒，以便停止请求最多约 100 毫秒即可被观察。`stop` 设置停止标志、取出句柄并 `join`；线程退出前将 `running` 复位。

server 信息路径独立于探活路径：`add` 去掉地址的旧顺序记录、追加到队尾、覆盖映射值，并从队首淘汰直至容量满足；`get` 命中后同样把地址提升到队尾；`delete` 同时清理映射和顺序队列。

## 数据与状态

- 时间统一使用单调时钟 `Instant`，不会把墙钟跳变引入 TTL 判断。`last_detect_time` 记录的是客户端调用完成时刻；`pkg/store/copr/mpp_probe_test.rs` 的 `failed_store_probe_detect_period_starts_after_request_completes` 明确验证慢探活不会提前消耗下一周期。
- `recovery_time` 表示当前连续成功区间的起点，而非最近一次成功；一次失败即清空。`is_recovered` 和清理逻辑均以此计算持续恢复时间，但使用不同阈值。
- `last_lookup_time` 在条目创建时设置，并且只有成功取得状态锁的 `is_recovered` 才刷新。扫描或探活本身不会把无人使用的失败地址永久续期。
- `stores` 的所有权结构为 `Arc<Mutex<HashMap<String, Arc<MppStoreState>>>>`：外层 `Arc` 供 worker 持有，内层状态 `Arc` 让扫描快照脱离映射锁后仍保持有效。
- LRU 的正确性依赖 `ServerInfoState.values` 与 `order` 同步更新。现有实现每次操作都在同一个互斥锁内完成，并在添加前用 `retain` 去重，因此同一地址正常情况下在顺序队列中只出现一次。
- 容量为 0 是被实现允许的边界：`add` 插入后立即从队首删除，最终保持空缓存。

## 依赖与调用关系

本文件的实现只使用 Rust 标准库：`HashMap`/`VecDeque`、`Arc`/`Mutex`/`OnceLock`、原子布尔、线程句柄以及 `Duration`/`Instant`。虽然所属 crate 在 `Cargo.toml` 中依赖 `tikv-client`、`tokio`、`kvproto` 等组件，本文件并未直接导入它们；真实传输由外部 `MppAliveClient` 实现注入。

模块装配关系是 `pkg/store/copr/lib.rs -> pub mod mpp_probe -> pub use mpp_probe::*`。RustCodeGraph 精确查询确认 `detect_mpp_store` 的可执行调用来自 `pkg/store/copr/mpp_probe_test.rs::failed_store_probe_tracks_failure_and_recovery`；测试还直接调用 prober 的 `add`、`scan`、`is_recovered`、`delete`、`run` 和 `stop`。对两个全局访问器的图查询未发现模块外调用，补充的 Rust 符号检索也未找到当前生产接线，因此扩展文档或代码时应保留这一迁移状态说明。

Go 主链提供未来接线依据，而不是当前 Rust 行为证据：`pkg/store/copr/batch_coprocessor.go` 在请求选择中调用 `GlobalMPPFailedStoreProber.IsRecovery`，失败时调用 `Add`；`pkg/planner/core/optimizer.go` 查询并更新 `GlobalMPPServerInfoManager`；`cmd/tidb-server/main.rs` 中相同 Go 风格表达式来自迁移桩命名，并不能证明它调用了本文件的 snake_case 全局访问器。

## 错误处理与边界

- trait 只返回 `bool`，所以具体 RPC 错误、超时、协议类型错误和“服务尚不可用”无法由本层区分或向上传播。调用者若需要诊断信息，必须在 `MppAliveClient::is_alive` 实现内部记录或扩展接口。
- `MppStoreState::{detect,is_recovered}` 与扫描的第二次状态读取使用 `try_lock`。竞争不是错误：探测直接跳过，恢复查询保守返回 `false`，清理判断留待后续扫描。
- 映射锁、LRU 锁和 worker 句柄锁若中毒会通过 `expect` panic；后台线程体没有 `catch_unwind`。若 `is_alive` panic，worker 会异常退出，且退出尾部的 `running.store(false, ...)` 不会执行，之后的 `run` 可能一直认为 worker 存在。这与 Go `scan` 的 `recover` 行为不同，是扩展时需要特别处理的可靠性边界。
- `stop` 忽略 `JoinHandle::join` 返回的 panic 载荷。它在观察到 `running == false` 时直接返回；正常线程退出会复位标志，但异常线程存在前述状态滞留风险。
- `is_recovered` 将“不在失败列表”定义为 `true`，因此删除后的地址也立即视为恢复；测试 `failed_store_probe_tolerates_unknown_and_deleted_entries` 固化了该契约。
- 比较都使用严格的 `>`。刚好等于恢复 TTL 或清理阈值时尚不满足条件，需要经过更多时间。
- LRU 锁中毒同样 panic；`get` 返回完整克隆，调用方对结果的修改不会回写缓存。

## 并发与资源生命周期

`MppFailedStoreProber` 使用两级锁减少映射级临界区，但单个 `MppStoreState::detect` 会在整个同步 `is_alive` 调用期间持有其时间锁。这保证同一地址不会并发探测，也意味着同地址的 `is_recovered` 在慢请求期间会立即保守失败。不同地址在一次 `scan_shared` 中按快照顺序串行探测；这不同于 Go `scan` 为每个条目启动 goroutine 的并行策略，store 数量或超时增加时可能拉长整轮扫描。

`running` 是 worker 唯一性门闩，`stop` 是退出信号，内存序使用 Acquire/Release/AcqRel。worker 拥有 `stores`、两个原子量及配置值的克隆；配置只在 `run` 时复制，worker 运行期间修改公开时长字段不会影响已经启动的线程，手工 `scan` 则读取最新字段。正常 `stop` 会等待线程退出，`Drop` 也执行同一路径；全局 `OnceLock` 对象通常活到进程结束。

LRU 管理器没有后台资源，所有操作由单个 `Mutex<ServerInfoState>` 串行化。返回 `MppServerInfo` 克隆避免把锁保护的数据引用泄露到临界区外，但大规模高频 `get` 会执行 `VecDeque::retain`，其更新时间复杂度与缓存容量线性相关。

## 与 Go 版本的对应关系

Rust 文件总体复刻 `pkg/store/copr/mpp_probe.go` 的两个职责、默认常量、恢复 TTL 语义、两种过期清理规则和容量 1000 的 LRU。`pkg/store/copr/mpp_probe_test.rs` 中可运行测试覆盖失败到恢复、探测周期从请求完成后计算、LRU 淘汰、后台 worker 单例和未知/已删除条目；同文件还保留不可执行的 `GO_REFERENCE` 字符串，而真正可运行测试位于字符串之后。

关键差异如下：

- Go 状态直接持有 `tikv.Client`，`detectMPPStore` 构造 `CmdMPPAlive`/TiFlash RPC，并处理错误及 `Available`；Rust 通过 `MppAliveClient` 返回布尔值，当前文件没有实际 RPC 适配器。
- Go 扫描逐地址启动 goroutine，Rust 在一个 worker 内串行探测；Go 以 1 秒 ticker 调度，Rust 每轮完成后约等待 1 秒，因而 Rust 的周期包含整轮扫描耗时。
- Go 写指标、日志并在 `scan` 顶层恢复 panic；Rust 当前没有指标、日志或 panic 恢复。
- Go 全局对象在 `init` 中构造且生产链已接线；Rust 使用 `OnceLock` 惰性访问器，但现有生产 Rust 调用边未验证到。
- Go 的 `sync.Map` 可容纳错误动态类型并在扫描时清理；Rust `HashMap<String, Arc<MppStoreState>>` 在类型系统上排除了该类断言失败，所以 Go 的异常值分支无需原样复制。
- Go `Run` 依赖 context/cancel、等待组和互斥锁；Rust 使用原子量、专用线程和 `JoinHandle`。两者都要求重复启动不产生多个后台任务。
- Go 复用通用 `SimpleLRUCache`，Rust 在文件内以 `HashMap + VecDeque` 实现等价容量与访问提升语义。

## 扩展指南

- 接入真实 Rust MPP 请求链时，优先新增一个位于网络/客户端边界的 `MppAliveClient` 实现，使其发送 TiFlash alive 请求，并在 `batch_coprocessor.rs` 的失败与重试决策处显式调用全局 prober；不要把具体网络依赖反向塞进状态机。同步更新独立的 `pkg/store/copr/mpp_probe_test.rs`，并为真实适配器在其所属模块增加独立测试。
- 若要补齐 Go 的可观测性，在 `detect`、`delete`、worker 生命周期和 panic 边界接入已有 metrics/logging crate，同时核对标签删除和心跳指标语义；这些依赖在 `Cargo.toml` 中目前多为 optional，接线可能影响 feature 组合。
- 若要并行探测，不可仅把循环替换为无界线程生成。应规定并发上限、停止传播、单地址互斥和扫描结束语义，并新增慢客户端、多地址、停止中探测及 panic 的独立测试。
- 修改时间规则时应同时覆盖首次成功、连续成功、成功后失败、严格阈值边界、慢请求完成时刻以及锁竞争。特别要保留 `last_lookup_time` 只代表业务查询、失败重置连续恢复窗口这两个不变量。
- 修改 LRU 时同步验证覆盖写入、读取提升、删除、零容量和并发访问；若容量显著增大，应考虑用 O(1) 顺序结构替代当前 `VecDeque::retain`。
- 调整 worker 生命周期时应补测客户端 panic 后能否重新启动，以及配置修改在运行中是否生效。测试逻辑继续放在 `mpp_probe_test.rs`，不要内嵌到生产源文件。
- 兼容风险主要是“不存在即恢复”的调用约定、严格 `>` 的 TTL 门槛、Go 风格别名和全局单例 API；性能风险主要是串行同步探测与 LRU 线性顺序更新。

## 验证依据

- RustCodeGraph `status`：索引有效，含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/store/copr`：确认目标源、Go 对照、Rust/Go 独立测试和 crate 入口均在索引中。
- RustCodeGraph `node --file pkg/store/copr/mpp_probe.rs`：逐行核对 398 行生产实现，包括常量、trait、状态、LRU、扫描、worker、单例和别名。
- RustCodeGraph 对 `MppFailedStoreProber`、`MppStoreState`、`MppServerInfoManager`、`global_mpp_failed_store_prober`、`global_mpp_server_info_manager`、`detect_mpp_store` 的 `query`/`explore`：确认符号身份、测试调用边，并确认全局访问器未检出模块外 Rust 生产调用。
- `pkg/store/copr/Cargo.toml` 与 `pkg/store/copr/lib.rs`：确认 crate 名、Go 包映射、模块声明、公开再导出和独立测试模块装配；本文件本身只依赖标准库。
- `pkg/store/copr/mpp_probe.go`、`pkg/store/copr/mpp_probe_test.go`：核对 Go 的真实 RPC、指标/日志、生产全局对象、并行扫描、TTL/清理规则和原测试意图。
- `pkg/store/copr/mpp_probe_test.rs`：核对当前 Rust 可执行测试的失败/恢复状态流、慢探测周期、LRU、worker 单例和删除幂等边界。
- 对精确 Rust/Go 符号的 `rg` 补查：在调用图未覆盖生产接线结论时，确认 Rust snake_case 全局访问器没有模块外使用，并定位 Go 的 batch coprocessor、optimizer 与启动/停止调用点。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查和文档范围自查。
