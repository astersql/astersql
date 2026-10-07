# `br/pkg/streamhelper/client.rs`

## 文件定位

源文件：[client.rs](client.rs)。`client.rs` 是 `astersql-br-pkg-streamhelper` crate 的日志备份元数据客户端实现。crate 入口 [lib.rs](lib.rs) 以 `pub mod client` 装载本文件，并通过 `pub use client::*` 将其公开类型和函数提升为包级 API。该 crate 的 [Cargo.toml](Cargo.toml) 将库入口设为 `lib.rs`，直接依赖 `serde`、`serde_json`、`regex` 和 `uuid`；本文件实际直接使用前两者完成任务与暂停记录的 JSON 编解码。

文件位于日志备份 checkpoint 推进链的元数据边界：上层通过 `MetaDataClient` 创建、查询、暂停、恢复和删除任务，通过 `Task` 读取范围、检查点和末次错误；下层不是直接绑定 etcd SDK，而是依赖 `stubs.rs` 中的 `EtcdKV: Send + Sync`。生产适配器和测试内存实现都必须遵守这个接口。本文件不是单纯门面：任务键空间的读写顺序、检查点解析、全局水位计算和暂停载荷协议都在这里实现。

## 核心职责

1. 定义暂停元数据协议：`RFC3339Time`、`PauseV2`、`PausePayload`、`NewLocalPauseV2`、`PauseWithMessage` 与 `PauseWithErrorSeverity` 负责构造、序列化、解析和展示暂停原因。
2. 定义 checkpoint 解释规则：`Checkpoint` 与 `CheckpointType` 根据 ID、epoch 和全局标志区分 Task、Store、Region、Global；`ParseCheckpoint` 把 etcd 键和值还原为该模型。
3. 封装任务元数据 CRUD：`MetaDataClient` 使用 `TaskOf`、`RangesOf`、`Pause`、`CheckPointsOf`、`LastErrorPrefixOf`、`GlobalCheckpointOf`、`StorageCheckpointOf` 等键生成函数操作完整任务键空间。
4. 提供远端任务句柄：`Task` 绑定任务静态信息和客户端，可继续执行暂停/恢复、读取范围、汇总检查点、上传全局 checkpoint 和读取各 store 末次错误。
5. 保留 Go 兼容数据：旧版空 Pause 值仍代表“已暂停但没有 V2 详情”；旧版无类型 checkpoint 后缀仍按 8 字节大端 store ID 解析；range 起始键保持任意二进制字节。

## 主要符号

- `SeverityError` / `SeverityManual`：暂停严重级别的稳定字符串协议，分别为 `ERROR` 和 `MANUAL`。
- `RFC3339Time(String)`：Pause JSON 中的时间字符串。`now` 以系统 Unix 秒生成 UTC 秒精度文本；`Parse`/`Deserialize` 校验日期、时间、可选小数秒和 `Z` 或数值时区；`Display` 原样输出保存的字符串。
- `PauseV2`：包含严重级别、主机、PID、操作时间、MIME 风格 `PayloadType` 和二进制 `Payload`。serde 将主机、PID、时间字段映射为 Go 兼容的 snake_case JSON 名。
- `PausePayload`：解码后的和类型，仅允许 `Text(String)` 或 `StreamErr(StreamBackupError)`。
- `PauseV2::{GetPayload, DisplayTable, SetTextMessage, SetBakcupStreamError}`：负责 MIME 分派、CLI 键值展示以及两种合法 payload 的编码。`SetBakcupStreamError` 的拼写刻意与 Go API 保持一致。
- `Checkpoint { ID, Version, TS, IsGlobal }` 与 `CheckpointType`：描述 Store、Region、Task、Global 四类 checkpoint；不能匹配的字段组合归为 `Invalid`。
- `ParseCheckpoint(task, key, value)`：校验任务 checkpoint 前缀，解析 `store/<id>`、`region/<id>/<epoch>`、`global` 或旧版 8 字节 store ID 后缀，并要求 value 恰为 8 字节大端 TSO。
- `PauseTaskOption = Box<dyn FnMut(&mut PauseV2) + Send>`：暂停前修改记录的闭包扩展点。
- `MetaDataClient { KV: Arc<dyn EtcdKV> }`：可克隆的 KV 客户端包装；`NewMetaDataClient` 是构造入口。
- `MetaDataClient::{PutTask, DeleteTask, PauseTask, ResumeTask, CleanLastErrorOfTask, GetTask, GetTaskWithPauseStatus, TaskByInfo, GetAllTasksWithRevision, GetAllTasks, GetTaskCount}`：任务元数据的写、删、暂停状态和列表 API。
- `Task { cli, Info }`：已加载任务句柄。`Pause`/`Resume` 转发写操作；`GetPauseV2`/`IsPaused` 区分详情和值存在性；`Ranges`、`NextBackupTSList`、`GetStorageCheckpoint`、`GetGlobalCheckPointTS`、`UploadGlobalCheckpoint`、`LastError` 读取或更新附属元数据。

## 执行流程

创建任务时，`MetaDataClient::PutTask` 先用 `serde_json::to_vec` 编码 `StreamBackupTaskInfo`，再逐个调用 `EtcdKV::PutBytes` 写入 `RangeKeyOf(task, StartKey) -> EndKey`，随后写 `TaskOf(task)`。若 `TaskInfo::Pausing` 为真，最后再写一个空的 Pause 值。先 range、后 task 的顺序保证任务 watch 观察到新增任务时范围已可见；但这只是可见性排序，不是事务原子性。

读取任务时，`GetTask` 读取并反序列化 `TaskOf`，再由 `TaskByInfo` 绑定客户端。`GetAllTasksWithRevision` 对 `PrefixOfTask` 做带 revision 的线性化前缀扫描，将每条值反序列化为 `Task`，并把同一次扫描的 revision 返回给 `advancer_cliext`，以便后续 watch 从正确位置衔接。`GetTaskWithPauseStatus` 先读任务，再用 `GetWithRevision(Pause(...))` 按键是否存在判断暂停状态。

暂停时，`PauseTask` 以 `NewLocalPauseV2` 生成 MANUAL 记录，依次执行所有 `PauseTaskOption`，序列化后覆盖 Pause 键。`PauseWithMessage` 写入 UTF-8 文本；调用方也可用 `PauseWithErrorSeverity` 和 `SetBakcupStreamError` 形成 ERROR protobuf 载荷。恢复只删除 Pause 键。`Task::GetPauseV2` 对空值返回 `None`，从而兼容 `PutTask(Pausing=true)` 产生的旧版空标记；`Task::IsPaused` 则只看键是否存在，因此空标记仍是暂停状态。

检查点读取由 `Task::NextBackupTSList` 扫描任务 checkpoint 前缀，并逐条交给 `ParseCheckpoint`。`GetGlobalCheckPointTS` 若看到 Global 类型立即返回其 TS；否则取所有 Store checkpoint 的最小值作为保守水位，再与 `GetStorageCheckpoint` 返回的 storage checkpoint 最大值取最大值。`GetStorageCheckpoint` 在无记录时从 `Info.StartTs` 起步。`UploadGlobalCheckpoint` 将 u64 以 8 字节大端形式写入 central-global 键。

删除任务时，`DeleteTask` 依次删除任务键、range 前缀、Pause 键、checkpoint 前缀、last-error 前缀、全局 checkpoint 键和 storage-checkpoint 前缀。`LastError` 扫描 last-error 前缀，把键后缀解析为 store ID，再反序列化每个 `StreamBackupError`。

## 数据与状态

`MetaDataClient` 自身只持有一个 `Arc<dyn EtcdKV>`，没有本地缓存；权威状态位于 KV 键空间。`Task` 的 `Info` 是读取时的静态快照，而 `Ranges`、Pause、checkpoint 和 last-error 每次从 KV 重新读取，因此可能比 `Info` 更新。

关键键空间由 `models.rs` 统一生成：任务定义位于 `TaskOf(name)`，范围位于 `RangesOf(name)` 前缀下，暂停状态位于 `Pause(name)`，普通和 central-global checkpoint 位于 `CheckPointsOf(name)` 下，外部存储 checkpoint 与末次错误各有独立前缀。`RangeKeyOf` 直接把原始 `StartKey` 字节追加到前缀，不能经 UTF-8 字符串转换；`models_test.rs::put_task_writes_binary_range_key_without_utf8_conversion` 对此有回归断言。

所有 checkpoint TS、旧版 store ID 和上传的全局 checkpoint 都采用 8 字节大端编码。Store checkpoint 用 `ID != 0, Version == 0` 表示；Region checkpoint 要求 ID 和 Version 都非零；Task checkpoint 两者均为零；`IsGlobal` 优先于其他字段。`GetGlobalCheckPointTS` 的 Store 最小值体现“所有 store 均已完成”的安全水位，而 storage checkpoint 最大值用于覆盖已持久化推进结果。

Pause V2 使用 JSON 外壳。文本 payload 的类型为 `text/plain;charset=UTF-8`；结构化错误的类型为 `application/x-protobuf;messagetype=brpb.StreamBackupError`，payload 字节交给 `StreamBackupError::{Marshal, Unmarshal}`。旧版空 Pause 值只表达状态，不包含可解析详情。

## 依赖与调用关系

下游依赖包括：

- `models.rs`：提供所有任务、range、暂停、checkpoint 与错误键的编码，以及 `TaskInfo`。
- `stubs.rs::EtcdKV`：提供 Put/Get/Delete、前缀扫描、revision 与 watch 边界；本文件要求该 trait 为 `Send + Sync`，并用 `Arc` 共享。
- `stubs.rs::{StreamBackupTaskInfo, StreamBackupError, KeyRange}`：任务协议、暂停/末错载荷和范围数据模型。
- `serde` / `serde_json`：RFC3339Time、PauseV2、Checkpoint 和任务信息的序列化边界。
- 标准库的 `SystemTime`、`Arc` 与 `HashMap`：时间生成、共享客户端和 store-error 映射。

RustCodeGraph 显示 `NewMetaDataClient` 的 Rust 调用者包括 `advancer_cliext_test.rs`、`integration_test.rs`、`models_test.rs`、`parity_test.rs` 等；`GetAllTasksWithRevision` 被 `advancer_cliext` 的初始任务快照路径使用；`ParseCheckpoint` 被 `Task::NextBackupTSList` 调用。`lib.rs` 的包级再导出使 `br/cmd/br/stream.rs` 等 crate 使用者可以直接获得这些符号。Go 调用图还表明同一 API 服务于 `br/pkg/task/stream.go` 的启动、暂停、恢复、停止与状态查询，以及 Lightning/PITR 冲突检查；这些是语义对照证据，不等同于已验证 Rust 生产调用全部完成接线。

## 错误处理与边界

本文件统一返回 `Result<_, String>`，下层 KV、serde 和协议解码错误通常转为字符串并立即传播。与 Go 的结构化 `errors.Annotate` 相比，Rust 版本不保留错误类型或调用栈，也较少附加任务名上下文；调用方不能依赖可判别的错误类别。

`ParseCheckpoint` 拒绝错误任务前缀、store/region 分段数不匹配、非十进制 ID/epoch、旧版后缀非 8 字节以及 value 非 8 字节。Global 分支只设置 `IsGlobal`，当前没有额外检查多余分段；扩展键格式时必须避免让未知文本首段落入旧版二进制分支并改变兼容错误行为。

`RFC3339Time::Parse` 校验日历日期、时分秒和时区形状，允许小数秒，但不会把时区归一化；`now` 只生成秒精度 UTC。系统时间早于 Unix epoch 时 `unwrap_or_default` 会回退 epoch，而不是报错。`hostname` 只读取 `HOSTNAME` 环境变量，缺失时使用 `localhost`，与 Go `os.Hostname()` 失败后写入错误文本并不完全相同。

`PauseV2::GetPayload` 只支持 text/plain 与指定 message type 的 x-protobuf。文本使用 `from_utf8_lossy`，非法 UTF-8 会被替换而不是报错；MIME 参数解析是本地轻量实现，不具备 Go `mime.ParseMediaType` 的全部语法能力。空 Pause 值不能反序列化为 PauseV2，调用方必须通过 `GetPauseV2 -> None` 与 `IsPaused -> true` 的组合识别旧格式。

`GetTask` 把空字节视为不存在；这依赖 `EtcdKV::Get` 无法区分缺键与空值的接口语义。`Ranges` 会再次检查每个结果键确实带前缀。`GetStorageCheckpoint`、`UploadGlobalCheckpoint` 和 `LastError` 均对大端长度、store ID 后缀或错误载荷解码失败立即返回，可能得到部分工作已发生但整个调用报错的状态。

## 并发与资源生命周期

`MetaDataClient` 和 `Task` 都可克隆；共享资源是 `Arc<dyn EtcdKV>`，线程安全责任由 `EtcdKV: Send + Sync` 及其实现承担。本文件不创建线程、异步任务、锁、channel 或 watcher，也不持有需要显式关闭的网络句柄。revision/watch 生命周期由 `advancer_cliext.rs` 和底层 `EtcdKV` 实现管理。

与 Go 版本的重要并发差异是事务性。Go `PutTask` 在一个 etcd transaction 中提交 task、ranges 和可选 Pause，`DeleteTask` 也在单事务中清理全部键；Rust `EtcdKV` 当前没有通用事务接口，因此两者都是多次顺序调用。中途失败会留下部分写入或部分删除。`PutTask` 通过先写 ranges、后发布 task 降低 watcher 看到不完整范围的概率，但无法提供回滚；`DeleteTask` 也不保证全有或全无。

`PauseTaskOption` 要求 `Send`，但其执行发生在当前调用线程，按 vector 顺序串行修改同一个局部 `PauseV2`。后一个 option 可以覆盖前一个 option 设置的字段。`GetAllTasksWithRevision` 返回的 revision 是建立“先快照、后 watch”一致性链条的资源；调用方必须原样使用，不能重新取一个不相关 revision。

## 与 Go 版本的对应关系

Rust 文件按 `br/pkg/streamhelper/client.go` 的公开模型和方法逐项移植：严重级别字符串、PauseV2 JSON 字段名与 MIME 值、`SetBakcupStreamError` 的历史拼写、checkpoint 类型推断、键格式、任务 CRUD、Task 便捷方法以及全局 checkpoint 汇总规则均有直接对应。`parity_test.rs::go_rust_public_contract_matches` 和 `task_metadata_methods_match_go_contract` 覆盖 store/global 解析、文本/错误 payload、任务写读、暂停恢复、范围、storage/global checkpoint、last-error 和删除清理。

当前实现存在必须如实保留的移植差异：

- Go 的 `StreamBackupTaskInfo` 与 `StreamBackupError` 使用 protobuf；Rust 任务路径在本文件中用 serde JSON，错误 payload 则委托本地 `Marshal`/`Unmarshal` 抽象。
- Go 客户端直接嵌入 `clientv3.Client`，使用事务、分页扫描、Context 和带 watcher mutex 的 reset；Rust 使用 `EtcdKV` trait，本文件没有 Context、分页参数、watcher 锁或 transaction。
- Go `GetTaskWithPauseStatus` 在同一事务快照读取 task 与 Pause；Rust 分两次读取，二者之间状态可能变化。
- Go `GetTaskCount` 用只取一项的分页扫描获取计数行为；Rust 读取并反序列化全部任务后取长度，任务多时成本更高且任一坏值会导致计数失败。
- Go `RFC3339Time` 基于 `time.Time`；Rust 保存原始字符串并自行校验。Go 通过 `os.Hostname()` 获取系统主机名；Rust 只读 `HOSTNAME`。
- Go 扫描范围与 checkpoint 使用分页器；Rust 假设 `GetPrefix` 一次返回全部条目，内存与响应规模边界由底层实现决定。

因此本文件应视为对 Go 行为契约的本地抽象移植，而不是 etcd 客户端实现细节的完全等价替换。

## 扩展指南

新增任务附属元数据时，应先在 `models.rs` 定义稳定且不与现有前缀重叠的键函数，再同时评估 `PutTask`、`DeleteTask`、快照/watch 事件和 Go 对照是否需要接线。清理类扩展必须加入 `DeleteTask`，否则停任务会遗留状态；涉及二进制起始键时必须使用 `PutBytes` 路径。对应测试应放在独立文件，优先扩展 `client_test.rs`、`models_test.rs`、`parity_test.rs` 或 `integration_test.rs`，不要把测试内嵌进 `client.rs`。

增加 Pause payload 类型时，应在 `PausePayload`、`GetPayload`、设置方法和 `DisplayTable` 中同时增加分支，并保持 MIME type/message type 的跨语言稳定性。需要补充 malformed MIME、未知 message type、编解码失败和 CLI 展示测试，同时核对 `client.go` 是否已有或需要同步协议；旧版空 Pause 的状态语义不可破坏。

扩展 checkpoint 格式时，应同步修改 `CheckpointType`、`Checkpoint::Type`、`ParseCheckpoint` 和 `models.rs` 的键编码，并覆盖前缀、分段、数值溢出、8 字节边界及旧版 store-ID 回退。全局水位算法的修改必须明确安全语义：Store 取最小、storage 取最大、central-global 优先是当前不变量，错误调整可能导致数据尚未备份就推进水位。

若要提高生产等价性，最优先的接入点是扩充 `EtcdKV` 的事务/一致性读取能力，再让 `PutTask`、`DeleteTask` 和 `GetTaskWithPauseStatus` 使用原子操作；其次是为大前缀扫描加入分页或流式接口。此类修改会影响 `advancer_cliext` 的快照/watch 交接，必须同步验证 revision 不丢事件，并评估大量任务/range/checkpoint 下的内存与延迟。

## 验证依据

- 目标源码：`br/pkg/streamhelper/client.rs`（674 行，RustCodeGraph 报告 66 个符号）；核对了全部常量、类型、函数、impl 和无条件编译路径，文件内没有测试模块或 feature 条件分支。
- crate 边界：`br/pkg/streamhelper/Cargo.toml` 与 `br/pkg/streamhelper/lib.rs`；确认 crate 名、`lib.rs` 入口、`client` 模块和包级再导出。
- 直接模型/KV 证据：RustCodeGraph `node EtcdKV`、`node models.rs::CheckPointsOf`、`node models.rs::RangeKeyOf`；确认线程安全 trait、revision/前缀 API、尾斜杠和二进制 range 键约束。
- 调用图证据：RustCodeGraph `explore "br/pkg/streamhelper/client.rs MetaDataClient PauseV2 Checkpoint"`；确认 `NewMetaDataClient`、`GetAllTasksWithRevision`、`ParseCheckpoint`、Pause 方法在 advancer、集成和契约测试中的上下游关系。精确 `callers/callees` 子命令在本次索引上未输出额外结果，因此调用边又以 `node` 的 Trail 和源码入口交叉核对。
- Go 对照：`br/pkg/streamhelper/client.go` 全文件；逐项核对 PauseV2、checkpoint 解析、事务 CRUD、Task 方法、storage/global checkpoint 与 last-error 语义，并记录事务、Context、分页、protobuf/JSON 和 hostname/time 差异。
- 独立 Rust 测试：`br/pkg/streamhelper/client_test.rs` 验证旧版空 Pause；`models_test.rs` 验证二进制 range key；`parity_test.rs` 验证公开契约与 Task 方法；`integration_test.rs` 验证 CRUD、暂停/恢复、删除清理、检查点和 StreamBackupError payload。相关 Go 测试语义由同路径 Go 实现及 Rust 测试中的 Go-equivalent 注释交叉确认。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含规定的 11 个二级标题，并人工复核重要结论均能回指上述符号或文件。
