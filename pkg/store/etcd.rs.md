# `pkg/store/etcd.rs`

## 文件定位

`pkg/store/etcd.rs` 属于 `astersql-store` crate（`pkg/store/Cargo.toml`），并由 `pkg/store/lib.rs` 的私有 `mod etcd` 装配后通过 `pub use etcd::*` 对外导出。它位于通用 `Storage` 抽象与 `etcd-client` 之间：从存储对象提取元数据 etcd 地址、TLS 配置和 keyspace codec，再构造带命名空间的异步客户端。

当前生产接线需要谨慎理解。`Storage::AsEtcdBackend` 在 `pkg/store/store.rs` 中默认返回 `None`，该文件中的 `RegisteredTiKVStorage` 和 `LocalStorage` 均未覆盖它；仓库搜索到的 Rust `EtcdBackend` 实现只有 `pkg/store/etcd_test.rs` 与 `pkg/store/migration_aster_unit_test.rs` 的测试后端。因此本文件的客户端能力已经实现并从 crate 根导出，但当前 `astersql-store` 的生产 `Storage` 实现不会经 `NewEtcdCli` 建立客户端。这是现有迁移状态，不应推断为完整生产主链已经接通。

## 核心职责

- `EtcdBackend` 定义存储后端向本层提供元数据 etcd 地址、PD 地址、TLS 和 GC Worker 启动能力的契约。
- `GetEtcdAddrs` 只接受显式暴露 `AsEtcdBackend` 的 `Storage`，并调用 `EtcdBackend::EtcdAddrs`；它不会用 `GetPDAddrs` 代替元数据地址。
- `NewEtcdCli` 在没有可用地址时无网络副作用地返回 `Ok(None)`；有地址时连接 etcd，并根据 `Storage::GetCodec` 设置 keyspace 前缀。
- `EtcdClient` 以 `NamespacedClient<Client>` 包装原生客户端，为本层提供自动加前缀的 `Put`/`Get`，同时保留访问原生客户端、端点和设置的接口。
- `NewEtcdCliWithSettings` 把拨号超时、keepalive 和可选 TLS 转换成 `ConnectOptions`，负责真正的异步连接和错误归一化。

## 主要符号

- `pub trait EtcdBackend: Send + Sync`：后端能力接口。`EtcdAddrs` 是本文件连接路径实际读取的地址；`TLSConfig` 是连接路径实际读取的安全配置。`GetPDAddrs` 在本文件内不调用，`StartGCWorker` 则由 `pkg/store/store.rs` 的驱动打开流程使用另一具体存储 API；保留这些方法是为了与 Go `kv.EtcdBackend` 公共契约对齐。
- `pub struct EtcdClientSettings`：连接策略值对象，默认值为自动同步 30 秒、拨号 5 秒、最大退避 3 秒、keepalive 间隔 10 秒、keepalive 超时 3 秒。当前只有 `dial_timeout`、`keep_alive_interval`、`keep_alive_timeout` 被转换到 `ConnectOptions`；`auto_sync_interval` 和 `backoff_max_delay` 仅保存在设置中，尚未应用到底层连接。
- `pub struct EtcdClient`：持有 `NamespacedClient<Client>`、原始端点副本与设置副本。字段私有，借助 `inner`、`inner_mut`、`namespace_prefix`、`endpoints`、`settings` 暴露受控观察或底层访问。
- `EtcdClient::Put`：把 UTF-8 key 和字节值交给命名空间包装器；成功丢弃 `PutResponse`，失败增加 `put etcd key` 上下文。
- `EtcdClient::Get`：支持单 key 或前缀读取；返回去掉命名空间前缀后的 `(key, value)` 字节对，失败增加 `get etcd key` 上下文。
- `NewEtcdCli`：面向 `Storage` 的高层可选构造器。
- `GetEtcdAddrs`：后端识别与地址提取函数。
- `EtcdNamespace`：调用 `MakeKeyspaceEtcdNamespace(store.GetCodec())`；API V1 得到空串，API V2 得到 `/keyspaces/tidb/{keyspace_id}`，无尾部斜杠。
- `NewEtcdCliWithAddrs`：用默认设置转发到显式设置构造器。
- `NewEtcdCliWithSettings`：唯一执行 `Client::connect` 的底层构造入口。

## 执行流程

1. 调用者把 `Option<&dyn Storage>` 交给 `NewEtcdCli`。
2. `GetEtcdAddrs` 先处理 `None`，再调用 `Storage::AsEtcdBackend`。两种缺失情况都返回 `(None, empty)`；后端存在时调用 `EtcdAddrs` 并原样传播 `StoreError`。
3. 空地址使 `NewEtcdCli` 立即返回 `Ok(None)`。非空地址必然伴随 `Some(backend)`；代码用 `expect` 固化这一内部不变量。
4. `NewEtcdCliWithAddrs` 生成 `EtcdClientSettings::default()`，再调用 `NewEtcdCliWithSettings`。
5. 底层构造器创建 `ConnectOptions`，设置 connect timeout、keepalive 及 idle keepalive；若 `TLSConfig` 返回值存在，再附加 TLS，随后异步调用 `Client::connect`。
6. 连接成功后，原生客户端被包入初始无前缀的 `NamespacedClient`，端点和设置被保存到 `EtcdClient`。
7. 回到 `NewEtcdCli` 后，若存在 `Storage`，则通过 codec 计算命名空间；只有非空前缀才调用 `SetEtcdCliByNamespace`。因此 API V1 保持原始 key，API V2 的后续 `Put`/`Get` 自动在 key 前拼接 `/keyspaces/tidb/{id}`。
8. `Get(prefix = true)` 让底层查询使用 etcd prefix 选项；`NamespacedClient` 返回结果时会剥掉命名空间字节，使调用者仍看到逻辑 key。

## 数据与状态

本文件没有全局可变状态。`EtcdClientSettings` 和端点在建连时按值移入 `EtcdClient`，之后仅通过共享引用读取；修改返回的原生客户端不会反向改变记录的端点或设置快照。

命名空间状态位于 `NamespacedClient` 的 `Vec<u8>` 中，初始为空，`SetEtcdCliByNamespace` 直接替换它。`Put` 和 `Get` 都要求 `&mut self`，所以同一包装器上的操作在 Rust 借用层面串行占用；本文件没有 `Mutex`、通道、后台任务或缓存。`inner_mut` 是逃生口：使用它直接操作 `etcd_client::Client` 会绕过本文件的 key 前缀包装，扩展代码必须明确是否需要这种行为。

## 依赖与调用关系

- crate 边界：`pkg/store/Cargo.toml` 声明 `etcd-client = 0.19`（启用 `tls`）、`astersql-util-etcd` 和 `astersql-keyspace`；`nextgen` feature 不改变本文件的条件编译，因为本文件没有 `cfg` 项。
- 上游：`pkg/store/lib.rs` 公开再导出全部符号。RustCodeGraph 将 `GetEtcdAddrs -> Storage::AsEtcdBackend -> EtcdBackend::EtcdAddrs`、`NewEtcdCli -> GetEtcdAddrs/NewEtcdCliWithAddrs/EtcdNamespace`、`NewEtcdCliWithAddrs -> NewEtcdCliWithSettings` 识别为本文件内的主调用层级。仓库文本搜索未发现测试之外对这些构造器的 Rust 直接调用，这与生产 `Storage` 尚未暴露后端的事实一致。
- 下游：`NewEtcdCliWithSettings` 调用 `etcd_client::Client::connect`；`EtcdNamespace` 调用 `keyspace_dependency::MakeKeyspaceEtcdNamespace`；命名空间设置和 KV 操作委托给 `etcd_dependency::NamespacedClient`。
- 相关存储抽象：`pkg/store/store.rs` 定义 `Storage` 和 `StoreError`。`Storage::AsEtcdBackend` 是可选能力探测点，默认 `None`。
- 独立测试：`pkg/store/etcd_test.rs` 覆盖地址选择、空输入短路及需真实 etcd 的命名空间读写；`pkg/store/migration_aster_unit_test.rs` 覆盖空路径、V2 前缀和默认设置值。

## 错误处理与边界

- `store == None`、存储不支持 etcd，以及后端返回空地址都是正常的“无客户端”路径，不是错误。
- `EtcdBackend::EtcdAddrs` 的错误通过 `?` 原样返回；不会尝试 PD 地址，也没有重试或降级。
- 连接错误转换为 `StoreError::other("create etcd client: ...")`；读写错误分别增加 `get etcd key` 或 `put etcd key` 上下文。错误被字符串化，底层 `etcd_client::Error` 没有保留为可下钻的 source。
- 非空地址而后端为空会触发 `expect` panic，但按 `GetEtcdAddrs` 当前返回不变量不可达。修改地址提取逻辑时必须同步维护这一不变量，或改成显式错误。
- 本层不校验地址格式、空字符串端点、重复端点、key 编码或 TLS 内容，交由 `etcd-client` 处理。
- 本层没有实现显式请求超时、应用级重试、watch、lease、delete 或关闭方法；原生能力只能经 `inner`/`inner_mut` 使用，客户端释放依赖所有权析构。

## 并发与资源生命周期

`EtcdBackend` 要求 `Send + Sync`，允许后端以共享引用跨线程使用。构造过程是异步的，网络资源由 `etcd_client::Client` 建立并被 `EtcdClient` 独占持有；本文件不启动自有 Tokio task，也不保存 runtime handle。

`EtcdClient::Put`、`Get` 和 `inner_mut` 使用可变借用，调用者不能在没有额外同步/拆分所有权的情况下并发操作同一个实例。`inner` 允许只读访问底层客户端，但直接使用底层 API不会自动应用命名空间。客户端离开作用域时由底层类型负责连接资源清理；Rust 测试中的真实 etcd 用例显式清理测试 key，但本类型没有业务级 key 清理责任。

## 与 Go 版本的对应关系

`pkg/store/etcd.go` 是直接语义对照。Rust 保留了 `NewEtcdCli`、`GetEtcdAddrs`、`NewEtcdCliWithAddrs` 和 `EtcdBackend` 的 Go 风格命名，并保持以下行为：只取 `EtcdAddrs` 而非 PD 地址、无地址返回 nil/`None`、V2 codec 添加 keyspace namespace、拨号超时 5 秒，以及 TLS/keepalive 配置意图。设置对象还保留了 Go 的自动同步 30 秒和最大退避 3 秒默认值，但如下所述尚未应用到底层连接。

已验证的差异如下：

- Go 通过接口类型断言发现 `kv.EtcdBackend`；Rust 通过 `Storage::AsEtcdBackend` 显式能力方法发现，且当前生产存储没有覆盖该方法。
- Go 返回原生 `clientv3.Client` 并替换其 KV/Watcher/Lease 为 namespace 包装；Rust 的客户端字段私有，因此使用 `EtcdClient` + `NamespacedClient`，当前只保证 `Put`/`Get` 的命名空间行为。
- Go 的 keepalive 时间来自全局 TiKV 配置；Rust 默认固定为 10 秒与 3 秒，也允许通过 `NewEtcdCliWithSettings` 显式传入。
- Go 实际把 `AutoSyncInterval` 和 gRPC `Backoff.MaxDelay` 传入客户端；Rust 的同名设置当前只被保存，没有写入 `ConnectOptions`。这两个字段不能在文档或调用方中视为已生效。
- Go 测试使用内嵌 etcd；Rust 对等网络测试由 `ASTER_ETCD_TEST_ENDPOINT` 驱动并标为 `ignore`，默认测试只验证无网络路径。

## 扩展指南

- 接通生产存储时，应在具体 `Storage` 实现上覆盖 `AsEtcdBackend`，并实现或适配 `EtcdBackend`；同时增加独立测试验证真实元数据组地址、TLS 和 keyspace id，而不是把测试写进 `etcd.rs`。
- 若补齐 Go 的连接策略，应从 `NewEtcdCliWithSettings` 入手，先确认 `etcd-client`/tonic 是否暴露 endpoint auto-sync 与最大退避配置；不能仅因字段存在就宣称策略生效。同步扩展 `pkg/store/etcd_test.rs` 或专门的独立测试文件。
- 若新增 delete/watch/lease/事务操作，优先扩展 `pkg/util/etcd/etcd.rs` 的 `NamespacedClient`，确保所有 key/range 都按相同规则加前缀并在响应中剥离前缀，再在本文件暴露窄接口。
- 若改变 namespace 格式，应同步检查 `pkg/keyspace/keyspace.rs`、Go `pkg/keyspace/keyspace.go`、真实 etcd 测试中的 `/keyspaces/tidb/42direct-key` 断言及所有跨 keyspace 兼容风险。
- 若改动空输入或后端识别规则，必须保留“没有能力/地址不触网”的不变量，并同步 `pkg/store/etcd_test.rs` 与 `pkg/store/migration_aster_unit_test.rs`。
- 性能风险主要在重复建连、端点同步和并发共享策略；兼容风险主要在前缀格式、TLS、keepalive 与 Go 客户端策略差异。不要通过 `inner_mut` 新增默认绕过 namespace 的业务路径。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；已查询 `EtcdBackend`、`GetEtcdAddrs`、`NewEtcdCliWithSettings`，并按文件读取 `pkg/store/etcd.rs`、`pkg/store/etcd_test.rs`、`pkg/store/store.rs`、`pkg/util/etcd/etcd.rs` 和 `pkg/keyspace/keyspace.rs`。
- 源码事实：`pkg/store/etcd.rs` 的 `EtcdBackend`、`EtcdClientSettings::default`、`EtcdClient::{Put, Get}`、`NewEtcdCli`、`GetEtcdAddrs`、`EtcdNamespace`、`NewEtcdCliWithAddrs`、`NewEtcdCliWithSettings`。
- crate 与装配事实：`pkg/store/Cargo.toml` 的依赖/feature，以及 `pkg/store/lib.rs` 的模块声明、公开再导出和独立测试模块路径。
- Go 对照：`pkg/store/etcd.go` 与 `pkg/store/etcd_test.go`；Rust 独立测试：`pkg/store/etcd_test.rs`、`pkg/store/migration_aster_unit_test.rs`。
- 调用与接线复核：仓库搜索 `NewEtcdCli`、`GetEtcdAddrs`、`impl EtcdBackend`、`AsEtcdBackend`，确认构造器当前只被 Rust 测试直接调用，Rust 后端实现也只存在于测试；`RegisteredTiKVStorage` 和 `LocalStorage` 使用默认 `AsEtcdBackend(None)`。
- 未运行 Cargo：任务是纯文档分析，计划明确禁止 Cargo。结构验证及文档人工复核在交付前执行。
