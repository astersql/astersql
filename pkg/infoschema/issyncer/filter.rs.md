# `pkg/infoschema/issyncer/filter.rs` 逻辑说明

## 文件定位

`filter.rs` 属于 `astersql-infoschema-issyncer` crate。该 crate 的清单是 `pkg/infoschema/issyncer/Cargo.toml`，crate 根 `pkg/infoschema/issyncer/lib.rs` 通过 `mod filter;` 纳入本文件，再以 `pub use filter::*;` 导出其公开项。因此外部代码使用的是 crate 级公开 trait `Filter`，而不是直接访问私有模块名。

本文件是 InfoSchema 同步加载流程的策略边界：它只声明“是否跳过某个 schema diff”以及“全量加载时是否跳过某个数据库”两个判定，不读取存储、不修改缓存，也不实现任何具体筛选规则。实际执行筛选的是同 crate 的 `Loader`（`pkg/infoschema/issyncer/loader.rs`），`Syncer::New`（`pkg/infoschema/issyncer/syncer.rs`）负责把可选过滤器注入 Loader。

## 核心职责

- 用 `Filter` trait 把调用方特有的库选择策略与通用 InfoSchema 加载器解耦。
- 为增量加载暴露 `SkipLoadDiff`：返回 `true` 表示忽略该 `SchemaDiff`，返回 `false` 表示继续默认应用流程。
- 为全量加载暴露 `SkipLoadSchema`：返回 `true` 表示不加载该 `DBInfo` 及其表，返回 `false` 表示保留。
- 以 `Send + Sync` 约束实现者，使 `Arc<dyn Filter>` 可以安全地随 `Loader`/`Syncer` 跨线程共享。

它不负责定义具体的 BR、系统库或 cross-keyspace 规则。测试实现 `TestNameFilter` 位于独立文件 `pkg/infoschema/issyncer/loader_test.rs`；cross-keyspace 限制则由 `Loader::skipLoadingDiffWithLatest` 和 `Loader::fetchAllSchemasWithTables` 自行处理。

## 主要符号

### `pub trait Filter: Send + Sync`

本文件唯一的公开类型，也是唯一的顶层业务符号；没有常量、结构体、枚举、实现块或条件编译项。crate 根将它再导出，因此它构成 `astersql-infoschema-issyncer` 的公开 API。

### `Filter::SkipLoadDiff(&self, diff: &SchemaDiff, latestIS: Option<&SchemaInfo>) -> bool`

`diff` 描述单次 DDL 引起的模式版本变化；`latestIS` 是 Loader 当前缓存的最新模式快照，首次加载或尚无缓存时可以是 `None`。接口只约定布尔值含义，不规定实现者如何根据动作类型、库 ID 或库名决策。调用方必须正确处理 `latestIS == None`，不能假定缓存一定存在。

### `Filter::SkipLoadSchema(&self, dbInfo: Option<&DBInfo>) -> bool`

用于全量加载阶段对数据库逐个判定。参数类型允许 `None`，这是从 Go 可空指针契约移植而来的边界；实现者应显式定义空值行为。当前 Rust Loader 在 `fetchAllSchemasWithTables` 中只以 `Some(db)` 调用它，但 trait 的公共契约仍允许其他调用者传入 `None`。

## 执行流程

增量加载路径如下：

1. `Loader::tryLoadSchemaDiffs`（`pkg/infoschema/issyncer/loader.rs`）按版本读取 `SchemaDiff`。
2. 它调用 `Loader::skipLoadingDiffWithLatest(&diff, Some(old))`；公开辅助入口 `Loader::skipLoadingDiff` 则先从互斥缓存克隆最新 `SchemaInfo`，再进入同一内部判定。
3. 若存在 `self.filter`，Loader 首先调用 `Filter::SkipLoadDiff`。返回 `true` 时立即跳过该 diff，并在增量循环中只推进 builder 的 schema 版本。
4. Filter 未跳过时，非 cross-keyspace Loader 继续应用 diff；cross-keyspace Loader 还会检查新旧表 ID 是否属于保留系统 ID。
5. 通过判定的 diff 最终交给 InfoSchema Builder 应用。

全量加载路径如下：

1. `Loader::LoadWithTS` 在不能或无需走增量路径时调用 `Loader::fetchAllSchemasWithTables`。
2. 普通 Loader 从 `SchemaReader::ListDatabases` 获取数据库列表；若配置了 Filter，则以 `!filter.SkipLoadSchema(Some(db))` 保留数据库。
3. Loader 只为保留下来的数据库调用 `ListTables`，因此被过滤数据库的表也不会读取。
4. `information_schema` 与 `metrics_schema` 两个内存虚拟库随后无条件注入，不受 Filter 控制。
5. cross-keyspace Loader 走独立系统库分支，而且其构造函数 `NewLoaderForCrossKS` 不接收 Filter。

## 数据与状态

`filter.rs` 自身没有字段、全局变量或可变状态。两个方法都通过共享引用 `&self` 调用；具体实现可以持有状态，但必须自行满足 `Send + Sync`，并自行保证内部同步和判定稳定性。

输入类型由 crate 根 `pkg/infoschema/issyncer/lib.rs` 定义：`SchemaDiff` 保存动作类型、版本及新旧库表 ID；`SchemaInfo` 保存当前版本及数据库/表快照并提供按 ID/名查询；`DBInfo` 保存数据库 ID、原始名、小写名和可选完整模型。Loader 以 `Option<Arc<dyn Filter>>` 保存策略，`Arc` 管理共享所有权，`Option` 表示“不筛选”。

过滤结果不是缓存的一部分：每次相关加载分支都会调用 trait 方法。实现者若依赖外部可变状态，应注意同一 schema 版本在不同时间得到不同结果可能造成缓存内容和增量链不一致。

## 依赖与调用关系

本文件仅从 crate 根导入 `DBInfo`、`SchemaDiff`、`SchemaInfo`，没有直接外部 crate 调用。三种类型背后的完整元数据能力来自 `pkg/infoschema/issyncer/lib.rs` 及 Cargo 依赖 `astersql-meta-model`、`astersql-infoschema`，但本 trait 不直接操作这些依赖。

主要上游接线为：

- `Syncer::New` / `newSyncer`（`pkg/infoschema/issyncer/syncer.rs`）接收 `Option<Arc<dyn Filter>>` 并传给 `newLoader`。
- `newLoader`（`pkg/infoschema/issyncer/loader.rs`）把它存入 `Loader::filter`。
- `Loader::skipLoadingDiffWithLatest` 调用 `SkipLoadDiff`。
- `Loader::fetchAllSchemasWithTables` 调用 `SkipLoadSchema`。
- `pkg/infoschema/issyncer/loader_test.rs` 的 `TestNameFilter` 是当前仓库内直接可见的 Rust 实现和行为验证载体。

RustCodeGraph 将 `filter.rs` 标为被 `loader.rs`、`loader_test.rs`、`syncer.rs` 使用，并确认 `fetchAllSchemasWithTables <- LoadWithTS`、`Syncer::New -> newSyncer`、`skipLoadingDiff -> skipLoadingDiffWithLatest`。动态 trait 调用的 callers/callees 查询部分关联到了同名 Go 符号或其他同名实现，因此具体 Rust 调用边又以这些文件中的直接引用核对。

## 错误处理与边界

两个 trait 方法都返回 `bool`，不返回 `Result`，因此过滤器不能通过接口传播可恢复错误。实现者若 panic，会沿 Loader 调用栈展开；本文件不捕获 panic。需要失败语义的新策略不能悄悄把错误折叠成“跳过”或“保留”，而应先评估并统一修改 trait、Loader 调用点和所有实现。

已验证的重要边界包括：

- `latestIS` 可以为 `None`；测试实现此时对需要按库名判断的 diff 选择跳过。
- `dbInfo` 可以为 `None`；测试实现返回“不跳过”。
- 没有 Filter 时，Loader 不做策略过滤。
- Filter 对增量 diff 有第一决策权；返回 `true` 后不会继续执行 cross-keyspace 判定或应用 diff。
- 全量过滤仅作用于存储返回的数据库；两个内存虚拟库始终注入。
- cross-keyspace 构造路径明确不带 Filter，其系统库/保留 ID 规则不是该 trait 的实现。

独立 Rust 回归测试 `test_loader_skip_loading_diff_for_br` 与 `test_load_for_br`（`pkg/infoschema/issyncer/loader_test.rs`）覆盖系统库、BR 临时库、用户库、未知库、无缓存、无 Filter、全量虚拟库注入及若干全局 DDL 动作。Go 对照测试位于 `pkg/infoschema/issyncer/loader_test.go`。

## 并发与资源生命周期

`Filter: Send + Sync` 是本文件唯一的并发保证：实现对象通常由 `Arc<dyn Filter>` 持有，可被 Loader/Syncer 共享。trait 不要求 `Clone`，也不规定锁类型；实现者若需要可变状态，应使用线程安全的内部可变性，并避免在判定中长时间阻塞。

Filter 的生命周期由传入 `Syncer::New` 或 `newLoader` 的 `Arc` 决定，Loader 持有期间对象保持存活，Loader 释放后引用计数递减。本文件不创建线程、任务、通道、锁、文件句柄或事务。`Loader::skipLoadingDiff` 为取得 `latest` 会短暂锁住 InfoCache，但先克隆快照再调用 Filter，因此 Filter 回调执行时不持有该缓存锁；全量路径调用 `SkipLoadSchema` 时也没有由本文件引入的资源持有。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/issyncer/filter.go`。Rust 保留了 Go `Filter` 的两个方法名、布尔语义和可空参数语义：Go 的 `*model.SchemaDiff` / `infoschema.InfoSchema` / `*model.DBInfo` 分别映射为 Rust 的 `&SchemaDiff` / `Option<&SchemaInfo>` / `Option<&DBInfo>`。

主要差异是：

- Go interface 没有显式并发标记；Rust trait 加上 `Send + Sync`，与 `Arc<dyn Filter>` 的共享用法配套。
- Go 使用完整 `infoschema.InfoSchema` 接口和 `model` 类型；当前 Rust crate 使用本地 `SchemaInfo`、`SchemaDiff`、`DBInfo` 表示，其字段和查询能力由 `lib.rs` 定义。
- Go Loader 字段是可空 interface，Rust 使用 `Option<Arc<dyn Filter>>` 明确表达可选性和共享所有权。
- Go 测试 `testNameFilter` 放行 `ActionCreateSchema` 与 `ActionCreatePlacementPolicy`；当前 Rust 测试实现还放行 placement-policy 的 alter/drop 及 resource-group 的 create/drop/alter。这个差异属于测试策略实现，不是 `filter.rs` trait 本身增加了默认行为。

因此，扩展时应以 Go 接口契约作为兼容基线，同时以 Rust Loader 的实际调用顺序和本地模型能力为准，不能把某个测试 Filter 的规则误写成 trait 默认规则。

## 扩展指南

新增筛选策略时，优先新增独立生产实现并实现 `Filter`，不要把场景规则硬编码进本接口。实现 `SkipLoadDiff` 时应分别考虑无缓存、零 `SchemaID`、未知库、跨库变更的 `OldSchemaID`、全局 DDL 动作以及是否允许 schema 版本只推进而不应用内容；实现 `SkipLoadSchema` 时应明确 `None` 和大小写语义。

若只改变某类调用方的筛选范围，修改该调用方的具体 Filter，并同步扩展 `pkg/infoschema/issyncer/loader_test.rs`；Go 语义也发生变化时同步核对 `pkg/infoschema/issyncer/filter.go` 和 `loader_test.go`。若改变方法签名或布尔含义，则至少需要同步 `filter.rs`、`loader.rs`、`syncer.rs`、所有 `impl Filter` 以及 crate 对外用户。

兼容风险主要是误跳过 DDL 导致 InfoSchema 与存储版本内容不一致；性能风险主要是每个 diff/数据库都执行昂贵或阻塞式判定；并发风险来自有状态实现内部同步。安全扩展应保持判定快速、无副作用且对同一输入稳定，并为增量和全量两条路径分别添加回归用例。Rust 测试必须继续放在独立测试文件，不能内嵌进 `filter.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引；目标目录包含 `filter.rs`、`loader.rs`、`syncer.rs` 与独立 `loader_test.rs`。
- RustCodeGraph `node --file pkg/infoschema/issyncer/filter.rs`：确认文件仅有 `Filter` trait 及两个必需方法，并显示使用文件为 `loader.rs`、`loader_test.rs`、`syncer.rs`。
- RustCodeGraph `node Filter --file ...`、`node skipLoadingDiff --file ...`、`node fetchAllSchemasWithTables --file ...`、`node New --file pkg/infoschema/issyncer/syncer.rs`：确认接口、注入链、增量入口和全量入口。
- RustCodeGraph 调用边：`Loader::skipLoadingDiff -> skipLoadingDiffWithLatest`、`LoadWithTS -> fetchAllSchemasWithTables`、`Syncer::New -> newSyncer`。对动态 trait 调用，另以 `rg` 的精确直接引用核对。
- 已读生产文件：`pkg/infoschema/issyncer/filter.rs`、`lib.rs`、`loader.rs`、`syncer.rs`、`Cargo.toml`；`pkg/infoschema` 下不存在需先读取的 `doc.go`。
- 已读 Go 对照：`pkg/infoschema/issyncer/filter.go`，以及 `loader.go` 中对应调用位置。
- 已读独立测试：`pkg/infoschema/issyncer/loader_test.rs` 与 `loader_test.go`，重点核对 BR diff 筛选和全量库筛选用例。
- 本任务是纯文档分析，按计划不运行 Cargo；交付时以固定十一章节结构命令验证文件存在和章节数量。
