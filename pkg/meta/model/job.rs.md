# `pkg/meta/model/job.rs`

## 文件定位

[`job.rs`](job.rs) 定义 AsterSQL Rust 侧 DDL Job 的持久化协议、状态判断、参数缓存、multi-schema 子任务代理和完成态历史快照。它位于 SQL DDL 提交、持久化、调度、执行与历史展示的共同模型边界：上游把一次 DDL 组织为 `Job`，元数据层把它编码进 DDL job 表，执行器根据 `ActionType`、`JobState` 与 `SchemaState` 推进状态，完成时再把最终 DB/Table 快照写入 `HistoryInfo`。

本文件并非由顶层 [`lib.rs`](lib.rs) 直接声明。`pkg/meta/model/internal/group3/lib.rs` 在私有 `job` 模块中用 `include!("../../job.rs")` 纳入源码，注入 `JobArgs`、`FinishedJobArgs`、`DDLReorgMeta`、`SchemaState`、DB/Table 类型和若干适配模块，再用 `pub use job::*` 公开；顶层 `astersql-meta-model` crate 通过 `group_3` 再导出该 API。顶层 [`Cargo.toml`](Cargo.toml) 只依赖四个内部 group crate，实际编译本文件的 `astersql-meta-model-group3` 在 `internal/group3/Cargo.toml` 声明 `group-1`、`serde`、`serde_repr` 与 `serde_json`。

目标文件没有条件编译模块；唯一测试门控行为是 `update_job_args_for_test` 内部的 `cfg!(test)`。文件内没有 trait 定义，`JobArgs` 与 `FinishedJobArgs` 由 group3 模块入口提供。

## 核心职责

1. 固定 DDL 动作编号和展示名：`ActionType = u8` 及 `ACTION_*` 常量是跨版本持久化协议，46、48 是 tombstone，66 留空，`[200, 256)` 留给下游 fork。
2. 表达 Job 生命周期：`JobState` 是持久化 FSM，`Job` 上的谓词集中定义完成、回滚、暂停、恢复、同步、可暂停、可修改和可回滚边界。
3. 维护 V1/V2 参数协议：V1 将参数保存为无类型 JSON 数组，V2 保存单个有类型 JSON 对象；`args`/`SubJob::args` 是内存缓存，`raw_args` 才进入 JSON wire。
4. 定义 JSON wire 兼容层：私有 `JobWire` 控制字段名、缺省值、跳过字段及 `raw_args` 的嵌入 JSON 形态，并在运行态锁保护字段和持久化 `DDLReorgMeta` 之间搬运 warnings。
5. 支持 multi-schema change：`SubJob` 保存每个子变更的执行状态，`to_proxy_job`/`from_proxy_job` 在父 Job 与可执行代理 Job 之间映射字段，`MultiSchemaInfo` 保存子任务和执行期冲突集合。
6. 为调度依赖提供涉及对象集合：`InvolvingSchemaInfo` 描述 database/table、placement policy 或 resource group，名称规范化和合法性检查防止依赖 key 大小写不一致或对象类型混合。
7. 保存完成态元数据：`HistoryInfo` 记录 schema version 与最终 DB/Table 快照；`SchemaDiff` 描述一个 schema version 的增量，供 infoschema 增量刷新使用。

## 主要符号

- `ActionType`、`ACTION_*` 与 `action_type_string`：公开的动作编号协议和展示映射。未知值、`ACTION_NONE` 及未列入分支的值显示为 `"none"`；常量编号不能重排或复用 tombstone。
- `MODIFY_TYPE_*` 与 `modify_type_to_string`：描述 modify-column 是否仅改元数据、需要范围检查、仅重组索引、重组行与索引或处于预检查；数值 6 因旧版兼容不能使用。
- `JobVersion::{V1,V2}`、`JOB_VER_IN_USE`、`set_job_ver_in_use`、`get_job_ver_in_use`、`init_job_version`：进程级默认协议版本。原子变量以 Release/Acquire 存取；读取值仅把 `2` 解释为 V2，其余回退 V1。
- `JobMutable`：由 `Mutex` 保护 `row_count`、`warnings` 和 `warning_counts`。这些运行期可变值通过 `Job` 的 getter/setter 访问。
- `Job`：核心 DDL 载体。身份和路由字段包括 `id`、`tp`、schema/table ID 与名称；生命周期字段包括 `state`、`schema_state`、时间戳、错误、依赖 ID 和序号；协议字段包括 `version`、`raw_args`、`reorg_meta`、`multi_schema_info`；调度上下文还包括优先级、涉及对象、暂停/恢复原因、trace、SQL mode、session vars、CDC/BDR 信息和 RU。
- `JobWire`：私有 serde 结构。用 `type`、`err`、`err_count`、`binlog` 等字段名保持 Go JSON 兼容，`#[serde(default)]` 允许旧数据缺字段，空涉及对象/session vars 和空 pause/resume reason 可省略，零 RU 不序列化。
- `raw_json::{serialize,deserialize}`：让 `Vec<u8>` 中的 JSON 作为 JSON 值嵌入，而不是字节数组；空字节编码为 `null`，非法 JSON 在编码时返回 serde 错误。
- `Job::{fill_args,fill_finished_args,encode,decode,decode_args_v1,clear_decoded_args}`：参数缓存与 JSON 持久化主 API。`encode(true)` 才把内存 args 刷到 raw args；`decode` 不主动恢复类型化 args。
- `Job::{is_pausable,is_alterable,may_need_reorg,is_rollbackable}`：DDL 管理和执行决策的集中规则。其中 rollback 分支同时依赖动作类型和当前 `SchemaState`，multi-schema 则依赖 `MultiSchemaInfo::revertible`。
- `Job::{get_involving_schema_info,normalize_involving_schema_info,check_involving_schema_info}`：生成、规范化和校验调度依赖对象。`*` 表示全部，空字符串表示未涉及。
- `SubJob` 与 `to_proxy_job`/`from_proxy_job`：将子任务自己的动作、状态、参数、reorg 进度映射到继承父 Job 调度/会话上下文的代理 Job，执行后只把子任务级进度写回。
- `MultiSchemaInfo` 与 `new_multi_schema_info`：保存 `sub_jobs`、整体可回滚标记和序号；`add/drop/modify_columns`、索引集合、外键和相对/定位列为 serde 跳过的执行期冲突检测数据。构造函数使新对象默认 `revertible = true`。
- `JobState`、`str_to_job_state`、`AdminCommandOperator`、`JobPauseReason`、`JobResumeReason`：持久化状态和管理命令来源。未知状态字符串回退 `None`；pause/resume 原因 wire 使用字段名 `type`。
- `SchemaDiff`、`AffectedOption`：描述一次 schema version 变化涉及的主/旧 schema/table、子动作和附加对象，以及是否需重建 schema map、回源 meta 或执行 refresh-meta。
- `HistoryInfo`：保存最终 DB/Table 快照；`add_db_info`、`add_table_info`、`set_table_infos` 都同步 schema version，`clean` 清空全部历史内容。
- `JobW` 与 `new_job_w`：同时携带已解析 `Job` 和数据库原始字节，不自行编码或持久化。

## 执行流程

典型 DDL Job 从提交到完成经过以下模型流程：

1. 提交侧创建 Job，设置 `tp`、schema/table 标识、查询文本、协议版本和涉及对象；`pkg/ddl/jobsubmit/submit.rs` 在入队前调用 `normalize_involving_schema_info`，随后调用 `check_involving_schema_info` 拒绝不合法依赖描述。
2. 参数类型实现 group3 的 `JobArgs`/`FinishedJobArgs`。`fill_args` 或 `fill_finished_args` 根据 `version` 生成 V1 数组或 V2 单对象，暂存在 `args`。
3. `encode(true)` 调用 `marshal_args` 刷新父 Job 的 `raw_args`；已填充 `SubJob::args` 的子任务也被刷新。随后 `JobWire::from_job` 获取 row count 与 warnings 快照，把 warnings 写入浅拷贝的 `reorg_meta`，最后由 `serde_json` 编码 wire。
4. `pkg/meta/meta.rs`、`pkg/ddl/systable/manager.rs` 及 session runtime 将编码字节写入或读出 DDL job 表。`Job::decode` 先按 `JobWire` 解析，再由 `into_job` 恢复运行态；warnings 从 `reorg_meta` 迁回 `JobMutable`，`args` 保持为空，`need_reorg` 等跳过字段回到运行态缺省值。
5. 执行器依据 `tp` 分派 DDL 动作，并通过 `state`/`schema_state` 推进 online DDL。`is_rollbackable` 规定每类动作越过哪个 schema state 后不可撤销；`is_pausable` 还排除 TiFlash columnar index 的 write-reorg 阶段。
6. 需要数据重组时，`may_need_reorg` 为索引、物化视图、分区变更或显式标记的 modify-column 返回真。外部调度/提交代码用该结果选择 reorg 路径。
7. 普通完成路径调用 `finish_table_job`、`finish_multiple_table_job` 或 `finish_db_job`，同步 Job 状态、schema state、schema version 和最终元数据快照；调用者再负责持久化或写历史，本文件不直接进行数据库 I/O。

multi-schema change 的子流程是：调度器从 `MultiSchemaInfo::sub_jobs` 选择子任务，`SubJob::to_proxy_job` 继承父 Job 的身份、时间、依赖、query、版本、优先级、会话和 RU，同时覆盖子任务动作、参数、状态、row count 与 reorg 阶段；执行结束后 `from_proxy_job` 只回写子任务的 schema/reorg/告警/参数和进度。`pkg/ddl/delete_range.rs` 直接用 `to_proxy_job` 判断和生成子任务 GC 范围。

## 数据与状态

`ActionType`、`JobState`、`JobVersion`、`AdminCommandOperator` 及 `InvolvingSchemaInfoMode` 都使用显式整数表示并参与持久化，新增枚举值只能追加兼容编号。动作 46/48、空缺 66 和 `[200,256)` 保留区尤其不能复用。`JobState` 的业务集合包括未开始、排队、运行、取消中、回滚中、回滚完成、完成、同步、暂停中和暂停；不同辅助谓词故意使用不同集合：

- `is_finished` 是 Done、RollbackDone 或 Cancelled；
- `in_final_state` 是 Synced、Cancelled 或 Paused，明确不把 RollbackDone 当作真正 rollback-synced；
- `not_started` 是 None 或 Queueing；
- `is_resumable` 只接受 Paused。

参数存在两层状态。`args`/`SubJob::args` 是不序列化的解码或待编码缓存，`raw_args` 是 wire 字段。V1 空参数编码为 JSON `null`，非空为数组；V2 取第一个元素作为单对象，空值同样为 `null`，且仅以 `debug_assert!` 检查不超过一个元素。`decode_args_v1` 只按 JSON 值数组恢复缓存，不负责把每项反序列化为具体参数类型。

row count 和 warnings 是 `Job` 内允许通过共享引用更新的状态，由单个 `Mutex<JobMutable>` 保护。序列化时取得快照，warnings 通过 `DDLReorgMeta` 落盘；若没有 reorg meta，warnings 没有独立 wire 字段。Mutex 中毒会因 `unwrap()` 触发 panic。

涉及对象有三种互斥类别：database/table、placement policy、resource group。每个 `InvolvingSchemaInfo` 必须且只能设置一种类别；database 与 table 必须同时非空，`database == "*"` 时 table 也必须是 `"*"`。未显式提供列表时，`get_involving_schema_info` 从 Job 名称生成一个条目，并把有 schema、无 table 的情况转换为 `schema.*`。规范化保留 `*` 与空哨兵，其他名称转小写。

## 依赖与调用关系

向下依赖分为三层：

- 标准库：`AtomicI64` 保存进程默认 Job 版本，`Mutex` 保护进度与 warnings，`Arc` 共享最终 DB/Table 快照，`HashMap` 保存 warnings 和 session variables。
- group3 Cargo 依赖：`serde`/`serde_repr` 派生结构和整数枚举协议，`serde_json` 实现 Job、参数和 raw JSON 编解码；`group-1` 提供正式 `DBInfo`、`TableInfo`、`SchemaState` 与 parser AST 身份。
- `internal/group3/lib.rs` 提供的同 crate 接口：`JobArgs`、`FinishedJobArgs`、`DDLReorgMeta`、`ReorgType`、`ReorgStage`、时间转换和 `errors`、`terror`、`mysql`、`tracing`、`kerneltype` 适配模块。

主要上游和消费关系如下：

- `pkg/ddl/jobsubmit/submit.rs` 在提交前调用涉及对象规范化/校验，并用 `may_need_reorg` 形成提交元数据。
- `pkg/meta/meta.rs` 的 Job 更新、读取和历史扫描调用 `Job::encode`/`Job::decode`；`pkg/ddl/systable/manager.rs` 从系统表字节恢复 Job。
- `pkg/session/runtime/crossks_owner.rs`、`modify_column_dist_backfill.rs`、`mlog_purge.rs` 等跨 keyspace/分布式执行代码解码和回写 Job。
- `pkg/ddl/persistent_actions.rs` 以及 create/drop/modify-column、物化视图和 masking 等持久化动作调用完成态方法写 `HistoryInfo`。
- `pkg/ddl/table_mode.rs` 和 `pkg/ddl/job_worker.rs` 用 encode/decode 做持久化模型边界转换，并查询 `is_rollbackable`。
- `pkg/ddl/delete_range.rs` 对 multi-schema sub-job 调用 `to_proxy_job`，以复用普通 Job 的 finished-args/GC 判断。
- `pkg/ddl/notifier/events.rs` 使用 `action_type_string` 和动作常量生成 DDL 通知事件。

RustCodeGraph 文件节点报告本文件被 25 个文件使用，列出的代表包括 `pkg/ddl/delete_range.rs`、`pkg/ddl/jobsubmit/types.rs`、`pkg/ddl/normal_policy.rs`、`pkg/ddl/notifier/events.rs`、`pkg/ddl/persistent_actions.rs`。本次对关键 Rust 方法执行的 `callers/callees` 没有返回边；上述边由限定 `.rs` 文件的精确调用检索补证，不能据图边为空推断方法未使用。

## 错误处理与边界

`encode`、`decode`、`decode_args_v1`、`clone_job` 和 `marshal_args` 传播 `serde_json::Error`。编码时 `raw_json::serialize` 会先解析 `raw_args`，所以非空但不是合法 JSON 的字节会使整个 Job 编码失败；解码时缺失字段因 `JobWire #[serde(default)]` 使用 Rust `Default`，损坏 JSON 则返回错误。`check_involving_schema_info` 返回带稳定说明文本的 `Result<(), String>`，供提交链直接拒绝不合法依赖关系。

若调用 `encode(false)`，即使 `args` 已改变也不会同步 `raw_args`；这是有意协议，调用者必须明确决定是否更新参数。反之，`encode(true)` 会修改 Job 本身，且只更新 `args` 非空的 sub-job。`clone_job` 使用 encode(true)+decode，因此会刷新源 Job 的 raw args，并只复制 JSON 可见状态；sub-job 的私有 `job_args` 之后显式恢复。`need_reorg`、执行期冲突集合和解码缓存均不通过 wire 克隆。

`is_rollbackable` 是安全边界而非一般状态判断：例如 drop index/primary key 进入 DeleteOnly/DeleteReorganization/WriteOnly 后不可回滚，drop schema/table/column 等只在 Public 可回滚，modify-column 和分区重组到 Public 后不可回滚，multi-schema 服从整体 `revertible`。增加动作却不补分支会落入默认 `true`，因此新动作的可逆阶段必须显式审查。

当前 group3 编译边界包含适配实现：`errors::ErrorID`/`terror::Error` 是简化类型，`mysql::SQLMode` 是 `u64`，`ts_convert_to_time` 是恒等转换，`kerneltype::is_next_gen()` 恒为 false。`init_job_version` 对应 Go 包初始化逻辑，但精确 Rust 检索未发现生产调用者；静态值自身默认为 V1，因此 classic 行为成立，不能据源码宣称 Rust 已自动在 NextGen 启动时切到 V2。

## 并发与资源生命周期

`JOB_VER_IN_USE` 是进程级静态原子值，生命周期持续到进程退出；写入使用 Release、读取使用 Acquire。它只保存一个整数，不提供版本协商，集群滚动升级时何时切换由外层 DDL 启动逻辑负责。

每个 `Job` 的 `mutable: Mutex<JobMutable>` 独立保护 row count 和两张 warning map。`set_warnings` 在一个临界区同时替换 warnings 与 counts，`get_warnings` 在同一锁下克隆两者，避免读到不同批次；`JobWire::from_job` 先取得 warnings 快照，再另行调用 `get_row_count`，二者不是跨字段原子快照。锁没有显式清理，随 Job 释放；panic 持锁会导致后续访问因中毒而 panic。

`HistoryInfo` 中 DB/Table 使用 `Arc`，clone 和 `JobWire::from_job` 共享底层不可变模型对象；`set_table_infos` 克隆 Arc 列表而非深拷贝表。`SubJob::to_proxy_job` 会克隆字符串、参数、session vars 和涉及对象，构造独立 `Mutex`；`from_proxy_job` 再克隆回子任务，不共享可变进度锁。

本文件不启动线程或异步任务，不创建通道、事务或 I/O 资源，也不负责 DDL owner/worker 生命周期。`JobW` 只是内存包装，不拥有数据库句柄或清理协议。

## 与 Go 版本的对应关系

直接对照是 [`job.go`](job.go)，测试对照是 [`job_test.go`](job_test.go)。Rust 的动作编号、展示文本、modify type、Job 字段、FSM 谓词、rollback 分支、multi-schema 代理字段、涉及对象约束和 `HistoryInfo` 操作整体逐项对应 Go。

主要实现差异如下：

- Go `Job` 直接靠 JSON tags 编解码；Rust 用私有 `JobWire` 隔离 `Mutex` 和运行期缓存，并显式在 `reorg_meta` 与 `JobMutable` 间迁移 warnings。
- Go `json.RawMessage` 对应 Rust `Vec<u8> + raw_json`；Rust serializer 先解析再嵌入 JSON，保持 wire 是 JSON 值而不是数字数组。
- Go V1 `decodeArgs(args ...any)` 可按调用者传入的具体目标逐项解码并容忍多余 raw 参数；当前 `job.rs::decode_args_v1` 只恢复 `Vec<serde_json::Value>`，具体类型转换由 `job_args.rs`/group2 兼容层承担。
- Go `Clone()` 遇到编解码失败会通过 `errors.Trace` 后 panic；Rust `clone_job` 返回 `Result`。两边都以 wire round trip 排除运行期字段，并额外恢复 sub-job 的私有 JobArgs。
- Go `JobVersion` 是开放的整数新类型，`String` 可输出 `unknown(n)`；Rust 是只含 V1/V2 的 repr 枚举，Display 只有两种分支，未知 JSON 枚举值会反序列化失败。
- Go 包 `init()` 自动按 kernel 类型设置默认协议版本；Rust 暴露 `init_job_version()`，但当前未发现生产调用点，且 group3 的 `kerneltype` 是恒 classic 的适配桩。
- Go `JobW` 嵌入 `*Job`；Rust `JobW` 按值拥有 `Job`。Go 的 `TimeZoneLocation` 仍在 `job.go`，Rust 已由 group1 的正式类型再导入，不在本文件重复定义。
- Go 的 `sync.Mutex` 字段与 Rust `Mutex<JobMutable>` 都不序列化；Rust API 允许 `&self` 更新受锁字段，语义对应 Go 指针 receiver。

独立 Rust 测试 [`job_test.rs`](job_test.rs) 覆盖 codec、旧 wire 缺省值、clone、proxy 映射、状态集合、V2 参数、涉及对象约束和暂停/恢复 wire；`go_merge_15_test.rs` 另行覆盖物化视图回滚边界。测试不是内嵌在生产源文件中。

## 扩展指南

新增 DDL 动作时，应先在 Go 与 Rust 中分配相同且未保留的 `ActionType` 数值，再同步 `action_type_string`、BDR 分类、参数类型、执行分派、`may_need_reorg` 与 `is_rollbackable`。尤其不能只增加常量：默认 rollbackable=true 可能让不可逆动作被错误取消，遗漏 reorg 分类则可能走错调度路径。同步更新独立 Rust 测试 `job_test.rs` 或同目录专项测试，并保留 Go `job_test.go` 的协议覆盖。

新增持久化字段时，至少同时修改 `Job`、`Default`、`JobWire`、`JobWire::from_job` 和 `JobWire::into_job`，明确 serde 默认值、旧版本缺字段行为、空值是否省略，以及 proxy job 是否继承/回写该字段。若字段属于 sub-job，还要同步检查 `SubJob::to_proxy_job`、`from_proxy_job`、`clone_sub_job` 和 `test_job_size` 的字段契约。不可把运行期缓存误加到 wire。

扩展参数协议时，应实现 `JobArgs`/`FinishedJobArgs` 的 V1 数组布局与 V2 对象布局，并同步 `job_args.rs` 及其独立测试。调用 `encode(true)` 前确保 args 已填充；若 V2 可能含多个对象，必须先改变协议和 `marshal_args`，不能依赖 release 构建中不生效的 `debug_assert!`。

调整状态机时，应把 `is_finished`、`in_final_state`、pause/resume、rollback 和 display/parse 作为一组审查。新增 `JobState` 或整数枚举值必须保持向后编号兼容，并验证旧 JSON、未知值处理和 system-table 读取行为。

涉及对象规则直接影响 DDL 并发排序。新增对象类别需要同步 `InvolvingSchemaInfo`、规范化、对象类型计数、校验错误、提交器依赖 key 构造和 `job_test.rs` 的合法/非法矩阵；兼容风险是作业错误并发或永久等待。性能风险主要来自频繁 encode 时克隆大型 query、args、warnings、session vars 和 table snapshots，以及热路径锁竞争；新增大字段应评估 wire 大小和 clone 成本。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件（7,032 个 Rust、4,415 个 Go）；`files --filter pkg/meta/model/job.rs` 确认目标文件 1,433 行、230 个符号且被 25 个文件使用；`node --file` 分段读取源码；`query` 定位 `encode`、`is_rollbackable`、`to_proxy_job`、`check_involving_schema_info` 和 `init_job_version`。精确 `callers/callees` 对这些 include 模块方法未返回边，因此调用关系由限定 Rust 文件的精确检索补证。
- 源码与 crate 装配：`pkg/meta/model/job.rs`、`pkg/meta/model/internal/group3/lib.rs`、`pkg/meta/model/internal/group3/Cargo.toml`、`pkg/meta/model/lib.rs`、`pkg/meta/model/Cargo.toml`。
- 直接 Rust 调用证据：`pkg/ddl/jobsubmit/submit.rs`、`pkg/meta/meta.rs`、`pkg/ddl/systable/manager.rs`、`pkg/ddl/delete_range.rs`、`pkg/ddl/table_mode.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/session/runtime/crossks_owner.rs`、`pkg/session/runtime/modify_column_dist_backfill.rs`。
- Rust 独立测试：`pkg/meta/model/job_test.rs`（完整 Job 协议和边界矩阵）、`pkg/meta/model/job_3_aster_unit_test.rs`（动作/状态/回滚、wire、warnings 与涉及对象抽查）、`pkg/meta/model/go_merge_15_test.rs`（物化视图动作与回滚边界）、`pkg/meta/model/dependency_tests.rs`（独立 include 编译依赖验证）。
- Go 对照：`pkg/meta/model/job.go` 与 `pkg/meta/model/job_test.go`；逐项核对动作编号、Job 字段与 tags、V1/V2 参数、状态谓词、rollback 分支、SubJob 映射、涉及对象、HistoryInfo 和初始化逻辑。
- 人工复核：确认文档能回答文件为何存在、如何编译和运行、持久化/运行态边界、锁与原子状态、主要调用者以及安全扩展点；明确记录 graph 调用边缺失、group3 适配桩和 Rust 初始化调用未发现，没有把预期设计写成已接线事实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并核对最终变更只包含本说明文档以及完成后删除编号任务文件，未修改 Rust、Go、Cargo 或只读 `plan.md`。
