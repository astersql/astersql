# [`pkg/meta/model/job_args.rs`](./job_args.rs)

## 文件定位

`job_args.rs` 是 DDL `Job` 参数的持久化兼容层：它把各类 schema、table、partition、column、index、placement、TiFlash、flashback 与物化视图操作的参数定义为类型化 DTO，并维护旧版位置数组与新版 JSON 对象之间的双向转换。文件本身不执行 DDL，也不访问存储；它负责让提交端、DDL worker、回滚路径和任务完成后的清理/GC 消费者对同一份 `Job.raw_args` 有一致解释。

该文件实际编译在 `astersql-meta-model-group2` 中：`pkg/meta/model/internal/group2/lib.rs` 先提供依赖类型、`JobArgsCompat` 和解码适配，再以 `include!("../../job_args.rs")` 纳入本文件。根 crate `astersql-meta-model` 由 `pkg/meta/model/Cargo.toml` 聚合 group1–group4，并在 `pkg/meta/model/lib.rs` 以 `group_2` 模块再导出本层 API。因此外部 Rust 调用通常写成 `astersql_meta_model::group_2::Get*Args`。

源码已有 `// Copyright 2026 AsterSQL.` 标记，说明它已进入 AsterSQL Rust 移植范围；其直接 Go 对照是 `pkg/meta/model/job_args.go`，独立 Rust 回归测试位于 `pkg/meta/model/job_args_test.rs`，另有 `job_args_2_aster_unit_test.rs` 和 `go_merge_15_test.rs` 补充兼容边界。

## 核心职责

1. **版本分流。** `getOrDecodeArgs` 依据 `job.version` 把 V1 交给 `getOrDecodeArgsV1`，其他版本交给 `getOrDecodeArgsV2`。V1 是按动作约定字段顺序的 JSON 数组；V2 是带字段名的单个 JSON 对象。
2. **参数协议抽象。** `JobArgs` 定义普通参数的 V1 编码与解码，`FinishedJobArgs` 额外定义任务完成后写回给下游组件的 V1 布局。调用方应通过 group2 的 `JobArgsCompat::FillArgs` / `FillFinishedArgs` 填充参数，而不是直接拼数组。
3. **动作相关兼容。** 同一结构可按 `job.tp`、`job.state` 或回滚状态选择不同位置协议，例如 `TablePartitionArgs`、`ModifyIndexArgs`、`ModifySchemaArgs` 和 `CreateTableArgs`。
4. **历史数据兼容。** 解码逻辑接受单项/批量索引布局、旧物化视图字段数、可缺省尾部字段、历史占位项及 V1 中未持久化的 V2-only 字段。
5. **完成态载荷。** 删除、截断、分区、索引、改列等动作结束后仍需由 delete-range、GC 或后续阶段读取 ID/键范围，本文件以独立 finished 解码入口保留这部分协议。

## 主要符号

- 基础层：`DynArg = serde_json::Value` 表示 V1 的动态槽位；`JobArgResult<T>` 统一返回 group2 的 `errors::Error`；`arg` 将字段转为 JSON 值，当前序列化失败会降级为 `Value::Null`；`is_default` 供 `serde(skip_serializing_if)` 省略默认字段。
- 编解码入口：`getOrDecodeArgsV1`、`getOrDecodeArgsV2`、`getOrDecodeArgs`。V1 调用具体类型的 `decodeV1` 后把 `raw_args` 数组缓存进 `job.args`；V2 优先从唯一缓存对象反序列化，否则解析 `raw_args` 并缓存原始 JSON 对象。
- 协议 trait：`JobArgs::{getArgsV1, decodeV1}` 和 `FinishedJobArgs::getFinishedArgsV1`。`impl_simple_args!` 只生成字段顺序固定的样板实现；复杂动作仍显式实现分支。
- schema/table 生命周期：`CreateSchemaArgs`、`DropSchemaArgs`、`ModifySchemaArgs`、`CreateTableArgs`、`BatchCreateTableArgs`、`DropTableArgs`、`TruncateTableArgs`、`RecoverArgs`、`RecoverTableInfo`、`RecoverSchemaInfo`、`RenameTableArgs`、`RenameTablesArgs`。
- partition 与 placement：`TablePartitionArgs`、`ExchangeTablePartitionArgs`、`AlterTablePartitionArgs`、`AlterTablePlacementArgs`、`PlacementPolicyArgs`、`MaskingPolicyArgs`、`AlterTableAttributesArgs`、`AlterTableAffinityArgs`、`AlterTableSetRegionSplitPolicyArgs`。
- column/index/constraint：`TableColumnArgs`、`ModifyColumnArgs`、`SetDefaultValueArgs`、`IndexArg`、`IndexArgSplitOpt`、`ModifyIndexArgs`、`AddForeignKeyArgs`、`DropForeignKeyArgs`、`AddCheckConstraintArgs`、`CheckConstraintArgs`。`IndexOp` 及 `OpAddIndex`、`OpDropIndex`、`OpRollbackAddIndex` 区分完成态布局。
- 运行与存储属性：`RebaseAutoIDArgs`、`ModifyTableAutoIDCacheArgs`、`ShardRowIDArgs`、`AlterTTLInfoArgs`、`SetTiFlashReplicaArgs`、`UpdateTiFlashReplicaStatusArgs`、`LockTablesArgs`、`AlterTableModeArgs`、`ModifyTableEngineAttributeArgs`、`FlashbackClusterArgs` 与复制自 kv 语义的半开区间 `KeyRange`。
- 物化视图扩展：`CreateMaterializedViewLogArgs`、`CreateMaterializedViewArgs`、`AlterMaterializedViewRefreshArgs`、`AlterMaterializedViewAttributesArgs`、`AlterMaterializedViewLogPurgeArgs`、`RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs`。
- 特殊辅助入口：`FillRollbackArgsForAddPartition` 和 `FillRollBackArgsForAddColumn` 重写回滚参数；`GetFinished*Args` 系列读取完成态；`GetDropIndexArgs` 单独处理 V1 drop-index；`IndexArg::GetColumnarIndexType` 把旧 `IsColumnar=true` 且无新枚举值的载荷解释为向量索引。

当前文件共有 58 个公开结构体、63 个公开函数、29 个显式 `JobArgs` 实现、6 个 `FinishedJobArgs` 实现以及 22 个 `impl_simple_args!` 调用。宏生成的实现也是协议的一部分，审查字段顺序时不能只搜索显式 `impl JobArgs`。

## 执行流程

普通参数主流程如下：

1. DDL 提交或状态推进代码构造某个 `*Args`，通过 `JobArgsCompat::FillArgs` 写入 `job.args`。V1 调用该类型的 `getArgsV1` 生成位置数组；V2 用 serde 生成一个类型化 JSON 对象。
2. `JobArgsCompat::Encode(true)`（定义在 `internal/group2/lib.rs`，底层复用 group3 `Job::encode`）把缓存写入 `job.raw_args`，随 Job 持久化或传递。
3. worker 或后续动作调用 `Get*Args(&mut Job)`。这些门面通常以默认值调用 `getOrDecodeArgs`，再由版本分流。
4. V1 的 `decodeV1` 按历史位置读取；缺失尾部槽位由 group2 `DecodeArgs` 保持目标默认值。某些类型先创建 `Box::default()`，保证成功后指针字段可用。V2 依字段名反序列化，`#[serde(default)]` 与 `skip_serializing_if` 支持缺省字段。
5. V2 首次解析后在 `job.args` 缓存一个 JSON 对象；再次读取优先使用缓存。`job_args_2_aster_unit_test.rs::v2_decodes_json_object_then_reuses_typed_cache` 证明即使之后破坏 `raw_args`，缓存命中仍返回原值。

完成态走另一条路径：worker 以 `FillFinishedArgs` 调用 `FinishedJobArgs::getFinishedArgsV1`（V2 仍保存完整对象），下游使用 `GetFinishedDropSchemaArgs`、`GetFinishedDropTableArgs`、`GetFinishedTruncateTableArgs`、`GetFinishedTablePartitionArgs`、`GetFinishedModifyIndexArgs` 或 `GetFinishedModifyColumnArgs` 解码。`pkg/ddl/delete_range.rs` 直接消费多种 finished 结果，用旧分区/表/索引 ID 生成清理范围。

典型特殊分支包括：

- `TablePartitionArgs::decodeV1` 按 add/drop/reorganize 与 rolling-back 状态切换 `PartNames`/`PartInfo` 布局；`GetTablePartitionArgs` 最终保证 `PartInfo` 非空。`FillRollbackArgsForAddPartition` 借一个 drop-partition 假 Job 生成符合目标版本的回滚载荷。
- `ModifyIndexArgs` 分别处理 rename、drop、add、primary key、columnar index；解码 add/drop 时先尝试批量布局，再退回历史单索引布局。完成态又按 add、rollback-add 和 drop 三类输出不同字段。
- `ModifyColumnArgs::getArgsV1` 基础布局固定为五项，仅当 `ChangingColumn` 存在时追加四项；完成态另写 `IndexIDs`、`PartitionIDs`、`NewIndexIDs`。
- `FlashbackClusterArgs` 在 V1 中将多个布尔开关编码为 `"ON"`/`"OFF"` 字符串，解码时恢复布尔值。
- `AlterMaterializedViewAttributesArgs::decodeV1` 先尝试三个字段，失败后回退到两个字段并令 `AlertRefreshFailed=false`，用于旧任务兼容。

## 数据与状态

持久状态集中在 `Job` 的三项：`version` 决定协议，`raw_args: Vec<u8>` 是持久化字节，`args: Vec<Value>` 是进程内缓存。V1 缓存表现为零到多个位置值；V2 必须恰好一个完整对象，`getOrDecodeArgsV2` 对非空且长度不为 1 的缓存执行 `intest::Assert`。

DTO 普遍派生 `Clone + Debug + Default + PartialEq + Serialize + Deserialize`，并使用 `#[serde(default)]` 接受旧 JSON 缺字段。显式 `rename` 保持 Go 的 snake_case JSON 键；默认值通常不写出。`#[serde(skip)]` 字段（例如 `TruncateTableArgs` 的运行期策略列表、`RenameTableArgs::OldSchemaIDForSchemaDiff`、`IndexArg::Global`、`ModifyIndexArgs::OpType`、`ModifyColumnArgs::ChangingIdxs`）不会进入 V2 JSON，调用方不能期待跨持久化边界保留它们。

V1 的字段顺序、数量和动作条件本身就是磁盘/集群兼容协议。例外占位也必须保留：add-primary-key 第六项固定为 `null`；resource-group create 的第二项固定写无用的 `false`；truncate-table 会附加只供提交端计算的第四项，而执行端只解前三项。

本文件不拥有数据库事务、schema state machine 或物理数据；它只转换值并在传入的 `&mut Job` 上更新 `args`/`raw_args`。业务元数据类型来自 group2 周边声明，其中 `Job`、版本、状态与动作编号复用 group3 的正式生产身份。

## 依赖与调用关系

下游直接依赖只有 `serde`、`serde_json::Value`、`std::collections::HashMap`，以及 include 环境提供的 `Job`、动作常量、AST/MySQL/PD/模型 DTO、`errors` 和 `intest`。`pkg/meta/model/Cargo.toml` 本身仅声明 group1–group4 路径 crate；serde 由 group2 crate 暴露给 include 文件。

主要上游调用来自：

- `pkg/ddl/persistent_create_table.rs`、`persistent_create_materialized_view*.rs`、`persistent_actions.rs`、`persistent_masking_actions.rs` 等 worker/持久动作，通过 `Get*Args` 恢复动作输入；解码错误通常映射为 cancel 或带上下文的 DDL 错误。
- `pkg/ddl/job_worker.rs` 及多阶段 persistent action 在状态推进时反复读取或保存参数。
- `pkg/ddl/delete_range.rs` 使用 finished 参数中的旧对象 ID、分区 ID、索引 ID等安排后续清理，是完成态参数存在的主要理由之一。
- session 的 normal-DDL 路径构造并编码 Job；对应测试如 `pkg/session/runtime/normal_ddl_create_table_test.rs` 通过 `group_2::GetCreateTableArgs` 验证队列/历史 Job 中的载荷。

RustCodeGraph 将该文件标记为被 25 个文件使用，列出的直接使用者包括 `pkg/ddl/delete_range.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/persistent_alter_materialized_view_attributes.rs` 和 `pkg/ddl/persistent_alter_materialized_view_log_purge.rs`。精确调用搜索还确认 `persistent_create_table.rs` 调用 `GetCreateTableArgs`/`GetBatchCreateTableArgs`，`persistent_actions.rs` 调用 schema/table/index/TTL 等多个门面，`delete_range.rs` 调用多种 `GetFinished*Args`。

## 错误处理与边界

- JSON 解析、字段反序列化和 `job.decodeArgs` 错误经 `errors::Trace` 转成 `JobArgResult`；非法索引动作在 `ModifyIndexArgs::decodeV1` 中以 `errors::Errorf("Invalid job type ...")` 返回。
- 版本前置条件、V2 缓存长度、add-partition 完成态及回滚辅助的动作条件使用 `intest::Assert`，违反不变量会 panic，而不是返回普通错误。
- `arg` 对序列化失败使用 `unwrap_or(Value::Null)`，因此编码端不会报告该错误；扩展包含自定义序列化器的类型时尤其要通过往返测试发现静默 `null`。
- 若 `raw_args == b"null"`，V1 将其视为空数组；group2 `DecodeArgs` 对缺失或显式 `null` 的槽位保留目标默认值。这提供向后兼容，但也意味着“字段缺失”和“默认值”通常不可区分。
- 多处代码基于非空集合索引，例如 `BatchCreateTableArgs::getArgsV1` 读取 `Tables[0]`，`ModifyIndexArgs` 读取 `IndexArgs[0]`/`[1]`，`GetFinishedModifyIndexArgs` 写 `out.IndexArgs[0]`。这些结构有由上游保证的最小长度不变量，传入畸形对象可 panic。
- 单项/批量索引 fallback 只覆盖已知历史布局；多个并行向量长度应保持一致，否则通过 `exists[i]`、`names[i]` 等索引组装时可能 panic。
- 当前 group2 的 `errors::Error` 是 `String` 适配层，`intest::Assert` 直接用 Rust `assert!`；这与 Go 的完整 errors/intest 类型体系并非同一错误身份，调用方应只依赖成功值或文本上下文，不应依赖 Go 错误类型断言。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务或外部句柄；所有转换均在调用线程同步完成。参数结构主要拥有 `String`、`Vec`、`HashMap` 和 `Box`，离开作用域后按 Rust 所有权释放，无显式资源清理。

并发安全边界在可变 Job：入口接收独占 `&mut Job`，所以单次编解码不会与另一 Rust 借用并发修改同一个缓存。V2 缓存是 Job 生命周期内的快照；若其他代码修改 `raw_args` 却不清空 `args`，随后读取会继续得到缓存值。这是测试明确验证的语义，不是自动一致性机制。跨节点或持久化边界只有 `raw_args` 生效，内存缓存不会共享。

大型参数的主要资源风险是克隆和 JSON 中间值：V2 读取会克隆缓存 `Value` 再反序列化，V1 读取会解析整个数组；批量建表、恢复 schema、批量索引与 flashback key ranges 的成本随载荷线性增长。`RecoverSchemaInfo::LoadTablesOnExecute` 的目的正是避免提交节点持久化过大的恢复表清单。

## 与 Go 版本的对应关系

`pkg/meta/model/job_args.go` 是权威语义对照。顶层声明对比显示 Go 与 Rust 的业务类型和 63 个顶层函数名称对齐；Rust 额外引入 `DynArg`、`JobArgResult`、`arg`、`is_default` 以适配 serde 和 `Result`。Rust 保留了 Go 风格公开名称（如 `Get*Args`、字段首字母大写），以降低逐文件移植与调用对照成本。

核心对应关系为：Go `[]any` ↔ Rust `Vec<Value>`，Go 泛型 `getOrDecodeArgs*` ↔ Rust 同名泛型函数，Go 私有 `JobArgs`/`FinishedJobArgs` 接口 ↔ Rust trait，Go `json.Unmarshal` ↔ serde 反序列化。Go V2 缓存持有类型化 `any`，Rust 当前缓存持有 JSON `Value`，所以 Rust 每次缓存命中仍会反序列化为目标类型；对外值语义一致，但类型错误表现为 serde 错误而非 Go 类型断言 panic。

独立 Rust 测试大体按 `job_args_test.go` 的测试名和场景移植，覆盖 schema/table/partition/index/column/constraint/placement/flashback 等 V1/V2 往返。Rust 还增加 `v2_args_decode_go_json_field_names` 验证 Go JSON 键、`job_version_and_action_survive_clone` 验证正式 Job 身份，并在 `job_args_2_aster_unit_test.rs` 验证 V2 缓存与旧 columnar-index fallback。`go_merge_15_test.rs` 进一步覆盖物化视图新增字段、旧两字段属性布局、V2-only TiFlash/index split 字段。

已知实现边界应如实保留：本文件所在 group2 仍为兼容适配 crate，周边若干 AST/PD/模型类型是满足参数协议的 Rust 表示；错误类型也是 `String`。文档不把这些边界描述成完整 Go 运行时等价物。

## 扩展指南

新增或修改 Job 参数时应按以下顺序处理：

1. 在本文件新增/修改 DTO，明确每个字段的 Go JSON 键、默认值、是否允许省略以及是否仅为运行期字段；可持久化字段不得误用 `#[serde(skip)]`。
2. 实现 `JobArgs`。只有所有动作都严格按同一字段顺序读写时才用 `impl_simple_args!`；否则按 `job.tp`/`state` 显式分支，并把 V1 顺序视为不可随意重排的兼容协议。
3. 若任务结束后仍需下游读取结果，实现 `FinishedJobArgs` 并提供对应 `GetFinished*Args`；同时检查 `pkg/ddl/delete_range.rs`、GC 或 worker 的实际消费点。
4. 提供 `Get*Args` 门面，避免业务调用者直接使用 trait 方法或手写 JSON。若存在回滚布局、单项/批量旧布局或历史占位，建立专用辅助函数而不是简化掉旧分支。
5. 同步 Go 对照 `pkg/meta/model/job_args.go` 的字段、顺序、动作分支与默认语义；若改的是 Go commit 对齐任务，只纳入该 commit 增量和不可缺少的局部接线。
6. 在独立测试文件 `pkg/meta/model/job_args_test.rs` 扩充普通态与完成态 V1/V2 往返；不要把测试写进生产源文件。涉及 group2 适配特性时同步 `job_args_2_aster_unit_test.rs`，涉及后续 Go merge 兼容时检查 `go_merge_15_test.rs`。

重点风险是持久化兼容而非算法复杂度：改变 V1 槽位顺序/数量、V2 JSON 键、默认值或 `serde(skip)` 会影响滚动升级和历史 Job；改变 finished 布局会破坏 delete-range/GC；放宽集合长度不变量可能引入 panic。性能上避免为大列表增加不必要的深拷贝或重复 JSON 往返。

## 验证依据

- 源码与符号：`pkg/meta/model/job_args.rs` 全部 2182 行；RustCodeGraph `files --filter pkg/meta/model/job_args.rs` 报告 210 个索引符号，`node --file ...` 给出源码与“used by 25 files”。`query getOrDecodeArgs --kind function --json` 定位 Rust 的 V1/V2/分流三个入口及同名 Go 对照。
- crate 边界：`pkg/meta/model/Cargo.toml`（`astersql-meta-model` 聚合 group1–group4）、`pkg/meta/model/lib.rs`（公开 `group_2` 再导出）、`pkg/meta/model/internal/group2/lib.rs`（复用 group3 Job 身份、定义 `JobArgsCompat`、`include!` 本文件）。
- Go 对照：`pkg/meta/model/job_args.go`；顶层类型/函数名称集合对比只显示 Rust 的四个适配辅助 `DynArg`、`JobArgResult`、`arg`、`is_default` 为额外项。
- 独立测试：`pkg/meta/model/job_args_test.rs`、`pkg/meta/model/job_args_2_aster_unit_test.rs`、`pkg/meta/model/go_merge_15_test.rs`，以及 Go 基准测试 `pkg/meta/model/job_args_test.go`。测试证明 V1/V2 普通态和完成态往返、分区回滚、索引历史单双布局、改列五/九槽位、旧物化视图布局、Go JSON 字段名和 V2 缓存行为。
- 直接调用证据：`pkg/ddl/persistent_create_table.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/persistent_masking_actions.rs`、`pkg/ddl/delete_range.rs`、`pkg/ddl/job_worker.rs` 及 session normal-DDL 测试中的 `group_2::Get*Args` 调用。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工核对只新增本说明、未修改 Rust/Go/Cargo/`plan.md`。
