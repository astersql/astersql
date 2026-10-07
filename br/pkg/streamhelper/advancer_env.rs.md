# `br/pkg/streamhelper/advancer_env.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-streamhelper`；包入口 `br/pkg/streamhelper/lib.rs` 以 `pub mod advancer_env` 挂载它，并再导出主要 trait、常量和配置解析函数。它位于日志备份 checkpoint 推进器与具体集群能力之间：`br/pkg/streamhelper/advancer.rs` 面向这里定义的 `Env` 编程，而实际能力由元数据客户端、TiKV/PD 适配器或测试环境提供。

当前 Rust 文件是接口与轻量适配层，并不是 Go 生产环境装配的完整复刻。Go 同路径文件还定义了 `clusterEnv`、`CliEnv`、`TiDBEnv`、真实 TiKV RPC 连接缓存和锁解析器；Rust 本文件没有这些构造路径，只有 `AdvancerExtEnv`、`PDRegionScanner` 委托包装以及配置字节解析逻辑。

## 核心职责

- 用 `Env` 把推进器需要的五组能力组合为单一边界：`TiKVClusterMeta`、`LogBackupService`、`StreamMeta`、`RegionLockResolver` 和 `LogBackupFlushIntervalGetter`。
- 用 `AdvancerExtEnv` 把 `AdvancerExt` 的 etcd/元数据操作适配为 `StreamMeta`。
- 解析各 TiKV Store 返回的 JSON 配置，并以最慢 Store 的最大 `log-backup.max-flush-interval` 作为推进器的保守节奏参考。
- 用 `PDRegionScanner` 将 GC 安全点、TSO、Region 扫描和 Store 枚举转发给一个 `Arc<dyn TiKVClusterMeta>`。
- 暴露日志备份服务标识、GC 安全点 TTL、拨号超时以及日志备份客户端共享句柄类型。

## 主要符号

- `logBackupServiceID`、`logBackupSafePointTTL`、`dialTimeOut`：分别对应 Go 的服务安全点标识、24 小时 TTL 和 8 秒拨号超时。当前 Rust 文件自身只定义这些值；真实 RPC/PD 装配尚未消费全部常量。
- `LogBackupFlushIntervalGetter::GetLogBackupFlushInterval`：为推进器提供 TiKV flush 周期。
- `StreamMeta`：定义任务事件初始化、全局 checkpoint 读写/清理和暂停任务。所有方法同步返回 `Result<_, String>`，没有 Go 接口中的 `context.Context` 与可变暂停选项。
- `RegionLockResolver::ResolveLocksForRange`：按 key 半开区间和版本上界解析锁；`advancer.rs` 的重试逻辑会在 locked 错误时降低版本上界。
- `Env`：空方法组合 trait，并通过 blanket impl 自动授予同时实现五个父 trait 的类型。
- `AdvancerExtEnv` / `NewMetaBoundEnv`：持有 `AdvancerExt`，将其 checkpoint 与暂停操作转交给 `AdvancerExt`/`MetaDataClient`；暂停固定传入空选项。
- `parseLogBackupFlushIntervalFromConfig` / `parse_go_duration`：从 JSON 读取字符串时长，并解析 Go 风格的 `ns`、`us`、`µs`、`μs`、`ms`、`s`、`m`、`h` 组合。
- `GetLogBackupFlushIntervalFromTiKVConfig`：遍历多 Store 配置，拒绝空集合并返回最大有效间隔。
- `PDRegionScanner`：持有共享 `TiKVClusterMeta` trait object，其五个公开方法均直接委托。
- `LogBackupClientTrait`、`LogBackupServiceTrait`、`SharedLogBackupClient`：兼容历史调用路径的再导出和 `Arc<dyn LogBackupClient>` 别名。

## 执行流程

推进器启动时，`CheckpointAdvancer` 通过 `Env::Begin` 获取已有任务事件；任务新增时读取已有全局 checkpoint，并尝试用 `BlockGCUntil(checkpoint - 1)` 建立 GC 保护；任务删除时清理 checkpoint 并 `UnblockGC`。进入单轮推进后，它读取任务 checkpoint 和当前 TSO，调用 `GetLogBackupFlushInterval` 决定锁处理节奏，收集 Region checkpoint，必要时通过 `ResolveLocksForRange` 处理阻塞锁，最后上传新的全局 checkpoint；失败达到策略条件时调用 `PauseTask`。这些直接消费点位于 `br/pkg/streamhelper/advancer.rs`。

配置路径先由外部环境收集每个 Store 的原始 JSON，再调用 `GetLogBackupFlushIntervalFromTiKVConfig`。每份 JSON 经 `serde_json` 映射到 `TikvConfigRoot`/`TikvLogBackupSection`，取出字符串后由 `parse_go_duration` 分段累加为纳秒；任何配置无字段、格式错误、非正值或溢出都会立即终止聚合。全部有效时返回最大值，使推进频率不比最慢 Store 更激进。

元数据适配路径由 `NewMetaBoundEnv` 把 `MetaDataClient` 包进 `AdvancerExt`。`AdvancerExtEnv` 的 `StreamMeta` 实现逐项转发；其中 `Begin` 调 `BeginSnapshot`，`PauseTask` 调 `MetaDataClient::PauseTask(taskName, Vec::new())`。

## 数据与状态

本文件没有全局可变状态。`AdvancerExtEnv` 只保存一个可克隆的 `AdvancerExt`；实际 checkpoint 与任务状态由其内部 `MetaDataClient` 管理。`PDRegionScanner` 通过 `Arc` 共享无所有权转移的集群元数据能力。

配置反序列化结构均为私有临时值。聚合器维护 `max_flush`、`min_flush` 和 `store_count`；当前返回值只使用最大值，最小值用于保持与 Go 聚合过程一致，但 Rust 版本没有 Go 在各 Store 配置不一致时发出的 warning。时长累计使用 `u128` 做受检乘加，并限制总纳秒不超过 `i64::MAX`，最后安全转换为 `Duration`。

## 依赖与调用关系

上游主要是 `br/pkg/streamhelper/advancer.rs`：它直接调用 `StreamMeta`、`TiKVClusterMeta`、`RegionLockResolver` 和 flush getter；`flush_subscriber.rs` 还通过环境的 `Stores` 枚举 Store。`lib.rs` 将本文件的公开 API 提升为 crate 级 API。

下游包括：`advancer_cliext::{AdvancerExt, TaskEvent}` 提供任务事件及 checkpoint 元数据操作；`client::MetaDataClient` 提供元数据存储；`regioniter::{TiKVClusterMeta, RegionWithLeader, Store}` 定义 PD/TiKV 拓扑边界；`stubs::{LogBackupClient, LogBackupService}` 定义日志备份 RPC 抽象。配置解析直接依赖 Cargo 中声明的 `serde` 与 `serde_json`；同 crate 还声明了本地 `config`、`spans` 子 crate、`regex` 和 `uuid`，但它们不是本文件的直接依赖。

RustCodeGraph 能检索到本文件 41 个符号，并确认同名 Rust/Go 函数及 `StreamMeta`、`PDRegionScanner` 定义；本次 `callers/callees` 命令没有输出可用调用边，因此具体调用关系另由上述源码引用点核验。

## 错误处理与边界

公共接口统一用 `String` 承载错误并原样传播，缺少结构化错误类型和上下文链。`AdvancerExtEnv` 不吞掉元数据错误；`PDRegionScanner` 也不增加检查或注释。特别是 Go `PDRegionScanner::BlockGCUntil` 会更新带 TTL 的服务安全点并校验 PD 返回的最小安全点不高于目标，而 Rust 包装器只相信底层 `TiKVClusterMeta::BlockGCUntil` 已实现这些语义。

JSON 缺少 `log-backup` 或 `max-flush-interval`、空字符串、空白、负数、零、未知单位、多个小数点和超出 `i64::MAX` 纳秒的值都会报错。解析器接受可选 `+`、复合单位和小数；小数最多取前 18 位参与纳秒换算，低于 1ns 的尾数被截断。聚合器遇到任一坏 Store 即失败，并拒绝空列表。

## 并发与资源生命周期

所有环境 trait 都要求 `Send + Sync`，允许推进任务跨线程共享环境。`PDRegionScanner` 和 `SharedLogBackupClient` 以 `Arc` 管理共享生命周期；本文件不创建线程、异步任务、通道、锁或网络连接。

GC 安全点、客户端连接缓存和后台任务的真实生命周期不在当前 Rust 实现中：接口只暴露 `BlockGCUntil`/`UnblockGC` 与 `ClearCache` 所需能力。调用者必须保证任务退出或删除时解除 GC 保护，并由具体生产实现负责 TTL 续约、连接超时和资源清理；不能把这里的常量定义视为这些机制已接线。

## 与 Go 版本的对应关系

Rust 保留了 Go `Env` 的能力分组、常量值、`PDRegionScanner` 表面 API、最大 flush 间隔选择和 `StreamMeta` 操作意图。`advancer_env_test.rs` 进一步覆盖 Go 时长语法中的复合值、小数和微秒符号。

差异是实质性的：Rust 去掉所有 `context.Context`；任务事件用可变 `Vec<TaskEvent>` 而非发送通道；暂停没有可变选项；错误降级为字符串。Go 文件内的真实 `clusterEnv`、StoreManager/grpc 客户端、HTTP 拉取 TiKV 配置、TLS 选择、TiDB/CLI 构造器及 `AdvancerLockResolver` 均未在此 Rust 文件实现。Go 会跳过 TiFlash、排除 tombstone Store，并在 flush 配置不一致时记录 warning；Rust 本文件没有对应生产取数与日志行为。`PDRegionScanner` 的 Rust 方法只是委托，行为完整度取决于注入的 `TiKVClusterMeta`。

## 扩展指南

新增推进器所需环境能力时，应先在专用 trait 中定义最小方法，再加入 `Env` 父 trait列表，同时更新所有独立实现：至少包括 `basic_lib_for_test.rs`、`collector_test.rs`、`parity_test.rs` 和相关 `regioniter_test.rs` 夹具；不要把测试实现放回生产文件。新增元数据操作应优先在 `AdvancerExt`/`MetaDataClient` 实现，再由 `AdvancerExtEnv` 做薄适配。

扩展时长语法或配置结构时，应同步 `advancer_env_test.rs` 的接受/拒绝边界，并与 Go `time.ParseDuration`、`configtypes.Duration` 行为核对，重点评估溢出、亚纳秒截断和多 Store 部分失败。若补齐生产环境，不能只增加构造器桩：还需对齐 Go 的 TLS/HTTP 配置拉取、TiFlash/tombstone 过滤、连接缓存清除、PD caller component、GC 安全点校验及锁解析语义，并放在独立源文件与独立测试文件中。

## 验证依据

- 目标源码：`br/pkg/streamhelper/advancer_env.rs`，核对常量、5 个环境能力 trait、blanket impl、适配器、JSON/时长解析、最大值聚合、PD 包装器及再导出。
- 入口与依赖：`br/pkg/streamhelper/lib.rs`、`br/pkg/streamhelper/Cargo.toml`；该目录没有 `doc.go`。
- Rust 调用证据：`br/pkg/streamhelper/advancer.rs` 中任务 Begin、checkpoint 读写、GC 安全点、TSO、flush 周期、暂停与锁解析调用；`flush_subscriber.rs` 中 Store 枚举；`advancer_cliext.rs` 与 `client.rs` 中实际元数据方法。
- Rust 测试：`br/pkg/streamhelper/advancer_env_test.rs` 验证复合/小数/微秒时长、空白/零/负数/溢出拒绝和多配置取最大值；`advancer_test.rs`、`parity_test.rs`、`basic_lib_for_test.rs` 提供 GC、checkpoint 与完整 `Env` 测试实现证据。
- Go 对照：`br/pkg/streamhelper/advancer_env.go`；Go 测试 `br/pkg/streamhelper/advancer_test.go::TestGetLogBackupFlushIntervalFromTiKVConfig` 覆盖聚合成功、解析失败和空 Store。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/streamhelper` 显示目标文件 41 个符号；`query` 定位 Rust/Go 同名配置函数、`StreamMeta`、`PDRegionScanner` 和 `NewMetaBoundEnv`。`explore/node/callers/callees` 本次未返回正文，故未将缺失图边作为事实依据。
