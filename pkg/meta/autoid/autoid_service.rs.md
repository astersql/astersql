# `pkg/meta/autoid/autoid_service.rs`

## 文件定位

非 mock 存储的发现与连接适配器由 `pkg/autoid_service/client.rs::client_discover` 创建，负责 etcd 与 gRPC 细节；因此本文件刻意只依赖 `LeaderDiscovery`、`AutoIdClientConnector`、`AutoIdClient` 和 `ClientConnection` 四个抽象边界。

## 核心职责

1. `get_auto_id_service_leader_etcd_path` 根据是否为 `NULLSPACE_ID` 生成与 Go 版一致的 Leader 键路径。
2. `ClientDiscover` 懒发现 Leader、建立并缓存 RPC 客户端，以连接世代 `version` 防止旧请求的失败清掉新连接，并在重置后延迟关闭旧连接。
3. `Backoffer` 为 Leader 未选出和 RPC 暂时失败提供 10ms 起步、最高 100ms 的可取消指数退避；内部初值为 5ms，首次调用先翻倍再等待。
4. `SinglePointAllocator` 实现通用 `Allocator` trait，使每次 `alloc`/`rebase` 都访问 AutoID Leader，同时维护当前进程观察到的最高水位。
5. `RpcRetryState`、`RpcRetryPolicy` 与 `RpcRetryLogState` 对 RPC 类错误执行“错误次数与持续时间同时达到阈值才终止”的策略，并记录进入重试、恢复/失败/取消和快速失败结果。
6. `transfer` 与 `force_rebase` 串行化改变性操作，并通过 30 秒写操作上下文限制其生命周期。

本文件不实现服务发现协议和网络传输本身，也不支持 sequence 的 `alloc_seq_cache`/`rebase_seq`；这些调用明确返回 `AutoIdError::NotImplemented`。

## 主要符号

- `AUTO_ID_LEADER_PATH` / `NULLSPACE_ID`：默认 etcd Leader 键与无独立 keyspace 的哨兵值。`get_auto_id_service_leader_etcd_path` 对 Nullspace 返回不带前导斜杠的 `tidb/autoid/leader`，其他 keyspace 返回 `/tidb/autoid/leader`；当前路径不把具体 keyspace 数值编码进去。
- `AutoIdRequest` / `AutoIdResponse`：分配 RPC 的内部传输模型。成功区间语义是 `(min, max]`；响应 `errmsg` 非空仍属于服务端业务失败。
- `RebaseRequest` / `RebaseResponse`：携带库表、新 base、`force` 和有无符号属性的 rebase 模型。
- `LeaderDiscovery::leader`：按路径读取可选 Leader 地址；`Ok(None)` 表示尚未选主，调用方继续退避。
- `AutoIdClient::{alloc_auto_id,rebase}`：远程服务调用边界；`AutoIdClientConnector::connect` 同时返回客户端和必须被释放的 `ClientConnection`。
- `ClientDiscover::{new,get_client,reset_conn_if_version,reset_conn}`：连接缓存生命周期的主体。`seed_client_for_test` 是公开的测试/内嵌 mock 注入入口，不建立实际连接。
- `Backoffer::{reset,backoff}`：受 `Context` 取消控制的指数等待器，范围由 `BACKOFF_MIN` 和 `BACKOFF_MAX` 限定。
- `SinglePointAllocator::new`：绑定库表身份、符号模式、keyspace、发现器和默认重试策略；`operation_lock` 控制普通操作与身份/水位强制变更之间的并发关系，`state` 保存身份及 `last_allocated`。
- `RpcRetryPolicy` / `RpcRetryState::observe`：默认阈值为至少 10 次错误且首错后至少 15 秒；两个条件为 AND 关系。测试策略的 `min_errors=0` 不表示禁用，而会由 `effective_retry_policy` 回退到默认值。
- `SinglePointAllocator::{alloc_inner,rebase_inner,retry_rpc}`：请求构造、错误分类、连接重置、退避及终止错误形成的核心循环。
- `Allocator for SinglePointAllocator`：公开行为入口，包括 `alloc`、`rebase`、`force_rebase`、`transfer`、`base`、`end`、`next_global_auto_id` 与 `get_type`。

## 执行流程

分配流程由 `Allocator::alloc` 取得 `operation_lock` 读锁后进入 `alloc_inner`：

1. 使用 `valid_increment_and_offset` 校验 MySQL 风格 increment/offset；失败直接返回参数错误，不接触网络。
2. `ClientDiscover::get_client` 先在读锁下尝试缓存，未命中后取得写锁并双重检查；随后循环检查 `Context`、读取 Leader、对未选主状态退避，最后由 connector 建连并同时缓存 client/connection。
3. 从 `SinglePointState` 快照库表身份，构造含 `n`、increment/offset、符号属性和 keyspace 的 `AutoIdRequest`，释放状态锁后再做 RPC，避免网络等待占住该锁。
4. 成功响应若有 `errmsg`，转成 `AutoIdError::Service`；否则以 `update_last_allocated(response.max)` 单调推进本地水位并返回 `(min,max)`。
5. 只有 `AutoIdError::Rpc` 进入 `retry_rpc`：先检查取消，记录错误，按原连接版本尝试重置，再检查取消；达到次数和时长双阈值则返回 `RpcRetryLimit`，否则按最多 100ms 退避后重试。其他错误原样终止，不重试。

普通 `rebase` 取得读锁后进入 `rebase_inner`，其发现、RPC 错误与退避流程与分配相同。非 force 成功时只允许水位上升；force 成功时直接把 `last_allocated` 设为目标值，所以允许降低 base。

`transfer` 取得写锁，若目标身份相同则立即成功；否则在同一个 30 秒上下文内先以 `alloc_inner(ctx, 0, 1, 1)` 刷新源表权威水位，再暂时切换库表身份，并把该水位 rebase 到目标表。目标 rebase 失败时恢复旧库表身份，但保留已经观察到的最新源水位。`force_rebase` 也持写锁并使用同一超时包装；`new_base == -1` 会在发 RPC 前拒绝，因为它意味着把下一个全局 ID 强制设为 0。

`next_global_auto_id` 通过零数量分配读取服务端最大值并用 `wrapping_add(1)` 得到下一个值；`base` 和 `end` 在单点模式中都返回同一个 `last_allocated`。

## 数据与状态

`ClientState` 在 `RwLock` 中保存成对的可选 client 和 connection；连接世代另存于 `AtomicU64 version`。成功建连不增加版本，只有赢得 `compare_exchange(version, version+1)` 的失败请求能触发一次重置，因此多个使用同一旧连接的并发失败不会反复清理后来建立的新连接。

`SinglePointState` 在 `Mutex` 中保存 `database_id`、`table_id` 与 `last_allocated`。普通分配可能并发且响应可能乱序，`update_last_allocated` 因而比较后才写入：有符号模式用 `i64` 顺序，无符号模式先转成 `u64` 比较，使负的 `i64` 位模式可表示大于 `i64::MAX` 的无符号 ID。非 force rebase 同样保持此单调不变量；force rebase 是唯一有意突破它的路径。

`operation_lock` 的读锁允许 `alloc` 与普通 `rebase` 并发，写锁则让 `transfer`/`force_rebase` 等到正在进行的普通操作结束并阻止新操作进入。它和 `state` 分工明确：前者保护跨多个 RPC 的高层操作顺序，后者保护短时身份/水位访问。

每次 alloc/rebase 建立独立的 `Backoffer`、`RpcRetryState` 和 `RpcRetryLogState`。重试日志的全局 `RPC_RETRY_REQUEST_SEQUENCE` 只在请求首次进入 RPC 重试时分配 ID；`Drop` 根据 `recovered`、最终错误和终止标记补发一次完成日志，已发快速失败日志时不会重复。

## 依赖与调用关系

上游选择链为 `pkg/meta/autoid/autoid.rs::new_allocators_from_table_info` → `new_allocator` → `Requirement::single_point_allocator` → `pkg/session/runtime/create_table_resources.rs::Requirement::single_point_allocator` → `SinglePointAllocator::new`。这说明本文件只服务 `AUTO_ID_CACHE=1` 的 AutoIncrement 分配器，并非 RowID、AUTO_RANDOM 或 SEQUENCE 的通用远程替代品。

运行时依赖链为 `pkg/session/runtime/create_table_resources.rs::requirement` → `pkg/autoid_service/client.rs::client_discover` → `ClientDiscover::new`。后者把 etcd 客户端、namespace、TLS 和 gRPC 环境封装成四个 trait 的具体实现；本文件下游只直接调用 `Context::{check,wait}`、`valid_increment_and_offset`、错误构造器及这些 trait。

RustCodeGraph 的文件使用关系还列出 `pkg/meta/reader.rs`、`pkg/server/server.rs` 等对本模块公开项的引用；对核心私有流，图查询明确得到 `alloc_inner → get_client`、`rebase_inner → get_client`，并得到 `alloc → alloc_inner`、`transfer → alloc_inner`、`rebase/force_rebase/transfer → rebase_inner`。源码进一步确认 client RPC 分别是两个 inner 循环的最终下游。

## 错误处理与边界

- increment/offset 非法在任何连接动作之前返回 `InvalidIncrementAndOffset`。
- Leader 读取或 connector 建连错误由 `get_client` 直接传播；只有“未选出 Leader”才循环退避。
- 服务端以成功 RPC 返回的非空 `errmsg` 映射为 `Service`，不触发重连；本地 `Canceled`、校验错误和其他非 `Rpc` 错误同样不重试。
- `Rpc` 错误先检查上下文，因此已取消请求只执行一次 RPC 并快速返回；未取消时清连接、退避并重试。次数与持续时间必须同时达到阈值才生成带 keyspace/db/table、最后错误和行动建议的 `RpcRetryLimit`。
- `force_rebase(-1)` 返回 `AutoIncrementReadFailed`；sequence 专用接口明确返回 `NotImplemented`。
- `with_write_timeout` 通过克隆 `Context`、计时线程和取消标志提供 30 秒边界。它能唤醒本文件内的 `Context::wait`，实际 RPC 的阻塞上限还依赖 `AutoIdClient` 适配器；当前 `pkg/autoid_service/client.rs` 为 alloc/rebase RPC 都配置了 30 秒 timeout。
- 所有标准锁都用 `unwrap()`；若持锁线程 panic 导致锁中毒，后续调用会 panic，而不是转成 `AutoIdError`。连接的 `close` 错误被有意忽略，因为它发生在旧连接的异步清理阶段。
- `next_global_auto_id` 使用 wrapping 加法，极值溢出会按二进制环绕；调用者不能把它视为额外的容量校验。

## 并发与资源生命周期

`ClientDiscover` 可跨线程共享。缓存读取走 `RwLock` 快路径，首次建连在写锁内串行化，避免重复连接。RPC 失败携带取得 client 时的版本；只有版本仍相等的调用能递增版本并清缓存。旧 `ClientConnection` 从状态中取出后，由新线程等待 200ms 再关闭，为仍使用它的 in-flight RPC 留出收尾窗口；每次有效重置最多产生一个短生命周期清理线程。

`SinglePointAllocator` 的普通 alloc/rebase 共享读锁，因此不会靠全局串行化掩盖乱序响应；`last_allocated` 的比较更新负责保持最高水位。transfer/force rebase 持写锁跨越完整远程操作，保证身份切换或强制降低 base 不与普通分配交错。状态 mutex 只在复制/更新小量字段时持有，RPC 前显式释放。

`with_write_timeout` 为每个 transfer 或 force rebase 创建一个计时线程。工作先完成时设置 `done`、unpark 并 join；超时时计时线程取消克隆的 Context，随后仍等待工作观察取消并返回。由此它是协作式取消而不是强行终止工作线程。

## 与 Go 版本的对应关系

两边保持的关键语义包括：Leader 路径规则、双检客户端缓存、连接版本 CAS、延迟 200ms 关闭、10ms 首次/100ms 上限退避、`(min,max]` 区间、RPC-only 重试、10 次且 15 秒的双阈值、单调水位、无符号比较、transfer 先刷新源水位并在失败时恢复身份，以及写操作 30 秒限制。

实现边界存在几项明确差异：Go 文件直接依赖 etcd、配置、TLS、gRPC、metrics、tracing 和 zap；Rust 将网络细节注入到 trait，具体适配器在 `pkg/autoid_service/client.rs`，日志目前用 `eprintln!`，本文件也没有 Go 对应的 metrics/tracing 埋点。Go 的水位是 `atomic.Int64` CAS，Rust 用 mutex 内的比较更新实现同一乱序不回退语义。

独立 Rust 测试 `pkg/meta/autoid/autoid_service_test.rs` 与 Go 测试 `pkg/meta/autoid/autoid_service_test.go` 共同验证上述移植意图；Rust 测试名称中的 `go_merge_5_*`、`go_merge_12_*` 还显式记录了相应语义合入批次。

## 扩展指南

- 新增真实发现或传输后端时，实现 `LeaderDiscovery`、`AutoIdClientConnector`、`AutoIdClient` 和 `ClientConnection`，不要把 etcd/gRPC 细节塞回分配循环；同时在 `pkg/autoid_service/client.rs` 的独立测试中验证地址、TLS、timeout 与错误到 `AutoIdError` 的分类。
- 修改请求字段时同步更新 `AutoIdRequest`/`RebaseRequest`、具体 wire 转换、Go kvproto 对照和 `pkg/meta/autoid/autoid_service_test.rs` 中对完整请求的断言。错误分类尤其重要：只有真正可重试的网络失败应成为 `AutoIdError::Rpc`。
- 修改重试策略时优先调整 `RpcRetryPolicy`、`RpcRetryState::observe` 和 `retry_rpc`，并同步验证次数/时长 AND 语义、取消优先级、连接版本增量、日志只出现一次及 terminal/recovery 状态；避免把业务错误纳入无限重试。
- 修改并发控制时必须保留三项不变量：普通并发响应不得降低水位；transfer/force rebase 不得与普通操作交错；旧版本失败不得清除新客户端。相应测试应继续放在独立的 `autoid_service_test.rs`，不要内嵌到生产文件。
- 扩展 transfer 时保持“源端零数量读取与目标 rebase 共用一个超时上下文”，并保留目标失败后的身份回滚；若增加更多中间状态，需要测试每个失败点的所有权和水位结果。
- 若要支持 sequence，不应简单让当前两个 `NotImplemented` 返回成功；必须先与 Go `Allocator` 的缓存、循环轮次和 rebase 语义完整对齐，并在独立测试中覆盖边界。
- 改动 keyspace 路径规则时，同时核验服务端注册键、etcd namespace 和 `get_auto_id_service_leader_etcd_path`；当前具体非 Nullspace ID 不进入字符串，这是已验证现状，不应凭直觉改写。
