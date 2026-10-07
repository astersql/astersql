# `lightning/pkg/progress/progress.rs`

## 文件定位

本文件实现 Lightning 导入进度的进程内状态机，是 Go 包 [`progress.go`](./progress.go) 的 Rust 对齐版本。crate 入口 [`lib.rs`](./lib.rs) 将本模块的全部公开项重新导出；[`Cargo.toml`](./Cargo.toml) 声明 crate 名为 `astersql-lightning-pkg-progress`，只直接依赖已移植的 `astersql-lightning-pkg-checkpoints`、`serde` 和 `serde_json`。`common`、`mydump`、`errors` 并非真实 Lightning crate，而由 [`stubs.rs`](./stubs.rs) 提供本文件所需的最小边界。

在完整 Rust 应用中，CLI [`../../cmd/tidb-lightning/main.rs`](../../cmd/tidb-lightning/main.rs) 根据 `StatusAddr` 启用进度；server [`../server/lightning.rs`](../server/lightning.rs) 广播任务开始、初始化和结束，并把两个序列化入口暴露给 HTTP；importer [`../importer/import.rs`](../importer/import.rs) 也会建立任务生命周期、初始化表、上报表 checkpoint 和表错误。它不是持久化 checkpoint 的所有者，而是保存适合状态接口读取的深拷贝快照。

## 核心职责

- 通过 `EnableCurrentProgress` 建立全局进度槽位，并用 `PROGRESS_ENABLED` 控制所有入口是否生效。
- 用 `TaskProgressGuarded` 保存任务状态、任务错误以及每张表的总大小、已写入量、状态、错误和分步骤进度。
- 用 `CheckpointsMap` 保存按唯一表名索引的 `TableCheckpoint` 深拷贝；接收 `TableCheckpointDiff` 后应用差量并重新聚合已写入字节数。
- 生成两个互相独立的 JSON 视图：`MarshalTaskProgress` 返回任务/表摘要，`MarshalTableCheckpoints` 返回单表 checkpoint 详情。
- 保持 Go 版本的可观察契约，包括状态码 `0/1/2`、短 JSON 字段名、空消息省略、未启用错误、缺失 checkpoint 的 not-found 语义和相同 step 的原位更新。

本模块只描述当前进程中的最近状态，不负责进度持久化、恢复调度、HTTP 路由或导入工作本身；这些职责分别属于 checkpoints、importer 和 server。

## 主要符号

- `CheckpointsMap { checkpoints: RwLock<HashMap<String, TableCheckpoint>> }`：私有 checkpoint 快照容器。`clear` 替换整张 map；`insert` 写入深拷贝；`update` 应用差量并返回 `Vec<TotalWritten>`；`marshal` 读取并序列化指定表。
- `TotalWritten`：私有传递对象，把 checkpoint 聚合出的 `(表键, 已写入字节数)` 交给任务摘要更新。
- `type TaskStatus = u8`、`TASK_STATUS_RUNNING = 1`、`TASK_STATUS_COMPLETED = 2`：与 Go `taskStatus` 数值保持一致；初始值 `0` 表示尚未开始。
- `TableProgress`：单个阶段的 `step` 与浮点 `progress`；同名阶段不会重复追加。
- `TableInfo`：任务摘要中的表级数据，序列化为 `w`（已写）、`z`（总量）、`s`（状态）、可选 `m`（消息）和可选 `progresses`。
- `TaskProgressGuarded`：受任务级 `RwLock` 保护的 `Tables`、`Status`、`Message`。`Tables: None` 序列化为 `"t": null`，不同于空表对象。
- `TaskProgress`：组合任务级锁与拥有独立锁的 `CheckpointsMap`。
- `TaskProgressJson<'a>`：借用锁内状态的序列化投影，精确控制 `t/s/m` 字段及空消息省略规则。
- `CURRENT_PROGRESS: OnceLock<Mutex<Option<TaskProgress>>>`：静态槽位；`OnceLock` 初始化外层 `Mutex`，槽内 `Option` 可在测试或重新启用时替换。
- `PROGRESS_ENABLED: AtomicBool`：快速开关，使用 `SeqCst` 读写。广播入口在关闭时静默返回，查询入口在关闭时返回错误。
- 公开 API：`EnableCurrentProgress`、`BroadcastStartTask`、`BroadcastEndTask`、`BroadcastInitProgress`、`BroadcastTableCheckpoint`、`BroadcastTableProgress`、`BroadcastCheckpointDiff`、`BroadcastError`、`MarshalTaskProgress`、`MarshalTableCheckpoints`。
- `marshal_table_checkpoint`：手工构造 Go 导出字段外观的 checkpoint JSON，包含表、引擎、chunk、文件元数据和基值字段。
- `reset_progress_for_test`、`checkpoints_contains_for_test`：仅在 `cfg(test)` 下编译，为独立 [`parity_test.rs`](./parity_test.rs) 隔离全局状态和观察清理结果。

## 执行流程

1. 启用：`EnableCurrentProgress` 锁住全局槽位，放入初始 `TaskProgress`（`Tables=None`、状态 `0`、空消息、空 checkpoint map），随后把原子开关设为 `true`。Rust CLI 在配置状态地址时调用它；Rust importer 的 `Controller::Run` 也会主动调用。
2. 任务开始：`BroadcastStartTask` 把任务状态改为 `1`，然后清空上一个任务的 checkpoint 快照。它不会初始化表，也不会清除既有 `Tables` 或任务 `Message`；正常调用顺序需要随后用 `BroadcastInitProgress` 覆盖表 map，并最终由 `BroadcastEndTask` 覆盖消息。
3. 表初始化：`BroadcastInitProgress` 遍历 `MDDatabaseMeta.Tables`，用 `common::UniqueTable` 生成带反引号转义的键，为每张表建立零写入、零状态、空消息/阶段列表和给定 `TotalSize` 的 `TableInfo`，最后一次性替换 `Tables`。
4. 表 checkpoint：`BroadcastTableCheckpoint` 先把已存在表的状态设为 `1`，再调用 `TableCheckpoint::DeepCopy` 并把副本写入 `CheckpointsMap`，避免调用方后续修改原对象影响 HTTP 快照。
5. 阶段进度：`BroadcastTableProgress` 在线性列表中查找相同 `Step`；命中则更新全部同名项的数值，未命中才追加新项。正常状态下列表由本函数维护，因此同名项应保持唯一。
6. checkpoint 差量：`BroadcastCheckpointDiff` 让 `CheckpointsMap::update` 对每个键执行 `TableCheckpoint::Apply`。若引擎状态至少为 `CheckpointStatusAllWritten`，每个 chunk 按 `TotalSize()` 计数；否则按 `chunk.Chunk.Offset - chunk.Key.Offset` 计数。聚合结束后再锁住任务摘要，覆盖对应表的 `TotalWritten`。
7. 错误与结束：`BroadcastError` 只在表存在时将其置为状态 `2` 并写入 `ErrorStack`；未知表是 no-op。`BroadcastEndTask` 独立地把任务置为状态 `2` 并写入任务级错误文本，`None` 对应空字符串。
8. 查询：`MarshalTaskProgress` 在任务读锁内借用状态并交给 `serde_json::to_vec`；`MarshalTableCheckpoints` 在 checkpoint 读锁内查找表，并经 `marshal_table_checkpoint` 输出 Go 风格详情。server 的 `handleProgressTask` 和 `handleProgressTable` 将结果写入 HTTP 响应，后者把 `not_found` 映射为 HTTP 404。

## 数据与状态

任务和表都使用三态数值：初始 `0`、运行 `1`、完成 `2`。表的完成状态由 `BroadcastError` 设置，因此这里的“完成”并不区分成功与失败；正常成功完成的表级最终状态主要由其他 checkpoint 状态表达。任务级与表级消息互不覆盖，checkpoint 的 `TableCheckpoint.Status`、引擎状态和摘要里的 `TableInfo.Status` 也允许短暂不同步；[`parity_test.rs`](./parity_test.rs) 明确验证表 checkpoint 顶层仍为 `Loaded` 而引擎已为 `AllWritten` 的情况。

`Tables` 使用 `Option<HashMap<...>>` 保留 Go `nil map` 的 JSON 外观：刚启用时为 `null`，初始化空数据库切片后则是空对象。表键由 `UniqueTable(schema, table)` 生成，标识符中的反引号会加倍转义；调用方必须始终使用同一规范化键。

checkpoint map 存放完整深拷贝，而任务表 map 只存展示摘要。差量先修改 checkpoint 副本，再把聚合结果回填摘要，因此两张 map 中的键必须同时存在。`marshal_table_checkpoint` 当前显式输出 `Status`、`Engines`、`TableID`、`TableInfo: null`、空 `Checksum` 以及三个 base 字段；chunk 的 `Checksum` 也固定输出空对象。这是当前移植数据模型的可见边界，不应描述为完整复刻所有未来 checkpoint 字段。

## 依赖与调用关系

上游 Rust 生产调用关系（由源码引用补充 RustCodeGraph 未返回的公开函数 caller 边）：

- [`../../cmd/tidb-lightning/main.rs`](../../cmd/tidb-lightning/main.rs) → `EnableCurrentProgress`，仅在 `StatusAddr` 非空时启用。
- [`../server/lightning.rs`](../server/lightning.rs) → `BroadcastStartTask` → `BroadcastInitProgress` → `BroadcastEndTask`，并由 HTTP handler 调用两个 `Marshal*` 入口。
- [`../importer/import.rs`](../importer/import.rs) 的 `Controller::Run` → 启用/开始/结束；`initCheckpoint` → 初始化表；`importTables` → 表 checkpoint；`saveStatusCheckpoint` → 表错误。

`rg` 在 Rust 生产文件中未找到 `BroadcastTableProgress` 或 `BroadcastCheckpointDiff` 的调用；它们当前只由 [`parity_test.rs`](./parity_test.rs) 覆盖。对应 Go 主链仍在 [`../importer/table_import.go`](../importer/table_import.go) 上报阶段进度，并在 [`../importer/import.go`](../importer/import.go) 的 checkpoint 更新监听流程广播差量。因此这两个入口的 Rust 生产接线不能视为已完成。

下游依赖为：

- `astersql_lightning_pkg_checkpoints::{TableCheckpoint, TableCheckpointDiff, CheckpointStatusAllWritten}`：深拷贝、应用差量、计算 chunk 总大小及 checkpoint 数据结构。
- [`stubs.rs`](./stubs.rs) 的 `common::UniqueTable`、`mydump::{MDDatabaseMeta, MDTableMeta}`、`errors::{Error, Result, New, NotFoundf, ErrorStack}`：最小的命名、元数据和错误边界。
- `serde`/`serde_json`：摘要派生序列化和 checkpoint 手工 JSON 组装。
- 标准库 `HashMap`、`AtomicBool`、`Mutex`、`OnceLock`、`RwLock`：全局状态和并发保护。

RustCodeGraph 对本文件的内部边确认包括 `BroadcastTableCheckpoint → insert/current`、`BroadcastCheckpointDiff → update/current`、`MarshalTableCheckpoints → marshal/current`；公开 API 的上游 caller 查询未返回结果，因此上游边以上述源码文本引用为准。

## 错误处理与边界

- 所有广播函数在未启用时均静默 no-op；两个查询函数则返回 `"progress is not enabled"`，让 HTTP 层可见配置/生命周期错误。
- `CheckpointsMap::marshal` 对未知表返回 `errors::NotFoundf("table {key}")`。本地桩同时携带 `not_found=true`，server 据此返回 404。
- `BroadcastError` 容忍 `Tables=None` 或表键不存在，保持 best-effort；它与其他表级更新的严格前置条件不同。
- `BroadcastTableCheckpoint`、`BroadcastTableProgress`、`BroadcastCheckpointDiff` 假定表已经初始化且键存在；checkpoint 差量还假定对应 checkpoint 已插入。违反顺序会在 `expect` 处 panic，而不是返回可恢复错误。
- 所有锁都通过 `unwrap` 获取；线程持锁 panic 导致 poison 后，后续访问也会 panic。序列化错误被压缩为仅含 `to_string()` 的本地 `Error`。
- `progress: f64` 没有范围检查；调用方可以写入小于 0、大于 1 或非有限值。非有限浮点可能使 JSON 序列化失败，此时错误从 `MarshalTaskProgress` 返回。
- 已写入量没有饱和或非负检查；未完成引擎直接计算 offset 差，输入 checkpoint 不变量若被破坏，摘要也可能为负数。
- 手工 checkpoint JSON 必须随 checkpoints 数据模型和 Go `encoding/json` 外观同步；新增字段不会自动出现。

## 并发与资源生命周期

生命周期是“进程全局槽位 → 单次启用/替换 → 多次广播和读取”。`OnceLock` 只保证外层 `Mutex<Option<_>>` 初始化一次；`EnableCurrentProgress` 仍会替换槽中的整个 `TaskProgress`。源码注释沿用 Go 约束，要求在进度开始前只初始化一次。Rust 的外层 `Mutex` 使替换动作本身免于数据竞争，但重复启用会丢弃旧快照，不能当作普通并发更新操作。

每个公开操作先检查原子开关，再锁住全局槽位并在函数余下时间持有该锁；随后按需取得任务或 checkpoint 的 `RwLock`。因此当前锁顺序统一为“全局槽位 → 内部锁”。`BroadcastCheckpointDiff` 先取得 checkpoint 写锁（在 `update` 返回前释放），再取得任务写锁；没有同时持有两把内部锁。任务 JSON 和单表 checkpoint JSON 分别受不同读锁保护，所以各自是自洽快照，但两次独立 HTTP 查询之间不保证跨结构原子一致。

Go 注释说明 checkpoint 写入来自单个 `listenCheckpointUpdates` goroutine、HTTP goroutine 可并发读取，因此单一 `RWMutex` 足够；Rust 保留同一粒度。不过当前 Rust 主链尚未检索到 `BroadcastCheckpointDiff` 的生产调用，不能据此断言 Rust 监听流程已完整复刻。

`BroadcastStartTask` 清空 checkpoint map，测试也验证旧表查询随后变为 not-found。生产代码没有禁用或销毁 API；进程结束前全局槽位常驻。`reset_progress_for_test` 才会把开关设回 false 并清空槽位，且只在测试构建中存在。

## 与 Go 版本的对应关系

结构和主流程逐项对应 [`progress.go`](./progress.go)：`checkpointsMap`、`totalWritten`、`taskStatus`、`tableProgress`、`tableInfo`、`taskProgress` 以及十个公开函数均有同名或直译实现。状态值、表名生成、checkpoint 深拷贝、差量聚合公式、错误文本入口和摘要 JSON 字段保持一致。

关键实现差异如下：

- Go 使用裸全局指针加 `atomic.Bool`，并明确 `EnableCurrentProgress` 非线程安全；Rust 使用 `OnceLock<Mutex<Option<TaskProgress>>>` 包装槽位，避免直接可变静态，但仍要求生命周期开始时一次性初始化。
- Go 的 `Tables` 是 `map[string]*tableInfo`，Rust 用 `Option<HashMap<String, TableInfo>>` 表达 nil，并通过借用视图保持 `"t": null`。
- Go 直接 `json.Marshal(TableCheckpoint)`；Rust checkpoints 类型的 serde 外观不直接等价，因此 `marshal_table_checkpoint` 手工选择字段。这是最容易产生兼容漂移的位置。
- Go 依赖真实 `common`、`mydump` 和 `pingcap/errors`；Rust crate 当前依赖本地桩，只保留本模块使用的字段/行为，错误堆栈也只返回消息文本。
- Go 的生产 importer 已调用阶段进度和 checkpoint 差量广播；Rust 当前生产引用搜索没有这两类调用，只有对齐测试覆盖 API 行为。

Go 同目录没有 `progress_test.go`；最接近的 Go 行为证据来自 [`../importer/table_import_test.go`](../importer/table_import_test.go)（初始化与 checkpoint 上报）和 [`../server/lightning_server_serial_test.go`](../server/lightning_server_serial_test.go)（用 `sync.Once` 启用全局进度）。Rust 的直接契约测试集中在独立 [`parity_test.rs`](./parity_test.rs)，没有把测试嵌入生产源文件。

## 扩展指南

- 新增任务/表摘要字段时，修改 `TaskProgressGuarded`、`TableInfo` 或 `TaskProgressJson`，同步核对 Go JSON tag，并扩展 [`parity_test.rs`](./parity_test.rs) 的 JSON 断言。必须决定零值是省略、`null` 还是显式值。
- 扩展 checkpoint 输出时修改 `marshal_table_checkpoint`，逐字段对照 Go `json.Marshal` 的字段名、嵌套、空值和数字类型；同步覆盖引擎/chunk 和新增字段，避免只让 Rust 内部类型编译通过。
- 修改写入量算法时从 `CheckpointsMap::update` 接入，同时覆盖 `AllWritten` 的 `TotalSize()` 分支和未完成的 offset 差分支；还应验证多引擎、多 chunk、缺失键和异常 offset 的政策。
- 新增广播入口前先明确调用顺序与缺失表策略：当前 `BroadcastError` 宽容，而 checkpoint、阶段和差量入口会 panic。若要改为返回错误，这是对公开契约和调用方签名的变更，需同步 Go 对照或明确记录偏差。
- 补齐 Rust 生产进度接线时，最可能修改 importer 的 checkpoint 更新监听和 table import 阶段；应调用现有 `BroadcastCheckpointDiff`/`BroadcastTableProgress`，而不是在调用方复制聚合逻辑。
- 并发模型变更必须维持固定锁顺序，避免同时持有 checkpoint 与任务写锁；若确认 HTTP 读取出现竞争，再考虑分片 map，不能使用无法保护值内部变更的等价 `sync.Map` 方案。
- 测试应继续放在独立 [`parity_test.rs`](./parity_test.rs) 或相邻独立测试文件中，不放入 `progress.rs`。至少覆盖正常状态流、未启用/缺失表、错误传播、清理、JSON 外观和新增生产接线。
- 兼容风险主要是 JSON 外观和错误分类；正确性风险主要是初始化顺序、键不一致和差量基线；性能风险主要是全局槽位锁让所有读取/广播串行进入，以及单 checkpoint map 写锁遍历全部差量。

## 验证依据

- RustCodeGraph 状态：索引覆盖 11,467 个文件，其中 Rust 7,032 个；目标目录索引到 `progress.rs`、`lib.rs`、`stubs.rs`、`parity_test.rs` 和 `progress.go`。
- RustCodeGraph 符号图：目标文件列出 28 个符号；精确查询确认 Rust/Go 两套同名公开函数；内部 callees 查询确认 `BroadcastTableCheckpoint → CheckpointsMap::insert/current`、`BroadcastCheckpointDiff → CheckpointsMap::update/current`、`MarshalTableCheckpoints → CheckpointsMap::marshal/current`。公开函数 caller 查询无结果，因此额外用源码引用搜索核对上游。
- 已读生产与边界文件：[`progress.rs`](./progress.rs)、[`lib.rs`](./lib.rs)、[`stubs.rs`](./stubs.rs)、[`Cargo.toml`](./Cargo.toml)、[`progress.go`](./progress.go)、[`../../cmd/tidb-lightning/main.rs`](../../cmd/tidb-lightning/main.rs)、[`../server/lightning.rs`](../server/lightning.rs)、[`../importer/import.rs`](../importer/import.rs)，以及 Go importer 的 [`../importer/table_import.go`](../importer/table_import.go)。
- 已读测试：Rust [`parity_test.rs`](./parity_test.rs)；Go [`../importer/table_import_test.go`](../importer/table_import_test.go) 与 [`../server/lightning_server_serial_test.go`](../server/lightning_server_serial_test.go)。直接 Rust 测试验证正常状态推进、step 去重、两种写入量公式、Go 风格 checkpoint JSON、未启用错误、空表状态、not-found、错误传播、checkpoint 清理和表名转义。
- Cargo 依赖引用搜索确认 Rust CLI、server、importer 三个 crate 直接依赖本 crate；Rust 源引用搜索同时确认 `BroadcastTableProgress`、`BroadcastCheckpointDiff` 尚无生产调用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务规定的 11 章节结构命令，并人工检查本文只描述可由上述符号、调用点、配置或测试复核的事实。
