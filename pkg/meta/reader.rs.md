# `pkg/meta/reader.rs`

## 文件定位

`pkg/meta/reader.rs` 属于 `astersql-meta` crate，并由 [`pkg/meta/lib.rs`](lib.rs) 的私有 `reader` 模块挂载后整体再导出。这个文件位于 SQL/DDL 逻辑与底层 KV 元数据编码之间，实际包含三层访问面：与 Go `pkg/meta/reader.go` 对齐的 `Reader` trait 及 harness `Mutator` 适配、面向真实 MVCC 快照的 `SnapshotReader`、面向已有 SQL 事务的 `TransactionMutator`。后两者还承担 Go 兼容键布局与 DDL job JSON 边界转换，不能把整个文件仅理解为只读接口。

crate 边界由 [`pkg/meta/Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-meta`，直接依赖 `astersql-kv`、`astersql-util-codec`、`astersql-meta-model`、`serde_json` 与 `chrono`。`lib.rs` 通过 `pub use reader::*` 向 DDL、session、domain 等上层暴露本文件的公开类型和函数。

## 核心职责

1. `Reader` 定义数据库、表、AutoID、DDL 历史、schema diff、策略、资源组及 bootstrap 状态的统一读取契约；`impl Reader for Mutator` 将每个 trait 方法委托给 `Mutator` 的已有实现。
2. `new_reader` 把 harness `kv::Snapshot` 包装成带 `META_PREFIX` 的只读 `structure`，并设置内部元数据请求来源。它返回 `Box<dyn Reader>`，主要用于该 crate 的迁移/契约测试，而不是真实 `astersql_kv` 存储路径。
3. `SnapshotReader` 在真实 `astersql_kv::Snapshot` 上读取 Go 兼容的数据库、表、资源组、历史 DDL job 和非空 schema diff，保留调用方取得快照时确定的 MVCC 视图。
4. `TransactionMutator` 借用调用方已经开启的 `astersql_kv::Transaction`，在同一事务和 statement stage 中读取或修改数据库、表、schema version/diff 与历史 job；它不自行 begin、commit 或 rollback。
5. `transaction_meta_hash_key`、`transaction_meta_string_key` 固化 Go `structure` 的 `m` 前缀键布局；`encode_go_ddl_job`/`decode_go_history_job` 在 Rust 的错误字符串 ABI 与 Go 的结构化 terror JSON 之间转换；`tso_history_datetime` 把 TSO 物理毫秒转为 UTC 历史时间文本。

## 主要符号

- `pub trait Reader`（第 36 行）：公开只读契约，共覆盖 30 余个方法。`Option<T>` 表示允许键不存在；`get_metadata_lock` 和 `get_schema_cache_size` 额外保留 Go 的 `isNull`，避免将缺失与零值混同；两个 iterator 方法返回 `Box<dyn LastJobIterator>`；visitor 式遍历在回调返回错误时停止。
- `impl Reader for Mutator`（第 141 行）：薄委托层。除 `get_all_name_to_id_and_the_must_loaded_table_info` 调用 crate 级辅助函数、两个历史 iterator 装箱外，其余方法直接调用 `Mutator` 同名固有方法。
- `pub fn new_reader`（第 298 行）：设置 `RequestSourceInternal=true` 和 `RequestSourceType=InternalTxnMeta`，以 `META_PREFIX` 创建 snapshot structure，并构造 `start_ts=0` 的只读 `Mutator`。
- `pub struct SnapshotReader`（第 310 行）：拥有 `Box<dyn astersql_kv::Snapshot>`。`new` 还设置 3000 ms 的 `TiKVClientReadTimeout`；私有 `hash_get` 统一完成 hash key 编码、快照读取、not-found 到 `None` 的转换。
- `SnapshotReader::{get_schema_version_with_non_empty_diff,get_database,get_table,get_resource_group,get_history_ddl_job}`：真实快照上的有限读取面。`get_table` 先验证数据库存在；资源组载荷允许 Go JSON 前存在 magic byte `0`；历史 job 经 `decode_go_history_job` 解码。
- `GoHistoryError` 与 `decode_go_history_job`（第 420、467 行）：把 Go `err`/`warning` 对象恢复为当前 Rust `Job` 模型使用的显示字符串，例如 `class=14, code=8259` 转为 `[schema:8259]...`。
- `transaction_meta_hash_key` / `transaction_meta_string_key`（第 486、493 行）：生成真实 KV key。hash key 为 `m` 前缀、hash 名、类型标记 `h` 和 field 的 codec 组合；string key 为 `m` 前缀、业务 key 与类型标记 `s` 的组合。
- `pub struct TransactionMutator<'a>`（第 499 行）：持有 `&'a mut dyn astersql_kv::Transaction`。构造时设置高优先级和 `AllowedOnAlmostFull`；`start_ts` 透传现有事务时间戳。
- `TransactionMutator` 的数据库/表方法：`get_database`、`list_databases`、`create_database`、`update_database`、`get_table`、`get_table_mode_value`、`update_table`、`list_tables`、`create_table`。列表方法扫描编码前缀并显式关闭 iterator；`update_table` 增加 revision，且仅在表为 `Public` 时写入事务 start TS。
- schema 与 DDL 方法：`gen_schema_version` 原子递增 `SchemaVersionKey`；`set_create_table_schema_diff`、`set_create_mlog_schema_diff`、`set_table_schema_diff`、`set_drop_mview_schema_diff`、`set_mview_cutover_schema_diff` 写入 `Diff:<version>`；`drop_table_only` 与 `drop_table_and_auto_ids` 分别保留或删除 allocator 字段；`add_history_ddl_job` 以大端 job ID 为 field 写入 `DDLJobHistory`。
- `encode_go_ddl_job`（第 881 行）：先调用完整 `Job::encode`，再把字符串型 `err`/`warning` 改写为 Go 可解码的 `{class,code,message,rfccode}`；可识别 `[schema:<code>]`、`[ddl:<code>]`，其他字符串使用 DDL 1105 兜底。
- `tso_history_datetime`（第 916 行）：通过 `ts >> 18` 取得 Go TSO 的物理毫秒，输出 UTC 毫秒精度字符串。

## 执行流程

旧的 trait 路径是：调用方取得 harness snapshot → `new_reader` 标记内部请求并用 `META_PREFIX` 创建 structure → `Mutator` 作为 `dyn Reader` 接收调用 → trait 实现委托至 `pkg/meta/meta.rs` 的固有方法 → structure/harness snapshot 返回模型或错误。`pkg/meta/meta_test.rs::test_snapshot` 和 `pkg/meta/migration_aster_unit_test.rs::database_table_and_reader_round_trip_match_go` 覆盖了写入后经该 reader 读取库表及 must-load 集合的流程。

真实只读路径是：上层存储按版本生成 `astersql_kv::Snapshot` → `SnapshotReader::new` 设置内部流量选项和读取超时 → 公开读取方法调用 `hash_get` 或 string-key `Get` → not-found 转为 `None`/版本回退，其他 KV 错误传播 → model codec 或 JSON 反序列化。上游实例包括 `pkg/domain/crossks/ddl_submit.rs::reader`、`pkg/ddl/schema_version.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/system_session.rs` 和 bootstrap 测试。

真实写事务路径是：DDL/session 已经 begin transaction，并可先建立 statement stage → `TransactionMutator::new(txn)` 借用该事务并设置元数据写优先级 → 读取当前数据库/表，修改完整模型 → 递增 schema version，写对应 diff 与历史 job → 调用方决定 release/cleanup stage 以及 commit/rollback。`pkg/ddl/table_mode_test.rs` 证明发生 KV 大小错误时由调用方 cleanup stage，版本和 diff 不会形成幽灵提交；该保证来自共享事务生命周期，而非 `TransactionMutator` 内部补偿。

## 数据与状态

文件本身没有全局可变状态。`Reader`/`Mutator` 使用 harness `kv`/`structure` 类型；`SnapshotReader` 独占一个 trait-object 快照；`TransactionMutator<'a>` 以可变借用绑定外部事务，使 Rust 借用规则阻止同一期间无协调地再次可变访问该事务。

真实元数据沿用 Go key 空间：`DBs` hash 的 `DB:<id>` field 保存数据库，`DB:<db-id>` hash 的 `Table:<id>` field 保存表，`Policies`/`ResourceGroups` 保存策略对象，`DDLJobHistory` 以 job ID 的八字节大端表示为 field；`SchemaVersionKey` 和 `Diff:<version>` 是 string keys。表删除的 allocator fields 包括 `TID:<id>`、`IID:<id>`、`TARID:<id>`。

schema diff 生成方法会去重 affected table ID，并明确填写 `schema_id`、`table_id`、`old_*`、`regenerate_schema_map` 和 `affected_options`。物化视图 cutover 将 shadow table 作为新表、旧物化视图作为 `old_table_id`，其余关联表以 affected options 触发失效/重载。

## 依赖与调用关系

下游依赖分为两组。harness reader 依赖 crate 内 `context`、`errors`、`kv`、`structure`、`meta::Mutator`、`meta_autoid` 与简化 `model`；真实 KV 路径依赖 `astersql-kv` 的 Snapshot/Transaction/Iterator、`astersql-util-codec` 的 bytes/uint 编解码、`astersql-meta-model` 的 Go 兼容模型 codec，以及 `serde_json`/`chrono`。

RustCodeGraph 对 `pkg/meta/reader.rs` 识别出 118 个符号；精确节点确认 `new_reader` 位于第 298 行并构造 `Mutator`。由于 `Reader`/`new_reader` 等名称在仓库中多义、trait-object 与外部 trait 调用属于动态边界，图的 callers/callees 未完整解析真实调用链，因此上游边用直接引用补证：`TransactionMutator` 被 `pkg/ddl/persistent_create_table.rs`、`persistent_actions.rs`、`persistent_modify_column.rs`、多个物化视图 persistent 模块以及 `pkg/session/runtime/session.rs` 使用；`decode_go_history_job` 被 `pkg/ddl/job_scheduler.rs`、`job_worker.rs` 与 session DDL runtime 使用；`SnapshotReader` 被 domain cross-keyspace、DDL schema-version、session control/system-session 使用。

值得注意的是，`Reader for Mutator` 的真正读取逻辑位于 [`pkg/meta/meta.rs`](meta.rs)，而真实持久化模型 codec 位于 `pkg/meta/model` crate；扩展时应先判断目标属于 harness 对齐面还是生产 `astersql_kv` 面，不能只给 trait 增加方法便认为生产调用链已接通。

## 错误处理与边界

`SnapshotReader::hash_get` 和 `TransactionMutator::get` 都把 `IsErrNotFound` 规范化为 `Ok(None)`；其他存储错误分别包装为 `errors::Error` 或字符串。模型解码、UTF-8、整数解析、JSON 编解码错误均通过 `?` 返回，不会静默使用损坏数据。`SnapshotReader::get_table` 对不存在的数据库返回错误，而存在数据库但表缺失返回 `Ok(None)`。

`SnapshotReader::get_schema_version_with_non_empty_diff` 当前只检查最新版本：版本大于零且最新 diff 非空时返回该版本，diff 缺失或空时返回 `version - 1`，并不向更旧版本循环搜索。这与方法注释表达的恢复语义一致，但扩展调用方不应误认为它会扫描任意长度的空洞。

资源组读取会剥离可选的首字节 `0` 后再反序列化；空 placement policy 编码、非零 magic、损坏的模型 JSON 都会报错。`encode_go_ddl_job` 对无法识别的错误前缀降级为 `ddl:1105`，属于兼容兜底而非保留任意错误 class。`tso_history_datetime` 对超出 chrono 可表示范围的物理毫秒使用 `expect`，正常 Go TSO 是其前置条件。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。`SnapshotReader` 的一致性来自调用方提供的不可变 MVCC snapshot；构造器只设置请求属性，不立即触发网络/磁盘读取。实际 IO 在调用 getter 时发生。

`TransactionMutator` 的生命周期严格受借用的外部事务约束，不拥有 store，也不负责提交和回滚。所有多步原子性、statement stage 的 release/cleanup、事务 commit/rollback 都由上层 DDL/session 代码控制；这使元数据修改可与同一 SQL/DDL 操作的其他写入共同提交。`list_databases` 与 `list_tables` 在正常完成路径显式 `Close` iterator；迭代中途 `?` 返回错误时，是否还需要显式关闭依赖底层 iterator 的 Drop/实现契约，当前源码未提供额外 guard。

`Reader` 的 visitor 参数为 `&mut dyn FnMut`，表示顺序调用且允许 visitor 保持局部可变状态；接口没有承诺并行遍历。`Box<dyn Reader>`、`Box<dyn LastJobIterator>` 也未添加 `Send`/`Sync` 约束，调用方不应推断可跨线程共享。

## 与 Go 版本的对应关系

`Reader` 的方法集合和 `new_reader` 的两项 request-source 设置直接对齐 [`pkg/meta/reader.go`](reader.go) 的 `Reader`/`NewReader`。Rust 用 `Option<T>` 表达 Go 的 nil 指针；仍以 `(value, isNull)` 保留必须区分零值和键不存在的接口；Go callback/iterator 错误停止语义也被保留。

生产 `SnapshotReader` 与 `TransactionMutator` 是 Rust 接入真实 `astersql_kv` 所需的局部接线，不是一一存在于 `reader.go` 的同名类型。其数据库、表、schema version、history job 行为应同时对照 [`pkg/meta/meta.go`](meta.go) 中 `GetDatabase`、`ListDatabases`、`GetTable`、`ListTables`、`CreateDatabase`、`UpdateDatabase`、`CreateTableOrView`、`UpdateTable`、`GenSchemaVersion`、`AddHistoryDDLJob` 等方法，以及 Go `structure/type.go` 的键布局。

Rust/Go 模型的显著边界差异是 DDL job 错误：当前 Rust `Job` 保存显示字符串，Go wire 保存 terror 对象，所以本文件必须在持久化边界双向转换。另一个差异是生产 Rust reader 目前只暴露一部分 `Reader` 能力；完整旧 trait 能力由 harness `Mutator` 提供，不能据此宣称真实 MVCC reader 已覆盖 Go `Reader` 的全部方法。

## 扩展指南

- 新增只读元数据能力时，先确定生产调用者需要的是 `Reader` trait、`SnapshotReader` 还是真实事务内读取；若 Go 公共 `Reader` 增加方法，应同步 trait、`impl Reader for Mutator`、`pkg/meta/reader.go` 对照说明及独立测试。生产路径需要另行给 `SnapshotReader` 或 `TransactionMutator` 接线。
- 新增键种类时复用 `transaction_meta_hash_key`/`transaction_meta_string_key`，先核对 Go `structure` 编码、hash/string 类型标记及 field 编码；不要手拼字节。同步测试应放在独立 `*_test.rs`，不可内嵌进本文件。
- 新增 DDL diff 分支时，最可能修改 `TransactionMutator::set_*_schema_diff`；应验证正常 release、错误 cleanup、affected ID 去重、old/new table 映射及 Go JSON 字段。相关现有测试面包括 `pkg/ddl/table_mode_test.rs`、`pkg/session/runtime/normal_ddl_*_test.rs` 和 `pkg/ddl/tests/partition/reorg_partition_test.rs`。
- 修改 DDL job wire 时必须同时验证 `encode_go_ddl_job` 与 `decode_go_history_job`，覆盖 `err` 和 `warning`、已知/未知 class、V1/V2 args 以及 Go 端可解码性；现有直接断言见 `pkg/ddl/table_mode_test.rs::crossks_align_normal_ddl_error_wire_is_structured_and_roundtrips` 及多个 normal-DDL 测试。
- 性能风险集中在 hash 全扫描与完整 JSON/model 解码；新增列表接口应维持精确前缀终止并及时关闭 iterator。兼容风险集中在 key 字节布局、magic byte、JSON 字段名及 error RFC code，任何变化都可能令 Go 与 Rust 互读失败。

## 验证依据

- 源码与边界：`pkg/meta/reader.rs`（921 行，`Reader`、`new_reader`、`SnapshotReader`、`TransactionMutator`、job/key/time helpers）、`pkg/meta/lib.rs`、`pkg/meta/Cargo.toml`、`pkg/meta/meta.rs`。
- Go 对照：`pkg/meta/reader.go` 的完整接口与 `NewReader`；`pkg/meta/meta.go` 的 schema、数据库、表和历史 job 方法。
- 独立 Rust 测试：`pkg/meta/meta_test.rs::go_merge_18_reader_exposes_starter_bootstrap_version`、`test_meta`、`test_snapshot`；`pkg/meta/migration_aster_unit_test.rs::database_table_and_reader_round_trip_match_go`；`pkg/ddl/table_mode_test.rs` 的真实事务、快照、stage cleanup 和错误 wire 往返测试。更上层使用证据来自 `pkg/ddl/persistent_*.rs`、`pkg/ddl/schema_version.rs`、`pkg/domain/crossks/ddl_submit.rs` 与 `pkg/session/runtime/*`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/meta/reader.rs` 确认目标文件含 118 个符号；`query` 定位 `Reader` trait（第 36 行）、`new_reader`（第 298 行）、`SnapshotReader`（第 310 行）、`TransactionMutator`（第 499 行）和 `decode_go_history_job`（第 467 行）；`node new_reader` 核对构造源码。宽泛 `explore` 与 callers/callees 因同名符号和动态调用未给出可信完整边，故以精确 `rg` 引用结果补足上游证据。
- 本任务是纯文档分析，未运行 Cargo。交付结构验证要求目标文档存在，并恰好包含本页所示 11 个固定二级标题。
