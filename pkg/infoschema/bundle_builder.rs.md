# `pkg/infoschema/bundle_builder.rs`

## 文件定位

本文件属于 `astersql-infoschema` crate：`pkg/infoschema/Cargo.toml` 将 `lib.rs` 设为 crate 根，`pkg/infoschema/lib.rs` 以 `pub mod bundle_builder` 挂载本模块。它位于 InfoSchema 构建末端的 placement bundle 派生阶段，不读取存储，也不把规则下发到 TiKV；它只依据调用方提供的元数据视图，在内存中计算 `physical_id -> Arc<PlacementBundle>` 映射。

生产入口在 `pkg/infoschema/builder.rs`：`Builder::Build` 调用 `Builder::build_bundles`，后者用当前数据库、表和 policy 集合构造 `BuilderBundleSchema`，再驱动本文件的 `bundleInfoBuilder`。构建结果随后由 `Builder::Build` 注入 v1 `infoSchema` 或 v2 `infoschemaV2`。文件末尾还有同名包级函数 `updateInfoSchemaBundles`，它只是委托 builder 方法；当前 Rust 生产主链直接调用方法，没有经过这个薄包装。

## 核心职责

- 用 `BundleSchema` 隔离 bundle 算法与 `Builder` 的具体元数据存储，使生产适配器和独立测试都能提供 policy、单表规格和全表规格。
- 在全量模式下清空旧结果并遍历全部表，在增量模式下继承旧缓存，只重算显式标记的表及受表级 policy 变更影响的表。
- 对每张表及其分区按可选 `policy_id` 执行 upsert：有 policy 时生成 bundle，无 policy 时删除对应物理 ID 的旧 bundle。
- 将找不到 policy 的错误收集为 `Vec<String>`，继续处理同表的其他分区和其他表；上游 `Builder::build_bundles` 逐条记录 warning。

当前实现是迁移中的精简表示：`PlacementBundle.rules` 是 `Vec<String>`，`upsert_bundle` 只写入 `policy:<id>:<原始名称>`。它没有复刻 Go `placement.NewTableBundle` / `NewPartitionBundle` 生成 PD placement rules 的完整语义，因而这里的结果应理解为 InfoSchema 内部的规则占位表示，而不是完整可下发的 PD bundle。

## 主要符号

- `PartitionBundleSpec { partition_id, policy_id }`：单个分区的最小输入。两个字段公开；`policy_id == None` 表示该物理分区不应保留独立 bundle。
- `TableBundleSpec { table_id, policy_id, partitions }`：一张表及其分区的构建快照。生产适配器 `BuilderBundleSchema::table_bundle_spec` 从 `Table::Meta().model_meta` 生成它；分区没有显式 policy 时，适配器会把表级 `policy_id` 作为分区的有效 policy。
- `BundleSchema`：只读输入 trait。`policy_by_id` 返回共享的 `PolicyInfo`，`table_bundle_spec` 支持按表增量查询，`all_table_bundle_specs` 支持全量扫描和 policy 影响扩展。
- `policyGetter<'a>`：持有 `&dyn BundleSchema` 的内部适配器。`GetPolicy` 把 `Option` 转成 `Result`，并保持 Go 错误文案 `Cannot find placement policy with ID: ...`。
- `bundleInfoBuilder`：状态机主体。`delta_update` 选择构建模式；`update_tables`、`update_policies` 保存本轮脏标记；`bundles` 保存继承或新建的结果。
- `new` / `initBundleInfoBuilder`：前者创建完全默认的 builder；后者只清空两组脏标记，不重置 `delta_update` 或 `bundles`。
- `SetDeltaUpdateBundles` / `inherit_bundles`：进入增量模式。后者同时接管一份旧 bundle map。
- `deleteBundle`、`markTableBundleShouldUpdate`、`markBundlesReferPolicyShouldUpdate`、`bundles`：分别删除结果、登记表脏标记、登记 policy 脏标记和只读暴露结果。
- `updateInfoSchemaBundles`（方法）：全量/增量调度入口；文件末尾的同名自由函数只是兼容式转发。
- `completeUpdateTables`：把“引用了变更 policy 的表”加入 `update_tables`。它只检查 `TableBundleSpec.policy_id`，不检查 `partitions[*].policy_id`。
- `updateTableBundles`：重算一张表及全部分区；表已不存在时只删除该 `table_id` 的旧条目并返回。
- `upsert_bundle`：单个物理 ID 的写入/删除原语，也是 policy 查找错误的产生点。

本文件没有条件编译项；仅用 crate 级 `allow(non_camel_case_types, non_snake_case)` 保留与 Go 对齐的名称。

## 执行流程

1. `Builder::Build` 在生成最终 InfoSchema 前调用 `Builder::build_bundles`（`pkg/infoschema/builder.rs`）。
2. `build_bundles` 构造 `BuilderBundleSchema { databases, policies }` 和新的 `bundleInfoBuilder`。若 `Builder` 是从旧 InfoSchema 初始化的增量构建，则通过 `inherit_bundles` 复制旧 map，并把 `bundle_updates`、`bundle_policy_updates` 分别写入两组脏标记。
3. `updateInfoSchemaBundles` 检查 `delta_update`：
   - 全量模式先清空 `bundles`，再对 `all_table_bundle_specs()` 中的每张表调用 `updateTableBundles`；
   - 增量模式先由 `completeUpdateTables` 扩展表级 policy 的影响集合，再复制 `update_tables` 的 ID，逐表调用 `updateTableBundles`。复制 ID 避免迭代集合时继续可变借用 builder。
4. `updateTableBundles` 先调用 `table_bundle_spec(table_id)`。查不到表时删除同 ID 的旧 bundle；查到时先处理表，再依次处理其全部分区。每个物理对象都交给 `upsert_bundle`。
5. `upsert_bundle` 对 `None` policy 删除旧条目并成功返回；对 `Some(id)` 先经 `policyGetter::GetPolicy` 校验 policy 存在，再以物理 ID 为键覆盖写入新的 `Arc<PlacementBundle>`。
6. 所有错误被聚合返回。`Builder::build_bundles` 用 `tracing::warn!` 记录错误，仍克隆并返回已经成功构建的 map。

因此，一个坏 policy 不会形成事务式回滚：成功条目保留，失败物理 ID 的既有条目也不会被 `upsert_bundle` 主动删除。

## 数据与状态

`bundleInfoBuilder` 是一次构建过程的可变工作区。默认值为全量模式、空脏集合和空结果；`inherit_bundles` 让旧 InfoSchema 的 `Arc<PlacementBundle>` 进入新一轮增量构建。`HashSet` 负责脏 ID 去重，`HashMap` 保证每个物理 ID 最多对应一个 bundle，但两者迭代顺序不稳定，因此错误列表及重建顺序不应被视为稳定 API。

`initBundleInfoBuilder` 的语义容易误用：它相当于“开始新的脏标记批次”，而不是完全重置对象。调用后旧 `bundles` 和增量模式仍然保留。生产 `Builder` 每次 `build_bundles` 都创建新 builder，当前只有独立测试显式复用并调用该方法。

共享所有权通过 `Arc` 表达：policy 可由 schema 视图共享返回，bundle 可从旧 InfoSchema 继承并在新 InfoSchema 中共享。更新某个物理 ID 时会分配新的 `Arc<PlacementBundle>`，不会原地修改旧 bundle。

生产适配器的数据规则位于 `BuilderBundleSchema`（`pkg/infoschema/builder.rs`）：它跨 `databases` 查找表，从 `model_meta` 读取表级 policy 和分区定义，并让未显式指定 policy 的分区继承表级 policy。这是本文件最终输入语义的一部分，但不由 `bundleInfoBuilder` 自身实现。

## 依赖与调用关系

上游调用链为 `Builder::Build -> Builder::build_bundles -> bundleInfoBuilder::updateInfoSchemaBundles`。`Builder::ApplyDiff` 在增量模式下把受 schema diff 影响的表 ID 写入 `bundle_updates`；placement policy 的创建、修改或删除则把 policy ID 写入 `bundle_policy_updates`。`Builder::InitWithOldInfoSchema` 从旧快照提取 `AllPlacementBundles()` 作为缓存并开启增量模式；`InitWithDBInfos` 则清空缓存并选择全量模式。

本文件的直接下游很小：标准库 `HashMap`/`HashSet` 和 `Arc`，以及同 crate 的 `PolicyInfo`、`PlacementBundle`。`pkg/infoschema/Cargo.toml` 没有为本文件引入额外外部 crate；日志发生在调用方 `builder.rs`，使用 crate 已声明的 `tracing`。

RustCodeGraph 的文件节点确认 `bundle_builder.rs` 被 `pkg/infoschema/builder.rs`、独立测试及其他模块引用；精确源码与仓库搜索确认真正构建接线集中在 `builder.rs`。同名 Go/Rust 符号很多，查询通用名称时会混入 `pkg/infoschema/bundle_builder.go`，因此调用结论以带文件路径的图节点及直接引用交叉核验。

## 错误处理与边界

- 唯一显式错误是 policy ID 在 `BundleSchema` 中不存在，文案由 `policyGetter::GetPolicy` 生成。公共更新接口返回字符串列表，没有结构化错误类型。
- `updateTableBundles` 不会因首个错误停止：表 bundle 失败后仍处理分区，一个分区失败后仍处理后续分区；`bundle_builder_test.rs::bundle_errors_do_not_stop_remaining_partitions_or_tables` 验证两个缺失 policy 会产生两个错误，而有效分区仍写入。
- policy 缺失时 `upsert_bundle` 在写 map 前返回，因此同一 physical ID 若已有继承条目，该旧值会保留。当前测试没有覆盖这一点；修改错误语义时应先补独立回归测试。
- 增量模式下，表不存在只删除 `table_id` 的旧条目，无法从缺失的表规格得知它原先有哪些分区，因此旧分区 bundle 不会在此分支被一并清理。调用方必须把相关旧物理 ID 纳入更新，或在上游维护正确的缓存淘汰；当前独立测试未验证此清理契约。
- `completeUpdateTables` 只检查表级 policy。`changed_partition_policy_does_not_mark_its_table_for_delta_update` 明确验证：若 policy 只被分区引用，仅标记 policy 变化不会刷新该分区，旧规则仍存在。这是当前实现限制，不是“已支持”的行为。
- 全量模式会先清空 map；若随后出现错误，最终结果是部分成功的全量快照。增量模式则在继承 map 上逐项覆盖，错误时可能保留旧值。
- `None` policy 是正常删除信号，不是错误。重复表/分区 ID 会按 `HashMap::insert` 的最后一次成功写入覆盖，代码不主动检测元数据 ID 冲突。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务或 I/O。所有更新都要求 `&mut bundleInfoBuilder`，Rust 借用规则保证一次调用内部对状态的独占修改；`BundleSchema` 仅以共享引用读取。类型本身没有承诺供多个线程同时修改，若上层需要并发，应在 builder 外部完成串行化。

`Arc` 只管理 `PolicyInfo` 和 `PlacementBundle` 的共享生命周期，不提供对 builder map 的并发写能力。全量构建时旧 map 在 `clear` 后释放其持有的引用；增量覆盖或删除时，仅减少当前 map 的引用计数，仍被旧 InfoSchema 持有的 bundle 会继续存活。函数退出时没有额外清理动作或后台资源。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/infoschema/bundle_builder.go`。Rust 保留了 `policyGetter`、`bundleInfoBuilder`、`initBundleInfoBuilder`、`SetDeltaUpdateBundles`、两种 mark 方法、`updateInfoSchemaBundles`、`completeUpdateTables` 和 `updateTableBundles` 的总体分工，以及“增量只更新脏表、全量遍历所有表”“policy 影响扩展只看表级引用”的控制流。

两版的关键差异如下：

- Go builder 直接修改 `infoSchema.ruleBundleMap`；Rust builder 自带 `bundles` map，完成后由 `Builder::Build` 注入新 InfoSchema。这让 Rust 可以先构建结果再发布快照。
- Go `updateTableBundles` 调用 `placement.NewTableBundle` 与 `placement.NewPartitionBundle`，生成完整 `placement.Bundle`；Rust `upsert_bundle` 只生成包含 policy ID/名称的字符串规则。Go 的行为由 `pkg/ddl/placement/meta_bundle_test.go` 及 `pkg/infoschema/infoschema_test.go::TestBuildBundle` 验证，不能把这些完整规则能力归于当前 Rust 文件。
- Go 在构建失败处直接记录日志并继续；Rust 把全部错误作为 `Vec<String>` 返回，由 `Builder::build_bundles` 统一记录。两者都采用部分成功语义。
- Go 的 bundle map 位于 InfoSchema，因此 `deleteBundle` 接受 InfoSchema；Rust 删除自己的 map。Rust 额外提供 `inherit_bundles`、`bundles`、输入规格结构和 `BundleSchema` trait，以适配其快照式 Builder。
- Rust 生产适配器会让没有显式 policy 的分区继承表级 policy；Go 文件把原始 `PartitionDefinition` 交给 `NewPartitionBundle`。是否生成相同实际规则取决于 Go placement 层语义，当前精简 Rust 表示不能证明完整等价。
- Go 的 `infoschema_v2_test.go` 还覆盖表 placement、policy 修改和分区 placement 的端到端更新；这些是 Go 整体 InfoSchema/placement 链证据，不是当前 Rust 单元测试已经覆盖的等价保证。

## 扩展指南

- 若要补齐真实 placement 规则生成，应优先替换或扩展 `upsert_bundle` 的输入与下游类型，并对照 `pkg/ddl/placement/bundle.rs` 及 Go 的 `NewTableBundle` / `NewPartitionBundle`；不要只扩写字符串格式。同步更新独立的 `pkg/infoschema/bundle_builder_test.rs`，必要时在 placement crate 的独立测试中验证 leader/follower、constraints、survival preferences 和 range key 等语义。
- 若要让 policy 修改覆盖仅由分区引用的表，应修改 `completeUpdateTables`，扫描 `TableBundleSpec.partitions`；先把现有“不会刷新”测试改造成期望的新回归测试，并同时核对 Go 当前行为与 v2 的 `completeUpdateTablesV2`，避免无意改变兼容契约。
- 若要保证表删除时清除旧分区 bundle，需要让输入或缓存保留旧分区 ID，不能只依赖已经消失的 `table_bundle_spec`。应新增删除/截断/交换分区的增量测试，验证所有旧 physical ID 都被清理。
- 若改变错误策略，需明确全量与增量是否回滚、删除旧值或保留旧值，并测试“表 bundle 失败但分区成功”“中间分区失败但后续成功”“继承旧值后 policy 消失”等情形。
- 新测试必须继续放在独立的 `pkg/infoschema/bundle_builder_test.rs`，不要嵌入生产源文件。涉及生产接线时还应检查 `Builder::ApplyDiff` 的 `bundle_updates` / `bundle_policy_updates` 收集和 `BuilderBundleSchema` 的继承规则。
- 性能上，全量构建和 `completeUpdateTables` 都扫描全部表；大量 policy 变更仍只触发一次全表扫描，但 `BuilderBundleSchema::table_bundle_spec` 会跨所有数据库查找表。优化时要保持去重、删除语义及错误继续处理行为。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引包含 Rust/Go；`node --file pkg/infoschema/bundle_builder.rs --offset 1 --limit 500` 读取了目标文件全部 192 行；`query` 核对了 `bundleInfoBuilder`、`BundleSchema`、`policyGetter`、`updateInfoSchemaBundles`、`completeUpdateTables`、`updateTableBundles`、`upsert_bundle`；`node` 读取了 `builder.rs` 的生产接线、`bundle_builder_test.rs` 全部测试、`lib.rs` 模块声明和 `infoschema.rs` 的 `PolicyInfo` / `PlacementBundle` 定义。
- Rust 源与测试：`pkg/infoschema/bundle_builder.rs`；`pkg/infoschema/builder.rs`；`pkg/infoschema/bundle_builder_test.rs`；`pkg/infoschema/lib.rs`；`pkg/infoschema/infoschema.rs`。
- crate 边界：`pkg/infoschema/Cargo.toml`，确认 crate 名为 `astersql-infoschema`、根为 `lib.rs`，本文件仅使用标准库和 crate 内类型。
- Go 对照与测试：`pkg/infoschema/bundle_builder.go`；`pkg/ddl/placement/meta_bundle_test.go::{TestNewTableBundle, TestNewPartitionBundle}`；`pkg/infoschema/infoschema_test.go::TestBuildBundle`；`pkg/infoschema/infoschema_v2_test.go` 中 placement bundle 的增量更新场景。
- 人工复核结论：本文件存在于 InfoSchema 快照发布前，把表/分区的 policy 绑定转成可查询的物理 ID bundle 缓存；安全扩展必须同时维护全量/增量、旧缓存清理、部分失败和 Go placement 语义四类契约。本文没有把当前字符串占位规则表述为完整 PD placement 支持。
