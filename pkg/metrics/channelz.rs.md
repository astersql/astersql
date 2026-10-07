# `pkg/metrics/channelz.rs`

## 文件定位

`channelz.rs` 是 `astersql-metrics` crate 内部的 gRPC channelz 到 Prometheus 的转换层。模块由 `pkg/metrics/lib.rs` 以私有 `mod channelz` 装配；外部不会直接构造它，而是经 `pkg/metrics/metrics.rs` 中的 `RegisterMetrics -> setup_channelz_collector -> init_grpc_channelz_collector_locked -> ChannelzCollector::new` 注册到默认 Prometheus registry。crate 边界和直接依赖由 `pkg/metrics/Cargo.toml` 确认：本文件直接使用 `grpcio`、`prometheus` 与 `serde_json`。

生产采集时，`Collector::collect` 直接读取 grpcio C-core 的进程级 channelz JSON 快照。它不是 gRPC 服务端，也不管理网络监听器或客户端连接；单例创建、注册、注销和测试隔离由相邻的 `pkg/metrics/metrics.rs` 负责。

## 核心职责

- `ChannelzCollector` 实现 `prometheus::core::Collector`，声明并产出 9 个指标族：通道调用量及最后调用时间、socket 流/消息/keepalive、最后流/消息时间、流控窗口，以及按 RPC 分类的抓取错误总数。
- `SystemSource` 把 `grpcio::channelz::{get_top_channels,get_channel,get_subchannel,get_socket}` 隔离在 `SnapshotSource` trait 后；测试可通过 `StaticSource` 复用同一遍历和编码流程。
- `Walker` 从顶层 channel 分页入口沿 channel、subchannel、socket 引用遍历，只采集叶子 subchannel 和非内部 socket，避免父子聚合值重复计数，也避免采集器自身连接污染观测结果。
- `families` 将中间 `Sample` 按名称、帮助文本和 Prometheus 类型稳定分组，再编码为 protobuf `MetricFamily`。
- `timestamp`、`format_address`、`decode_base64` 等辅助函数兼容 channelz JSON 的字符串数值、RFC 3339 UTC 时间以及 named/UDS/TCP-IP 地址表示。

## 主要符号

- `ChannelzCollector { descs, errors, handle }`：crate 内可见的采集器。`descs` 和 `errors` 通过 `Arc` 在克隆间共享；`handle` 只用于 `handle_id` 验证重复初始化复用同一实例身份。
- `ChannelzCollector::new() -> prometheus::Result<Self>`：调用 `channelz_descs` 预先校验全部描述符；描述符冲突或标签定义非法时把 Prometheus 错误交给注册层处理。
- `Collector::{desc, collect}`：`desc` 返回预构造描述；`collect` 用进程级 `LazyLock<grpcio::Environment>` 保证 C-core 环境存在，再以 `SystemSource` 执行一次采集。
- `SnapshotSource` / `SystemSource`：四种 channelz 读取操作的抽象与生产实现。空字符串经 `nonempty` 变成 `None`，按抓取失败处理。
- `FetchErrors`：四个 `AtomicU64` 分别记录 `GetTopChannels`、`GetChannel`、`GetSubchannel`、`GetSocket` 的缺失响应或 JSON 解析失败。
- `Walker`：持有快照源、错误计数、待编码样本，以及三组 `seen_*` ID 集合。关键方法为 `walk_top_channels`、`walk_channel`、`walk_fetched_channel`、`walk_socket`、`add_channel_samples`、`add_socket_samples` 和 `add_fetch_errors`。
- `SampleKind` / `Sample`：尚未编码成 protobuf 的 Counter/Gauge 样本模型；标签值在采集时拥有自己的 `String`。
- `id` / `number`：分别宽容解析字符串或 JSON number 形式的 ID 与数值；ID 无法解析时返回 `None`，普通指标数值无法解析时降为 `0.0`。
- `channelz_descs` / `families`：集中固定指标名、帮助文本、类型和标签集合，并完成 Prometheus protobuf 编码。
- `timestamp` / `days_from_civil`：仅接受带 `Z` 的 UTC 日期时间，转换成含小数秒的 Unix 秒。
- `format_address` / `decode_base64`：依次识别 other、UDS、TCP/IP 地址；支持 IPv4、IPv4-mapped IPv6、IPv6、空地址及未知字节序列。
- `StaticSource` / `collect_snapshots_for_test`（仅 `cfg(test)`）：把静态 JSON 拓扑注入生产转换路径；未知节点种类会立即 panic，防止测试 fixture 静默拼错。

## 执行流程

1. `RegisterMetrics` 调用 `setup_channelz_collector`。非测试环境下，后者在进程级互斥锁内幂等创建 `ChannelzCollector`，随后注册其克隆；测试环境按 `cfg!(test)` 或 `InTest` 整段跳过。
2. Prometheus gather 调用 `ChannelzCollector::collect`。采集器初始化一次 grpcio `Environment`，随后 `collect_from(&SystemSource)` 创建本轮 `Walker`。
3. `walk_top_channels` 从 `start_id = 0` 分页读取顶层 JSON。每页先解析 `channel` 数组并记录最大 channel ID；服务端 `end=true`、空页或游标没有前进都会结束，防止异常响应造成死循环；否则以下一 ID 继续。
4. `walk_channel` 先拒绝缺失/零 ID、重复 ID和内部 `bufnet` target。普通 channel 本身不产出调用指标，只继续沿 `channelRef`、`subchannelRef`、`socketRef` 遍历。
5. 只有同时满足“是 subchannel、具有 socket 引用、不再引用 channel/subchannel”的叶节点才由 `add_channel_samples` 产出 started/succeeded/failed counter，以及存在时的最后调用时间 gauge。
6. `walk_fetched_channel` 按引用类型获取并解析单个 channel/subchannel；某个子节点失败只增加相应错误计数并返回，不中断其他引用。
7. `walk_socket` 去除零 ID、重复 ID和缺少远端端点的内部 socket，然后由 `add_socket_samples` 生成流、消息、keepalive、活动时间与流控窗口指标。
8. 遍历结束后，`add_fetch_errors` 总是加入四条累计错误样本；`families` 用 `BTreeMap` 分组，因此指标族顺序按分组键稳定，但同族内样本保持遍历产生顺序。

## 数据与状态

`ChannelzCollector` 的描述符、错误计数和身份句柄都通过 `Arc` 共享。描述符创建后只读；`handle` 不参与生产逻辑；`FetchErrors` 跨多次 gather 累计，因此 `tidb_grpc_channelz_fetch_errors_total` 是采集器生命周期内的累计 counter，而不是单轮错误数。

每轮 gather 都新建一个 `Walker`。其 `samples` 和 `seen_channels`、`seen_subchannels`、`seen_sockets` 只存在于本轮采集：相同 ID 即使被多条拓扑边引用也只处理一次，环形引用也会在再次访问时终止。顶层 channel 和 subchannel 使用不同的去重集合，数值相同不会互相遮蔽。

标签具有较高基数：channel/subchannel 使用 `id`、`target`，socket 使用 `id`、`local`、`remote`，并附加 `type`、`direction` 或 `side`。新增标签会同时改变 `channelz_descs` 与样本编码契约，必须同步两侧，否则 Prometheus 收集可能因描述和实际标签不一致而失效。

## 依赖与调用关系

上游生产链为 `pkg/metrics/metrics.rs::RegisterMetrics` → `setup_channelz_collector` → `init_grpc_channelz_collector_locked` → `ChannelzCollector::new`，运行时由 Prometheus registry 调用 `Collector::desc/collect`。`pkg/metrics/lib.rs` 保持模块私有，只由 crate 内的注册与测试代码访问。

下游依赖包括：

- `grpcio::Environment` 与 `grpcio::channelz::*`：提供进程内 C-core channelz JSON；本文件没有自建 channelz gRPC server。
- `serde_json::Value`：以宽容、按字段读取的方式解析不同节点快照。
- `prometheus::{Collector, Desc}` 和 `prometheus::proto::*`：声明 collector 并手工构造指标族。
- `crate::metrics::{is_internal_channelz_target,is_internal_channelz_socket}`：复用与 Go 版本一致的内部连接过滤规则。
- 标准库 `Arc`、`LazyLock`、`AtomicU64`、`HashSet`/`HashMap`/`BTreeMap`：分别承担共享生命周期、一次性环境初始化、无锁错误累计、去重/测试 fixture/稳定分组。

RustCodeGraph 的符号查询能定位 `ChannelzCollector`（`channelz.rs:72`）和 `collect_snapshots_for_test`（`channelz.rs:733`），但本次 `callers/callees` 查询未返回图边并长时间无输出；上述调用关系因此由 `lib.rs`、`metrics.rs` 和独立测试中的直接引用核对，而非根据缺失图边推断。

## 错误处理与边界

- 顶层 RPC 返回空串/`None` 或顶层 JSON 无法解析时，增加 `top_channels` 错误并结束本轮拓扑遍历；已经累计的错误指标仍会输出。
- 单个 channel、subchannel 或 socket 获取/解析失败只增加对应错误 counter，其他分支继续采集。未知或缺失字段通常由 `serde_json::Value` 的空值语义降级为空数组、空字符串或零。
- 缺失/非法 ID不会生成节点指标；ID 为零显式跳过。普通数值缺失或格式错误按零输出，这是可用性优先的选择，也意味着“真实零”和“无法解析”只能通过代码审计区分，当前不会增加 fetch error。
- `timestamp` 对非字符串、无 `Z`、日期/时间字段非法的输入返回 `None`，相应 gauge 不输出；最后流创建时间还额外过滤 Unix 秒恰为 `0.0` 的值。函数没有完整验证月/日范围，输入可信度依赖 channelz 生产方。
- `format_address` 对非法 Base64 或非对象 TCP 地址返回空字符串；空 IP 字节表示 `<nil>`，负端口不拼接端口，IPv6 用方括号包裹后再加端口。
- 顶层分页除尊重 `end` 外还检查空页和游标前进，避免无限循环。三组 `seen_*` 进一步防止引用环和重复指标。
- `ChannelzCollector::new` 是本文件主要可传播错误点；实际注册层对初始化失败采取 best-effort（忽略错误并返回 `Ok`），注册失败则仍可从 `setup_channelz_collector` 返回。

## 并发与资源生命周期

collector 可被克隆和并发 gather：共享描述符只读，错误只通过 `AtomicU64` 以 `Ordering::Relaxed` 累加；该计数不与其他内存状态建立先后关系，因此 relaxed 足以满足独立总数统计。每轮 `Walker` 是栈上独占状态，不共享样本或去重集合，不需要锁。

`GRPC_ENV` 是 `Collector::collect` 内的进程级 `LazyLock`，首次采集创建单线程 grpcio 环境，随后保持到进程结束。`SystemSource` 本身无字段，其调用依赖 grpcio C-core 的全局 channelz 状态。

进程级注册互斥、collector 单例和注销位于 `metrics.rs::GRPC_CHANNELZ_COLLECTOR`，不属于本文件。Rust 状态仅保存 `collector` 与 `registered`，cleanup 注销 collector 后重置状态；与 Go 版本不同，它没有 listener/server/client connection 可关闭。测试使用 `GRPC_CHANNELZ_TEST_LOCK` 串行化会修改该进程级单例的用例。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/metrics/metrics.go`。两版对齐的外部语义包括：在指标注册流程安装单例 collector；测试模式跳过；过滤 target 为 `bufnet`/`passthrough:///bufnet` 的内部通道和无 remote/remoteName 的内部 socket；只采集叶子 subchannel 与 socket；重复初始化保持幂等；cleanup 后清空状态。`pkg/metrics/metrics_internal_test.go` 和 Rust 的 `metrics_internal_test.rs` 对这些单例、跳过、过滤和 gather 行为做了对应验证。

实现机制存在重要差异。Go 版创建 `bufconn.Listener`、channelz gRPC server、client connection，再使用 `tikvcollectors.NewChannelzCollector` 通过 RPC 采集；Rust 版没有移植这组网络资源，而是在 `SystemSource` 中直接调用 `grpcio` C-core channelz API，并在本文件自行完成 JSON 遍历、过滤、地址/时间转换及 Prometheus protobuf 编码。因此扩展 Go 的 `ChannelzCollectorOpts` 或 tikv collector 指标时，Rust 侧必须显式审查并同步相应字段、标签和边界逻辑，不能假设依赖库自动继承变化。

`pkg/metrics/channelz_test.rs` 进一步固定了 Go 风格边界：零 ID 与无 `data` 节点不产出业务指标；UDS/TCP 地址格式化一致；Unix epoch 的最后流时间不输出。`metrics_internal_test.rs::test_grpc_channelz_snapshot_emits_leaf_subchannel_and_socket_metrics` 验证真实形状的静态拓扑能够生成叶子 subchannel 和 socket 指标，而不只是错误 counter。

## 扩展指南

- 新增一个 channelz 指标时，应同时增加名称/帮助常量、在 `channelz_descs` 固定标签集合、在 `add_channel_samples` 或 `add_socket_samples` 生成同标签样本，并在独立的 `pkg/metrics/channelz_test.rs` 或 `pkg/metrics/metrics_internal_test.rs` 增加 fixture 与断言；不要把测试内嵌回生产文件。
- 扩充遍历节点类型或引用边时，优先扩展 `SnapshotSource` 与 `Walker`，并保持“失败局部化”“ID 去重”“内部连接过滤”和分页必终止四项不变量。若要对解析错误单独计数，应明确是否改变现有四类 RPC counter 的语义。
- 调整过滤条件时，需要同步审查 `metrics.rs::is_internal_channelz_target`、`is_internal_channelz_socket` 和 Go 的 `channelzCollectorOpts`，避免采集器自观测导致递归噪声或指标基数膨胀。
- 改动地址或时间格式必须覆盖 IPv4、IPv4-mapped IPv6、IPv6、UDS、named address、空/非法 Base64、带小数秒和 epoch 等边界。当前手写 Base64 和日期转换没有外部 crate 的完整校验，替换实现时需确认输出兼容性。
- 性能风险主要来自全量 JSON 解析、递归遍历、每个样本的标签字符串分配和高基数标签。新增字段前应评估 gather 频率、拓扑规模和指标数量，避免把父级聚合节点也纳入而重复计数。
- 若修改 collector 的共享字段或错误统计，必须维持 `Clone + Send + Sync` 的实际可用性，并核对 `metrics.rs` 中克隆注册/注销及 `handle_id` 单例测试。

## 验证依据

- 源码全读：`pkg/metrics/channelz.rs`；模块与注册入口：`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`；crate 声明：`pkg/metrics/Cargo.toml`。
- Rust 独立测试：`pkg/metrics/channelz_test.rs`；相关单例、过滤、gather 和静态拓扑验证：`pkg/metrics/metrics_internal_test.rs`；另由 `pkg/metrics/metrics_2_aster_unit_test.rs` 覆盖内部 target/socket 判定。
- Go 对照：`pkg/metrics/metrics.go` 的 `setupChannelzCollector`、`initGrpcChannelzCollectorLocked`、`channelzCollectorOpts`、内部连接判定和 cleanup；测试依据为 `pkg/metrics/metrics_internal_test.go`。
- RustCodeGraph：`status` 显示本仓库索引可用（11,467 files、307,296 nodes、1,848,419 edges）；`query ChannelzCollector --kind struct` 和 `node ChannelzCollector` 定位到 `channelz.rs:72`，`query collect_snapshots_for_test --kind function` 定位到 `channelz.rs:733`。`files --filter pkg/metrics/channelz` 没有命中，`callers/callees` 查询未产出可用边，因此调用边另以直接源码引用核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认文档存在且恰好包含 11 个固定二级章节，并人工检查没有修改 Rust、Go、Cargo 或只读的 `plan.md`。
