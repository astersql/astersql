# `pkg/ddl/jobsubmit/table_mode.rs`

## 文件定位

本文件属于 `astersql-ddl-jobsubmit` crate 的表模式提交侧实现，由 [`lib.rs`](lib.rs) 公开 `table_mode` 模块并再导出其符号。它把已经解析并补全元数据的表模式变更请求校验、转换成 `JobType::AlterTableMode`（系统表 type 码 75）的版本 2 DDL Job，并生成与 Go `model.AlterTableModeArgs` 兼容的参数载荷。

这里不是表模式的持久化执行器：它不读取 InfoSchema、不写 `mysql.tidb_ddl_job`、不通知 DDL Owner，也不修改表元数据。跨 Keyspace 主链在 `pkg/domain/crossks/ddl_submit.rs::DdlClient::alter_table_mode` 中完成“解析目标 → 调用本文件构建 → 提交 → 通知 → 等待历史 Job”；Owner 侧实际改变模式及 schema version 的逻辑位于 `pkg/ddl/table_mode.rs`。

## 核心职责

1. `TableMode::can_transition_to` 定义提交前的迁移矩阵：同模式、`Normal ↔ Import`、`Normal ↔ Restore` 合法，只有 `Import ↔ Restore` 两个方向非法。
2. `build_alter_table_mode_job` 区分非法、幂等和真实变更三条路径；真实变更会同时返回 Job 与类型化参数。
3. 构造与 Go `BuildAlterTableModeJob` 一致的关键提交元数据：版本 2、schema/table ID 和小写名称、内部查询占位符 `"skip"`、非空 BinlogInfo 标志、会话的 CDC 来源与 SQL mode、初始 `JobState::None` 和冲突检测所需的 involved schema/table。
4. `table_mode_args` 将类型化参数编码成 `JobArgs::Opaque` JSON，供 `submit_batch`/Job 编码链写入持久化 Job。

本文件仅建立提交契约，不承担 DDL schema-state 状态机、reorg/backfill、取消/回滚、MDL、租约同步或历史 Job 清理。

## 主要符号

- `pub enum TableMode { Normal, Import, Restore }`：提交 crate 内部的三态枚举。它与 `astersql_meta_model::TableMode` 是不同 Rust 类型，跨 crate 调用方需显式映射。
- `TableMode::can_transition_to(self, target) -> bool`：纯值判断，无 I/O；只拒绝 `Import → Restore` 与 `Restore → Import`。
- `pub struct AlterTableModeTarget`：已解析目标，包含当前/目标模式、schema/table ID，以及原始名称。构建器信任这些字段已经由上游元数据解析器核验。
- `pub struct AlterTableModeArgs`：写入 Job args 的最小载荷，只含目标模式、schema ID、table ID。
- `pub struct SessionVariables`：构建 Job 所需的会话快照子集，含 `cdc_write_source` 与 `sql_mode`；`Default` 两项均为 0。
- `build_alter_table_mode_job(...) -> Result<(Option<Job>, Option<AlterTableModeArgs>, bool), Error>`：核心公开构建器。三元结果的不变量是：真实变更为 `(Some, Some, false)`，同模式为 `(None, None, true)`；非法迁移直接返回 `Err`。
- `table_mode_args(args) -> JobArgs`：将模式映射为 Go 数值 `Normal=0`、`Import=1`、`Restore=2`，并编码键 `table_mode`、`schema_id`、`table_id`。

文件没有模块级常量、trait、异步函数或条件编译项；所有类型和函数均为公开 API。

## 执行流程

`build_alter_table_mode_job` 的流程如下：

1. 调用 `current_mode.can_transition_to(target_mode)`。非法时构造 `ErrorKind::Invalid`，错误文本包含当前模式、目标模式和表名，且不会产生 Job/Args。
2. 若当前模式等于目标模式，立即返回 no-op；上游据此跳过刷新 server state、持久化提交和等待。
3. 用目标模式和对象 ID 构造 `AlterTableModeArgs`。
4. 构造版本 2 `Job`：设置 `JobType::AlterTableMode`、`query="skip"`、`binlog_info_present=true`、会话字段、`JobState::None`，并把 schema/table 名称转换为小写后写入 Job 与 `involving_schemas`；其余字段沿用 `Job::default()`。
5. 返回 `(Some(job), Some(args), false)`。

在跨 Keyspace 生产链中，`pkg/domain/crossks/ddl_submit.rs::DdlClient::build_alter_table_mode_job` 调用该函数并把错误转换为 domain 错误；随后用后端读取的真实会话变量覆盖 Job 的两个会话字段。`SubmitOnlyBackend::submit` 再调用 `table_mode_args`，把 Job/Args 交给 `astersql_ddl_jobsubmit::submit_batch`，回填分配出的 Job ID。独立 session runtime fixtures 也组合这两个函数，编码后插入 `mysql.tidb_ddl_job`，证明载荷能进入持久化调度链。

## 数据与状态

- 模式是值类型，不在本文件中共享或持久化。非法的特殊模式互转必须经 `Normal` 中转。
- `AlterTableModeTarget` 同时携带身份、显示名称和当前状态；本文件只消费它，不重新解析或防止 ID/名称不匹配。该安全边界由 `DdlClient::resolve_alter_table_mode_target` 等上游解析器负责。
- Job 名称统一小写，以对应 Go `ast.CIStr.L`。传入 `String::to_lowercase()` 使用 Unicode 小写规则；跨 Keyspace 测试覆盖名称比较和标准 ASCII 名称归一化。
- `version=2` 决定参数采用对象式 JSON；`table_mode_args` 的数值判别与 Go `TableMode` 的 `iota` 顺序一致。
- `binlog_info_present=true` 表示 Go `Job.BinlogInfo` 非空；`involving_schemas` 为单个小写 `(database, table)`，供后续提交冲突判断。
- 本文件不改变表的 schema state，也不持有 schema version。执行侧成功修改表模式后才推进元数据版本；幂等请求在提交侧直接结束。

## 依赖与调用关系

直接依赖仅有 crate 内 `Error`、`Job`、`JobState`、`JobType`、`JobArgs` 与外部 `serde_json`。`Cargo.toml` 声明 crate 名为 `astersql-ddl-jobsubmit`、入口为 `lib.rs`，正常依赖为 `astersql-meta-model` 和 `serde_json = "1"`；其余 DDL 依赖位于永不成立的 `cfg(any())` 清单，不是本文件当前编译路径的直接能力。

RustCodeGraph 对 `build_alter_table_mode_job` 给出的明确下游边是调用 `can_transition_to` 和实例化 `AlterTableModeArgs`。索引的文件使用关系指向 `pkg/domain/crossks/ddl_submit.rs`、`pkg/session/runtime/normal_ddl_fixture.rs`、`pkg/session/runtime/durable_scheduler_test.rs` 等；文本调用核验还显示 `table_mode_args` 被前述提交适配器和 fixtures 使用。`lib.rs` 通过 `pub use table_mode::*` 暴露这些 API，并把 `table_mode_test.rs` 作为独立测试模块接入。

Go 对应上游是 `pkg/domain/crossks/ddl_submit.go::ddlClient.alterTableMode`：解析目标后调用 `jobsubmit.BuildAlterTableModeJob`，no-op 时返回，否则刷新 server state、`SubmitBatch`、通知 Owner 并轮询历史 Job。Rust `DdlClient::alter_table_mode` 保持同一阶段划分。

## 错误处理与边界

- 非法 `Import ↔ Restore` 返回 `ErrorKind::Invalid`；Rust 测试固定了文本 `invalid table mode transition Import -> Restore for TestTable`。这与 Go 的 `infoschema.ErrInvalidTableModeSet` 属于同一语义，但 Rust 当前不是同一结构化错误类型。
- 同模式不是错误，而是显式 no-op；调用方必须检查 bool 或 `Option<Job>`，不能把无 Job 当作构建失败。
- schema/table 不存在、名称不匹配、读取当前模式失败均不由本文件处理，应在传入 `AlterTableModeTarget` 前完成。
- `table_mode_args` 通过 `serde_json::to_vec(...).expect(...)` 编码仅含整数的固定 JSON 对象；当前数据形态不会正常触发序列化错误，但如果以后加入可失败的自定义序列化字段，应把 panic 改为可传播错误。
- 本文件不保证 Job 最终入库或执行成功；存储错误、Owner 通知、取消、终态错误和意外历史状态由提交/等待层处理。

## 并发与资源生命周期

本文件是同步、无副作用的构建逻辑：不创建线程/任务，不持有锁、事务、通道、session lease 或外部资源。输入按值移入，返回值由调用方拥有，因此自身没有清理或并发竞态。

资源生命周期从上游开始：Go 构建入口从 session pool 借用并 `defer Put`；Rust `SubmitOnlyBackend` 使用注入的 snapshot/session provider。构建完成后，提交层负责事务写入 `mysql.tidb_ddl_job`、Owner 通知和 Job ID；等待层响应取消并轮询历史状态。后续扩展不应把这些长生命周期职责塞进本文件，以免绕过正常 DDL 的持久化、故障恢复和 Owner 调度边界。

## 与 Go 版本的对应关系

主要对照是 `pkg/ddl/jobsubmit/table_mode.go::BuildAlterTableModeJob`：两者先校验迁移、再处理同模式 no-op，随后构造同一组 Args 和 Job 字段。Rust 的 `TableMode` 及迁移矩阵也与 `pkg/meta/model/table_mode.go::{TableMode, CanTransitionTo}` 一致；Args JSON 字段与 `pkg/meta/model/job_args.go::AlterTableModeArgs` 的标签一致。

差异与迁移状态如下：

- Go 从 `sessionctx.Context` 直接读取 CDC/SQL mode；Rust 构建器接收精简 `SessionVariables`。跨 Keyspace Rust 适配器先用默认值构建，再从后端读取并覆盖，最终持久化字段仍与 Go 对齐。
- Go 名称取 `CIStr.L`；Rust 对传入字符串调用 `to_lowercase()`。上游需继续确保名称已经按 Go CIStr 语义解析和核验。
- Go 返回指针与 `model.JobArgs` 接口；Rust 用 `Option` 表达 nil，并把类型化参数显式交给 `table_mode_args` 生成 `JobArgs::Opaque`。
- Go 返回 `infoschema.ErrInvalidTableModeSet`；Rust返回本 crate 的 `ErrorKind::Invalid` 和兼容文本。调用者若依赖错误身份而非语义，需要额外适配。
- 本文件已接入 Rust 跨 Keyspace 提交与持久化测试路径，并非未接线桩；执行侧仍由独立的 `pkg/ddl/table_mode.rs` 负责。

## 扩展指南

- 新增模式或修改迁移规则时，应同步更新 `TableMode`、`can_transition_to`、`table_mode_args` 的数值映射，以及 Go `pkg/meta/model/table_mode.go` 和 Rust meta-model 对应类型；必须保持已持久化数值兼容。
- 修改 Job 字段时，在 `build_alter_table_mode_job` 完成，并同步检查 `pkg/domain/crossks/ddl_submit.rs::SubmitOnlyBackend::submit` 是否仍完整转抄字段。不要在这里直接提交或执行 Job。
- 修改参数结构时，同时更新 Go/Rust meta-model 的 `AlterTableModeArgs`、版本 1/2 编解码兼容路径及 `table_mode_args`；保留旧 Owner 可读取的 JSON 字段与默认值语义。
- 错误契约变化时，应同步 `pkg/ddl/jobsubmit/table_mode_test.rs` 与 Go `table_mode_test.go`，并审查跨 Keyspace 调用层的错误映射。
- 测试继续放在独立文件：构建与边界测试在 `pkg/ddl/jobsubmit/table_mode_test.rs`；真实编码/队列接线可扩展 `pkg/session/runtime/durable_scheduler_test.rs` 或 `normal_ddl_fixture.rs` 的使用场景；执行侧元数据状态机测试属于 `pkg/ddl/table_mode_test.rs`，不要内嵌进生产源文件。
- 性能风险很低（固定大小对象与一次名称小写/JSON 编码）；兼容风险主要来自模式判别值、Job version、JSON 键名、错误语义及 involved-schema 名称归一化。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/ddl/jobsubmit` 确认模块文件集合；`node --file pkg/ddl/jobsubmit/table_mode.rs` 枚举 12 个符号；精确 callees 查询确认 `build_alter_table_mode_job → can_transition_to` 及 `AlterTableModeArgs` 实例化。宽泛 callers 查询因同名 `table_mode.rs` 符号歧义未给出可靠调用边，因此又以索引的 “used by” 关系和精确文本调用位置交叉核验。
- 源码与 crate 边界：`pkg/ddl/jobsubmit/table_mode.rs`、`pkg/ddl/jobsubmit/lib.rs`、`pkg/ddl/jobsubmit/types.rs`、`pkg/ddl/jobsubmit/Cargo.toml`。
- Rust 上下游：`pkg/domain/crossks/ddl_submit.rs`、`pkg/session/runtime/normal_ddl_fixture.rs`、`pkg/session/runtime/durable_scheduler_test.rs`、`pkg/ddl/table_mode.rs`。
- Go 对照：`pkg/ddl/jobsubmit/table_mode.go`、`pkg/domain/crossks/ddl_submit.go`、`pkg/meta/model/table_mode.go`、`pkg/meta/model/job_args.go`。
- 独立测试：`pkg/ddl/jobsubmit/table_mode_test.rs` 覆盖合法构建、字段传递、名称小写、no-op、`Normal → Restore` 和非法特殊模式互转；`pkg/ddl/jobsubmit/table_mode_test.go` 验证 Go 契约；`pkg/domain/crossks/ddl_submit_test.rs` 覆盖上游 session 字段与名称；持久化 fixtures 验证 `table_mode_args` 可进入 Go-compatible Job 编解码链。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；交付验证仅执行固定 11 章节结构检查并人工复核上述符号、调用边与边界陈述。
