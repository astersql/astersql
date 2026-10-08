# `pkg/store/mockstore/unistore/pd/client.rs`

## 文件定位

本文件是独立 crate `astersql-store-mockstore-unistore-pd` 的主体实现；crate 入口 `pkg/store/mockstore/unistore/pd/lib.rs` 公开 `client` 模块并再导出其全部符号，根 `Cargo.toml` 又以 `facade_store_mockstore_unistore_pd` 引入它，最终由 `pkg/lib.rs` 暴露在 `store::mockstore::unistore::pd` 路径下。它对接真实或外部 PD，负责成员发现、leader 切换、元数据 RPC、TSO 和 Region 心跳流，不是 `pkg/store/mockstore/unistore/pd.rs` 中进程内 mock PD 的实现。

当前接线应按事实理解：RustCodeGraph 将本文件纳入索引，但对关键方法查询不到调用边；精确搜索只找到根 facade 再导出，没有找到 Rust 业务代码直接调用 `NewClient` 或本文件的 `PdClient`。`server/Cargo.toml` 对本 crate 的依赖位于永不启用的 `cfg(any())` 下，`tikv/Cargo.toml` 的依赖只在 Windows 目标下声明。因而它目前是已经实现并可独立测试的移植边界，不能据此断言已进入默认 UniStore 运行主链。

## 核心职责

- `PdClient::new` 规范化端点，发现可用 PD leader，缓存集群 ID，并启动 leader 检查和 Region 心跳两个后台线程。
- `Client` trait 描述 UniStore 所需的 PD 能力，包括 ID 分配、Bootstrap、Store/Region 查询、Region 分裂、GC safe point、Store/Region 心跳和 TSO。
- `do_request` 为一元 RPC 统一提供 1 秒超时、最多 10 次尝试以及失败后异步触发 leader 刷新的策略。
- `heartbeat_stream_loop` 维护到 leader 的双向 Region 心跳流，发送失败时通过 `PendingHeartbeat` 保存请求，重建流后优先重放。
- `update_leader`、`switch_leader` 和 `get_or_create_channel` 共同维护成员 URL 顺序、当前 leader 以及按端点复用的 gRPC channel。
- `check_response_header` 将 PD 协议响应头中的错误转换成 `PdError::Response`，避免把协议层失败误当作成功。

## 主要符号

- 常量 `PD_TIMEOUT`、`RETRY_INTERVAL`、`MAX_RETRY_COUNT` 分别固定为 1 秒、1 秒和 10 次；它们决定一元请求和初始化的等待上界。
- `PdError` 枚举覆盖 gRPC 错误、非法端点、缺少 leader/时间戳、PD 响应错误、关闭和重试耗尽；`Result<T>` 是本模块统一返回类型。
- `Region` 是对 Go `router.Region` 的本地映射，保存可选 Region 元数据、可选 leader、down peers 和 pending peers。
- `Client` 是线程安全能力接口；`PdClient` 是可克隆句柄，内部以 `Arc<Inner>` 共享所有状态。
- `PendingHeartbeat::{restore,take,next}` 管理单个发送失败请求；`next` 保证 pending 请求优先于队列请求。生产流实际使用 `take` 加 `region_rx.try_recv`，`next` 主要提供同等语义的可测试辅助入口。
- `ConnectionState` 保存 `endpoint -> Channel` 缓存和 leader 地址；`Inner` 保存 URL、集群 ID、gRPC 环境、通道、锁、停止标志和线程句柄。
- `normalize_pd_urls` 给无 scheme 的地址补 `http://`；`ordered_member_urls` 保持非 leader URL 在前、leader URL 在后。
- `retry_with_policy` 是通用同步重试器；每次失败都通知并休眠，包括最后一次失败之后，最终统一返回 `TooManyRetries`，不保留最后一个具体错误。
- `check_response_header`、`check_optional_header` 和 `response_region` 分别完成响应错误检查、可选头兼容和 Region DTO 转换。
- `PdClient::{get_all_stores,get_cluster_config}` 是未列入 `Client` trait 的具体类型附加 API；前者明确排除 Tombstone Store。
- `NewClient` 只是保留 Go 命名的兼容构造函数，直接委托 `PdClient::new`；私有 `Pipe` trait 只服务于 TSO async 结果的链式包装。

## 执行流程

1. 构造阶段：`PdClient::new` 调用 `normalize_pd_urls`，创建容量为 1 的 leader 通知通道和容量为 64 的 Region 请求通道，并初始化共享状态。随后最多 10 次调用 `update_leader`；成功响应中的 `header.cluster_id` 被原子写入（缺头时为 0），然后 `start_workers` 启动两个线程。
2. leader 发现：`update_leader` 按当前 URL 顺序调用 `get_members`。只有响应包含至少一个 leader client URL 才算有效；随后用 `ordered_member_urls` 重排地址，并由 `switch_leader` 确保首个 leader 地址已有 channel、再更新当前 leader。
3. 普通 RPC：各 `Client` 方法通过 `request_header` 带上缓存的 cluster ID，构造 protobuf 请求并交给 `do_request`。后者每次重新取得当前 leader client，附加 `PD_TIMEOUT`，失败时非阻塞通知刷新线程。需要协议校验的方法再调用 `check_optional_header`；`alloc_id` 和 `bootstrap` 没有额外检查响应头，与对应 Go 实现保持一致。
4. Region 查询：`get_region`/`get_region_by_id` 成功后经 `response_region` 拆出元数据、leader、pending peers，并从 `PeerStats` 中过滤出实际 down peer。
5. TSO：`get_ts` 在 `do_request` 内创建 TSO 双向流，发送 `count = 1` 的请求、关闭发送端并等待首个响应；缺少响应映射为 gRPC `RemoteStopped`，缺少 timestamp 映射为 `MissingTimestamp`。
6. Region 心跳：`report_region` 将请求阻塞写入容量 64 的队列。后台线程创建 leader 心跳流，`run_heartbeat_stream` 每轮优先发送 pending、否则尝试取队列；发送失败先恢复该请求再返回。接收侧以约 20ms tick 周期重新检查停止状态，收到响应则调用当前 handler。
7. 故障恢复：心跳流创建或运行失败后，`schedule_update_leader` 合并重复通知，`sleep_until_retry` 以 10ms 小步等待约 1 秒；leader 线程也每 60 秒主动刷新一次。
8. 关闭：`close` 用原子交换保证幂等，唤醒 leader 线程，等待两个 worker 退出，最后清空 channel 缓存。`Client` 文档约定关闭后不得继续使用。

## 数据与状态

`urls` 与 `connections` 用 `RwLock` 分隔成员发现数据和连接数据；读 RPC 通常只需读取当前 leader/channel，切换或新增连接才持写锁。`cluster_id` 在启动发现完成后以 `Release` 写入、以 `Acquire` 读取，之后没有更新路径。`stopped` 同样使用 Acquire/Release 类顺序协调关闭可见性。

Region 请求队列是有界 crossbeam channel（容量 64），leader 刷新通知也是有界 channel（容量 1），因此重复刷新请求会被 `try_send` 合并。`PendingHeartbeat` 只保存一个请求；发送循环串行取出并发送，请求失败后立即退出当前流，所以不会在正常控制流中覆盖多个失败请求。handler 初始为空闭包，存于 `RwLock<Arc<dyn Fn...>>`，分发前克隆 `Arc`，调用时不持 handler 锁。

连接缓存以完整 endpoint 字符串为键，实际 `ChannelBuilder` 目标由 URL 的 host 到 port 片段生成；channel 可克隆，缓存清理依赖引用计数释放。`workers` 只在启动时追加，在关闭时整体取出并 join。

## 依赖与调用关系

上游装配链是 `pd/lib.rs -> client::*`、根 `Cargo.toml` facade 依赖、`pkg/lib.rs -> store::mockstore::unistore::pd` 再导出。当前未检索到默认 Rust 业务路径中的直接调用者；Go 对照路径中，`tikv/region.go` 会调用 `pd.Client.ReportRegion`，说明该能力在原 Go UniStore Region 管理流程中的位置，但这不是 Rust 当前接线的证据。

下游依赖由 `pd/Cargo.toml` 明确：`grpcio` 提供 channel、调用选项和双向流；带 `protobuf-codec` 的带 tag Git 依赖 `tikv-client-proto`（包名 `kvproto`）提供 `metapb`/`pdpb`；`crossbeam-channel` 承载线程间请求与通知；`futures`、`futures-timer` 驱动双向流；`url` 解析 endpoint。标准库的 `Arc`、原子类型、`Mutex`/`RwLock` 和线程句柄管理共享状态与生命周期。

关键内部调用边为：`new -> update_leader -> get_members/get_or_create_channel + switch_leader`，`Client` 一元方法 `-> do_request -> leader_client -> pdpb::PdClient::*_opt`，`report_region -> region_tx -> run_heartbeat_stream`，以及流失败 `-> schedule_update_leader -> check_leader_loop -> update_leader`。RustCodeGraph 已成功索引文件和符号，但对所查 `NewClient`、`report_region`、`get_ts`、`get_all_stores` 未返回 callers/callees，因此这些边由本文件源码和精确仓库搜索核验。

## 错误处理与边界

非法 URL、无 host 或空连接目标返回 `InvalidEndpoint`；遍历所有端点仍找不到带 URL 的 leader 返回 `MissingLeader`。一元 RPC 的具体 gRPC 错误会触发 leader 刷新，但耗尽后对外折叠为 `TooManyRetries`。初始化稍有不同：它保留并返回最后一个 `update_leader` 错误；如果没有记录则返回 `MissingLeader`。

响应头缺失被 `check_optional_header` 当作成功，这是兼容策略；响应头存在错误时，错误类型会转换为类似 Go protobuf 文本的全大写下划线形式，并保留 message。调用者不能只以传输成功判断业务成功。TSO 还单独校验 timestamp 是否存在。

所有锁使用 `expect("... lock poisoned")`，锁中毒会 panic；heartbeat handler 自身 panic 也会终止心跳 worker，`close` 忽略 join 的 panic 结果。`report_region` 是阻塞发送：队列满且消费者停滞时调用者会等待；关闭并不显式断开 `region_tx`，所以关闭后的调用并非预先检查 `stopped`，只有接收端断开时才会得到 `Closed`。这些是扩展时必须保留或明确修订的可观察边界。

`ask_batch_split` 将 `i32 count` 直接转换为 `u32`，负值会按 Rust `as` 规则变成很大的无符号数；当前代码没有输入校验。`get_or_create_channel` 接受 URL scheme 但只取 host/port 建立未配置 TLS 的 grpcio channel，因此 `https://` 的规范化保留不等于建立 TLS 连接。

## 并发与资源生命周期

每个 `PdClient` clone 指向同一个 `Inner`。构造成功后固定存在两个 OS 线程：leader 线程等待通知或 60 秒超时；心跳线程在同步线程内用 `futures::executor::block_on` 驱动一条双向流。没有 Tokio runtime，也没有为每条心跳创建新线程。

关闭通过 `stopped` 协调：leader 线程由通知立即唤醒；心跳流在最多约 20ms 的 tick 后观察停止并 cancel 收发端，重试等待也每 10ms 检查一次。`close` join worker 后清空缓存；它不会销毁其他仍持有的 `PdClient` clone，但共享停止标志使这些句柄不应再使用。类型未实现 `Drop`，若调用者丢弃所有句柄前没有显式 `close`，没有代码层面的自动 join 保证。

连接建立采用先读缓存、锁外创建 channel、再写锁 `entry` 的方式；并发竞争可能临时创建多条 channel，但只有一条进入缓存，其余随局部 clone 释放。leader 地址更新和连接缓存共用一个 `RwLock<ConnectionState>`，确保读者不会看到 leader 指向尚未插入缓存的地址。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/pd/client.go`。Rust 保留了 Go 的无 scheme 补 `http://`、非 leader URL 在前而 leader 在后、1 秒 RPC 超时、1 秒重试间隔、10 次上限、容量 1 的 leader 通知、容量 64 的 Region 队列、每分钟刷新 leader、失败心跳优先重放及 nil/`None` handler 退化为空实现。

并发模型做了等价的语言适配：Go 的 `context + cancel + WaitGroup + goroutine` 对应 Rust 的 `AtomicBool + JoinHandle + thread`；Go `sync.RWMutex`/`atomic.Value` 对应 Rust `RwLock` 与 `Arc` handler；Go gRPC 对应 `grpcio`。Rust `Client` 方法不接收 `context` 或可选查询参数，因此调用级取消只能由内部固定超时和全局 `close` 控制；这是一项接口差异，而非已验证的完全替代。

Rust 的 `Region` 自行替代 Go `router.Region`。Rust 额外公开 `PdError` 和若干可测试辅助函数；`get_all_stores`、`get_cluster_config` 与 Go 的具体 `client` 方法对应，但两边都没有把它们列入本文件顶部的 `Client` 接口。Rust 清空缓存时依赖 channel drop，Go `Close` 会逐个调用连接的 `Close`。真实 PD 互操作和上述资源释放差异未由当前迁移单测覆盖。

## 扩展指南

- 新增 PD 一元 RPC 时，应在 `Client` trait（若属于通用能力）及 `impl Client for PdClient` 同步声明，统一走 `request_header`、`do_request` 和适用的 `check_optional_header`；同时对照 Go 方法是否检查协议响应头。
- 改变重试或 leader 策略时，应集中修改 `retry_with_policy`、`do_request`、`update_leader` 和 `heartbeat_stream_loop`，避免普通 RPC 与流式 RPC 形成不一致的故障语义。
- 改变 Region 心跳时，必须保持“发送失败请求不丢失且先于新请求重放”的不变量，并审查容量 64 队列、单槽 pending、handler panic 和关闭时阻塞发送问题。
- 增加 TLS、鉴权或非标准 URL 支持时，接入点是 `get_or_create_channel`；当前只把 host/port 交给明文 `ChannelBuilder`，不能仅修改 `normalize_pd_urls`。
- 改变关闭语义时，需要同时处理两个 worker、正在进行的双向流、重试等待、队列生产者和所有 clone；如引入自动清理，应避免任一 clone 的 `Drop` 提前关闭共享客户端。
- 测试必须继续放在独立文件 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。辅助逻辑可扩展现有单测；真实请求、leader 切换、TSO、关闭/背压和心跳重连需要独立的可控 gRPC 服务测试。若未来完成业务接线，还应在实际调用 crate 的独立测试中覆盖接口集成。
- 兼容风险集中在 Go/Rust 接口差异、响应头检查和错误折叠；性能风险集中在阻塞 `thread::sleep`、20ms 心跳轮询、每次 TSO 新建流及 channel 竞争创建。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/store/mockstore/unistore/pd` 找到 `client.rs`、`lib.rs`、`client.go` 和独立迁移测试；`node --file ... --offset 1/501` 完整读取 `client.rs` 1--876 行。对 `NewClient`、`report_region`、`get_ts`、`get_all_stores` 的 callers/callees 查询无结果，故没有把图中不存在的调用边写成事实。
- 生产源码：`client.rs` 的 `PdClient::new`、`update_leader`、`do_request`、`run_heartbeat_stream`、`impl Client for PdClient` 和 `close` 是初始化、RPC、心跳与生命周期结论的直接依据。
- crate 与装配：`pd/Cargo.toml`、`pd/lib.rs`、根 `Cargo.toml` 的 facade 依赖和 `pkg/lib.rs` 的再导出验证 crate 边界；`server/Cargo.toml`、`tikv/Cargo.toml` 与仓库精确搜索验证当前有限接线状态。
- Go 对照：`pd/client.go` 验证接口、重试、leader、心跳、TSO、响应头与关闭语义；`tikv/region.go` 的 `ReportRegion` 调用只用于说明 Go 主链位置。
- 独立 Rust 测试：`pd/migration_aster_unit_test.rs` 覆盖 URL 规范化、leader URL 排序、10 次重试与提前成功、pending 心跳优先级、协议错误 message/type 保留；它没有覆盖真实网络 RPC、TSO、线程关闭或背压。
- 最近的目标目录及其 `pkg/store` 上级范围内未找到 `doc.go`，因此没有额外包契约可引用。本文为纯文档分析，按任务约束未运行 Cargo；最终仅执行固定 11 章节的结构验证。
