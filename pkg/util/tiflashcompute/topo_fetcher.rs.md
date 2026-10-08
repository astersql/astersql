# `pkg/util/tiflashcompute/topo_fetcher.rs`

## 文件定位

本文件属于 `astersql-util-tiflashcompute` crate。crate 入口 `pkg/util/tiflashcompute/lib.rs` 公开 `topo_fetcher` 模块并再导出其中的公开项；`pkg/util/tiflashcompute/Cargo.toml` 表明它直接依赖 `astersql-config`、`serde`/`serde_json`、`thiserror`、`ureq` 和 `url`，分别承担 AutoScaler 类型识别、AWS JSON 解码、错误类型、HTTP GET 和查询参数编码。

它实现 TiFlash Compute AutoScaler 拓扑获取边界：根据配置选择 Mock、AWS 或测试 fetcher，把具体实现隐藏在 `TopoFetcher` trait 后，并为 MPP 内存超限恢复提供重新拉取拓扑的入口。当前真实 Rust 上游是 `pkg/executor/internal/mpp/recovery_handler.rs` 的 `MemLimitHandlerImpl::doRecovery`；该调用通过全局 fetcher 触发 `RecoveryTypeMemLimit` 恢复。

需要特别区分实现与进程接线：`cmd/tidb-server/main.rs` 虽在 Disaggregated TiFlash 与 AutoScaler 同时启用时调用 `tiflashcompute::InitGlobalTopoFetcher`，但该名字当前来自 `cmd/tidb-server/stubs.rs`，只记录事件并返回成功，不会初始化本文件的 `globalTopoFetcher`。因此本文件具备真实实现和 executor 侧调用点，但 tidb-server 启动链尚未把两者接通。

## 核心职责

1. 用 `TopoFetcher` 统一普通拉取与错误恢复后拉取两种操作，并要求实现满足 `Send + Sync`。
2. 用 `InitGlobalTopoFetcher` 根据 `config::GetAutoScalerType` 构造具体 fetcher，并通过 `GetGlobalTopoFetcher` 发布进程级共享实例。
3. 为 Mock AutoScaler 请求 `/fetch_topo`，把分号分隔的文本解析为 CN 地址列表。
4. 为 AWS AutoScaler 区分固定池 `/sharedfixedpool` 与动态池 `/resume-and-get-topology`，在动态恢复时携带集群 ID、恢复类型和原 CN 数。
5. 用响应时间戳保护 AWS 缓存，避免较旧的并发响应覆盖较新的拓扑。
6. 提供 `TestTopoFetcher`，让测试获得确定的空拓扑，同时明确拒绝恢复操作。

文件只读取和缓存 AutoScaler 返回值，不负责验证 CN 地址格式、解释 AWS 响应中的 `HasError`/`ErrorInfo`/`State`，也不负责把新拓扑直接写入 MPP 调度器。

## 主要符号

- `TopoFetcherError(String)`：本文件的统一错误类型；`new` 为内部构造器，公开 API 以该类型返回可读错误字符串。
- `RecoveryType(u32)`：Go 风格恢复类别。`RecoveryTypeNull` 为普通拉取，`RecoveryTypeMemLimit` 为内存超限恢复；`toString` 只把这两个值映射成 AutoScaler 协议字符串，其他数值报错。
- `TopoFetcher`：公开 trait。`FetchAndGetTopo` 总是尝试获得当前拓扑，允许成功返回空列表；`RecoveryAndGetTopo` 先表达恢复意图再获得拓扑。
- `globalTopoFetcher: RwLock<Option<Arc<dyn TopoFetcher>>>`：进程级可替换共享实例。`InitGlobalTopoFetcher` 写入，`GetGlobalTopoFetcher` 克隆 `Arc` 后返回，不把锁守卫暴露给调用方。
- `MockTopoFetcher`：保存 `addr` 和受 `RwLock` 保护的 `Vec<String>`；`fetchTopo` 拉取并替换缓存，`getTopo` 返回副本。
- `AWSTopoFetcher` / `AWSTopoFetcherState`：不可变配置为 `addr`、`clusterID`、`isFixedPool`，可变状态为 `topo` 与 `topoTS`。`fetchAndGetTopo` 是统一内部入口，`fetchFixedPoolTopo` 和 `fetchTopo` 构造两类请求，`tryUpdateTopo` 执行时间戳条件更新。
- `resumeAndGetTopologyResult`：AWS JSON 响应映射。所有字段带默认值，并通过 `deserializeNullAsDefault` 把显式 `null` 处理为目标类型零值，以对齐 Go `encoding/json`。
- `httpGetAndParseResp`：共享 HTTP GET 与响应体读取层；`mockHTTPGetAndParseResp` 和 `awsHTTPGetAndParseResp` 在其上分别解析文本与 JSON。
- `TestTopoFetcher`：普通拉取固定返回空向量，恢复固定返回 `RecoveryAndGetTopo not implemented`。

## 执行流程

初始化流程从 `InitGlobalTopoFetcher(typ, addr, clusterID, isFixedPool)` 开始。函数先拒绝空集群 ID 或空 AutoScaler 地址，再调用 `config::GetAutoScalerType`：Mock 构造 `MockTopoFetcher`，AWS 构造 `AWSTopoFetcher`，Test 构造 `TestTopoFetcher`；GCP 明确返回“尚未实现”，未知类型会先把全局实例清空再报错。成功构造后，实例被包装成 `Arc<dyn TopoFetcher>` 写入全局锁。GCP 分支在返回错误前不改动已有全局实例，这是 `migration_aster_unit_test.rs::global_fetcher_initialization_matches_go_type_switch` 明确固定的行为。

Mock 普通拉取依次执行 `FetchAndGetTopo -> fetchTopo -> mockHTTPGetAndParseResp -> httpGetAndParseResp`。请求 URL 为 `http://{addr}/fetch_topo`；非空响应按 `;` 切分，随后整体替换缓存，最后 `getTopo` 克隆结果返回。Mock 的恢复入口没有网络行为，直接返回未实现错误。

AWS 普通拉取把 `(RecoveryTypeNull, 0)` 交给 `fetchAndGetTopo`，恢复拉取则透传调用参数。该入口先拒绝未知恢复类型，并拒绝 `MemLimit + oriCNCnt == 0`。固定池先读取缓存：非空立即返回，空缓存才请求 `/sharedfixedpool`；因此固定池成功得到非空拓扑后只请求一次。动态池每次请求 `/resume-and-get-topology`，始终添加 `tidbclusterid`，仅在内存超限恢复时再添加 `recovery=MemLimit` 与 `cn_cnt`。响应经 JSON 解码后进入 `tryUpdateTopo`，最后返回当前缓存副本。

`tryUpdateTopo` 先解析十进制 `Timestamp`，再做两阶段检查：读锁下若 `cachedTS >= newTS` 则丢弃响应；取得写锁后重新读取时间戳，若此时 `state.topoTS > newTS` 也丢弃，否则同时替换拓扑和时间戳。第二次判断允许相同时间戳在竞争窗口内再次写入，而第一次判断会拒绝已经缓存的相同时间戳；这是当前源码的精确行为。

## 数据与状态

全局状态是 `Option<Arc<dyn TopoFetcher>>`。未初始化或遇到未知类型初始化失败时为 `None`；调用方必须显式处理缺失。获取函数返回克隆的 `Arc`，所以后续重新初始化只替换全局槽位，不会使已经取得的旧实例失效。

Mock 缓存只存拓扑列表，没有版本信息。每次成功拉取会原子地替换整个向量；HTTP 或解析失败不会改变旧缓存，但公开 `FetchAndGetTopo` 会把错误返回给调用方，而不是退回旧值。

AWS 缓存把 `topo` 与 `topoTS` 放在同一 `RwLock<AWSTopoFetcherState>` 中，初始拓扑为空、时间戳为 `-1`。更新时二者在同一写锁临界区内同步变化。返回值总是克隆的快照。AWS 响应中的 `HasError`、`ErrorInfo` 和 `State` 被反序列化但当前没有参与控制流；是否接受响应只取决于 JSON 可解码、时间戳可解析及其与缓存时间戳的关系。

## 依赖与调用关系

上游直接证据如下：

- `pkg/executor/internal/mpp/recovery_handler.rs::MemLimitHandlerImpl::doRecovery` 调用 `GetGlobalTopoFetcher`，在实例存在时调用 `RecoveryAndGetTopo(RecoveryTypeMemLimit, info.NodeCnt)`，丢弃返回拓扑，只保留成功或错误。注释说明调度重建会另行取得拓扑。
- `pkg/executor/internal/mpp/recovery_handler_aster_unit_test.rs::mem_limit_handler_calls_the_real_global_tiflash_compute_fetcher` 初始化真实全局 Test fetcher，并证明 MPP 恢复路径抵达其未实现恢复错误。
- `pkg/util/tiflashcompute/lib.rs` 公开并再导出本模块。`pkg/lib.rs` 又通过 `facade_util_tiflashcompute` 提供门面；工作区根 `Cargo.toml` 将该门面依赖指向本 crate。
- `cmd/tidb-server/main.rs` 的启动条件与 Go 版相似，但当前调用的是 `cmd/tidb-server/stubs.rs::tiflashcompute::InitGlobalTopoFetcher`，不是本文件函数。

下游调用链是：`InitGlobalTopoFetcher -> config::GetAutoScalerType -> New*AutoScalerFetcher`；Mock/AWS 拉取共同进入 `httpGetAndParseResp -> ureq::get`；AWS JSON 经 `serde_json::from_slice` 解码，动态 URL 由 `url::Url` 构造。日志通过 `log` crate 输出。

RustCodeGraph 的文件节点确认 `topo_fetcher.rs` 含 35 个符号，并能定位 Rust/Go 同名接口；但本次 `callers`/综合 `explore` 查询在 10–30 秒内没有返回结果，因此上述跨文件调用边以仓库 `rg` 和源码读取为准，而不是声称来自未完成的图查询。

## 错误处理与边界

- 初始化要求 `clusterID` 与 `addr` 都非空，即使 Test 或 Mock 实现不一定使用 cluster ID；这是公开入口的一致前置条件。
- GCP 类型明确返回 `topo fetch not implemented yet(...)`，但不会清除先前的全局 fetcher；未知类型会清除。扩展调用方时不能把所有初始化错误都等价理解为“全局状态为空”。
- `ureq::get(...).call()` 会把传输错误以及非成功 HTTP 状态转换为错误；读取响应体失败也使用 `get tiflash_compute topology failed` 前缀。与 Go 版相比，Rust 错误包含底层 `ureq` 错误文本，而 Go 版将其统一映射为内部错误码消息。
- Mock 只拒绝长度为零的响应。诸如 `";"`、尾部分号或空白文本会形成空节点字符串，当前没有地址清洗或格式校验。
- AWS JSON 语法错误统一映射为 `httpGetFailedErrMsg`；缺失或 `null` 字段先取零值。空时间戳随后在 `tryUpdateTopo` 中变成 `parseTopoTSFailedErrMsg`。
- AWS 不检查 `HasError`、`ErrorInfo`、`State`，也不拒绝空 `Topology`。时间戳更新可以把非空缓存替换为空列表；固定池下下一次调用会因缓存为空再次请求。
- 恢复只支持 Null 与 MemLimit。MemLimit 的原 CN 数只排除零，不排除负数；负值会按十进制字符串发给 AutoScaler。
- 所有 `RwLock` 获取都用 `expect`；锁中毒会 panic，而不是转换成 `TopoFetcherError`。
- HTTP 客户端未在本文件配置显式超时、重试、认证或 TLS；URL 固定使用 `http`。这些运维边界若要改变，应先明确协议兼容要求。

## 并发与资源生命周期

`TopoFetcher: Send + Sync` 加上 `Arc<dyn TopoFetcher>` 允许 executor 线程共享实例。全局锁只保护实例槽位，`GetGlobalTopoFetcher` 在锁内克隆 `Arc` 后立即释放读锁；网络请求不会持有全局锁。重新初始化会原子替换槽位，旧实例由仍持有的 `Arc` 继续维持生命周期。

Mock 在网络读取完成后才取得写锁，因此慢请求不会阻塞缓存读取；最后完成的请求无条件覆盖缓存。AWS 同样不在网络期间持锁，并用时间戳阻止旧响应覆盖新响应。`tryUpdateTopo` 的读后再写模式在写锁内重新检查版本，覆盖了两个请求交错返回的主要竞争窗口。

HTTP 响应 reader 在 `read_to_end` 完成后随局部变量释放；本文件不创建后台线程、异步任务或 channel。缓存向调用方返回克隆值，因此调用方修改自己的 `Vec<String>` 不会影响共享状态，代价是每次读取都会复制全部地址字符串。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/tiflashcompute/topo_fetcher.go`。Rust 保留了 Go 的公开命名、恢复类型数值、Mock/AWS/Test 三种实现、两个 AWS 路径、查询参数名、初始时间戳 `-1`、固定池缓存策略以及时间戳双重检查。`deserializeNullAsDefault` 及结构体级 `#[serde(default)]` 专门弥合 serde 与 Go `encoding/json` 在缺失/`null` 字段上的默认值差异；对应测试位于 `migration_aster_unit_test.rs`。

已验证的差异包括：

- Go 注释声明全局变量“不线程安全”，Rust 用 `RwLock<Option<Arc<_>>>` 使读写槽位线程安全，并让返回实例拥有独立生命周期。
- Go 的 `getTopo` 返回底层 slice，Rust 返回克隆的 `Vec`，隔离调用方修改。
- Go `httpGetAndParseResp` 显式检查状态码；Rust 依赖 `ureq::call` 对非成功状态返回错误。Rust 测试用 HTTP 500 固定了外部错误语义。
- Go 使用 `dbterror` 内部错误类别并记录更丰富的响应/状态日志；Rust 使用字符串包装的 `TopoFetcherError`，没有 TiDB errno 分类，也没有打印完整 AWS 响应。
- Go 的 `InitGlobalTopoFetcher` 与生产 server 包直接接线；Rust server 启动目前仍调用 stub，真实初始化尚未接通。

Go 的相关使用证据包括 `pkg/store/copr/batch_coprocessor.go` 的普通拓扑拉取、`pkg/executor/internal/mpp/recovery_handler.go` 的内存超限恢复，以及各自测试；这些说明了原始设计意图，但不能证明 Rust 已拥有相同的普通调度接线。仓库搜索未发现 Rust 生产代码调用本文件的 `FetchAndGetTopo`；当前 Rust 生产调用只确认到 MPP recovery handler 的恢复入口。

## 扩展指南

新增 AutoScaler 类型时，应同时扩展 `config::GetAutoScalerType` 的类型集合、`InitGlobalTopoFetcher` 的 match 分支、具体 `TopoFetcher` 实现及 `migration_aster_unit_test.rs` 中的独立测试；若是 GCP 实现，还需替换当前明确的未实现分支，并决定失败时是否保留旧全局实例。不要把测试写回本源文件，现有测试装配点是 `pkg/util/tiflashcompute/lib.rs` 中独立的 `migration_aster_unit_test.rs`。

调整 AWS 协议时，最可能涉及 `resumeAndGetTopologyResult`、`fetchTopo`、`fetchFixedPoolTopo`、`awsHTTPGetAndParseResp` 和 `tryUpdateTopo`。必须保持查询参数 URL 编码、缺失/`null` JSON 字段兼容、时间戳单调更新和固定池缓存语义，并补充乱序并发响应、相同时间戳、空拓扑、负 CN 数或服务端业务错误字段等边界测试。

若要让完整 Rust 服务真正使用该实现，接线点在 `cmd/tidb-server/main.rs` 与 `cmd/tidb-server/stubs.rs` 的模块导入边界；需要把启动调用改接本 crate，并验证与 executor 链路共享同一个静态全局实例。该工作超出本文档任务范围，不能仅删除 stub 就视为完成，还应覆盖启动配置、初始化失败和 MPP 恢复的集成行为。

性能扩展时应关注两个复制点：`getTopo` 克隆整个列表，以及 `tryUpdateTopo` 克隆响应中的 `Topology`。若改用共享不可变快照，必须保持调用方无法原地修改缓存的约束。增加超时、重试或 HTTPS 时，应在共享 HTTP 层实施并为 Mock/AWS 两条路径各自验证错误映射。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/tiflashcompute` 确认目标 Rust/Go 文件与测试文件；`node --file pkg/util/tiflashcompute/topo_fetcher.rs --offset 1 --limit 500` 返回目标文件 437 行及全部 35 个符号；`query TopoFetcher` 返回 Rust trait、Go interface 与相关函数候选。`callers InitGlobalTopoFetcher` 和综合 `explore` 在限定时间内未返回，调用边改由下列源码搜索验证。
- 目标实现：`pkg/util/tiflashcompute/topo_fetcher.rs`，重点符号为 `InitGlobalTopoFetcher`、`GetGlobalTopoFetcher`、三个 `TopoFetcher` 实现、`AWSTopoFetcher::fetchAndGetTopo` 与 `tryUpdateTopo`。
- crate 与模块边界：`pkg/util/tiflashcompute/Cargo.toml`、`pkg/util/tiflashcompute/lib.rs`、工作区根 `Cargo.toml`、`pkg/lib.rs`。
- Rust 上游：`pkg/executor/internal/mpp/recovery_handler.rs`、`pkg/executor/internal/mpp/recovery_handler_aster_unit_test.rs`；启动接线限制由 `cmd/tidb-server/main.rs` 和 `cmd/tidb-server/stubs.rs` 共同确认。
- Go 对照：`pkg/util/tiflashcompute/topo_fetcher.go`；Go 普通拉取与恢复调用参考 `pkg/store/copr/batch_coprocessor.go` 和 `pkg/executor/internal/mpp/recovery_handler.go`。
- 独立 Rust 测试：`pkg/util/tiflashcompute/migration_aster_unit_test.rs`，覆盖恢复类型、Test/Mock/AWS 实现、HTTP 路径与错误、查询编码、初始化分支、固定池缓存、JSON 默认值及时间戳解析。
- 本任务为纯文档分析，按总计划不运行 Cargo。结构验收使用任务文件指定命令，检查目标文件存在且固定二级标题恰好为 11 个；人工复核同时确认没有把 stub 启动路径描述为真实接线，也没有把 Go 调用链描述成 Rust 已支持行为。
