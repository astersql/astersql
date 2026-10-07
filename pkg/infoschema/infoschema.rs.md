# `pkg/infoschema/infoschema.rs`

源文件：[`infoschema.rs`](./infoschema.rs)

## 文件定位

本文件是 `astersql-infoschema` crate 的 InfoSchema V1 核心数据模型与查询实现。crate 根模块 `pkg/infoschema/lib.rs` 以 `pub mod infoschema` 装配本文件，并重导出 `InfoSchema`、`infoSchema`、`Table`、`TableInfo`、`DBInfo`、脱敏策略类型及常用辅助函数。`pkg/infoschema/Cargo.toml` 指定 `lib.rs` 为库入口，并直接依赖 `astersql-meta-model`、`astersql-infoschema-context`、`astersql-parser-ast`、`astersql-util-dbterror` 等元数据和错误基础 crate。

在完整应用中，本文件提供“某一 schema version 下可并发读取的元数据快照”：`pkg/infoschema/builder.rs` 的 `Builder::Build` 创建 `infoSchema` 并调用 `add_schema`；`pkg/domain/canonical_domain.rs` 也会从 catalog 数据构造该类型；`pkg/session/runtime/session.rs` 用 `SessionExtendedInfoSchema` 在基础快照之上叠加会话临时表和 MDL 固定元数据。它不负责从持久化元数据增量计算快照（由 Builder 负责），也不实现 InfoSchema V2 的缓存/惰性表加载（见 `pkg/infoschema/infoschema_v2.rs`）。

## 核心职责

- 定义大小写不敏感标识符和精简元数据形状：`CiString`、`DBInfo`、`TableInfo`、`ColumnInfo`、`IndexInfo`、分区与外键结构；`Table::from_model` 从完整 `astersql_meta_model::TableInfo` 投影查询索引，同时在 `model_meta` 中保留无损完整模型。
- 以 `InfoSchema` trait 固化按名/ID查库表、枚举 schema、查分区、查询 placement、读取脱敏缓存、识别 V2 和版本 GC 等公共表面；默认实现明确了 V1/可选能力的空语义。
- 以 `infoSchema` 保存 V1 快照：名称哈希索引、schema ID 反查、512 桶的有序表 ID 索引、外键反向索引、placement/resource group/bundle、全局临时表 ID 与脱敏策略缓存。
- 提供查找和判定辅助：`SchemaByTable`、`FindTableByTblOrPartID`、`TableIsView`、`TableIsSequence`、`HasAutoIncrementColumn`、`AllSchemaNames`。
- 实现脱敏策略的惰性加载、过滤 ID 规范化、1024 条分批、稳定排序、持久化字符串解析和 SQL 形状构造。
- 提供 `SessionTables` 与 `SessionExtendedInfoSchema`，让本地临时表和 MDL 固定表优先于基础快照参与按名/ID查询。

## 主要符号

- `CiString { original, lower }`：保留展示大小写，同时以 `lower` 作为库表、策略和资源组的哈希键。`CiString::new` 当前使用 Unicode `to_lowercase`。
- `TableInfo`：缓存所需的表 ID、库 ID、列、索引、分区、外键、视图/序列标记；`model_meta` 是完整模型的可选 `Arc`。其手写 `PartialEq` 有意忽略 `model_meta`。
- `Table(Arc<TableInfo>)`：共享表句柄。`from_model` 负责投影，`ModelMeta` 在未保留完整模型时返回 `ErrTableMetadataUnavailable`。
- `InfoSchema: Send + Sync`：跨线程只读查询契约。必需方法覆盖 schema/table/partition 主查询；`SchemaSimpleTableInfos`、`ListTablesWithSpecialAttribute` 等基于更原始方法组合；placement、masking、V2/GC 方法具有 V1 安全默认值。
- `infoSchema`：V1 具体快照。`schema_map` 处理按小写名查找，`schema_id_to_name` 处理 ID 反查，`sorted_table_buckets` 处理表 ID 二分查找；`RwLock` 保护策略、资源组与脱敏映射，`Mutex<bool>` 串行化首次脱敏加载。
- `tableBucketIdx` / `bucketCount`：以 `id.unsigned_abs() % 512` 选桶；每个桶由 `add_schema` 排序，`TableByID` 据此二分查找并拒绝非正 ID。
- `MaskingPolicyLoader` 与 `LoadMaskingPolicies` / `loadMaskingPoliciesWithTableIDs`：将存储访问抽象为注入式 loader；无过滤时加载全部，有过滤时移除非正 ID、去重排序并按 1024 分批，最终按 `(table_id, column_id, id)` 排序。
- `SessionTables`：维护名称索引和 ID 索引的会话表容器；`AddTable` 同时检查 ID 与名称冲突，`RemoveTable` 同步清理空 schema。
- `SessionExtendedInfoSchema`：查询优先级为 `temporary`、`mdl_tables`、`base`；`DetachTemporaryTableInfoSchema` 只移除本地临时层，保留 MDL 层。

## 执行流程

1. Builder 或 domain 侧先取得数据库和表元数据，构造 `infoSchema::new(schema_version)`。
2. 每次 `add_schema` 都把表写入小写名称映射，将表按 ID 放入 512 桶之一，建立父表到子表外键的反向索引，然后对各桶按表 ID 排序；最后写入 schema 名称和 ID 两套索引。
3. 按名称查询通过 `schema_map[schema.lower].tables[table.lower]` 完成；不存在时 `TableByName` 返回带 `ErrTableNotExists.mysql_name` 的 `InfoSchemaError`。按 ID 查询先校验 ID 为正，再在目标桶二分查找。
4. 分区 ID 查询目前遍历全部 schema、表和分区定义；`FindTableByTblOrPartID` 先走快速表 ID 路径，未命中才回退到分区扫描。
5. 脱敏策略首次查询会进入 `loadMaskingPoliciesIfNeeded`。持有 `masking_loaded` 互斥锁的线程决定是否加载：无 loader 或系统表尚未就绪会标记完成；成功时写入“表 ID → 列 ID → 策略”映射；普通错误保留未加载状态，使后续访问重试。
6. 需要按指定表集合加载时，`normalizeMaskingPolicyTableIDs` 区分“没有过滤条件”和“给了过滤条件但无合法 ID”；后者直接返回空结果，前者加载全部，其余 ID 分批调用 loader 并稳定排序。
7. 会话查询经 `SessionExtendedInfoSchema` 先检查本地临时表，再检查 MDL 固定表，最后委托基础 `InfoSchema`。脱离临时表时复制 base 和 MDL 层而创建新的共享视图。

## 数据与状态

- `infoSchema` 的结构性快照状态（schema/表/外键/bundle/临时表 ID）在构建期通过 `&mut self` 更新，建成后通常包在 `Arc<dyn InfoSchema>` 中共享；查询返回 `Arc` 或 `Table` 克隆，不转移底层元数据所有权。
- 表名和 schema 名以小写键索引，但 `original` 用于显示和错误文本。调用方必须通过 `CiString` 进入名称 API，避免自行选择不一致的规范化方式。
- `sorted_table_buckets` 的关键不变量是“桶数固定为 512、同一桶按表 ID 升序”。`TableByID` 的二分正确性依赖所有新增路径维持该排序。
- placement policy、resource group 和 masking policy 是内部可变缓存，分别由 `RwLock<HashMap<...>>` 保护；策略/资源组的 ID 查询当前在线性扫描 map 值。
- `masking_loaded` 与 `masking` 分锁：加载状态锁覆盖整个 loader 调用，因此同一 `infoSchema` 上只有一个首次加载者，其他读取者等待；写入映射时再取得 masking 写锁。
- `SessionTables` 不是内部加锁容器，增删要求 `&mut self`；共享并发语义应由持有它的 session 生命周期保证。`DetachTemporaryTableInfoSchema` 克隆 MDL 表集合，其中表元数据仍由 `Arc` 共享。

## 依赖与调用关系

上游直接证据：

- `pkg/infoschema/builder.rs:746-754`：`Builder::Build` 创建 `infoSchema` 并为每个 schema 调用 `add_schema`，是常规 V1 构建入口。
- `pkg/domain/canonical_domain.rs:1797-1816`：canonical domain 从 catalog 版本与数据库表集合组装 `infoSchema`，证明该类型位于 domain 可见元数据链上。
- `pkg/session/runtime/session.rs:1497`：创建 `SessionExtendedInfoSchema`，把 session 层查询接到基础快照上。
- `pkg/executor/test/infoschema/infoschema_test.rs:486`：实际调用 `FindTableByTblOrPartID` 验证分区 ID 回退路径；多个 planner/session 测试调用 `MockInfoSchema` 作为规划元数据输入。

下游直接证据：

- `Table::from_model` 读取 `astersql_meta_model::TableInfo` 的列、索引、分区、外键、视图和序列字段；`ModelTableInfoByName` 通过 `ModelMeta` 向 planner/executor 保留完整元数据。
- `InfoSchema::ListTablesWithSpecialAttribute` 使用 `astersql_infoschema_context::SpecialAttributeFilter`，只把存在 `model_meta` 且通过过滤器的表写入结果。
- 缺表错误码来自 `crate::error::ErrTableNotExists`；跨 crate 错误可由 `InfoSchemaError::into_shared` 转为 `astersql_util_dbterror::errors::SharedError`。
- 脱敏存储细节被隔离在 `MaskingPolicyLoader::load` 后；本文件只规定过滤、批次、顺序、缓存与重试语义，不直接拥有 session/SQL executor。

RustCodeGraph 对 `add_schema` 给出的被调用边为 `tableBucketIdx`、`addReferredForeignKeys` 和 `Table::Meta`；对 Rust `TableByID` 给出的被调用边为 `tableBucketIdx` 与 `Table::Meta`；对 `loadMaskingPoliciesWithTableIDs` 给出的边包括 `normalizeMaskingPolicyTableIDs`、`LoadMaskingPolicies` 与 `MaskingPolicyLoader::load`。当前索引未为这些符号返回可靠的跨 crate callers，因此上游接线另以 `rg` 精确搜索并读取上述文件确认。

## 错误处理与边界

- `TableByName` 对不存在的库或表统一返回 `ErrTableNotExists`；`SchemaTableInfos` 对不存在的 schema 返回空列表而不是错误，这一差异由 `test_basic` 明确固定。
- `TableByID` 对零和负 ID 返回 `None`；`SchemaByTable` 优先使用正 `db_id`，否则按表 ID 回查，仍未命中时返回 `None`。
- `Table::ModelMeta` 对只含精简字段的测试/迁移表返回 `ErrTableMetadataUnavailable`，调用完整元数据消费者时必须处理该错误，不能从精简字段有损重建。
- `SessionTables::AddTable` 对 ID 或同库同名冲突返回 `ErrTableExists`，并用 `assert_eq!` 强制数据库 ID 与表的 `db_id` 一致；错误调用会 panic，而非返回可恢复错误。
- `maskingPolicyStatusFromString`、`maskingPolicyTypeFromString` 和 `maskingPolicyRestrictOpsFromString` 对未知持久化字符串分别返回专用错误码；限制操作以逗号拆分并累积位图。
- 脱敏 loader 的“系统表未准备好”错误被视为终态空缓存，以避免无限重试；其他错误被有意吞下并保留未加载状态，下次访问重试。该查询 API 返回 `Option`，因此不能把“加载失败待重试”和“确实不存在策略”从返回值中区分开。
- `FindTableByPartitionID` 是全量线性扫描；schema/table 数量大时是潜在性能边界。策略和资源组按 ID 查询也为线性复杂度。

## 并发与资源生命周期

`InfoSchema` 要求 `Send + Sync`，生产快照主要通过 `Arc` 共享。`Table`、`DBInfo`、策略和 bundle 的查询都克隆 `Arc`，其生命周期可超过当前查找调用，并避免复制完整元数据。

`RwLock` 允许 placement policy、resource group、masking cache 多读单写；所有锁获取都用 `expect("... lock poisoned")`，发生持锁 panic 后后续访问也会 panic。脱敏首次加载持有 `masking_loaded: Mutex<bool>` 执行可能较慢的 loader，从而保证单加载者但也会阻塞所有首次访问；这与 Go 版本用 channel 协调单加载者的目标一致，调度机制不同。loader 本身存为 `Arc<dyn MaskingPolicyLoader>`，快照销毁后才释放最后一个引用。

结构性字段没有内部锁，约定在 Builder 阶段独占修改，再作为不可变快照发布。`SessionTables` 同样通过可变借用串行更新；`SessionExtendedInfoSchema` 的 detach 操作创建新值，不原地破坏其他共享视图。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/infoschema.go`，Rust 保留了以下核心语义：大小写不敏感名称索引；表 ID 分桶和二分查找；`SchemaByTable` 的 DBID/表 ID 回退；分区扫描；全局临时表标记；placement/resource group 查询；脱敏策略首次加载、系统表未就绪处理、1024 条分批、过滤 ID 去重排序和结果稳定排序；本地临时表优先与 MDL 元数据保留。

已确认的实现差异与迁移边界：

- Go 的 `model.TableInfo` 与 `table.Table` 更完整；Rust 同时维护精简索引字段和可选 `model_meta`，缺少后者时部分完整元数据 API 会报错。
- Go 的 masking loader 直接获取 session resource、执行 restricted SQL、处理 snapshot option 并解析 row；Rust 用 `MaskingPolicyLoader` trait 注入这些动作，`buildLoadMaskingPoliciesQuery` 仅保留查询和参数形状。
- Go 首次加载用 `maskingPoliciesLoadCh` 让其他 goroutine 等待，Rust 以覆盖 loader 调用的 `Mutex<bool>` 实现单加载者；可见结果相同，但锁持有时间和阻塞方式不同。
- Go 的 SessionExtendedInfoSchema 类型字段为可选的 LocalTemporaryTables/MdlTables；Rust 构造时总是创建两个空 `SessionTables`，查询优先级保持一致。
- Rust `MaskingPolicyByName` 与当前 Go 代码都在同名策略跨表歧义时返回未命中；扩展时应维持这项精确查找边界，优先使用表/列 ID API。

## 扩展指南

- 新增 InfoSchema 查询能力时，先判断它属于公共 trait、V1 专有方法还是组合辅助。若加入 `InfoSchema`，必须同步检查 `infoSchema`、`SessionExtendedInfoSchema`、`infoschema_v2.rs` 以及仓库内测试替身的实现，避免只让 V1 可编译。
- 新增或改变表索引时，应从 `infoSchema::add_schema` 和 Builder 更新路径接入；必须保持名称小写键、schema ID 映射、桶内排序、外键反向索引同步。若引入增删表的独立路径，应为所有相关索引增加一致性测试。
- 扩展完整表元数据消费者时，应优先使用 `Table::ModelMeta`，并显式处理测试构造表缺少 `model_meta` 的情况；不要悄悄用精简字段重建类型、默认值或索引状态。
- 修改脱敏策略加载时需同步核对 `loadMaskingPoliciesIfNeeded`、`loadMaskingPoliciesWithTableIDs`、三类字符串解析器和 Go 同名函数；保留“nil/空即无过滤”“非空但全非法即匹配空集”、1024 分批与稳定排序契约。
- 修改临时表覆盖顺序或 detach 语义时，应同时覆盖按名、按 ID、schema ID、MDL 表与 `HasTemporaryTable`；生产接线在 `pkg/session/runtime/session.rs`，独立测试应继续放在 `pkg/infoschema/infoschema_test.rs` 或对应 session 测试文件，不能内嵌回生产源文件。
- 性能扩展的重点是 `FindTableByPartitionID`、按 ID 扫描策略/资源组以及首次 masking loader 的锁持有时间；任何新索引都要说明构建成本、快照内存和更新一致性。

## 验证依据

- 源码与边界：完整阅读 `pkg/infoschema/infoschema.rs`；读取 crate 入口 `pkg/infoschema/lib.rs` 和依赖声明 `pkg/infoschema/Cargo.toml`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标文件含 179 个符号；执行了 `files --filter pkg/infoschema/infoschema.rs`，以及对 `InfoSchema`、`infoSchema`、`MockInfoSchema`、`FindTableByTblOrPartID`、`LoadMaskingPolicies`、`SchemaByTable` 等的 `query`；对 `add_schema`、`TableByID`、`loadMaskingPoliciesIfNeeded`、`loadMaskingPoliciesWithTableIDs` 等执行了 `callers`/`callees`。
- 生产接线：读取 `pkg/infoschema/builder.rs` 中 `infoSchema::new`/`add_schema` 调用，`pkg/domain/canonical_domain.rs` 中 catalog 构建入口，以及 `pkg/session/runtime/session.rs` 中 `SessionExtendedInfoSchema::new` 调用。
- Go 对照：读取 `pkg/infoschema/infoschema.go` 中结构体与基础查询、脱敏加载/批次/解析、`SessionExtendedInfoSchema`、`FindTableByTblOrPartID` 等同名实现。
- 独立测试：完整阅读 `pkg/infoschema/infoschema_test.rs`。其中 `test_basic` 固定大小写名称、缺失 schema 空列表、正 ID 约束；`test_local_temporary_tables` 固定会话表生命周期；`test_schema_by_table_falls_back_to_table_id` 固定 DBID 回退；`test_masking_policy_parsers_match_go_persisted_contract` 与 `test_masking_policy_query_and_batches_match_go` 固定持久化字符串、SQL 形状、1024 分批和排序。
- 本任务为纯文档分析，按计划不运行 Cargo；最终结构以任务指定命令检查 11 个固定二级标题。
