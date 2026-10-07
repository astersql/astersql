# `pkg/ingestor/ingestcli/interface.rs`

## 文件定位

[源文件 `interface.rs`](./interface.rs) 是 `astersql-ingestor-ingestcli` crate 的公共协议层，定义“把 KV 流式写给 TiKV worker 生成 SST，再把该 SST 导入目标 Region”所需的数据类型和 trait。crate 根 `pkg/ingestor/ingestcli/lib.rs` 将 `interface` 声明为私有子模块，再以 `pub use interface::*` 重新导出这些符号；真正的 HTTP、后台线程和线格式实现位于同 crate 的 `client.rs`，错误分类位于 `ingest_err.rs`。

`pkg/ingestor/ingestcli/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/ingestor/ingestcli`，库入口为 `lib.rs`。本文件自身只直接依赖标准库的 `Arc` 和 crate 根导出的 `Error`、`NextGenSstMeta`，不执行网络 I/O，也没有条件编译项、模块级常量或自由函数。

当前接线范围要谨慎理解：Rust 生产代码中，`ClientImpl`、`WriteClientImpl`（`client.rs`）和独立 mock（`mock/client_mock.rs`）实现这些 trait；搜索 `pkg/ingestor/**/*.rs` 未发现 Rust `ingestctrl` 生产路径消费该 crate。完整应用级调用顺序目前仍由 Go `pkg/ingestor/ingestctrl/job_worker.go` 直接证明。因此本文件是可用的 Rust 接口与实现边界，但不能据此宣称 Rust 导入控制主链已经完成接线。

## 核心职责

1. 用 `Pair`、`WriteRequest` 和 `WriteResponse` 表达一次或多次 KV 写入及最终生成的 SST 元数据。
2. 用 `RegionEpoch`、`Peer`、`Region`、`RegionInfo`、`Store` 和 `IngestRequest` 提供 ingest 定位所需的最小 Region/Store 模型，避免接口层依赖 Go 的 protobuf 类型。
3. 用 `WriteClient` 规定“多次写、一次收尾接收、错误路径关闭”的流式生命周期，用 `Client` 规定创建写流和执行 ingest 的两个入口。
4. 用 `SplitClient` 隔离按 `store_id` 查询 TiKV 状态地址的 PD 能力，用 `RequestContext` 保留请求边界的取消检查，用 `SharedSplitClient` 统一线程安全共享方式。
5. 提供 `Write`、`Recv`、`Close`、`WriteClient`、`Ingest` 这些 Go 风格兼容别名；它们只负责克隆必要的请求并转发给 snake_case 核心方法，不包含另一套行为。

## 主要符号

- `Pair { key, value }`：拥有键和值的字节向量；`Clone + Default + Eq` 便于请求转发、构造和精确断言。
- `WriteRequest { pairs }`：一个写批次。接口不限制批次数、单批大小、键顺序或空批次；这些策略由上游和实现负责。
- `WriteResponse { pub(crate) next_gen_sst_meta }`：封装写流生成的 `NextGenSstMeta`。字段仅 crate 内可写，外部通过 `sst_meta(&self) -> Option<&NextGenSstMeta>` 只读访问，因此“写成功但没有元数据”可显式表示为 `None`。
- `RegionEpoch { conf_ver, version }`：分别表达副本配置版本和键范围版本。当前 `ClientImpl::ingest` 只把 `version` 放入 HTTP URL，`conf_ver` 仍是模型的一部分但不由此实现发送。
- `Peer { id, store_id }`：Region 副本身份及所在 Store；当前 ingest 使用 leader 的 `store_id` 查地址。
- `Region { id, start_key, end_key, region_epoch, peers }`：目标键范围及拓扑。当前 HTTP ingest 实现使用 `id` 和可选 epoch 的 `version`，其余字段为完整边界模型和上游决策保留。
- `RegionInfo { region, leader }`：把 Region 与可选 leader 绑定；leader 缺失是合法可表示状态，由具体实现返回错误。
- `IngestRequest { region, write_response }`：将写阶段结果与目标 Region 组合，是 `Client::ingest` 的所有权参数。
- `Store { status_address }`：`SplitClient` 返回的最小 Store 视图，供 `ClientImpl` 拼接 `/ingest_s3` 地址。
- `SplitClient: Send + Sync`：定义 `get_store(&dyn RequestContext, store_id) -> Result<Store, Error>`，允许 `ClientImpl` 跨线程安全持有 PD 侧适配器。
- `RequestContext: Send + Sync`：只暴露 `is_cancelled()`；默认返回 `false`，且 `()` 实现该 trait，便于无取消需求的调用和测试。
- `WriteClient: Send`：核心方法为 `write(&mut self, WriteRequest)`、`recv(&mut self)`、`close(&mut self)`。可变借用强制同一实例上的操作串行化；它没有 `Sync` 约束。
- `Client: Send + Sync`：核心方法为 `write_client(&self, context, commit_ts)` 和 `ingest(&self, context, IngestRequest)`，允许通过共享引用并发发起彼此独立的请求。
- `SharedSplitClient = Arc<dyn SplitClient>`：动态分发且引用计数的 SplitClient 句柄。

## 执行流程

接口预期的主流程由 `Client`/`WriteClient` 契约、`client.rs` 实现和 Go 上游共同确认：

1. 上游取得 commit timestamp，调用 `Client::write_client(context, commit_ts)`。`ClientImpl::write_client` 先检查 `RequestContext::is_cancelled`，随后创建 `WriteClientImpl`；实现会启动后台线程并建立容量为 8 的同步通道。
2. 上游把 KV 分批包装成 `WriteRequest`，对同一 `WriteClient` 多次调用 `write`。`WriteClientImpl::write` 把每对 KV 编为 `u16` 小端 key 长度、key、`u32` 小端 value 长度、value，再送往后台 HTTP PUT 流。
3. 全部批次发出后调用 `recv`。实现关闭发送端、等待后台线程，收集 HTTP `/write_sst` 返回的 `NextGenSstMeta`，并把它放入 `WriteResponse`。后台服务错误可能直到这一步才显现，`client_test.rs::test_client_write_server_error` 固化了该时序。
4. 上游把 `WriteResponse` 与目标 `RegionInfo` 组成 `IngestRequest`，调用 `Client::ingest`。实现检查取消状态，要求存在 leader，经 `SplitClient::get_store` 查询 leader Store 的 `status_address`，再携带 cluster、Region id、epoch version 和 SST 元数据 POST `/ingest_s3`。
5. 任何提前退出路径都应调用 `close`。Go 的 `objStoreRegionJobWorker.write` 在创建流后立即 `defer writeCli.Close()`，随后分批 `Write` 并最终 `Recv`；Rust `WriteClientImpl` 还以 `Drop` 作为兜底，但 trait 契约仍明确要求调用者在错误路径关闭。

大写兼容方法没有独立流程：`Write` 克隆借用的 `WriteRequest` 后调用 `write`，`Ingest` 克隆借用的 `IngestRequest` 后调用 `ingest`，其余别名直接转发。

## 数据与状态

接口数据全部为拥有型 Rust 值。字节字段用 `Vec<u8>`，请求嵌套值直接持有；可缺失的 SST、epoch、leader 用 `Option` 表示。除 `WriteResponse::next_gen_sst_meta` 为 `pub(crate)` 外，数据结构字段均公开，调用者可以直接构造协议对象。

重要状态不变量如下：

- 一个 `WriteResponse` 只有在 `sst_meta()` 返回 `Some` 时才能被当前 `ClientImpl::ingest` 使用；否则后者返回 `Error::MissingSstMeta`。
- 当前 ingest 实现要求 `RegionInfo::leader` 存在；缺失时返回 `Error::MissingLeader`。`region_epoch` 缺失不会在接口层阻止请求，具体实现以版本 `0` 拼 URL。
- `RequestContext` 不携带 deadline、值或唤醒机制，只提供调用时快照式取消检查。`()` 的默认实现永不取消。
- `WriteClient` 的顺序状态（可写、已收尾、已关闭）没有编码进类型系统，由实现维护。`WriteClientImpl` 取走 sender 和 worker 后，再写会得到 `ClosedPipe`；`close` 丢弃收尾错误。
- `Pair`、请求和 Region 类型派生 `Clone`，主要服务于 Go 风格借用别名和 mock；大请求经 `Write`/`Ingest` 别名调用会产生深拷贝，snake_case 方法则直接取得所有权。

## 依赖与调用关系

下游依赖：

- `WriteResponse` 依赖 crate 根的 `NextGenSstMeta`；该类型及 JSON/HTTP 处理在 `client.rs`。
- 三个 trait 的失败统一使用 crate 根 `Error`；实际 HTTP、取消、leader/SST 缺失和 ingest API 错误由 `client.rs`、`ingest_err.rs` 产生。
- `SharedSplitClient` 依赖 `std::sync::Arc`；接口没有直接依赖异步运行时或网络库。

直接实现者与调用边：

- `client.rs::WriteClientImpl` 实现 `WriteClient`，`client.rs::ClientImpl` 实现 `Client`；`NewClient` 返回 `Arc<dyn Client>`。
- `mock/client_mock.rs::MockClient` 和 `MockWriteClient` 分别实现两个核心 trait，供独立测试配置期望与响应。
- `interface_test.rs::ContextAwareSplitClient` 实现 `SplitClient`，验证传入的同一个 `RequestContext` 能到达 Store 查询边界。
- RustCodeGraph 显示别名边 `WriteClient::Write -> WriteClient::write`、`Recv -> recv`、`Close -> close`、`Client::WriteClient -> write_client`、`Ingest -> ingest`；`client_test.rs` 直接覆盖具体实现的 `write_client`、`write`、`recv`、`ingest` 和 `sst_meta`。

应用位置以 Go 版本为当前直接证据：`pkg/ingestor/ingestctrl/job_worker.go::objStoreRegionJobWorker` 持有 `ingestcli.Client`，在 `write` 中创建流、分批写入、接收结果，在 `ingest` 中组装请求并导入。Rust 生产树搜索目前未发现对应 `ingestctrl` 消费者，故 Rust 应用级上游标为“尚未验证接线”，而不是推断存在。

## 错误处理与边界

接口不包装错误，所有核心方法直接传播 `crate::Error`。具体实现证明了以下边界：请求开始时取消返回 `Error::Canceled`；缺 leader 返回 `MissingLeader`；写结果无 SST 返回 `MissingSstMeta`；Store 查询、HTTP 传输、非 200 响应、JSON/protobuf 解码和后台线程 panic 都沿统一错误类型返回。

写流具有延迟报错特性：`write` 只保证数据成功进入同步通道；服务端非 200 或响应解析失败可能在 `recv`/`finish` 时返回。调用者不能把一次成功的 `write` 当成服务端已接受数据。`close` 无返回值，适合清理但不能代替需要结果与错误的 `recv`。

类型层没有验证 key/value 长度能否装入实际线格式的 `u16`/`u32`，也不验证空请求、Region 范围、peer/leader 一致性、epoch 新鲜度或 commit timestamp；这些不是本接口当前承诺。扩展文档或调用者时不应凭字段存在推断这些校验已经实现。

Go 风格别名会克隆请求，因此保持语义但可能增加内存和 CPU 成本。需要处理大批 KV 的新 Rust 调用者应优先使用取得所有权的 snake_case 方法，并在选择兼容别名前评估复制代价。

## 并发与资源生命周期

`Client`、`SplitClient`、`RequestContext` 要求 `Send + Sync`，可通过 `Arc` 在多个线程间共享；`Client` 方法只借用 `&self`。`WriteClient` 只要求 `Send` 且操作需要 `&mut self`，表达“写流可转移线程，但同一时刻由一个所有者按序驱动”的约束。

接口本身不创建线程、锁或通道，但其生命周期契约由 `WriteClientImpl` 落实：每条写流拥有一个容量 8 的 `sync_channel`、一个后台 HTTP worker、一个受 `Mutex` 保护的最终结果。`recv`/`close` 会关闭发送端并 join worker；`Drop` 再兜底调用 `finish`，避免正常所有权释放时遗留 worker。调用者仍应显式 `recv` 获取结果，或在错误路径显式 `close`，不能依赖析构取得被丢弃的错误。

`RequestContext` 只在 `ClientImpl::write_client`、`ClientImpl::ingest` 和 `SplitClient::get_store` 边界传递。它不保证已启动的后台写线程能响应随后发生的取消；这与 Go `context.Context` 绑定到 HTTP request、可中途取消的能力不同，是当前 Rust 接口的重要限制。

## 与 Go 版本的对应关系

`interface.rs` 对应 `pkg/ingestor/ingestcli/interface.go`，保留了 `WriteRequest`、`WriteResponse`、`IngestRequest`、`WriteClient` 和 `Client` 的核心职责及调用顺序。`Write`/`Recv`/`Close`、`WriteClient`/`Ingest` 大写别名让命名也可与 Go 对照。

主要迁移差异是：

- Go 的 KV 使用 `[]*import_sstpb.Pair`，Region 使用 `*split.RegionInfo`；Rust 定义拥有型本地镜像 `Pair`、`RegionInfo` 等，未直接绑定 protobuf/split crate。
- Go 请求大量使用指针且可以为 nil；Rust 必填嵌套值直接拥有，仅真正可缺失的 SST、epoch、leader 使用 `Option`，因此空值状态更明确。
- Go `WriteResponse.nextGenSSTMeta` 私有；Rust同样限制字段为 crate 可见，并增加公开只读方法 `sst_meta()`。
- Rust 新增最小 `SplitClient`、`Store` 和 `SharedSplitClient` 边界；Go 的 `client` 直接持有 `split.SplitClient`，该接口定义在 BR split 包。
- Go `context.Context` 支持 deadline、取消通知和值传播，并绑定 HTTP 请求；Rust `RequestContext` 目前只有 `is_cancelled`，只在请求边界轮询，不能视为完全等价。
- Go `Write`/`Ingest` 借用指针；Rust snake_case 方法取得请求所有权，大写兼容别名通过深克隆维持借用调用形态。
- Go `writeClient` 用 `io.Pipe`、goroutine 和 WaitGroup；Rust实现用有界同步通道、线程、`JoinHandle` 和 `Mutex<Option<Result<...>>>`。接口层保持相同的多写一次接收与错误路径关闭语义。

Go `job_worker.go` 是当前完整主链证据；Rust `client_test.rs` 证明具体传输语义，但 Rust `ingestctrl` 尚未发现生产接线。后续迁移不能只复制类型签名，还需保留上游分批、清理和延迟错误处理行为。

## 扩展指南

- 新增请求字段时，先确定它属于协议边界还是 HTTP 实现：公共、实现无关的数据放入本文件；编码、URL 和网络行为放入 `client.rs`。同步更新 `interface_test.rs` 或 `client_test.rs`，并核对 Go `interface.go`、`client.go` 的真实语义。
- 修改 `WriteClient` 生命周期时，应同时检查 `WriteClientImpl`、`MockWriteClient`、Go `job_worker.go` 的 `defer Close` 路径，以及“服务端错误在 `recv` 暴露”的测试。不要让 `close` 静默吞错的清理用途替代 `recv`。
- 扩展取消语义时，最可能修改 `RequestContext`、`ClientImpl::write_client`/`ingest` 和 HTTP transport；需新增独立测试验证开始前取消、进行中取消和传给 `SplitClient` 的上下文。保持 Rust 单元测试在独立 `*_test.rs` 文件中。
- 新增 Region/Store 字段时，要区分“为模型对齐保留”与“实际在线格式使用”；同步测试 URL、Store 选择、缺 leader/epoch 等边界，避免只增加未消费字段。
- 如需减少大请求复制，优先让新 Rust 调用方使用 snake_case 所有权 API；若改变 Go 风格别名签名或克隆策略，需要评估 mock、外部 crate API 兼容性和内存峰值。
- 将此接口接入 Rust `ingestctrl` 时，应以 Go `objStoreRegionJobWorker` 为行为基线：按大小分批、出错必清理、最后 `recv`、保存 `WriteResponse`、再按 Region 调 `ingest`；同时为生产接线另建独立测试，而不是把测试嵌入本源文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 `interface.rs` 被识别为 29 个符号。
- RustCodeGraph `node --file pkg/ingestor/ingestcli/interface.rs`：核对了本文件 174 行全部定义、可见性、派生、默认方法和 trait 约束。
- RustCodeGraph `query`：查询了 `WriteClient`、`Client`、`SplitClient`、`RequestContext`、`Pair`、`IngestRequest`，定位到 Rust trait、具体实现、mock、Go 接口和相关测试。
- RustCodeGraph 调用关系：确认五个 Go 风格默认方法向 snake_case 方法转发；确认 `client.rs::ClientImpl`/`WriteClientImpl` 为具体实现，`client_test.rs` 覆盖关键调用。
- 已读生产与声明文件：`pkg/ingestor/ingestcli/interface.rs`、`lib.rs`、`client.rs`、`Cargo.toml`，以及 Go 对照 `interface.go`、`client.go`、上游 `pkg/ingestor/ingestctrl/job_worker.go`。
- 已读独立测试：`pkg/ingestor/ingestcli/interface_test.rs` 验证 RequestContext 原样进入 `SplitClient`；`client_test.rs` 验证写入线格式、SST 访问、延迟 HTTP 错误、成功/失败 ingest、Store 地址选择和耗时指标。
- 生产 Rust 搜索 `astersql_ingestor_ingestcli|ingestcli::|WriteRequest|IngestRequest|SharedSplitClient`：消费点限于本 crate 实现和 mock，未发现 Rust `ingestctrl` 主链接线；这构成本文迁移状态判断的直接证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标文件存在且恰含 11 个固定二级标题，并人工复核未把接口模型误写成已实现校验或已接线功能。
