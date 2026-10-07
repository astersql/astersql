# `pkg/autoid_service/autoid.rs`

## 文件定位

本文件是 `astersql-autoid_service` crate 的核心服务实现，源码入口为 [`autoid.rs`](autoid.rs)，由 [`lib.rs`](lib.rs) 的 `mod autoid` 纳入并通过 `pub use autoid::*` 重新导出。它把 `pkg/meta/autoid` 提供的事务性 ID 存储抽象包装成按表缓存号段的服务，同时提供进程内 `AutoIdClient` 与 kvproto gRPC `AutoIdAlloc` 两种调用面。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：关键依赖包括别名 `autoid_dependency = astersql-meta-autoid`（请求、错误、存储 trait 和进程内客户端协议）、`owner_dependency = astersql-owner`（etcd owner 选举）、`kvproto`/`grpcio`（线协议）、`etcd-client`、`metrics`、`fail` 与 `tokio-util`。文件自身不创建 SQL 表元数据，也不决定何时采用单点分配器；已验证的上层接线在 `pkg/session/runtime/create_table_resources.rs`，其中单点 allocator 通过 `ClientDiscover` 使用远程服务，mock storage 则注入 `mock_for_test`。

## 核心职责

1. 以 `(database_id, table_id)` 为键维护 `AutoIdValue { base, end }`：`(base, end]` 是已经从持久化存储预留、尚可在内存消费的区间。
2. 在缓存不足时通过 `AutoIdStorage::run_in_transaction` 原子读取并推进 `IncrementId(TABLE_INFO_VERSION_5)`，每次至少预取 `BATCH = 4000`，降低存储访问频率。
3. 按 MySQL `auto_increment_increment`/`auto_increment_offset` 规则计算实际占用跨度，分别保持 signed `i64` 与 unsigned `u64` 位模式语义。
4. 实现普通 rebase（只前进）和 force rebase（允许回退），并把 rebase 结果记录到 `tidb_autoid_rebase_seconds` 指标。
5. 通过 etcd owner 管理器限制真实服务只由 owner 响应；成为新 owner 时丢弃所有本地号段，防止重新当选后使用陈旧缓存。
6. 将内部结果适配为进程内 trait 或 gRPC 响应；区分 RPC/路由错误与写入响应 `errmsg` 的业务错误。

## 主要符号

- `AUTO_ID_LEADER_PATH`：owner 选举路径后缀 `tidb/autoid/leader`；`new_with_client` 会在前面拼接 `AutoIdStorage::etcd_namespace()`。
- `BATCH`：默认预取步长 4000。单次请求跨度大于它时，实际事务步长提升到请求所需跨度。
- `InitError`：只包装服务初始化时的 etcd 与 owner 错误；分配阶段使用 `autoid_dependency::Result`/`AutoIdError`。
- `AutoIdStorage: IdStore`：服务的存储契约。除事务接口外，还要求 `uuid()`（mock 服务复用）、`keyspace_id()`（请求隔离）与 `etcd_namespace()`（选举隔离）。
- `AutoIdCacheKey`：仅以库 ID、表 ID 定位缓存；列的 signed/unsigned 属性不在键中，因此同一表应以一致的列属性调用。
- `AutoIdValue`：单表状态。`base` 是已消费到的位置，`end` 是持久层预留上界；`is_unsigned` 与容量为 1 的 `token` 目前被保留但未参与分配控制，真实串行化由外层 `Mutex<AutoIdValue>` 完成。
- `AutoIdValue::{allocate_signed, allocate_unsigned}`：号段消费与补段核心；返回 `(min, max]`。
- `AutoIdValue::{rebase_signed, rebase_unsigned, force_rebase}`：普通/强制调整计数器。
- `ServiceInner`/`Service`：分别保存共享状态与可克隆句柄。映射锁保护 allocator 集合，每个 allocator 自有锁。
- `Service::{new, new_with_client, with_manager, new_mock, close}`：构造与资源生命周期入口；`new` 建立 etcd 连接，`new_with_client` 注册 listener 并以 10 秒租约参数竞选，`new_mock` 不配置 manager。
- `Service::{allocate, rebase_ids}`：不依赖具体传输协议的业务入口。
- `impl AutoIdClient for Service`：检查 `Context` 后把内部请求转换成 kvproto 请求，供同进程调用。
- `impl AutoIdAlloc for Service`/`create_grpc_service`：gRPC 服务端适配与注册对象构造。
- `mock_for_test`/`MOCK_SERVICES`：按存储 UUID 进程内复用 mock `Service`。
- `OwnerListener`：持有 `Weak<ServiceInner>`，当选时清空缓存，不形成引用环。
- `seek_to_first_auto_id_{signed,unsigned}` 与 `calc_needed_batch_size`：步长/偏移对齐算法。
- `read_failed`、`standard_read_failed`、`response_error`、`rebase_error`、`spawn_grpc`：错误文案、响应编码和异步发送辅助函数。

## 执行流程

### 初始化和选主

`Service::new` 先以调用方提供的 endpoints/`ConnectOptions` 连接 etcd，再进入 `new_with_client`。后者构造 `election_path = store.etcd_namespace() + AUTO_ID_LEADER_PATH`，创建 `NewOwnerManager`，将弱引用形式的 `OwnerListener` 注册给 manager，然后调用 `CampaignOwner(&[10])`。初始化成功只表示竞选已发起；请求时仍由 `ensure_owner` 实时检查 `Manager::IsOwner()`。

### 分配

`Service::allocate` 的顺序是不变量的一部分：

1. 比较请求 `keyspace_id` 与存储 `keyspace_id()`；不匹配直接返回 RPC 类 `not leader`。
2. `ensure_owner` 拒绝配置了 manager 但当前非 owner 的实例；`new_mock` 的 manager 为 `None`，因此被视为可服务。
3. 执行 `mockErr` failpoint，然后按 `(db_id, tbl_id)` 取得并锁定单表 allocator。
4. `n == 0` 是只读探测：已有缓存就返回 `(base, base]` 的退化表示；未初始化则事务读取存储值并同步 `base/end`，不推进计数器。
5. 正常请求按 `is_unsigned` 分流。分配器先在需要时 rebase 到 `offset - 1`，再由 `calc_needed_batch_size` 算出从当前 base 到第 `n` 个合法 ID 的跨度。
6. 若本地 `(base, end]` 足够，直接推进 `base`。否则事务读取持久值；若首次加载或 `stored_base != cached_end`，以持久值重置缓存并重新计算跨度；随后以 `max(BATCH, needed)` 为候选步长并受数值上界截断，原子 `inc` 得到新 `end`。
7. 成功返回旧 `base` 与新 `base`，即左开右闭区间 `(min, max]`；存储/溢出错误被编码进响应 `errmsg`，而不是变成传输失败。

例如 `base=139, n=1, increment=10, offset=5` 时，下一个合法值是 145，`needed=6`，结果为 `(139,145]`；客户端只使用其中符合步长/偏移的值。

### Rebase

`Service::rebase_ids` 先检查 owner 并锁定单表 allocator。若 `force=true`，`force_rebase` 通过存储当前值与目标值的差量直接把计数器设为目标，并把缓存收缩为 `base=end=required_base`；随后仍执行普通 rebase。普通 rebase 有三条路径：目标不大于 `base` 时无操作；目标落在缓存区间时只移动内存 `base`；目标超过 `end` 时事务写入 `max(stored_end, required_base) + BATCH`（在数值上界处饱和），并将缓存设为新的 base/end。

### 调用适配

进程内 `AutoIdClient` 实现先执行 `Context::check()`，再调用上述业务入口。gRPC `AutoIdAlloc` 实现同步计算业务结果后交给 `spawn_grpc`：成功使用 `UnarySink::success`，入口返回的 RPC 类错误映射为 `RpcStatusCode::UNKNOWN`；发送失败仅记录日志。`create_grpc_service` 只是将 `Service` 交给 kvproto 生成的注册函数。

## 数据与状态

- 持久状态的键由 `AutoIdValue::key` 固定编码为 `AutoIdKey { database_id, table_id, kind: IncrementId(5) }`；版本 5 与 Go 的 `model.TableInfoVersion5` 对齐。
- 持久值表示全局已预留的号段上界，内存 `base` 表示本实例已经交付到的位置。服务故障会造成已预留但未消费的 ID 空洞，这是批量预取换取吞吐的预期代价，不会导致重号。
- unsigned 值仍存放在 `i64` 字段中，通过 `as u64` 和 wrapping 运算解释位模式。因此越过 `i64::MAX` 后响应可表现为 `i64::MIN`，而 `-1` 表示 `u64::MAX`。
- `MOCK_SERVICES` 是进程级 `OnceLock<Mutex<HashMap<String, Service>>>`，生命周期到进程结束；相同 UUID 的 store 会共享服务与进度。它是测试便利入口，不能用于隔离要求不同但 UUID 相同的测试实例。
- `Service` 克隆只克隆 `Arc`，所有克隆共享 allocator 映射、manager 和 store。

## 依赖与调用关系

RustCodeGraph 对 `autoid.rs::allocate` 的调用边显示：上游为本文件的两个 `alloc_auto_id`（进程内 trait 与 gRPC trait）以及迁移测试；下游为 `ensure_owner`、`get_allocator`、`allocate_signed`/`allocate_unsigned`、`AutoIdValue::key` 和 `response_error`。`rebase_ids` 同理由两个 `rebase` 适配入口调用，下游进入三种 rebase 函数与 `rebase_error`。`calc_needed_batch_size` 由 signed/unsigned 分配路径调用，再调用对应的 `seek_to_first_auto_id_*`。

存储调用统一穿过 `autoid_dependency::IdStore::run_in_transaction` 与 `IdTransaction::{get,inc}`，使本文件不依赖具体 TiKV transaction 类型。owner 路径使用 `owner_dependency::{Manager, Listener, NewOwnerManager}`；协议路径使用 `kvproto::autoid` 和 `grpcio`。

已验证的应用侧消费在 `pkg/session/runtime/create_table_resources.rs`：single-point auto increment allocator 构造 `ClientDiscover`；mock storage 把本文件的 `mock_for_test` 注入发现器，非 mock storage 使用同 crate 的 [`client.rs`](client.rs) 从 etcd 发现 leader 并发起 gRPC。仓库搜索没有找到生产 Rust 代码直接调用 `Service::new`/`new_with_client` 或 `create_grpc_service`；`create_grpc_service` 的直接调用者目前是 [`autoid_test.rs`](autoid_test.rs)、[`client_test.rs`](client_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。因此“Rust 服务端已由哪个进程启动和注册”在本次直接证据中未验证，不能仅凭服务实现推定已经完成生产接线。

## 错误处理与边界

- 初始化错误通过 `InitError` 向调用者传播，不 panic；`close` 则委托 manager 退出选举。
- keyspace 不匹配、非 owner、failpoint 和 `Context::check()` 失败属于调用/RPC 层错误。经 gRPC 入口时统一成为 `UNKNOWN` 状态；客户端应重新发现 owner 或传播错误。
- 分配/存储/数值耗尽发生在 owner 已接受请求之后，`allocate`/`rebase_ids` 会返回成功的 Rust `Result`，但响应 `errmsg` 非空。调用方必须同时检查调用结果与 `errmsg`。
- signed 分配要求剩余空间严格大于所需跨度；达到 `i64::MAX` 后返回 `auto increment action failed`。unsigned 在最外层预检查耗尽时保留 Go `autoid.ErrAutoincReadFailed` 的标准文案 `[autoid:1467]Failed to read auto-increment value from storage engine`，事务补段阶段的耗尽使用普通 action-failed 文案。
- 算术刻意使用 wrapping 操作以复现 Go 的二进制补码/无符号转换语义；这不等于任意输入都有效。文件没有显式拒绝 `increment <= 0`、异常 offset 或 `n > i64::MAX`，这些参数约束依赖上游协议/SQL 层。新增直连调用者不能把 helper 当作已校验输入的公共数学 API。
- 所有 `std::sync::Mutex::lock()` 使用 `unwrap()`；持锁线程 panic 会导致锁 poisoning，后续访问也会 panic。当前代码没有恢复策略。
- `OnRetireOwner` 为空；安全性依赖请求入口每次检查 owner，以及下一次当选时清缓存。

## 并发与资源生命周期

`ServiceInner::auto_id_map` 使用两级锁：外层 map 锁仅用于查找/创建每表 allocator，返回 `Arc` 后立即释放；内层 allocator 锁覆盖单表的读取、计算、持久事务与缓存更新。因此同一表请求严格串行，不同表通常可并行，但各自的存储实现仍可能施加更粗粒度锁。

owner listener 使用 `Weak<ServiceInner>`，manager 持有 listener 时不会反向延长服务生命周期或形成环。成为 owner 时取得 map 锁并整体 `clear`；已经取得某个 allocator `Arc` 的并发请求不会被 `clear` 强制取消，所以正确性还依赖 owner 检查与选举时序。该文件没有在事务中二次检查 leadership。

etcd client 被 manager 接管；`Service::close().await` 是显式清理入口。gRPC 响应 future 由 `RpcContext::spawn` 驱动，发送失败只记录。`MOCK_SERVICES` 不提供删除接口，测试进程中按 UUID 持久保留服务。`AutoIdValue::token` 当前创建但不收发消息，不承担资源同步作用。

## 与 Go 版本的对应关系

直接对照文件是 [`autoid.go`](autoid.go)，独立测试对照为 [`autoid_test.go`](autoid_test.go) 与 Rust 的 [`autoid_test.rs`](autoid_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。主要映射如下：

| Go | Rust | 对齐情况 |
| --- | --- | --- |
| `autoIDValue.alloc4Signed/alloc4Unsigned` | `AutoIdValue::allocate_signed/allocate_unsigned` | 对齐缓存补段、4000 默认 batch、存储脱节重载、signed/unsigned 上界 |
| `rebase4Signed/rebase4Unsigned` | `rebase_signed/rebase_unsigned` | 对齐只前进和额外预留 batch；Rust 指标名直接写为 metrics histogram |
| `forceRebase` | `force_rebase` | 对齐允许回退和 signed/unsigned 差量运算 |
| `Service.AllocAutoID` + `allocAutoID` | `Service::allocate` + 两类 trait 适配 | Rust 合并了 Go 外层重试循环；当前 Rust 业务入口没有返回 `nil` 响应的分支，因此无需对应循环 |
| `Service.Rebase` | `Service::rebase_ids` | 对齐 owner 门禁、force 后普通 rebase、响应 `Errmsg` |
| `ownerListener.OnBecomeOwner` | `OwnerListener::OnBecomeOwner`/`on_become_owner` | 对齐重新当选时清空缓存；公开 `on_become_owner` 便于直接测试 |
| 全局 `map[string]*mockClient` | `OnceLock<Mutex<HashMap<String, Service>>>` | 均按 store UUID 复用；Rust 为全局 map 增加互斥保护 |

已知实现形态差异：Go `New` 自行读取全局配置构造 etcd TLS、keepalive 和 namespace，并在失败时 panic；Rust 把 endpoints/`ConnectOptions`/namespace 抽象交给调用者并返回 `InitError`。Go 的 `autoIDValue` 自身嵌入 mutex；Rust 将 `Mutex` 放在 `Arc<Mutex<AutoIdValue>>` 外层。Go `init()` 将 `meta/autoid.MockForTest` 指向本服务；Rust 没有对应全局函数指针，而是由上层显式调用/注入。Rust 的 `is_owner()` 在 mock 模式返回 false，虽然 `ensure_owner()` 将 mock 视为可服务；这与 Go `IsOwner()` 在 nil leadership 时返回 false 一致。

测试证据确认：基础区间、步长/偏移、`n=0`、普通和强制 rebase、有符号/无符号极值、30 路并发、keyspace 门禁、owner 缓存重置、相同 UUID 复用和真实 kvproto gRPC 均有 Rust 用例；Go `TestGRPC` 包含真实 etcd 选举，而 Rust `test_grpc` 明确用 `new_mock` 聚焦线协议，故真实 Rust etcd 选举生命周期仍不由这些单测覆盖。

## 扩展指南

- 修改分配算法时，应同时审查 `allocate_signed`、`allocate_unsigned` 和 `calc_needed_batch_size`，保持两种数值域以及 `(min,max]` 契约；在 [`autoid_test.rs`](autoid_test.rs) 或 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 增加独立测试，不要把测试写进生产文件。
- 修改存储键或 table-info 版本时，入口是 `AutoIdValue::key`；必须与 `pkg/meta/autoid` 的 `AutoIdKeyKind`、Go `GetAutoIDAccessors(...).IncrementID(model.TableInfoVersion5)` 以及历史数据兼容性一起评估。
- 修改缓存/并发策略时，保持“持久层先预留、内存后交付”和 owner 切换清缓存两个不变量；重点回归同表并发、不同表并行、失去/重新获得 owner 以及存储值与 `end` 不一致的分支。
- 新增错误类型时，先决定它是 RPC 层错误还是响应体业务错误，再分别修改 `spawn_grpc` 或 `response_error`/`rebase_error`；错误文案可能被 Go 兼容测试精确断言。
- 新增租户隔离字段时，不能只改请求 proto；还需同步 `AutoIdStorage`、选举 namespace、缓存键和 mock 全局键，防止不同 keyspace 共享缓存或 owner 路径。
- 若要完成生产服务端接线，应从实际 server 生命周期创建 `Service::new`、注册 `create_grpc_service`、在 shutdown 调用 `close`，并新增覆盖真实 etcd campaign/leader 切换的独立集成测试；当前搜索结果不足以声称这部分已接线。
- 调整 `BATCH` 会改变持久事务频率、故障空洞大小和热点表锁持有时间，应同时评估吞吐与 ID 浪费，不应只以功能测试判断。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录列出 `autoid.rs` 72 个符号。读取了目标文件全部 863 行。
- RustCodeGraph `node autoid.rs::allocate`：调用 `ensure_owner`、`get_allocator`、signed/unsigned 分配和 `response_error`；调用者包含两个 `alloc_auto_id` 适配入口及迁移测试。
- RustCodeGraph `node autoid.rs::rebase_ids`：调用三种 rebase、owner 检查与响应编码；调用者为进程内/gRPC `rebase` 及并发迁移测试。
- RustCodeGraph `node autoid.rs::allocate_unsigned`、`node autoid.rs::rebase_unsigned`、`node calc_needed_batch_size`：确认补段、rebase、对齐 helper 的调用边；`calc_needed_batch_size` 由 signed/unsigned 分配路径调用。
- RustCodeGraph `node autoid.rs::new_with_client` 与 `node autoid.rs::OnBecomeOwner`：确认选举路径、listener 实例化和当选清缓存逻辑。`node create_grpc_service` 的直接调用者仅为本 crate 测试。
- 已读源码/配置：[`autoid.rs`](autoid.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`client.rs`](client.rs)、`pkg/session/runtime/create_table_resources.rs`。
- 已读 Go 对照与测试：[`autoid.go`](autoid.go)、[`autoid_test.go`](autoid_test.go)、[`autoid_test.rs`](autoid_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；测试断言提供了上述区间、边界、并发、keyspace、缓存重置、mock 复用及 gRPC 证据。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前仅执行任务指定的 11 章结构验证，并人工复核文档没有把未找到的生产服务端接线写成既成事实。
