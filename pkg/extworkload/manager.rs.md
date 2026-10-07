# `pkg/extworkload/manager.rs`

## 文件定位

`manager.rs` 是 `astersql-extworkload` crate 中 `Manager` trait 的生产实现与构造入口。crate 由 [`pkg/extworkload/Cargo.toml`](./Cargo.toml) 定义，库入口是 [`pkg/extworkload/lib.rs`](./lib.rs)；后者以 `manager_impl` 模块加载本文件，并重新导出 `NewManager`、`NewManagerWithTLS` 和 `manager`。接口契约本身位于 [`pkg/extworkload/external_workload.rs`](./external_workload.rs)，本文件负责把该契约落到 `client::Client` RPC 调用上。

在当前 Rust 应用链中，[`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 调用 `NewManagerWithTLS` 创建管理器并把它挂入 domain；[`pkg/store/gcworker/gc_worker.rs`](../store/gcworker/gc_worker.rs) 从运行时取出 `Box<dyn Manager>`，在 GC safe point 成功推进后调用 `RecycleGCV2`，并按角色和有效 GC 生命周期调用 `RegisterGCV2`。因此本文件位于“session/domain 生命周期与 GC 等后台业务”到“external workload controller 客户端”的适配边界，而不是控制器协议或任务调度算法的实现位置。

本文件没有条件编译项。它包含两个 30 秒超时常量、两个数据类型、一个 context 键类型、两个公开构造函数、一个内部拨号函数、`Manager for manager` 的完整实现，以及指标/context/错误辅助函数。

## 核心职责

1. `NewManager` / `NewManagerWithTLS` 根据 `config::ExternalWorkload.Enable` 决定是否安装管理器；启用时强制要求 keyspace 元数据，建立客户端并在返回前执行一次 `Ping`。
2. `dialClient` 把 keyspace、TiDB pool、控制器地址、TLS 与指标拦截器组装成 `client::Option`，并保证首次探活失败时关闭已经创建的客户端。
3. `manager` 保存客户端、当前角色和 keyspace 元数据，实现 `Manager` 的 GC V2、TTL 和 Auto Analyze 转发方法。
4. 每个业务 RPC 都通过 `withRequestTimeout` 派生 30 秒超时 context；其中需要计数的动作再通过 `withMetric` 注入 worker/action 标签。
5. `metricsInterceptor` 在真正执行 RPC invoker 前读取 context 标签并递增 `metrics::ExternalWorkloadTaskCounter`，但指标缺失或标签缺失都不会阻断 RPC。

本文件不维护任务队列、safe point 状态或 TTL/Analyze 状态；这些状态属于调用方和外部控制器。本地 `manager` 只是有生命周期约束、超时和观测语义的请求转发层。

## 主要符号

- `dialTimeout: Duration`：30 秒，覆盖“创建客户端 + 首次 Ping”阶段使用的 context。
- `requestTimeout: Duration`：30 秒，覆盖每一次 `Manager` 业务方法发出的客户端请求。
- `ManagerError = Box<dyn Error + Send + Sync>`：本文件公开的擦除错误类型；与 trait 文件中的同形别名兼容。
- `manager`：`Manager` 的具体实现。`cli: Box<dyn client::Client>` 是唯一可变外部资源；`role` 是构造时配置快照；`meta` 是构造时复制的 `KeyspaceMeta`，所以 `Meta()` 返回的是 manager 自有数据的借用。
- `metricLabels { workerType, action }`：随 context 传递的指标维度。它只供客户端拦截器读取，不参与 RPC 请求体。
- `metricLabelsKey`：零尺寸专用 context 键，避免和其他 context 值发生类型冲突。
- `NewManager(context, keyspaceMeta, config)`：无显式 TLS 文件参数的公开便捷入口，直接调用 `NewManagerWithTLS(..., None)`。
- `NewManagerWithTLS(..., tls_files)`：生产构造入口。关闭配置时返回 `Ok(None)`；启用时返回 `Ok(Some(Box<dyn Manager>))` 或构造错误。
- `dialClient(...)`：内部资源创建与探活函数，返回已经通过 Ping 的 `Box<dyn client::Client>`。
- `impl Manager for manager`：实现 `Close`、`Role`、`Meta`，以及 GC V2、TTL、Auto Analyze 的注册、回收和配置更新方法。
- `metricsInterceptor()`、`withMetric(...)`、`withRequestTimeout(...)`：分别负责消费指标标签、写入标签和建立单次请求 deadline。
- `boxedError(...)`、`annotateError(...)`：把消息或“前缀 + 下游错误显示文本”转换为 `ManagerError`。

可见性上，`manager`、其字段、`metricLabels`、`metricLabelsKey` 和两个构造函数是 `pub`；`dialClient`、拦截器和辅助函数均为文件内部实现。内部类型被设为公开主要是为了现有独立测试和 crate 内装配使用，业务调用方应优先依赖 `Manager` trait 和构造函数。

## 执行流程

### 创建与安装

1. `NewManager` 把调用原样交给 `NewManagerWithTLS`。
2. `NewManagerWithTLS` 先检查 `config.Enable`。关闭时立即返回 `Ok(None)`，不会读取 keyspace、TLS 或连接控制器。
3. 启用时，`keyspaceMeta` 必须为 `Some`；否则返回 `external workload controller requires a non-nil keyspace meta`。
4. `dialClient` 选择 TLS 来源：显式 `tls_files` 优先；否则在全局 `Security.ClusterSSLCA` 非空时调用 `ClusterSecurity().ToTLSConfig()`；两者都没有时使用明文配置。
5. 它用 `dialTimeout` 创建派生 context，组装 `client::Option`：keyspace ID/name、TiDB pool、控制器地址、TLS，以及唯一的 `metricsInterceptor`。
6. `client::New` 创建客户端。创建失败时先调用 cancel，再返回错误。
7. 新客户端用同一个拨号 context 执行 `Ping`，随后立即 cancel。Ping 失败时尝试 `Close`；Close 失败只记 warning，主返回值仍是带 `ping external workload controller` 前缀的 Ping 错误。
8. 构造成功后，`NewManagerWithTLS` 保存 client、role 和克隆后的 meta，记录角色/keyspace/name/id 日志，并以 trait object 返回。

### 业务方法转发

所有带 context 的方法都先调用 `withRequestTimeout`，在底层同步调用返回后显式 cancel，再将 `client::ClientError` 擦除为 `ManagerError`。参数映射如下：

| `Manager` 方法 | 下游客户端方法与参数 | 指标标签 |
| --- | --- | --- |
| `InitializeGCV2(ctx, life)` | `RegisterGCV2(ctx, 0, life.as_secs_f64() as i64)` | `gcv2/init` |
| `AbortGCV2(ctx)` | `RecycleGCV2(ctx, u64::MAX)` | `gcv2/abort` |
| `RegisterGCV2(ctx, safe, life)` | `RegisterGCV2(ctx, safe, life.as_secs_f64() as i64)` | `gcv2/register` |
| `RecycleGCV2(ctx, safe)` | `RecycleGCV2(ctx, safe)` | `gcv2/recycle` |
| `UpdateGCLifeTime(ctx, life)` | `UpdateGCLifeTime(ctx, life.as_secs_f64() as i64)` | 无 |
| `RegisterTTLTask(ctx, table, enabled)` | `RegisterTTLTask(ctx, table, enabled)` | `ttl/register` |
| `DeleteTTLTableInfo(ctx, table)` | `DeleteTTLTableInfo(ctx, table)` | 无 |
| `RecycleTTLTask(ctx, create_time)` | `RecycleTTLTask(ctx, create_time)` | `ttl/recycle` |
| `UpdateTTLJobEnable(ctx, enabled)` | `UpdateTTLJobEnable(ctx, enabled)` | 无 |
| `RegisterAutoAnalyze(ctx, task)` | `RegisterAutoAnalyze(ctx, task)` | `auto-analyze/register` |
| `RecycleAutoAnalyze(ctx, task)` | `RecycleAutoAnalyze(ctx, task)` | `auto-analyze/recycle` |

`metricsInterceptor` 先尝试从 context 取出 `metricLabels`；仅当标签存在且全局 counter 已初始化时递增计数，然后无条件用原 method/request/reply/connection/options 调用 `invoker`。它统计的是发起动作的次数，不根据 RPC 成功或失败回滚计数。

## 数据与状态

`manager` 的持久状态只有三个字段：客户端 trait object、角色字符串型配置值和一份 keyspace 元数据。创建之后，业务方法不会修改 `role` 或 `meta`；变化只发生在 `cli` 内部或远端控制器。`Role()` 克隆角色，`Meta()` 始终返回 `Some(&self.meta)`，因为构造路径已经拒绝空 meta。

GC 生命周期由 `Duration` 转为 `i64` 秒。代码使用 `as_secs_f64() as i64`，所以小数秒会被截断；[`pkg/extworkload/manager_test.rs`](./manager_test.rs) 的 `test_gcv2_lifetime_truncates_fractional_seconds` 用 3600.999 秒验证结果为 3600。`InitializeGCV2` 固定 safe point 为 0；`AbortGCV2` 用 `u64::MAX` 作为“回收全部”的协议哨兵。

指标状态不保存在 `manager` 中。`metricLabels` 只存在于单次派生 context；计数器是 `metrics::ExternalWorkloadTaskCounter` 全局可选值。未标注的更新/删除方法仍有 30 秒 deadline，只是不产生这个动作计数。

## 依赖与调用关系

上游直接证据：

- [`pkg/extworkload/lib.rs`](./lib.rs) 重新导出构造函数和实现类型，并将 [`pkg/extworkload/manager_test.rs`](./manager_test.rs) 作为独立测试模块挂载。
- [`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 在创建 domain 外部工作负载管理器时，把 background context、keyspace meta、配置和可选 CA/cert/key 路径传给 `NewManagerWithTLS`；初始化 GCV2 时再调用 `InitializeGCV2`。
- [`pkg/store/gcworker/gc_worker.rs`](../store/gcworker/gc_worker.rs) 的 `notifyGCV2AfterGC` 仅在 PD safe point 成功推进后取 manager；它检查 keyspace GC 和角色，再调用 `RecycleGCV2` 与 `RegisterGCV2`。
- RustCodeGraph 对本文件报告的使用方还包括 [`pkg/extworkload/migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，用于 Go 语义迁移回归。

下游直接依赖：

- `client::New` 和 `client::Client`：创建连接并提供 Ping/Close/各类工作负载 RPC。实际生产地址由 [`pkg/extworkload/real_client.rs`](./real_client.rs) 接入 tonic 客户端；测试用 `stub://` 地址由 crate 门面提供桩实现。
- `context::{WithTimeout, WithValue}`：创建带取消函数、deadline 和类型化值的 context。
- `config`：读取启用开关、角色、控制器地址、TiDB pool 和集群 TLS 配置。
- `tonic::transport`：从显式 PEM 文件构造 CA 与客户端 identity。
- `metrics`：提供 worker/action 常量和可选计数器。
- `logutil` / `zap`：记录安装成功与 Ping 失败后 Close 失败的诊断日志。

Cargo 边界上，直接声明的依赖是同目录 `astersql-extworkload-client`、带多线程运行时和 time feature 的 `tokio`、以及带 transport/tls feature 的 `tonic`。其他 Go 风格模块通过 crate 门面提供。

## 错误处理与边界

- 配置关闭不是错误，结果为 `Ok(None)`；调用方必须处理“未安装 manager”的正常状态。
- 配置开启但缺 keyspace meta 是构造前置条件错误；不会尝试拨号。
- 显式 TLS 文件使用 `std::fs::read`；CA、证书或私钥不可读时直接返回 I/O 错误。只要 cert 或 key 任一路径非空，代码就会同时读取两者，因此配置应成对提供。
- 全局集群 TLS 转换失败会添加 `build external workload TLS config` 前缀；`dialClient` 的全部错误在构造层再添加 `init external workload client` 前缀。
- `client::New` 失败时没有客户端可关闭，但仍取消拨号 context。Ping 失败时主动 Close；Close 失败只写 warning，避免掩盖决定构造失败的 Ping 错误。
- 业务方法不重试、不吞错，也不加方法名注解；下游错误仅被装箱并原样向上返回。调用方负责决定记录、降级或终止策略。
- `metricsInterceptor` 对计数器未初始化采取跳过策略，对 RPC 则始终透传；指标系统不可用不会变成业务错误。
- `Duration` 到 `i64` 秒的转换存在精度截断；新增调用若依赖亚秒精度，不能复用当前协议映射而不先修改接口约定和对照测试。

## 并发与资源生命周期

`Manager: Send`，但不是 `Sync`，且所有会操作客户端的方法接收 `&mut self`。本文件自身不创建线程、异步任务、锁或通道；共享策略由上游决定。当前 GC 调用方把 `Box<dyn Manager>` 放在 `Arc<Mutex<_>>` 中并在调用期间持锁，因此同一 manager 的 RPC 是由外层互斥串行化的。

构造期资源按以下顺序收尾：拨号 context 建立后，无论 `client::New` 或 Ping 成败都会显式调用 cancel；Ping 失败还会 Close 客户端。构造成功后，客户端所有权转入 `manager.cli`，最终应由上游调用 `Manager::Close` 释放连接。本类型没有 `Drop` 实现，所以遗漏显式 Close 时，本文件不提供额外的协议级关闭保证。

每次业务 RPC 都在调用前创建独立的超时 context，在同步客户端调用返回后 cancel。标签 context 由 timeout context 再派生，因此同时保留上游取消链和当前请求的 deadline；标签只活到该次调用结束。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/extworkload/manager.go`](./manager.go)，Rust 保留了 Go 的核心控制流：禁用时返回空 manager、启用时要求 meta、30 秒拨号/Ping、30 秒单请求超时、Ping 失败 Close、相同 RPC 参数、相同指标标签矩阵，以及错误向上传播。

主要语言映射与差异：

- Go 的 `(Manager, error)` 中 `nil, nil` 映射为 Rust 的 `Result<Option<Box<dyn Manager>>, ManagerError>` 中 `Ok(None)`。
- Go `*keyspacepb.KeyspaceMeta` 被 Rust 构造函数借用后克隆进 manager；因此 Go 测试断言指针相同，而 Rust 测试断言值相同/借用来自 manager 自身。
- Go `defer cancel()` 映射为每条返回路径上的显式 `cancel()`；修改控制流时必须继续覆盖成功和错误路径。
- Go 的 `math.MaxUint64` 映射为 `u64::MAX`。
- Go 的 `time.Duration.Seconds()` 转 `int64` 与 Rust 的浮点秒转 `i64` 都截断小数秒；Rust 有专门回归测试锁定这一点。
- `NewManagerWithTLS` 和显式 `tls_files` 是 Rust 生产接线需要的扩展；基础 `NewManager` 仍保持 Go 入口语义。
- trait 同时提供 `RegisterTTLTask` 和默认别名 `RegisterTTLTableInfo`；本实现覆盖 `RegisterTTLTask`，调用 `RegisterTTLTableInfo` 时由 trait 默认方法转发，兼容已有 Rust 调用方 ABI。

[`pkg/extworkload/manager_test.rs`](./manager_test.rs) 是本文件的独立 Rust 单元测试；[`pkg/extworkload/migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 再次验证 Go 参数、deadline、标签、角色谓词与错误语义。Go 侧对应测试是 [`pkg/extworkload/manager_test.go`](./manager_test.go)。

## 扩展指南

- 新增一种控制器动作时，先在 [`pkg/extworkload/external_workload.rs`](./external_workload.rs) 扩展 `Manager` trait 和在客户端 trait/真实客户端中扩展 RPC，再在本文件的 `impl Manager for manager` 增加转发。不要只在本文件添加未进入接口的私有方法。
- 明确该动作是否需要 `workerType/action` 指标。若需要，复用 `withRequestTimeout` 后再调用 `withMetric`；若 Go 对照没有标签，不要为了“统一”擅自添加，以免改变指标基数和语义。
- 所有新请求都应保证 RPC 返回后 cancel；如果增加早返回分支，要逐条审核 cancel 和客户端 Close 路径。更稳妥的重构需要保持 Go `defer` 所表达的全路径收尾语义。
- 修改 TLS 优先级或文件加载策略时，同步检查 session 传入的 CA/cert/key 三元组、全局 `ClusterSSLCA` 回退和 tonic identity 的成对证书要求。
- 修改 GC 生命周期转换、哨兵值或 safe point 映射会改变控制器协议，必须同步 Go 对照、客户端协议和 `manager_test.rs` 的精度/参数断言。
- 测试逻辑应继续放在独立的 [`pkg/extworkload/manager_test.rs`](./manager_test.rs) 或迁移测试文件中，不要内嵌到生产源文件。至少补齐成功参数、deadline/标签、下游错误、资源关闭或 TLS 错误中的相关分支。
- 性能风险主要来自同步持锁 RPC、每次请求创建 timeout context 和指标标签基数；新增高频动作前应确认调用频率、锁持有范围与标签是否有界。

## 验证依据

- RustCodeGraph `status`：索引存在，覆盖 7032 个 Rust 文件；`files --filter pkg/extworkload` 确认源、Go 对照和测试均在索引中。
- RustCodeGraph `node --file pkg/extworkload/manager.rs --offset 1 --limit 260` 与 `--offset 261 --limit 220`：读取完整 446 行实现，并报告直接使用方 `manager_test.rs`、`migration_aster_unit_test.rs`、`session/runtime/session.rs`、`store/gcworker/gc_worker.rs`。
- RustCodeGraph `query`：确认 `NewManager`、`NewManagerWithTLS`、`dialClient`、`metricsInterceptor`、`withMetric`、`withRequestTimeout` 的定义位置；精确 `callers/callees` 命令本次未返回边明细，因此上游/下游关系以索引的文件使用方和下列源码调用点交叉验证，没有把缺失图边当作已验证事实。
- 源与边界文件：[`pkg/extworkload/manager.rs`](./manager.rs)、[`pkg/extworkload/Cargo.toml`](./Cargo.toml)、[`pkg/extworkload/lib.rs`](./lib.rs)、[`pkg/extworkload/external_workload.rs`](./external_workload.rs)、[`pkg/extworkload/real_client.rs`](./real_client.rs)。
- 上游调用点：[`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 的 `NewManagerWithTLS` / `InitializeGCV2`；[`pkg/store/gcworker/gc_worker.rs`](../store/gcworker/gc_worker.rs) 的 `notifyGCV2AfterGC`。
- 语义对照与测试：[`pkg/extworkload/manager.go`](./manager.go)、[`pkg/extworkload/manager_test.go`](./manager_test.go)、[`pkg/extworkload/manager_test.rs`](./manager_test.rs)、[`pkg/extworkload/migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。Rust 测试覆盖构造生命周期、Ping 失败、真实 gRPC 地址拒绝、全部方法的参数/deadline/标签、错误透传、有效生命周期和小数秒截断。
- 本任务是纯文档分析；按任务约束不运行 Cargo。交付前以任务指定命令验证目标文件存在且固定二级标题恰好为 11 个，并人工复核没有把测试桩或期望设计描述成生产事实。
