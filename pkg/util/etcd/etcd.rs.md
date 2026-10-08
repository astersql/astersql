# `pkg/util/etcd/etcd.rs` 逻辑说明

## 文件定位

`pkg/util/etcd/etcd.rs` 是 Cargo 包 `astersql-util-etcd` 的核心实现文件。crate 入口 `pkg/util/etcd/lib.rs` 将私有模块 `etcd` 的全部公开项重新导出；仓库总门面 `pkg/lib.rs` 又在 `pkg::util::etcd` 下重新导出该 crate。`pkg/util/etcd/Cargo.toml` 声明它依赖 `etcd-client`、`tokio`、`async-trait`、`metrics` 和 `tracing`，没有 feature 条件或条件编译分支。

该文件位于原始 etcd 客户端和上层存储适配之间，提供两类能力：给 key 统一附加 keyspace/租户前缀，以及用逐次超时和固定次数重试删除 key。当前可确认的生产接线位于 `pkg/store/etcd.rs`：`NewEtcdCliWithSettings` 创建原始 `etcd_client::Client` 并包入 `NamespacedClient`，`NewEtcdCli` 根据 `EtcdNamespace` 调用 `SetEtcdCliByNamespace`，随后 `EtcdClient::Put`、`EtcdClient::Get` 委托给本文件的方法。仓库精确搜索未发现 `delete_key_from_etcd` 或 `DeleteKeyFromEtcd` 的生产调用者，因此删除辅助目前是已实现、已测试并公开，但尚未在已搜索到的生产主链接线的能力。

## 核心职责

1. 用 `KEY_OP_DEFAULT_TIMEOUT`（2 秒）、`KEY_OP_DEFAULT_RETRY_COUNT`（5 次）和 `KEY_OP_RETRY_INTERVAL`（30 毫秒）表达与 Go 包一致的 key 操作策略；同时提供 Go 风格别名 `KeyOpDefaultTimeout`、`KeyOpDefaultRetryCnt`、`KeyOpRetryInterval`。
2. 用 `NamespacedClient<C>` 在客户端边界保存字节前缀，并在 delete/put/get 发出请求前拼接前缀。字节表示避免把 etcd key 或 value 强制解释成 UTF-8。
3. 用最小异步接口 `DeleteClient` 解耦删除重试算法和具体 `etcd_client::Client`，使测试能注入脚本化错误或永久挂起的客户端。
4. 用 `delete_key_from_etcd` 为每次删除尝试分别施加超时，失败时记录 `tidb_retryable_error_total` 指标和 warning 日志，成功时立即停止，耗尽预算时返回最后一次错误。
5. 用 `NamespacedClient<etcd_client::Client>::put/get` 提供当前 `pkg/store/etcd.rs` 所需的真实 KV 操作；get 返回时剥离 namespace，使上层仍看到调用时使用的逻辑 key。

## 主要符号

- `KEY_OP_DEFAULT_TIMEOUT`、`KEY_OP_DEFAULT_RETRY_COUNT`、`KEY_OP_RETRY_INTERVAL`：Rust 风格公开常量；Go 风格别名位于文件末尾，值完全相同。
- `DeleteClient`：公开异步 trait，只要求 `delete(Vec<u8>)`；关联错误必须实现 `Error + Send + Sync + 'static`。文件内为 `etcd_client::Client` 和任意 `NamespacedClient<C>` 提供实现。
- `NamespacedClient<C>`：公开泛型包装器，内部字段 `inner` 与 `namespace_prefix` 均为私有。`new` 创建空前缀视图；`inner`、`inner_mut`、`into_inner` 管理底层客户端访问；`namespace_prefix` 暴露只读前缀；私有 `prefixed_key` 完成字节拼接。
- `set_etcd_client_namespace` / `SetEtcdCliByNamespace`：直接替换包装器保存的前缀；后者只是兼容 Go 命名的公开转发函数。
- `DeleteKeyError<E>`：公开错误枚举。`Client(E)` 保留底层错误及 `source()`，`Timeout(Duration)` 表示 Tokio 定时器到期且没有底层 source。
- `delete_key_from_etcd` / `DeleteKeyFromEtcd`：相同删除算法的 Rust 风格与 Go 风格公开入口。
- `NamespacedClient<etcd_client::Client>::put`：拼接 key 后调用真实客户端 `put`，value 和 `PutOptions` 原样传递。
- `NamespacedClient<etcd_client::Client>::get`：拼接查询 key；`prefix=true` 时构造 `GetOptions::with_prefix()`；将响应转换为拥有所有权的 `(key, value)` 字节向量，并尝试从返回 key 开头剥离当前 namespace。

文件没有模块级可变静态状态、宏定义、类型别名、独立 `impl Drop`、线程创建或 `#[cfg]` 条件项。

## 执行流程

命名空间客户端的生产流程如下：

1. `pkg/store/etcd.rs::NewEtcdCliWithSettings` 用地址、TLS 和连接参数建立 `etcd_client::Client`，再调用 `NamespacedClient::new`；此时前缀为空。
2. `pkg/store/etcd.rs::NewEtcdCli` 从存储 codec 计算 `EtcdNamespace`；非空时调用 `SetEtcdCliByNamespace`，把字符串按原始字节保存。
3. 上层调用 `pkg/store/etcd.rs::EtcdClient::Put` 或 `Get`。本文件先通过 `prefixed_key` 分配 `prefix.len() + key.len()` 容量并依次复制前缀、逻辑 key，再调用真实 etcd 客户端。
4. put 原样返回 etcd 响应；get 收集所有 KV，剥离响应 key 上的当前前缀并保留二进制 value。若服务端返回意外的不带前缀 key，`strip_prefix` 失败时会保留原 key，而不是报错。

删除流程由 `delete_key_from_etcd` 驱动：

1. 初始化 `last_error = None`，按 `0..retry_count` 循环；`retry_count == 0` 时循环不执行并直接成功。
2. 每轮把 `client.delete(key.as_bytes().to_vec())` 包入独立的 `tokio::time::timeout(timeout, ...)`。如果 client 是 `NamespacedClient<C>`，其 `DeleteClient::delete` 会先拼前缀。
3. 底层成功则立即 `Ok(())`；底层报错变成 `DeleteKeyError::Client`；定时器到期变成 `DeleteKeyError::Timeout(timeout)`。
4. 每次失败都以错误文本作为 `tidb_retryable_error_total` 的 `error` 标签加一，并输出包含 `key`、零起始 `retry` 序号和错误文本的 warning；随后覆盖 `last_error`。
5. 尝试耗尽后返回最后一次错误。函数内部不等待 `KEY_OP_RETRY_INTERVAL`；该常量只是提供给外部调用者控制操作间隔。

## 数据与状态

`NamespacedClient<C>` 的持久状态只有底层客户端 `inner` 和 `Vec<u8>` 形式的 `namespace_prefix`。`set_etcd_client_namespace` 是替换而非追加：重复设置不会形成嵌套前缀。空字符串恢复为空前缀语义。前缀与 key 只是字节直接连接，函数不会自动插入 `/`，所以分隔符必须由调用者提供。

`prefixed_key` 每次操作都会新分配一个向量并复制前缀和 key；它不缓存组合结果。get 的输出同样复制服务端 key/value 到拥有所有权的向量，且对 key 做逻辑视图还原。删除函数只在栈上保存最后一个错误；之前的具体错误在记录指标和日志后被替换，不会聚合返回。

默认常量是 API 策略值，但 `delete_key_from_etcd` 不会自行套用它们，调用者必须显式传入重试次数和超时。`KEY_OP_RETRY_INTERVAL` 也没有在本文件消费。

## 依赖与调用关系

- 上游生产调用：`pkg/store/etcd.rs::{NewEtcdCli, NewEtcdCliWithSettings, EtcdClient::Put, EtcdClient::Get}` 使用 `NamespacedClient`、`SetEtcdCliByNamespace` 以及真实 put/get 方法。RustCodeGraph 将 `pkg/store/etcd.rs` 列为直接使用者，并进一步显示该存储包装被 server、session、domain 等文件使用，因此本文件处于应用访问 etcd 的底层公共边界。
- 门面导出：`pkg/util/etcd/lib.rs` 通过 `pub use etcd::*` 导出全部公开符号；`pkg/lib.rs` 的 `util::etcd` 再导出 `facade_util_etcd::*`。
- 下游运行时依赖：`etcd_client::Client::{delete, put, get}` 执行 RPC；`tokio::time::timeout` 约束单次删除；`metrics::counter!` 记录失败；`tracing::warn!` 记录上下文；`async_trait` 使带泛型 mock 的异步 trait 可用。
- Cargo 边界：`pkg/util/etcd/Cargo.toml` 的 crate 名为 `astersql-util-etcd`，`lib.rs` 是唯一库入口。若干 Cargo 包声明对它的路径依赖，但当前精确符号搜索确认的生产直接消费点是 `pkg/store/etcd.rs`；Cargo 依赖本身不等于运行时调用证据。
- 测试调用：`pkg/util/etcd/etcd_test.rs` 覆盖命名空间写入以及可选真实 etcd 二进制 round-trip；`pkg/util/etcd/migration_aster_unit_test.rs` 直接覆盖常量、delete 前缀、错误重试、最后错误、逐次超时和零重试。

`br/pkg/utils/metadata_register.rs` 使用的是 `astersql_metaservice::NamespacedEtcdClient`，`pkg/domain/infosync/info.rs` 定义自己的 `EtcdClient` trait；虽然 RustCodeGraph 的文件级“used by”结果包含它们，但源码没有引用本文件的符号，不能据此视为直接调用者。

## 错误处理与边界

- `DeleteKeyError::Client` 保留真实错误，显示文本直接委托给底层错误，`source()` 可继续错误链；`Timeout` 的文本固定包含超时时长，且没有 source。
- 每轮失败都会计数和告警，包括超时；成功尝试不会记录。指标标签包含完整错误文本，新增错误类型时需要留意标签基数。
- 重试没有错误分类：所有底层错误和超时都消耗一次预算并继续，直至成功或耗尽；没有指数退避，也没有内部 sleep。
- `retry_count == 0` 返回成功且不调用客户端，这是 Go 循环空操作的兼容行为，并由 `zero_retry_count_matches_go_noop_result` 固定。
- `timeout == Duration::ZERO` 并不保证调用底层 delete；除零重试外，它通常会由 Tokio 立即超时，具体轮询边界应视 Tokio 调度语义处理。
- delete 的输入是 `&str`，只能接受 UTF-8 字符串 key；put/get 的 key 和 value 是 `Vec<u8>`，可以保存任意二进制内容。
- get 在 `prefix=true` 时会做范围查询；剥离 namespace 使用当前包装器状态。如果调用进行期间前缀可被更换会产生歧义，但 Rust 的 `&mut self` 借用阻止同一实例在安全代码中被同时修改。
- `inner_mut` 允许调用者绕过前缀直接操作原始客户端；这是有意的逃生口，扩展代码不能假设所有底层操作都自动隔离。

## 并发与资源生命周期

该文件不创建线程、后台任务、锁、channel、lease 或 watcher，也不实现显式关闭。`NamespacedClient` 以所有权持有底层客户端；`into_inner` 消耗包装器并归还底层对象，正常离开作用域时由底层类型自行执行析构。是否可克隆、是否可跨线程完全由泛型 `C` 决定；本类型没有自行实现 `Clone`。

所有真实 KV 方法和 `DeleteClient::delete` 都要求 `&mut self`，同一包装实例不能在安全 Rust 中并发发起这些操作。调用者若要共享，需在外层选择锁、任务拆分或复制底层客户端策略。本文件不会替调用者序列化多个实例。

删除超时会丢弃被 `tokio::time::timeout` 包装的 future，从调用者角度取消本轮等待；是否已经把请求送到服务端、服务端是否最终执行，取决于 etcd 客户端与网络时序。因此 delete 必须保持幂等，调用方不能把“超时”解释为服务端确定未删除。每次重试彼此独立，没有贯穿整个函数的总 deadline。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/etcd/etcd.go`，主要映射如下：

- 三个默认常量数值一致。Rust 同时保留 Rust 风格名和 Go 风格别名。
- Go `SetEtcdCliByNamespace` 原地替换 `clientv3.Client` 的 `KV`、`Watcher`、`Lease` 为 namespace wrapper；Rust `etcd-client` 不暴露这些字段，因此改为 `NamespacedClient<C>` 保存单一前缀。当前真实 Rust 实现覆盖 delete/put/get，没有 Watcher 或 Lease 的 namespaced 操作，不能把 Go 的完整三接口覆盖视为 Rust 已实现。
- Go delete 使用 `context.WithTimeout` 为每轮创建 child context并立即 cancel；Rust用 `tokio::time::timeout` 为每轮 future 设置边界。二者都在成功时提前返回、失败时计数和告警、耗尽后返回最后错误、零重试时返回 nil/Ok。
- Go 函数接收 `retryCnt int`，Rust 接收 `usize`，因此 Rust API 没有负数输入。
- Go `errors.Trace(err)` 为最后错误增加 PingCAP 错误栈语义；Rust `DeleteKeyError::Client` 保留 source，但不额外捕获 backtrace。
- Go 的 namespace 包装由 etcd 官方库负责范围转换及响应 key 处理；Rust 在本文件显式前缀化，并在 get 响应上手动剥离前缀。

Go 测试 `pkg/util/etcd/etcd_test.go::TestSetEtcdCliByNamespace` 使用真实集成集群验证加前缀写入。Rust 的普通单元测试以 `SharedKv` 验证同一逻辑；`namespaced_real_kv_roundtrip_keeps_binary_keys_and_values` 提供真实 etcd 证据，但默认 `#[ignore]`，需要 `ASTER_ETCD_TEST_ENDPOINT`。Rust 迁移补充测试比 Go 同目录测试更直接地固定删除重试边界。

## 扩展指南

- 新增真实 etcd 操作时，优先在 `impl NamespacedClient<etcd_client::Client>` 中接入并复用 `prefixed_key`；若响应包含 key，还要像 `get` 一样决定是否向上层剥离前缀。同步扩展 `pkg/util/etcd/etcd_test.rs`，二进制 key/value 行为需要真实 etcd 或等价协议级证据。
- 若要对 Watch、Lease、事务或批量操作达到 Go 对等，必须先核对 `etcd-client` 对每类请求中所有 key/range_end 的表达方式；不能只给首个 key 加前缀。尤其 prefix range、事务比较条件和 watch 返回 key 都需要双向转换测试。
- 扩展删除策略时修改 `delete_key_from_etcd`，并同步 `pkg/util/etcd/migration_aster_unit_test.rs`。应保持“每轮独立 timeout、首个成功停止、耗尽返回最后错误、零重试空操作”的既有兼容契约，除非上层调用者和 Go 差异被明确评审。
- 如需实际使用 `KEY_OP_RETRY_INTERVAL`，应在调用方或明确的新策略层实现，避免无意改变现有 helper 的无 sleep 行为和总延迟。
- 修改 namespace 后应检查 `pkg/store/etcd.rs::{NewEtcdCli, EtcdClient::Put, EtcdClient::Get}` 及其测试，因为这里是当前生产接线。还需注意 `inner_mut` 的绕过路径。
- 性能风险主要是每次操作复制前缀/key、get 复制所有返回 KV，以及高频错误文本形成指标高基数；兼容风险主要是漏加/重复添加前缀和把二进制 key 错转为字符串。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/util/etcd` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file pkg/util/etcd/etcd.rs --offset 1 --limit 500` 读取了完整 267 行实现，并报告 35 个符号及 6 个文件级使用者；精确 `query` 定位了 `NamespacedClient`、`DeleteClient`、`set_etcd_client_namespace`、`delete_key_from_etcd`、`SetEtcdCliByNamespace` 和 `DeleteKeyFromEtcd`。
- RustCodeGraph `node` 读取 `pkg/store/etcd.rs`，确认构造、namespace 设置和 put/get 调用边；读取 `pkg/lib.rs` 确认门面再导出。精确 `callers` 命令在本地索引上约 30 秒超时且没有输出，因此又用精确符号搜索核对生产调用者，并把未确认的文件级结果排除为直接调用证据。
- 已读源与配置：`pkg/util/etcd/etcd.rs`、`pkg/util/etcd/lib.rs`、`pkg/util/etcd/Cargo.toml`、`pkg/store/etcd.rs`、`pkg/lib.rs`。
- 已读对照与测试：`pkg/util/etcd/etcd.go`、`pkg/util/etcd/etcd_test.go`、`pkg/util/etcd/etcd_test.rs`、`pkg/util/etcd/migration_aster_unit_test.rs`。`pkg/util/etcd` 与 `pkg/util` 均无 `doc.go`。
- 人工复核结论：本文区分了当前生产接线与仅公开未接线能力，明确 Go Watcher/Lease 对等缺口、二进制边界、错误传播、逐次取消语义及扩展测试位置；没有把 Cargo 依赖或文件级图关系误写成直接调用。
