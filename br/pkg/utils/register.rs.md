# `br/pkg/utils/register.rs`

## 文件定位

[`register.rs`](./register.rs) 属于 `astersql-br-pkg-utils` crate；[`lib.rs`](./lib.rs) 以 `pub mod register` 暴露该模块，`Cargo.toml` 将该 crate 标记为 Go 包 `br/pkg/utils` 的 Rust 移植库。它是 BRIE 任务互斥注册的策略层：用带 lease 的 etcd key 表示正在运行的 Restore、Lightning 或 Import Into 任务，并负责续租、断流恢复、撤销和有效任务枚举。

该文件不直接依赖具体 etcd SDK。`EtcdRegisterClient` 把所需 I/O 收敛为七个同步操作；生产侧由 [`metadata_register.rs`](./metadata_register.rs) 的 `MetadataRegisterClient` 接到 `astersql-metaservice::NamespacedEtcdClient`，测试侧由 [`register_test.rs`](./register_test.rs) 的 `MemEtcd` 实现。当前 Rust 生产调用链已接入 `lightning/pkg/importer/import.rs::registerTaskToPD`，因此本文件不是未接线门面或桩。

注册 key 固定为 `/tidb/brie/import/<type>/<task-name>`。该命名空间用于让导入/恢复入口发现互斥任务；key 的 value 始终为空，存在性、绑定 lease 和 lease TTL 才是协议数据。

## 核心职责

- 用 `RegisterTaskType` 稳定生成 `restore`、`lightning`、`import-into` 三类路径段，并由 `NewTaskRegisterWithTTL`/`NewTaskRegister` 构造隐藏实现 `TaskRegisterImpl`。
- 通过 `TaskRegister::RegisterTask` 完成 grant、put、创建 keepalive 流，并在后台维持长任务的租约。
- 当 keepalive 通道断开时，根据自最后一次回执以来的时间判断能否复用当前 lease；租约接近过期时重新 grant，并重新 put key 绑定新 lease，然后重建 keepalive 流。
- 通过 `TaskRegister::RegisterTaskOnce` 支持由外部周期调度的短调用：key 不存在时创建 lease/key，已存在时复用首条记录的 lease 并执行单次续租。
- 通过 `Close` 取消注册器自己的子上下文、等待后台线程结束并撤销当前 lease，保证任务 key 随生命周期结束而清理。
- 通过 `GetImportTasksFrom` 前缀扫描仍有正 TTL 的任务，生成面向冲突检查与 CLI 提示的 `RegisterTasksList`。
- 提供进程内 failpoint 表，使独立 Rust 测试可以确定性覆盖 grant 失败、re-put 失败、keepalive 中断和重试间隔，而不把测试逻辑嵌入生产测试模块。

本文件只负责注册状态机和抽象数据模型；真实连接、keyspace namespace、RPC worker 与底层 keepalive 实现属于 `metadata_register.rs`/`astersql-metaservice`。

## 主要符号

- `RegisterTaskType::{RegisterRestore, RegisterLightning, RegisterImportInto}` 与 `as_str`：公开任务分类及稳定路径编码。不同于 Go `String()` 的兜底 `"default"`，Rust enum 是封闭集合，所有变体都必须显式匹配。
- `RegisterImportTaskPrefix`：公开根前缀 `/tidb/brie/import`。`RegisterRetryInternal` 是断流恢复默认 10 秒重试间隔；私有 `defaultTaskRegisterTTL` 是默认 3 分钟；`NO_LEASE` 用 `0` 表示尚无可撤销 lease。
- `KeyValue`、`LeaseGrantResponse`、`GetResponse`、`LeaseKeepAliveResponse`、`LeaseTimeToLiveResponse`：与状态机所需的 etcd 数据对应的精简结构。它们避免将具体 SDK 类型泄漏到策略层，也供测试替身和生产适配器共同实现协议。
- `LeaseNotFound`：类型化特殊错误，显示文本保持为 `etcdserver: requested lease not found`。`Close` 可 downcast 后把已不存在的 lease 当作幂等成功。
- `EtcdRegisterClient: Send + Sync`：I/O 抽象，定义 `put`、`grant`、`keep_alive`、`keep_alive_once`、`get`、`revoke`、`time_to_live`。接口没有事务语义；状态机依靠顺序调用和 lease 绑定维护可见性。
- `TaskRegister: Send + Sync`：对外生命周期接口，包含 `Close(&mut self, ...)`、`RegisterTask(&mut self, Context)`、`RegisterTaskOnce(&mut self, ...)`。Go 注释中的重要契约仍适用：不要在同一实例上混用持续注册和一次性注册。
- `TaskRegisterImpl`：私有实现，保存客户端、原始 TTL、秒数 TTL、完整 key、共享当前 lease、可取消子上下文及后台线程计数。
- `NewTaskRegisterWithTTL`：按前缀、类型和任务名拼 key，初始化 `NO_LEASE`，返回 `Box<dyn TaskRegister>`；`NewTaskRegister` 只是在此基础上选择 3 分钟 TTL。
- `TaskRegisterImpl::{grant, keepalive_loop, sleep_retry_interval}`：分别处理 grant 响应的双重失败通道、断流恢复状态机和可由 failpoint 缩短的睡眠。
- `RegisterTask` 与 `RegisterTasksList`：用户可见的任务快照。前者的 `MessageToUser` 将 lease ID 格式化为十六进制；后者逐项拼接并保留 Go 版本的尾随 `", "`，`Empty` 用于判空。
- `GetImportTasksFrom`：公开有效任务枚举入口；逐 KV 查询 TTL，并过滤已消失或非正 TTL 的 lease。
- `EnableFailpoint`、`DisableFailpoint`：公开测试控制入口；私有 `inject_failpoint`/`inject_failpoint_value` 读取 `OnceLock<Mutex<HashMap<String, i32>>>`。

文件没有条件编译项。生产类型和测试 failpoint API 当前都参与普通构建；调用方应把后者视为测试设施，不应在生产流程中启用。

## 执行流程

持续注册 `RegisterTask` 的主流程如下：

1. 从调用方 `Context` 派生 child token，只把 child 保存到 `cancel`；这样 `Close` 不会取消父上下文。
2. `grant` 请求 `secondTTL` 秒租约。RPC `Err` 直接传播；响应的 `error` 字段非空也转换为失败，并在入口处增加 `failed grant a lease` 上下文。
3. 把 lease ID 写入共享 `curLeaseID`，随后以空 value 将完整 key 绑定到该 lease。
4. 为 lease 创建 keepalive receiver；成功后增加 `wg`，克隆必要状态到新的 `TaskRegisterImpl`，启动线程运行 `keepalive_loop`。线程退出时递减 `wg`。
5. `keepalive_loop` 默认以 `max(ttl / 4, 20s)` 为“剩余时间不足”阈值。收到回执就刷新 `last_update_time`；一秒超时只用于再次检查取消，不代表断流；sender 断开才进入恢复阶段。
6. 恢复阶段先估计 `ttl - elapsed`。若剩余时间不足阈值，则重新 grant、更新 `curLeaseID` 并置 `need_reput_kv`；新 lease 必须成功 put 回原 key 后才能继续。
7. 使用当前 lease 重建 keepalive 流。grant、re-put 或 keepalive 创建失败都会记录警告、检查取消、等待默认 10 秒后重试；取消可在各失败点终止循环。

一次性注册 `RegisterTaskOnce` 先精确读取 key。无记录时走 grant+put，并记录新 lease；有记录时采用第一条 KV 的 lease，写入 `curLeaseID` 后调用 `keep_alive_once`。这条路径不启动后台线程，调用方必须周期性调用以维持 lease。

关闭 `Close` 先取出并取消 child context，再以 10 毫秒间隔等待 `wg` 归零，最后撤销非 `NO_LEASE` 的当前 lease。持续恢复期间若 lease 已被替换，`curLeaseID` 的共享更新保证关闭撤销最新 lease。`LeaseNotFound` 被视为正常的幂等关闭结果，其他撤销错误记录后返回。

枚举 `GetImportTasksFrom` 对根前缀执行 prefix get，再逐项查 `time_to_live`。查询时 lease 已被并发撤销会产生 `LeaseNotFound`，该条被跳过；TTL `<= 0` 也被过滤；其余条目保留 key、lease ID 和当前 TTL。

## 数据与状态

`TaskRegisterImpl` 的核心不变量是：`key` 在实例生命周期内不变，而 `curLeaseID` 可以在初次注册或断流恢复时变化；可见 key 必须绑定 `curLeaseID` 指向的有效 lease。`need_reput_kv` 只存在于一次恢复迭代中，用来防止拿到新 lease 后忘记重新发布 key。re-put 失败时该标志保持为真，下一轮先重试 put，不会误把仅有 lease、没有 key 的状态当作已恢复。

TTL 同时以 `Duration` 和整数秒保存：`ttl` 用于本地剩余时间判断，`secondTTL = ttl.as_secs() as i64` 用于 grant。亚秒部分会在 grant 参数中截断；调用方若传入不足一秒的 TTL，底层将收到 0，当前实现不预先校验。默认值为 180 秒。

并发共享状态包括：

- `Arc<dyn EtcdRegisterClient>`：注册器与后台线程共享客户端。
- `Arc<Mutex<i64>> curLeaseID`：后台恢复写入、关闭和 failpoint 撤销读取。
- `Mutex<Option<Context>> cancel`：主实例持有 child token；后台克隆的该字段固定为 `None`。
- `Arc<AtomicI32> wg`：记录本实例启动且尚未退出的 keepalive 线程数。
- 全局 `FAILPOINTS`：所有测试共享的进程级名称到整数载荷映射，不随注册器实例销毁。

`RegisterTasksList` 是读取时快照，不自动更新；其顺序跟随客户端 `get` 返回顺序，本文件不排序、不去重。`KeyValue.value` 虽属于抽象响应，但注册协议写入空值，当前枚举也不读取它。

## 依赖与调用关系

上游调用者与装配关系：

- `br/pkg/utils/lib.rs` 公开 `register` 模块，并在独立的 `register_test.rs` 中挂载单元测试。
- `lightning/pkg/importer/import.rs::registerTaskToPD` 是 RustCodeGraph 确认的生产调用者：建立 namespaced metadata client，包装为 `MetadataRegisterClient`，以 `RegisterLightning` 和随机 `lightning-<uuid>` 名称调用 `NewTaskRegister`/`RegisterTask`。返回的清理闭包只执行一次，先 `Close` 注册器，再关闭 metadata client。
- `br/pkg/utils/metadata_register.rs` 实现本文件的 `EtcdRegisterClient`，把策略操作映射到真实 `NamespacedEtcdClient`；其独立真实-etcd测试复用 `NewTaskRegisterWithTTL` 与 `RegisterTaskOnce`。
- `br/pkg/utils/register_test.rs` 直接覆盖构造、持续注册、一次性续租、任务枚举、用户文案、grant/re-put 失败恢复与关闭语义。

下游依赖：

- `crate::stubs::context::Context` 提供 child token、取消检查和取消操作；它决定后台线程的停止信号语义。
- `astersql_errors::{SharedError, Trace, Annotate}` 统一具体客户端错误，保留 Go `errors.Trace`/`Annotate` 风格的传播边界。
- `astersql_br_pkg_logutil::{log, Field}` 在恢复和撤销失败时输出结构化警告。
- 标准库 `mpsc::Receiver` 表示 keepalive 回执流，`thread` 承载后台状态机，`Mutex`/`AtomicI32` 管理跨线程状态，`Path::join` 负责 key 拼接。

`Cargo.toml` 没有为注册功能声明 feature；实际 etcd/metadata 能力来自同 crate 已声明的 `astersql-metaservice` 路径依赖，并经相邻适配器接入。本文件本身只直接使用日志和错误 crate。

## 错误处理与边界

初始注册是同步失败的：grant、put 或首次 keepalive 创建任一步失败，`RegisterTask` 返回错误且不会启动 `wg` 线程。这里存在一个资源边界：grant 成功后若 put 或 keepalive 创建失败，当前方法不主动 revoke 已授予的 lease；生产调用者 `registerTaskToPD` 会在错误路径调用 `Close`，因此可撤销记录在 `curLeaseID` 中的 lease。新增调用者也应采用同样清理模式。

后台恢复失败不返回给早已完成的 `RegisterTask` 调用，而是记录日志并持续重试，直到恢复或 child context 被取消。短暂期间 key 可能因旧 lease 被撤销/过期而不可见；测试明确验证失败解除后 key 会重新出现，不能把“`RegisterTask` 返回成功”理解为此后永不出现可见性空窗。

`grant` 同时检查客户端调用的 `Err` 和 `LeaseGrantResponse.error`；生产适配器目前让逻辑错误走 `Err`、将 `error` 置空，但测试替身或未来客户端仍必须遵守这两条失败通道。`GetImportTasksFrom` 只吞掉类型化 `LeaseNotFound` 和非正 TTL；其他 TTL 查询错误会附加十六进制 lease ID 后终止整次枚举。根 prefix get 失败同样整体返回。

锁使用 `expect`，因此 mutex poisoning 会 panic，而不是转换为 `SharedError`。线程启动使用 `thread::spawn` 且没有保留 `JoinHandle`；若 `keepalive_loop` panic，结尾的 `wg.fetch_sub` 不会执行，`Close` 可能永久等待。这是当前实现边界，扩展时不应在循环中引入新的可 panic 操作。

接口没有禁止重复调用 `RegisterTask`，也没有运行时阻止与 `RegisterTaskOnce` 混用；Go 契约通过注释禁止这种用法。重复持续注册会覆盖 `cancel` 并增加多个线程，而旧线程失去可由 `Close` 直接取消的 token，因此调用方必须保证每个实例只选择一种注册模式并按预期调用。

路径由 `Path::join` 构造。在当前 Unix 目标上得到 Go `path.Join` 对应的 `/` 分隔路径；若 `task_name` 含路径分隔、绝对路径或 `..`，本文件没有显式校验，可能影响最终 key 形态。新增外部输入入口时应在调用边界约束任务名，而不是假定任意字符串都保持单一层级。

## 并发与资源生命周期

持续模式每次成功 `RegisterTask` 启动一个状态机线程；生产 `MetadataRegisterClient::keep_alive` 还会启动一个把单次元数据续租适配成回执流的线程。因此一个正常 Lightning 注册当前通常涉及两层后台线程。状态机线程每秒最多因 `recv_timeout` 醒来检查取消；正常回执节奏由下游适配器决定。

`Close` 的顺序是取消、等待、撤销。先等状态机线程退出可避免它在撤销后又 grant/re-put；等待完成后读取最新 `curLeaseID`，从而撤销恢复阶段替换出的 lease。`Ordering::Acquire` 只用于等待计数读取，而增加/减少使用 `Relaxed`；lease 数据同步另由 mutex 提供，计数仅承担存活标记。

child context 将注册循环的取消与调用方父 context 分离：`Close` 调用 `child.cancel()`，不会令父 context 进入取消状态，Rust 测试对此有明确断言。反过来，child token 是否随父 context 取消由 `Context::child_token` 实现保证；本文件在读通道和每个恢复失败分支检查 `is_cancelled`。

一次性模式不创建本文件的线程；租约能否持续存活完全取决于调用方是否按小于 TTL 的周期再次调用。`Close` 对两种模式都撤销当前 lease。生产客户端所有权不属于注册器：`Close` 不关闭 `EtcdRegisterClient`，Lightning 清理闭包在注册器完成后另行关闭原始 metadata client。

全局 failpoint map 通过 mutex 保证单次访问线程安全，但名称是进程共享状态；并行测试若使用相同名称仍会相互影响，所以每个测试必须在结束前禁用其启用项。`thread::sleep` 的重试不能被取消信号提前唤醒，最坏关闭延迟等于当前重试间隔（默认约 10 秒，测试可缩短）。

## 与 Go 版本的对应关系

Rust 文件按 [`register.go`](./register.go) 的状态机逐项移植：三种任务类型、根前缀、10 秒重试间隔、3 分钟默认 TTL、grant 响应错误检查、持续注册、一次性续租、断流后按 `max(ttl/4, 20s)` 判断 re-grant、必要时 re-put、关闭撤销、有效任务枚举和用户消息格式均保持一致。四个 failpoint 名称及用途也与 Go 测试对齐。

主要结构差异来自客户端与并发模型：

- Go `taskRegister` 直接持有 `*clientv3.Client`；Rust 以 `EtcdRegisterClient` 隔离 SDK，并由 `MetadataRegisterClient` 提供生产实现。
- Go 用 goroutine、channel、`sync.WaitGroup` 和普通 `curLeaseID`；Rust 用 OS thread、`mpsc::Receiver`、原子计数以及 `Arc<Mutex<i64>>`，以便主线程和恢复线程共享最新 lease。
- Go 的取消句柄是 `context.CancelFunc`；Rust 保存可调用 `cancel()` 的 child `Context`。
- Go `RegisterTaskType.String()` 对未知整数返回 `default`；Rust enum 不允许未知值，`as_str` 无兜底分支。
- Go failpoint 由编译注入框架提供；Rust 当前是始终编译的进程内全局 map，适合测试但不等同于 Go 的构建期插桩机制。

[`register_test.go`](./register_test.go) 使用真实嵌入式 etcd，验证持续注册、一次性注册保持 lease ID 并提升 TTL、grant 失败恢复和 re-put 失败恢复。Rust [`register_test.rs`](./register_test.rs) 用 `MemEtcd` 保持同一测试意图，并额外精确断言 `MessageToUser` 文案、`Close` 不取消父 context、关闭后列表为空；模拟器不模拟真实时间自动过期。相邻 [`metadata_register_test.rs`](./metadata_register_test.rs) 还有一个需要 `ASTER_ETCD_TEST_ENDPOINT` 的 `#[ignore]` 测试，验证真实适配器的一次性 lease 复用、key 清理和重复关闭。

## 扩展指南

- 新增任务类型时，在 `RegisterTaskType` 和 `as_str` 中增加稳定路径段，并同步 Go 枚举/`String()`、冲突检查调用方及 `register_test.rs` 的精确 key 断言；路径一旦写入 etcd 即属于兼容协议。
- 修改注册 I/O 时先扩展 `EtcdRegisterClient`，再同时更新 `metadata_register.rs::MetadataRegisterClient` 和 `register_test.rs::MemEtcd`。不要把具体 metadata/etcd 类型引回状态机。
- 调整 keepalive 恢复策略时保持三个不变量：新 lease 必须更新共享 `curLeaseID`，新 lease 必须成功 re-put 后任务才算恢复，关闭必须能停止所有恢复活动后撤销最新 lease。同步扩展 grant/re-put/keepalive 失败测试。
- 若改造线程生命周期，优先保证 panic 也能递减计数，并考虑用可取消等待替代 `thread::sleep`；必须验证 `Close` 不死锁、不取消父 context、不会在 revoke 后重新发布 key。
- 修改 `RegisterTaskOnce` 时保留分布式 owner 切换语义：已有 key 的 lease 来自读取结果，不能假设实例内 `curLeaseID` 仍是 owner 当前 lease。应继续断言第二次注册复用相同 lease 且 TTL 增加。
- 修改枚举或用户文案时同步 `test_register_tasks_list_message_to_user_matches_go`、Go `register_test.go` 及使用 `GetImportTasksFrom`/`MessageToUser` 的冲突提示逻辑；谨慎对待尾随逗号、十六进制 lease 和 TTL 过滤等可观察行为。
- 若接受用户提供的任务名，应新增明确的合法字符/单路径段校验并在独立测试文件覆盖绝对路径、分隔符和 `..`；不要把测试放回生产 `.rs` 文件。
- 新生产调用者应采用 `RegisterTask` 失败也执行 `Close`、正常结束先关闭注册器再关闭客户端的资源顺序，并明确选择持续模式或周期性一次性模式，不能混用。

兼容性风险集中在 etcd key 结构、消息格式、lease 复用和错误分类；正确性风险集中在断流恢复与关闭的竞态；性能风险主要是每个持续注册的线程占用、每秒取消轮询及失败时的固定间隔重试。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件、11,467 个总文件；目标文件可从索引完整读取。
- RustCodeGraph `node --file br/pkg/utils/register.rs --offset 1 --limit 500` 与 `--offset 489 --limit 120`：确认 572 行源码中的常量、类型、trait、状态机、任务枚举和 failpoint 实现。
- RustCodeGraph `query TaskRegister`、`query NewTaskRegisterWithTTL --kind function --json`、`query GetImportTasksFrom --kind function --json`：区分 Go/Rust 同名符号并确认 Rust 定义位置；`explore` 还给出 `lightning/pkg/importer/import.rs::registerTaskToPD` 及 Rust 独立测试的直接调用关系。
- RustCodeGraph `node --file lightning/pkg/importer/import.rs --offset 730 --limit 55`：核对 Lightning 生产入口的构造、注册失败清理以及返回清理闭包。
- RustCodeGraph `node --file br/pkg/utils/metadata_register.rs` 与 `metadata_register_test.rs`：核对生产 I/O 适配、真实 etcd lease 复用与清理验证边界。
- RustCodeGraph `node --file br/pkg/utils/register.go` 和 `register_test.go`：核对 Go 状态机、注释契约、failpoint、嵌入式 etcd 测试和 Rust/Go 差异。
- RustCodeGraph `node --file br/pkg/utils/register_test.rs`：核对 `MemEtcd` 模拟边界，以及持续注册、一次性续租、消息格式、grant/re-put 恢复和关闭生命周期断言。
- 直接读取 `br/pkg/utils/Cargo.toml` 与检索 `br/pkg/utils/lib.rs`：确认 crate 边界、`astersql-metaservice`/日志/错误依赖、公开模块及独立测试挂载；目标包没有 `doc.go`。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试。交付前使用任务指定的结构命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核只新增本说明、没有修改源码、Cargo、Go 或只读 `plan.md`。
