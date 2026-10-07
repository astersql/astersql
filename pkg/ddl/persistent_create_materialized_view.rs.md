# `pkg/ddl/persistent_create_materialized_view.rs`

## 文件定位

本文件是 `astersql-ddl` crate 中普通 DDL worker 对“创建物化视图”持久化作业的状态推进器。`pkg/ddl/lib.rs` 将其声明为公开模块；统一入口 `pkg/ddl/persistent_actions.rs::step` 在 `job.tp == 86` 时调用本文件的 `step`。这个数值与 Go 的 `model.ActionCreateMaterializedView` 对应。

它不负责解析 `CREATE MATERIALIZED VIEW` SQL 或生成作业；前端建 job 的 Go 路径在 `pkg/ddl/materialized_view.go`。本文件接手已经持久化的 `Job`，在 owner worker 的逐步执行中完成元数据创建、依赖登记、初始数据构建、刷新信息发布，以及失败或取消后的清理。它是 job-based 路径，不是 metadata-only 快速路径。

crate 边界由 `pkg/ddl/Cargo.toml` 确认：本文件直接依赖同 crate 的 worker/持久化模块，以及 `astersql-meta`、`astersql-meta-model`、`astersql-ddl-notifier`、`astersql-util-dbterror` 和 `serde_json`。`[package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/ddl`。

## 核心职责

- `step` 解码 `CreateMaterializedViewArgs`，验证物化视图定义、基础表和物化视图日志（MLog）的持久化元数据。
- 在 `SchemaState::None` 阶段创建物化视图物理表，把新视图 ID 双向登记到基础表的 `MaterializedViewBase.MViewIDs` 和日志表的 `MaterializedViewLog.DependentMViewIDs`，生成 schema version/diff，发布建表通知，并预写刷新记录。
- 在 `SchemaState::WriteReorganization` 阶段通过独立执行会话构建初始数据。第一次成功构建只保存读 TSO 和行数作为可恢复检查点；下一次推进才发布刷新信息、把 `InitBuildState` 改为 ready，并以 `Done/Public` 完成多表作业。
- `rollback` 逆向移除依赖、删除已创建的视图表和 auto ID、清理刷新记录，生成回滚 schema diff，并把作业置为 `RollbackDone/None`。
- `cancel`、`invalid`、`missing`、`get` 统一实现状态变更、错误文本和事务内表读取。

本文件不直接执行 SQL，也不持有连接池；这些能力通过 `JobExecutionContext` 注入。真实适配器是 `pkg/session/runtime/system_session.rs::ConcreteJobExecutionContext`。

## 主要符号

- `pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：唯一公开业务入口。返回本步产生的 schema version；没有版本变化或仅保存重组检查点时返回 `0`。
- `fn rollback(context, job, args, base_ids, log_ids) -> Result<i64, String>`：回滚创建过程并返回回滚 schema version。`args` 是原始物化视图 `TableInfo`，用于按 job 版本重新编码参数。
- `fn cancel(job, error) -> String`：将 job 置为 `Cancelled` 后返回字符串错误。只用于尚未进入必须回滚的错误。
- `fn invalid(job, reason) -> String`：构造 `ErrInvalidDDLJob`，并经 `cancel` 取消 job。
- `fn missing(job, id) -> String`：构造带 schema/table ID 的 1146 错误，并取消 job。
- `fn get(meta, job, id) -> Result<TableInfo, String>`：从当前 schema 读取表；表不存在时调用 `missing`。

文件内没有类型、trait、常量或条件编译项。公开 API 只有 `step`，其余函数均为文件内实现细节。

## 执行流程

1. `step` 用 `GetCreateMaterializedViewArgs` 同时兼容 V1/V2 job 参数；解码失败、缺少 `TableInfo`/`MaterializedView`、基础表 ID 为空、为零或重复时取消作业。
2. 若 job 正在 `Cancelling`，先改为 `Rollingback`，本轮返回 `0`；`Pausing`/`Paused` 原样返回，避免开始新工作；已经处于 `Rollingback` 的作业进入 `rollback`。
3. `SchemaState::None` 下先在一个 metadata transaction 中验证：数据库存在；每个基础对象不是 view、sequence 或临时表；没有分区；状态为 public；存在合法 MLog；MLog 反向指向该基础表且为 public。
4. 调用 `persistent_create_table::create_table(..., false)` 创建物化视图表，并记录 `job.table_id`。随后一个 transaction 幂等地补充基础表与 MLog 的依赖 ID；重复 ID 不会再次插入，重复的 `MLogTableIDs` 会被跳过。
5. 新 transaction 生成 schema version，并通过 `set_create_mlog_schema_diff(job, version, affected, false)` 写入包含受影响基础表/MLog 的 diff；然后 `async_notify_event` 发布 `NewCreateTableEvent`。
6. `prewrite_create_mview_refresh(table.ID)` 使用独立会话先行提交 `mysql.tidb_mview_refresh_info`。失败时转为 `Rollingback`。成功后为 owner 恢复重新编码完整 V1/V2 参数（保留 MLog IDs），转入 `WriteReorganization/Running` 并返回本阶段 version。
7. `WriteReorganization` 下先重新读取实际表元数据，再调用 `build_create_mview_data`。首次成功返回 `(read_ts, count)` 时把 TSO 写入 `job.snapshot_ver`、行数写入 job，然后返回 `0`，让 worker 先持久化检查点。`read_ts == 0` 或构建错误都会转为回滚。
8. 已有非零 `snapshot_ver` 的后续推进调用 `finish_create_mview_refresh` 发布成功刷新信息；再把 `InitBuildState` 改为 ready，更新视图表，生成 schema version/diff，收集当前仍存在的基础表与视图表，最后 `finish_multiple_table_job(Done, Public, ...)`。
9. `rollback` 从仍存在的基础表/MLog 中移除当前视图 ID，必要时清空空的 `MaterializedViewBase`，删除视图表及 auto ID，并调用 `delete_create_mview_refresh`。最后生成带 `rollback=true` 的 schema diff，恢复 V1/V2 参数载荷并进入 `RollbackDone/None`。

状态主链是 `None -> WriteReorganization -> Public`；失败主链是 `Cancelled`（创建前的不可恢复输入错误）或 `Rollingback -> RollbackDone/None`（已经产生持久化副作用后）。本文件没有 delete-only/write-only 等传统表结构中间态；数据构建边界由 write-reorganization 和 job 检查点表达。

## 数据与状态

- `Job.state`：区分排队/运行、暂停、取消、回滚和完成。进入数据或刷新副作用后的错误统一转为 `Rollingback`，而前置元数据校验错误通常直接 `Cancelled`。
- `Job.schema_state`：本流程只接受 `None` 或 `WriteReorganization`；其他状态返回错误。完成时为 `Public`，回滚完成时恢复 `None`。
- `Job.snapshot_ver`：初始构建使用的真实读 TSO，也是“构建已经完成并持久化检查点”的标志。它非零后，下一步不会把本轮视为首次构建。
- `Job.row_count`：保存初始构建实际行数，供持久化进度/历史信息使用。
- `Job.raw_args`：每次进入可恢复阶段或完成回滚时按 `JobVersion` 重写。V1 使用数组 `[table, mlog_ids]`，V2 使用对象 `{"table_info": ..., "mlog_table_ids": ...}`，防止 owner 切换后丢失 MLog 依赖。
- `TableInfo.MaterializedView.InitBuildState`：建表后保持 building；只有刷新信息发布成功、最终 metadata transaction 写入时才改为 ready。
- 基础表 `MaterializedViewBase.MViewIDs` 与 MLog `DependentMViewIDs`：是创建和回滚必须成对维护的反向依赖集合；插入和删除都按 ID 幂等处理。
- durable side effects：目标表和 auto ID、schema version/diff、DDL notifier 事件，以及 `mysql.tidb_mview_refresh_info`。刷新 alert 的清理由真实 context 作为 best effort 处理。

## 依赖与调用关系

上游调用链为：持久化普通 DDL worker -> `pkg/ddl/persistent_actions.rs::step` -> `persistent_create_materialized_view::step`。RustCodeGraph 能确认目标文件、符号和 “used by” 文件集合；对精确 `step` 执行 `callers` 得到空集，模块分派边因此以 `persistent_actions.rs` 的源码接线为准。

主要下游如下：

- `astersql_meta_model::group_2::GetCreateMaterializedViewArgs`：解码 Go 兼容 job 参数。
- `TransactionMutator::{get_database,get_table,update_table,drop_table_and_auto_ids,gen_schema_version,set_create_mlog_schema_diff,set_table_schema_diff}`：读写元数据和 schema diff。
- `persistent_create_table::create_table`：复用普通表创建逻辑，不在本文件复制 ID/表元数据创建过程。
- `persistent_actions::async_notify_event` 与 `astersql_ddl_notifier::NewCreateTableEvent`：登记 schema change 通知。
- `JobExecutionContext::{build_create_mview_data,prewrite_create_mview_refresh,finish_create_mview_refresh,delete_create_mview_refresh}`：把 SQL、独立事务和连接池资源隔离到 session runtime。
- `Job::{set_row_count,finish_multiple_table_job}`：持久化进度并完成涉及多个表的作业。

`pkg/session/runtime/system_session.rs` 的真实 context 使用池化独立 session 执行构建：TiKV 走 `IMPORT INTO ... WITH disable_precheck`，其他存储走 `REPLACE INTO ... SELECT`；它设置并恢复数据库、SQL mode、时区和维护 session variables，缓存同一 job 的完成结果，并拒绝 owner 重启后已有残留行的重复构建。

## 错误处理与边界

- 参数缺失、空/零/重复基础表 ID、无 MLog 或 MLog 归属错误被视为坏 job，使用 `ErrInvalidDDLJob` 并取消。
- 数据库或表不存在、错误对象类型、分区基础表、非 public 基础表/MLog 均在创建副作用前拒绝；这些语义与 Go `onCreateMaterializedViewBaseCheck` 对齐。
- `prewrite_create_mview_refresh`、数据构建、无效读 TSO或最终刷新发布失败发生在已有副作用之后，因此必须回滚，不能直接标记失败完成。
- 不认识的 schema state 明确返回 `invalid create materialized view schema state ...`，不会猜测推进。
- 回滚容忍基础表、MLog 或目标视图已经不存在，以保证重试可达终态；但实际 metadata 更新、删表、schema version/diff 或刷新清理错误仍向上传播。
- 完成阶段收集基础表时跳过已经不存在的表，而视图表本身来自刚更新的实际元数据。`MaterializedView.as_mut().unwrap()` 依赖此前和重读元数据的验证不变量；如果存储中的视图定义被非正常破坏，这里可能 panic，这是扩展时应保护的不变量。
- Rust 的 context 将缺失 `mysql.tidb_mview_refresh_info` 转成可识别的 invalid-job 错误；回滚删除该表时则忽略“系统表缺失”，并 best-effort 删除 refresh alert。

## 并发与资源生命周期

本文件本身不创建线程、锁或异步任务；它由单个 owner 的普通 DDL worker 分步调用。每个 `with_transaction` closure 都限定一次元数据事务，借用在调用独立 SQL session 前结束，避免同一个 worker transaction 与外部 SQL 生命周期交叠。

初始构建和刷新记录预写刻意使用独立池化 session。预写会在外围 metadata/job transaction 提交前独立提交，所以事务冲突或 owner 重启后可能已经存在刷新记录；实现依靠 upsert、依赖 ID 去重和回滚清理保证可恢复。数据构建也先于 job 检查点提交，因此 `snapshot_ver` 是必要的持久化边界；真实 context 的进程内 `completed` map 可在同一 owner 内复用结果，而新 owner 没有该缓存时会检查并拒绝残留物理行，防止静默重复构建。

暂停在进入本文件业务阶段前返回；取消先转换为回滚。schema version/diff 与相关表元数据在各自 transaction 内写入，最终通过 DDL worker 外层提交和 schema barrier 同步到其他节点。通知事件发生在进入重组前；刷新成功信息则必须先于把视图公开为 ready。

## 与 Go 版本的对应关系

直接对照是 `pkg/ddl/mview_worker.go::onCreateMaterializedView`、`onCreateMaterializedViewBaseCheck` 和 `rollbackCreateMaterializedView`。

- 两端都验证 job 参数、基础表集合、对象类型、分区限制、public 状态及 MLog 反向关系；都复用建表逻辑、更新依赖、生成 schema version、发布建表事件、预写刷新记录，再进入 write-reorganization。
- 两端都把初始数据构建作为可恢复 reorg，保存读 TSO/行数，随后发布刷新信息、将 build state 改为 ready，并以多表 job 完成。
- 两端回滚都会移除基础表和日志依赖、删除目标表/auto ID、删除刷新信息、更新 schema version，并保留 V1/V2 参数兼容。
- Go 的 `runReorgJob`、reorg context、暂停/取消错误分类和 failpoint 逻辑没有逐行放进本文件；Rust 将真实构建、残留行检测、session 状态设置/恢复和独立事务封装在 `ConcreteJobExecutionContext`，将通用持久化/owner 驱动放在 worker 外层。因此不能仅比较这一个 Rust 文件的行数判断行为缺失。
- Go 回滚显式记录 refresh-alert 删除失败日志；Rust 的 `delete_create_mview_refresh` 直接忽略 alert 删除错误，外部可见的 best-effort 语义一致，但当前 Rust 适配器没有同等日志。
- Go `updateMaterializedViewBaseInfoOnDrop` 可从实际表信息推导关联项；Rust `rollback` 依赖已持久化的 `BaseTableIDs` 和 `MLogTableIDs`。这也是本文件在每次阶段转换时完整保留 raw args 的原因。

## 扩展指南

- 新增前置合法性规则时，优先修改 `step` 的 `SchemaState::None` transaction，并同步 Go `onCreateMaterializedViewBaseCheck` 的错误类别；确保规则发生在建表和独立预写之前。
- 新增或变更依赖元数据时，同时修改创建登记、`rollback` 逆操作、`affected` schema diff 列表以及 `finish_multiple_table_job` 的表集合，不能只改单向引用。
- 改变数据构建或刷新发布时，保持“独立数据提交 -> 持久化 `snapshot_ver` 检查点 -> 发布刷新信息 -> ready/public”的次序；否则 owner failover 可能重复写数据或过早暴露 ready 状态。
- 改动 job 参数必须同时维护 V1 数组和 V2 对象编码，并检查 `GetCreateMaterializedViewArgs` 的兼容行为；MLog IDs 不得在 owner 恢复载荷中丢失。
- 需要新 SQL/外部资源能力时扩展 `JobExecutionContext`，在 `pkg/session/runtime/system_session.rs::ConcreteJobExecutionContext` 实现；不要把连接池或 session 细节引入本文件。
- Rust 回归测试应放在独立文件 `pkg/session/runtime/normal_ddl_create_materialized_view_test.rs`，不要内嵌到本源文件。至少覆盖 V1/V2、前置拒绝、事务冲突/owner 重启、暂停取消、构建检查点、最终发布和回滚；Go 侧同步检查 `pkg/ddl/mview_worker_test.go` 与 `pkg/ddl/tests/materializedview/materialized_view_create_test.go`。
- 兼容风险集中在 job wire payload、schema diff 的 affected tables 和错误类别；正确性风险集中在跨独立事务的幂等性；性能风险集中在初始全量构建及逐基础表/MLog 的元数据读写。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用（目标文件含 24 个符号）；`files --filter pkg/ddl/persistent_create_materialized_view.rs` 定位目标；`node --file ...` 阅读全部 332 行；`query step --kind function` 唯一定位 `persistent_create_materialized_view.rs::step`；精确 `callers` 返回空集，故没有把未发现的静态调用边写成事实。
- Rust 入口与接口：`pkg/ddl/lib.rs`、`pkg/ddl/persistent_actions.rs::step`、`pkg/ddl/job_worker.rs::JobExecutionContext`、`pkg/session/runtime/system_session.rs::ConcreteJobExecutionContext`。
- crate/依赖边界：`pkg/ddl/Cargo.toml`。
- Go 对照：`pkg/ddl/mview_worker.go::onCreateMaterializedView`、`onCreateMaterializedViewBaseCheck`、`rollbackCreateMaterializedView`；作业创建参考 `pkg/ddl/materialized_view.go`。
- Rust 独立回归：`pkg/session/runtime/normal_ddl_create_materialized_view_test.rs` 覆盖 V1/V2 创建、19 类依赖错误、预写失败、独立提交冲突与重启、构建读 TSO/行数、残留行、暂停/取消、owner epoch、发布及回滚；测试没有放入生产源文件。
- Go 回归：`pkg/ddl/tests/materializedview/materialized_view_create_test.go` 覆盖构建失败/取消回滚、刷新信息失败、残留行重试、schema version、刷新记录可见性及暂停恢复；`pkg/ddl/mview_worker_test.go` 覆盖依赖元数据更新辅助逻辑；`pkg/ddl/rollingback_internal_test.go` 覆盖不可取消状态。
- 人工复核结论：该文件存在于 owner 驱动的持久化 DDL 主链中；以两阶段 schema state、独立构建事务和可恢复 job 字段协调创建/构建/发布；安全扩展必须成对维护依赖与回滚，并保持 wire 兼容和检查点顺序。
