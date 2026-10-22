# `pkg/metaservice/etcd.rs`

## 文件定位

不创建真实 PD/etcd 连接：PD 查询由调用方实现的 `PdClient` 完成，etcd 对象也只通过 `EtcdClient` trait object 保存；实际 keyspace/meta-service-group 拨号流程位于相邻的 `dial.rs`，分组配置解析位于 `metamanager.rs`。

## 核心职责

1. 用 `Context` 在同步代码中表达取消，并让退避等待可被主动唤醒或由外部取消检查器终止。
2. 用 `PdClient`、`PdMember` 抽象“获取全部 PD 成员”所需的最小边界，避免把具体 PD SDK 类型带入该模块。
3. 用 `Client` 同时保存可选 PD 客户端与调用方拥有的 keyspace etcd 客户端；`new_client` 保证两个依赖均不存在时不产生无功能实例。
4. 在 `get_pd_addrs` 中查询 PD 成员、跳过非法 URL、按需保留 HTTP(S) scheme，并在查询失败时执行与 Go `tikv.BoRegionMiss` 参数一致的指数退避。
5. 用 `parse_url` 统一解析可拨号的 HTTP、HTTPS 与 Unix-family 服务 URL，并提供 Go 命名兼容别名。

本文件不负责 keyspace 元数据加载、meta service group 选择、etcd namespace 设置或网络客户端关闭；这些能力不能从本文件的 `Client`/`EtcdClient` 抽象推断为“已支持”。

新增 `DiagnosticRpcGuard` 在诊断模式按明确白名单过滤 PD/TiKV RPC；这与 etcd 对象持有、成员发现和退避是独立职责，不表示 etcd 标记 trait 自身具有 RPC 拦截能力。

## 主要符号

- `GET_ALL_MEMBERS_BACKOFF_MS: u64 = 5_000`：成员查询失败时允许累计休眠的预算。内部常量 `REGION_MISS_BACKOFF_BASE_MS = 2` 与 `REGION_MISS_BACKOFF_CAP_MS = 500` 分别规定指数退避起点和单次上限。
- `Context`：可克隆的取消句柄。内部 `Arc<(Mutex<bool>, Condvar)>` 支持本地 `cancel()`；可选 `cancellation_checker` 通过 `with_cancellation_checker` 接入上层上下文。`is_cancelled` 合并两种取消源，`wait_backoff` 执行可取消等待。
- `PdMember { client_urls }`：本模块所需的最小 PD 成员投影，只保留客户端 URL。
- `PdClient::get_all_members`：同步查询边界，返回成员列表或 `MetaServiceError`。
- `EtcdClient: Any + Send + Sync`：调用方持有对象的类型擦除标记； blanket impl 允许任意满足约束的具体类型进入 `Arc<dyn EtcdClient>`，`as_any` 可供下转型。它没有 etcd 操作方法。
- `Client`：保存 `Option<Arc<dyn PdClient>>` 和 `Option<Arc<dyn EtcdClient>>`。字段私有，外部只能通过构造函数及访问器使用。
- `new_client` / `new_etcd_meta_service_client`：核心构造与 Rust 风格包装。`NewEtcdMetaServiceClient`、`newClient` 是 Go 命名兼容入口。
- `Client::keyspace_etcd_client` / `GetKeyspaceEtcdCli`：借用并返回已保存的 etcd trait object，不转移或关闭所有权。
- `ServiceClient for Client`：将 `get_pd_addrs` 与 `get_pd_http_addrs` 转发至自由函数；未配置 PD 时返回 `PdClientNotFound`。`GetPDAddrs`、`GetPDHttpAddrs`、`GetPDServiceURLs` 是公开兼容方法，其中 service URL 路径通过 trait 默认实现最终保留 scheme。
- `get_pd_addrs` / `GetPDAddrs`：成员发现与地址抽取的主流程；兼容参数名 `with_schema` 实际语义是“是否保留 scheme”。
- `parse_url` / `ParseURL`：把服务 URL 拆成 `(scheme_prefix, address)`；错误被映射为 `MetaServiceError::ServiceUrl`。

- `DiagnosticRpcGuard: RpcInterceptor`：`pd_allowed` 按完整方法路径匹配，`kv_allowed` 对 `Any` 请求作 protobuf 类型判断；未知请求默认拒绝。
- `EnsureDiagnosticRpcGuard()`：以 `OnceLock<InterceptorGuard>` 保存进程级注册句柄，调用 client-rust 的 `rpc_interceptor::register`。

## 执行流程

构造流程如下：调用方把可选的 etcd 与 PD 对象包装为 `Arc` 后交给 `new_client`；诊断 guard 安装检查后，若两者都为 `None`，返回 `None`。否则 `Client` 原样保存引用计数对象。`new_etcd_meta_service_client` 及两个 Go 风格构造名都不增加额外逻辑。

查询 PD 地址时，`Client` 的 `ServiceClient` 实现先要求 `pd_client` 存在，再调用 `get_pd_addrs(ctx, pd_client, with_scheme)`。主循环每轮按以下顺序运行：

1. 查询前检查 `Context::is_cancelled`。
2. 调用 `PdClient::get_all_members`。
3. 成功时遍历所有成员的全部 `client_urls`，逐个调用 `astersql_util::service_url::ParseServiceURL`；非法项被跳过，合法项通过 `Endpoint(with_scheme)` 输出。HTTP(S) 在 `with_scheme == false` 时成为 `host:port`，Unix-family 地址仍保留拨号所需 scheme。
4. 若成功响应中没有任何合法地址，返回 `NoUsablePdUrl`；否则保持成员及 URL 的遍历顺序返回结果，不去重。
5. 查询失败时再次检查取消；若累计休眠预算已耗尽，返回 `Pd("region unavailable")`。否则等待当前延迟，将延迟计入预算，并令下一次延迟翻倍但不超过 500 ms。

`parse_url` 不经过 PD 查询：它直接复用统一服务 URL 解析器，并返回规范化的 scheme 前缀和地址部分。生产代码 `lightning/pkg/importer/import.rs::importTables` 用它剥离配置中 PD URL 的 scheme，再拼装 TiKV 驱动参数。

`new_client` 首先检查诊断模式并安装 guard，然后才检查两种客户端是否均为空；因此返回 `None` 也可能已完成全局注册。guard 每次调用重新读取 `diagnosticmode::Enabled()`：普通模式直接放行，诊断模式只放行白名单。PD 白名单除成员、store、region、keyspace、配置读取外，还明确包含 TSO、Watch、QueryRegion 与 health check，不能简化为按 RPC 名称是否以 Get 开头判断。KV 允许 Get/BatchGet/Scan、Raw 读取、coprocessor、MVCC/锁信息、TiFlash 系统表、MPP IsAlive 等指定类型；Prewrite、ResolveLock 和未知类型被拒绝。

## 数据与状态

`Context` 的本地取消状态位于共享的 `Arc<(Mutex<bool>, Condvar)>` 中，所以克隆句柄观察同一个布尔值；取消是单向状态，文件中没有复位操作。锁中毒时使用 `PoisonError::into_inner` 继续读取或写入状态，不把 panic 历史转换为业务错误。

设置 `cancellation_checker` 后，外部闭包成为额外取消源。由于闭包无法配合本地 `Condvar` 发通知，`wait_backoff` 最多以 10 ms 小段 sleep 轮询它；未设置闭包时则使用 `Condvar::wait_timeout_while`，`cancel()` 可立即唤醒等待者。

`Client` 仅通过 `Arc` 共享客户端对象，没有内部可变业务状态。`get_pd_addrs` 的 `total_sleep_ms` 与 `delay_ms` 都是单次调用的栈上状态；5000 ms 预算只累计本模块发起的休眠，不包含 PD RPC 自身耗时。地址结果是新分配的 `Vec<String>`，不缓存到 `Client`。

## 依赖与调用关系

- 上游装配：`pkg/metaservice/lib.rs` 声明并重新导出 `etcd`；`pkg/metaservice/metamanager.rs` 定义 `ServiceClient` 与 `MetaServiceError`，并在更高层组合查询中调用 `get_pd_addrs`。
- 已确认的生产调用：`lightning/pkg/importer/import.rs::importTables` 调用 `astersql_metaservice::parse_url`；Lightning 的 `precheck_impl.rs`、`import.rs` 和 `stubs.rs` 使用 `Context::with_cancellation_checker` 把上层 Go 风格上下文取消接入 Rust 流程。其他 BR/session 调用也使用 crate 再导出的 `Context`，但不应据此推断它们都直接调用本文件的地址发现函数。
- 下游解析：`get_pd_addrs` 和 `parse_url` 都调用 `astersql_util::service_url::ParseServiceURL`，由统一解析器决定合法 scheme、主机端口和 Unix socket 语义。
- 抽象回调：`get_pd_addrs` 调用动态分派的 `PdClient::get_all_members`；本文件不知道具体 SDK、RPC 或 mock 实现。
- Cargo 边界：`pkg/metaservice/Cargo.toml` 声明 `astersql-util`、`etcd-client`、`tokio` 和带固定 tag 的 `tikv-client` 等依赖；本文件使用 client-rust 拦截器与 proto；etcd/Tokio 具体拨号逻辑由 crate 内其他模块使用。

RustCodeGraph 的文件关系显示 `pkg/metaservice/etcd.rs` 被多个 BR、Lightning 与 metaservice 文件引用；精确符号搜索进一步确认当前最直接的跨 crate 生产入口是 `parse_url` 和 `Context`。不能把 Go 版本的 51 个引用等同于 Rust 版本已完成相同接线。

guard 直接使用 `tikv_client::rpc_interceptor`、其 proto 类型和 `tonic::Status`，模式开关来自 `astersql-config`。冻结版本的 `pkg/store/driver/tikv_driver.rs` 与 `pkg/util/metricsutil/common.rs` 也在建连前调用 `EnsureDiagnosticRpcGuard`；这是实际生产接线，不只靠本模块构造函数安装。

## 错误处理与边界

- `Client` 缺少 PD 依赖时，trait 方法返回 `MetaServiceError::PdClientNotFound`；自由函数要求调用方已提供 `&dyn PdClient`，因此没有空指针分支。
- PD 查询返回的具体错误会触发重试，但不会被保留为最终错误；预算耗尽统一变为 `MetaServiceError::Pd("region unavailable")`。这与 Go backoffer 的对外行为目标一致，但会丢失最后一次底层错误细节。
- 取消在首次查询前、失败后和退避等待中均可生效，统一返回 `Cancelled`。正在执行的同步 `get_all_members` 只能由实现方观察传入的 `Context` 后自行中止，本模块不能强制抢占它。
- 单个非法成员 URL 被静默跳过；仅当整个成功响应都没有合法 URL 时返回 `NoUsablePdUrl`。函数不去重，也不拒绝空成员列表之外的重复地址。
- `parse_url` 与成员解析共享解析器，但前者会返回解析错误，后者选择跳过坏项。测试证明 HTTP(S) 必须含可拨号端口，IPv6 必须使用方括号；Unix/Unixs 地址保留 scheme，HTTP URL 的路径会被拒绝。
- `GET_ALL_MEMBERS_BACKOFF_MS` 是累计休眠阈值检查，不是严格墙钟 deadline；最后一次 sleep 可使累计值超过 5000 ms，下一次失败才返回耗尽错误。

拦截失败返回 `tikv_client::Error::GrpcAPI`，状态码为 `PermissionDenied`；PD 错误带方法名，KV 错误为固定文本，不格式化请求内容。`before` 不修改传入 timeout；它不取消已经发出的请求，也不代替成员发现的退避/取消逻辑。

## 并发与资源生命周期

`Context`、`PdClient` 和 `EtcdClient` 都满足跨线程共享要求；两个客户端 trait 要求 `Send + Sync`，`Context` 的共享状态由 `Mutex` 保护。`cancel()` 在持锁写入后通知所有等待者，因此同一上下文的多个退避循环都可被唤醒。

本文件不启动异步任务或后台线程。带外部检查器的等待发生在当前线程并进行最多 10 ms 粒度的短睡眠；无检查器时当前线程阻塞在条件变量。每次 `get_pd_addrs` 调用维护独立退避状态，多个调用之间不共享预算或结果。

`Client` 持有的两个 `Arc` 决定底层对象生命周期；`keyspace_etcd_client` 只返回借用。该模块没有 `Drop` 实现，也不会调用 PD/etcd 的 `close`，资源关闭责任仍在具体对象及其拥有者。扩展时不得在这里擅自关闭调用方传入的客户端。

注册句柄由静态 `OnceLock` 长期保留，多次调用不会重复安装或在构造函数返回时撤销。拦截器覆盖上游 client-rust 同步 ClientRuntime 与异步发送路径；其完整网络行为仍需上游与集成验证。

## 与 Go 版本的对应关系

- Go `client` / `newClient` / `NewEtcdMetaServiceClient` 对应 Rust `Client` / `new_client` / `new_etcd_meta_service_client` 及兼容别名；两端都在 etcd 与 PD 均为空时返回 nil/`None`，也都允许仅 PD 客户端存在。
- Go 直接保存具体 `*clientv3.Client` 和 `pd.Client`；Rust 用最小 trait object 解耦 SDK，故 `EtcdClient` 只是所有权/类型擦除边界，`PdMember` 也只保留 URL。
- Go 使用 `context.Context` 与 TiKV backoffer；Rust 使用同步 `Context`、条件变量和显式退避循环。Rust 的外部取消检查器是连接现有上层上下文的适配机制。
- 当前 Go `etcd.go` 还包含 `resolveEtcdDialInfo`、`NewEtcdClientFromPDClient`、`DialEtcdClient`、keyspace 元数据加载和 namespace 设置；这些不在 Rust `etcd.rs` 中。Rust 对应能力应到 `pkg/metaservice/dial.rs` 与 `metamanager.rs` 核验，不能把该文件描述为 Go 文件的完整逐函数镜像。

测试对照也存在范围差异：Go `etcd_test.go` 使用真实嵌入式 etcd 集群验证具体客户端，而 Rust `etcd_test.rs` 使用 trait mock 和占位 `EtcdClient`，验证的是本文件边界，不证明真实 etcd 网络互操作。

诊断防护沿用 Go 的 PD/KV 允许清单意图；Rust 通过 client-rust 统一拦截接口接线，不能把 Go gRPC/client-go 拦截器的具体装配方式照搬为 Rust 事实。

## 扩展指南

- 新增地址输出策略时，优先扩展 `get_pd_addrs` 或底层 `astersql-util` 服务 URL 解析器，并同步 `pkg/metaservice/etcd_test.rs` 与 `migration_aster_unit_test.rs`；必须保留 Unix-family scheme、坏 URL 跳过和全坏时失败等现有契约。
- 调整重试策略时，集中修改三个退避常量与失败分支，同时更新调用次数/预算测试。需要明确预算是休眠时间还是墙钟时间，并评估同步阻塞与 PD 故障时延；不要只为缩短测试而弱化 Go 对齐参数。
- 增加取消来源时，应维持 `Context` 克隆共享和及时唤醒不变量。若要消除 10 ms 轮询，需要让外部取消源可注册唤醒，而不是在持锁区调用不受控闭包。
- 若需暴露真实 etcd 操作，不应随意向标记 trait 堆叠 SDK 方法；先决定抽象属于本文件、`dial.rs` 还是独立适配层，并维持调用方所有权及关闭责任。
- 新增 `Client` 行为应先通过 `ServiceClient` 的 Rust 风格方法表达，再按兼容需要提供 Go 命名别名，避免两套入口产生不同逻辑。
- 测试逻辑必须继续放在独立的 `pkg/metaservice/etcd_test.rs` 或迁移测试文件中，不要内嵌回生产源文件；若涉及真实拨号，则应扩展 `dial_test.rs` 或相应集成测试，而不是用桩声称端到端可用。
- 兼容风险主要是地址格式、错误文案/变体和重试时延；性能风险主要是同步 RPC、每次重新发现成员以及外部取消检查器的轮询。修改前应同时核对 Go `pkg/metaservice/etcd.go` 与其测试。

新增诊断允许项须同步审查 PD 方法路径或 KV proto 类型，并在独立 `diagnostic_rpc_test.rs` 增加允许和拒绝断言；保留未知请求拒绝及普通模式放行。
