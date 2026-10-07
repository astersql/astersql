# `pkg/infoschema/builder_misc.rs`

## 文件定位

本文件属于 `astersql-infoschema` crate；crate 根 `pkg/infoschema/lib.rs` 以 `pub mod builder_misc` 公开该模块。它不是 Placement Policy、Resource Group 或临时表状态的实际存储实现，而是一个兼容 Go 命名的薄适配层：把杂项 DDL 操作转换成 `pkg/infoschema/builder.rs` 中统一的 `Builder::ApplyDiff` 调用，或直接转发到 `Builder::addTemporaryTable`、`Builder::initMisc`。

`pkg/infoschema/Cargo.toml` 将 crate 根设为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/infoschema"` 标明 Go 对照包。目标文件只直接使用同 crate 的 `builder` 与 `infoschema` 模块，不引入新的外部依赖或 feature 条件。

## 核心职责

- `policy_diff` 复制输入 `SchemaDiff`，将 `table_id` 改写为 `schema_id`，并替换 `action_type`。原因是 Go 的策略/资源组 diff 以 `SchemaID` 携带对象 ID，而 Rust 的统一分派器 `Builder::ApplyDiff` 对这些动作从 `diff.table_id` 取 ID。
- 五个 DDL 包装入口把创建、修改、删除 Placement Policy 和 Resource Group 映射为相应 `ActionType`，统一交给 `Builder::ApplyDiff`。
- `addTemporaryTable` 与 `initMisc` 保留 Go 风格的模块级入口，分别委托 Builder 登记临时表，以及批量注入初始策略和资源组。
- 本文件不自行读取元数据、不持有缓存、不构建最终 `InfoSchema`；真正的状态变更和错误产生于 `builder.rs`。

## 主要符号

- `fn policy_diff(source: &SchemaDiff, action_type: ActionType) -> SchemaDiff`：私有转换器。保留 `version`、`schema_id`、old ID、`affected_options` 和 `sub_action_types` 等字段，只覆盖动作类型与 `table_id`；输入通过 `clone` 保持不变。
- `pub fn applyCreatePolicy(...) -> Result<(), String>`：生成 `CreatePlacementPolicy` diff，调用 `ApplyDiff`，并用 `map(|_| ())` 丢弃受影响 ID 列表。
- `pub fn applyAlterPolicy(...) -> Result<Vec<i64>, String>`：生成 `AlterPlacementPolicy` diff，并保留 `ApplyDiff` 返回的受影响表 ID。
- `pub fn applyDropPolicy(...) -> Result<Vec<i64>, String>`：生成 `DropPlacementPolicy` diff，并保留受影响表 ID。
- `pub fn applyCreateOrAlterResourceGroup(..., alter: bool) -> Result<(), String>`：`alter` 为真时选择 `AlterResourceGroup`，否则选择 `CreateResourceGroup`；两条路径都丢弃空的受影响 ID 列表。
- `pub fn applyDropResourceGroup(...) -> Result<Vec<i64>, String>`：生成 `DropResourceGroup` diff，返回统一分派器的结果。
- `pub fn addTemporaryTable(builder: &mut Builder, table_id: i64)`：调用同名 Builder 方法；后者同时更新 Builder 的 `temporary_table_ids` 与 v2 `Data`。
- `pub fn initMisc(builder: &mut Builder, policies: Vec<PolicyInfo>, resource_groups: Vec<ResourceGroupInfo>)`：按值接收两个集合并交给 Builder；Builder 以 ID 为键扩展内部 map。

文件没有模块级常量、类型、trait、条件编译项或异步入口。除 `policy_diff` 外的函数均为公开 API，但仓库内 Rust 搜索未发现这些模块级包装函数的调用者；现有 Rust 主链和测试直接调用 Builder 方法。

## 执行流程

策略和资源组 DDL 包装函数遵循同一流程：

1. 调用者提供可变 `Builder`、`MetadataReader` 和原始 `SchemaDiff`。
2. `policy_diff` 把原始 `schema_id` 复制到新 diff 的 `table_id`，并写入目标 `ActionType`。
3. `Builder::ApplyDiff` 先用 diff 的 `version` 更新 Builder schema 版本，再按动作分派。
4. 创建/修改策略通过 `MetadataReader::policy` 读取对象并写入 `Builder.policies`；修改还调用 `tables_referencing_policy` 收集受影响表。当前该辅助函数在 Rust 中明确返回空向量。
5. 删除策略直接按 ID 从 `Builder.policies` 删除，再查询关联表；创建/修改资源组通过 `MetadataReader::resource_group` 写入 `Builder.resource_groups`，删除则按 ID 移除。
6. 包装函数按 API 约定返回 `()` 或受影响 ID；错误原样向上传播。

`addTemporaryTable` 不经过 diff 分派：它立即把 ID 插入 Builder 的集合，并通知 `info_data.addTemporaryTable`。`initMisc` 也不经过元数据读取：它把传入对象按自身 ID 合并到 Builder 的策略/资源组 map。全量初始化 `Builder::InitWithDBInfos` 在库表初始化完成后调用 `Builder::initMisc`，体现这些集合依赖 Builder 已建立的主体状态。

## 数据与状态

本文件唯一创建的新值是经 `policy_diff` 克隆得到的短生命周期 `SchemaDiff`。`source` 不被修改；`action_type` 和 `table_id` 被覆盖，其中 `table_id = source.schema_id` 是本适配层最重要的不变量。若调用者没有把策略或资源组 ID 放在 `schema_id`，包装函数会以错误 ID 读写状态。

持久状态均在 `Builder`：`policies: HashMap<i64, PolicyInfo>`、`resource_groups: HashMap<i64, ResourceGroupInfo>`、`temporary_table_ids: HashSet<i64>`，以及共享的 `Arc<Data>`。`initMisc` 的 `extend` 语义会以同 ID 的新对象覆盖旧值；`addTemporaryTable` 的集合插入天然去重。最终 `Builder::Build` 将这些状态投影到 v1 或 v2 InfoSchema；本文件不负责快照发布。

## 依赖与调用关系

上游边界如下：

- `pkg/infoschema/lib.rs` 公开模块，但 RustCodeGraph 对目标文件报告 `used by 0 files`，并且对关键函数的 `callers` 查询没有产生调用边。
- Rust 仓库文本搜索同样未发现目标文件之外对这些包装函数的调用；当前 Rust 流程主要直接使用 `Builder::ApplyDiff`、`Builder::initMisc` 和 `Builder::addTemporaryTable`。
- Go 主链 `pkg/infoschema/builder.go` 的动作分派会直接调用 `builder_misc.go` 中同名函数，因此这些 Rust API 主要承担移植对齐和未来接线接口的角色。

下游依赖全部位于同 crate：`SchemaDiff`、`ActionType`、`Builder`、`MetadataReader` 来自 `builder.rs`，`PolicyInfo` 与 `ResourceGroupInfo` 来自 `infoschema.rs`。关键调用边是 `apply* -> policy_diff -> Builder::ApplyDiff`、`addTemporaryTable -> Builder::addTemporaryTable -> Data::addTemporaryTable`、`initMisc -> Builder::initMisc`。

## 错误处理与边界

所有 diff 包装函数使用 `Result<_, String>` 并直接传播 `Builder::ApplyDiff` 的错误。创建或修改策略时，`MetadataReader::policy` 的读取错误会透传，返回 `None` 则产生 `policy {id} not found`；资源组路径对应 `resource group {id} not found`。`MetadataReader` 对这两类读取的 trait 默认实现均返回 `Ok(None)`，因此只实现库表读取的 reader 不能成功执行创建/修改操作。

删除策略和资源组对不存在的 ID 是幂等的：底层 `HashMap::remove` 不报错。传给删除资源组包装函数的 `metadata` 仍满足统一签名，但当前 `DropResourceGroup` 分支并不读取它。`policy_diff` 不校验 ID 是否为正，也不改变版本；版本会在底层状态变更前由 `ApplyDiff` 设置，因此后续元数据读取失败时，Builder 的 schema 版本已经被更新，这是调用者需要理解的现有语义。

创建策略和创建/修改资源组包装函数主动丢弃 `ApplyDiff` 的 ID 向量；修改/删除策略和删除资源组保留它。当前 Rust 的策略关联表扫描尚未实现，`tables_referencing_policy` 返回空，因此修改/删除策略实际也返回空向量。不能把接口上的 `Vec<i64>` 误解为已经完整计算了关联表。

## 并发与资源生命周期

所有入口同步执行并要求独占的 `&mut Builder`，所以本文件没有锁、任务、通道、事务或异步取消逻辑；同一个 Builder 的并发修改由 Rust 借用规则在调用边界排除。`MetadataReader` 仅以共享引用借用，生命周期限于一次调用。

`policy_diff` 的克隆值只存活到 `ApplyDiff` 返回。`initMisc` 消费传入向量，把元素所有权移入 Builder map；`addTemporaryTable` 只复制 `i64`。Builder 内的 `Arc<Data>` 可能被 v2 InfoSchema 共享，具体同步由 `Data` 自身的锁负责，而目标文件不持锁，也没有跨调用保持锁的风险。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/builder_misc.go`。公开意图一一对应：创建/修改/删除策略、创建或修改/删除资源组、登记临时表、初始化杂项集合。

实现结构存在重要差异：Go 函数直接调用 `meta.Reader` 并操作 `b.infoSchema`，使用 `diff.SchemaID`；Rust 包装层将 `schema_id` 搬到 `table_id` 后复用 `Builder::ApplyDiff`。Go 的 create/alter resource group 由单一函数处理而不接收布尔参数；动作由 `builder.go` 的分派决定，Rust 包装函数则用 `alter` 显式选择动作。

Go 在重复创建策略时会标记引用该策略的 bundle 更新，修改策略也会标记；Rust 底层目前只覆盖 policy map，并且 `tables_referencing_policy` 是返回空向量的迁移占位。Go 的策略/资源组缺失错误使用结构化 `ErrPlacementPolicyNotExists`、`ErrResourceGroupNotExists`，Rust 当前使用字符串错误。Go `initMisc` 还接收但忽略 masking policies 参数，Rust 签名只保留 policies 与 resource groups。上述差异是当前代码事实，不应描述为完整语义等价。

## 扩展指南

- 新增杂项 DDL 包装时，应先在 `ActionType` 和 `Builder::ApplyDiff` 建立真实行为，再在本文件做最小 ID/动作适配，避免在门面重复维护状态逻辑。
- 修改 `policy_diff` 时必须保持输入不变，并确认所有调用者是否仍以 `schema_id` 携带对象 ID；若统一分派器改为读取 `schema_id`，应同步删除这层搬运而不是留下双重转换。
- 补齐 Go 语义时，优先实现 `Builder::tables_referencing_policy` 和 bundle 更新规则，并在独立测试文件中验证修改/删除策略返回的表 ID；不要把测试写入本生产文件。
- 应在 `pkg/infoschema/builder_test.rs` 或相邻独立 `*_test.rs` 中增加对本文件公开包装函数的直接回归，至少覆盖 create/alter/drop、`alter` 两分支、缺失元数据、错误透传、原始 diff 未修改，以及 `schema_id -> table_id` 映射。
- 性能风险主要来自 `SchemaDiff::clone` 中的 `affected_options`/`sub_action_types` 复制，以及未来真实实现关联表扫描后的复杂度；兼容风险集中于 Go 结构化错误与 Rust 字符串错误、bundle 更新缺口和受影响 ID 返回值。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/infoschema/builder_misc.rs` 确认目标文件 108 行、11 个符号并报告 `used by 0 files`；`node --file ... --offset 1 --limit 500` 用于读取完整目标源码；`query` 确认 Rust/Go 同名符号；关键 `callers`/`callees` 查询无返回边，因此调用关系又以模块和源码搜索交叉核对。
- 生产源码：`pkg/infoschema/builder_misc.rs`（全部符号）、`pkg/infoschema/builder.rs`（`ActionType`、`SchemaDiff`、`MetadataReader`、`Builder::ApplyDiff`、三个委托目标及 `Build`/全量初始化接线）、`pkg/infoschema/lib.rs`（模块公开）、`pkg/infoschema/infoschema_v2.rs`（临时表 Data 状态）。
- crate 配置：`pkg/infoschema/Cargo.toml`（crate 根、同仓依赖和 Go 包移植元数据）。`pkg/infoschema` 下未发现 `doc.go`，因此没有额外包契约可读。
- Go 对照：`pkg/infoschema/builder_misc.go`（直接实现与语义差异）、`pkg/infoschema/builder.go`（Go 动作分派及全量初始化调用点）。
- 测试证据：`pkg/infoschema/infoschema_v2_test.go::TestMisc` 直接覆盖 Go 同名函数；`pkg/infoschema/infoschema_v2_test.rs::test_misc_resource_groups`、`test_bundles_via_apply_diff_policy`、`test_special_attribute_and_policies` 覆盖 Rust 委托目标；`pkg/infoschema/infoschema_test.rs::test_build_schema_with_global_temporary_table` 覆盖临时表登记。现有 Rust 测试未直接调用本文件包装函数，这是明确的覆盖限制。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终仅执行固定 11 章节结构检查并人工复核上述事实。
