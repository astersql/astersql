# `pkg/ddl/rollingback.rs`

## 文件定位

本文件属于 `astersql-ddl` crate：`pkg/ddl/Cargo.toml` 的 `[lib]` 将 crate 根设为 `pkg/ddl/lib.rs`，而 `lib.rs` 以 `pub mod rollingback;` 公开本模块，并在 `#[cfg(test)]` 下装配独立测试 `pkg/ddl/rollingback_test.rs`。

它提供一个不依赖真实元数据、事务或 worker 的 DDL 回滚决策模型：输入简化的 `RollbackJob`，依据 `JobAction` 与 `SchemaState` 改写内存中的作业状态。当前它不是完整应用中实际驱动 DDL 回滚的执行器。RustCodeGraph 对 `convert_job_to_rollback`、`update_columns_null_to_not_null` 和 `record_rollback_conversion_error` 未找到生产调用方，仓库搜索只发现 `rollingback_test.rs` 使用这些 API；实际在线主链仍是 Go 的 `pkg/ddl/job_worker.go::runOneJobStep` 调用 `pkg/ddl/rollingback.go::convertJob2RollbackJob`。

因此，本文件目前更准确的角色是 Go 回滚分支的局部 Rust 移植/语义契约，而不是会持久化 `mysql.tidb_ddl_job`、推进 schema version 或等待集群 schema 同步的 owner-side worker 实现。

## 核心职责

- `convert_job_to_rollback` 把统一入口收到的动作分派到各类私有转换函数，并先保存触发回滚的错误文本。
- 各转换函数把当前 schema 阶段归入三类结果：尚未开始则取消；可逆的中间态转为 `RollingBack` 并标记参数需要改写；不可安全反转的删除/重组阶段恢复 `Running` 继续前滚。
- `update_columns_null_to_not_null` 模拟 Go `UpdateColsNull2NotNull` 的列标志修复：将索引涉及列设为非空并清除临时阻止 NULL 写入的标志。
- `record_rollback_conversion_error` 模拟 Go 转换失败计数超过全局限制后的兜底取消及错误文案。

本文件只表达决策结果。`args_rewritten: bool` 只是“参数已改写”的标志，不包含 Go 中 `FillFinishedArgs`/`FillRollBackArgsForAddColumn` 等真实参数；它也不读取表结构、不更新元数据、不生成 schema version、不删除索引键。

## 主要符号

- `JobAction`：公开动作枚举，覆盖索引、列、分区、表/视图/schema、约束等有限动作；`Other` 是默认取消分支。它不是 `astersql-meta-model` 中真实 `model.ActionType` 的别名。
- `SchemaState`：公开的简化 schema 状态枚举，包含 `None`、`DeleteOnly`、`WriteOnly`、两种 reorganization、`ReplicaOnly` 和 `Public`。
- `JobState`：公开的简化作业状态枚举。转换逻辑会写入 `Running`、`RollingBack`、`RollbackDone` 或 `Cancelled`；`Cancelling` 通常是调用前状态。
- `RollbackJob`：公开、可变的作业快照，保存 `id`、动作、schema/job 状态、字符串错误、错误次数及参数改写标志。当前实现不读取 `id`。
- `RollbackError`：公开错误枚举。当前代码可返回 `Cancelled` 或 `InvalidState`；`CannotCancel` 已声明但没有任何分支构造它。
- `convert_job_to_rollback(&mut RollbackJob, impl Into<String>)`：主要公开入口；内部调用 12 个动作转换/取消帮助函数。
- `ColumnNullability` 与 `update_columns_null_to_not_null`：公开的简化列标志模型及批量更新函数。
- `record_rollback_conversion_error`：公开的错误计数辅助函数；当 `error_count > limit`（严格大于）时取消。

其余 `convert_add_index`、`rollback_add_column`、`rollback_drop_column`、`rollback_modify_column`、`rollback_drop_index`、`rollback_exchange_partition`、`convert_truncate_partition`、`convert_add_partition`、`convert_reorganize_partition`、`rollback_add_constraint`、`rollback_drop_or_alter_constraint`、`cancel_only_unhandled` 和 `cancel` 均为模块私有实现。

## 执行流程

1. 调用者构造 `RollbackJob`，通常令 `state = Cancelling`；`rollingback_test.rs::job` 展示了这一调用约定，但入口本身不校验原状态。
2. `convert_job_to_rollback` 无条件把 `occurred_error` 写入 `job.error`，再按 `job.action` 分派。
3. 添加索引/主键和添加列：`None` 直接取消；其余状态转 `RollingBack`、规范化为 `DeleteOnly`，并置 `args_rewritten = true`。
4. 删除列/索引：对象尚为 `Public` 时可取消；删除已经开始的允许状态改回 `Running`，表示不逆转而继续前滚；未列入的状态返回 `InvalidState`。
5. 修改列：`None` 取消；`DeleteOnly`、`WriteOnly`、`WriteReorganization` 转入回滚并标记参数改写；`Public` 继续前滚；其余状态非法。
6. 分区：exchange 在非 `None` 时直接恢复 `Public` 并置 `RollbackDone`；truncate 只在 `Public`/`WriteOnly` 取消，否则前滚；add 在非 `None` 时进入回滚；reorganize 在 `None` 取消、`Public` 前滚、其他状态回滚。
7. drop table/view/schema/partition 和 rename index 仅在各自动作的初始 `Public` 状态取消；truncate table 的初始态是 `None`；动作已推进时全部恢复 `Running`。
8. 添加约束在非 `None` 时回滚并标记参数改写；删除/修改约束在 `Public` 取消，否则前滚。
9. `cancel` 先写 `Cancelled`，再返回 `Err(RollbackError::Cancelled)`；因此“正常取消”在此 API 中通过错误值表达。

列标志辅助流程独立于作业转换：`update_columns_null_to_not_null` 按给定 ID 顺序线性查找列，找到后立即写 `not_null = true`、`prevent_null_insert = false`；任一 ID 缺失即返回 `InvalidState`。

## 数据与状态

核心状态机由 `(JobAction, SchemaState)` 决定，并原地修改 `RollbackJob`。重要不变量和限制如下：

- `convert_job_to_rollback` 不改变 `job.action`；测试 `add_operations_fill_rollback_args_without_changing_the_job_action` 明确验证这一点。
- `args_rewritten` 只会由部分添加/重组路径从 `false` 置为 `true`，没有清零逻辑；重复调用不会恢复初始快照。
- 转换开始就覆盖 `job.error`，包括最终成功返回 `Ok(())`、继续前滚或返回 `InvalidState` 的路径。
- `record_rollback_conversion_error` 每次先加一，再以严格的 `>` 比较限制；限制为 3 时第 4 次失败才取消，这与 Rust/Go 测试证据一致。取消时会覆盖原错误字符串，但不会重置计数或参数标志。
- `update_columns_null_to_not_null` 不是事务式批量更新：若前几个 ID 已成功、后一个 ID 不存在，先前修改不会回滚；重复 ID 会重复写同一列，结果保持幂等。
- 所有类型都是当前模块自己的值类型；没有与真实 `model.Job`、`TableInfo`、`IndexInfo` 或持久化编码直接互操作。

## 依赖与调用关系

本文件没有 `use`、外部 crate 调用或条件编译项，只依赖 Rust 标准库能力（派生 trait、切片迭代、`Option`、`Result`、`format!` 和 `Into<String>`）。因此 `pkg/ddl/Cargo.toml` 的大量 DDL 依赖并未被本文件直接使用；Cargo 证据只确定它编译进 `astersql-ddl` crate。

RustCodeGraph 的内部调用边为：

`convert_job_to_rollback` → 各动作帮助函数 → `cancel`（仅需要取消的分支）。

图查询显示 `convert_job_to_rollback` 会直接调用所有 13 个私有帮助函数；对三个公开函数的生产 callers 查询为空。文本搜索补充确认当前直接消费者是 `pkg/ddl/rollingback_test.rs`。这与 Go 主链形成明确对照：`pkg/ddl/job_worker.go::worker.runOneJobStep` 在真实 job 为 cancelling 时调用 `pkg/ddl/rollingback.go::convertJob2RollbackJob`，随后根据 `job.IsRollingback()` 决定是否持久化改写后的 raw args。

## 错误处理与边界

- `Cancelled` 既表示状态已写成 `JobState::Cancelled`，也作为返回错误通知调用者；调用者不能仅以 `Result::is_err()` 区分正常取消和非法状态。
- `InvalidState` 用于删除列/索引、修改列的未支持状态，以及列 ID 缺失。返回前入口已写入触发错误；某些辅助函数也可能已经产生局部修改。
- `CannotCancel` 当前不可达，不能据此声称 Rust 模型已经表达 Go 的所有“不可取消”错误。
- `Other` 无条件取消；这与 Go switch 覆盖更多具体动作后才在 `default` 取消不同，新增动作若只落入 `Other` 可能丢失真实语义。
- 本文件不验证 `RollbackJob.state` 必须为 `Cancelling`，也不验证动作与 schema 状态组合来自合法的前向状态机；调用者可以传入不真实的快照。
- 本文件没有 Go 中的元数据读取失败、参数解码失败、版本更新失败、failpoint、日志、错误类别保留或“转换错误时维持 cancelling 供重试”等机制。

## 并发与资源生命周期

所有 API 都是同步的纯内存原地修改，没有锁、原子变量、异步任务、通道、事务、文件、网络或后台 worker；借用规则保证一次调用期间对 `RollbackJob`/列切片的独占可变访问。函数本身不提供跨线程协调，也没有资源释放流程。

真实 Go 流程的生命周期不应投射到本文件：owner worker、持久化事务、schema version、reorg worker 停止、step context 清理和失败重试均在其他实现中完成。尤其是 `job_worker.go::runOneJobStep` 才负责识别 cancelling job、调用真实转换并让上层持久化改写参数；本 Rust 文件目前没有这条生产调用边。

## 与 Go 版本的对应关系

主要意图对应 `pkg/ddl/rollingback.go`，但属于显著简化的局部模型：

- Rust `convert_job_to_rollback` 对应 Go `convertJob2RollbackJob` 的动作分派；Rust 帮助函数名称大体对应 Go `rollingbackAddColumn`、`rollingbackDropColumn`、`rollingbackModifyColumn`、`rollingbackDropIndex`、`rollingbackExchangeTablePartition` 等。
- Rust `update_columns_null_to_not_null` 对应 Go `UpdateColsNull2NotNull`。Go 从真实 `TableInfo`/`IndexInfo` 找列并操作 MySQL flag；Rust 使用 `ColumnNullability` 和列 ID。Go 的 `getNullColInfos` 只返回已有的相关列且函数总是返回 `nil`，Rust 对缺失 ID 返回 `InvalidState`，语义并非完全相同。
- Rust `record_rollback_conversion_error` 对应 Go `convertJob2RollbackJob` 尾部的 `job.ErrorCount++` 与 `ErrorCount > TiDBDDLErrorCountLimit` 分支，保留了限制为 3 时第 4 次取消及相同核心错误文案；它省略全局变量加载、结构化 `terror`、日志和正常取消错误的特殊处理。
- Go 各帮助函数会读取真实表/索引/分区/约束元数据，解码和重写 job args，更新 table info/schema version，并保留原错误类别；Rust 只改简化字段和布尔标志。
- Go switch 还覆盖 columnar index、物化视图及 shadow/log、sequence、foreign key、multi-schema change、remove/alter partitioning、charset/collation、placement 等动作；Rust `JobAction` 未覆盖这些分支。
- Go 修改列逻辑还区分 `ModifyColumnType`、旧列 flag 与 reorg 类型；Rust 仅依据 `SchemaState`。Go add index 会处理多个索引、主键列标志、分区 ID 与 duplicate-key 错误；Rust 没有这些数据。

Go 回归测试 `pkg/ddl/rollingback_test.go::TestCancelAddIndexJobError` 使用 failpoint 和真实 DDL job 验证第四次转换错误后的历史 job 状态。Rust 独立测试用内存模型复现计数、主要动作分支、约束与列标志行为，但不能替代 Go 集成覆盖。

## 扩展指南

- 新增动作时，先核对 Go `convertJob2RollbackJob` 及对应帮助函数的真实初始态、可逆点和参数格式，再扩展 `JobAction` 与 `convert_job_to_rollback`；不要用 `Other` 代替尚未移植的语义。
- 若要接入 Rust 生产 DDL 主链，应把简化类型替换/适配为真实 job 与元数据类型，并补齐元数据事务、版本更新、raw args 持久化、结构化错误、日志和重试契约；在完成这些接线前，不应让调用者把 `args_rewritten` 当作已实际重写参数。
- 修改状态分支时，同步扩展独立文件 `pkg/ddl/rollingback_test.rs`，为每个合法/非法 schema 状态增加表驱动断言；Rust 单元测试不可内嵌回生产源文件。
- 修改错误计数时，同时对照 Go `pkg/ddl/rollingback_test.go` 的第 4 次失败边界和错误文案，并覆盖 `limit = 0` 等边界。
- 修改列批处理时，应明确是否保留当前“遇错前的修改不回滚”语义；若要求原子性，应先验证所有 ID，再统一写入，并增加“中途缺失 ID”回归测试。
- 性能上，各动作分派为常数成本；列更新为 `O(index_column_ids × columns)` 的嵌套线性查找。若真实宽表或大索引会调用该模型，再考虑一次构建 ID 到下标的映射，同时维持重复 ID 和错误行为。

## 验证依据

- Rust 源码：`pkg/ddl/rollingback.rs`（RustCodeGraph `node --file` 读取全部 339 行；查询到 58 个符号）。
- Rust 模块与 crate：`pkg/ddl/lib.rs`（`pub mod rollingback` 与 `#[cfg(test)] mod rollingback_test`）；`pkg/ddl/Cargo.toml`（package `astersql-ddl`、`[lib] path = "lib.rs"`、Go package porting metadata）。
- Rust 测试：`pkg/ddl/rollingback_test.rs`，覆盖第四次转换错误取消、添加/删除动作、exchange/partition、约束及列标志更新/缺失列。
- RustCodeGraph：`callers` 对三个公开函数未发现生产调用边；`callees rollingback.rs::convert_job_to_rollback` 列出 13 个私有帮助函数；仓库 `rg` 进一步确认公开 API 只由独立 Rust 测试直接引用。
- Go 对照：`pkg/ddl/rollingback.go` 的 `UpdateColsNull2NotNull`、各动作帮助函数、`convertJob2RollbackJob`；`pkg/ddl/job_worker.go::worker.runOneJobStep` 的实际调用边；`pkg/ddl/rollingback_test.go::TestCancelAddIndexJobError` 的失败计数集成证据。
- 人工复核结论：本文件存在是为了保存可测试的 Rust 回滚决策子集；它通过动作/状态分派运行；安全扩展必须同时维护状态分支与独立测试，并在宣称生产可用前补齐当前不存在的 worker、元数据和持久化接线。
