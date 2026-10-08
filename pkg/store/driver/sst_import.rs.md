# `pkg/store/driver/sst_import.rs`

## 文件定位

本文件属于 `astersql-store-driver` crate，由 `pkg/store/driver/lib.rs` 以公开模块 `pub mod sst_import` 挂载。它位于 `pkg/kv::Storage` 的抽象接口与 TiKV/PD gRPC 协议之间：`pkg/store/driver/kv_adapter.rs` 中 TiKV store 的 `ImportSSTWithOptions` 实现取得 PD 地址、TLS 配置和 keyspace ID 后，调用本文件的 `write_and_ingest_with_options`，再把内部 `ImportStats` 转成 `pkg/kv/kv.rs` 的 `SSTImportStats`。

这是物理 SST 导入传输层，不负责解析 SQL、生成索引 KV、检测业务层重复键或管理本地导入引擎。上游示例位于 `pkg/session/runtime/import_sst.rs`：`StoreBridge::WriteAndIngestData` / `WriteAndIngest` 先整理有序 KV、检查冲突，再调用 storage 接口。本文件只把已经准备好的 KV 按 Region 写到各 TiKV peer，并要求 leader 执行 `MultiIngest`。

## 核心职责

- 校验固定提交时间戳和输入键严格递增这一传输前置条件（`write_and_ingest_with_options`）。
- 按 API V1/V2 对逻辑键增加 TiKV keyspace 编码，再用 memcomparable 编码查询 PD Region 边界（`KeyCodec`、`region_key`）。
- 发现 PD leader、查询 Region 与 store 地址，并缓存各 store 的 `ImportSstClient`（`Connection`）。
- 为一个 Region 生成一个随机 SST UUID，把同一批 KV 以 Meta 帧加多个 WriteBatch 帧并发流式发送给该 Region 的全部 peers（`Connection::import_region`）。
- 从 leader 对应的 Write 响应取得 SST metadata，再向 leader 发起 `MultiIngest`；只有 ingest 成功后才推进游标和累计统计。
- 在网络/Region 拓扑类临时错误上重连、重新定位未完成范围并重试，同时传播取消和限流结果。

文件不提供事务 `Put/Commit` 降级路径；`write_and_ingest` 的文档注释和 `pkg/kv/kv.rs::Storage::ImportSST` 都要求不支持物理导入时明确报错，不能静默改成普通写入。

## 主要符号

- `type Result<T>`：本模块内部统一返回 `ImportError`。
- `type PdClient` / `type ImportClient`：分别是基于 tonic `Channel` 的 PD 与 ImportSST protobuf 客户端别名。
- `ImportError { message, retryable }`：公开错误类型，但字段私有。`permanent` 构造不可重试错误，`retry` 构造可重试错误；`From<tonic::Status>` 仅把 `Unavailable`、`DeadlineExceeded`、`ResourceExhausted` 标为可重试。
- `ImportStats { keys, bytes, write_rpcs, ingest_rpcs }`：成功导入统计。字段公开；计数在每个 Region ingest 成功后更新。
- `region_key(&[u8]) -> Vec<u8>`：实现 8 字节分组、`0xff` 标记和尾组补零的 memcomparable 编码，仅用于 PD/Region 范围；WriteBatch 中仍使用 `KeyCodec` 编码后的原始 TiKV 键。
- `channel(address, tls)`：补齐 HTTP/HTTPS scheme，设置 10 秒连接超时与 60 秒请求超时，并按 `TlsConfig` 装载 CA 和可选客户端证书/私钥。
- `check_header`：把 PD response header 中的逻辑错误转为可重试错误。
- `Connection`：持有 PD client、cluster ID、TLS 配置及按 store ID 缓存的 ImportSST clients。
- `Connection::connect`：依次尝试配置的 PD endpoints，通过 `GetMembers` 取得 cluster ID，并在存在 leader URL 时切到 PD leader。
- `Connection::store`：经 PD `GetStore` 解析 TiKV 地址，创建并缓存对应 client。
- `Connection::import_region`：本文件的单 Region 核心状态机，返回本次成功消费的 KV 数。
- `context`：构造带 Region epoch、peer、`internal_lightning:import` request source、事务来源和 API/keyspace 信息的 `kvrpcpb::Context`。
- `write_and_ingest`：使用 API V1、默认运行选项的便捷公开入口。
- `write_and_ingest_with_options`：支持 keyspace、取消和共享写限流器的完整公开入口。

本文件唯一条件编译项是末尾的 `#[cfg(test)] #[path = "sst_import_test.rs"] mod tests;`，测试逻辑保持在独立文件中。

## 执行流程

1. `write_and_ingest` 把参数转交 `write_and_ingest_with_options`，补入 `None` keyspace 与默认 `SSTImportOptions`。
2. 完整入口拒绝 `commit_ts == 0`、超出 `i64::MAX` 的时间戳，以及相邻键不满足严格递增的输入；空输入直接返回零统计，不建立网络连接。
3. 根据 `keyspace_id` 创建 V1 或 V2 `KeyCodec`，先把每个逻辑键编码成 TiKV 实际键。V2 路径把数值 keyspace ID 编入键前缀；测试确认 `0x010203` 对应的前缀字节进入 WriteBatch。
4. 入口创建启用全部组件的 Tokio 多线程 runtime，并在其中连接 PD。`Connection::connect` 逐个尝试 endpoints，通过 `GetMembers` 校验 header、保存 cluster ID，必要时改连 PD leader。
5. 外层循环从 `cursor` 指向尚未完成的有序 KV 后缀。`import_region` 用首键的 `region_key` 调用 PD `GetRegion`，校验 Region 和 leader，然后用 Region 的 memcomparable `end_key` 计算当前 Region 可消费的前缀长度。空前缀被视为 stale range，交给重试流程重新定位。
6. 单 Region 调用生成 UUID 和 `SstMeta`。其 range 起点是首键、终点是最后一键；这里的终点是 SST metadata 的最大键语义，不应当按一般半开区间自行加一。
7. 对每个 Region peer，先准备 Meta 帧，再惰性生成 WriteBatch 帧。每批按键值字节数累加，目标约 1 MiB；单个超大 KV 仍会单独成批。每批都复用同一 `commit_ts`。
8. 每个实际 batch 发送前检查取消；若存在 `SSTWriteLimiter`，通过 `spawn_blocking` 调用 `WaitN(context, store_id, size)`。这使阻塞式限流不占用 async executor worker，并让限流按 store、按实际传输批次生效。
9. 所有 peer 的 `Write` future 由 `try_join_all` 并发等待。任何传输错误、限流错误或 TiKV Write response error 都使本 Region 失败。成功后，从响应集合中选出 leader peer 返回的 metas；leader 缺失为可重试错误，非空输入却没有 metas 为永久错误。
10. 使用 leader store client 和 leader context 调用 `MultiIngest`。成功后才增加 `keys`、`bytes`、`write_rpcs`、`ingest_rpcs` 并返回消费数量。
11. 外层成功时推进 cursor 并清零连续失败次数；可重试失败最多触发 29 次重试（加首次尝试，单个未完成游标最多 30 次导入尝试），退避从 100ms 线性增加并封顶 1s，每次重试前重新连接 PD。`tokio::select!` 同时监听整个导入 context 的取消。

## 数据与状态

- 输入 `pairs` 在公开入口拥有所有权。完成 keyspace 编码后形成新的有序向量；单 Region 又把其前缀复制进 `Arc<Vec<_>>`，供多个 peer 的并发 Write stream 共享。该复制换取了独立异步流的所有权安全，也意味着峰值内存包含完整输入和当前 Region 前缀的副本。
- `cursor` 是跨 Region 的完成边界；只有 `import_region` 完整完成 Write 与 MultiIngest 后才前移。发生 Region split、leader 变化或模糊网络失败时，仍从同一未完成位置重新查询 PD。
- `Connection::stores` 以 store ID 缓存可克隆的 gRPC client；发生可重试导入错误时整个 `Connection` 被重建，旧缓存随之释放。
- `SstMeta.uuid` 每次调用 `import_region` 都重新生成。所有 peers 在同一次 Region 尝试中收到相同 meta；重试会生成新 UUID，但固定使用调用方给定的同一 commit TSO。
- `ImportStats.write_rpcs` 在发起并成功收齐本 Region 的 peer Write 后按 peer 数增加；`ingest_rpcs` 在发起 leader MultiIngest 前增加，但统计仅在整个函数最终成功返回时对调用者可见。某次失败尝试的远端 RPC 不会反映到返回的成功统计对象中。
- `keyspace_id.is_some()` 同时决定 KeyCodec、SST meta API version 与 RPC context API version；`None` 是 V1，`Some(id)` 是 V2，并写入 context 的数值 ID。

## 依赖与调用关系

上游调用链由精确源码引用确认：

`pkg/session/runtime/import_sst.rs::StoreBridge::{WriteAndIngestData, WriteAndIngest}` → `pkg/kv/kv.rs::Storage::ImportSSTWithOptions` → `pkg/store/driver/kv_adapter.rs` 的 TiKV store 实现 → `sst_import::write_and_ingest_with_options` → `Connection::import_region`。

其中 session 层负责本地 engine 生命周期、冲突检查、统计汇总与调用取消；kv trait 定义跨 store 契约；adapter 负责从具体 TiKV store 提取 PD/TLS/keyspace 配置；本文件负责物理网络协议。

下游依赖与 `pkg/store/driver/Cargo.toml` 一致：

- `tikv-client`（固定为 AsterSQL 上游 tag `v0.4.2-aster.10`）提供 PD、meta、KV RPC、ImportSST protobuf 类型；
- `tonic` 提供 TLS channel、client streaming 和 RPC status；
- `futures` 提供惰性 batch stream、Meta/Batch 串接与 peer 并发汇合；
- `tokio` 提供多线程 runtime、取消竞速、退避定时及阻塞限流桥接；
- `uuid` 生成 SST 标识；
- `astersql-kv` 提供 `SSTImportOptions`、取消 context 和 limiter 契约；
- `astersql-store-copr::network_backend::KeyCodec` 提供 API V1/V2 TiKV 键编码。

RustCodeGraph 对目标文件识别出 33 个符号，并确认内部直接边 `write_and_ingest` → `write_and_ingest_with_options`，以及 `import_region` → `region_key` / `check_header` / `Connection::header` / `Connection::store` / `context`。图索引对部分泛型同名符号产生跨包误配，因此跨文件上游关系使用上述精确源码引用复核。

## 错误处理与边界

- 永久错误包括非法 commit TSO、键未严格递增、URI/TLS 文件或 TLS 配置错误、runtime 构建失败、KeyCodec 初始化失败、取消、限流器失败、blocking task join 失败，以及 leader 对非空输入返回空 metas。它们不进入内部导入重试。
- 可重试错误包括连接失败、PD header error、缺少 Region/leader/store、stale Region range、leader 不在 peers、TiKV Write/Ingest response error，以及三种明确的 tonic status code。其他 tonic code 默认永久失败。
- `Connection::connect` 会容错单个 PD endpoint，但若所有 endpoint 都失败只返回最后一个错误；空 endpoint 列表返回“no PD endpoints configured”。重试阶段若重新连接本身失败，`?` 会立即退出，而不是继续保留原连接。
- 重试粒度是当前未完成 Region 后缀，且 commit TSO 不变。代码允许在 split 或 ingest 响应不确定后重新定位，但本文件没有额外的幂等确认查询；安全性依赖 TiKV ImportSST 协议对 SST/相同 MVCC 版本的处理。
- 严格递增检查发生在 keyspace 编码前；同一 keyspace 的固定前缀保持原键顺序。调用方不得混用不同 keyspace 的键，也不得传入已经自行添加 keyspace 前缀的键。
- 本文件不检查键值业务合法性、重复值冲突或 commit TSO 是否确由当前 PD 分配；这些是 `Storage` 调用方的责任。它只检查数值范围。
- `std::sync::Mutex` 仅保存 stream 生成阶段不能直接返回的失败。所有 `lock().unwrap()` 假设该短生命周期 mutex 不会被其他代码 panic 污染；一旦发生 poisoning 会 panic，而不是返回 `ImportError`。

## 并发与资源生命周期

- 公共 API 是同步函数，每次非空导入都会创建一个新的 Tokio 多线程 runtime，并用 `block_on` 执行完整导入；runtime 在函数返回时销毁。调用方应把它视为阻塞边界，避免在需要保持响应性的 async worker 上直接调用。
- 同一 Region 的 peer Write 并发执行，确保每个副本都先收到同一 SST 内容；MultiIngest 严格等待全部 Write 成功后才发送给 leader。不同 Region 由外层 cursor 顺序处理，不并行导入。
- 每个 peer 的 batch stream 惰性生成，限流在 batch 形成后、发送前执行。共享 limiter 必须线程安全，并遵守 `SSTWriteLimiter` 的约定：context 取消后停止等待。
- 双层取消路径分别是外层 `tokio::select!` 和 batch 生成时的显式检查。独立测试证明 limiter 阻塞期间取消会返回错误、不发送 batch、也不进入 ingest。
- `Arc` 保护 Region pairs、options 以及跨 stream 的错误槽；store clients 通过 tonic clone 共享底层 channel。没有常驻后台任务或显式文件句柄；TLS 文件只在建连时同步读取。
- RPC deadline 由 `channel` 的 endpoint 统一设为 60 秒，连接 deadline 为 10 秒；内部线性退避最长 1 秒。最坏完成时间还会乘以 endpoint 数、Region 数和最多 30 次尝试，调用方应依靠 context 取消控制整体生命周期。

## 与 Go 版本的对应关系

`pkg/store/driver` 中不存在 `sst_import.go`，`pkg/store/driver/tikv_driver.go` 也没有与 `write_and_ingest_with_options` 一一对应的 Go 实现。因此本文件不是“同路径 Go 文件”的机械逐函数翻译，而是 Rust storage 适配链为物理导入补出的专用 transport；这一差异必须保留，不能虚构 Go 对应符号。

协议层可由两处 Go 代码交叉核对：

- `pkg/lightning/tikv/local_sst_writer_test.go` 的手工 TiKV 写入流程同样创建 `ImportSSTClient`，依次发送 `SSTMeta` 与携带固定 `CommitTs` 的 `WriteBatch`，检查 response error 并取得 metas。
- `br/pkg/restore/snap_client/import.go::ingestSSTs` 同样以 Region、epoch、leader 构造 `kvrpcpb.Context`，将 metas 放入 `MultiIngestRequest` 并发给 leader store。

Rust 实现把这些协议步骤组合成可由 `Storage` 同步调用的完整路径，并额外实现 PD endpoint/leader 发现、逐 Region 切分、向全部 peers 写入、V2 keyspace、每 store 每 batch 限流、取消和有限重试。Go BR 对 Region 错误有更细分类处理；本文件目前把 TiKV Write/Ingest response 中的任何错误统一标为可重试，最多按固定次数重定位。

## 扩展指南

- 增加新的输入约束或统计字段：修改 `write_and_ingest_with_options` / `ImportStats`，同时同步 `pkg/kv/kv.rs::SSTImportStats`、`pkg/store/driver/kv_adapter.rs` 的映射，以及独立测试 `pkg/store/driver/sst_import_test.rs`。不要把测试内嵌回生产文件。
- 调整 Region 切分或键编码：优先修改 `region_key`、`KeyCodec` 选择或 `import_region` 的 `partition_point`，并增加空键、8 字节边界、V1/V2、Region end key 与 split 后重定位用例。这里最主要的兼容风险是把用户键、TiKV keyspace 编码键和 PD memcomparable Region key 混为一层。
- 调整批大小或限流：修改 batch stream 中的字节累计逻辑，保持 limiter 在每个 peer 的每个实际 WriteBatch 前调用。性能评估应覆盖大单 KV、跨 Region、大副本数与热更新 limiter；不要只按逻辑输入总量限一次流。
- 扩充重试策略：应在 `ImportError` 分类和外层 cursor 循环处实现，并针对 NotLeader、EpochNotMatch、ServerIsBusy、PD leader 切换和 ambiguous ingest 分别验证。要特别评估重新生成 UUID、相同 commit TSO 和远端已 ingest 状态的幂等语义。
- 改动 TLS/寻址：集中在 `channel`、`Connection::connect`、`Connection::store`，测试应覆盖 scheme、CA-only、mTLS、多个 PD endpoint 与 store 地址。同步 I/O 读取证书和每次重连重建 channel 的成本需要显式评估。
- 引入 Region 并行：当前 `cursor` 顺序语义简单；并行化需要重新设计统计合并、取消、每 store limiter 公平性、错误后未完成范围归属和内存上限，不能只把外层循环替换成并发集合。

## 验证依据

- 生产源码：`pkg/store/driver/sst_import.rs`，已通过 RustCodeGraph `node --file` 读取全部 453 行，并用 `query/callers/callees` 核对关键符号和内部调用边。
- crate 与模块边界：`pkg/store/driver/Cargo.toml`、`pkg/store/driver/lib.rs`。
- Rust 上游契约与调用者：`pkg/kv/kv.rs`、`pkg/store/driver/kv_adapter.rs`、`pkg/session/runtime/import_sst.rs`、`pkg/session/runtime/system_session.rs`。
- 独立 Rust 测试：`pkg/store/driver/sst_import_test.rs`。覆盖非法 TSO、乱序/重复键、空输入、memcomparable 边界、真实三副本 TiKV（ignored）、tonic wire 协议、V2 keyspace、逐 peer 限流、取消中止 ingest、以及每 batch 读取共享 limiter。
- Go 协议参照：`pkg/lightning/tikv/local_sst_writer_test.go`、`br/pkg/restore/snap_client/import.go`；仓库精确搜索确认 `pkg/store/driver` 无同路径 Go 实现。
- 未运行 Cargo 或代码测试：本任务仅新增说明文档，且总计划明确禁止运行 Cargo。验证范围是静态事实、调用结构与文档结构，不等同于重新证明远端 TiKV 行为。
