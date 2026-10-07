# `pkg/ddl/schemaver/syncer.rs`

## 文件定位

本文件是 `astersql-ddl-schemaver` crate 的 etcd 版 schema 版本同步协议实现。crate 入口 `pkg/ddl/schemaver/lib.rs` 将本模块与 `mem_syncer.rs` 一并重导出；`pkg/ddl/schemaver/Cargo.toml` 声明直接依赖 `etcd-client`、`tokio` 和 `astersql-domain-serverinfo`。生产环境由 `pkg/session/runtime/session_factory.rs` 把 `serverinfo::RealEtcdClient` 包装为本文件的 `RealEtcdClient`，再通过 `NewEtcdSyncer` 建立协议对象；内存后端则用于测试和不接真实 etcd 的运行路径。

在完整 DDL 链路中，它处于“owner 发布版本/等待全员追上”和“follower 观察版本/上报本机进度”的协调边界。owner 侧 `pkg/ddl/schema_version.rs::NormalDdlSchemaBarrier::{wait,recover}` 调用 `OwnerUpdateGlobalVersion` 与 `WaitVersionSynced`；info schema 同步器由 `pkg/infoschema/issyncer/syncer.rs::InitRequiredFields` 接收同一个 `Syncer`，并同步 MDL/next-gen 开关。它不执行 DDL 元数据变更，也不加载 info schema；它只传递版本、跟踪参与节点并提供等待屏障。

## 核心职责

1. 定义 `Syncer` 协议，使调用方只依赖初始化、版本发布/上报、watch、会话恢复、等待和关闭能力，而不依赖具体 etcd 客户端。
2. 维护三类 etcd 路径：全局版本 `DDLGlobalSchemaVersion`、非 MDL 节点版本前缀 `DDLAllSchemaVersions`、MDL 按 job 隔离的版本前缀 `DDLAllSchemaVersionsByJob`。
3. 在非 MDL 模式轮询所有节点版本；在 MDL 模式结合 serverinfo 的在线实例集合和按 job watch 缓存，只在目标实例都达到 `latestVer` 后解除 owner 屏障。
4. 管理带 TTL 的 etcd session、节点版本键的 lease、watch 重建和关闭清理，使异常退出的节点最终从参与集合中消失。
5. 通过 `EtcdClient` 同时支持真实 etcd (`RealEtcdClient`) 与确定性的进程内模型 (`MemoryEtcdClient`)；后者还实现 revision/history/watch 和失败注入，支撑独立 Rust 测试。
6. 提供 Go `context.Context`、session done channel 和一次性匹配回调在 Rust 中的对应物：`Context`、`DoneSignal`、`Session`、`nodeVersions`。

## 主要符号

- 常量与全局开关：`InitialVersion` 为初始值 `"0"`；`keyOpDefaultRetryCnt` 为有限重试次数；`putKeyRetryUnlimited` 表示重试到上下文结束；`checkVersInterval` 为非 MDL 轮询间隔；`SessionTTL` 为租约 TTL。`SetMDLEnabled`、`SetNextGen` 控制两条协议分支，其他 `SetMock*` 和 `MOCK_*` 主要用于测试注入。
- `SyncError`：本 crate 的字符串错误边界；底层 etcd、serverinfo 和 runtime 错误在进入本模块时转为它。
- `Context`：共享取消状态并可携带 deadline、父上下文和 scheduler done check。`Child` 拥有独立取消位但继承父结束；`WithTimeout` 缩短而不延长既有 deadline；`Wait` 用 `Condvar` 等待且尊重取消/超时；`Cancelled` 与 `DeadlineExceeded` 刻意区分 owner 退出和租约截止。
- `DoneSignal`、`Session`、`SessionCleanup`：分别表示一次性关闭信号、lease 生命周期和幂等后端清理。`Session::WithLease` 允许真实后端注册 revoke/abort 回调，最后一个清理对象析构时仍会执行清理。
- `EventType`、`KeyValue`、`WatchResponse`、`WatchChan`、`GetResponse`：隔离具体 etcd crate 的 KV/watch 数据模型。
- `EtcdClient`：同步器所需最小后端接口，包含 session、CAS 创建、普通/单调写、读取、删除与从 revision 开始的 watch。
- `MemoryEtcdClient`：用 `RwLock<BTreeMap<...>>`、原子 revision、历史事件和订阅表模拟 etcd；`PutMono` 以修改 revision 做一次 CAS，`Watch` 在持有 history 锁时完成历史回放和注册，避免两阶段之间漏事件。
- `Watcher`：保存全局版本 watch；`Watch/Rewatch` 会取消旧子上下文并替换通道，析构时也取消当前订阅。
- `SyncSummary`：报告参与服务器数及 assumed 服务器数。
- `Syncer`：面向 owner/follower 的公共 trait。`impl Syncer for etcdSyncer` 只是将 trait 方法转发到同名固有方法。
- `nodeVersions`：每个 job 的 `node ID -> schema version` 表及至多一个 `MatchFn`。只有 `add` 可能消费成功回调；删除不会触发成功判定。
- `etcdSyncer` / `NewEtcdSyncer`：协议主体及构造器，持有本机路径、后端、当前 session、全局 watch、DDL ID、按 job 缓存和可选 serverinfo 同步器。
- `decodeJobVersionEvent`：解析 `{jobID}/{nodeID}` 后缀；PUT 还必须解析版本，DELETE 将版本视为 0。
- `calculateUpdatedMap`：按 `ip:port` 去重 serverinfo，保留 `StartTimestamp` 最大的 ID，并计算 assumed 数量。
- `RealEtcdClient`：带 namespace 的真实 etcd 适配器；内部 tokio runtime 执行异步 API 并将其呈现为同步 trait。

## 执行流程

初始化时，`etcdSyncer::Init` 先用 `PutIfAbsent` 保证全局版本键存在但不覆盖已有值，随后用 `newSession` 获取 TTL 为 `SessionTTL` 的 session，订阅全局版本键，并把本机非 MDL 版本路径以初始值绑定到 session lease。生产构造顺序可在 `pkg/session/runtime/session_factory.rs::{create_with_server_info,install_serving_ddl_runtime}` 中看到。

follower 上报由 `UpdateSelfVersion` 完成。MDL 关闭时，它把版本无限重试写到 `selfSchemaVerPath` 并绑定当前 lease；MDL 开启时，`jobID == 0` 是明确的空操作，否则用 `PutMono` 将版本写到 `/all_schema_by_job_versions/{jobID}/{ddlID}`，有限重试且不绑定 session lease。

owner 先通过 `OwnerUpdateGlobalVersion` 无限重试发布全局版本，再通过 `WaitVersionSynced` 等待：

- 非 MDL：先等待 `CheckVersFirstWaitTime`，随后按前缀读取所有节点版本。已达到目标的键进入本轮调用的 `updated` 缓存，不再重复解析；任何未达到目标或读取失败都会继续轮询，直到全部已观察键达到目标或 `Context` 结束。
- MDL：`getServersForISSync` 从 serverinfo 枚举应参与的在线实例；next-gen 且 `checkAssumedSvr == false` 时排除 assumed 实例。`calculateUpdatedMap` 去除同地址旧进程，`waitVersionSyncedWithMDL` 给该 job 安装一次性回调，检查每个目标 ID 的版本是否存在且不低于目标。成功由容量为 1 的通知通道唤醒；每次内部等待最多一秒，超时会清除回调并让外层重新枚举服务器，从而适应成员变化。

MDL 缓存由 `SyncJobSchemaVerLoop` 驱动。每轮 `syncJobSchemaVer` 先做前缀快照、清空旧值并回收无数据无回调的 job，再处理快照，然后从 `response.Revision + 1` 建立 watch，避免快照与增量之间的空窗。PUT 调用 `nodeVersions::add`，DELETE 删除节点并回收空 job；watch 错误、断开或 compaction 会结束本轮，外层等待一秒后重新做全量同步。

session 失效后，`Restart` 创建新 session 并重新发布本机初始版本；`Done` 暴露当前 session 的关闭状态。`Close` 删除本机非 MDL 版本键、关闭 session 并取消全局 watch。真实后端的 `NewSession` 同时启动 keep-alive/响应任务；任务退出会关闭 done 信号，显式关闭或清理对象析构会 abort 任务并在超时保护下 revoke lease。

## 数据与状态

etcd 是跨进程事实源。全局键存 owner 发布的目标版本；非 MDL 路径的每节点键受 session lease 保护；MDL 路径按 job/节点拆分，其清理由上层 `NormalDdlSchemaBarrier::clean_mdl` 在 job 同步后完成。`etcdSyncer` 内的 `jobNodeVersions` 只是 watch 派生缓存，可在 compaction、错误或重启后由全量 Get 重建。

进程内状态分为几层：原子量保存全局模式和测试注入；`RwLock<Arc<Session>>` 支持无破坏地替换 session；`Mutex<HashMap<i64, Arc<nodeVersions>>>` 保护 job 表；每个 `nodeVersions` 再以自身 mutex 保护版本表和一次性回调。锁的嵌套路径很短，但扩展时仍应保持“先 job 表、后单 job 状态”的现有顺序，避免反向加锁。

`MemoryEtcdClient` 的 `values` 与 `revision` 必须描述同一个提交，所以写入时先取得 values 写锁再递增 revision；`Get` 在 values 读锁内同时形成 KV 快照并读取 revision。history 锁跨越历史记录与通知/注册的关键区间，保证从指定 revision 建立 watch 时不漏提交。

版本判定是不小于而非严格等于：节点版本 `>= latestVer` 即视为已同步。MDL 参与集合以 server ID 为键，但同一 `ip:port` 的多个 ID 只保留启动时间最新者；`SyncSummary` 的 `ServerCount` 因此是去重实例数。

## 依赖与调用关系

上游生产调用主要有三组：

- `pkg/session/runtime/session_factory.rs` 创建 `RealEtcdClient`/`NewEtcdSyncer`、调用 `Init`，并将对象装配进 serving DDL runtime 或跨 keyspace manager。
- `pkg/ddl/schema_version.rs::NormalDdlSchemaBarrier` 在 owner 执行/恢复 DDL job 时调用 `OwnerUpdateGlobalVersion`、`WaitVersionSynced`，并在完成后清理 MDL SQL 行和按 job etcd 键。
- `pkg/infoschema/issyncer/syncer.rs::InitRequiredFields` 注入 `Arc<dyn Syncer>`；`configure_protocol` 把 session 变量的 MDL 开关和内核 next-gen 模式传给本模块。follower 通过全局 watch 感知新版本，加载完成后再调用上报接口。

下游依赖为：`etcd-client` 的 KV、事务、lease 与 watch API；`tokio` runtime/任务/定时器；`astersql-domain-serverinfo::Syncer` 的在线节点枚举及 `RealEtcdClient` 的连接、TLS 和 namespace 策略。crate manifest 中 `cfg(any())` 下的旧 Go 移植依赖当前永不启用，不能把它们视为运行时依赖。

RustCodeGraph 对目标文件给出 77 个引用文件，并确认 `WaitVersionSynced` 下游包括 `waitVersionSyncedWithMDL`、`isUpdatedLatestVersion`、`EtcdClient::Get` 与上下文等待；由于同名 trait/Go/Rust 符号较多，生产上游边又以精确文本搜索和上述装配文件核验。

## 错误处理与边界

所有可恢复后端错误最终表现为 `SyncError`。`putRetry` 根据调用点选择一次、三次或无限重试，但无限重试仍必须在 `Context` 结束时退出。`newSession` 每次失败间隔 200ms；真实 etcd 单次同步调用由 `RealEtcdClient::run` 再限制为最多一秒，并每 20ms 检查取消。

非 MDL `WaitVersionSynced` 会忽略一次 Get 错误并继续等待；MDL 的 serverinfo 枚举错误则直接返回，因为没有可靠参与集合。watch 的通道错误、取消、断连和 compaction 都不尝试在原流内修补，而是退出到全量重建循环。格式错误的 job KV 被 `handleJobSchemaVerKV` 静默忽略；`decodeJobVersionEvent` 接受空 node ID（这与 Go 的 `strings.Split/TrimPrefix` 行为一致），调用者不能假设 ID 非空。

锁中毒使用 `expect`，因此属于进程内不变量破坏而非可恢复业务错误。`Close` 对删除本机路径的错误采取 best-effort，随后仍关闭 session/watch；这与 Go 版本记录错误但不中断关闭的意图一致，不过 Rust 当前不接指标/结构化日志。

需特别注意空集合语义：非 MDL 前缀 Get 若返回零键，会立即成功并报告 0 个服务器；MDL 回调明确拒绝空 `versions`。这两种行为均来自当前实现，修改前必须用集群成员/lease 语义证明不会提前越过屏障。

## 并发与资源生命周期

`Context::Cancel` 通过原子位和 `Condvar::notify_all` 唤醒等待者；子上下文取消不影响父上下文，父取消与 deadline 会传递给子。`Watcher::Rewatch` 只取消旧订阅的子上下文，不会取消调用方上下文，相关不变量由 `crossks_align_schema_protocol_uses_backend_sessions_and_recovers` 验证。

`nodeVersions::matchOrSet` 在同一 mutex 临界区内先对现有数据匹配，失败才保存回调；后续 `add` 取出回调执行，失败再放回，成功则永久消费。当前回调在持锁状态执行，因此扩展回调必须短小、不可反向调用同一 `nodeVersions`，也不应执行阻塞 I/O。

真实 session keep-alive 是 tokio 后台任务：按约 TTL/3 发送 keep-alive，同时监听响应、外部上下文和 done 信号。`SessionCleanup` 用 `AtomicBool::swap` 保证 revoke/abort 至多一次；`Drop` 是兜底，不应依赖析构时机完成业务可见同步。

`MemoryEtcdClient` 的 watch 使用标准 mpsc；失去 receiver 的注册会在下一次通知时被清除。`WatchChan` 把 receiver 包在 mutex 中，因此克隆通道是共享同一消费端，不是广播给多个消费者。生产 watch 后台任务在发送端断开或上下文结束时退出。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/schemaver/syncer.go`，Rust 基本保留同名结构和流程：`Syncer`、`nodeVersions`、`etcdSyncer`、`Init/Restart/UpdateSelfVersion/OwnerUpdateGlobalVersion/WaitVersionSynced`、MDL job watch、事件解码、实例去重及关闭清理均有一一对应入口。`pkg/ddl/schemaver/Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向该 Go 包。

关键保持项包括：全局键只在不存在时初始化；非 MDL 上报绑定 lease 且无限重试；MDL `jobID == 0` 跳过；MDL 写使用单调 CAS；快照后从 revision+1 watch；compaction 后全量重建；按 `ip:port` 保留最新启动实例；next-gen 可排除 assumed server；等待判定使用 `>= latestVer`。

已确认的实现差异：Rust 自建 `Context`、watch/channel 和同步 `EtcdClient` 抽象，并在 `RealEtcdClient` 内桥接 tokio；Go 直接使用 clientv3、concurrency session、failpoint、metrics 与 zap。Go 的非 MDL Get 失败会立即继续循环而不 sleep，Rust 当前同样忽略错误，但随后走 20ms `Context::Wait`；Rust 的取消等待因而更可控。Go 会周期打印未同步节点并记录指标，Rust 的 `isUpdatedLatestVersion` 保留参数形状但没有日志/指标。Rust `Close` 还显式关闭 session 和 watcher，而 Go 此文件只删除 self path。Rust 的 `MemoryEtcdClient`、`SessionCleanup` 和真实适配层是移植所需的额外基础设施，不是 Go 文件中的独立同名实现。

独立测试对应关系：`syncer_test.rs::etcd_syncer_simple_flow_matches_go` 对齐 Go `TestSyncerSimple`；`monotonic_put_path_preserves_values_and_context_errors` 对齐 `TestPutKVToEtcdMono`；`syncer_nokit_test.rs` 分别对齐 Go `TestNodeVersions`、`TestDecodeJobVersionEvent`、`TestSyncJobSchemaVerLoop`、`TestCalculateUpdatedMap` 与 `TestGetServersForISSync`。Rust 另有后端 session 恢复、幂等 cleanup、子上下文 deadline 和 scheduler 取消传播测试。

## 扩展指南

- 新增同步协议操作：先扩展 `Syncer` trait，再同时修改 `etcdSyncer` 转发实现和 `mem_syncer.rs`；调用方应继续持有 `Arc<dyn Syncer>`，不要绕过协议直接操作具体类型。
- 新增 etcd 能力：保持 `EtcdClient` 为最小边界，并同步实现 `MemoryEtcdClient` 与 `RealEtcdClient`。涉及 snapshot/watch 时必须维持“同一快照 revision + 从 revision+1 订阅”的无漏事件不变量。
- 改变版本路径或编码：同步修改常量、`UpdateSelfVersion`、`decodeJobVersionEvent`、`NormalDdlSchemaBarrier::clean_mdl`，并兼容滚动升级期间新旧节点共存；路径是跨版本协议，不能只改一端。
- 改变 MDL 等待：重点审查 `waitVersionSyncedWithMDL`、`calculateUpdatedMap`、`getServersForISSync` 和 `nodeVersions`，验证节点重启去重、assumed keyspace、成员动态变化、空集合及超时重枚举。
- 改变 session/取消：同步检查 `Context::{Child,WithTimeout,Cancelled,DeadlineExceeded}`、`Session::WithLease`、`Watcher::Rewatch` 和 `RealEtcdClient::{run,NewSession,Watch}`；owner 退休必须中断等待，不能被误判为允许非 MDL fallback 的 deadline。
- 测试必须放在独立文件。纯协议流程与真实后端抽象测试扩展 `pkg/ddl/schemaver/syncer_test.rs`；无需外部 kit 的节点缓存、解码、compaction、serverinfo/MDL 测试扩展 `syncer_nokit_test.rs`；同时核对同路径 Go 测试意图，不在 `syncer.rs` 内嵌测试。
- 性能风险主要来自短间隔轮询、持锁回调、每个真实适配器自建双线程 tokio runtime 以及 watch 重连的全量扫描；兼容风险主要来自 etcd 路径、lease 绑定和等待参与集合的改变。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/ddl/schemaver` 找到目标及两份独立 Rust 测试；`node --file pkg/ddl/schemaver/syncer.rs` 分段读取全部 1729 行；`callees WaitVersionSynced` 和 `callees Init` 核对核心下游边。图中同名符号存在歧义，因此上游再用精确搜索核验。
- 目标源码：`pkg/ddl/schemaver/syncer.rs`，重点符号为 `Context`、`EtcdClient`、`MemoryEtcdClient`、`Session`、`Watcher`、`Syncer`、`nodeVersions`、`etcdSyncer`、`WaitVersionSynced`、`syncJobSchemaVer`、`RealEtcdClient`。
- crate 与模块：`pkg/ddl/schemaver/Cargo.toml`、`pkg/ddl/schemaver/lib.rs`、根 `Cargo.toml` workspace 成员与 facade 声明。
- 生产接线：`pkg/session/runtime/session_factory.rs`、`pkg/ddl/schema_version.rs`、`pkg/infoschema/issyncer/syncer.rs`。
- Go 对照：`pkg/ddl/schemaver/syncer.go`；Go 测试为 `syncer_test.go`、`syncer_nokit_test.go`。
- Rust 测试：`pkg/ddl/schemaver/syncer_test.rs`、`pkg/ddl/schemaver/syncer_nokit_test.rs`，覆盖普通/MDL 流程、CAS、session 恢复、watch 重建、事件解码、节点去重、assumed 过滤和取消/deadline 传播。
- 本任务是只读分析加文档，不运行 Cargo；交付结构检查要求文档存在且恰有十一个固定二级标题。
