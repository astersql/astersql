# `pkg/ddl/multi_schema_change.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod multi_schema_change` 对外暴露。它用一组独立的 Rust 数据类型和纯内存状态机表达“一条 `ALTER TABLE` 包含多个 DDL 子操作”的核心规则：收集子操作、做语句内冲突检查、推进可回滚/不可回滚阶段、汇总 schema version，以及传播磁盘满暂停和云存储选择。

它目前不是完整 DDL owner/worker 实现。RustCodeGraph 能索引本文件的符号与内部调用边，但没有找到来自生产 Rust 模块的调用者；`run_multi_schema_change` 的已索引调用者仅是 `pkg/ddl/multi_schema_change_test.rs` 中 3 个直接回归测试。因此，本文件当前更接近可测试的领域模型/移植边界；真正的 TiDB 运行链仍可在同路径 Go 文件的 `onMultiSchemaChange`、worker、meta mutator 和 job 持久化逻辑中看到。

## 核心职责

- `append_to_sub_jobs` 与 `fill_multi_schema_info` 把 `MultiAction` 变成 `SubJob`，同时维护列、索引、外键和相对列集合，为提交前检查提供材料。
- `check_operate_same_col_and_idx`、`check_operate_drop_index_used_by_foreign_key` 和 `check_multi_schema_info` 拒绝同一语句重复操作对象、非法最终列数，以及删除新增外键唯一依赖索引的组合。
- `merge_add_index` 在没有新增外键时把多个 `AddIndex` 合并到一个尾部子任务，保留外键依赖的顺序约束。
- `run_multi_schema_change` 是主状态机：逆序回滚、逐项推进可回滚子任务、批量跨越不可回滚边界、再逐项收尾。
- `promote_disk_full_pause`、`update_parent_job_from_proxy`、`collect_warnings` 与 `finish_multi_schema_job` 把代理子任务上产生的 durable 信息汇总回父任务。
- `check_need_analyze` 仅依据开关、analyze version、分区属性和索引是否处于 write-reorganization 判断是否需要 analyze；它不执行 analyze。

## 主要符号

- `MultiJobState`：父/子 job 的精简状态集合；`is_finished` 只把 `Cancelled`、`RollbackDone`、`Done` 视为终态。
- `MultiAction`：允许参与 multi-schema change 的动作枚举，覆盖列、索引、主键、外键、auto-id、注释、字符集和 engine attribute。无法表示的动作在该 Rust API 层无法传入；`MultiSchemaError::UnsupportedAction` 已定义，但当前函数没有产生它。
- `SubJob`：保存动作、状态、子 job schema version、可回滚标志、是否需要 reorg，以及字符串化 error/warning。
- `MultiSchemaInfo`：子任务列表及冲突检查所需的规范化名称集合；派生 `Default`，但默认 `revertible` 为 `false`，调用者创建新父任务时必须显式设为 `true` 才会走可回滚阶段。
- `MultiSchemaJob`：父 job 状态、暂停/恢复原因、最大 schema version 和持久化的 `use_cloud_storage` 选择。
- `TableShape` / `IndexShape` / `ForeignKeySpec`：校验所需的最小表、索引、外键投影，不持有真实 table/meta 对象。
- `MultiSchemaError`：校验、取消、物理层和子任务错误的分类；`Display` 当前输出 `Debug` 文本，没有 TiDB 错误码或错误参数格式。
- `StepResult`：一次代理 step 的状态回传，含 version、错误、磁盘满暂停和云存储模式。
- `SubJobRunner::run_step`：注入真实步进器或测试桩的唯一执行边界；参数分别指示回滚、复用既有 schema version，以及父任务已选的云存储模式。
- `run_multi_schema_change`：核心入口；RustCodeGraph 的 callee 边为 `is_finished`、`SubJobRunner::run_step`、`update_parent_job_from_proxy`、`promote_disk_full_pause`、`handle_revertible_exception`。

## 执行流程

1. 组装阶段调用 `append_to_sub_jobs`。它先由 `fill_multi_schema_info` 将名称统一转成 ASCII 小写并记录依赖，再追加初始状态为 `None`、`schema_version = 0`、`revertible = true` 的子任务。之后可调用 `merge_add_index` 和 `check_multi_schema_info` 做归并与整体验证。
2. 若父任务仍可回滚且处于 `RollingBack`，`run_multi_schema_change` 用 `rposition` 找最后一个未结束子任务，以 `rolling_back = true` 运行一步；没有剩余项时把父任务置为 `RollbackDone`。这保证回滚顺序与正向子任务顺序相反。
3. 若仍处于可回滚阶段，状态机先找第一个 `sub.revertible && !is_finished()` 的子任务，仅推进一步。结果先写回父/子版本及错误；若 KV 磁盘满则恢复子任务的步进前状态并暂停父任务，否则调用 `handle_revertible_exception` 决定是否让整组进入回滚。
4. 当没有仍处于可回滚点的子任务时，状态机遍历所有未结束子任务并批量推进到不可回滚边界。第一个产生非零 version 的 step 之后，后续调用以 `skip_version = true` 复用该批次版本；全部成功后才设置 `info.revertible = false`。
5. 不可回滚阶段每次调用只推进第一个未结束子任务。全部子任务终结后父任务变为 `Done`。
6. 各分支都会以 `max` 维护父任务 schema version；`finish_multi_schema_job` 还能在外部收尾时重新扫描全部子版本并取最大值。

`rolling_back_multi_schema_change` 是管理员取消入口的状态转换辅助：可回滚时把 `Running` 子任务改为 `Cancelling`、未开始项改为 `Cancelled`，父任务改为 `RollingBack` 并返回 `Cancelled`；父任务已不可回滚时则恢复 `Running` 并返回成功，继续完成剩余工作。

## 数据与状态

名称冲突字段存放在 `Vec<String>` 中，写入时通常经 `to_ascii_lowercase` 规范化，检查时再用 `BTreeSet` 保证确定性。`ModifyColumn` 同名修改进入 `modify_columns`；改名则同时计入旧名删除和新名新增。`AddIndex` 同时登记索引名、索引列和隐藏列依赖；新增外键只登记名称与列前缀。

父任务的 `schema_version` 是所有已观察到版本的最大值，而批量越过不可回滚点时的 `generated`/`skip_version` 控制“一批只生成一次版本”的协议。`update_parent_job_from_proxy` 对 `use_cloud_storage` 执行单调 OR：一旦任一代理任务选择云存储，后续本地模式结果不能清除它，从而让后续代理任务及 owner failover 后重建的任务继承同一选择。

暂停路径有意区分父子状态：`promote_disk_full_pause` 把父 job 置为 runner 返回的暂停状态，却把子 job 恢复为 `previous`，避免恢复时以 Paused/Pausing 子任务重建代理 job 而卡住。它同时清空 `resume_reason`，复制 pause reason/error，并在 runner 未提供错误时生成默认磁盘满文案。

`need_reorg`、`warning` 由本文件保存或汇总，但状态机不依据 `need_reorg` 分支，也不负责把 warning 写回 session；这些是交给更上层接线使用的数据。

## 依赖与调用关系

本文件唯一直接标准库依赖是 `std::collections::BTreeSet`，没有直接引用 `pkg/ddl/Cargo.toml` 中的其他 workspace crate。这使状态机可通过 `SubJobRunner` 与真实 DDL 引擎解耦，但也意味着它本身不访问 meta、KV、系统表、schema syncer 或 session。

模块装配边是 `pkg/ddl/lib.rs -> pub mod multi_schema_change`。RustCodeGraph 对 `run_multi_schema_change` 找到的调用者是 `disk_full_pause_preserves_the_generated_schema_version`、`multi_schema_parent_preserves_cloud_mode_for_later_proxy_jobs`、`every_multi_schema_proxy_path_persists_cloud_mode`，均位于独立测试文件 `pkg/ddl/multi_schema_change_test.rs`；仓库级 `rg` 未发现生产 Rust 调用。其他公开辅助函数也未发现生产调用者，因此不能声称当前 SQL executor 已通过 Rust 路径调用这些 API。

概念上的上游是 ALTER TABLE 组装/校验和 DDL job 调度，下游应是实现 `SubJobRunner` 的单 job worker；当前仓库中的完整实证对应仍是 Go：`onMultiSchemaChange` 创建 proxy job 并调用 `w.runOneJobStep`，再把结果写回 `model.SubJob` 和父 `model.Job`。

## 错误处理与边界

- `fill_multi_schema_info` 的 Rust `match` 穷举 `MultiAction`，所以当前永远返回 `Ok(())`；与 Go 的 `fillMultiSchemaInfo` 对未知 `ActionType` 返回 `ErrRunMultiSchemaChanges` 不同。
- `check_multi_schema_info` 按“对象冲突 -> 最终列数 -> 外键索引依赖”的顺序短路。列数使用 `checked_sub` 防止删除数大于当前加新增后的数量，并额外拒绝最终为 0；超过 `max_columns` 返回 `TooManyColumns`。
- 外键检查采用索引列的大小写无关左前缀匹配：只有待删除索引能支持新增外键且所有保留索引都不能支持时才报错。
- `SubJobRunner::run_step` 返回的 `Err(MultiSchemaError)` 通过 `?` 立即向外传播；返回 `Ok(StepResult { error: Some(..) })` 则被当作 job 级错误写回并可能触发回滚。这两个错误通道语义不同。
- 回滚分支没有 Go `handleRollbackException` 对“取消错误可忽略、物理错误需重试”的细分，`Physical` 与 `SubJob` 变体目前也未在本文件内部构造。
- 批量跨越不可回滚边界时，Rust 版不保留/恢复已经处理过的子任务快照，也没有 Go 版的 table-info 恢复、schema diff、storage-class transition staging 或 analyze-before-boundary 逻辑。扩展时不能把现有精简状态机当作完整事务原子性实现。
- `merge_add_index` 用逗号拼接索引名并扁平化列列表/隐藏依赖，属于简化表示；Go 版合并的是 `ModifyIndexArgs.IndexArgs`，仍保留每个索引的独立结构。

## 并发与资源生命周期

所有 API 都是同步的借用式调用：`&mut MultiSchemaJob` 与 `&mut dyn SubJobRunner` 保证一次调用期间独占可变访问；文件内没有线程、异步任务、锁、通道或 I/O。它也不自行持久化，因此进程崩溃恢复、owner 选举、job table 事务和跨节点 schema 同步都不在当前实现中。

状态机每次调用最多推进一个回滚子任务、一个普通可回滚子任务、一个普通不可回滚子任务，或一次性推进整批不可回滚边界。调用方负责反复调度直至父任务终态，并负责在每步之后持久化父子状态。云存储布尔值的单调更新是跨代理生命周期的不变量；磁盘满时恢复子状态、提升父暂停则是 pause/resume 生命周期的不变量。

Go 测试记录稿覆盖并发提交、MDL、failpoint 和 session 等场景，但其文件头明确说明 `CaseRecorder` 只记录步骤、不执行数据库逻辑。因此这些记录不能作为 Rust 并发正确性或资源清理已经验证的证据。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/multi_schema_change.go`，主要映射如下：

- Rust `run_multi_schema_change` 对应 Go `onMultiSchemaChange` 的四段主流程：逆序回滚、逐项推进可回滚子 job、批量越过不可回滚边界、不可回滚收尾。
- `update_parent_job_from_proxy` 对应 `updateParentJobFromProxy`，共同保证 proxy 发现的 `UseCloudStorage` 回写 durable parent；Go 还回写 RU，Rust 模型没有对应字段。
- `promote_disk_full_pause` 对应 `promoteProxyKVDiskFullPause`；两者都恢复子状态并把暂停提升到父 job，Go 还处理 admin operator 和结构化 TiDB error。
- `append_to_sub_jobs` / `fill_multi_schema_info`、`check_operate_same_col_and_idx`、`merge_add_index`、`check_need_analyze`、`check_operate_drop_index_used_by_foreign_key`、`check_multi_schema_info`、`rolling_back_multi_schema_change`、`finish_multi_schema_job` 均有同名语义的 Go 函数。
- Rust `collect_warnings` 对应 Go `appendMultiChangeWarningsToOwnerCtx` 的收集部分，但没有 session `StmtCtx.AppendNote` 副作用。

差异必须视为当前迁移状态而非设计等价：Go 版使用真实 `model.Job`/`SubJob`、`meta.Mutator`、table/infoschema/session、worker 和结构化错误；还包括 analyze、物化视图约束、table-info 恢复、持久化收尾等逻辑。Rust 版用字符串错误和 shape 投影，未接生产调度链。`pkg/ddl/multi_schema_change_test.go` 是 Go 的真实 DDL 回归；Rust 同名测试文件的大部分内容仅机械记录这些步骤，只有文件末尾 3 个测试直接执行 Rust 状态机。

## 扩展指南

- 新增可参与 multi-schema 的动作时，先扩展 `MultiAction`，再同步 `fill_multi_schema_info` 的冲突集合映射，并在 `pkg/ddl/multi_schema_change_test.rs` 的独立测试中覆盖允许组合、重复对象、相对列和顺序约束。不要把测试内嵌回生产文件。
- 若接入真实 DDL worker，应实现 `SubJobRunner` 适配器，并明确每步状态持久化、schema version 生成/复用、结构化错误映射、owner failover 恢复和 schema sync；不能只调用当前状态机而省略 Go `onMultiSchemaChange` 的事务性补偿。
- 扩展回滚时优先补齐 Go `handleRollbackException` 的物理错误重试与取消错误忽略语义，并测试正向/逆向顺序、部分成功后的恢复以及不可回滚后的取消行为。
- 扩展批量边界时，需要保留 Go 的“处理前快照、失败恢复、单 schema version”不变量；任何性能优化都不得让多个子 job 产生对外可见的中间版本。
- 修改 pause/resume 或云存储逻辑时必须同步 3 个直接 Rust 回归：磁盘满版本保留、后续代理继承云模式、四条 proxy 路径均持久化云模式；还应新增 owner failover/persistence 层测试，因为当前单元测试只验证内存模型。
- 修改索引合并或外键逻辑时，应与 Go 的结构化 `IndexArgs` 语义比对，尤其避免逗号拼名和列扁平化在真实接线后丢失索引边界。
- 兼容性风险集中在已持久化 job 字段和错误语义；正确性风险集中在部分批次失败、回滚顺序和 schema version；性能风险集中在 reorg 子任务是否能安全合批。当前文件没有锁或 I/O，不能由这里的微观成本推断完整 DDL 性能。

## 验证依据

- 源码：`pkg/ddl/multi_schema_change.rs`（711 行），核对全部枚举、结构体、trait、impl、公开函数和私有 `update_parent_job_from_proxy`；文件无条件编译项。
- crate/模块：`pkg/ddl/Cargo.toml` 确认 crate 为 `astersql-ddl`、lib 入口为 `lib.rs`；`pkg/ddl/lib.rs` 确认生产模块公开及测试模块以 `#[cfg(test)]` 独立装配。
- 包契约：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md` 用于理解 DDL job、schema version/sync、owner/worker 背景；本文对具体行为仍以源码和测试为准。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；精确 `query/node/callers/callees` 确认 `run_multi_schema_change` 位于第 585 行，其内部调用边及 3 个测试调用者；对其余主要公开函数未找到生产调用者。早先 `files --filter pkg/ddl/multi_schema_change` 返回空与精确符号可查存在工具过滤异常，因此又用仓库 `rg` 复核生产调用范围。
- Rust 测试：`pkg/ddl/multi_schema_change_test.rs`；末尾 `disk_full_pause_preserves_the_generated_schema_version`、`multi_schema_parent_preserves_cloud_mode_for_later_proxy_jobs`、`every_multi_schema_proxy_path_persists_cloud_mode` 是直接行为证据。该文件其余 `CaseRecorder` 用例明确不执行数据库逻辑，只能作为 Go 场景目录。
- Go 对照：`pkg/ddl/multi_schema_change.go` 与 `pkg/ddl/multi_schema_change_test.go`，核对 `onMultiSchemaChange`、校验/合并/取消/收尾辅助函数及真实测试意图。
- 按任务约束未运行 Cargo；交付验证仅执行文档 11 个固定章节的结构检查，并人工复核文档没有把未接线能力写成已支持。
