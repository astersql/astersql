# `br/pkg/stream/stream_status.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-stream`（`br/pkg/stream/Cargo.toml`），由 `br/pkg/stream/lib.rs` 以 `pub mod stream_status` 装入，并通过 `pub use stream_status::*` 扁平导出。它承载日志备份任务的状态快照、checkpoint 选择、状态文本、打印器抽象，以及从 TiKV metrics 文本提取计数并估算 QPS 的纯函数。

当前 Rust 实现是一个已由独立单元测试覆盖、但尚未接入 Rust BR `stream status` 命令主链的库级移植。仓库文本搜索只发现 `br/pkg/stream/stream_status_test.rs` 与 `stream_misc_test.rs` 使用这些状态 API；`br/cmd/br/stream.rs` 没有导入本 crate 的 `TaskStatus`、打印器或状态控制器。Go 的同名文件则包含完整的视图和 `StatusController` 编排。因此，本文不能把 Rust 文件描述成已经能够自行查询 etcd/PD/TiKV 并输出完整命令结果。

目录中不存在 `doc.go`；包级 Rust contract 来自 `br/pkg/stream/lib.rs`。源文件没有条件编译项，测试由 `lib.rs` 在 `#[cfg(test)]` 下从独立文件挂载。

## 核心职责

- 用 `CheckpointType` 与 `Checkpoint` 表示 store 级或 global checkpoint，并提供不会构造非法组合的两个构造器。
- 用 `TaskStatus` 聚合任务 protobuf 桩、暂停状态、全局与逐 store 进度、QPS、最后错误和 Pause V2 严重度。
- 用 `StatusString` 将内部状态按 `ERROR > PAUSE > NORMAL` 的优先级压缩为稳定展示文本。
- 用 `GetMinStoreCheckpoint` 选择最小 store checkpoint；一旦遇到 global checkpoint，则立即以 global 为最终结果；没有 store/global 数据时回退到任务 `StartTs`。
- 用 `TaskPrinter` 解耦任务收集与展示；`CollectTaskPrinter` 提供简单文本实现，`TeeTaskPrinter` 同时留存任务副本并转发给另一个打印器。
- 用 `ParseLogBackupHandleKvBatchSum` 兼容新旧 TiKV 指标名前缀，并用 `EstimateQPSFromCounts` 从两次累积计数计算速率。

本文件不负责获取任务列表、访问 metadata、采样 store、HTTP/TLS、等待采样间隔、表格/JSON 序列化或关闭连接；这些能力只存在于 Go 对照实现，Rust 当前没有对应 `StatusController`/`MaybeQPS` 主流程。

## 主要符号

- `pub const WildCard: &str = "*"`：与 Go 过滤约定同名的全任务标记；当前 Rust 文件内不消费它，也未发现 Rust 生产调用者。
- `CheckpointType::{Store, Global}`：checkpoint 的两种当前可表达作用域。默认值是 `Store`。
- `Checkpoint { pub ID, pub TS, kind }`：`ID` 与 `TS` 可读写，`kind` 私有，外部只能通过 `Type()` 读取。`Store(id, ts)` 保留 store ID；`Global(ts)` 强制 `ID = 0`。
- `Severity::{None, Error}` 与 `PauseV2 { Severity }`：仅保留状态判定所需的 Pause V2 子集。它没有 Go `PauseV2` 的操作者、时间、payload 类型和 payload 字段。
- `TaskStatus`：任务状态快照。`Info` 与 `LastErrors` 来自 `crate::stubs::backuppb`；`paused` 和 `globalCheckpoint` 为模块私有字段，其余主要展示字段公开。手写 `Clone` 会深拷贝向量、map、stub protobuf 和可选 Pause V2。
- `TaskPrinter`：两个变更方法组成的同步 trait，`AddTask` 按值取得状态，`PrintTasks` 刷新当前实现所持内容。
- `CollectTaskPrinter`：保存 `tasks` 和生成的 `lines`。`new`/`Default` 创建空容器；`AddTask` 只入队，`PrintTasks` 追加摘要行。
- `teeTaskPrinter<'a>` / `TeeTaskPrinter`：私有实现类型与公开工厂。对象在生命周期 `'a` 内独占借用调用者的 `Vec<TaskStatus>`，同时拥有 `Box<dyn TaskPrinter + 'a>`。
- `TaskStatus::onError`：私有错误判定，要求任务已暂停，并且 `LastErrors` 非空或 `PauseV2.Severity == Error`。
- `TaskStatus::StatusString`：返回静态字符串 `ERROR`、`PAUSE` 或 `NORMAL`。
- `TaskStatus::GetMinStoreCheckpoint`：选择展示进度用 checkpoint。
- `EstimateQPSFromCounts`：`elapsed_secs <= 0` 时返回零，否则以 `c1.wrapping_sub(c0) / elapsed_secs` 计算。
- `ParseLogBackupHandleKvBatchSum`：用惰性编译、进程内复用的正则提取第一处十进制计数，返回 `Option<u64>`。

## 执行流程

状态判定流程从 `StatusString` 进入。它先调用 `onError`；只有 `paused == true` 且有任一最后错误，或 Pause V2 明确标为 `Error`，才返回 `ERROR`。否则，暂停任务返回 `PAUSE`，未暂停任务即使残留错误或错误严重度也返回 `NORMAL`。这一顺序避免把未暂停的状态仅因附带旧错误信息而展示成错误。

checkpoint 选择从任务 `Info.GetStartTs()` 构造一个默认 store checkpoint，并用 `initialized` 区分“尚未见过 store”与“已选出 store”。遍历 `Checkpoints` 时，每个更小的 store TS 都替换当前值；遇到任何 global checkpoint 则立即返回，不再比较其前后元素。若没有可用 checkpoint，返回值的 `TS` 是任务起始 TS、`ID` 是零、类型仍为默认 `Store`。

打印流程中，调用者先把状态逐个交给 `AddTask`，再调用 `PrintTasks`。`CollectTaskPrinter` 对空队列追加 `No Task Yet.`；非空时先追加 `Total N Tasks.`，再按一开始编号生成 `name/status/qps` 行，QPS 固定保留两位。它不会清空 `tasks` 或 `lines`，所以重复打印会累加输出。tee 实现的 `AddTask` 先克隆到外部 `output`，再把原值交给内部打印器；`PrintTasks` 只转发。

metrics 辅助先以 `tikv_(stream|log_backup)_handle_kv_batch_sum ([0-9]+)` 搜索输入文本的第一处匹配，再解析捕获组为 `u64`。调用者应在外部完成两次抓取及时间测量，把样本交给 `EstimateQPSFromCounts`；本文件没有网络请求、睡眠或多 store 汇总。

## 数据与状态

`TaskStatus` 是一次性状态快照，不持有 metadata client 或 store 连接。`Info` 保存名称、起止 TS、table filter 和 storage URI 的 stub 表示；`Checkpoints` 保存进度候选；`globalCheckpoint` 与 `Checkpoints` 是两个独立字段，当前方法只读取后者；`QPS` 是外部填充的展示值；`LastErrors` 以 store ID 为键；`PauseV2` 只影响错误状态判定。

`Checkpoint.kind` 与构造器维护作用域不变量，但 `ID`、`TS` 仍可被调用者修改。global 构造时 ID 为零，不过外部之后可以改写公开 `ID`；`Type()` 仍以私有 `kind` 为准。默认 checkpoint 表示 `Store { ID: 0, TS: 0 }`，并不等价于 Go `streamhelper.Checkpoint::Type` 中按 `ID`、`Version`、`IsGlobal` 推导出的 task checkpoint。

`CollectTaskPrinter` 的 `tasks` 与 `lines` 都是可增长容器，没有自动 drain/reset。`teeTaskPrinter` 的外部向量是独占可变借用，因此 tee 活跃期间调用者不能并行或另行修改该向量。正则由 `LazyLock` 初始化一次；后续解析共享不可变 `Regex`。

## 依赖与调用关系

上游方面，`br/pkg/stream/lib.rs` 公开本模块及其符号。RustCodeGraph 的 `query` 精确定位了 Rust/Go 的 `TaskStatus`、`GetMinStoreCheckpoint`、`StatusString`，并将 QPS/metrics 函数的引用定位到 `stream_status_test.rs`；`callees` 显示 Rust `GetMinStoreCheckpoint` 调用 `Checkpoint::Type` 与 clone，`StatusString` 调用 `onError`。仓库级 Rust 文本核验只找到 `stream_status_test.rs` 和 `stream_misc_test.rs` 对本状态模型的直接使用，没有找到 Rust 生产入口调用 `TaskStatus`、`TeeTaskPrinter`、`StatusString`、QPS 或 metrics 辅助。

下游方面：

- `crate::stubs::backuppb::StreamBackupTaskInfo` 提供 `GetName`、`GetStartTs` 等 Go 风格 getter；`StreamBackupError` 提供错误码、消息与发生时间字段。两者是本 crate 的 Serde stub，而非 `kvproto` 真实生成类型。
- 标准库 `HashMap` 保存 store 错误，`Vec` 保存 checkpoint、任务与输出行，trait object 提供打印器动态分派。
- `regex::Regex` 是本文件唯一直接使用的第三方依赖；`regex = "1"` 由 `br/pkg/stream/Cargo.toml` 声明。
- Cargo manifest 虽声明 `astersql-br-pkg-streamhelper`，本文件没有复用其 Rust checkpoint/Pause 类型，而是定义了本地简化模型。

Go 主链为 `StatusController::PrintStatusOfTask -> getTask -> fillTask -> TaskStatus -> printToView -> TaskPrinter`；`fillTask` 读取暂停、Pause V2、checkpoint、storage checkpoint、最后错误与 QPS。该链只可作为语义对照，当前 Rust 文件没有这些上游边。

## 错误处理与边界

本文件的公开函数都不返回 `Result`。metrics 不匹配、捕获值超过 `u64`、或解析失败均折叠为 `None`；它不区分“指标缺失”与“数值非法”。正则要求指标名与数值之间恰有一个 ASCII 空格，但不锚定行首/行尾，所以可在一行中间匹配，并允许数值后继续出现文本。

`EstimateQPSFromCounts` 对零或负 elapsed 返回 `0.0`。计数下降时使用 `u64::wrapping_sub`，会得到模 2^64 的巨大差值；这与 Go 无符号减法回绕一致，也由测试固定。源文件顶部注释声称使用 `saturating_sub`，但函数实现与测试明确是 `wrapping_sub`，扩展时应以可执行实现和测试为准并修正注释漂移。

`GetMinStoreCheckpoint` 的 global 语义是“第一个遇到的 global 立即胜出”，不是选择最小或最后一个 global。空列表回退 StartTs；普通 store 可以为 ID 0；它忽略 `TaskStatus.globalCheckpoint` 字段。`StatusString` 不检查错误内容是否合法，只检查 map 是否非空；未暂停时任何错误证据均被忽略。

打印器不转义任务名，也不返回格式化错误。`CollectTaskPrinter::PrintTasks` 重复调用会重复总数与任务行；在空队列打印后再添加任务，旧的 `No Task Yet.` 仍保留。tee 先写外部输出再调用内部打印器；若未来 trait 改为可失败接口，需要明确两侧部分成功的回滚策略。

## 并发与资源生命周期

所有 API 都是同步的，本文件不创建线程、异步任务、锁、channel、HTTP client、timer 或取消令牌。`TaskPrinter` 需要 `&mut self`，同一实例的添加与打印由调用者串行化；trait 没有 `Send`/`Sync` 约束。`TaskStatus` 的 clone 会复制全部集合，tee 每接收一个任务都产生一次完整克隆，成本随 checkpoint 数、错误数和任务信息大小线性增长。

`teeTaskPrinter<'a>` 通过 Rust 借用保证 `output` 至少存活到打印器销毁，并防止活跃期间发生其他可变访问；内部 trait object 也被同一生命周期限制。`LazyLock<Regex>` 的初始化由标准库保证线程安全，此后只有无状态匹配。`CollectTaskPrinter` 的容器在实例销毁时释放，或由调用者显式清空；`PrintTasks` 不释放缓存。

Go `MaybeQPS` 会并发遍历存活 store、对每个 store 两次 HTTP 拉取并间隔一秒，然后通过 `sync.Map` 汇总；Rust 只保留纯解析/计算步骤。因此 Rust 当前没有 Go 主链的并发请求、错误注释、best-effort 汇总和连接关闭生命周期。

## 与 Go 版本的对应关系

Rust 的 `TaskStatus`、`TaskPrinter`、tee、`onError`、`StatusString`、`GetMinStoreCheckpoint`、metrics 正则和 QPS 差分，分别对应 `br/pkg/stream/stream_status.go` 的同名结构/函数或 `MaybeQPS` 内部逻辑。`stream_status_test.rs` 覆盖 tee 转发、两个指标前缀、三态优先级、global 优先及无符号回绕；`stream_misc_test.rs` 对照 Go `stream_misc_test.go::TestGetCheckpointOfTask` 覆盖多 store 最小 TS 的动态变化。

已验证的迁移差异包括：

- Go 文件还实现彩色表格、JSON 输出、Pause V2 展示、PD store 枚举、HTTP metrics 抓取、并发 QPS 汇总、`StatusController` 查询/关闭和 CLI 视图分发；Rust 均未移植，`CollectTaskPrinter` 只是测试友好的简化文本实现。
- Go `TaskStatus.Info`/`LastErrors` 使用真实 `kvproto` 类型，checkpoint 与 Pause V2 来自 `streamhelper`；Rust 使用本 crate 的 stub protobuf，并重新定义仅含 Store/Global 与 None/Error 的简化类型。
- Go checkpoint 模型还能表达 region、task 与 invalid，且 Pause V2 支持 manual 严重度、操作人、时间和 payload；Rust 无法表达这些状态，也不能复现 Go 表格/JSON 的完整字段。
- Go `MaybeQPS` 指标缺失时以魔数 `42` 作为单次计数，并吞掉单 store 抓取错误后汇总成功项；Rust parser 返回 `None`，由尚不存在的上游决定策略。
- Go `PrintTaskByTable`/`PrintTaskWithJSON` 已用于状态命令；Rust 未发现生产接线。因此这里是局部行为对齐，不是完整端到端替代。

## 扩展指南

- 接入 Rust `stream status` 命令时，应先补真实 metadata/PD/TiKV 边界与 `StatusController`，再把状态交给打印器；不要把网络、睡眠和连接关闭塞入 `TaskStatus` 的纯判定方法。
- 恢复 Go 完整 checkpoint 语义时，优先复用或补全 `astersql-br-pkg-streamhelper` 的 canonical 类型，明确 region/task/invalid 与 global 的判定，再同步 `GetMinStoreCheckpoint` 边界测试；避免让 `ID` 与私有 `kind` 出现相互矛盾的双重真相。
- 扩展 Pause V2 时，需要兼容 Go JSON 字段、manual/error 严重度、RFC3339 时间和 protobuf/text payload；`onError` 仍应只把 error 严重度视为错误暂停。
- 实现真实 QPS 采样时，保留 parser 与差分函数的可测试纯边界，但要决定 metrics 缺失、HTTP 错误、counter reset/回绕、非有限 elapsed 和多 store 部分失败的策略。若不再接受回绕，应同时修改现有测试，并评估与 Go 的兼容性。
- 添加打印格式时可实现新的 `TaskPrinter`，并在独立 `stream_status_test.rs` 验证空列表、重复打印、任务顺序、浮点格式和错误字段；不要把测试嵌入生产源文件。
- 若 tee 的内部打印可能失败，应把 trait 升级为返回错误，并明确先转发还是先记录、部分状态是否保留。目前的先 clone 后转发顺序是可观察 contract。
- 修改本文件行为应同步 `br/pkg/stream/stream_status_test.rs`；checkpoint 最小值还需同步 `stream_misc_test.rs`。对 Go 语义的变更需核验 `stream_status.go`、`stream_misc_test.go` 与 `br/pkg/streamhelper/client.go`。

## 验证依据

- RustCodeGraph：`status` 检查了现有索引；`files --filter br/pkg/stream` 确认文件被索引；`node --file br/pkg/stream/stream_status.rs --offset 1 --limit 500` 读取完整 267 行；`query TaskStatus/GetMinStoreCheckpoint/StatusString/EstimateQPSFromCounts/ParseLogBackupHandleKvBatchSum` 定位 Rust、Go 与测试符号；`callees` 验证 `GetMinStoreCheckpoint -> Checkpoint::Type/clone`、`StatusString -> onError`。批量 `callers` 查询未产生可用输出，故调用接线另以仓库文本核验。
- Rust 生产与 crate 边界：`br/pkg/stream/stream_status.rs`、`lib.rs`、`Cargo.toml`、`stubs.rs`；仓库级搜索确认其他生产 crate 使用该 stream crate 的 metadata/table history 能力，但未使用本状态模块 API。
- Rust 独立测试：`br/pkg/stream/stream_status_test.rs`、`stream_misc_test.rs`；覆盖 tee、指标解析、三态判定、store/global checkpoint、回绕差分及多 store 最小进度。
- Go 对照：`br/pkg/stream/stream_status.go`、`stream_misc_test.go`、`br/pkg/streamhelper/client.go`；前者提供完整状态命令主链，后两者固定 checkpoint 与 Pause V2 语义。
- 包目录没有 `br/pkg/stream/doc.go`。本任务仅新增说明文档，按计划不运行 Cargo；交付前以固定十一标题命令做结构验证，并人工复核当前接线与迁移缺口没有被描述成“已支持”。
