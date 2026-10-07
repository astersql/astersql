# `pkg/ddl/util/util.rs`

## 文件定位

`util.rs` 是 `astersql-ddl-util` 子 crate 的通用工具实现，crate 入口 `pkg/ddl/util/lib.rs` 通过 `mod util` 装入本文件并以 `pub use util::*` 全量再导出。根工作区又以 `facade_ddl_util` 引入该 crate，`pkg/lib.rs` 将其暴露为 `ddl::util`。因此，本文件定义的是可被 DDL、session、domain、GC worker 等模块复用的公共边界，而不是某一种 DDL job 的 worker 或状态机执行器。

本文件目前同时承担两类角色：一类是生产接线所需的协议常量和错误类型；另一类是以进程内状态模拟 Go 实现依赖的 etcd、SQL 会话和元数据对象。代码搜索显示 Rust 生产路径已经直接使用 `DDLOwnerKey`、`ServerGlobalState` 和 `DdlUtilError`，例如 `pkg/session/runtime/session.rs`、`pkg/session/runtime/session_factory.rs` 与 `pkg/ddl/db_integration_test.rs`；但本文件的大多数函数当前只由同 crate 的 `util_test.rs` 覆盖，或者尚未被 Rust 生产代码直接调用。不能仅因它们被公开导出就推断完整 DDL/GC 主链已经接入。

`pkg/ddl/util/Cargo.toml` 声明包名为 `astersql-ddl-util`，库入口为 `lib.rs`，移植来源是 Go 包 `pkg/ddl/util`；其 `[dependencies]` 为空，说明本文件刻意只使用标准库和本地精简模型，不直接依赖真实 etcd client、sessionctx、TiDB model 或 KV codec。

## 核心职责

- 定义 DDL 协调所用的 etcd 键路径与租约常量：`DDLOwnerKey`、`DDLAllSchemaVersions`、`DDLAllSchemaVersionsByJob`、`DDLGlobalSchemaVersion`、`AddingDDLJobNotifyKey`、`ServerGlobalState`、`SessionTTL`。
- 提供统一错误 `DdlUtilError`，覆盖取消、暂停约束、十六进制解码、etcd、内部 SQL 和唯一键冲突。
- 以 `EtcdClient` 模拟键值、修订号、CAS、前缀删除、精确键 watch 和可控失败，并在其上实现重试写入工具。
- 以 `SessionContext` 模拟 GC 删除范围表、全局变量、时区和 Raft 引擎查询，提供加载、完成、删除和推进删除范围的操作。
- 用精简 `Job` 模型实现系统库判断和暂停状态转换。
- 提供模拟器 GC 开关、TopSQL 内部资源标签、键的 SQL/十六进制格式化、目录探测和唯一索引冲突错误构造。

这些职责是横向工具，不负责 DDL job 持久化、owner 调度、schema version 推进或 reorg/backfill 本身。尤其是 `EtcdClient` 和 `SessionContext` 都是内存实现，不能等同于 Go 文件中真实的 etcd 与内部 SQL 访问。

## 主要符号

协议与错误：

- `DELETE_RANGES_TABLE`、`DONE_DELETE_RANGES_TABLE` 对应 GC 删除范围的活动表和完成表名称；etcd 路径常量及 `SessionTTL` 与同路径 Go 常量保持字符串/数值一致。
- `DdlUtilError` 是本文件所有可失败 API 的枚举错误；`Display` 生成用户可读消息，其中 `KeyExists` 采用 MySQL 风格 `Duplicate entry ... for key ...`。
- `CancellationToken(Arc<AtomicBool>)` 提供 `cancel`、`is_cancelled`、`check`；它只在显式检查点协作式取消，不会打断睡眠或锁等待。

etcd 模型：

- `EtcdOperation` 标识可注入失败的 `Get`、`Put`、`Delete`、`CompareAndSwap`、`Watch`。
- `EtcdValue` 保存字符串键值和 `mod_revision`；`WatchEvent`/`WatchEventKind` 表达 Put/Delete 事件；`WatchChannel` 包装共享接收端。
- `EtcdState` 用 `BTreeMap` 保存值、用递增 `revision` 维护全局版本、用 `VecDeque` 保存失败队列，并保存精确路径 watcher。
- `EtcdClient::{get,put,compare_and_put,delete_prefix,watch}` 是内存操作；`fail_next` 只让队首且类型匹配的后续操作失败。
- `DeleteKeysWithPrefixFromEtcd`、`PutKVToEtcdMono`、`PutKVToEtcd` 在此模型上提供有界重试；`PutKVToEtcdMono` 使用“读当前修订号，再 CAS”的顺序。

DDL job 与删除范围：

- `JobState`、`SchemaState`、`AdminCommandOperator`、`InvolvingSchemaInfo` 和 `Job` 是仅覆盖本文件需求的精简模型。
- `HasSysDB` 对涉及 schema 做 ASCII 不区分大小写匹配，系统库集合为 `mysql`、`information_schema`、`performance_schema`、`metrics_schema`、`sys`。
- `PauseRunningJob` 拒绝 `Pausing`/`Paused`，只允许 `None`、`Queueing`，以及 `Running && pausable`，成功后写入 `Pausing` 和操作者。
- `DelRangeTask` 保存 `[start_key,end_key)`、`job_id` 和 `element_id`；`Range` 返回键副本。
- `LoadDeleteRanges`、`LoadDoneDeleteRanges`、`CompleteDeleteRange`、`RemoveFromGCDeleteRange`、`RemoveMultiFromGCDeleteRange`、`DeleteDoneRecord`、`UpdateDeleteRange` 操作 `SessionContext` 内的两组记录。

其他工具：

- `LoadGlobalVars`、`GetTimeZone`、`IsRaftKv2` 读取或更新内存会话状态。
- `EmulatorGCEnable`、`EmulatorGCDisable`、`IsEmulatorGCEnable` 操作进程级原子开关。
- `GetInternalResourceGroupTaggerForTopSQL` 返回可把 `RpcRequest.resource_group_tag` 设为 `[0]` 的线程安全闭包；`IsInternalResourceGroupTaggerForTopSQL` 检查该标签。
- `WrapKey2String`、`DecodeHexKey` 与私有 `encode_hex` 处理字节键；`FolderNotEmpty` 做容错式目录探测。
- `IndexColumnInfo`、`IndexInfo`、`TableInfo`、`TableLockInfo`、`SessionInfo`、`TableLockTpInfo` 是本地精简元数据。`SetKeyspaceName` 与 `GenKeyExistsErr` 共同构造重复索引错误。

## 执行流程

`PutKVToEtcdMono` 的主流程是：若 `retry_count == 0` 立即成功；每轮先调用 `CancellationToken::check`，再 `get(key,false)` 读取当前 `mod_revision`（键不存在视为 0），随后调用 `compare_and_put`。CAS 成功即返回；读取错误、CAS 错误或 CAS 冲突都会记录最后错误、按 `retry_interval` 休眠并重试，耗尽次数后返回最后错误。这个流程防止在读取之后发生的并发更新被无条件覆盖，但它不比较值大小，“Mono”指修订号上的条件写而非业务值单调递增。

`PutKVToEtcd` 使用相同的零次重试、取消检查、休眠和最后错误规则，但直接调用 `put`。`DeleteKeysWithPrefixFromEtcd` 不接收取消令牌，只重试 `delete_prefix`；成功时不关心删除数量。

删除范围流程如下：

1. `LoadDeleteRanges`/`LoadDoneDeleteRanges` 选择活动或完成集合，经一次失败注入检查后，只返回 `ts < safe_point` 的任务。
2. `CompleteDeleteRange` 依次模拟 `BEGIN`、可选的 `INSERT IGNORE ... SELECT`、删除活动记录和 `COMMIT` 四个 SQL 边界。
3. 需要记录完成时，它按 `(job_id,element_id)` 查找活动记录，并仅在完成集合没有相同二元组时插入。
4. `UpdateDeleteRange` 还要求 `old_start_key` 匹配才推进起始键，因此可避免陈旧进度无条件覆盖较新的进度。

暂停流程先处理幂等性/重复操作边界：已经 `Pausing` 或 `Paused` 返回 `PausedJob` 且不改变操作者；其他不可暂停状态返回 `CannotPauseJob`；只有检查通过后才同时修改状态与 `admin_operator`。

`GenKeyExistsErr` 在已配置 keyspace、键首字节为 `x` 且长度大于 4 时移除四字节前缀；随后用 `table.name.index.name` 组成索引名。它优先取键第 19 字节之后的内容，否则回退到传入 `value`，按 NUL 或 `|` 拆列，应用前缀索引长度，并对 binary 或非 UTF-8 列输出十六进制，最后用连字符拼接各列展示值。

## 数据与状态

`EtcdClient` 的所有克隆共享一个 `Arc<Mutex<EtcdState>>`。每次成功 Put/CAS 增加一次全局 revision；前缀删除对每个被删键分别增加 revision。`get(prefix=true)` 利用 `BTreeMap` 的稳定顺序过滤，但 API 没有声明分页、压缩 revision、lease 或事务语义。

watcher 按完整字符串路径匹配，不支持前缀 watch。发送端保存在 `EtcdState.watchers`；Put/Delete 时同步发送，接收端已释放导致发送失败时才从列表移除。`WatchChannel` 可克隆，但克隆共享同一个 `mpsc::Receiver` 锁，因此事件由竞争到锁的某一个接收者消费，而不是广播给每个克隆。

`SessionContext` 的克隆同样共享 `Arc<Mutex<SessionState>>`。删除范围、完成范围、全局变量、当前会话变量、Raft 引擎和失败队列都属于该共享内存状态。`SessionVars::default` 为 UTC、可加载、偏移 0、chunk 大小 1024；本文件的删除范围加载并不使用 chunk 大小。

`EMULATOR_GC_ENABLE` 是进程级 `AtomicI32`，默认值为 1；`KEYSPACE_NAME` 是进程级 `Mutex<String>`。两者会跨本 crate 的调用者和测试共享，应在测试结束时恢复，避免串行/并行用例互相污染。

## 依赖与调用关系

下游依赖全部来自标准库：集合用于状态表与失败队列，`Arc`/`Mutex`/atomics 提供共享状态，`mpsc` 提供 watch 通道，`thread::sleep` 支持重试间隔，`fs::read_dir` 支持目录探测。`pkg/ddl/util/Cargo.toml` 没有第三方或工作区依赖。

模块出口是 `pkg/ddl/util/lib.rs`，它同时导出 `dead_table_lock_checker.rs`、本文件和 `watcher.rs`。根 crate 的 `pkg/lib.rs` 再将 `facade_ddl_util::*` 暴露在 `ddl::util` 下。

当前可确认的 Rust 生产调用边包括：

- `pkg/session/runtime/session.rs`、`crossks_runtime.rs` 和 `crossks_owner.rs` 使用 `DDLOwnerKey` 组装 owner 选举路径。
- `pkg/session/runtime/session_factory.rs` 使用 `ServerGlobalState` 注册/观察服务器全局状态；相关 lifecycle 测试也检查该键。
- `pkg/ddl/db_integration_test.rs` 直接引用 `DdlUtilError`。

其余公开函数没有在目标文件和 `util_test.rs` 之外发现同一符号的 Rust 生产调用。相邻子系统存在独立实现：例如 `pkg/ddl/schemaver/syncer.rs` 自己实现真实/抽象 etcd 的 CAS 写入，`pkg/ddl/jobsubmit/submit.rs::job_has_system_schema` 独立实现系统库判断，`pkg/ddl/ingest/util.rs` 独立映射唯一键冲突。它们是 Go 语义的平行移植证据，不是对本文件函数的调用边。

Go 侧则通过 `pkg/ddl/util/util.go` 的真实 `sessionctx.Context`、etcd client、KV/model/table codec 被 DDL、GC 与 schema version 代码广泛调用；两种语言当前接线程度不同。

## 错误处理与边界

- poisoned `Mutex` 统一用 `PoisonError::into_inner` 恢复数据访问，因此线程 panic 后不会继续传播 poison 错误；调用者仍需接受状态可能处于部分更新状态。
- `fail_next` 使用 FIFO 队列，但只有队首操作类型与当前操作一致时才消费；类型不匹配时失败会保留给未来匹配操作。
- 三个 etcd 重试函数在 `retry_count == 0` 时返回 `Ok(())`，即使取消令牌已取消，也不会执行检查。这一细节由 `zero_etcd_retries_match_go_noop_success` 固定。
- `CompleteDeleteRange` 仅模拟事务边界，没有回滚快照。若插入完成记录后，后续失败注入发生在删除或提交步骤，前面状态不会撤销；这与真实 SQL 事务原子性有差距，扩展测试时不能把该内存行为当作完整事务实现。
- `LoadDeleteRanges` 使用严格小于 `safe_point`；等于 safe point 的记录不会返回。
- `GetTimeZone` 只依据预置的 `location_loadable` 标志，不会实际加载时区数据库；不可加载或空名称时返回空名称和固定偏移。
- `IsRaftKv2` 只检查 `raft_engines.first()`；空列表和首项不是 `raft-kv2` 都返回 false。
- `FolderNotEmpty` 将不存在、不可读和空目录统一视为 false，无法区分 I/O 错误。
- `DecodeHexKey` 拒绝奇数长度和非法十六进制；接受大小写数字并返回原始字节。
- `GenKeyExistsErr` 是启发式精简解码，不是 Go 的 `tables.GenIndexValueFromIndex`：19 字节偏移、NUL/竖线拆分和 UTF-8 判定不能覆盖完整 TiDB datum/index codec。解码失败也不会返回独立错误，而是以十六进制显示。

## 并发与资源生命周期

`EtcdClient` 和 `SessionContext` 都通过单个互斥锁串行化各自全部操作。状态读取、修改和 watcher 发送均可能在持锁期间发生；特别是 `notify` 使用无缓冲 `mpsc::Sender::send`，如果已注册 watcher 尚未接收，`put`、CAS 或删除可能阻塞且一直持有 etcd 状态锁。这是内存模型的重要并发限制。

`WatchChannel::recv` 在持有接收器互斥锁时阻塞；同一 channel 的其他克隆无法同时进入 `recv`/`try_recv`。丢弃所有接收端后，发送失败会在下一次匹配事件时清理 watcher；本文件没有显式取消 watch API。

`CancellationToken` 的 Release 写与 Acquire 读保证跨线程可见，但仅 `PutKVToEtcdMono`、`PutKVToEtcd` 和 `IsRaftKv2` 检查它。`retry_pause` 的睡眠不可取消。资源由 `Arc` 和通道析构自动回收，没有后台线程、async task、连接池或外部文件句柄长期存活。

`ResourceGroupTagger` 要求闭包 `Send + Sync`，返回闭包只复制一个固定字节切片。`EMULATOR_GC_ENABLE` 使用 Acquire/Release 顺序；`KEYSPACE_NAME` 的读取与设置都在 mutex 内完成。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/util/util.go`，独立 Go 测试是 `pkg/ddl/util/util_test.go`；Rust 独立测试为 `pkg/ddl/util/util_test.rs`。

保持一致的部分包括：etcd 路径和 TTL 常量；系统库判定；暂停时对 Pausing/Paused 与不可暂停状态的错误分支；删除范围的二元组主键语义；`ts < safePoint`；零次重试不执行操作而成功；CAS 写入的大致顺序；空键格式化为 `''`、非空键格式化为 `0x...`；目录不存在/为空返回 false；模拟器 GC 默认启用；内部资源标签为单字节 0。

Rust 对 Go 的主要替代和差异如下：

- Go 使用真实 `model.Job`、`sessionctx.Context`、etcd v3 client、SQL record set、KV key 与 table/index codec；Rust 在本文件中定义精简结构和内存后端，Cargo 也没有对应依赖。
- Go 的 etcd 操作为每次请求创建超时 context，并记录日志与重试指标；Rust 使用调用方给定的 sleep 间隔，没有请求超时、日志、指标、lease 或操作选项。
- Go `CompleteDeleteRange` 依赖数据库事务；Rust 的失败注入可以暴露部分更新状态。
- Go `GetTimeZone` 实际调用 `time.LoadLocation` 并在失败时计算当前 offset；Rust 读取预设的名称可加载标志和固定 offset。
- Go `IsRaftKv2` 执行内部 SQL、关闭 record set 并 drain rows；Rust 检查内存列表首项。
- Go `GenKeyExistsErr` 使用真实索引 codec，并在解码失败时记录日志、回退为基于 key 的错误；Rust 采用简化分隔规则和本地错误枚举。
- Rust 新增 `DecodeHexKey` 作为公开辅助函数；Go 同文件在加载删除范围时直接使用 `hex.DecodeString`，没有同名公共 API。

测试对应关系也不完整：两边都覆盖 `FolderNotEmpty`、`HasSysDB`、`PauseRunningJob`；Rust 另测零次 etcd 重试。Go 的真实 CAS 并发语义在 `pkg/ddl/schemaver/syncer_test.go::TestPutKVToEtcdMono`，而当前 Rust schema-version crate 在自己的 `syncer_test.rs::monotonic_put_path_preserves_values_and_context_errors` 验证其独立实现，不直接调用本文件的 `PutKVToEtcdMono`。

## 扩展指南

新增或修改 etcd 行为时，应先决定目标是扩展本文件的内存契约，还是接入 `pkg/ddl/schemaver` 的真实 transport。若修改本文件，应同步维护 `EtcdOperation` 失败注入、revision 更新、watch 通知、零次重试和取消边界，并在 `pkg/ddl/util/util_test.rs` 增加独立测试；不要把测试写回 `util.rs`。

扩展 GC 删除范围时，最可能修改 `DeleteRangeRecord`、`SessionState` 和 Load/Complete/Remove/Update 系列函数。必须明确事务失败后的状态、`safe_point` 比较、记录唯一键和 start-key 条件更新；若目标是生产接线，还需对照 `pkg/store/gcworker/gc_worker.rs` 的 runtime trait，而不是仅完善内存列表。

扩展暂停规则时，应同时检查 `job_is_pausable`、`PauseRunningJob` 和 `pkg/ddl/jobsubmit/submit.rs::job_has_system_schema` 等平行实现，并把新的 `JobState`/`SchemaState` 分支加入 `util_test.rs`。系统库集合变化也要同步 Go 的 `metadef.IsSystemRelatedDB` 语义。

扩展索引冲突解码时，优先复用或建立真正的 Rust index datum codec；当前 `GenKeyExistsErr` 的字节偏移和分隔符策略只适合已建模输入。应为 keyspace 前缀、多列、前缀索引、binary、非法 UTF-8、短键和空列分别补独立测试，并与 `pkg/ddl/ingest/util.rs` 的错误映射保持一致。

全局状态修改需考虑测试隔离：修改 `KEYSPACE_NAME` 或模拟器 GC 开关的测试应保存并恢复原值。watch 并发扩展则需先处理持锁阻塞发送问题；若需要广播、前缀监听或取消，应改变数据结构并加入多线程生命周期测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ddl/util` 确认本 crate 的 Rust/Go 实现和独立测试文件。
- RustCodeGraph `node --file pkg/ddl/util/util.rs --offset ...`：逐段核对了本文件 1,117 行源码、130 个符号及所有常量、类型、函数和 impl。
- RustCodeGraph `query`：区分了 Rust/Go 的 `HasSysDB`、`PutKVToEtcdMono`、`GenKeyExistsErr` 同名符号；通用 `explore/callers/callees` 对该大仓库的同名结果不够精确，因此生产接线另以限定 Rust 文件的符号搜索核验。
- crate/出口证据：`pkg/ddl/util/Cargo.toml`、`pkg/ddl/util/lib.rs`、根 `Cargo.toml` 的 `facade_ddl_util`、`pkg/lib.rs` 的 `ddl::util` 再导出。
- Go 对照证据：`pkg/ddl/util/util.go`、`pkg/ddl/util/util_test.go`、`pkg/ddl/schemaver/syncer_test.go::TestPutKVToEtcdMono`。
- Rust 测试与相邻实现证据：`pkg/ddl/util/util_test.rs`、`pkg/ddl/schemaver/syncer.rs`、`pkg/ddl/schemaver/syncer_test.rs`、`pkg/ddl/jobsubmit/submit.rs`、`pkg/ddl/ingest/util.rs`、`pkg/store/gcworker/gc_worker.rs`。
- 已人工复核：文档明确回答了该文件的 crate 位置、当前生产接线范围、每组工具如何运行、共享状态和失败边界、Go 移植差异及安全扩展位置；未把公开 API 或 Go 调用关系误写成 Rust 已接线行为。
