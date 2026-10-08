# `pkg/session/runtime/crossks_store.rs`

## 文件定位

本文件位于 `astersql-session` crate 的会话运行时层，由 `pkg/session/runtime.rs` 以公开模块 `crossks_store` 装配。它是跨 keyspace 运行时与 TiKV 驱动之间的资源适配层：为目标 keyspace 单独打开一个 `TikvStore`，再把它包装成 `astersql_domain_crossks::Store`，供 `pkg/session/runtime/crossks_runtime.rs` 创建目标 keyspace 的 Domain、会话池和 DDL 后端。

当前生产调用链是 `CrossKSProductionRuntimeFactory::new` 构造 `store_opener` 闭包，`CrossKSProductionRuntimeFactory::create_runtime` 调用该闭包并取得 `Arc<CrossKSStore>`。仓库内对 `CrossKSProductionRuntimeFactory` 的直接使用目前只见于 `pkg/session/runtime/crossks_runtime_test.rs`；因此这条生产实现已经具备完整装配逻辑，但其仓库内可见接线主要由测试覆盖，不能据此宣称所有服务启动路径都使用它。

## 核心职责

- `CrossKSStore` 同时保存目标 keyspace 名和实际 `TikvStore`，使跨 keyspace 管理器既能识别资源归属，也能把真实 Store 交给会话运行时。
- `open_target_store*` 系列把 PD 地址与 keyspace 编成 TiKV URL，并通过 `TiKVDriver::OpenWithOptions` 打开独立客户端。
- `open_target_store_with_tls` 把 serving Store 创建时捕获的 CA、证书和私钥路径显式转成 `WithSecurity(Security { ... })`。这样即使全局配置之后变化，目标 Store 仍使用与 serving Store 一致的 TLS 材料；依据是该函数注释及 `CrossKSProductionRuntimeFactory::new` 对 `tls_files` 的克隆捕获。
- `Store` trait 实现定义关闭语义：调用底层 `TikvStore::Close`，并声明该包装器拥有独立打开的客户端，运行时关闭时应一并关闭，即使目标名为 `SYSTEM`。

本文件不负责校验“当前 keyspace 不能获取自身”、管理运行时缓存、创建 Domain/会话池、连接 etcd 或执行 DDL；这些职责分别在 `pkg/domain/crossks/cross_ks.rs` 与 `pkg/session/runtime/crossks_runtime.rs`。

## 主要符号

- `pub struct CrossKSStore { keyspace: String, inner: TikvStore }`：目标 Store 的所有权包装器。字段不公开，避免调用方绕过构造流程伪造 keyspace 与 Store 的对应关系。
- `CrossKSStore::inner(&self) -> &TikvStore`：公开只读借用底层 Store。`create_runtime` 会克隆该句柄交给 `CanonicalSessionFactory::from_crossks_tikv_store`；`TikvStore` 自身的克隆共享内部 `Arc<Mutex<_>>` 状态。
- `impl Store for CrossKSStore`：`keyspace()` 返回包装时保存的名称；`close()` 调用 `TikvStore::Close` 并增加 `close keyspace <name>` 错误上下文；`close_on_runtime_shutdown()` 固定返回 `true`。
- `open_target_store(pd_endpoints, keyspace)`：公开的无显式 TLS 入口，委托给 `open_target_store_with_tls(..., None)`。真实 TiKV 集成测试直接使用它。
- `open_target_store_with_tls(pd_endpoints, keyspace, tls_files)`：公开生产入口，每次创建新的默认 `TiKVDriver`，随后进入统一实现。
- `pub(crate) open_target_store_with_driver(driver, pd_endpoints, keyspace)`：crate 内测试缝，允许注入带 `InMemoryBackend` 的 Driver，不配置 TLS。
- `open_target_store_with_driver_and_tls(...)`：私有统一实现，负责参数校验、URL 构造、安全选项构造、驱动调用、错误映射与包装返回。

本文件没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

1. 调用方提供 PD 地址列表、目标 keyspace，以及可选的 `(CA, cert, key)` 文件路径。
2. `open_target_store_with_driver_and_tls` 首先拒绝空 PD 列表或空 keyspace，统一返回 `ManagerError("target keyspace and PD endpoints are required")`，因此不会把缺失参数交给 Driver。
3. 函数用 `url::form_urlencoded::Serializer` 生成 `keyspaceName=<编码后名称>`，再把 PD 地址以逗号连接成 `tikv://<pd1>,<pd2>?<query>`。例如测试中的 `tenant/a` 会安全地经过查询参数编码，而不是直接拼接原始值。
4. 无 TLS 时传入空选项数组；有 TLS 时创建一个 `WithSecurity(Security { cluster_ssl_ca, cluster_ssl_cert, cluster_ssl_key })` Driver 选项。字符串会被克隆，打开过程不借用调用方的元组。
5. `TiKVDriver::OpenWithOptions` 重置 Driver 配置、应用安全选项、解析地址与 `keyspaceName`，并以 API V2 keyspace 配置连接后端。其失败被包装为 `ManagerError("open keyspace <name>: <driver error>")`。
6. 成功后把输入 keyspace 复制到 `CrossKSStore.keyspace`，连同返回的 `TikvStore` 放入 `Arc`。上层 `create_runtime` 用其 `inner()` 创建目标 Domain，并把同一个包装器转成 `Arc<dyn Store>` 交给 `SessionManager`。
7. 若后续 Domain、会话池或其他生命周期组件装配失败，`crossks_runtime.rs::create_runtime` 显式调用 `Store::close`；正常关闭时 `cross_ks.rs::SessionManager::close` 在会话池和其他生命周期组件之后关闭 Store。

## 数据与状态

`CrossKSStore` 只有两个持久字段。`keyspace` 是创建时的不可变副本，用于 trait 查询与错误上下文；`inner` 是可克隆的 `TikvStore` 句柄。底层 `TikvStore` 用 `Arc<Mutex<TikvStoreInner>>` 共享连接、缓存身份和 `closed` 状态，所以 `inner()` 返回的克隆与包装器观察同一个关闭状态。

目标隔离的关键输入是 URL 中的 `keyspaceName`。`pkg/store/driver/tikv_driver.rs::parse_path` 将它解码到 `ParsedPath.keyspace_name`；`OpenWithOptions` 用该值配置 API V2 客户端，并将 Store 缓存身份构造成 `tikv-<cluster_id>/<keyspace>`。因此同一 PD 集群的不同 keyspace 不会具有相同 Store 身份。`crossks_store_test.rs` 用 `tenant/a` 与 `SYSTEM` 验证两个句柄保留不同的 keyspace。

本文件自身没有全局可变状态、缓存或后台线程。缓存、连接和 Safe Point 等状态由 `TikvStore` 管理；运行时级共享由外层 `Arc` 管理。

## 依赖与调用关系

上游直接关系：

- `pkg/session/runtime.rs` 公开装配模块，并在测试构建中装配独立的 `crossks_store_test.rs`。
- `pkg/session/runtime/crossks_runtime.rs::CrossKSProductionRuntimeFactory::new` 调用 `open_target_store_with_tls`；`create_runtime` 消费返回值，使用 `inner()` 初始化 `CanonicalSessionFactory`，并把 Store 注册给跨 keyspace `SessionManager`。
- `pkg/session/runtime/crossks_runtime_test.rs` 的真实 TiKV 忽略测试直接调用 `open_target_store`；同文件也覆盖 Store 打开失败时虚拟服务注册的清理。
- `pkg/session/runtime/crossks_store_test.rs` 通过 `open_target_store_with_driver` 注入内存后端，验证 keyspace 身份、独立关闭与关闭策略。

下游直接关系：

- `astersql_domain_crossks::{Store, ManagerError}`（路径依赖 `../domain/crossks`）提供资源契约和跨 keyspace 管理错误。
- `astersql_store_driver::{TiKVDriver, TikvStore, Security, WithSecurity}`（路径依赖 `../store/driver`）负责解析 URL、连接 PD/TiKV、应用 TLS、缓存和关闭底层资源。
- `url = "2"` 用于查询参数编码。以上三项均在 `pkg/session/Cargo.toml` 的普通依赖中，无额外 feature 门控；crate 的 `nextgen` feature 不直接包围本文件中的任何符号。

RustCodeGraph 能定位 `CrossKSStore` 和四个打开函数，但当前索引对这些函数的 `callers`/`callees` 查询均返回空；上述直接调用边因此由精确源码搜索和相邻文件复核补足。

## 错误处理与边界

- 空 PD 地址列表和空 keyspace 共用同一个前置错误；两者同时为空也不会提供更细分类别。地址字符串本身的合法性由 Driver 后续解析/连接处理。
- keyspace 被作为查询参数编码，避免 `/`、空格、`&` 等字符破坏 URL 查询结构；PD 端点则按 Driver 支持的逗号分隔 authority 直接连接，调用方必须提供不带额外分隔歧义的端点字符串。
- TLS 元组只表示“有或无”，本层不检查文件存在性，也不检查证书/私钥是否成对；`Security::to_tls_config` 和后端连接负责验证。空字符串组成的 `Some` 会覆盖 Driver 的全局安全配置并等效为无 TLS，而不是回退全局配置。
- 打开失败与关闭失败都转换成 `ManagerError` 并附加目标 keyspace，保留底层错误文本。成功构造前没有可由本层关闭的 Store；成功后的清理由上层生命周期负责。
- `TikvStore::Close` 是幂等的：底层已关闭时直接成功。`CrossKSStore` 不实现 `Drop`；若绕开 `SessionManager` 且从不显式调用 `close`，不能仅凭 `Arc` 释放断言所有驱动资源已按 Close 路径清理。
- 当前独立单元测试未直接覆盖空参数、URL 特殊字符、TLS 覆盖和 Driver 打开/关闭错误映射，这些属于扩展时应补齐的回归边界。

## 并发与资源生命周期

`Store` trait 要求 `Send + Sync`，`CrossKSStore` 通过内部线程安全的 `TikvStore` 满足该契约；外层以 `Arc<CrossKSStore>` 在运行时工厂、Domain 和 `SessionManager` 之间共享。`inner()` 不暴露可变引用，底层可变状态由 `TikvStore` 的互斥锁保护。

正常生命周期为“打开 Store → 创建目标 Domain/池/后台组件 → 关闭池 → 逆序关闭生命周期组件 → 关闭 Store”。`SessionManager::close` 使用原子 `closed.swap` 保证整个关闭序列只执行一次，并根据 `close_on_runtime_shutdown()` 决定是否关闭 Store；本适配器固定返回 `true`，表示包括独立打开的 `SYSTEM` 客户端在内均归该运行时所有。该覆盖有意区别于 trait 默认值：默认规则不会关闭共享的 SYSTEM Store。

构建失败路径由 `crossks_runtime.rs::create_runtime` 分阶段清理：创建 Domain 或池失败会立即关闭已打开 Store；后续组件失败则先停止 owner、刷新循环和 schema，同步关闭池与 Domain，最后关闭 Store。`CrossKSStore::close` 本身不开线程、不等待任务，也不重试；Driver 的 `Close` 会标记关闭、移出 Store 缓存，并关闭 Safe Point、PD、coprocessor 与后端 Store 资源。

## 与 Go 版本的对应关系

Go 主链在 `pkg/domain/crossks/cross_ks.go`：`Manager.createSessionManager` 调用 `getOrCreateStore`；非 SYSTEM keyspace 进入 `pkg/store/store.go::InitStorage`，以 `?keyspaceName=<name>` 构造 Store URL；SYSTEM 则复用 `kvstore.GetSystemStorage()`。若后续创建失败，Go 用 defer 关闭新取得的 Store；`SessionManager.close` 负责正常资源回收。

Rust 文件保留了 Go 的核心语义：以目标 keyspace 打开 TiKV API V2 Store、将其用于独立运行时、在装配失败和生命周期结束时关闭，并由 Driver 使用包含 keyspace 的缓存身份。Rust 版本存在三点明确差异：

1. `open_target_store_with_tls` 显式传递 serving Store 捕获的 TLS 文件，避免重新读取可能已变化的全局默认值；Go 的 `InitStorage` 从当前全局配置构造并打开 Store。
2. Rust 使用 `form_urlencoded::Serializer` 编码 keyspace；Go `InitStorage` 当前通过 `fmt.Sprintf` 直接插入 `keyspaceName`。
3. Rust 的 `CrossKSStore` 总是由目标运行时拥有，`close_on_runtime_shutdown()` 对 SYSTEM 也返回 `true`；Go 的 `getOrCreateStore` 对 SYSTEM 复用全局 Store。Rust trait 的默认实现仍保留“不关闭共享 SYSTEM Store”的 Go 语义，只有该独立打开适配器覆盖它。

所以本文件不是 Go `cross_ks.go` 的逐函数翻译，而是 Rust 生产工厂为同一跨 keyspace Store 生命周期提供的窄适配层。

## 扩展指南

- 若增加连接参数（超时、PD client 配置、TiKV client 配置等），优先扩展 `open_target_store_with_driver_and_tls` 的选项组装，并确保 `CrossKSProductionRuntimeFactory` 从 serving 环境捕获稳定快照；不要在多个公开包装函数中复制 URL 或 Driver 逻辑。
- 若改变 keyspace URL 规则，必须同步检查 `pkg/store/driver/tikv_driver.rs::parse_path`，并在 `pkg/session/runtime/crossks_store_test.rs` 增加包含 `/`、空格、`&`、Unicode 等名称的独立测试，确认编码后仍由 Driver 还原为原名。
- 若改变所有权或关闭策略，应同时检查 `astersql_domain_crossks::Store::close_on_runtime_shutdown`、`SessionManager::close` 和 `crossks_runtime.rs::create_runtime` 的所有失败分支；尤其不能把独立 SYSTEM 客户端误当成共享系统 Store。
- 若增加 TLS 行为，应测试 `None`、完整三元组、空字符串以及证书/私钥不匹配，并验证错误仍带目标 keyspace 上下文。测试逻辑继续放在独立的 `pkg/session/runtime/crossks_store_test.rs`，不要内嵌回生产源文件。
- 若引入自动析构或异步关闭，需要明确与 `TikvStore::Close` 的幂等性、锁顺序、后台任务停止顺序及错误可观察性的关系；当前显式关闭允许上层控制“先停使用者，后关 Store”的不变量。
- 性能风险集中在每个目标 keyspace 的独立 PD/TiKV 客户端及相关后台资源；新增重试或缓存时必须保持 keyspace 隔离，避免跨租户复用错误身份或重复创建连接。

## 验证依据

- 目标实现：`pkg/session/runtime/crossks_store.rs`，核对 `CrossKSStore`、`Store` trait 实现及四个打开函数的完整源码。
- 模块与 crate：`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`，核对公开模块、独立测试模块、`astersql-domain-crossks`、`astersql-store-driver` 与 `url` 依赖。
- 直接上游：`pkg/session/runtime/crossks_runtime.rs`，核对 `store_opener`、`create_runtime`、`inner()` 使用、成功交接与失败清理。
- trait 与关闭顺序：`pkg/domain/crossks/cross_ks.rs`，核对 `Store` 默认 SYSTEM 策略和 `SessionManager::close` 的幂等关闭顺序。
- Driver 行为：`pkg/store/driver/tikv_driver.rs`，核对 `parse_path`、`WithSecurity`、API V2 keyspace 配置、缓存身份、内部共享状态与 `Close`。
- Rust 测试：`pkg/session/runtime/crossks_store_test.rs` 验证不同 keyspace 身份、独立关闭及 shutdown 所有权；`pkg/session/runtime/crossks_runtime_test.rs` 提供忽略的真实 TiKV 使用路径和 Store 打开失败清理证据。
- Go 对照：`pkg/domain/crossks/cross_ks.go::createSessionManager/getOrCreateStore`、`pkg/store/store.go::InitStorage`、`pkg/store/driver/tikv_driver.go`，核对目标 Store 创建、SYSTEM 复用、API V2 与错误清理语义。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`query` 精确定位 `crossks_store.rs::CrossKSStore`、`open_target_store`、`open_target_store_with_tls`、`open_target_store_with_driver`、`open_target_store_with_driver_and_tls`。这些符号的 callers/callees 查询返回空，调用边改由上述精确源码搜索验证。
- 本任务为纯文档分析，未运行 Cargo；交付前使用任务指定命令验证文档存在且恰好包含 11 个固定二级标题，并人工复查没有把未接线范围或未覆盖测试写成已支持事实。
