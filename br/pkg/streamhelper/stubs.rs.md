# `br/pkg/streamhelper/stubs.rs`

## 文件定位

本文件属于 `astersql-br-pkg-streamhelper` library crate。crate 根 [`lib.rs`](lib.rs) 以 `pub mod stubs` 挂载，并以 `pub use stubs::*` 把公开类型提升到包级 API；[`Cargo.toml`](Cargo.toml) 将该 crate 对应到 Go 包 `br/pkg/streamhelper`，porting lane 为 2。

它不是某个 Go 同名文件的逐行移植，而是 Rust 迁移期集中设置的外部边界层：用本地结构表示 `kv.KeyRange`、`metapb.Region/Peer`、`backuppb.StreamBackupTaskInfo` 和 log-backup RPC 消息，用 trait 表示 TiKV log-backup 与 etcd 能力，并提供只供测试或 slim 路径使用的 `MemEtcd`。文件头明确声明真实 gRPC/etcd 客户端尚未在此接入，因此这些类型不能被理解为完整协议实现。

## 核心职责

1. 为 region 扫描、checkpoint 收集和任务元数据提供最小数据模型，包括 `KeyRange`、`Region`、`Peer`、`StreamBackupTaskInfo` 及请求/响应结构。
2. 以 `LogBackupClient`、`LogBackupService` 和 `EtcdKV` trait 隔离网络/存储边界，使生产逻辑依赖能力而不是具体客户端。
3. 为元数据请求提供可取消、带 deadline 的 `WatchContext`/`MetadataRequestContext`，并通过 `MetadataRequestError` 保留“超时、服务不可用、其他错误”的重试身份。
4. 用 `MemEtcd` 模拟 revision、历史回放、前缀 watch 和 progress notification，支撑独立 Rust 测试，而不启动真实 etcd。
5. 用 `key_next` 实现 Go `kv.Key.Next()` 的字节语义，供 `regioniter.rs::locateKeyOfRegion` 构造只覆盖目标 key 的扫描区间。

## 主要符号

- 基础值对象：`KeyRange` 与 `Entry` 表示半开键区间和 KV 条目；`RegionEpoch`、`Region`、`Peer` 只保留 region 定位、epoch 与 leader store 所需字段。`Region::{GetId, GetEndKey, GetRegionEpoch}`、`Peer::GetStoreId` 模拟 protobuf getter。
- 任务模型：`StorageBackend`、`StreamBackupTaskInfo`、`StreamBackupError` 可由 serde 编解码；`StreamBackupError::{Marshal, Unmarshal}` 把 JSON 错误转换为 `String`。
- log-backup 边界：`RegionIdentity`、`GetLastFlushTSOfRegionRequest/Response`、`RegionError`、`RegionCheckpoint` 描述按 region 查询 flush TS；`FlushEvent` 描述订阅流事件；`LogBackupClient` 与 `LogBackupService` 分别抽象单 store RPC 和按 store 获取/清理客户端缓存。
- etcd watch 模型：`WatchEventType::{Put, Delete, Progress}`、`WatchEvent`、`RevisionedValue` 携带事件、修改 revision 和读取时观察到的全局 revision。
- 请求生命周期：`WatchContext` 共享父级取消标志；`MetadataRequestContext` 叠加单次请求取消标志与 deadline；`MetadataRequestError` 区分 `DeadlineExceeded`、`Unavailable`、`Other`。
- `EtcdKV`：规定点写、点读、删除、前缀读删、带 revision 读取、watch、progress 请求和 watcher 重置。三个 `*WithRequestContext` 默认实现会在底层调用前后检查取消/deadline。
- `MemEtcd`/`MemEtcdState`/`publish`：进程内实现值表、单调 revision、事件历史和 watcher 列表。
- `key_next(key) -> Vec<u8>`：复制 key 后追加 `0x00`。

## 执行流程

checkpoint 收集链中，[`collector.rs`](collector.rs) 把 `RegionWithLeader` 转成 `RegionIdentity { Id, EpochVersion }`，按 leader store 组装 `GetLastFlushTSOfRegionRequest`，通过 `LogBackupService::GetLogBackupClient` 获取 `LogBackupClient` 并调用 `GetLastFlushTSOfRegion`。响应中带 `Err` 的 region 被放回失败区间；成功项参与最小 checkpoint 计算。RPC 失败时收集器调用 `ClearCache`，让后续连接重建。

flush 订阅链中，[`flush_subscriber.rs`](flush_subscriber.rs) 获取同一 trait 客户端并调用 `SubscribeFlushEvents`。trait 默认返回 `Unimplemented`，所以只有显式覆盖该方法的适配器/测试夹具才具备订阅能力；该默认值是能力探测边界，不代表已接真实 TiKV 流。

元数据链中，[`client.rs`](client.rs) 让 `MetaDataClient` 持有 `Arc<dyn EtcdKV>`；[`advancer_cliext.rs`](advancer_cliext.rs) 先以 `GetPrefixWithRevision` 获取一致快照，再从 `revision + 1` 建立任务与暂停前缀 watch。写入/删除经 `MemEtcd` 增加 revision、更新值表、记录历史并调用 `publish`；新 watcher 创建时先回放 `ModRevision >= 起始 revision` 的匹配历史，再登记实时 sender。`RequestWatchProgress` 向所有仍存活 watcher 发送当前 revision 的 `Progress` 事件。

单次元数据请求由 `MetadataRequestContext::check` 在调用前后检查父级取消、本次取消和 deadline；上层 `runMetadataRequestWithRetry` 只重试 `DeadlineExceeded`/`Unavailable`。region 定位链则由 [`regioniter.rs`](regioniter.rs) 调用 `key_next`，执行 `RegionScan(key, key\0, 1)`。

## 数据与状态

`MemEtcdState.values` 以原始 `Vec<u8>` 键保存 `(value, mod_revision)`，因此 `PutBytes` 可无损承载非 UTF-8 range start key；trait 默认 `PutBytes` 会尝试 UTF-8 转换，但 `MemEtcd` 覆盖了该方法。`revision` 只在实际 put 或存在键的 delete 时递增；删除不存在的键不产生事件。`history` 保留全部 put/delete 事件且无压缩策略，`watchers` 保存 `(prefix, start_revision, sender)`。

`Get` 对不存在的键返回空 `Vec`，而 `GetWithRevision` 以 `Value: None`、`ModRevision: 0` 区分缺失，并总是返回当前全局 `Revision`。前缀读取按键排序以保证测试确定性。`WatchContext` 的 clone 共享同一个 `AtomicBool`；每个 `MetadataRequestContext` 共享自己的请求取消标志，但同时引用父 context。

消息结构大多拥有 `Vec`/`String`，没有借用调用方缓冲区。`StreamBackupTaskInfo` 与 `StreamBackupError` 使用 JSON，而 Go 生产实现中的 backuppb 是 protobuf 类型；这是当前桩层的持久化差异。

## 依赖与调用关系

文件直接依赖标准库 `HashMap`、`Arc`、`Mutex`、原子量、`mpsc` 和 `Duration`，以及 [`Cargo.toml`](Cargo.toml) 声明的 `serde`、`serde_json`。crate 根对外再导出全部公开符号。

RustCodeGraph 的目标文件节点记录 84 个符号，并列出 7 个直接使用文件，包括生产文件 `advancer.rs`、`advancer_cliext.rs`、`advancer_env.rs`、`collector.rs`、`regioniter.rs`、`client.rs`/`prefix_scanner.rs` 中的边界消费，以及对应测试引用。源码核验得到的关键边为：`collector.rs -> LogBackupService/LogBackupClient -> GetLastFlushTSOfRegion`；`flush_subscriber.rs -> SubscribeFlushEvents`；`advancer_cliext.rs/client.rs -> EtcdKV/WatchContext/MetadataRequestContext`；`regioniter.rs -> key_next`；`models.rs -> KeyRange/StorageBackend/StreamBackupTaskInfo`。

`advancer_env.rs::Env` 把 `LogBackupService` 与集群元数据、流元数据、锁解析、flush 间隔能力组合起来；因此本文件是 advancer 与外部 TiKV/etcd 协议之间的类型与 trait 接缝，而不是 advancer 算法本身。

## 错误处理与边界

该层统一以 `Result<_, String>` 表示多数底层错误，仅元数据超时/重试路径使用可判别的 `MetadataRequestError`。`Display` 把 deadline 固定为 `context deadline exceeded`，而 `Unavailable`/`Other` 直接输出内部消息。serde 编解码、UTF-8 转换和 channel send 错误均降为字符串，错误类型信息不会继续保留。

`MetadataRequestContext::check` 优先报告父 watch 取消为 `Other("watch canceled")`，随后才检查本请求取消或过期并返回 `DeadlineExceeded`。`EtcdKV` 的 context 默认实现只能在同步底层调用前后检查；若真实适配器会阻塞，必须覆盖这些方法并让传输层主动响应取消，不能依赖默认实现提供硬超时。

`MemEtcd` 的所有锁都用 `unwrap`，锁中毒会 panic；它也不模拟 etcd 事务、lease、压缩、权限、网络错误或 watch 缓冲背压。历史无限增长，不能用于长时间生产服务。`WatchPrefix` 的历史发送发生在持锁期间；若接收端已断开则返回错误，watcher 不会登记。`LogBackupClient::SubscribeFlushEvents` 默认失败，`EtcdKV::ResetWatcher` 默认失败，调用方必须正确处理“不支持”。

## 并发与资源生命周期

`WatchContext` 和请求 context 用 Acquire/Release 原子序共享取消状态。`MemEtcd` 的值、revision、历史和 watcher 注册统一受一个 `Mutex` 保护，因此每次内存操作呈串行顺序；`publish` 在同一临界区记录历史并发送事件，保证值更新与事件 revision 的对应关系。

watch 使用标准库无界 `mpsc`。`publish` 与 progress 通知通过 `retain` 删除发送失败的 watcher；receiver 被丢弃后，下一次发布/进度请求才清理 sender。`ResetWatcher` 立即清空所有 sender，使 receiver 观察到断开。该文件不创建线程；消费线程由 `advancer_cliext.rs`、`collector.rs` 和 `flush_subscriber.rs` 管理。

`LogBackupClient`、`LogBackupService`、`EtcdKV` 都要求 `Send + Sync`，允许通过 `Arc<dyn Trait>` 跨 worker 共享。`MemEtcd::clone` 共享同一状态而非复制快照。`history` 与无界 channel 没有容量限制，规模风险只在测试范围内可接受。

## 与 Go 版本的对应关系

本文件综合对应多个 Go/外部类型边界：[`regioniter.go`](regioniter.go) 使用 `metapb.Region/Peer`、`kv.KeyRange` 和 `kv.Key(key).Next()`；[`collector.go`](collector.go) 使用 `logbackuppb` 的请求、region identity、checkpoint 与 region error；[`advancer_env.go`](advancer_env.go) 定义 `LogBackupService`；[`client.go`](client.go) 直接包装 `clientv3.Client`；[`flush_subscriber.go`](flush_subscriber.go) 使用真实 `SubscribeFlushEvent` gRPC stream。

Rust 保留了 Go/protobuf 风格字段和 getter 名以降低移植差异，但能力明显更窄：没有 protobuf unknown fields/完整 getter，没有 Go `context.Context` 参数贯穿所有 RPC，没有 etcd transaction/lease/compaction，也没有真实 gRPC 客户端。Go `MetaDataClient` 使用 etcd transaction 原子提交多键任务，Rust `MemEtcd` 是逐操作内存模拟；不得据此推断生产等价的事务保证。

已验证的语义对齐包括：key-next 追加零字节、region checkpoint 批处理与错误标记、失败后清客户端缓存、revision watch 从指定版本回放、progress 事件、取消/deadline 重试身份，以及二进制 range key 不经 UTF-8。JSON 替代 protobuf、字符串错误和内存 watch 无压缩属于明确迁移差异。

## 扩展指南

- 接入真实 TiKV/etcd 时，应实现现有 trait 或在独立适配层替换桩类型，并保持 `collector.rs`、`advancer_cliext.rs`、`flush_subscriber.rs` 的调用契约；不要把网络逻辑塞进 `MemEtcd`。
- 扩展 log-backup 消息时，同步检查 `collector.rs`、`flush_subscriber.rs`、`basic_lib_for_test.rs`、`collector_test.rs` 和 `parity_test.rs`。新增 region 错误类别必须明确上层是重扫、重连还是终止。
- 扩展 `EtcdKV` 时必须定义 revision、历史回放、取消、超时与 watcher 重置语义；真实阻塞 I/O 要覆盖 `*WithRequestContext`，并为 deadline/unavailable/非重试错误分别增加 `advancer_cliext_test.rs` 用例。
- 修改 `MemEtcd` 时，回归放在独立测试文件：二进制键在 `models_test.rs`，任务 CRUD 在 `client_test.rs`/`integration_test.rs`，watch、compaction/retry/cancel 在 `advancer_cliext_test.rs`，公共 Go/Rust 契约在 `parity_test.rs`。不要把测试嵌入 `stubs.rs`。
- 若把 JSON stub 换成真实 protobuf，需要设计已有 Rust 测试数据/持久化值的兼容策略，并核对默认值、缺失字段和错误编码；这比机械替换类型名范围更大。

主要正确性风险是 revision/watch 边界与取消竞态；兼容性风险是字段/序列化格式偏离 Go protobuf；性能风险是 `MemEtcd` 在单锁内发送、无限历史与无界 channel。它们是桩后端限制，不应被优化成新的生产子系统而扩大当前文件职责。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter br/pkg/streamhelper/stubs.rs` 确认目标已索引；`node --file ... --offset ...` 读取全部 529 行并报告 84 个符号、7 个直接使用文件。`query MemEtcd --kind struct` 精确定位结构；通用 `explore`/`callers` 因同名符号产生跨仓库噪声或超时，调用边改由已索引文件节点与下列直接引用交叉验证。
- crate 与 Rust 生产路径：`br/pkg/streamhelper/Cargo.toml`、`lib.rs`、`collector.rs`、`advancer_env.rs`、`advancer_cliext.rs`、`client.rs`、`flush_subscriber.rs`、`regioniter.rs`、`models.rs`、`prefix_scanner.rs`。
- Go 对照：`br/pkg/streamhelper/regioniter.go`、`collector.go`、`advancer_env.go`、`client.go`、`flush_subscriber.go`；真实消息/etcd 类型来自这些文件的 `kvproto`、`backuppb`、`clientv3` imports，而仓库内没有独立的 Go `stubs.go`。
- Rust 独立测试：`models_test.rs` 验证任意二进制 range key；`collector_test.rs` 验证批次、尾批和 region error；`advancer_cliext_test.rs` 验证 watch revision、progress、超时/不可用重试、非重试错误、取消与 watcher reset；`integration_test.rs` 以共享 `MemEtcd` 覆盖任务生命周期；`parity_test.rs::go_rust_public_contract_matches` 串联公开契约；`basic_lib_for_test.rs` 提供 log-backup trait 假实现。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前执行任务指定结构命令，确认目标文件存在且固定二级标题恰为 11 个，并人工复核未把 stub 能力、事务语义或真实网络接入写成现状。
