# `br/pkg/restore/internal/prealloc_db/db.rs`

## 文件定位

`db.rs` 是 Cargo 包 `astersql-br-pkg-restore-internal-prealloc-db` 的主体实现。包入口 `br/pkg/restore/internal/prealloc_db/lib.rs` 通过 `#[path = "db.rs"] pub mod db` 挂载本文件，并用 `pub use db::*` 扁平导出其公开符号。对应的 `Cargo.toml` 将该包声明为 workspace 内的 library，Go 包映射为 `br/pkg/restore/internal/prealloc_db`，唯一直接 Cargo 依赖是相邻的 `astersql-br-pkg-restore-internal-prealloc-table-id`。

它对应 Go 文件 `br/pkg/restore/internal/prealloc_db/db.go`：位于 BR 快照恢复过程中“已有备份元数据”与“目标 TiDB session”之间，负责创建数据库、placement policy 和表，使用预分配 ID 改写表/分区 ID，并在建表后恢复 sequence、AUTO_INCREMENT 或 AUTO_RANDOM 元数据。

当前 Rust 接线必须与 Go 生产路径区分：根 `Cargo.toml` 把该 crate 纳入 workspace，但仓库内其他 Cargo manifest 尚未依赖它；`br/pkg/restore/snap_client/client.rs` 当前通过其本地 `stubs::PreallocDB` trait 调用 `RegisterPreallocatedIDs`、`CreateDatabase`、`CreateTables`、`CreateTable` 和 `ExecDDL`。因此，本文件是可独立验证的 Go 对齐移植，而不是已经由 Rust `snap_client` 直接实例化的生产实现。

## 核心职责

1. `NewDB` 从 `Glue` 获取 session，关闭 SQL mode 的兼容限制，并探测目标 TiDB 是否支持 `tidb_placement_mode`。
2. `DB::RegisterPreallocatedIDs` 保存由 `prealloc_table_id` 产生的 ID 映射；`rewrite_table_info` 在建表前按该映射替换表 ID 和分区 ID。
3. `DB::CreateDatabase`、`CreatePlacementPolicy`、`CreateTable` 和 `CreateTables` 将恢复元数据转换为 session 操作，并处理 placement policy、TTL 与已分配 ID 选项。
4. `DB::CreateTablePostRestore` 在对象创建后恢复 sequence 值，或仅对增量恢复中已存在的表校正 AUTO_INCREMENT/AUTO_RANDOM 基值。
5. `DB::ExecDDL` 重放 DDL job：CREATE SCHEMA/TABLE 走结构化 session API，其余 job 必要时先切库，再执行原始 query。
6. `DB::Close` 把连接释放委托给 session。

文件还内置了 `Context`、`Error`、`Storage`、`CIStr`、`model`、`metautil`、`Session` 和 `Glue` 等局部兼容定义。源码注释明确这些是为 Darwin 可移植性而设置的 stand-in，不能等同于完整的 TiDB `kv/domain/session` 实现。

## 主要符号

- `Error { msg, code }`：本地错误载体。`Equal` 在两边都有 code 时比较 code，否则比较消息；`errors::Trace` 当前保持原值，不增加堆栈或上下文。
- `Context`：以 `Arc<Mutex<Option<Error>>>` 保存取消原因；`Background` 创建空状态，`cancel` 写入，`Err` 克隆读取。当前 DB 主流程只把它透传给 session，没有主动轮询取消状态。
- `CIStr { O, L }`：保留原始拼写和小写形式；placement policy map 与批量建表分组使用 `L`，SQL/日志展示使用 `O`。
- `model::{DBInfo, TableInfo, PartitionInfo, PolicyInfo, Job, ...}`：本文件实际使用到的 Go `pkg/meta/model` 字段子集。`TableInfo::ClearPlacement` 同时清空表级和全部分区级 policy 引用。
- `utils::NeedAutoID`：无主键句柄/公共句柄时需要隐藏 row ID，或存在 auto-increment 列时需要 auto ID。`EncloseName` 用反引号包围标识符并把内部反引号加倍。
- `Session` / `BatchCreateTableSession` / `Glue`：外部边界。`Session` 提供 SQL、库表和 policy 操作；批量能力通过 `as_batch_create_table_session` 动态探测；`Glue::CreateSession` 允许 raw KV 模式返回 `None`。
- `DB { se, prealloced_ids }`：核心状态对象。源码直接声明其“不线程安全”；所有修改方法均要求 `&mut self`。
- `NewDB`：构造入口，返回 `(Option<DB>, support_policy)`；`None` 表示 raw KV 模式没有 SQL session。
- `rewrite_table_info`：私有 ID 改写桥。它把本地 `TableInfo` 压缩成 prealloc crate 的最小模型，调用 `PreallocIDs::RewriteTableInfo`，再把新表 ID 和逐项 zip 的分区 ID 写回完整克隆。
- `DB::ExecDDL`：DDL job 重放入口。
- `DB::{CreateDatabase, CreatePlacementPolicy, CreateTable, CreateTables}`：恢复对象创建入口。
- `DB::{restoreSequence, CreateTablePostRestore}`：建表后的元数据校正入口。
- `DB::{ensurePlacementPolicy, ensureTablePlacementPolicies}`：私有去重创建逻辑；policy 从共享 map 中移除后才创建，缺失即视为已创建。

## 执行流程

构造流程：

1. `NewDB` 调用 `Glue::CreateSession`；错误立即透传，`None` 则返回 `(None, false)`。
2. 对 session 执行 `set @@sql_mode=''`；失败时构造中止。
3. `policy_mode` 非空时执行 `set @@tidb_placement_mode='...';`。成功置 `support_policy = true`；只有 `ErrUnknownSystemVar` 被解释为旧目标不支持并忽略，其他错误仍中止。
4. 返回持有 session、尚未注册 `prealloced_ids` 的 `DB`。

数据库与 policy 流程：

1. `CreateDatabase` 在目标不支持 policy 时直接清空传入 `DBInfo.PlacementPolicyRef`。
2. 若仍有引用且提供了 policy map，先由 `ensurePlacementPolicy` 原子地锁定并 `remove` 对应小写名称，再创建 policy。
3. 调用 `CreateDatabaseOnExistError`。成功返回 `false`；`ErrDatabaseExists` 返回 `true`；其他错误返回失败。

单表创建流程：

1. `CreateTable` 根据 `support_policy` 清空 placement，或确保表级及各分区 policy 已创建。
2. 若有 TTL，将原始 `table.Info.TTLInfo.Enable` 改为 `false`，避免恢复后立即启用 TTL。
3. 要求已经注册 `prealloced_ids`，经 `rewrite_table_info` 生成使用目标 ID 的完整表信息。
4. 调用 session `CreateTable(..., WithIDAllocated(true))`，明确告诉 DDL 层 ID 已分配。
5. 成功后调用 `CreateTablePostRestore`：view 无动作；sequence 恢复值；增量恢复中标记为需校正的既有表执行 ALTER；其他表无动作。

批量创建流程：

1. `CreateTables` 首先检查 `prealloced_ids`，缺失返回 `"preallocedIDs is nil"`。
2. 通过 `as_batch_create_table_session` 探测能力；不支持批量时直接成功返回，本方法不会自动回退到逐表创建。
3. 对每张表执行 policy/TTL 处理和 ID 改写，按数据库小写名聚合到 `HashMap<String, Vec<TableInfo>>`。
4. 非空时调用一次 batch `CreateTables(..., WithIDAllocated(true))`。
5. 批量创建全部成功后，再按输入顺序逐表执行 `CreateTablePostRestore`；任何一个后处理失败都会停止后续处理。

DDL job 流程：

- `ActionCreateSchema` 直接调用结构化建库接口，并把“数据库已存在”视为幂等成功。
- `ActionCreateTable` 克隆 job 内的 `TableInfo`，以 `SchemaName` 构造 `CIStr` 后调用结构化建表接口。
- 其他类型若 query 为空则忽略；若 job 带 `TableInfo`，先执行 `use <quoted schema>;`，再执行原始 query。

## 数据与状态

`DB` 只有两个长期字段：`se` 是独占的 boxed session；`prealloced_ids` 是可替换的 ID 映射。每次 `RegisterPreallocatedIDs` 覆盖旧映射，没有增量合并。单表路径在映射缺失时使用 `expect` 触发 panic，批量路径则返回普通错误；调用方必须在两个路径前都先完成注册。

建表操作会有意修改调用方传入的元数据：不支持 placement 时，`CreateDatabase` 清空数据库 policy，`CreateTable(s)` 清空表及分区 policy；存在 TTL 时把 `Enable` 置为 `false`。ID 改写发生在克隆上，原始 `TableInfo.ID` 和分区 ID 不会被替换。

`policy_map` 用 `Mutex<HashMap<String, PolicyInfo>>` 表达 Go `sync.Map.LoadAndDelete` 的一次性消费语义。key 必须是 `CIStr.L`。一个 policy 第一次遇到时在持锁期间被移出，随后在锁外调用 session；之后再遇到同名引用即视为已经创建。

批量表信息按数据库小写名聚合，因此大小写不同但 `L` 相同的数据库名进入同一批组。`UniqueTableName` 的增量校正 key 则使用 `DBInfo.Name.String()` 与 `TableInfo.Name.String()` 的原始形式，要求构造 map 的一侧保持相同拼写。

`Context` 内部状态可在 clone 之间共享；policy map 也可由多个协作者共享。但这些同步原语只保护局部数据，不改变 `DB` 本身需要独占可变访问的约束。

## 依赖与调用关系

下游依赖：

- `astersql-br-pkg-restore-internal-prealloc-table-id::PreallocIDs`：提供稳定的表/分区 ID 映射及 `RewriteTableInfo`；本文件把其错误消息转换成本地 `Error`。
- `Glue` → `Session`：承载全部真实副作用，包括执行 SQL、创建数据库/表/policy 和关闭连接。
- `BatchCreateTableSession`：可选批量能力；只有 session 返回该接口时 `DB::CreateTables` 才做工作。
- 本地 `model` / `metautil` / `utils`：是 Go 类型与辅助函数的最小移植，并非跨 crate 的 canonical TiDB 模型。

上游证据分两层：

- Go 生产主链中，RustCodeGraph 显示 `db.go` 被 `br/pkg/restore/snap_client/client.go`、`log_client/client.go` 及其测试使用；Go `snap_client` 负责创建/持有 PreallocDB，并调用这些对象创建方法。
- Rust 侧 `br/pkg/restore/snap_client/client.rs` 的同名调用点包括：预分配后 `RegisterPreallocatedIDs`，schema 阶段 `CreateDatabase`，表阶段 `CreateTables`/`CreateTable`，以及 DDL 回放阶段 `ExecDDL`。但这些调用以该文件自己的 `stubs::PreallocDB` trait 为静态类型；Cargo 搜索未发现 `snap_client` manifest 依赖本 crate，故不能把它写成对 `DB` 的已验证直接调用边。

crate 边界由 `lib.rs` 再导出公开 API；私有的 `rewrite_table_info`、`restoreSequence` 和两个 ensure 方法只能在本实现内部使用。

## 错误处理与边界

- session 创建、SQL mode 设置、建库建表、policy 创建、ID 改写和恢复后 SQL 的错误均短路返回；本地 `errors::Trace` 只是恒等包装，不保留 Go `pingcap/errors` 的调用栈能力。
- `NewDB` 仅吞掉精确匹配 `ErrUnknownSystemVar` 的 placement mode 错误；任意其他错误均阻止构造。
- 建库和 `ExecDDL(ActionCreateSchema)` 仅吞掉精确匹配 `ErrDatabaseExists` 的错误，实现幂等重放。
- `ExecDDL` 对 CREATE SCHEMA/TABLE 使用 `expect` 取得相应 history 字段；畸形 job 会 panic，而不是返回可诊断错误。
- `CreateTable` 在未注册 ID 时 panic；`CreateTables` 在同样条件下返回错误。这是当前源码差异，扩展时不应误写为统一行为。
- `rewrite_table_info` 通过 `zip` 写回分区 ID，没有显式检查两侧分区数量相等；正确性依赖 prealloc crate 保持输入分区形状。
- `CreateTables` 对不支持 batch 的 session 返回 `Ok(())` 且不创建任何表。这与 Go 的类型断言分支一致，但上层必须负责选择单表路径，不能把成功返回理解为已经执行。
- policy map 缺失时 `CreateDatabase`/`CreateTable(s)` 不会创建被引用的 policy；map 中缺少某个 key 也被当作已创建。这里没有从目标集群反查验证。
- `Context` 的 mutex 使用 `unwrap`，policy map 的 mutex 使用 `expect`；锁中毒会 panic。
- SQL 字符串中的对象名经 `EncloseName` 转义，但 `policy_mode` 直接插入单引号字符串；当前调用契约假定它来自受控配置值。

## 并发与资源生命周期

源码将 `DB` 明确标记为“not thread-safe”。Rust API 通过 `&mut self` 串行化 session 操作；`Session: Send` 只允许所有权在线程间移动，不表示可并发共享。若上层需要并发，必须在更高层做任务划分或把整个 `DB` 放入同步容器，并保持 session 的顺序语义。

policy map 的锁只覆盖 `remove`，锁释放后才执行远端/会话创建操作，避免在可能阻塞的 session 调用期间持锁。代价是：policy 从 map 移除后若创建失败，条目不会自动放回；同一 DB 实例后续重试会把缺失解释为已创建。调用方若要安全重试，应重建/补回 map，或未来显式设计失败回滚。

`Context` clone 共享取消状态，但本文件只透传给 session。实际取消响应取决于具体 session 是否检查该 context。

`DB` 没有实现 `Drop`；资源释放必须显式调用 `Close`，它只调用一次 `Session::Close`，没有关闭标记或幂等保护。`db_test.rs::test_policy_mode` 和 `parity_test.rs::go_rust_public_contract_matches` 均用记录 session 验证 Close 被转发。

## 与 Go 版本的对应关系

主要流程与 `db.go` 基本逐段对应：

- Go `NewDB` 的 nil session、清空 SQL mode、placement mode 探测及未知变量兼容分支，在 Rust 中分别对应 `Option<Box<dyn Session>>`、两次 `Execute` 和错误 code 比较。
- Go `*prealloctableid.PreallocIDs` 对应 Rust `Option<PreallocIDs>`；`ddl.WithIDAllocated(true)` 对应 `CreateTableOption`。
- Go `sync.Map.LoadAndDelete` 对应 Rust `Mutex<HashMap>::remove`。
- Go 的 batch session 类型断言对应 `Session::as_batch_create_table_session`。
- sequence cycle 的三步操作保持一致：按 increment 符号先 `setval(MinValue/MaxValue)`，再 `nextval` 触发 cycle round，最后 `setval(AutoIncID)`。
- 增量恢复仅校正 `toBeCorrectedTables` 中的对象；`NeedAutoID` 优先于 `ContainsAutoRandomBits`，普通主键表不执行 ALTER。

Rust 版本的结构性差异也必须保留在认知中：

- 为避开当前平台上的 `kv/domain` 依赖，Rust 文件内定义了 model、glue、session、context 和 error 子集；Go 使用仓库真实类型。
- Rust `Error::Trace` 不提供 Go errors stack；日志用 `eprintln!` 代替结构化 `log`/`zap`。
- Rust 的 `CreateTable` 用 `expect` 处理缺失预分配 ID，而 Go 会在 nil 指针调用处失败；批量路径两者都显式检查 nil/None。
- Rust `CreateDatabase` 接受 `Option<&Mutex<HashMap<...>>>`；Go 接受 `*sync.Map`，且 `ensurePlacementPolicy` 自身显式处理 nil。外部效果相同，但空 map 的表达位置不同。
- Rust 当前尚未与 Rust `snap_client` 的 stub trait 建立 Cargo 级直接接线；Go 文件已经处于实际 BR 调用链。

独立 Rust 测试保留了 Go 测试意图：`test_restore_auto_inc_id` 验证增量既有表只在 correction map 命中时 ALTER；`test_policy_mode` 验证 policy 去重、库表创建和 Close；`test_create_tables_in_db` 验证 batch 分组创建；`test_ddl_job_map` 验证 AUTO_RANDOM/AUTO_INCREMENT/隐藏 row ID 分支；`test_db_exec_ddl*` 验证空 query、结构化 create 和切库；`test_create_table_consistent` 验证 batch 与单表路径的 sequence 后处理一致。

## 扩展指南

- 新增建库/建表语义时，优先修改 `DB` 的对应方法，并同步核对 `db.go` 的同名逻辑。若新增模型字段，应先决定它属于本地兼容子集还是应通过 canonical crate 接入，避免继续无界复制 TiDB 模型。
- 新增 DDL job 特判应进入 `ExecDDL` 的 type match，并在 `db_test.rs` 增加独立测试，覆盖结构化 API、是否切库、空 query 和错误传播；不要把测试写回 `db.rs`。
- 修改 ID 行为应同步检查 `rewrite_table_info` 与上游 `prealloc_table_id` crate，至少验证表 ID、所有分区 ID、分区数量不变量及缺失映射错误。
- 修改 placement policy 时必须考虑一次性 remove 的重试语义、表级与分区级引用、目标不支持 policy 时的原地清理，并同步 `test_policy_mode` 与 parity 测试。
- 修改建表后处理时，应维持 view 无副作用、sequence cycle 顺序、`NeedAutoID` 与 AUTO_RANDOM 的优先级，以及只校正增量既有表的不变量。
- 若将本 crate 接入 Rust `snap_client`，需要显式解决两套 `PreallocDB`/model/session trait 的类型边界，并在相应 Cargo manifest 增加依赖；不能仅凭同名方法认为类型兼容。
- 性能方面，batch 路径会克隆完整表信息并按库暂存；大规模恢复若调整该结构，应评估峰值内存。正确性方面，批量创建成功而后处理部分失败会留下已创建对象，重试必须继续保持幂等。
- 所有新增测试继续放在 `db_test.rs` 或 `parity_test.rs`，保持生产源文件与 Rust 单元测试分离。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/restore/internal/prealloc_db` 确认本目录的 `db.rs`、`lib.rs`、`db_test.rs`、`parity_test.rs` 及 Go 对照文件均被索引。
- RustCodeGraph `node --file br/pkg/restore/internal/prealloc_db/db.rs`：读取 1–936 行，核对全部常量、类型、trait、函数、`DB` impl 和无条件编译结构；文件没有 `#[cfg]` 生产分支。
- RustCodeGraph `explore`：确认 `rewrite_table_info` 下游调用 `PreallocIDs::RewriteTableInfo`；`CreateTable(s)` 调用 policy ensure、ID 改写、session 建表和后处理；测试调用 `prealloc_ids`、`NewDB` 及公开方法。对同名 Go/Rust 符号执行 callers/callees 未产生可区分输出，因此未把该结果当作直接接线证明。
- `br/pkg/restore/internal/prealloc_db/Cargo.toml` 与 `lib.rs`：核对 crate 名、library 入口、唯一依赖、Go 包映射和公开再导出。
- RustCodeGraph `node` 读取 `db.go` 1–387 行：核对 Go 的 session、policy、ID 改写、sequence、增量校正、batch 与 Close 语义。
- RustCodeGraph `node` 读取 `db_test.rs` 的相关段落及 `parity_test.rs` 500–691 行：核对 policy 去重、TTL 禁用、缺失 ID 错误、数据库已存在、sequence cycle、AUTO_INCREMENT/AUTO_RANDOM、DDL 重放、batch/单表一致性和 Close。
- `rg` 搜索 Cargo manifest 与 `br/pkg/restore/**/*.rs`：确认 workspace 成员关系、没有其他 manifest 依赖该 crate，以及 Rust `snap_client/client.rs` 当前调用其本地 stub 接口的事实。

本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的结构命令验证本文恰有十一个固定二级章节，并人工复核没有把 Rust stub 接线描述为已完成的生产集成。
