# `pkg/dxf/framework/schstatus/status.rs`

## 文件定位

本文件定义 DXF（Distributed eXecution Framework）调度状态的 Rust 数据契约，以及该契约与 Go JSON 表示之间的转换规则。crate 入口 [`lib.rs`](lib.rs) 将 `status` 声明为公开模块并再导出全部公共项；[`Cargo.toml`](Cargo.toml) 将该目录声明为独立 crate `astersql-dxf-framework-schstatus`，只依赖 `chrono`、`serde` 和 `serde_json`。`pkg/dxf/framework/handle/status.rs` 的 `GetScheduleStatus` 是最直接的生产构造方：它查询任务、节点和标志后组装本文件的 `Status`。因此，本文件位于“调度运行时事实 → 稳定状态快照/JSON”边界，不负责采集事实、持久化状态或执行扩缩容。

Go 端的应用入口可以在 `pkg/server/handler/tikvhandler/dxf.go` 中复核：状态 HTTP 处理逻辑调用 Go `handle.GetScheduleStatus`，而后者在 `pkg/dxf/framework/handle/status.go` 中构造同路径 Go `schstatus.Status`。RustCodeGraph 将本文件识别为 16 个符号，并确认其被 `handle`、`storage`、`scheduler` 等 DXF crate 引用；精确 `callers` 查询在本次分析中超时，因此具体生产边以源码引用复核为准，未把模糊图结果当作调用结论。

## 核心职责

1. 用 `Version`、`TaskQueue`、`Node`、`NodeGroup` 和 `Status` 表达一个可序列化的调度器状态快照。
2. 用 `Flag`、`PauseScaleInFlag`、`TTLInfo` 和 `TTLFlag` 表达带有效期的调度控制标志。当前已定义的标志名只有 `pause_scale_in`，其用途是减少调度与异步缩容互相冲突时的子任务反复迁移。
3. 通过字段 `rename`、`default`、`skip_serializing_if` 和 `flatten` 保持 Go `encoding/json` 的字段名、`omitempty` 与匿名嵌入语义。
4. 通过 `duration_nanoseconds` 和 `system_time_rfc3339` 两个私有 serde 适配模块，对齐 Go `time.Duration` 的整数纳秒 JSON 和 `time.Time` 的 RFC3339 JSON。
5. 提供 `TTLFlag::String` 与 `Status::String`。前者直接输出 JSON；后者为日志可读性只展示前五个 TiDB 忙碌节点，再追加一个携带总数的说明节点，同时保证原快照不被修改。

## 主要符号

- `is_zero<T: Default + PartialEq>(&T) -> bool`：私有通用零值判断器，供多个 `skip_serializing_if` 使用。它比较 `T::default()`，所以新增使用点前必须确认该类型的 Rust `Default` 等同于 Go 零值。
- `go_zero_time() -> SystemTime`：返回 Unix epoch 之前 `62_135_596_800` 秒，对应 Go `time.Time{}` 的 `0001-01-01T00:00:00Z`，是 `TTLInfo::ExpireTime` 的反序列化默认值。
- `duration_nanoseconds::{serialize, deserialize}`：把 `Duration` 写成有符号 64 位纳秒整数；序列化时对超过 `i64::MAX` 纳秒的值报错，反序列化时拒绝负数。
- `system_time_rfc3339::{serialize, deserialize}`：在 `SystemTime` 与 UTC RFC3339 字符串之间转换；解析接受带时区的 RFC3339，随后归一化为 UTC 时刻。
- `Version = i32` 与 `Version1 = 1`：状态 schema 版本类型与当前首版常量，对应 Go 的 `type Version int` 和 `iota + 1`。
- `TaskQueue { ScheduledCount: i32 }`：记录 running/modifying 等已调度任务数；短生命周期的 cancelling、pausing、resuming 不应计入，筛选规则由上游 `GetScheduleStatus` 执行。
- `Node { ID, IsOwner }`：`ID` 对应 `mysql.tidb_background_subtask.exec_id`，`IsOwner` 标明 DXF Owner。
- `NodeGroup { CPUCount, RequiredCount, CurrentCount, BusyNodes }`：表示 TiDB 或 TiKV worker 组的容量、需求和繁忙成员。`BusyNodes` 是控制器缩容时应避开的节点集合。
- `Flag = String` 与 `PauseScaleInFlag = "pause_scale_in"`：可扩展的标志键类型和当前唯一约定键。Rust 使用 `String` 作为 map 键，常量则是便于借用的 `&str`。
- `TTLInfo { TTL, ExpireTime }`：TTL 的时长与绝对到期时刻；自定义 `Default` 保证到期时间是 Go 零时间，而不是 `SystemTime::UNIX_EPOCH`。
- `TTLFlag { Enabled, TTLInfo }`：带 TTL 的布尔标志。`#[serde(flatten)]` 令 JSON 成为 `enabled`、`ttl`、`expire_time` 同级结构。
- `TTLFlag::String(&self) -> String`：序列化完整标志；通过 `unwrap_or_default()` 将序列化失败折叠为空字符串，以复现 Go 忽略 `json.Marshal` 错误的行为。
- `Status { Version, TaskQueue, TiDBWorker, TiKVWorker, Flags }`：顶层调度快照。三个结构字段始终序列化为对象；版本和空标志表等零值字段可省略。
- `Status::String(&self) -> String`：克隆快照，必要时仅截断 `TiDBWorker.BusyNodes`，再序列化克隆；`TiKVWorker.BusyNodes` 不参与截断。

本文件没有 trait、条件编译项或运行时全局变量。除 serde 适配模块及两个零值辅助函数外，类型、常量和 `String` 方法均为公开 API；crate 根还会将它们再导出。

## 执行流程

生产状态主链如下：

1. `pkg/dxf/framework/handle/status.rs::GetScheduleStatus` 从运行时取得 running/modifying 任务、受管节点、忙碌节点和有效标志，并计算所需节点数。
2. 它构造 `Status`：`Version=Version1`；任务数进入 `TaskQueue.ScheduledCount`；TiDB 节点信息进入 `TiDBWorker`；当前 TiKV 只复用所需节点数；标志进入 `Flags`。
3. 常规 serde 输出按照字段属性产生 snake_case JSON。`TTLInfo.TTL` 经 `duration_nanoseconds::serialize` 变为纳秒整数，`ExpireTime` 经 `system_time_rfc3339::serialize` 变为 UTC RFC3339 字符串。
4. 反序列化时，缺失字段使用各自默认值；`TTLInfo` 的缺失 `expire_time` 特别使用 `go_zero_time`。持续时间先读为 `i64`，再检查能否转换为非负 `u64`。
5. 调用 `Status::String` 时先克隆整个状态。若 TiDB 忙碌节点不超过五个，直接序列化克隆；若超过五个，保留前五个，再追加 `Node { ID: "... too many nodes, total N busy nodes ...", IsOwner: false }`，最终展示六项。
6. 调用 `TTLFlag::String` 时无截断步骤，直接序列化当前值。两种 `String` 都在理论上的序列化错误发生时返回空字符串，而不是传播错误。

暂停缩容标志的生命周期不在本文件内驱动。`pkg/dxf/framework/handle/status.rs::GetScheduleFlags` 读取 `TTLFlag`，只有 `Enabled=true` 时才加入 `Status.Flags`；`getPauseScaleInFlag` 会把 `ExpireTime < SystemTime::now()` 的已启用标志重置为默认关闭状态。本文件只保存与编解码这个结果。

## 数据与状态

所有结构都是可克隆、可比较并可 serde 编解码的值对象，没有内部可变性或隐藏缓存。顶层数据关系是 `Status → TaskQueue + TiDBWorker + TiKVWorker + HashMap<Flag, TTLFlag>`，而 `NodeGroup → Vec<Node>`、`TTLFlag → TTLInfo`。

JSON 零值规则需要特别区分：

- `Version`、计数、布尔值、空字符串、空向量和空 map 使用 `skip_serializing_if`，表现为 Go `omitempty`。
- `TaskQueue`、`TiDBWorker`、`TiKVWorker` 仅有 `default` 而没有顶层 `skip_serializing_if`，因此默认 `Status` 仍会输出三个空对象。这一现状由 `migration_aster_unit_test.rs::status_json_uses_go_field_names_embedding_and_omitempty` 明确断言。
- `TTLFlag.TTLInfo` 被展平，JSON 不会出现名为 `TTLInfo` 的嵌套对象。
- `TTLInfo.ExpireTime` 没有省略规则；默认值也会输出为 Go 零时间字符串。`TTL=0` 则会省略。
- `SystemTime` 的比较和 `Duration` 的范围由 Rust 标准库类型约束；Rust `Duration` 无法表示 Go 的负持续时间。

`Status::String` 为日志构造一个临时克隆，其截断结果不是新的真实调度状态。说明节点的 `ID` 是展示文本而非合法执行节点 ID，消费日志 JSON 的代码不能把它作为可调度节点。

## 依赖与调用关系

上游直接证据：

- `pkg/dxf/framework/handle/status.rs::GetScheduleStatus` 构造 `Status`、`TaskQueue` 和两个 `NodeGroup`。
- 同文件 `GetBusyNodes` 构造并标记 `Node`；`GetScheduleFlags`/`getPauseScaleInFlag` 读取和过滤 `TTLFlag`。
- `pkg/dxf/framework/storage/nodes.rs::GetBusyNodes` 返回 `Vec<schstatus::Node>`，是节点数据的存储层来源之一。
- `pkg/dxf/framework/handle/handle.rs` 的运行时接口和公开包装函数使用 `Node`、`TTLFlag` 与相邻 `TTLTuneFactors`，表明这些类型同时跨越 handle/storage 边界。
- `pkg/dxf/framework/scheduler/lib.rs` 和 `pkg/dxf/framework/handle/lib.rs` 将本 crate 以 `schstatus` 名再导出；相应 Cargo manifest 以路径依赖接入它。

下游依赖：

- `serde` 派生结构编解码，并驱动两个 `with` 适配模块。
- `serde_json::to_string` 实现两个 `String` 方法；`serde_json` 也在测试中把输出重新解析为 `Status` 或 `Value`。
- `chrono::DateTime<Utc>` 承担 `SystemTime` 与 RFC3339 的转换和格式化。
- `std::collections::HashMap`、`std::time::{Duration, SystemTime}` 是内存表示。

RustCodeGraph 的 `files --filter pkg/dxf/framework/schstatus` 显示该区域的 Rust/Go 实现与测试均已索引；`node --file` 给出本文件完整源码和符号位置。其精确 `callers` 命令在 60 秒内未返回，已中止；因此调用关系进一步用上述源文件引用和 Cargo 路径依赖验证，没有宣称图中未能稳定返回的边。

## 错误处理与边界

- duration 序列化仅支持 `0..=i64::MAX` 纳秒。更长的 Rust `Duration` 会通过 serializer error 返回失败；负 JSON 整数会得到 `negative Go duration cannot fit std::time::Duration`。
- RFC3339 非法文本由 `DateTime::parse_from_rfc3339` 转换为 serde 反序列化错误；有效的非 UTC 偏移会映射到等价 UTC `SystemTime`。
- 缺失 `expire_time` 与显式非法 `expire_time` 不同：前者使用 Go 零时间，后者报错。
- `TTLFlag::String` 和 `Status::String` 有意吞掉序列化错误并返回空字符串。这与 Go 源码忽略 `json.Marshal` 错误相符，但调用者若要求可诊断失败，应直接使用 `serde_json::to_string` 并处理 `Result`，不应复用这两个兼容方法。
- `ScheduledCount`、节点计数等使用 `i32`，与本文件的 Go `int` 近似但范围更窄。当前构造方对 `tasks.len()`、`nodes.len()` 使用 `i32::try_from(...).unwrap_or(i32::MAX)` 饱和；本文件自身不校验负计数或字段间一致性。
- `Status::String` 的阈值严格是“大于 5”；恰好五项不追加说明节点。它只截断 TiDB 列表，保持 Go 当前实现的非对称行为。
- `HashMap` 不承诺稳定键顺序。消费者应按 JSON 对象语义读取 `Flags`，测试也不应比较完整字符串的字段顺序。
- 本文件不判断 TTL 是否过期，也不校验 `ExpireTime` 是否等于“当前时间 + TTL”；该一致性属于构造方职责。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、文件句柄或网络资源。各结构由调用方拥有；序列化只进行共享借用。`Status::String` 的完整克隆隔离了日志裁剪与原对象，避免修改共享状态快照；`status_test.rs::test_status_print` 和迁移回归测试都验证原 `BusyNodes` 长度仍为 10。

标志的业务生命周期由上游控制：HTTP/handle 层创建或更新 `TTLFlag`，存储层保存它，读取路径按 `ExpireTime` 把过期值视为默认关闭。`TTLInfo` 中同时保存相对 TTL 和绝对过期时刻，便于持久化和外部观察，但本文件不会启动定时器或清理存储。`SystemTime::now()` 只出现在上游过期判断中，不出现在本文件的纯数据转换中。

## 与 Go 版本的对应关系

直接对照文件为 [`status.go`](status.go)，对应关系如下：

- Go `Version int` / `Version1 = iota + 1` 对应 Rust `Version = i32` / `Version1 = 1`。
- 六个主要结构及 JSON 字段逐项对应；Rust 的 serde `rename` 保留 Go 的 snake_case tag，`flatten` 模拟 Go 匿名嵌入。
- Go `time.Duration` 的 JSON 是整数纳秒；Rust 通过专用适配器保持该线格式。差异是 Go duration 可为负，而 Rust `Duration` 不可为负，所以负值在 Rust 反序列化时明确失败。
- Go `time.Time{}` 的零值不是 Unix epoch；Rust 自定义 `go_zero_time` 保持 `0001-01-01T00:00:00Z` 语义。
- Go 的 `Flag` 是独立 string 类型；Rust 用 `type Flag = String`，因此没有新的名义类型约束。Go `PauseScaleInFlag` 的常量类型是 `Flag`，Rust 常量是 `&str`，插入 map 时需 `to_owned()`。
- Go 两个 `String` 方法使用指针接收者并忽略 `json.Marshal` 错误；Rust 使用不可变借用并以 `unwrap_or_default()` 返回空串。
- Go `Status.String` 复制顶层结构后对切片做截断和 append；Rust 深克隆整个结构后修改克隆。可观察结果一致，Rust 的隔离更直接地保证原向量不变。
- Rust 字段和方法保留 Go 风格大写命名，由 `lib.rs` 的 lint allow 支持；这是迁移兼容选择，不代表通常的 Rust 命名建议。

`status_test.go::TestStatusPrint` 与 `status_test.rs::test_status_print` 一一核对截断结果和非修改语义。`migration_aster_unit_test.rs` 额外覆盖版本、字段名、零值省略、flatten、纳秒与 RFC3339 表示，是 Rust 移植特有的契约回归证据。

## 扩展指南

- 新增状态字段时，先在 Go `status.go` 和 Rust 对应结构中确认字段名、零值、是否省略及默认反序列化行为；随后扩展独立的 `status_test.rs` 或 `migration_aster_unit_test.rs`。不要把 Rust 测试内嵌回生产文件。
- 新增标志时，定义稳定键常量，明确其 TTL/过期处理，并更新 `pkg/dxf/framework/handle/status.rs::GetScheduleFlags` 的收集逻辑。仅向 `Flags` map 增加常量不会使其自动出现在状态中。
- 新增时间或持续时间字段时，复用适配器前先确认线格式完全一致。需要负 duration 时不能复用 `std::time::Duration`，应设计能表达符号的新类型并加入边界测试。
- 修改 `Status::String` 的日志裁剪策略时，要同步 Go `Status.String`、Rust `status_test.rs` 和 Go `status_test.go`；尤其应保持“不修改原值”、阈值、说明节点格式以及 TiDB/TiKV 选择的一致性。
- 若序列化调用场景需要可靠错误报告，应新增返回 `Result` 的 API，而不是改变现有 `String` 的兼容语义，以免破坏 Go 对齐。
- 若扩大版本 schema，应新增版本常量并让构造方显式选择版本；不能只添加字段却继续假设 `Version1` 消费者一定理解它。
- 性能上，`Status::String` 会克隆所有字段，忙碌节点很多时成本与完整状态大小成正比。若要优化，必须先证明不会改变输出与原对象隔离，并添加大列表等价测试；不能通过原地截断换取性能。
- crate API 或依赖变化还需同步 `Cargo.toml` 及直接消费者 manifest；本次文档任务未更改任何代码或 Cargo 配置。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/dxf/framework/schstatus` 确认同目录 8 个 Rust/Go 源与测试文件；`node --file pkg/dxf/framework/schstatus/status.rs --offset 1 --limit 260` 和后续 251 行读取覆盖完整 269 行源码；`query` 确认 `TaskQueue`、`Node`、`NodeGroup`、`TTLInfo`、`TTLFlag`、`Status` 及四个 serde 适配函数。精确 `callers` 查询超时并被中止，未作为完成证据。
- 生产源码：[`status.rs`](status.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、`pkg/dxf/framework/handle/status.rs`、`pkg/dxf/framework/storage/nodes.rs`、`pkg/dxf/framework/handle/handle.rs`、`pkg/dxf/framework/scheduler/lib.rs`。
- Go 对照：[`status.go`](status.go)、`pkg/dxf/framework/handle/status.go`、`pkg/server/handler/tikvhandler/dxf.go`。
- 独立测试：[`status_test.rs`](status_test.rs)、[`status_test.go`](status_test.go)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并参考 `pkg/dxf/framework/handle/status_testkit_test.rs` 验证生产构造结果与过期标志行为。
- 人工复核结论：本文件存在的理由是集中维护 DXF 调度状态的跨语言数据/JSON 契约；运行时由 handle 层组装值、由 serde 或两个 `String` 方法输出；安全扩展必须同时维护 Go 对照、构造方和独立契约测试。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构验证命令与退出码在交付时记录。
