# `pkg/domain/crossks/ddl_submit.rs`

## 文件定位

本文件是 `astersql-domain-crossks` crate 的跨 Keyspace DDL 提交层，专门把目标 Keyspace 上的 `Alter Table Mode` 请求转换为 DDL Job、写入目标系统表，并等待该 Job 出现在目标 KV 的历史区。crate 由 [`lib.rs`](./lib.rs) 声明并公开再导出 `ddl_submit`；[`Cargo.toml`](./Cargo.toml) 将其归入 Go 包 `pkg/domain/crossks` 的 Rust 移植，并声明对 `astersql-ddl-jobsubmit`、`astersql-kv`、`astersql-meta`、`astersql-meta-model` 和 `astersql-domain-serverinfo` 的直接依赖。

上游调用路径是 [`cross_ks.rs`](./cross_ks.rs) 中的 `RuntimeHandle::alter_table_mode` → `SessionManager::alter_table_mode` → 本文件 `DdlClient::alter_table_mode`。生产运行时可在 `pkg/session/runtime/crossks_runtime.rs` 中组装另一种 `DdlBackend`；本文件自带的 `SubmitOnlyBackend` 则提供只提交、不启动选举或 worker 的 Go 兼容适配器。

## 核心职责

1. 用 `DdlBackend::resolve_metadata` 从目标存储的一致性视图解析数据库、表名和真实 `current_mode`，拒绝对象不存在或调用方名称与目标对象不一致的请求。
2. 复用 `astersql_ddl_jobsubmit::build_alter_table_mode_job` 校验 Table Mode 迁移并构造基础 Job，再从目标系统会话补入 `cdc_write_source` 与 `sql_mode`。
3. 在入队前刷新 server state，借助 `SubmitOnlyBackend::submit` 调用 `submit_batch` 分配 Job ID 并持久化；提交后尽力通知 DDL Owner。
4. 每 100 ms 查询目标历史 Job，直到成功、失败、异常终态或调用方取消。
5. 用 `SubmitOnlyBackend` 把快照读取、会话变量、server-state 刷新、Owner 通知等既有组件接到统一的 `DdlBackend` 边界上，而不在此创建后台线程、选主器或 DDL worker。

## 主要符号

- `DDL_HISTORY_POLL_INTERVAL`：历史表轮询间隔，固定为 100 ms。
- `Error(String)`：本层轻量错误封装；下游错误在边界处转成字符串，因此不保留结构化错误类型。
- `TableMode::{Normal, Import, Restore}`：本层模式枚举；私有 `jobsubmit_mode` 映射到 `astersql_ddl_jobsubmit::TableMode`。
- `AlterTableModeTarget`：请求及解析后的目标，携带 schema/table ID、名称、当前模式与目标模式。输入的 `current_mode` 不可信，解析成功后由元数据值覆盖。
- `SessionVariables`：写入 Job 的 `cdc_write_source` 与 `sql_mode` 快照。
- `AlterTableModeJob`：本层在构建与提交之间传递的 Job 表示；`submit` 成功后会回填 `id`。
- `HistoryJobState::{Synced, Failed, Unexpected}`：把历史 Job 压缩为等待逻辑关心的三类终态。
- `Cancellation`：基于 `AtomicBool` 的协作式取消令牌；`cancel` 使用 `Release`，`is_cancelled` 使用 `Acquire`。
- `DdlBackend`：元数据解析、会话变量读取、server-state 刷新、持久化、通知与历史查询的可替换同步接口。默认 `resolve_metadata` 先查库再查表；具体后端可以覆盖以保证同一快照。
- `DdlClient`：流程编排入口，公开 `new`、`alter_table_mode`、`build_alter_table_mode_job`、`resolve_alter_table_mode_target` 和 `wait_ddl_finished`。
- `SnapshotProvider`、`SessionVariablesProvider`、`ServerStateRefresh`：`SubmitOnlyBackend` 注入真实目标组件的闭包类型。
- `SubmitOnlyBackend`：基于目标快照和 `astersql_ddl_jobsubmit::SubmitOptions` 的只提交后端。
- `EtcdOwnerNotifier`：把通用 `OwnerNotifier` 接到 `EtcdClient::Put`，写入 `/tidb/ddl/add_ddl_job_general`，值为 `0`。

本文件没有条件编译项；测试由 [`lib.rs`](./lib.rs) 通过 `#[cfg(test)] #[path = "ddl_submit_test.rs"]` 放在独立文件中。

## 执行流程

`DdlClient::alter_table_mode` 的顺序具有语义约束：

1. `resolve_alter_table_mode_target` 调用 `backend.resolve_metadata(schema_id, table_id)`。数据库或表缺失立即报错；名称以 Unicode `to_lowercase` 后比较；成功时用存储中的模式覆盖请求模式。
2. `build_alter_table_mode_job` 把目标转换给 `astersql_ddl_jobsubmit::build_alter_table_mode_job`。下游允许同模式、`Normal ↔ Import`、`Normal ↔ Restore`，拒绝 `Import ↔ Restore`。同模式返回 `None`，整个流程无刷新、无提交、无通知。
3. 有 Job 时再取 `backend.session_variables()`，覆盖基础 Job 的 CDC 来源与 SQL Mode，并转换成本文件的 `AlterTableModeJob`。
4. `refresh_server_state()` 必须在提交前执行，使 `submit_batch` 能看到最新升级状态并把非系统 schema Job 标记为系统暂停。
5. `submit(&mut job)` 持久化 Job 并回填 ID。`SubmitOnlyBackend::submit` 构造版本 2、类型 `AlterTableMode`、带 binlog info 和 involving-schema 的 `JobSpec`，用 `table_mode_args` 编码目标模式、schema ID、table ID，然后调用 `submit_batch`。
6. `notify_owner()` 在提交成功后调用，但返回值被显式忽略：通知只是加速 Owner 感知，失败不能回滚已提交 Job。
7. `wait_ddl_finished` 先检查取消，再睡眠 100 ms，随后查历史；未出现或读取失败都继续重试。`Synced` 成功返回，`Failed` 透传历史错误文本，其他终态返回包含 Job ID 和状态的错误。

## 数据与状态

客户端自身只持有 `Arc<dyn DdlBackend>`，不缓存 schema、表或 Job 状态。每次请求都重新解析目标元数据，避免依赖调用方传入的 `current_mode` 或用户侧 InfoSchema 缓存。

`SubmitOnlyBackend` 持有可克隆的提交选项和三个 provider，以及可选通知器。`reader()` 每次通过 `SnapshotProvider` 创建新的 `astersql_meta::SnapshotReader`；覆盖后的 `resolve_metadata` 在同一个 reader 上先读库再读表，保证一次目标解析使用同一不可变快照。单独的 `resolve_database`、`resolve_table` 与 `history_job` 调用各自开启新快照，因此跨调用不承诺同一版本。

Job 的初始 `id` 通常为 0，`submit_batch` 分配全局 ID 后由 `SubmitOnlyBackend::submit` 从 `spec.job.id` 回填。真正的表模式不在本文件内直接修改：Owner/worker 后续执行 Job 才改变元数据；只提交而没有 Owner 时，系统表中可见排队 Job，但表模式保持原值。

## 依赖与调用关系

- 上游：`pkg/domain/crossks/cross_ks.rs` 的 `RuntimeHandle` 和 `SessionManager` 提供代理入口；`pkg/session/runtime/crossks_runtime.rs` 把 `DdlClient` 装入每个目标 Keyspace 的 `SessionManager`。
- 构建下游：`astersql_ddl_jobsubmit::build_alter_table_mode_job` 负责迁移合法性、no-op、名称规范化和基础 Job 字段；`table_mode_args` 编码 Go 兼容的版本 2 参数。
- 提交下游：`astersql_ddl_jobsubmit::submit_batch` 处理 flashback/BDR/升级状态检查、会话借还、全局 ID 分配、系统表插入和可重试事务。
- 元数据下游：`astersql_meta::SnapshotReader::{get_database,get_table,get_history_ddl_job}` 从目标 KV 快照读取 Go 兼容元数据。
- 通知下游：`EtcdOwnerNotifier::notify` 经 `astersql_domain_serverinfo::EtcdClient::Put` 发出通用 Job 提示。
- 生产接线：`pkg/session/runtime/session_factory.rs` 直接构造 `SubmitOnlyBackend`、`EtcdOwnerNotifier` 与 `DdlClient`；`pkg/session/runtime/crossks_runtime.rs` 也可注入拥有同一 trait 契约的生产后端。

RustCodeGraph 将 `ddl_submit.rs` 标记为被 `pkg/domain/crossks/cross_ks.rs`、`pkg/session/runtime/crossks_runtime_test.rs` 等文件使用；精确文本搜索进一步确认上述代理、装配和测试调用点。图的精确 `callers/callees` 命令在本次环境中未在 30 秒内返回边，因此未把缺失的图边当作事实来源。

## 错误处理与边界

- `resolve_metadata` 的存储错误直接终止；数据库不存在时不再查表。对象不存在与名称不匹配分别生成包含 ID 或期望/实际名称的错误。
- 名称比较与 Job 名称规范化使用 Rust Unicode 小写转换，测试覆盖 `TÉST`/`tést` 和 `T_Ä`/`t_ä`；这对应 Go `CIStr.L` 的当前移植意图，但不是通用 SQL 排序规则实现。
- 非法模式迁移由 jobsubmit 返回错误；同模式是成功 no-op。`Import ↔ Restore` 不能直接转换，需经 `Normal`。
- `refresh_server_state`、会话变量读取和提交失败都会阻止后续步骤；提交失败时没有通知和等待。
- Owner 通知错误刻意忽略，因为 Job 已经提交。扩展时不得把通知失败改成请求失败或尝试回滚，除非同步改变 Go 契约。
- 历史读取错误没有次数上限，也不向调用方暴露，会持续重试至成功终态或取消；因此后端永久故障会导致无限等待，调用方必须提供可取消令牌。
- 取消只在每轮 sleep 前检查，最坏响应延迟约为一个轮询周期，且不能中断正在执行的同步后端调用。
- `HistoryJobState` 判断顺序是先 `Synced`，再看历史错误，最后归为 `Unexpected`；已同步 Job 即使携带错误字段仍视为成功。

## 并发与资源生命周期

`DdlBackend: Send + Sync` 且由 `Arc` 共享，因此 `DdlClient` 可跨线程使用；具体后端/provider 必须自行保证其内部并发安全。`Cancellation` 是单向、幂等的原子标志，没有复位能力，也不拥有线程或通道。

`wait_ddl_finished` 是阻塞式轮询：它占用调用线程并使用 `std::thread::sleep`，不会创建后台任务。`SubmitOnlyBackend` 同样不启动 Owner、选举、scheduler 或 worker；这些组件的启动与关闭属于目标运行时。其 `SnapshotReader` 是调用内局部值，读取结束即释放；由 `submit_batch` 借出的系统会话由下游保证无论成功失败都归还。

通知具有“已提交后尽力而为”语义。若通知丢失，正常 DDL Owner 的轮询/调度仍应最终发现 Job；本文件的等待者只观察历史区，不拥有 Job，也不会在取消时撤销已提交的 Job。

## 与 Go 版本的对应关系

直接对照文件是 [`ddl_submit.go`](./ddl_submit.go)：

- 两端均按“解析目标 → 构建/no-op → 刷新升级状态 → 批量提交 → 尽力通知 → 轮询历史”执行，轮询间隔同为 100 ms。
- Go 用 `kv.RunInNewTxn` 在同一事务 reader 中解析库和表；Rust `SubmitOnlyBackend::resolve_metadata` 用单个不可变快照提供对应保证。
- Go `BuildAlterTableModeJob` 从 `SessPool` 取得真实会话变量；Rust 通过 `SessionVariablesProvider` 注入目标系统会话读取，并在构建基础 Job 后覆盖字段。
- Go 用 `context.Context` 同时传播内部事务来源、取消和下游调用上下文；Rust 目前只用 `Cancellation` 控制历史轮询，同步 backend 方法没有上下文参数，也没有显式 `InternalTxnDDL` 标记。
- Go 记录历史读取失败的采样 warning；Rust 静默重试。两端都不因临时历史读取失败结束请求。
- Go `NotifyDDLOwnerByEtcd` 的错误不影响结果；Rust 明确丢弃 `notify_owner` 错误，并用相同 etcd key/value 实现适配器。
- Rust 额外抽象出 `DdlBackend`，便于独立测试和多种生产装配；`SubmitOnlyBackend` 明确不复制 Go 运行时的 Owner 生命周期。

Go 集成测试 `cross_ks_test.go::TestDomainAlterTableModeInKeyspaceSubmitOnly` 验证实际模式往返、幂等重试、名称不匹配、升级期暂停和 context 取消；Rust 的 `cross_ks_test.rs::test_domain_alter_table_mode_in_keyspace_submit_only` 保留同一编排意图，`pkg/session/runtime/crossks_runtime_test.rs` 则补充真实持久化与 Go 元数据兼容性证据。

## 扩展指南

- 新增模式或迁移规则时，首先修改 `TableMode::jobsubmit_mode` 与 `SubmitOnlyBackend::mode` 的双向映射，并同步 `pkg/ddl/jobsubmit/table_mode.rs` 的合法性和参数编码；必须覆盖未知/新增持久化枚举的兼容策略。
- 增加 Job 字段时，应在 `AlterTableModeJob`、`build_alter_table_mode_job` 转换和 `SubmitOnlyBackend::submit` 的 `JobSpec` 构造三处保持一致，并在独立的 [`ddl_submit_test.rs`](./ddl_submit_test.rs) 或 `pkg/session/runtime/crossks_runtime_test.rs` 验证持久化值。不要把 Rust 测试内嵌到生产文件。
- 改变解析逻辑时优先覆盖 `DdlBackend::resolve_metadata`，保持库表同快照；不要退化为两个独立快照读取。名称比较变化需同时核对 Go `CIStr` 语义和 Unicode 用例。
- 引入异步等待、退避或超时时，要保持取消可达、临时历史读取错误可重试、已提交 Job 不因通知失败回滚，并评估 100 ms 固定轮询对目标 KV 的负载与完成延迟。
- 若要传播结构化错误或 context，应评估所有 `DdlBackend` 实现、生产装配及调用链，而非只改 `DdlClient`；当前字符串化边界可能丢失可重试分类。
- 修改提交顺序时必须保持 server-state 刷新先于 `submit_batch`，否则升级状态可能陈旧；同时保留 no-op 不产生任何持久化或通知副作用。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件，目标目录 17 个已索引源码文件；`node --file pkg/domain/crossks/ddl_submit.rs` 读取了目标文件完整 410 行，并显示其被 11 个文件使用。
- RustCodeGraph 源码证据：`pkg/domain/crossks/cross_ks.rs` 第 610–758 行（代理入口与 `SessionManager` 持有关系）；`pkg/domain/crossks/ddl_submit_test.rs` 第 1–82 行（Unicode 名称和 `Normal → Restore`）；`pkg/domain/crossks/ddl_submit.go` 第 1–202 行（Go 主流程）；`pkg/ddl/jobsubmit/table_mode.rs` 第 1–135 行（迁移规则、no-op、Job 字段和参数编码）；`pkg/ddl/jobsubmit/submit.rs` 第 1–100 行（提交约束）；`pkg/session/runtime/crossks_runtime.rs` 第 350–438 行（生产装配）。
- 配置与模块证据：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)。目标目录没有 `doc.go`。
- 测试证据：[`ddl_submit_test.rs`](./ddl_submit_test.rs)、[`cross_ks_test.rs`](./cross_ks_test.rs) 的 `test_domain_alter_table_mode_in_keyspace_submit_only`、`pkg/session/runtime/crossks_runtime_test.rs` 中 `crossks_align_submit_only_*` 测试，以及 Go [`cross_ks_test.go`](./cross_ks_test.go) 的对应集成场景。
- 调用边补证：`rg` 精确定位了 `RuntimeHandle::alter_table_mode`、`SessionManager::alter_table_mode`、`DdlClient` 构造、`SubmitOnlyBackend`/`EtcdOwnerNotifier` 装配和所有直接测试调用；RustCodeGraph 的精确 `callers/callees` 查询本次超时，故没有声称额外静态图关系。
- 本任务仅新增说明文档，未运行 Cargo。结构检查要求文档存在且恰有上述 11 个固定二级标题；交付前还应检查链接路径、diff 范围和任务文件删除状态。
