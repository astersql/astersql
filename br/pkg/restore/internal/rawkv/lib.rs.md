# `br/pkg/restore/internal/rawkv/lib.rs`

## 文件定位

`lib.rs` 是 workspace 成员 crate `astersql-br-pkg-restore-internal-rawkv` 的根文件；`br/pkg/restore/internal/rawkv/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定它，并在 `package.metadata.porting.go-package` 中把该 crate 对应到 Go 包 `br/pkg/restore/internal/rawkv`。根 `Cargo.toml` 将此目录列为 workspace member。

这个文件本身不实现 RawKV 算法，而是模块门面：用 `#[path = "rawkv_client.rs"]` 挂载实现模块，以 `pub use rawkv_client::*` 扁平导出其公开 API，并仅在测试构建中挂载 `parity_test.rs` 与 `rawkv_client_test.rs`。当前 Rust 生产链尚未消费这个 crate：仓库搜索只找到本 crate 的测试引用这些导出，而 `br/pkg/restore/log_client/client.rs` 仍使用 `br/pkg/restore/log_client/stubs.rs` 内的 `stubs::rawkv::RawKVBatchClient`。Go 生产链则由 `br/pkg/restore/log_client/client.go::SetRawKVBatchClient` 调用本 Go 包。

## 核心职责

1. 确立 `rawkv_client.rs` 为该 crate 的实现模块，并把模块公开为 `rawkv_client`。
2. 将实现模块的全部公开项再导出到 crate 根，使使用方可直接写 `crate::RawKVBatchClient`、`crate::NewRawkvClient` 等；两个 Rust 测试文件正是通过这种路径导入符号。
3. 保证测试逻辑与生产实现分文件存放：`parity_test` 和 `rawkv_client_test` 只在 `cfg(test)` 下参与编译，不进入普通库构建。
4. 在迁移期统一放宽 Go 风格命名和暂未使用代码的 lint。这里的 crate 级 `allow` 是兼容措施，不代表这些命名符合惯用 Rust 风格。

它不负责连接 PD、缓存 KV、批量写入、去重或指标记录；这些行为全部位于 `rawkv_client.rs`。也不能仅凭门面导出来断言 Rust BR 已经接入真实 TiKV，因为当前默认拨号器会明确报“未配置”，且上层 Rust restore 仍走独立桩类型。

## 主要符号

`lib.rs` 没有自定义常量、结构体、trait 或函数，只有模块声明和再导出。经 `pub use rawkv_client::*` 暴露的关键 API 如下：

- `RAWKV_CUSTOM_TIMEOUT: Duration`：固定为 10 秒，传给 `PdRawkvDialer::NewClient`，对应 Go 的 `opt.WithCustomTimeoutOption(10*time.Second)`。
- `Error`、`Result<T>`：本地轻量错误及结果别名；`Error::Trace` 当前是恒等包装。
- `Context`、`Security`、`RawOption`、`SetColumnFamily`：分别模拟 Go context、TiKV TLS 配置和 RawKV 列族选项。
- `RawkvClient`：底层 RawKV 能力 trait，包含 `Get`、`Put`、`BatchGet`、`BatchPut`、`Close`，并要求实现者 `Send + Sync`。
- `PdRawkvDialer`：把 PD/TiKV 客户端创建隔离为可注入边界。
- `NewRawkvClient`：使用默认拨号器创建客户端；当前默认实现 `UnconfiguredPdDialer` 总是返回明确错误，未连接真实 PD/TiKV。
- `NewRawkvClientWithDialer`：可注入拨号器的构造入口，并固定转发 `RAWKV_CUSTOM_TIMEOUT`。
- `RawKVBatchClient`、`NewRawKVBatchClient`：有状态的批量写入器及构造函数。
- `metrics`：进程内观测桩，记录批大小和耗时；其 `take_*`、`clear_observations` 主要服务测试。

两个私有测试模块 `parity_test` 与 `rawkv_client_test` 不是 crate 的公开 API。`rawkv_client` 模块本身公开，因此调用方既可通过 crate 根的扁平名字，也可通过 `rawkv_client::...` 访问公开项。

## 执行流程

普通库构建时，编译器先应用 crate 级 lint 放宽项，然后根据显式路径载入 `rawkv_client.rs`，最后把该模块的公开项提升到 crate 根；两个 `cfg(test)` 模块被排除。

通过门面使用批客户端时，主流程是：

1. 调用方提供 `Arc<dyn RawkvClient>`，用 `NewRawKVBatchClient` 创建写入器，并用 `SetColumnFamily` 设置目标列族。
2. `RawKVBatchClient::Put` 通过私有 `TruncateTS` 去掉 key 末尾八字节时间戳，以逻辑 key 作为 `HashMap` 键。
3. 新逻辑 key 增加 `size`；重复逻辑 key 只有在 `originTs` 更大时才替换原条目，因此单批内保留较新版本。
4. 当不同逻辑 key 数达到 `cap`，收集 map 中的原始 key/value，调用底层 `BatchPut`，附带 `SetColumnFamily(self.cf)`，并无论成功失败都记录批大小和耗时。
5. `BatchPut` 成功后 `reset` 清空 map 和计数；失败则经 `Error::Trace` 返回并保留缓冲，供上层决定是否重试。
6. 结束输入时，调用 `PutRest` 刷出未满批的条目；空缓冲直接成功。最终可调用 `Close` 尽力关闭底层客户端。

创建底层客户端的另一条流程是 `NewRawkvClient -> NewRawkvClientWithDialer -> PdRawkvDialer::NewClient`。当前默认 dialer 不具备网络实现，因此只有显式注入实现时才可能成功；`parity_test.rs` 的 `RecordingDialer` 用于验证地址和 10 秒超时被原样转发。

## 数据与状态

门面文件自身没有运行时状态。实现模块中的主要状态集中在 `RawKVBatchClient`：

- `cf` 保存后续 `BatchPut` 使用的列族；未调用 `SetColumnFamily` 时为空字符串，代码本身不拒绝该值。
- `cap` 是以“批内不同逻辑 key 数”为单位的刷写阈值。
- `size` 与 `kvs.len()` 共同描述当前批的不同逻辑 key 数；重复 key 覆盖不会递增。
- `kvs: HashMap<Vec<u8>, KVPair>` 以截断时间戳后的逻辑 key 索引，`KVPair` 保存比较用的 `ts` 和最终下发的完整 key/value。
- `rawkvClient: Arc<dyn RawkvClient>` 共享底层客户端所有权，但批写器本身的方法需要 `&mut self`，没有声明为可并发共享。

`Context` 用 `Arc<Mutex<Option<Error>>>` 保存取消原因；当前批写流程没有检查 `Context::Err`，只是把 context 引用传给底层 trait。`metrics` 使用两个全局 `Mutex<Vec<Observation>>` 保存观测，测试取值时会清空。`HashMap` 迭代顺序不稳定，因此批次内部的下发顺序没有保证，测试在比较前排序。

## 依赖与调用关系

crate 边界由 `br/pkg/restore/internal/rawkv/Cargo.toml` 定义，`[dependencies]` 为空；实现仅使用标准库集合、同步原语和时间类型。Go 版本依赖 `tikv/client-go/rawkv`、PD option、restore utils、metrics 和 `hack.String`，Rust 版本则以内联类型、trait、`TruncateTS` 和 metrics 桩替代这些依赖。

Rust 侧内部调用边为：

- `NewRawkvClient` 调用 `NewRawkvClientWithDialer`，后者调用 `PdRawkvDialer::NewClient`。
- `RawKVBatchClient::Put` 调用 `TruncateTS`；满批时调用 `RawkvClient::BatchPut`、`SetColumnFamily`、两个 metrics 观测函数，成功后调用 `reset`。
- `RawKVBatchClient::PutRest` 复用同一批写、观测和成功清理顺序。
- `RawKVBatchClient::Close` 调用底层 `RawkvClient::Close`，但丢弃其错误。

上游现状需要区分语言：Go 的 `br/pkg/restore/log_client/client.go::SetRawKVBatchClient` 调用 `NewRawkvClient` 和 `NewRawKVBatchClient`，随后由日志恢复流程持有并使用。Rust 的 `br/pkg/restore/log_client/client.rs::LogClient` 当前持有的是同目录 `stubs.rs` 定义的另一种 `RawKVBatchClient`；根 `br/pkg/restore/Cargo.toml` 也没有依赖本 crate。因此此文件当前是可独立编译和测试的移植单元，不是 Rust restore 生产主链的已完成接线点。

## 错误处理与边界

- `NewRawkvClient` 当前必然返回“rawkv PD dialer not configured”错误；这是显式的迁移边界，不应描述为真实集群客户端创建成功。
- `NewRawkvClientWithDialer` 直接传播注入 dialer 的错误，不附加地址或安全配置上下文。
- `Put`/`PutRest` 在底层 `BatchPut` 失败时返回同一错误内容，并且不调用 `reset`。这保留了当前批，但接口没有内建重试或幂等保证；是否重试由上层决定。
- 指标在 `BatchPut` 返回之后、检查错误之前记录，所以失败尝试也计入批大小与耗时。
- `Close` 明确忽略底层关闭错误，属于尽力清理语义。
- `TruncateTS` 对空 key 或短于八字节的 key 不报错，而是原样作为逻辑 key。调用方必须确认这是否符合其 key 编码协议。
- `cap <= 0` 没有构造期校验；第一次插入新逻辑 key 后即满足 `size >= cap` 并触发刷新。扩展时若要拒绝非法容量，需要同步定义与 Go 的兼容行为。
- 相同逻辑 key 且 `originTs` 相等时，先到条目保留；只有严格更大的时间戳覆盖。
- 去重只发生在当前缓冲批内；成功 `reset` 后，同一逻辑 key 的另一个版本可以在后续批次再次写入。
- `Mutex::lock().unwrap()` 在锁中毒时会 panic；当前轻量 context 和 metrics 桩没有把锁错误转换为 `Result`。

## 并发与资源生命周期

`RawkvClient` 和 `PdRawkvDialer` 要求 `Send + Sync`，并通过 `Arc` 共享；但 `RawKVBatchClient::Put`、`PutRest`、`SetColumnFamily` 都需要可变借用，类型注释也明确其不是线程安全的批处理器。若多个任务需要写入，应在更高层串行化或为每个任务建立独立批客户端，不能仅因底层 trait 可共享就并发修改同一个批缓冲。

一个正常生命周期是：创建/注入底层 client → 创建 batch client → 设置 CF → 多次 `Put` → `PutRest` → `Close`。类型未实现 `Drop` 自动刷写或关闭；遗漏 `PutRest` 会留下尚未写出的残余数据，遗漏 `Close` 则由底层 `Arc` 的实现自行承担释放后果。`Close` 接收 `&self` 且没有本地 closed 标志，多次调用会多次转发；当前测试只验证一次调用。

批写失败后缓冲仍在，允许调用方再次触发写入或调用 `PutRest`，但新输入可能继续修改该缓冲。由于 RawKV 批写是否幂等取决于下游协议与 key 设计，增加自动重试前必须验证真实 TiKV 客户端语义。全局 metrics 向量会持续增长，除测试辅助函数外没有清理策略；它是本地移植桩，不应被当作生产 Prometheus 实现。

## 与 Go 版本的对应关系

`rawkv_client.rs` 机械上对应 `rawkv_client.go` 的接口、10 秒创建超时、`KVPair`、`RawKVBatchClient` 以及 `Put`/`PutRest`/`reset` 流程。两边都按截断 TS 后的逻辑 key 去重，保留较大 `originTs`，达到不同 key 容量后批写，成功才清空，并忽略 `Close` 错误。

关键差异如下：

- Go `NewRawkvClient` 直接调用 `tikv/client-go` 创建真实客户端；Rust 默认 dialer 是明确失败的本地桩，真实网络能力尚未接入。
- Go 使用 `context.Context`、`config.Security`、可变参数 `rawkv.RawOption`、Prometheus 指标及 `errors.Trace`；Rust 使用本地轻量等价物、切片 options、内存观测向量和恒等 `Trace`。
- Go 用 `utils.TruncateTS` 与 `hack.String`；Rust 内联截断逻辑并拥有 `Vec<u8>` map key，避免借用输入缓冲，但会产生分配/拷贝。
- Go 构造函数返回指针；Rust 返回拥有值。Go 的 map 与 Rust `HashMap` 都不保证迭代顺序。
- Rust 额外提供 `NewRawkvClientWithDialer`，用来在无 PD/TiKV 依赖的环境中注入 fake 并验证创建契约。

测试对应关系明确：`rawkv_client_test.rs::{test_raw_kv_batch_client,test_raw_kv_batch_client_duplicated}` 对齐 Go 同名测试；`parity_test.rs::go_rust_public_contract_matches` 又覆盖空 `PutRest`、较小 TS 不覆盖、失败保留、默认 dialer 错误、超时/地址转发、指标与关闭调用。Go 的上层使用还由 `br/pkg/restore/log_client/client_test.go` 验证重试场景，但 Rust 上层目前使用桩，不能把该 Go 测试视为 Rust 接线证明。

## 扩展指南

- 若要接入真实 Rust BR 主链，优先在独立上游客户端仓库实现并发布所需 RawKV/PD 能力，再以带 tag 的 Git 依赖接入；不得把外部依赖复制到 `vendor/`、`third_party/` 或用本地 `[patch]`。随后需要让 `br/pkg/restore/log_client` 依赖本 crate，并移除或收窄其重复的 `stubs::rawkv`，同时增加独立集成测试。
- 新增底层操作时，先扩展 `RawkvClient` trait 及真实/fake 实现，再决定是否通过 `pub use rawkv_client::*` 自动暴露。trait 变更会影响所有 fake，实现与测试必须同步更新。
- 修改批写策略时，应集中修改 `RawKVBatchClient::{Put,PutRest,reset}`，保持“成功才清空”“批内去重”“CF 透传”和失败指标的既有可观察语义；若有意改变，必须同步 Go 对照分析和两个独立 Rust 测试文件。
- 若抽取重复 flush 逻辑，应保持错误发生时的缓冲内容与指标顺序不变，并增加针对 `PutRest` 失败后重试的回归用例。
- 若引入线程安全包装，不要只给 `RawKVBatchClient` 增加 `Sync`；需要明确锁粒度、flush 期间是否允许插入、`SetColumnFamily` 与批次的原子边界，以及 `Close`/`PutRest` 的竞态语义。
- 若公开 API 不再需要扁平导出，应先迁移所有 `crate::{...}` 测试与外部使用者，再收窄 `pub use`；否则会造成源码兼容性破坏。
- 测试必须继续放在独立文件。直接行为对齐优先更新 `rawkv_client_test.rs`，迁移边界、错误、指标和资源契约更新 `parity_test.rs`；真实接线另增上层测试，不要把网络测试内嵌进 `lib.rs`。

## 验证依据

本说明基于以下直接证据：

- `br/pkg/restore/internal/rawkv/lib.rs`：RustCodeGraph `node --file` 显示该文件共 30 行，声明一个公开实现模块、一次扁平再导出和两个 `cfg(test)` 独立测试模块。
- `br/pkg/restore/internal/rawkv/Cargo.toml` 与根 `Cargo.toml`：确认 crate 名、`lib.rs` 入口、Go 包映射、空依赖表和 workspace 成员关系。
- RustCodeGraph 对 `RawkvClient` 的查询：确认 Go interface 与 Rust trait 的 `Get`/`Put`/`BatchGet`/`BatchPut`/`Close` 表面相对应。
- RustCodeGraph 对 `rawkv_client.rs` 的分段 `node --file`：确认创建、10 秒超时、去重、满批/残余刷新、错误保留、指标、关闭和短 key 边界。
- `br/pkg/restore/internal/rawkv/parity_test.rs`：确认满批、残余、批内/跨批重复、较小 TS、BatchPut 错误、未配置 dialer、地址/超时、指标和 Close 契约。
- `br/pkg/restore/internal/rawkv/rawkv_client_test.rs` 与 `rawkv_client_test.go`：确认两个同名核心用例及 batchCount=3 的期望。
- `br/pkg/restore/internal/rawkv/rawkv_client.go`：确认 Go 生产实现和 Rust 移植的逐项对应关系。
- `br/pkg/restore/log_client/client.go::SetRawKVBatchClient`、`br/pkg/restore/log_client/client.rs::LogClient` 与 `br/pkg/restore/log_client/stubs.rs::rawkv`：确认 Go 已接线而 Rust 上层仍使用另一桩类型。
- 仓库 `rg` 搜索：未发现非测试 Rust 文件依赖 `astersql-br-pkg-restore-internal-rawkv` 或调用其构造函数；因此本文把 Rust 生产接线标记为未完成，而不是从 Go 主链推断 Rust 已支持。

本任务是纯文档分析，未运行 Cargo。交付结构验证要求目标文件存在且恰好包含上述十一个固定二级标题。
