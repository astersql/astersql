# `pkg/lightning/tikv/tikv.rs`

## 文件定位

本文件属于 `astersql-lightning-tikv` crate 的集群管控部分。`pkg/lightning/tikv/lib.rs` 以 `mod tikv` 装入本文件并通过 `pub use tikv::*` 公开其 API；同一 crate 还包含本地 SST 写入和属性收集，但本文件不参与 SST 编码。

它位于 Lightning 与 PD/TiKV 远端能力之间：定义可注入的 PD、TiKV 和 schema 客户端接口，并实现 Store 遍历、模式切换、压缩、模式探测和版本兼容检查。当前 Rust 文件本身不创建真实 gRPC/TLS/HTTP 客户端；网络连接、超时、认证和连接释放由 `PdClient`、`TiKvConnector`、`TiKvClient`、`RemoteSchema` 的实现者负责。因此它是可复用的业务编排与协议抽象层，不等同于 Go 文件中的完整网络实现。

crate 边界由 `pkg/lightning/tikv/Cargo.toml` 确认：库入口是 `lib.rs`，直接依赖仅为 `regex`、`semver`、`thiserror`，移植元数据指向 Go 包 `pkg/lightning/tikv`。

## 核心职责

1. 用 `PdClient`、`TiKvConnector`/`TiKvClient` 和 `RemoteSchema` 隔离 PD 查询、TiKV RPC 与远端 schema 获取。
2. `ForAllStores` 按 `StoreState` 上界过滤节点，并以 scoped thread 并行执行回调。
3. `SwitchModeOnStore`、`SwitchMode`、`Compact` 和 `FetchMode` 通过一次性 TiKV 连接执行节点操作。
4. `FetchModeFromMetrics` 从 Prometheus 文本中的 RocksDB 配置项推断 Normal/Import 模式。
5. `CheckPDVersion`、`ForTiKVVersions`、`CheckTiKVVersion` 解析语义化版本并执行半开区间兼容检查。
6. 将兼容旧 TiKV 的 `Unimplemented` 特判集中在 `ignoreUnimplementedError`，避免模式切换或压缩因缺少 RPC 直接失败。

## 主要符号

- `TikvError`：统一错误枚举。`Io` 接收 `std::io::Error`；`InvalidArgument` 当前未在本文件构造；`InvalidData` 用于缺少模式指标；`Unimplemented` 是可忽略的远端能力缺失；`Remote` 保留其他远端业务错误；`Version` 表示解析或区间错误。
- `StoreState::{Up, Offline, Tombstone}` 与 `Store { id, address, version, state }`：本地 PD Store 投影。派生的顺序决定 `ForAllStores` 的 `state <= max_state` 过滤语义。
- `PdClient`：提供 `GetStores` 和 `GetPDVersion`，是节点枚举与版本检查的唯一 PD 边界。
- `SwitchMode::{Normal, Import}`、`KeyRange { start, end }`：模式及半开键区间 `[start, end)`。
- `TiKvClient`：单节点能力集合，包含 `SwitchMode`、`Compact`、`FetchMetrics`；trait 需要 `Send`，以允许客户端在并发边界中安全拥有。
- `TiKvConnector`：以地址构造 `Box<dyn TiKvClient>`；`Send + Sync` 允许共享连接器。
- `withTiKVConnection<T>`：连接一次、执行一个 `FnOnce` 并透传结果。客户端随函数退出而 drop，但是否对应真实网络连接以及 drop 的关闭语义由具体 connector/client 实现决定。
- `ForAllStores`：取得 Store 快照、过滤状态、为每个入选 Store 生成 scoped thread，等待全部线程退出，并返回互斥槽中记录的一个错误。
- `ignoreUnimplementedError`：只把 `TikvError::Unimplemented` 转为成功，其余结果原样返回。
- `SwitchModeOnStore` / `SwitchMode`：前者执行连接和兼容性特判，后者只是同签名别名。
- `Compact`：向单节点下发 level 和 resource group，并忽略未实现错误。
- `FetchMode` / `FetchModeFromMetrics` / `FETCH_MODE_RE`：获取指标并匹配 `tikv_config_rocksdb{cf="default",name="hard_pending_compaction_bytes_limit"}`；捕获值严格等于字符串 `"0"` 才判为 Import。
- `DBInfo`、`TableInfo`、`RemoteSchema`、`FetchRemoteDBModelsFromTLS`、`FetchRemoteTableModelsFromTLS`：远端 schema 的最小模型和委托入口。函数名保留 Go 的 TLS 语义，但 Rust 函数只调用 trait，不直接处理 TLS 或 HTTP 路径。
- `check_version`：私有区间检查器；下界比较完整 semver，上界按 `found.major >= required_max.major` 拒绝。
- `CheckPDVersion`：去掉至多一个前导 `v`、解析 PD 版本，再调用 `check_version`。
- `ForTiKVVersions`：遍历至 `Offline` 状态，跳过 Tombstone，解析版本并把 `TiKV (at <address>)` 与版本交给回调。
- `CheckTiKVVersion`：以 `ForTiKVVersions` 并行检查所有活跃 TiKV 节点。

## 执行流程

节点批处理主链如下：调用者进入 `ForAllStores` → `PdClient::GetStores` 返回快照 → 仅保留 `state <= max_state` → 每个 Store 在一个 scoped thread 中调用共享 `action` → 回调错误争用 `Arc<Mutex<Option<TikvError>>>`，只有第一个取得空槽的错误被保存 → scope 等待全部任务结束 → 返回保存的错误或 `Ok(())`。这里的“首个错误”是并发观察顺序，不保证等于 Store 列表顺序。

单节点操作链为 `SwitchMode` → `SwitchModeOnStore` → `withTiKVConnection` → `TiKvConnector::Connect` → `TiKvClient::SwitchMode` → `ignoreUnimplementedError`。`Compact` 走相同连接链后调用 `TiKvClient::Compact`。`FetchMode` 连接后调用 `FetchMetrics`，再由 `FetchModeFromMetrics` 解析；它不应用 `ignoreUnimplementedError`，所以连接、指标 RPC 和解析错误均向上传播。

模式解析先由惰性全局 `FETCH_MODE_RE` 找到目标指标行，再取得第一个捕获组。找不到指标时返回 `InvalidData("import mode status is not exposed")`；捕获字符串恰为 `0` 时返回 Import，任何其他字符串（包括 `0.0` 和非数字文本）均返回 Normal。

版本链有两条：`CheckPDVersion` 直接读取并解析一个 PD 版本；`CheckTiKVVersion` → `ForTiKVVersions` → `ForAllStores(..., Offline, ...)` 并行处理全部非 Tombstone Store。两条链都只剥离一个前导 `v`，然后调用 `check_version`。低于最小版本时报 too old；major 达到最大版本 major 时（包括该 major 的 beta）时报 too new。

远端 schema 两个入口没有额外编排：数据库入口直接委托 `RemoteSchema::FetchDatabases`；表入口携带原始 `schema` 字符串委托 `FetchTables`。

## 数据与状态

本文件没有持久化状态。`Store`、`KeyRange`、`DBInfo`、`TableInfo` 都是按值传递的数据模型；`SwitchMode` 和 `StoreState` 是可复制枚举。

唯一的进程级共享对象是 `FETCH_MODE_RE: LazyLock<Regex>`：首次解析指标时编译固定正则，之后只读复用。正则字面量固定且通过 `unwrap` 构造；由于表达式随源码发布，失败代表开发期常量错误而不是运行时输入错误。

`ForAllStores` 在调用开始时物化经过过滤的 `Vec<Store>`，后续并发工作不再观察 PD 列表变化。共享错误槽是本次调用局部的 `Arc<Mutex<Option<TikvError>>>`，scope 结束后取走并销毁。回调自己的共享状态、幂等性与副作用同步由调用者负责。

版本对象使用 `semver::Version`；错误消息包含组件、要求区间与实际版本。上界实现依赖 `required_max.major`，所以接口虽接收完整上界版本，实际约束假定上界是下一个不兼容 major。

## 依赖与调用关系

向下依赖方面，`withTiKVConnection` 调用 `TiKvConnector::Connect`，节点操作再调用 `TiKvClient` 的三个方法；`ForAllStores` 调用 `PdClient::GetStores`；`CheckPDVersion` 调用 `PdClient::GetPDVersion`；schema 入口调用 `RemoteSchema`。纯库依赖分别用于：`regex` 解析指标、`semver` 解析和比较版本、`thiserror` 派生错误展示与来源转换。

RustCodeGraph 对本文件给出的内部关键边为 `CheckTiKVVersion` → `ForTiKVVersions` → `ForAllStores`，以及 `SwitchModeOnStore`/`Compact`/`FetchMode` → `withTiKVConnection`。索引还显示 `ForAllStores` 被 `lightning/pkg/server/lightning.rs` 的 `SwitchMode` 使用；本文件自身 API 由 `lib.rs` 全量再导出，另有 `pkg/lightning/tikv/tikv_test.rs` 直接覆盖 Store 遍历、指标解析与版本检查。部分精确 callers/callees CLI 查询受同工作区并发索引查询阻塞，本文没有据此虚构更多生产调用者。

Go 侧对应调用链来自 `pkg/lightning/tikv/tikv.go`：`CheckTiKVVersion` → `ForTiKVVersions` → `ForAllStores`，并由真实 PD HTTP、gRPC ImportSST/Debug、TLS 和模型类型承接边界。Rust crate 当前只声明三项轻量依赖，进一步证明真实传输层不在此文件内实现。

## 错误处理与边界

- 所有公开操作使用 `Result<_, TikvError>`，`?` 保留连接、PD、远端 schema、指标和版本解析错误。
- `ignoreUnimplementedError` 的容错范围严格限定为 `TikvError::Unimplemented`；`Remote`、`Io` 等不会被吞掉。Rust 层没有自行把 gRPC status 映射为该变体，映射责任在适配器。
- `ForAllStores` 在 `GetStores` 失败时不会启动任何工作；回调失败后不会取消其余工作，所有已筛选 Store 都会执行并等待完成。`tikv_test.rs::ForAllStoresPropagatesActionError` 明确断言两个回调均被调用。
- `Mutex::lock().unwrap()` 假设回调线程不会在持锁区间 panic。当前持锁区间只含检查和赋值；若 mutex 被毒化，本函数会 panic，而不是返回 `TikvError`。线程回调自身 panic 也会在 scoped thread 汇合时传播 panic。
- `FetchModeFromMetrics` 只验证指标是否存在，不验证捕获值是否为数值；除精确 `0` 外都归为 Normal。这与 Go switch 语义一致，也是新增解析增强时必须谨慎维护的兼容边界。
- 版本字符串仅移除一个小写 `v`。非法 semver、重复前缀（如 `vv8.5.1`）返回 `Version` 错误；TiKV 解析错误附带节点地址。
- `check_version` 的上界不是普通完整 semver `< required_max`，而是 major 防线；`9.0.0-beta` 在最大版本 `9.0.0` 时也被拒绝。
- `FetchRemote*FromTLS` 当前不增加 Go 侧的 `cannot read ... from remote` 上下文，schema 名也未编码或校验；这些应由 `RemoteSchema` 实现处理或在未来有意补齐。

## 并发与资源生命周期

`ForAllStores` 使用 `std::thread::scope`，因此回调可借用当前栈上的只读状态，函数返回前保证所有子线程已结束，不产生后台任务。每个筛选后的 Store 对应一个 OS 线程，没有并发度上限；Store 数量很大时可能带来线程创建与调度成本。它与 Go `errgroup.WithContext` 的重要差异是：Rust 版本没有派生取消上下文，某一回调失败不会通知其他回调取消。

错误收集器通过 `Arc<Mutex<_>>` 在并发线程间共享。只记录一个错误以匹配“返回一个错误”的外部契约，但并发竞争使具体错误不确定；不得依赖节点顺序选择错误。

`withTiKVConnection` 拥有 connector 返回的 boxed client，action 完成或提前返回错误后客户端立即 drop。没有显式 `close`/`shutdown` 调用，因此适配器必须用 RAII 保证连接资源释放。`FetchMode`、`SwitchModeOnStore`、`Compact` 都是同步阻塞接口；本文件没有异步 runtime、重试、超时或取消令牌。

`FETCH_MODE_RE` 的 `LazyLock` 初始化由标准库保证线程安全；初始化后无锁修改。其余模型无内部锁或全局可变状态。

## 与 Go 版本的对应关系

Rust 符号基本逐项对应 `pkg/lightning/tikv/tikv.go`：Store 过滤、Unimplemented 容错、模式切换、压缩、指标判定、远端 schema 获取、PD/TiKV 版本检查均保留。`pkg/lightning/tikv/tikv_test.go` 与 Rust 独立测试共同验证 Offline 包含/Tombstone 排除、指标值 `0`、版本过旧/过新以及最大 major 的 beta 拒绝。

但当前移植不是完整生产网络等价物，主要差异如下：

- Go `withTiKVConnection` 使用 `grpc.DialContext`、keepalive、TLS dial option，并 `defer conn.Close()`；Rust 只操作注入 connector，未提供真实 gRPC 实现。
- Go `ForAllStores` 使用 `errgroup.WithContext`，失败可使派生 context 取消；Rust 会等待并执行全部筛选节点，不提供取消信号。两者都并行且最终返回一个错误，但中止协作语义不同。
- Go 的 StoreState 来自 `metapb`，注释包含更多中间状态；Rust 本地枚举只有 Up、Offline、Tombstone。当前 `<= Offline` 的主要行为已覆盖，不能据此认为所有 PD 状态值均已建模。
- Go `SwitchMode`/`Compact` 构造 protobuf 请求并记录阶段日志；Compact 还写入 resource-control context 和 Lightning request source。Rust 只向 trait 传递 mode/ranges 或 level/resource group，protobuf 元数据和日志须由适配器或上层补齐。
- Go `FetchMode` 使用 Debug gRPC `GetMetrics`；Rust 复用 `TiKvClient::FetchMetrics`。两者的核心正则和严格 `"0"` 判定一致。
- Go schema 函数通过 TLS HTTP GET 访问 `/schema` 与 `/schema/<schema>`，失败时增加上下文；Rust 仅委托最小 `RemoteSchema`，且 `DBInfo`/`TableInfo` 只保留名称。
- Go 的 PD 版本通过 `pdutil.FetchPDVersion`、区间通过公共 `version.CheckVersion`；Rust 本地解析。Rust 使用 `semver` crate，测试覆盖一个前导 `v`、非法版本和最大 major beta。

因此安全维护原则是：保持已覆盖的可观察契约，同时不要把 trait 替身当作真实网络层已完成的证据。若目标是生产接线，应在独立适配器/上游依赖中实现传输和协议细节，而不是把测试桩逻辑塞入本文件。

## 扩展指南

- 新增 TiKV 节点 RPC：先扩展 `TiKvClient`，再通过 `withTiKVConnection` 添加小型编排函数；明确该 RPC 是否像 SwitchMode/Compact 一样允许 Unimplemented。同步在独立 `pkg/lightning/tikv/tikv_test.rs` 中加入 connector/client mock 覆盖，不要把测试放入生产源文件。
- 扩展 Store 状态：同时审查 `StoreState` 的判别值与派生排序、所有 `state <= max_state` 调用以及 Go `metapb.StoreState` 排序。错误判别值会静默访问不应访问的节点。
- 修改并发策略：若增加限流、取消或聚合错误，应保持 scope 结束前资源回收，并显式定义“首个错误”。需要新增多节点并发、失败后行为和 panic/取消测试；同时比对 Go `errgroup.WithContext`。
- 修改模式指标：更新 `FETCH_MODE_RE` 与 `TestFetchModeFromMetrics`，覆盖缺失行、精确 `0`、`0.0`、非数字、多行及标签变化。若改为数值解析，会改变现有“非精确 0 即 Normal”的兼容行为。
- 修改版本策略：优先复核 Go 公共 `version.CheckVersion`，尤其是最大 major 的 prerelease。同步更新 PD 与 TiKV 两组测试，覆盖前导 `v`、非法版本、min/max 边界和地址化错误文本。
- 补齐真实网络适配：应实现 trait 并验证 TLS、keepalive、context/超时、protobuf request source、HTTP 路径和 RAII 关闭；不要让本文件依赖测试 stub。外部 Rust 依赖必须按仓库政策在独立上游仓库移植、发布 tag，再以统一 tag 的 Git 依赖接入。
- 扩展 schema 模型：当前 `DBInfo`/`TableInfo` 只有 `name`。增加字段前要确认 Go `model` 序列化兼容和调用者需求，并为错误上下文、schema 名编码和不完整响应添加独立测试。

兼容风险集中在状态排序、严格指标字符串、版本 major 上界、Unimplemented 容错和错误文本；性能风险集中在每 Store 一个 OS 线程及无并发上限；正确性风险集中在适配器是否真正实现连接关闭、RPC 状态映射和 Go 请求元数据。

## 验证依据

- RustCodeGraph `status`：索引存在，共 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录包含 `tikv.rs`、`tikv.go` 和两份独立测试。
- RustCodeGraph `node --file pkg/lightning/tikv/tikv.rs`：完整读取 319 行，核对全部错误、模型、trait、函数、惰性正则与版本逻辑。
- RustCodeGraph `explore "pkg/lightning/tikv/tikv.rs ForAllStores CheckTiKVVersion FetchRemoteDBModelsFromTLS FetchRemoteTableModelsFromTLS"`：确认 `CheckTiKVVersion` → `ForTiKVVersions` → `ForAllStores`，并识别 `lightning/pkg/server/lightning.rs` 和 `tikv_test.rs` 的调用证据。
- RustCodeGraph `query`：核对 `TiKvClient`、`withTiKVConnection`、`SwitchModeOnStore`、`FetchMode`、`Compact` 的定义与 Go/Rust同名候选。精确 `callers/callees` 查询因共享工作区已有多项 RustCodeGraph 长查询而未稳定返回；已使用 `explore` 的调用流作为图证据，并把限制明确记录在本文。
- `pkg/lightning/tikv/lib.rs`：确认模块装载、公开再导出及 `#[path = "tikv_test.rs"]` 独立测试接线。
- `pkg/lightning/tikv/Cargo.toml`：确认 crate 名、入口、三项直接依赖和 Go 包移植元数据；该目录无 `doc.go`。
- `pkg/lightning/tikv/tikv.go`：核对真实 gRPC/TLS/HTTP、errgroup、指标、版本与错误注解语义。
- `pkg/lightning/tikv/tikv_test.rs`：核对状态过滤、失败后全部回调、严格指标值、非法/重复版本前缀、最大 major beta 与地址化错误。
- `pkg/lightning/tikv/tikv_test.go`：核对 Go 原测试的 Store 集合、模式指标和版本边界意图。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前另运行任务规定的 11 章节结构检查，并人工复核仅新增本文件、未修改 Rust/Go/Cargo/总计划。
