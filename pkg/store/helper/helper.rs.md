# `pkg/store/helper/helper.rs`

源文件：[helper.rs](./helper.rs)；Go 对照：[helper.go](./helper.go)；crate 入口：[lib.rs](./lib.rs)。本文只描述当前仓库中的真实实现，不把 `Storage` 默认方法所保留的 Go 接口形状当作已经接入真实 TiKV 客户端的能力。

## 文件定位

本文件是 `astersql-store-helper` crate 的主体，由 [Cargo.toml](./Cargo.toml) 声明为以 `lib.rs` 为入口的独立库，并由 `lib.rs` 的 `pub mod helper; pub use helper::*;` 对外再导出。它位于 TiDB 上层诊断/管理逻辑与 TiKV、PD 状态接口之间，集中承担四类工作：MVCC 查询与锁解析、PD Region/热点查询、Region 键范围到库表/索引元数据的映射，以及 TiFlash/TiKV 内部 HTTP 状态采集。

当前生产接线并不覆盖文件中的全部公开 API。`pkg/domain/infosync/info.rs::CalculateColumnarIndexProgress` 调用 `CollectColumnarStatus` 汇总列存索引进度；`pkg/session/runtime/normal_ddl_service.rs` 调用 `CollectStorageClassStatus` 汇总 storage-class transition 进度。`pkg/lib.rs` 还通过 `store::helper` facade 再导出本 crate。RustCodeGraph 将目标文件识别为 191 个符号，并显示它被 `pkg/session/runtime/normal_ddl_service.rs`、`pkg/store/mockstore/mockstorage/canonical_storage.rs`、若干运行时测试及本 crate 测试引用；未发现生产侧对 MVCC/热点映射入口的直接调用，因此这些入口应理解为已实现且有测试的公共移植面，而不是已经贯穿全部应用主链。

## 核心职责

1. 以 `Storage`、`RegionCache`、`Oracle`、`LockResolver` 和 `PdClient` 等 object-safe trait 隔离具体客户端，让核心算法可由生产适配器或确定性测试替身驱动。
2. `Helper` 缓存 `Store`、`RegionCache` 和惰性初始化的 PD HTTP 客户端，提供 Region 全量查询、按键或事务时间戳的 MVCC 查询、热点 Region 查询及表/索引归属解析。
3. 将 TiDB tablecodec 键空间解释为 `FrameItem`、`RegionFrameRange`、`TableInfoWithKeyRange`，并按半开区间 `[start, end)` 计算 Region 与表、分区及索引的交集。
4. 通过 TiFlash/TiKV status HTTP API 读取副本同步、列存索引和存储级别迁移状态，同时保留 Go 上下文的取消与截止时间语义。
5. 保持 `helper.go` 的外部行为和边界选择，包括 keyspace v2 前缀、MVCC 锁重试、解码容错、TiFlash 声明数量不一致仅忽略、以及 Columnar 可选字段的“是否出现”信息。

## 主要符号

- `RequestContext`：用共享 `Arc<AtomicBool>` 表示取消，用 `Option<Instant>` 表示截止时间。`check` 产生 `context canceled` 或 `context deadline exceeded`；`request_timeout` 为 HTTP 请求计算剩余时间，未设置截止时间时默认 30 秒，最短 1 毫秒。
- `Codec(ApiVersion)`：`v1` 原样保留用户键；`v2` 验证 keyspace ID 不超过 uint24，然后添加 `x + 3 字节 keyspace ID` 前缀。`EncodeRegionRange` 再调用 `tablecodec::codec::EncodeBytes` 生成 Region 查询边界。
- `Storage`：复刻 Go `Storage` 的宽接口。只有 `GetRegionCache`、`GetCodec` 等少数方法有可用默认值；大多数默认方法经 `unsupported` 返回“未实现”。零尺寸 `Transaction`、`Snapshot`、`KvClient` 等类型只是接口占位，不包含真实客户端行为。
- `Helper` / `NewHelper` / `TryGetPDHTTPClient`：`NewHelper` 固定从 store 捕获 Region cache；PD 客户端第一次使用时取自 store、附加 caller ID `tidb-store-helper` 并缓存在 `pdHTTPCli`。
- `GetMvccByEncodedKeyWithTS`、`GetMvccByEncodedKey`、`GetMvccByStartTs`：分别实现按编码键查询、默认时间戳查询和按事务 `start_ts` 跨 Region 反查。
- `FetchHotRegion`、`ScrapeHotInfo`、`FetchRegionTableIndex`、`FindTableIndexOfRegion`：从 PD 拉取读/写热点，按 Region ID 定位边界，再映射到库表或索引。
- `NewFrameItemFromRegionKey`、`NewRegionFrameRange`、`RegionFrameRange::{GetRecordFrame, GetIndexFrame}`：解码 Region 边界并判断记录/索引键空间覆盖关系。
- `GetTablesInfoWithKeyRange`、`newTableInfoWithKeyRange`、`ParseRegionsTableInfos`：构造普通表、物理分区、局部/全局索引的编码范围，排序后使用双层推进算法求 Region 交集。
- `GetPDRegionStats`：按整表前缀或记录前缀构造范围，经当前 store codec 编码后查询 PD Region 状态。
- `ComputeTiFlashStatus`、`CollectTiFlashStatusWithCtx`、`SyncTableSchemaToTiFlash`：解析 TiFlash 两行文本协议、查询副本同步状态及触发表 schema 同步。
- `CollectStorageClassStatusWithCtx` / `StorageClassStatusResp`：查询单个物理表的存储级别迁移状态，并验证 HTTP 200、JSON 字段完整以及 `Ready <= Total`。
- `CollectColumnarStatusWithCtx` / `ColumnarStatusResp`：查询列存状态；自定义 `Deserialize` 区分缺失的 `fts-index-ready` 与显式的零值。

## 执行流程

MVCC 按键查询从 `GetMvccByEncodedKeyWithTS` 开始：创建累计上限 5000 毫秒的 `Backoffer`，通过 `RegionCache::LocateKey` 定位 Region，调用 `Storage::SendReq` 发送一分钟超时的 `MvccGetByKey`。Region error 会触发退避并重新定位；响应类型不匹配、业务错误字符串非空或 `Info` 缺失都会立即返回错误。若调用方给出非零 `start_ts` 且响应含锁，算法先从 `Oracle` 获取最新低精度时间戳，拒绝未来快照，再把 `LockInfo` 转成 `Lock`，以 `Lite=true`、`ForRead=false` 调用 `ResolveLocksWithOpts`；返回 TTL 大于零时继续退避，随后重新查询，直到得到无须解析的响应。

`GetMvccByStartTs` 同样逐 Region 查询，但 RPC 超时为一小时且请求标为低优先级。命中键时返回大写十六进制键、Region ID 和 MVCC 信息；未命中时，若结束键已落入当前 Region 或当前 Region 没有结束边界则返回 `None`，否则把 `start_key` 推进到当前 Region 的 `EndKey`。

热点流程由 `ScrapeHotInfo` 串联：`FetchHotRegion` 根据 `HotRead`/`HotWrite` 选择 PD API，把 leader peer 的 byte rate 和 hot degree 按 Region ID 聚合；`FetchRegionTableIndex` 用 500 毫秒退避预算按 ID 定位 Region，构造 `RegionFrameRange`，再遍历 schema 的库表、物理分区、记录范围和索引范围。无法定位某个 Region 时该项被跳过；能定位但无法映射时仍返回仅含 Region 指标的记录。

Region/表映射先由 `GetTablesInfoWithKeyRange` 枚举 schema。非分区表产生一条记录范围；分区表为每个物理分区产生记录范围。全局索引只产生逻辑表级范围，局部索引则按每个分区产生范围。所有范围经 `Codec::EncodeRegionRange` 编码并按起始键排序。`ParseRegionsTableInfos` 同样排序 Region，以 `isBehind` 跳过已经结束的表范围，再以 `isIntersecting` 收集半开区间交集，因此相邻边界不会重复归属。

HTTP 状态采集统一经 `status_url` 处理显式 scheme，缺省采用 `TIDB_INTERNAL_HTTP_SCHEMA` 或 `http`。TiFlash 同步状态使用 blocking client 并把响应交给 `ComputeTiFlashStatus`；storage-class 状态也使用 blocking client并检查状态码与计数不变量；Columnar 状态则创建单线程 Tokio runtime，让异步请求与每 5 毫秒检查一次的取消分支竞争，从而能在等待响应期间响应取消。

## 数据与状态

`Helper` 持有三个可选引用：`Store` 与 `RegionCache` 在正常 `NewHelper` 路径中同时存在，`pdHTTPCli` 则按需写入。`Default` 会创建三个 `None`，主要供无 store 的纯键范围算法和测试使用；调用需要 store/cache 的方法会明确报错。

键范围存在三种表示：原始 `Vec<u8>` 用于 TiKV/Region cache，tablecodec 的 memcomparable 编码用于 PD Region 边界，`bytesKeyToHex` 生成的大写十六进制字符串用于 `RegionInfo` 与排序/交集。空结束键表示正无穷；`KeyLocation::Contains` 和 `WithKeyRange` 辅助函数都采用半开区间语义。

`FrameItem` 同时承载表 ID、索引 ID、记录 handle 或 common handle/index values。`NewFrameItemFromRegionKey` 对表前缀之前/之后的非表键分别映射为 `i64::MIN`/`i64::MAX` 哨兵；裸 `t{id}` split key 只填表 ID。代码有意保留 Go 中“高于 table prefix 时重复设置 TableID、IndexID 保持默认零”的行为。

`RegionMetric` 的 `FlowBytes` 从 PD `f64 ByteRate` 直接转换为 `u64`，重复 Region ID 以后写入者覆盖先前值。`ComputeTiFlashStatus` 则对同一 Region 使用 `entry(...).or_insert(0)` 累加副本数。

`ColumnarStatusResp::HasFtsIndexReady` 不参与序列化，只记录 JSON 是否真的包含 `fts-index-ready`；这使上层能区分旧 TiKV 不支持该字段和已支持但当前计数为零。`StorageClassStatusResp` 的 `Ready`、`Total` 没有 serde 默认值，字段缺失会解码失败。

## 依赖与调用关系

crate 直接依赖 `anyhow`（统一错误与上下文）、`hex`、`serde`/`serde_json`、带 `blocking/json/rustls-tls` feature 的 `reqwest`、`tokio` 的 `macros/rt/time`，以及本仓库的 `astersql-meta-metadef` 和 `astersql-tablecodec`。`metadef::IsMemDB` 用于 `FilterMemDBs`，`tablecodec` 提供键前缀、行/索引解码、key range 和 memcomparable codec。

RustCodeGraph 核对的关键下游边包括：`GetMvccByEncodedKeyWithTS -> LocateKey / SendReq / GetLowResolutionTimestamp / ResolveLocksWithOpts / Backoff`；`ScrapeHotInfo -> FetchHotRegion -> FetchRegionTableIndex`；`ParseRegionsTableInfos -> isBehind / isIntersecting`；`CollectTiFlashStatusWithCtx -> status_url / http_client / ComputeTiFlashStatus`；`CollectColumnarStatusWithCtx -> RequestContext::{check, request_timeout} / status_url`。

已确认的直接上游是：

- `pkg/domain/infosync/info.rs::CalculateColumnarIndexProgress -> CollectColumnarStatus`，对正常 store 的错误上抛、对 Tombstone store 跳过，并按 vector/FTS 类型选择 ready 计数。
- `pkg/session/runtime/normal_ddl_service.rs -> CollectStorageClassStatus`，逐物理表和 store 汇总迁移进度。
- `pkg/lib.rs::store::helper` 与本 crate `lib.rs` 提供 facade/通配再导出。
- `pkg/store/mockstore/mockstorage/canonical_storage.rs` 为 `Storage` trait 提供 mock storage 适配；服务器 handler 测试直接使用 Region frame API。

## 错误处理与边界

公共 fallible API 统一返回 `anyhow::Result`，并在构建客户端、网络请求和 JSON 解码处增加上下文。`Storage` 的默认实现不能冒充真实客户端：未支持的方法返回 `"<method> is not implemented by this storage"`；`GetPDHTTPClient` 缺失、`Store`/`RegionCache` 为 `None` 也分别报明确错误。

`Backoffer` 以饱和加法累计请求的睡眠毫秒数，超过预算时失败；真实睡眠被限制到每次最多 10 毫秒，因此这里保留的是重试预算/测试语义，不是 Go TiKV backoff 的完整时间策略。Region error 与锁 TTL 都会消耗同一个 MVCC 预算。

`FetchHotRegion` 对除 `read`/`write` 外的值直接报错；这比 Go 的 switch 无 default 更显式，避免未初始化响应被后续访问。`FetchRegionTableIndex` 对单个 Region 定位失败采取 best-effort 跳过，`FindTableIndexOfRegion` 对单个 schema 表枚举错误也继续扫描；相比之下，Region 边界解码失败会终止当前映射请求。

`NewFrameItemFromRegionKey` 在头部已成功解码后有意忽略记录/索引 payload 的普通解码失败，与 Go 一致；但 common handle 的 `Data`/`ToString` 错误仍会传播。TiFlash 文本的 EOF、计数或 Region ID 解析错误会失败，声明数与实际数不一致只被计算为内部布尔值而不报错，保持 Go 行为。

storage-class 与 Columnar API 明确拒绝非 200 响应并附带响应体。storage-class 另拒绝 `Ready > Total`。`CollectTiFlashStatusWithCtx` 当前没有显式检查 HTTP 状态码，而是直接按两行协议解析响应体；扩展时不能假设三类 endpoint 的状态码处理完全一致。

## 并发与资源生命周期

所有客户端 trait 都要求 `Send + Sync`，并由 `Arc<dyn ...>` 共享。`RequestContext::cancel` 使用 `Release` 写，`check` 使用 `Acquire` 读；克隆 context 会共享同一取消标志，但 deadline 是复制的 `Instant` 值。`Helper::TryGetPDHTTPClient` 需要 `&mut self`，缓存字段本身没有锁，因此单个 `Helper` 不支持多个任务在无外部同步时并发惰性初始化；可以共享底层 `Arc` 客户端，或让调用方用互斥保护 Helper。

blocking HTTP 函数依赖 reqwest response/`BufReader` 的作用域析构关闭连接，没有显式后台任务。`CollectColumnarStatusWithCtx` 每次调用创建一个 current-thread Tokio runtime，在 `block_on` 内同时驱动请求和取消轮询，函数返回时 runtime、client 和未胜出的 future 一并销毁。它没有复用 runtime，频繁轮询时存在构建 runtime 的固定成本。

`Backoffer` 是每个请求独占的可变状态，不跨调用共享。`FetchRegionTableIndex` 为每个 Region 创建独立 backoffer；MVCC 流程则在整个循环中复用一个 backoffer，使 Region miss、锁等待和后续重试共享总预算。`ComputeTiFlashStatus` 原地更新调用方传入的 `HashMap`，在中途解析失败前已经完成的累加不会回滚。

## 与 Go 版本的对应关系

Rust 文件明确以同目录 `helper.go` 为移植基线，公开命名也保留 Go 风格。核心路径逐项对应：`Storage`/`Helper` 结构、PD caller ID、keyspace 范围编码、MVCC 锁转换和 `Lite=true, ForRead=false`、跨 Region start-ts 扫描、热点表映射、Region frame 哨兵、分区与全局索引规则、半开区间求交、TiFlash/Columnar/storage-class endpoint 均有同名 Go 实现。

Rust 为脱离 Go 客户端类型而新增了 transport trait、响应 enum 和占位句柄；因此 `Storage` 的方法签名并非 ABI 级复刻，而是算法可测试的 Rust 边界。Go `Storage::Closed` 返回 channel，Rust 当前只返回 `bool`；Go 的真实 `Transaction`、`Snapshot`、KV/MPP client 在 Rust 中仍是零尺寸占位。这些差异意味着不能仅凭 trait 方法存在就宣称完整存储接入。

Rust `FetchHotRegion` 对未知类型返回错误，而 Go 当前没有 default 分支。Rust `FetchRegionTableIndex` 的 `_filter` 参数和 Go 的 `filter` 参数一样未在该方法内使用；真正的 schema filter 只由 `GetTablesInfoWithKeyRange` 应用。Rust 的 `GetRecordFrame` 在 common handle 分支同步更新边界缓存中的 `IndexName`，以保持随后读取的一致性。

`helper_1_aster_unit_test.rs` 是额外的 Rust 对齐测试，覆盖 Go 测试未直接表达的移植契约；`helper_test.rs` 则对应同目录 Go 测试的主要行为面。测试逻辑与生产源分文件存放，符合仓库约束。

## 扩展指南

- 接入真实 TiKV/PD 能力时，优先为现有 `Storage`、`RegionCache`、`LockResolver`、`Oracle` 或 `PdClient` 提供适配器，不要把客户端细节塞回 MVCC/映射算法。新增必需方法时应同时审查 `pkg/store/mockstore/mockstorage/canonical_storage.rs` 及本目录测试替身。
- 修改 MVCC 重试时，应保持 Region miss 重新定位、未来快照拒绝、锁字段完整转换、`Lite=true`/`ForRead=false` 和共享总退避预算；在独立 `helper_1_aster_unit_test.rs` 添加锁、TTL、响应类型和边界 Region 回归。
- 修改键范围或 tablecodec 逻辑时，必须同时覆盖 v1/v2 keyspace、空边界、split key、普通/分区表、局部/全局索引、common handle 以及相邻半开区间；对应测试位置是 `helper_test.rs` 和 `helper_1_aster_unit_test.rs`，不可把测试内嵌回生产文件。
- 新增状态 endpoint 时，可复用 `status_url` 与 `RequestContext`，但需明确是否要求非 200 检查、blocking 还是 async、字段缺失语义及计数不变量。若是高频调用，应评估复用 HTTP client/runtime，避免沿用每次构造 runtime 的成本。
- 扩展 `ColumnarStatusResp` 的可选字段时，应像 `HasFtsIndexReady` 一样区分“缺失”和“零值”，避免上层把版本不兼容误判为进度为零。
- 改动 `TryGetPDHTTPClient` 缓存或让 Helper 跨线程共享时，需先定义同步策略；当前 `&mut self` 已保证单次访问互斥，但没有内部锁。
- 保持 Go 同路径行为同步；若刻意修正 Go 中的兼容怪异点（例如 frame 高位哨兵的重复 TableID 赋值），应先建立 Go/Rust 双侧回归，而不是单边“清理”。

## 验证依据

事实来源如下：

- RustCodeGraph：`status` 确认索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/store/helper` 确认 `helper.rs`、两份 Rust 测试、Go 源与 Go 测试均已索引；`node --file pkg/store/helper/helper.rs` 分段读取全部 1,836 行；对 `NewHelper`、MVCC、热点、Region 映射和三类 HTTP 状态函数执行了 `query`、`callers`、`callees`。`callers` 对这些 Rust 方法未给出可靠输出，因此上游入口另以精确引用搜索核对，未据此臆造调用关系。
- 源与边界：`pkg/store/helper/helper.rs`、`pkg/store/helper/lib.rs`、`pkg/store/helper/Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/store/helper/helper.go`；Go 测试：`pkg/store/helper/helper_test.go`。
- Rust 独立测试：`pkg/store/helper/helper_test.rs` 覆盖 Region/表映射、keyspace、热点、PD stats、TiFlash、取消和范围判断；`pkg/store/helper/helper_1_aster_unit_test.rs` 覆盖 MVCC 锁解析、Go 对齐的分区/全局索引、frame 解码、HTTP 路径、可选 FTS 字段、错误状态及 storage-class 不变量。
- 生产调用证据：`pkg/domain/infosync/info.rs`、`pkg/session/runtime/normal_ddl_service.rs`；适配/消费证据：`pkg/store/mockstore/mockstorage/canonical_storage.rs`、`pkg/server/handler/tests/http_handler_test.rs`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前运行任务指定的结构命令，要求目标文档存在且恰好包含上述 11 个固定二级标题；并人工复核本文能够回答文件为何存在、主要流程、当前接线状态、失败边界与安全扩展位置。
