# `pkg/session/runtime/crossks_job_submit.rs`

## 文件定位

本文件位于 `astersql-session` crate 的 `runtime` 模块中，由 `pkg/session/runtime.rs` 以 `pub mod crossks_job_submit` 暴露。它是跨 keyspace 表模式 DDL 的“持久化提交适配器”：上游 `pkg/domain/crossks/ddl_submit.rs` 已完成目标元数据解析、模式迁移合法性判断和会话变量采集，本文件把得到的 `AlterTableModeJob` 转成 `astersql_ddl_jobsubmit::JobSpec`，并通过目标 keyspace 的真实 SQL 会话池写入 `mysql.tidb_ddl_job`。

生产接线位于 `pkg/session/runtime/crossks_runtime.rs`：`CrossKSProductionRuntimeFactory::create_runtime` 构造提交器并放入 `CrossKSProductionDdlBackend`；后端的 `DdlBackend::submit` 再调用 `CrossKSJobSubmitter::submit_table_mode`。因此本文件处在跨 keyspace DDL 客户端与通用 DDL jobsubmit 子系统之间，不负责解析 SQL，也不负责 owner 执行 Job。

## 核心职责

1. `CrossKSBDRPolicy` 把系统表中的字符串 BDR 角色映射为 `astersql_ddl_bdr::ast::BDRRole`，并复用 `bdr::IsDenied` 判断 `AlterTableMode` 是否允许提交。
2. `CrossKSJobSubmitter::new` 把跨 keyspace 的会话池、flashback guard、最小 Job ID 提供器、可选 server state 组装成通用 `jobsubmit::SubmitOptions`，固定配置最多 5 次尝试和线性退避。
3. `CrossKSJobSubmitter::submit_table_mode` 将领域对象 `AlterTableModeJob` 编码为 Go 兼容的 version-2 DDL Job 和参数载荷，调用 `jobsubmit::submit_batch` 持久化，并只在成功后把分配出的 Job ID 回填给调用者。

本文件不负责刷新 server state、通知 DDL owner、轮询历史 Job 或推进表模式变更。这些分别由 `DdlClient::alter_table_mode`、`CrossKSProductionDdlBackend::notify_owner` 和 `CrossKSDdlOwner` 等相邻组件完成。

## 主要符号

- `struct CrossKSBDRPolicy`：文件私有、无状态的 BDR 策略适配器。其 `jobsubmit::BdrPolicy::is_denied(&self, role, job_type, _args)` 对角色执行 ASCII 小写归一化：`primary`、`secondary`、`none`/空串分别映射为对应枚举，其余值映射为 `Unknown`；随后使用 `job_type.code() as u8` 调用 `bdr::IsDenied`。当前适配器不把 `JobArgs` 传给 BDR 判断。
- `pub struct CrossKSJobSubmitter { options: jobsubmit::SubmitOptions }`：对通用提交配置的窄封装。字段私有，调用方不能在构造后改动策略。
- `pub fn CrossKSJobSubmitter::new(...) -> Self`：接收共享的 `CrossKSSessionPool`、`CrossKSFlashbackGuard`、`CrossKSMinJobId` 和可选 `ServerState`。会话池经 `CrossKSJobSessionPool` 转成 `jobsubmit::SessionPool`；`before_insert_with_assigned_ids` 固定为 `None`；`max_retry_count` 为 5；退避时第 `attempt` 次睡眠 `10 * (attempt + 1)` 毫秒。
- `pub fn CrossKSJobSubmitter::submit_table_mode(&self, job: &mut AlterTableModeJob) -> Result<(), jobsubmit::Error>`：本文件唯一的业务入口。它映射表模式、构造参数和 JobSpec、同步调用提交函数，并在成功后更新 `job.id`。

文件没有常量、条件编译项或自建线程；唯一显式时间参数是构造器闭包中的退避时长。

## 执行流程

1. 上游 `DdlClient::alter_table_mode` 先解析 schema/table 元数据，调用通用 `build_alter_table_mode_job` 校验当前模式到目标模式的迁移；同模式直接返回，不会进入本文件；随后刷新 server state。
2. `CrossKSProductionDdlBackend::submit` 将可变的 `AlterTableModeJob` 传给 `submit_table_mode`。
3. `submit_table_mode` 把 `TableMode::{Normal, Import, Restore}` 分别编码为 `0、1、2`，生成包含 `table_mode`、`schema_id`、`table_id` 的 JSON version-2 参数字节。
4. 方法构造 `jobsubmit::Job`：版本为 2，复制 schema/table ID 与名称、查询文本、CDC 写来源和 SQL mode；类型固定为 `JobType::AlterTableMode`；`binlog_info_present` 为真；`involving_schemas` 包含当前 `(schema_name, table_name)`。
5. `JobSpec.args` 使用 `JobArgs::Opaque(raw_args)`，下游 `encode_job_args` 会原样保存这段字节；`id_allocated = true` 表示 schema/table 对象 ID 已存在，但下游仍会为 Job 自身分配全局 ID。
6. `jobsubmit::submit_batch` 借用目标 keyspace 会话，检查 flashback 冲突，读取 BDR role/start TS，规范化并校验 involving schema，应用 BDR 和升级期暂停策略，然后在悲观事务中锁全局 ID、分配 Job ID、插入系统表并提交。可重试错误按配置退避，最多进入 5 次尝试；会话无论成功失败都会归还池中。
7. 只有 `submit_batch` 返回成功后，`submit_table_mode` 才执行 `job.id = spec.job.id` 并返回 `Ok(())`。之后上游会尝试通知 owner，并轮询历史表直到同步成功、失败、出现非预期终态或取消。

## 数据与状态

输入 `AlterTableModeJob` 是上游构建好的传输对象，携带 schema/table 标识与小写名称、目标模式、查询占位文本、CDC 来源和 SQL mode。该方法不重新解析元数据，也不复核 current mode；迁移合法性和 no-op 判断属于 `DdlClient::build_alter_table_mode_job`。

持久化 `jobsubmit::Job` 的关键不变量是：`version == 2`、`job_type == AlterTableMode`（数值 type 码 75）、`binlog_info_present == true`，且 `involving_schemas` 中 schema/table 名均非空。`jobsubmit::submit_batch` 会补充 `trace_info_present`、`start_ts`、`bdr_role` 和排队/暂停状态。

可观察的原地状态变更只有两个层次：下游原地修改局部 `JobSpec`（包括 Job ID 和提交状态字段），本文件在提交成功后再把局部 Job ID复制回调用方的 `AlterTableModeJob.id`。若提交失败，`?` 会提前返回，调用方原对象的 `id` 不会由本文件更新。

## 依赖与调用关系

上游主链由 RustCodeGraph 和源码共同确认：

`DdlClient::alter_table_mode` (`pkg/domain/crossks/ddl_submit.rs`) → `CrossKSProductionDdlBackend::submit` (`pkg/session/runtime/crossks_runtime.rs`) → `CrossKSJobSubmitter::submit_table_mode`（本文件）→ `jobsubmit::submit_batch` (`pkg/ddl/jobsubmit/submit.rs`)。

构造链为 `CrossKSProductionRuntimeFactory::create_runtime` 创建 `CrossKSSessionPool`、system-table manager、flashback guard、min-job-ID refresher、server-state syncer 和 `CrossKSJobSubmitter`，随后把它们装入生产 DDL backend。

直接依赖及用途如下：

- `astersql-domain-crossks`：提供领域层 `AlterTableModeJob` 和 `TableMode`。
- `astersql-ddl-jobsubmit`：提供 Job/JobSpec/JobArgs/SubmitOptions、策略 trait、错误类型和持久化提交算法。
- `astersql-ddl-bdr`：提供 Go 对齐的 BDR 角色与拒绝规则。
- `crossks_session_pool`：提供目标 keyspace 的线程亲和 SQL 会话适配、flashback 系统表检查和最小 Job ID 缓存。
- `serde_json`：编码 version-2 `AlterTableModeArgs` 不透明载荷。

这些 crate 均由 `pkg/session/Cargo.toml` 以本工作区 path dependency 声明；该文件没有专属 Cargo feature，`runtime.rs` 也没有为生产模块设置条件编译。

## 错误处理与边界

`submit_table_mode` 的可恢复错误完整透传为 `jobsubmit::Error`：包括会话池获取失败、flashback Job 冲突、BDR 限制、involving schema 非法、server upgrade 策略相关错误、事务/全局 ID/系统表写入失败和重试耗尽。生产 backend 在边界处把它转成字符串型 `crossks::Error`。

JSON 编码使用 `expect("serialize table-mode DDL arguments")`。当前字段仅为整数，按 `serde_json` 的数据模型不会正常失败；若未来参数包含自定义可失败序列化类型，应把此处改为显式错误传播，避免进程 panic。

角色字符串无法识别时不是解析错误，而是映射为 `BDRRole::Unknown` 并交给统一 BDR 策略决定。`cdc_write_source != 0` 时，下游 `submit_batch` 会跳过 BDR 拒绝检查；系统 schema 也由下游豁免。升级期有 server state 时，非系统 schema Job 会被标成系统操作并进入 `Pausing`，而不是在本文件直接拒绝。

本方法不等待 Job 执行成功：返回 `Ok(())` 只证明 Job 已持久化并取得 ID。owner 通知失败在当前上游 `DdlClient::alter_table_mode` 中被忽略，随后仍依靠历史状态轮询；这不是本文件的错误语义。

## 并发与资源生命周期

`CrossKSJobSubmitter` 自身不含可变共享状态；`submit_table_mode` 只通过共享 `SubmitOptions` 工作，因此并发安全依赖其中各 trait object 的 `Send + Sync` 约束。每次提交构造独立的栈上 `JobSpec`，不会共享待写 Job 数据。

`CrossKSJobSessionPool` 从固定大小的 `CrossKSSessionPool` 借出线程亲和 lease。下游 `submit_batch` 在所有正常错误路径上执行 `SessionPool::put`，适配器通过 drop lease 将 worker 索引归还并唤醒一个等待者。实际悲观事务由下游开启和提交；失败时若事务已开始则回滚。重试期间持有同一借用会话，并执行同步 `std::thread::sleep`，因此会暂时占用一个池槽和当前调用线程。

本文件不启动或关闭 worker。会话池、min-job-ID 刷新线程、DDL owner 和 schema syncer 均由 `CrossKSProductionRuntimeFactory` 组装到 runtime lifecycle 中统一关闭。构造器保存多个 `Arc`，保证这些提交依赖至少存活到 submitter 被释放。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/domain/crossks/ddl_submit.go` 与 `pkg/ddl/jobsubmit/{table_mode.go,submit.go}`。Go 的 `ddlClient.alterTableMode` 在一个流程内完成目标解析、构建 Job、刷新 server state、`SubmitBatch`、etcd 通知和等待历史终态；Rust 将该流程拆为领域层 `DdlClient`、本文件的 session 适配器以及生产 backend/owner。

字段语义保持对齐：Job version 为 2、类型为 `ActionAlterTableMode`/数值 75、schema/table ID 与名称被复制、query 沿用上游构建出的 `"skip"`、BinlogInfo 非空，并保留 `CDCWriteSource`、`SQLMode` 与 involving schema。表模式参数的数值对应为 Normal=0、Import=1、Restore=2，version-2 载荷字段为 `table_mode`、`schema_id`、`table_id`。

Go `SubmitBatch` 与 Rust `submit_batch` 都负责 flashback guard、BDR role/start TS、involving schema、升级期暂停、全局 ID 和系统表插入。Rust 构造器把这些依赖显式注入 `SubmitOptions`，并使用 5 次上限及 10/20/30/40/50 毫秒线性退避；这属于当前 Rust 接线参数，不应泛化为所有 Go 调用点的固定策略。

## 扩展指南

- 新增 `AlterTableMode` 持久化字段时，优先同步上游 `AlterTableModeJob`、本文件的 `jobsubmit::Job`/JSON 映射、`pkg/ddl/jobsubmit` 的持久化模型及 Go `model.AlterTableModeArgs`；保持 version-2 字段名和数值编码兼容。
- 修改 BDR 行为时，调整 `CrossKSBDRPolicy::is_denied`，并补覆盖大小写角色、未知角色、CDC 来源和系统 schema 的独立测试；不要在这里复制 `submit_batch` 已有策略。
- 修改重试或退避时，在 `CrossKSJobSubmitter::new` 接线，评估同步睡眠占用池槽的延迟和并发影响；相应边界更适合在 `pkg/ddl/jobsubmit/submit_test.rs` 验证，本目录测试验证实际池接线。
- 若要增加 owner 通知、取消或等待行为，应修改 `pkg/domain/crossks/ddl_submit.rs` 和 `pkg/session/runtime/crossks_runtime.rs` 的相应 backend 方法，而不是扩张 `submit_table_mode` 的职责。
- 本文件的直接回归测试应继续放在独立文件 `pkg/session/runtime/crossks_job_submit_test.rs`，不要把测试内嵌到生产源文件。端到端 owner 消费可扩展 `crossks_owner_test.rs`，完整 runtime/RealTiKV 路径可扩展 `crossks_runtime_test.rs`；通用提交边界则属于 `pkg/ddl/jobsubmit/submit_test.rs`。
- 兼容性风险集中在 Job type/version、参数 JSON、名称规范化和状态字段；正确性风险集中在成功前错误回填 ID、绕过 flashback/BDR/upgrade 检查；性能风险集中在线性退避、同步阻塞和有限会话池容量。

## 验证依据

- 目标源码：`pkg/session/runtime/crossks_job_submit.rs`，核对 `CrossKSBDRPolicy`、`CrossKSJobSubmitter::{new,submit_table_mode}` 的全部实现。
- 模块与 crate：`pkg/session/runtime.rs` 确认生产模块公开且测试独立；`pkg/session/Cargo.toml` 确认 `astersql-domain-crossks`、`astersql-ddl-jobsubmit`、`astersql-ddl-bdr` 和 `serde_json` 依赖。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`explore "pkg/session/runtime/crossks_job_submit.rs CrossKSJobSubmitter submit_table_mode CrossKSBDRPolicy"` 找到生产 runtime、直接测试和 owner 测试；文件节点进一步核对 `crossks_runtime.rs`、`domain/crossks/ddl_submit.rs`、`ddl/jobsubmit/submit.rs`、`ddl/jobsubmit/types.rs`、`crossks_session_pool.rs` 的调用与生命周期边界。图对 `submit_table_mode` 的部分方法边未完整解析，因此以这些精确文件节点交叉确认。
- Rust 测试：`pkg/session/runtime/crossks_job_submit_test.rs` 验证提交后 ID 大于零，system-table manager 可按 ID 解码 Job，且 `mysql.tidb_ddl_job` 中 schema ID=100、table ID=200、type=75、processing=0；`crossks_owner_test.rs` 继续验证持久化 Job 被 owner 消费并进入历史；`pkg/ddl/jobsubmit/submit_test.rs` 覆盖通用提交的会话归还、拒绝、重试和状态边界。
- Go 对照：`pkg/domain/crossks/ddl_submit.go`、`pkg/ddl/jobsubmit/table_mode.go`、`pkg/ddl/jobsubmit/submit.go` 及其 `*_test.go`，用于核对端到端职责拆分、Job 字段、提交语义和系统表结果。
- 本任务为纯文档分析，按计划不运行 Cargo；交付时仅执行任务指定的 11 章节结构检查，并人工复核没有把通知、等待或 owner 执行误写为本文件职责。
