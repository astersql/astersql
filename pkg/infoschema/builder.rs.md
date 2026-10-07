# `pkg/infoschema/builder.rs`

## 文件定位

`builder.rs` 位于 `astersql-infoschema` crate，是把全量元数据或按版本排列的 DDL `SchemaDiff` 转换成不可变 `InfoSchema` 快照的核心构建层。crate 根模块在 `pkg/infoschema/lib.rs` 中公开 `Builder`、`SchemaDiff`、`MetadataReader`、`NewBuilder` 和 `getKeptAllocators`；生产侧的直接增量入口是 `pkg/infoschema/issyncer/loader.rs` 中的 `Loader::tryLoadSchemaDiffs`，它以旧快照初始化 Builder，依次应用 `(usedVersion, newVersion]` 的 diff，最后调用 `Build`。

该文件同时服务两种后端：v1 将库表收集到 `DatabaseState`，在 `Build` 时组装 `infoSchema`；v2 在变更发生时同步写入共享 `Arc<Data>`，在 `Build` 时创建带版本和时间戳的 `infoschemaV2`。`pkg/infoschema/Cargo.toml` 将其归入 `astersql-infoschema`，直接依赖配置、meta auto-id/model、parser、table、错误与 tracing 等 workspace crate；没有本文件专属 feature 条件。

## 核心职责

1. 用 `MetadataReader` 隔离数据库、表、placement policy 和 resource group 的读取来源，并将读取失败统一传播为 `Result<_, String>`。
2. 在 `Builder::ApplyDiff` 中把 `ActionType` 分派为建库/删库、表更新、分区与重命名、物化视图切换、策略、资源组及 PITR `RefreshMeta` 等增量操作，并返回去重排序的物理表/分区 ID。
3. 通过 `InitWithDBInfos` 执行全量装载，通过 `InitWithOldInfoSchema` 继承旧快照以执行增量装载，通过 `Build` 产出 v1 或 v2 的 `Arc<dyn InfoSchema>`。
4. 在快照构建期间维护 placement bundle、masking policy 缓存、临时表集合、内置 `information_schema` 表及 schema 版本。
5. 提供若干与 Go Builder 对齐的公共辅助函数：受影响 ID 展开、allocator 筛选、虚拟表注册、表 ID 校验与兼容转换入口。

## 主要符号

- `ActionType`：Rust 侧 DDL 动作枚举。显式列出 Builder 当前识别的 schema、table、partition、materialized view、policy、resource group、masking policy 与 allocator 动作；未单列的普通表动作可由 `TableUpdate(u8)` 携带协议类型。
- `AffectedOption` / `SchemaDiff`：描述主库表 ID、旧 ID、附属库表和多 schema 子动作。`SchemaDiff::version` 在每次 `ApplyDiff` 开始时写入 Builder。
- `MetadataReader`：增量装载边界。`database` 与 `table` 必须由调用方实现；`policy` 和 `resource_group` 默认返回 `Ok(None)`，因此相关动作若未覆写会得到明确的“not found”错误。
- `DatabaseState`：内部可变状态，包含一份 `DBInfo` 与按表 ID 索引的 `Table`；不对 crate 外公开。
- `Builder`：持有 v1/v2 选择、版本、库表、策略、资源组、临时表、masking cache 和 bundle 增量标记。`WithCrossKS`、`WithStorageClassEnabled` 提供构建级配置；`WithStore` 目前只是 Go API 形状的恒等占位。
- `Builder::ApplyDiff`：增量主入口。关键内部实现包括 `applyCreateSchema`、`applyDropSchema`、`applyRecoverSchema`、`refresh_schema`、`applyRefreshMeta`、`applyTableUpdate`、`apply_table_ids` 与 `applyDropTable`。
- `Builder::InitWithDBInfos` / `InitWithOldInfoSchema` / `Build`：分别是全量初始化、旧快照增量初始化和终态构建入口。
- `Builder::build_bundles` 与 `BuilderBundleSchema`：把 Builder 当前库表/策略适配成 `BundleSchema`，支持全量重建或继承旧 bundle 后按表、策略做增量刷新。
- `needRefreshMaskingPoliciesForTableDiff`：判定哪些动作会令表/列引用缓存失效。
- `appendAffectedIDs`：把表自身 ID 与所有分区定义 ID 加入返回集合。
- `getKeptAllocators`：按 rebase/auto-ID cache/multi-schema 子动作过滤旧 `Allocators`。
- `RegisterVirtualTable` / `virtual_table_drivers`：以 `OnceLock<Mutex<Vec<_>>>` 保存进程级虚拟表驱动。当前 `InitWithDBInfos` 并不读取该列表，而是直接调用 `tables::information_schema_db_with_storage_class` 注入内置库。

## 执行流程

全量流程从 `NewBuilder` 开始：先对共享 `Data` 设置缓存容量，再创建空 Builder。调用方传入 `DBInfo` 列表给 `InitWithDBInfos`；该方法清空本地库表与 bundle 增量状态，v2 后端先执行 `Data::resetBeforeFullLoad`，随后逐库取走 `db.tables`。已实际加载的表名会按 `name.original` 从 `table_name_2_id` 删除，未加载项保留以支持惰性加载。每张表进入 `DatabaseState`，v2 同时调用 `Data::addDB`/`Data::add`。非 cross-keyspace 构建再注入按 storage-class 配置裁剪的 `INFORMATION_SCHEMA`，最后装载 policy 与 resource group。

增量流程通常由 `Loader::tryLoadSchemaDiffs` 驱动：它从旧 schema 数据构造 Builder，按版本读取 diff；被过滤的 diff 只推进版本，需重建 schema map 的 diff直接报错，其他 diff 转换后交给 `ApplyDiff`。`ApplyDiff` 先设置版本，再按动作分派。表更新最终汇聚到 `apply_table_ids`：读取新表、在 ID 或名称变化时删除旧表、写入 `db_id`、执行兼容规范化、替换库内表，并在 v2 同步 `Data`。删除则移除表和临时表标记，并返回表及分区 ID。

特殊分支包括：

- `DropTable` 在 metadata 中的 model state 尚非 `StateNone` 时仍刷新表，直到最终状态才删除；物化视图删除采用相近但略宽松的 state 判定。
- `MViewRefreshOutOfPlaceCutover` 先验证数据库存在，再删除旧物化视图和可能残留的 shadow 表名，写入新表，并刷新 `affected_options` 指向的 base/log 等关联表。
- `RefreshMeta` 以 `table_id == 0` 区分 schema 操作；库不存在时删库，库存在时刷新或创建。表级刷新若库已不存在则忽略，metadata 已无表则删除，否则更新。
- placement policy alter/drop 会请求重算引用表 ID，但当前 `tables_referencing_policy` 返回空；bundle 自身仍可借 `bundle_policy_updates` 在旧快照增量路径中按策略刷新。

`Build` 消费 Builder。它先计算 bundles；v2 直接用共享 `Data`、版本和 `schema_ts` 创建 `infoschemaV2`，再注入 bundles/policies 与 masking cache。v1 则创建 `infoSchema`，恢复 masking loader/cache，设置 bundles 与临时表，再逐项写入库表、policy 和 resource group。

## 数据与状态

Builder 的核心不变量是：`schema_version` 表示当前已应用版本；`databases` 是待构建快照的库表真源；在 v2 模式下，对库表的每次增删还必须以同一版本同步到共享 `Data`。`ApplyDiff` 的结果在返回前排序去重，使上游 `RelatedSchemaChange` 得到稳定的物理 ID 序列。

`temporary_table_ids` 只在 v1 `Build` 时整体注入，但添加/删除也同步通知 `Data`。`masking_cache` 与 `masking_loaded` 从旧快照继承；命中 `needRefreshMaskingPoliciesForTableDiff` 的动作会清空缓存并把 loaded 设为 false，使后续访问重新加载。`masking_loader` 也会从旧快照继承，v1 构建时带入当前 `schema_ts`。

bundle 状态有两种模式：全量初始化将 `delta_bundles=false` 并清空旧 cache；`InitWithOldInfoSchema` 保存旧 bundles、置 `delta_bundles=true`。之后 `ApplyDiff` 把直接或附属表 ID 放入 `bundle_updates`，策略动作放入 `bundle_policy_updates`；`build_bundles` 继承旧映射并只标记这些对象重算。`BuilderBundleSchema::table_bundle_spec` 从 model metadata 提取表级 policy，并让分区 policy 覆盖或继承表级 policy。

## 依赖与调用关系

上游生产调用者是 `pkg/infoschema/issyncer/loader.rs`：`tryLoadSchemaDiffs` 使用 `Builder::new`、`WithCrossKS`、`InitWithDBInfos`、`ApplyDiff` 和 `Build` 完成增量 schema load；`pkg/infoschema/issyncer/lib.rs` 也用全量初始化后 `Build` 生成 schema。`pkg/infoschema/builder_misc.rs` 将 policy/resource-group diff 的 `schema_id` 改写到本文件期望的 `table_id` 字段后委托 `ApplyDiff`。

主要下游依赖是：`infoschema.rs` 的 `InfoSchema` trait、v1 `infoSchema` 与元数据类型；`infoschema_v2.rs` 的共享 `Data` 和 `infoschemaV2`；`bundle_builder.rs` 的 `bundleInfoBuilder`/`BundleSchema`；`tables.rs` 的内置信息模式定义；`astersql-meta-autoid` 的 allocator 类型；`astersql-config` 的 storage-class 开关。bundle 构建错误不阻断 `Build`，而是通过 `tracing::warn!` 记录后继续返回其余可用 bundle。

RustCodeGraph 的文件关系显示 `builder.rs` 被 `builder_test.rs`、`go_merge_45_test.rs`、`issyncer/lib.rs`、`issyncer/loader.rs` 和 domain 测试等直接使用。其方法级索引未完整识别 Rust `impl` 调用，因此具体生产边由上述文件中的直接调用点补证。

## 错误处理与边界

metadata reader 的错误原样以 `String` 经 `?` 传播；需要存在的库、表、policy 或 resource group 缺失时，内部方法构造带 ID 的明确错误。`MViewRefreshOutOfPlaceCutover` 在库未预载时直接失败；`apply_table_ids` 还会在目标库未载入时报错。相反，删除不存在的库/表是幂等的，返回空受影响集合；`RefreshMeta` 的表操作在库已被一致快照移除时也返回空。

`tableBucketIdx` 对非正 ID 使用 `assert!`，不是可恢复错误；普通路径应先通过 `tableIDIsValid` 或动作分支保证 ID 为正。虚拟表注册使用 `Mutex::lock().expect(...)`，锁中毒会 panic。bundle 计算采取降级策略：逐条警告而不让整个 schema 构建失败。

当前实现存在应在扩展时显式考虑的迁移边界：`ConvertOldVersionUTF8ToUTF8MB4IfNeed` 是空实现；`ConvertCharsetCollateToLowerCaseIfNeed` 只规范化表/列名称的 `lower` 字段，并未实现 Go 中按版本转换 charset/collation 的逻辑；`tables_referencing_policy` 恒为空；`WithStore` 不保存 storage；已注册的 `DRIVERS` 未被全量初始化消费。它们是现状限制，不应作为“已支持”的能力依赖。

## 并发与资源生命周期

Builder 本身通过 `&mut self` 顺序变更，不包含内部并行执行；典型生命周期是“创建 → 全量/旧快照初始化 → 顺序应用若干 diff → 消费式 Build”，`Build(self, ...)` 防止同一 Builder 在产出后继续修改。跨线程共享主要依赖 `Arc<Data>`、`Arc<TableInfo>`、`Arc<PlacementBundle>` 与 `Arc<dyn InfoSchema>`，同步语义由这些下游类型承担。

唯一显式锁是虚拟表驱动的全局 `OnceLock<Mutex<Vec<virtualTableDriver>>>`：首次访问惰性初始化，注册时短暂独占锁，函数指针与 `DBInfo` 被保存到进程生命周期。masking loader 以 `Arc<dyn MaskingPolicyLoader>` 跨快照复用。全量 v2 装载不能简单替换 `Data`，而是调用 `resetBeforeFullLoad` 保留共享对象与历史版本机制、同时清理当前版本的陈旧表。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/builder.go`。Rust 保留了 Go 的主 API 形状和核心流程：`ApplyDiff` 分派、`InitWithDBInfos`、`InitWithOldInfoSchema`、`Build`、虚拟表注册、受影响表/分区 ID 收集及 `getKeptAllocators`。`pkg/infoschema/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/infoschema"` 也明确记录了迁移来源。

已验证的一致语义包括：allocator 在 RebaseAutoID/ModifyTableAutoIDCache 时丢弃 RowID 和 AutoIncrement，在 RebaseAutoRandomBase 时丢弃 AutoRandom，MultiSchemaChange 检查子动作；全量加载按原始大小写从 `TableName2ID` 移除已加载表并保留未加载项；PITR refresh 按 metadata 是否存在决定创建/刷新/删除；物化视图 cutover 处理旧 ID、shadow 名和关联表；cross-keyspace 不注入 `INFORMATION_SCHEMA`。

Rust 并非 Go 的完整等价实现。Go 的 `ApplyDiff` 将更多 action 分派给细粒度 helper，并维护 allocator、外键反向引用、排序桶、临时表等完整状态；Rust 用较紧凑的 `HashMap` 替代。Go 的 masking invalidation 还包含 DropColumn/ModifyColumn，而 Rust `ActionType` 当前没有对应显式项。Go 会扫描引用 placement policy 的表，Rust 当前返回空。Go 的 charset/collation 与旧 UTF8 兼容函数会根据版本和全局配置改写真实字符集字段，Rust 对应功能尚未移植。Go `NewBuilder` 还接收 autoid requirement、resource factory 与 store，Rust 接口已简化。

## 扩展指南

新增 DDL 动作时，先扩展 `ActionType` 与来自上游 diff 的转换，再在 `ApplyDiff` 中选择正确的更新类别。若动作会改变表或分区，必须通过 `appendAffectedIDs` 或等价逻辑返回完整物理 ID，并确认 bundle 更新集合与 masking cache 失效条件；涉及旧/新 ID 或跨库重命名时还要覆盖 `affected_options`。不要把未知动作静默映射为不会更新的 `None`。

扩展 v1/v2 行为必须同时维护两条路径：本地 `databases` 是 v1 构建依据，`Data::addDB/add/remove_by_id/deleteDB/resetBeforeFullLoad` 是 v2 可见性的依据。新增初始化状态还应在 `InitWithDBInfos` 的全量清理、`InitWithOldInfoSchema` 的继承和 `Build` 的终态注入三处检查生命周期闭环。

测试应放在独立文件而非 `builder.rs` 内。核心回归优先扩展 `pkg/infoschema/builder_test.rs`；涉及 Go 新提交移植、物化视图、storage class 或 v1/v2 双路径时扩展 `pkg/infoschema/go_merge_45_test.rs`；生产 loader 集成边界可在 `pkg/infoschema/issyncer` 的独立测试中验证。兼容风险主要是旧版本元数据转换和 Go/Rust action 覆盖差异；正确性风险是旧表残留、版本不同步、漏报分区 ID和缓存未失效；性能风险集中在全库 bundle 扫描、克隆大块 DB/Table metadata 和不必要的 v2 全量 reset。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 文件、307,296 节点；`files --filter pkg/infoschema/builder.rs` 确认目标文件含 108 个符号；`node --file ... --offset/--limit` 完整读取 980 行并给出直接使用文件。`query` 确认 Rust `Builder`、Go `Builder` 与 `NewBuilder` 候选；方法级 Rust caller 查询未能解析完成，故用直接调用搜索补证。
- 源码与模块边界：`pkg/infoschema/builder.rs`、`pkg/infoschema/lib.rs`、`pkg/infoschema/Cargo.toml`、`pkg/infoschema/builder_misc.rs`、`pkg/infoschema/issyncer/loader.rs`、`pkg/infoschema/issyncer/lib.rs`。
- Go 对照：`pkg/infoschema/builder.go`，重点核对 `ApplyDiff`、`needRefreshMaskingPoliciesForTableDiff`、`getKeptAllocators`、`Build`、`InitWithOldInfoSchema`、`InitWithDBInfos`、字符集兼容函数、虚拟表注册与 `NewBuilder`。
- 独立测试：`pkg/infoschema/builder_test.rs` 与 `pkg/infoschema/builder_test.go` 验证 allocator 和 `TableName2ID` 语义；Rust 测试另覆盖 RefreshMeta、CreateTables、cross-keyspace 更新与 drop 中间状态。`pkg/infoschema/go_merge_45_test.rs` 验证物化视图、bundle、masking cache、storage-class 和 v1/v2 行为。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务文件规定的 11 章节结构命令、路径检查及人工事实复核作为验证。
