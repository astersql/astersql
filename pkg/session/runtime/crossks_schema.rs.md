# `pkg/session/runtime/crossks_schema.rs`

## 文件定位

本文件属于 `astersql-session` crate 的 `runtime` 模块，由 `pkg/session/runtime.rs` 公开为 `crossks_schema`。它位于跨 keyspace DDL 运行时与目标 keyspace 的 etcd/`Domain` 之间：`CrossKSProductionRuntimeFactory::create_runtime` 在创建目标 keyspace 的 `Domain` 后构造并启动这里的 schema 同步器，同时构造并首次刷新 server-state 同步器（`pkg/session/runtime/crossks_runtime.rs`）。

文件只实现跨 keyspace 运行时所需的两段协调逻辑，不是通用 DDL schema 同步器的完整 Rust 移植：`CrossKSSchemaSyncer` 发布、观察和等待 schema 版本；`CrossKSStateSyncer` 读取并缓存集群是否处于升级状态。crate 边界与直接依赖由 `pkg/session/Cargo.toml` 中的 `astersql-domain`、`astersql-domain-crossks`、`astersql-domain-serverinfo`、`astersql-ddl-jobsubmit` 和 `serde_json` 声明确认；`nextgen` feature 没有条件编译本文件中的行为。

## 核心职责

1. `CrossKSSchemaSyncer` 启动时申请 90 秒 etcd lease，把目标 `Domain` 当前的 `SchemaMetaVersion` 写入 `/tidb/ddl/all_schema_versions/<server-id>`，然后启动命名后台线程轮询 `/tidb/ddl/global_schema_version`（`start`、`publish_self`、`run`）。
2. DDL owner 完成一次目标 keyspace 的变更后，`publish_global` 先写全局版本，再刷新本实例版本；`wait_all_versions_with_cancel` 以当时本地 schema 版本为屏障，等待 `/tidb/server/info/` 下的每个注册服务器都发布不小于目标值的版本（调用点为 `CrossKSDdlOwner::process_once`，见 `pkg/session/runtime/crossks_owner.rs`）。
3. follower 线程发现全局版本大于本地版本时调用 `Domain::reload`，成功后发布新的本实例版本（`run`、`reload`）。
4. `CrossKSStateSyncer::refresh` 从 `/tidb/server/global_state` 读取 JSON，仅当 `state == "upgrading"` 时设置升级标志；其 `ServerState::is_upgrading` 实现供持久化 DDL 提交逻辑读取。`CrossKSProductionDdlBackend::refresh_server_state` 在提交前触发刷新（`pkg/session/runtime/crossks_runtime.rs`、`pkg/domain/crossks/ddl_submit.rs`）。

## 主要符号

- `GLOBAL_VERSION_KEY`、`ALL_VERSIONS_KEY`、`GLOBAL_STATE_KEY`：分别对应全局 schema 版本、各服务器 schema 版本前缀和全局运行状态；值与 Go 的 `pkg/ddl/util/util.go` 常量一致。
- `CrossKSSchemaSyncer`：公开、以 `Arc<Self>` 使用的同步器。`domain` 提供当前 info schema 和重载能力；`etcd` 是目标 keyspace 的 `EtcdClient`；`id` 形成实例版本键；`lease`、`thread` 保存可关闭资源；`stopped` 同时是运行线程停止位和默认等待取消位。
- `CrossKSSchemaSyncer::new(domain, etcd, id) -> Arc<Self>`：只组装状态，不执行 I/O。
- `start(&Arc<Self>) -> Result<(), String>`：申请 lease、发布初始版本并创建 `crossks-schema-<id>` 线程。必须在需要同步的实例投入服务前调用。
- `self_key`、`publish_self`：内部键构造和带 lease 的本实例版本写入；后者要求 `lease` 已存在。
- `publish_global`：把本地版本无 lease 写到全局键，再把同一版本写到本实例键。
- `wait_all_versions`、`wait_all_versions_with_cancel`：前者以自身 `stopped` 作为取消源；后者额外接受 owner 的取消位，便于 owner 丢失时中断屏障。
- `reload`、`run`：分别执行一次 `Domain::reload` 后的版本发布，以及每 100 ms 检查全局版本的后台循环。
- `close` 与 `impl Lifecycle`：幂等停止线程、等待退出、撤销 lease；trait 适配使其可进入跨 keyspace `SessionManager` 的生命周期集合。
- `CrossKSStateSyncer`：保存目标 etcd 客户端和原子升级标志。`new` 不读远端；`refresh` 才更新缓存；`impl ServerState::is_upgrading` 是提交器消费的只读接口。

## 执行流程

创建阶段由 `CrossKSProductionRuntimeFactory::create_runtime` 驱动：取得目标 keyspace 的 etcd 客户端和 `Domain`，以虚拟 server ID 调用 `CrossKSSchemaSyncer::new`，随后 `start`。`start` 先保存 lease，再读取 `domain.info_schema().SchemaMetaVersion()` 并写入实例键；任何初始写入错误都会调用 `close` 撤销 lease。线程创建失败也执行相同清理。随后工厂创建 `CrossKSStateSyncer` 并同步调用一次 `refresh`，避免提交器读取未初始化的默认缓存。

正常 follower 流程在 `run` 中循环：读取全局版本键；只有读取成功、存在首条值、值可解析为 `i64`，且它严格大于本地版本时才尝试 `reload`。`reload` 先重载 `Domain`，再发布重载后的版本。单次读取、解析或重载失败不会终止线程，下一轮 100 ms 后重试。

DDL owner 成功应用变更后，`CrossKSDdlOwner::process_once` 调用 `publish_global`，再以 90 秒超时调用 `wait_all_versions_with_cancel`。等待循环每轮分别读取已注册 server-info 和所有实例版本；对每个 server-info 键取最后一个路径段作为 ID，并要求存在同 ID 的版本项且数值 `>= target`。全部满足即成功；取消、同步器关闭、etcd 读取失败或到期则返回错误，owner 据此决定任务结果。

DDL 提交流程通过 `DdlClient::alter_table_mode` 在入队前调用后端的 `refresh_server_state`。生产后端转到 `CrossKSStateSyncer::refresh`；成功解析后，`CrossKSJobSubmitter` 持有的 `Arc<dyn ServerState>` 可通过 `is_upgrading` 判断是否需要按升级语义处理用户 DDL。

## 数据与状态

schema 版本是十进制 `i64` 文本。全局版本键没有 lease；本实例版本键绑定 `start` 取得的 lease，`close` 撤销 lease 后该键消失。`wait_all_versions_with_cancel` 的 `target` 在进入函数时从本地 `Domain` 快照一次，循环期间不会追随更高版本改变，因此它验证的是一次确定的 DDL 屏障。

`lease: Mutex<Option<i64>>` 和 `thread: Mutex<Option<JoinHandle<()>>>` 表示资源是否已经建立及是否仍待回收；`Option::take` 保证只回收一次。`stopped: AtomicBool` 从 `false` 单向变为 `true`，同步器关闭后没有重启路径。`CrossKSStateSyncer::upgrading` 默认是 `false`，只在一次成功的 `refresh` 末尾更新；远端无值也归一为 `false`，其他字符串同样表示正常运行。

server-info 与版本条目的对应不依赖返回顺序，而依赖完整键相等。空 server-info 集合满足 `all` 的空集语义，会立即通过；存在 server-info 但缺失、不可解析或低于目标的版本项则继续等待。

## 依赖与调用关系

上游装配链为 `CrossKSProductionRuntimeFactory::create_runtime` → `CrossKSSchemaSyncer::{new,start}` / `CrossKSStateSyncer::{new,refresh}`（`pkg/session/runtime/crossks_runtime.rs`）。schema 同步器随后安装到 `CrossKSDdlOwner::install_schema_syncer`；owner 的 `process_once` 在变更成功后调用 `publish_global` 和 `wait_all_versions_with_cancel`（`pkg/session/runtime/crossks_owner.rs`）。运行时构造 `CrossKSProductionDdlBackend` 时保留 state，并把同一个 `Arc` 转成 `Arc<dyn astersql_ddl_jobsubmit::ServerState>` 交给 job submitter。

下游依赖包括：`Domain::info_schema`/`SchemaMetaVersion` 和 `Domain::reload`；`EtcdClient::{GrantLease,Put,Get,RevokeLease}` 与 `Context::Background`；`Lifecycle` 用于统一关闭；`ServerState` 暴露升级缓存；`serde_json` 解析全局状态。`pkg/session/Cargo.toml` 证明这些 crate 是 `astersql-session` 的直接依赖，而非经测试偶然可见。

RustCodeGraph 将目标文件索引为 28 个符号，并标出它被 `pkg/session/runtime/crossks_runtime.rs`、`pkg/session/runtime/crossks_owner.rs` 以及相关测试使用。对常见方法名的全仓调用图存在同名噪声，因此具体调用点以带类型的模块引用和源码调用表达式复核。

## 错误处理与边界

公开 I/O 方法把底层错误统一转成 `String`。`start` 对 lease 申请、初始发布、线程创建错误向上传播，并对已经取得的资源做清理；`publish_global` 若写全局键成功但写实例键失败，会留下已推进的全局版本，调用方必须把它当作未完成的同步流程，而不是原子事务。`reload` 若 `Domain::reload` 成功但版本发布失败，同样返回错误；后台 `run` 会忽略该错误并在后续轮次重试。

等待函数会优先检查两个取消位；etcd `Get` 错误立即返回，不在函数内部重试。超时按 `Instant` 判断，但每轮末尾固定睡眠 20 ms，所以返回时间可能略晚于 deadline。server-info 键若不能取得最后一个路径段、版本缺失、非 UTF-8（按 lossy 文本处理后仍须能解析）或非整数，都视为尚未同步。

`refresh` 在 etcd 读取或 JSON 解析失败时保留旧的 `upgrading` 值，因为原子写发生在解析完成之后；无键或无 `state` 字符串则成功写入 `false`。它只查看返回值的第一项，没有验证多值情形。`Mutex::lock` 使用 `expect`，锁中毒会 panic；后台线程 panic 或 lease 撤销失败在 `close` 中被忽略，调用者只能得到关闭流程已尽力完成，不能据此确认远端清理成功。

## 并发与资源生命周期

`Arc` 让工厂、owner、backend、submitter和生命周期容器共享同步器。`AtomicBool` 使用 Acquire/Release（关闭用 AcqRel），确保关闭信号在线程和等待循环之间可见；升级状态也用 Release 写、Acquire 读。两个 `Mutex<Option<_>>` 只保护 lease 和线程句柄，不包围 etcd I/O 或 `Domain` 重载。

`start` 预期只调用一次：代码没有显式拒绝重复启动，第二次调用会覆盖 lease/线程句柄而泄漏旧资源，因此扩展方不得把它当作可重入 API。`close` 通过 `swap(true)` 幂等化；首次关闭先 join 后台线程，再撤销 lease。由于线程每轮睡眠 100 ms，join 最多需等待当前睡眠结束附近的时间。`close` 不会主动唤醒 `sleep`，等待屏障则以 20 ms 周期观察停止位。

运行时构造失败路径显式调用 `schema.close()`；成功路径通过 `Lifecycle for CrossKSSchemaSyncer` 交给 `SessionManager` 统一关闭。测试 `pkg/session/runtime/crossks_schema_test.rs` 验证关闭后 lease 绑定的实例键消失。代码没有实现 lease keepalive；90 秒 lease 是否自动续租取决于 `EtcdClient::GrantLease` 的具体实现，不能仅从本文件断言长期续租行为。

## 与 Go 版本的对应关系

Go 的主要对应实现位于 `pkg/ddl/schemaver/syncer.go` 和 `pkg/ddl/serverstate/syncer.go`，跨 keyspace 装配位于 `pkg/domain/crossks/cross_ks.go`、提交前刷新位于 `pkg/domain/crossks/ddl_submit.go`。Rust 使用的三个 etcd 路径与 `pkg/ddl/util/util.go` 的 `DDLGlobalSchemaVersion`、`DDLAllSchemaVersions`、`ServerGlobalState` 完全一致；`publish_global` 对应 Go `OwnerUpdateGlobalVersion` 加 `UpdateSelfVersion`，等待条件对应 `WaitVersionSynced` 的“节点版本不小于最新版本”，`refresh`/`is_upgrading` 对应 `GetGlobalState`/`IsUpgradingState`。

当前 Rust 是跨 keyspace 所需子集，不应描述为 Go `Syncer` 的完整等价实现。Go schema syncer具有 watch channel、session Done/Restart、重试、指标、MDL 按 job 版本、server-info syncer 与同步摘要；Rust 用固定周期轮询全局键，并按 server-info 集合直接扫描通用版本前缀。Go state syncer具有 watch/rewatch、状态更新和带超时重试的读取；跨 keyspace Go 装配刻意只调用 `GetGlobalState` 来播种缓存，Rust 同样在创建时和提交前同步刷新，但只暴露 `ServerState::is_upgrading`。这些差异是当前移植边界，不是文档推断的未来承诺。

Rust 独立测试 `pkg/session/runtime/crossks_schema_test.rs::go_merge_43_crossks_schema_and_state_use_target_etcd` 覆盖初始实例键、全局键、缺失/补齐 follower 版本、两种 state 和关闭清理。Go 的 `pkg/ddl/schemaver/syncer_test.go::TestSyncerSimple` 覆盖全局/自身版本及等待，`pkg/ddl/serverstate/syncer_test.go::TestStateSyncerSimple` 覆盖状态同步；它们用于核对协议意图，但不能替代 Rust 行为证据。

## 扩展指南

若增加 schema 协议能力，优先在 `CrossKSSchemaSyncer` 内保持“全局版本推进—本实例发布—owner 屏障”顺序，并同步检查 `CrossKSDdlOwner::process_once`。增加 watch、重试或 keepalive 时，应明确线程/任务的取消与 join 方式，避免新增不可回收资源；若改变服务器集合判定，要同时核对 `server-info` 注册生命周期，防止新加入、过期或异常退出节点造成错误放行或永久等待。

若扩展全局状态格式，应修改 `CrossKSStateSyncer::refresh` 的解析和 `ServerState` 消费契约，并核对 Go `StateInfo` 的 JSON 兼容性。不能在解析失败时无声覆盖旧缓存；如需 fail-closed/fail-open 策略，应由提交链显式决定。若新增公开方法或状态，保持生产逻辑在本文件、测试逻辑在独立的 `pkg/session/runtime/crossks_schema_test.rs`，不要把 Rust 单元测试嵌入源文件。

必要测试至少覆盖：启动中各阶段失败后的资源回收；取消和超时；server-info 缺项、坏版本和版本超前；reload 失败与后续恢复；重复关闭；无状态、坏 JSON 与未知状态；并发 `refresh`/`is_upgrading`。若行为需要与 Go 完整同步器靠拢，还应明确 MDL/job 版本、重试/指标和 watch 断线恢复是否在本任务范围内，避免只复制接口而遗漏语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/session/runtime/crossks_schema.rs` 命中目标；`node --file ... --offset 1 --limit 400` 读取 243 行和 28 个符号；`query` 精确确认 `CrossKSSchemaSyncer`、`CrossKSStateSyncer`、`publish_global`、`wait_all_versions_with_cancel` 的路径与签名。
- 生产源码：`pkg/session/runtime/crossks_schema.rs`；直接装配/调用点 `pkg/session/runtime.rs`、`pkg/session/runtime/crossks_runtime.rs`、`pkg/session/runtime/crossks_owner.rs`、`pkg/domain/crossks/ddl_submit.rs`。
- crate 与依赖：`pkg/session/Cargo.toml`，package 为 `astersql-session`，相关依赖与 `serde_json` 均为直接依赖。
- Rust 测试：`pkg/session/runtime/crossks_schema_test.rs`。Go 对照：`pkg/ddl/util/util.go`、`pkg/ddl/schemaver/syncer.go`、`pkg/ddl/schemaver/syncer_test.go`、`pkg/ddl/serverstate/syncer.go`、`pkg/ddl/serverstate/syncer_test.go`、`pkg/domain/crossks/cross_ks.go`、`pkg/domain/crossks/ddl_submit.go`。
- 本任务是只读行为分析加 Markdown 文档，不运行 Cargo。交付前使用任务指定的结构命令确认文件存在且固定二级标题恰好为 11 个，并人工复核文档分别回答文件为何存在、如何运行和如何安全扩展。
