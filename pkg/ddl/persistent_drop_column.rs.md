# `pkg/ddl/persistent_drop_column.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml`），实现持久化 DDL worker 中 `DROP COLUMN` 的单步状态机。模块由 `pkg/ddl/lib.rs` 公开声明；`pkg/ddl/persistent_actions.rs::step` 在 `job.tp == 6`（`ActionDropColumn`）时把作业分派给本文件的 `step`。它不是 SQL 解析或作业创建入口，而是在作业已经持久化、DDL owner 驱动 worker 反复执行时，逐次修改表元数据并推进 schema 状态。

该流程属于 job-based Online DDL，而不是 metadata-only 快路径。每次调用最多推进一个状态，并通过新的 schema version 让集群观察到阶段变化。列真正从 `TableInfo.Columns` 移除后，关联单列索引的物理键仍由 delete-range 流程异步清理；本文件负责把所需 ID 写回作业参数，`pkg/ddl/delete_range.rs` 再读取这些参数生成清理范围。

## 核心职责

- 兼容解码及保存 V1 与较新版本的 DROP COLUMN 作业参数（`decode_args`、`save_args`）。
- 在第一次状态推进前验证列能否安全删除（`check_column`），包括物化视图日志、生成列/函数索引、最后一列、多列或主键索引、检查约束、部分索引条件、外键和 TTL 列限制。
- 驱动 `Public -> WriteOnly -> DeleteOnly -> DeleteReorganization -> None` 状态链，并同步相关单列索引状态或从元数据移除索引（`step`）。
- 为 `NOT NULL` 列补存 `OriginDefaultValue`，保证旧 schema 仍可能写该列时有兼容默认值（`origin_default`）。
- 在作业完成前清理指向被删列的 masking policy，并记录索引 ID、分区 ID，供后续 delete-range GC 使用。
- 处理 multi-schema change 的“先标记不可回退、下一轮再真正推进”边界，以及回滚完成状态。

## 主要符号

- `DropColumnArgs`：文件内私有的序列化参数结构。`Col` 保存目标列信息，`IgnoreExistenceErr` 对应 `IF EXISTS`，`IndexIDs` 与 `PartitionIDs` 是状态机后段补写的清理信息。字段通过 serde rename 与 Go JSON 字段兼容；`#[serde(default)]` 允许缺失的新字段取默认值。
- `decode_args(job: &Job) -> Result<DropColumnArgs, String>`：按 `JobVersion` 选择参数格式。V1 把 `raw_args` 当位置数组读取；其他版本直接反序列化结构体。V1 至少要求第一个元素为列名，后续三个元素可选。
- `finished_range_ids(job: &Job) -> Result<(Vec<i64>, Vec<i64>), String>`：crate 内可见的完成参数读取口，返回 `(IndexIDs, PartitionIDs)`；调用点是 `pkg/ddl/delete_range.rs` 的 `ACTION_DROP_COLUMN` 分支。
- `cancel(job: &mut Job, error: impl ToString) -> String`：统一把作业置为 `JobState::Cancelled` 并把错误转成字符串。
- `check_column(...)`：删除前的不变量检查。检查失败通常通过 `cancel` 终止作业；读取元数据失败则直接传播。
- `origin_default(col: &mut ColumnInfo)`：仅在 origin default 尚未设置且列为 `NOT NULL` 时计算兼容值。普通类型借助 `astersql_table::column::GetZeroValue`，枚举取第一个元素；`CURRENT_TIMESTAMP` 对 timestamp 使用 UTC、对 datetime 使用本地时间，并按列的小数秒精度截断。
- `save_args(job, args)`：重新编码参数到 `job.raw_args` 并清空内存态 `job.args`。V1 保持位置数组格式；只有存在索引 ID 时才追加索引与分区数组。
- `step(context, job) -> Result<i64, String>`：唯一公开执行入口，返回本步生成的 schema version；未更新版本的分支返回 0。

## 执行流程

1. `step` 先解码参数并取得规范化列名 `Col.Name.L`。解码失败或缺列参数时调用 `cancel`。
2. 在 `JobExecutionContext::with_transaction` 提供的元数据事务内，通过 `persistent_actions::public_table` 取得目标表，并只匹配非隐藏列。目标不存在时：`IgnoreExistenceErr` 为真则记录 warning、把作业置为 `Done` 并不生成 schema version；否则取消作业并返回 `ErrCantDropFieldOrKey`。
3. `check_column` 验证删除限制。成功后记录列 ID，并检查 multi-schema 作业：若当前不是回滚且 `revertible` 仍为真，则只调用 `mark_non_revertible`、保存当前 schema state、生成 schema version、写 schema diff 和表元数据，然后结束本轮；下一轮才推进列状态。
4. 普通状态推进为：
   - `Public -> WriteOnly`：同步把仅包含该列的单列索引置为 `WriteOnly`，把列移动到列数组末尾，并计算 origin default。
   - `WriteOnly -> DeleteOnly`：从 `TableInfo.Indices` 删除这些单列索引，把索引 ID 写回作业参数。
   - `DeleteOnly -> DeleteReorganization`：继续保留末尾列元数据，让旧 schema 与写路径按阶段收敛。
   - `DeleteReorganization -> None`：从末尾弹出列并标记流程完成。
5. 每个实际状态推进都生成 schema version，写入 table schema diff，并持久化更新后的 `TableInfo`；事务成功后再把 `job.schema_state` 更新为新状态。
6. 最终阶段在元数据事务回调结束后执行 masking policy 查询和逐条删除。非回滚作业随后保存所有分区 ID；最后调用 `finish_table_job`，正常路径进入 `Done`，回滚路径进入 `RollbackDone`，schema state 均为 `None`。
7. 历史作业进入 delete-range 注册时，`finished_range_ids` 交出索引和分区 ID。非分区表以 `job.table_id` 为物理 ID，分区表对每个分区生成对应索引键范围。

## 数据与状态

持久状态分为三组：表元数据、作业元数据和后续清理参数。表侧的 `ColumnInfo.State` 是状态机主状态，相关单列 `IndexInfo.State` 只在离开 `Public` 时同步一次；索引对象在离开 `WriteOnly` 时从表元数据删除。列每轮都被移动到 `Columns` 末尾，因此最终 `pop` 删除的是目标列，同时保持可见列 offset 的演进方式与 Go 版本一致。

作业侧会修改 `job.state`、`job.schema_state`、`job.warning`、`multi_schema_info.revertible`、`raw_args` 和最终 table job 结果。`IndexIDs` 在 `WriteOnly -> DeleteOnly` 时形成，`PartitionIDs` 在正常完成时形成；二者必须留在历史作业中，不能在重构参数编码时丢失，否则关联索引数据无法被 delete-range 正确枚举。

每次元数据改变都以 `TransactionMutator::gen_schema_version` 生成版本，并通过 `set_table_schema_diff` 与 `update_table` 一起落盘。文件本身不扫描或回填行数据；`DeleteReorganization` 是在线删除的 schema 阶段，不代表本文件执行数据 backfill。

## 依赖与调用关系

上游主链为 `persistent_actions::step -> persistent_drop_column::step`。更外层由 `pkg/ddl/job_worker.rs` 的持久化 worker 提供 `JobExecutionContext`，其接口把元数据事务 (`with_transaction`) 与 SQL 查询 (`query`) 暴露给动作实现。`pkg/ddl/lib.rs` 负责模块装配。

主要下游依赖如下：

- `astersql_meta::TransactionMutator`：读取数据库/表、生成 schema version、记录 schema diff、更新表。
- `astersql_meta_model`：提供 `Job`、`JobState`、`JobVersion`、`ColumnInfo`、`TableInfo`、`SchemaState` 及其 Go 兼容方法。
- `astersql_util_dbterror`：生成与 Go/MySQL 兼容的 DDL 错误。
- `astersql_sessionctx_vardef::EnableForeignKey`：决定是否执行外键双向引用检查。
- `astersql_table::column`、`astersql_parser_mysql`、`chrono`：生成 origin default 和时间值。
- `persistent_masking_actions::policies_on_column`：严格读取并校验 masking policy 行；本文件再逐条发出 DELETE。
- `pkg/ddl/delete_range.rs`：通过 `finished_range_ids` 消费作业中保存的 ID，形成异步索引范围清理任务。

`pkg/ddl/Cargo.toml` 明确声明上述内部 crate 以及 `serde`、`serde_json`、`chrono` 依赖，并以 `lib.rs` 作为 crate 根；未见本文件专属 feature gate。

## 错误处理与边界

参数 JSON 无法解码、缺少目标列参数、非法 enum 零值、默认值转换失败、元数据访问失败、schema version/table 更新失败、masking policy 查询或删除失败都会以 `String` 向上返回。参数/业务合法性错误多会同时把作业置为 `Cancelled`；事务基础设施错误不一定改写状态，由外层 worker 决定重试或终止。

主要业务拒绝条件由 `check_column` 明确编码：物化视图日志仍引用列；生成列或隐藏函数索引依赖列；表仅剩一列；主键、多列索引或列存索引包含列；多列检查约束包含列；部分索引条件引用列；启用外键时本表外键或其他表的引用外键包含列；TTL 使用该列。允许自动移除的索引只限本文件收集的“恰好一个索引列且名称匹配”的索引。

未知列若带 `IF EXISTS` 是特殊成功路径：作业记录 warning 后直接 `Done`，不写表、不生成版本。其他未知列会取消。隐藏列始终不作为用户 DROP COLUMN 的目标。

状态不在四个预期阶段时返回 `ErrInvalidDDLJob`。当前实现构造错误参数时使用的是 `t.State`，与 Go `onDropColumn` 的错误参数一致；文档不把它解释为列状态诊断。

masking policy 清理发生在元数据 `with_transaction` 回调返回之后，但仍在同一次 `step` 调用内。若清理失败，表元数据事务可能已经完成而作业尚未调用 `finish_table_job`；因此该动作必须具备按持久作业重入的预期，扩展时不能假定两类写入是同一个原子事务。源码模块注释中的“same transaction”应结合 `JobExecutionContext` 的实际边界理解，当前可验证事实是事务借用在 SQL 执行前释放。

## 并发与资源生命周期

本文件不创建线程、任务、锁或通道。并发安全与 owner 任期、作业行持久化、事务提交、schema version 同步由 `job_worker.rs` 和更外层调度器负责。`step` 使用 `&mut JobExecutionContext` 与 `&mut Job`，单次调用内独占可变作业状态。

元数据生命周期被限制在 `with_transaction` 回调内：`TransactionMutator` 借用事务，更新 schema diff 和表后释放。回调外才执行 masking system-table SQL，符合 `JobExecutionContext` 注释所述“transaction callbacks release their borrow before SQL execution”。每个状态只前进一步，使 worker 可在提交和全局 schema 同步后再次调用；multi-schema 的不可回退标记还额外占用一轮，从而在真正改变可见状态前持久化取消边界。

物理索引数据不在本文件生命周期内同步删除。索引与分区 ID 随作业进入历史记录，随后由 delete-range/GC 在安全点条件满足后异步回收；这也是 `save_args` 与 `finished_range_ids` 的持久化契约。

## 与 Go 版本的对应关系

直接对照实现是 `pkg/ddl/column.go` 的 `worker.onDropColumn`、`checkDropColumn`、`checkDropColumnForStatePublic`、`checkDropColumnWithMLogBaseConstraint` 和 `isDroppableColumn`。Rust 保留了 Go 的四段 schema 状态、列移至末尾、单列索引随状态移除、multi-schema 先变为不可回退、正常/回滚终态、完成参数供 delete-range 使用等主语义。

对应关系中的实现形态差异包括：Go 通过 `model.GetTableColumnArgs`/`job.FillArgs` 管理版本化参数，Rust 在本文件显式处理 V1 数组与新结构 JSON；Go 将外键检查委托给 infoschema/owner helper，Rust 在启用外键时通过事务元数据遍历数据库和表；Go 的 masking policy 清理由 worker helper 封装，Rust 显式查询策略并逐条 DELETE。Go 在 `StateWriteOnly` 分支还有 `onDropColumnStateWriteOnly` failpoint，当前 Rust 文件没有对应 failpoint。

Go 测试中的 `pkg/ddl/db_change_test.go` 覆盖 WriteOnly、DeleteOnly、DeleteReorganization 等可见行为，`pkg/ddl/column_type_change_test.go` 提供 origin default 相关语义背景。Rust 侧未发现与 `persistent_drop_column.rs` 同名的独立测试；相关但非完整状态机覆盖分布在 `pkg/ddl/delete_range_test.rs`（DropColumn 索引范围）、`pkg/ddl/rollingback_test.rs`（开始删除后的取消语义）、`pkg/ddl/multi_schema_change_test.rs`（多 schema 删除列组合）以及 `pkg/ddl/tests/serial/serial_test.rs` 等 SQL 路径测试。

## 扩展指南

- 新增删除限制时，优先接入 `check_column`，选择与 Go 相同的错误类型，并同步 SQL 提交前检查与 owner 执行期检查，防止作业排队期间元数据变化绕过约束。
- 改动状态机时必须维持“一次调用至多一个可见阶段”、schema version/diff/table 同事务更新、列位于末尾后才删除、相关索引 ID 在移除前保存等不变量；同时核对 Go `onDropColumn`。
- 改动参数格式时必须同时更新 `decode_args`、`save_args`、`finished_range_ids` 和 `delete_range.rs` 消费端，保留 V1 历史作业可恢复性，并为旧/新版本 JSON 添加独立 Rust 测试文件，不能把测试内嵌进生产源文件。
- 改动默认值算法时应覆盖 enum 空元素、表达式默认值、`CURRENT_TIMESTAMP`、timestamp UTC/datetime 本地时区及 0 到 6 位小数秒精度，并与 Go `generateOriginDefaultValue` 对照。
- 改动 masking policy 清理时要显式处理元数据事务与 SQL 操作并非同一回调事务的失败窗口，并验证重试不会错误删除无关策略。
- 最适合新增的直接回归文件是同目录独立的 `persistent_drop_column_test.rs`，并在 `lib.rs` 以 `#[cfg(test)] mod persistent_drop_column_test;` 接入；跨模块行为继续扩展 `delete_range_test.rs`、`rollingback_test.rs` 或现有 SQL 集成测试。按照仓库规则，Rust 测试不要放进本生产文件。
- 性能风险集中在外键检查遍历所有数据库/表，以及 masking policy 逐条 DELETE；新增扫描或 SQL 循环前应评估大 schema/大量策略场景。兼容风险集中在历史 `raw_args` 编码、错误码和 schema 状态顺序。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file pkg/ddl/persistent_drop_column.rs --offset 1 --limit 400` 读取目标文件 390 行并报告其被 12 个文件使用；`query` 确认 `step`、`finished_range_ids`、`check_column` 等符号。精确 caller/callee 查询对部分符号未返回边，因此调用接线又由下列源码检索核验。
- 目标源码：`pkg/ddl/persistent_drop_column.rs`，重点符号为 `DropColumnArgs`、`decode_args`、`finished_range_ids`、`check_column`、`origin_default`、`save_args`、`step`。
- Rust 接线与边界：`pkg/ddl/lib.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/persistent_masking_actions.rs`、`pkg/ddl/delete_range.rs`。
- crate 边界：`pkg/ddl/Cargo.toml` 的 `[package]`、`[dependencies]`、`[lib]` 与 `package.metadata.porting`。
- Go 对照：`pkg/ddl/column.go`；相关入口/清理证据还包括 `pkg/ddl/job_worker.go`、`pkg/ddl/delete_range.go`、`pkg/ddl/rollingback.go`。
- 测试证据：`pkg/ddl/delete_range_test.rs`、`pkg/ddl/rollingback_test.rs`、`pkg/ddl/multi_schema_change_test.rs`、`pkg/ddl/tests/serial/serial_test.rs`，以及 Go 的 `pkg/ddl/db_change_test.go`、`pkg/ddl/column_type_change_test.go`。未运行 Cargo，未声称这些测试在本任务中执行；本任务按计划只做文档结构与事实核对。
