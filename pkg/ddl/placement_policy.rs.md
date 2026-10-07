# `pkg/ddl/placement_policy.rs`

## 文件定位

本文件属于根 crate `astersql-ddl`；`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`，而 `pkg/ddl/lib.rs` 通过 `pub mod placement_policy` 将其公开。它提供一套只依赖标准库集合类型的放置策略内存模型，覆盖策略配置、名称/ID 索引、创建与修改、分步删除、引用归一化和占用检查。

需要特别区分当前 Rust 接线与完整 TiDB DDL：仓库内生产 Rust 文件没有直接调用这里的 `PlacementPolicyCatalog` 或辅助函数，直接使用集中在 `pkg/ddl/placement_policy_test.rs`、`pkg/ddl/placement_policy_ddl_test.rs` 和 `pkg/ddl/placement_sql_test.rs`。因此它目前是对 Go 放置策略语义的可测试内存化子集，不是 Go `pkg/ddl/placement_policy.go` 中持久化 DDL job、InfoSchema/Meta、PD rule bundle 更新链的完整替代品。

## 核心职责

- `PlacementPolicyCatalog` 同时维护 `policies: BTreeMap<i64, PolicyInfo>` 和小写名称到 ID 的 `by_name` 索引，并在成功变更后推进 `schema_version`。
- `create`、`alter`、`drop_step` 表达创建/替换、修改以及 `Public -> WriteOnly -> DeleteOnly -> None` 删除状态机；`drop_step` 在最终态同时清除 ID 与名称索引。
- `normalize_ref`、`handle_table_placement` 和 `remove_table_placement` 处理表及一级分区的策略引用；名称 `default` 表示清除引用。
- `check_not_in_use` 和 `depended_object_ids` 从数据库、表、分区及特殊 range 名称映射中判断或枚举依赖。
- `build_policy_info`、`set_direct_placement_opt` 与 `check_policy_validation` 将 AST 风格选项写入配置并执行本文件支持的最小合法性检查。
- `get_range_placement_policy_name` 只解析已经取得的 rule ID；`collect_policy_ids` 收集对象及其一级分区引用过的策略 ID。

## 主要符号

- `PolicyState::{None, Public, WriteOnly, DeleteOnly}`：策略可见性状态。新对象由 `build_policy_info` 构造成 `None`，`create` 发布为 `Public`，删除由 `drop_step` 每次推进一步。
- `PlacementSettings`：保存主区域、区域列表、各类副本数、调度模式、各种约束和存活偏好。全部字段是拥有所有权的 `String` 或 `u64`，默认值为空串/零。
- `PolicyInfo { id, name, state, settings }` 与 `PolicyRef { id, name }`：分别表示目录实体和对象上的引用。
- `PlacementObject { id, policy_ref, partitions }`：统一表示数据库或表；表可在 `partitions` 中嵌套一级分区。本文件的遍历只检查这一层，不递归更深层级。
- `PlacementOptionType`：把十二类直接放置选项映射到 `PlacementSettings` 字段。
- `PolicyError`：声明 `AlreadyExists`、`NotFound`、`InvalidState`、`InvalidOption`、`InvalidSettings`、`InUse` 和 `RangeBackend(String)`；当前实现实际产生前五者中的 `AlreadyExists`、`NotFound`、`InvalidState`、`InvalidSettings`、`InUse`，未产生 `InvalidOption` 或 `RangeBackend`。
- `PlacementPolicyCatalog`：核心可变状态容器。公开方法是 `create`、`alter`、`drop_step`、`by_name`、`normalize_ref`、`check_not_in_use` 和 `depended_object_ids`。
- 模块级公开函数：`check_policy_validation`、`build_policy_info`、`set_direct_placement_opt`、`remove_table_placement`、`handle_table_placement`、`get_range_placement_policy_name`、`collect_policy_ids`。内部辅助函数 `ref_matches`、`object_uses_policy` 只服务于引用判断。

## 执行流程

创建路径从 `build_policy_info(id, name, options)` 开始：逐项调用 `set_direct_placement_opt` 机械写字段，返回状态为 `None` 的 `PolicyInfo`。`PlacementPolicyCatalog::create` 会先无条件重置传入状态为 `None`，再调用 `check_policy_validation`。名称键使用 `to_ascii_lowercase`；同名且未允许替换时返回 `AlreadyExists`，允许替换时保留既有 ID、仅替换 settings 并增加版本。新建时状态改为 `Public`，依次写入名称索引和 ID 索引，再增加版本。

修改路径由 `alter(policy_id, settings)` 先检查 ID 是否存在，再验证新配置，最后要求旧策略处于 `Public`。成功后整体替换 settings 并增加版本。这一顺序保留了 Go `onAlterPlacementPolicy` 的错误优先级：不存在的策略即使携带非法配置也先返回 `NotFound`。

删除路径由 `drop_step` 每次调用先执行 `check_not_in_use`，然后推进一个状态。`Public`、`WriteOnly`、`DeleteOnly` 分别转为下一状态；原状态为 `None` 是非法状态。到达 `None` 时从两个索引移除对象。每个成功步骤都推进一次 `schema_version`，调用方必须重复调用三次才能完成从公开到移除。

引用处理时，`handle_table_placement(..., ignore = true)` 调用 `remove_table_placement` 清空表和全部一级分区引用，并返回是否真的发生清理；非 ignore 模式依次对表和分区调用 `normalize_ref`。空引用原样为空，名称大小写无关的 `default` 被转为空，其他名称通过目录查找并把真实 ID 回填到引用；任一名称不存在即提前返回 `NotFound`。

占用检查由 `object_uses_policy` 检查对象自身及其一级分区，再检查 `range_policy_names` 的值是否与策略名大小写无关地相等。`depended_object_ids` 分别生成数据库 ID、分区 ID、表 ID，返回顺序固定为 `(db_ids, partition_ids, table_ids)`；各列表保留底层 `Vec` 的迭代顺序。

## 数据与状态

`policies` 是 ID 权威表，`by_name` 是 ASCII 小写名称的辅助索引；正确性依赖二者同步。正常创建和最终删除同时维护二者，OR REPLACE 则通过既有名称找到旧 ID，因此调用方传入的新 ID 不生效。`by_name` 和 `policies` 是私有字段，外部不能绕过这些方法直接破坏索引。

`schema_version` 初始为零，使用 `saturating_add(1)`，到达 `u64::MAX` 后保持最大值而不溢出。版本只代表此内存目录中的成功变更次数，不等同于 Go 实现持久化的全局 schema version，也不会触发 follower schema sync。

`databases`、`tables` 和 `range_policy_names` 均由调用方公开填充；目录不会自动从真实 InfoSchema、Meta 或 PD 同步它们。`BTreeMap`/`BTreeSet` 让索引和 `collect_policy_ids` 结果具有按键排序的确定性；依赖 ID 的三个 `Vec` 则遵循对象输入顺序。

配置验证当前只有三个规则：`followers` 与 `voters` 不能同时大于零；非空 `primary_region` 必须精确匹配逗号切分并 `trim` 后的某个 region；非空 `schedule` 只能是大写 `EVEN` 或 `MAJORITY_IN_PRIMARY`。`set_direct_placement_opt` 并不会把 schedule 转成大写，尽管函数上方注释写有“统一转大写”；测试 `direct_schedule_assignment_preserves_go_input` 证明实际契约是原样赋值，语义校验发生在 create/alter 阶段。

## 依赖与调用关系

下游依赖只有 `std::collections::{BTreeMap, BTreeSet}` 以及 `std::fmt`/`std::error::Error`；本文件没有使用 `pkg/ddl/Cargo.toml` 中列出的 Meta、Parser、Placement 子 crate 或网络依赖。这也说明其目录、对象与错误类型是本地简化模型，而非共享 `astersql-meta-model` 类型。

模块内调用边为：`create` 和 `alter` 调用 `check_policy_validation`；`build_policy_info` 调用 `set_direct_placement_opt`；`drop_step` 调用 `check_not_in_use`；`check_not_in_use` 调用 `object_uses_policy`，后者调用 `ref_matches`；`depended_object_ids` 直接调用 `ref_matches`；`handle_table_placement` 调用 `remove_table_placement` 或 `PlacementPolicyCatalog::normalize_ref`，而 `normalize_ref` 调用 `by_name`。

RustCodeGraph 的文件节点显示该文件被 17 个文件引用，但精确 caller 查询在本次环境中超时；用 `rg` 对符号逐一核验后，核心 API 的显式调用方是上述三个独立 Rust 测试模块。`pkg/ddl/lib.rs` 是生产侧模块入口，但没有发现 Rust executor/job worker 到此目录模型的调用边。

Go 主链中，SQL/DDL executor 构造 job 后由 `onCreatePlacementPolicy`、`onAlterPlacementPolicy`、`onDropPlacementPolicy` 操作 Meta、更新 schema version 并维护 job 状态；修改还经 `updateExistPlacementPolicy` 重建表、分区和特殊 range 的 PD bundles。Rust 文件没有对应的 job、事务、InfoSchema cache、PD HTTP 或通知逻辑。

## 错误处理与边界

所有可失败入口用 `Result<_, PolicyError>`，`Display` 直接输出 `Debug` 形式，没有 Go 错误码、参数化用户消息或错误链。`create` 在任何目录写入前验证配置；`alter` 在写入前验证并检查状态；`drop_step` 在改变状态前检查引用，因此这些错误路径不会推进版本。

`create` 的新建分支没有显式拒绝“不同名称但重复 ID”：它会先插入新名称索引，再用相同 ID 覆盖 `policies` 中的旧策略，可能留下旧名称或新名称指向同一被覆盖 ID。正常 DDL 上游应保证 ID 唯一；若将此类型用于不可信输入，应在 `create` 增加原子冲突检查和相应独立回归测试。

`normalize_ref` 仅按名称解析，不检查目标状态是否为 `Public`；`check_not_in_use` 仅检查当前公开容器快照；对象遍历仅覆盖一级分区。`get_range_placement_policy_name` 对 `None`、无 `_rule_`、或分隔符位于首字符的输入返回空串；存在多个分隔符时使用最后一个。它不访问 PD，所以不会产生后端错误，当前 `RangeBackend` 变体也未使用。

`set_direct_placement_opt` 的枚举 match 已穷尽，因此当前不会返回 `InvalidOption`。约束字符串、副本组合等完整合法性不在本地解析；Go `checkPolicyValidation` 通过 `placement.NewBundleFromOptions` 覆盖更丰富的约束规则，不能把 Rust 的三项检查视为等价完整验证。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或外部资源。修改方法要求 `&mut self`，Rust 借用规则保证单次目录变更期间的独占访问；若跨线程共享，调用方必须自行包裹 `Mutex`/`RwLock` 等同步原语。

全部策略、引用和字符串由容器拥有；`by_name` 返回的 `&PolicyInfo` 生命周期绑定到目录借用。删除最终步骤移除拥有的 `PolicyInfo` 和名称索引，离开作用域即释放。不同于 Go DDL，Rust 状态机没有持久化检查点、owner failover、job 重试或 schema sync 等生命周期保证，进程退出会丢失整个目录状态。

## 与 Go 版本的对应关系

`PlacementPolicyCatalog::create` 对应 Go `onCreatePlacementPolicy` 的关键局部语义：忽略调用者状态、先校验、名称冲突、OR REPLACE 保留旧 ID、`None -> Public` 和版本推进。差异是 Go 写 Meta、设置/结束 DDL job 并返回具体 TiDB 错误；Rust 仅改内存。

`alter` 对应 `onAlterPlacementPolicy` 的“先取旧策略、再验证新 settings、替换配置、推进版本”。Go 的 `updateExistPlacementPolicy` 还查询依赖对象、重建 rule bundles 并通知 PD；Rust 的 `depended_object_ids` 只返回 ID，不被 `alter` 自动调用，也不更新任何外部规则。

`drop_step` 对应 `onDropPlacementPolicy` 的三阶段状态迁移和使用中拒绝。Go 根据 InfoSchema 版本选择 cache 或 Meta 检查，并额外访问 global/meta 特殊 range；Rust 只查调用方维护的三个内存容器，range 后端错误无法在这里出现。

`build_policy_info`/`set_direct_placement_opt` 对应 Go `buildPolicyInfo`/`SetDirectPlacementOpt` 的直接赋值职责；`get_range_placement_policy_name` 对应 Go `GetRangePlacementPolicyName` 中对第一条 rule ID 的字符串解析部分，但省略 `infosync.GetRuleBundle`。`remove_table_placement`、`handle_table_placement` 和 `normalize_ref` 对应 Go 同名/相邻辅助逻辑的表与分区子集；Go ignore 模式还追加 statement note，严格模式从事务 InfoSchema 查询。

Rust 独立测试验证了这些局部对齐：`placement_policy_test.rs` 覆盖创建/替换/修改/删除、错误优先级、直接选项赋值、引用归一化、ignore/default 和 rule ID；`placement_policy_ddl_test.rs` 对照 Go 的 InfoSchema/Meta 使用检查与依赖枚举；`placement_sql_test.rs` 覆盖 ignore 模式在简化 SQL 模型中的效果。Go 的 `placement_policy_test.go`、`placement_policy_ddl_test.go` 和 `placement_sql_test.go` 则覆盖真实 SQL、Meta、InfoSchema、PD bundle 与警告行为。

## 扩展指南

- 增加配置字段时，同步修改 `PlacementSettings`、`PlacementOptionType` 和 `set_direct_placement_opt`；若字段存在组合约束，扩展 `check_policy_validation`。测试放在独立的 `pkg/ddl/placement_policy_test.rs`，不要内嵌到生产文件，并对照 Go `model.PlacementSettings`、`SetDirectPlacementOpt` 与 `placement.NewBundleFromOptions`。
- 扩展生命周期时，优先修改 `PlacementPolicyCatalog::{create, alter, drop_step}`，保持双索引原子一致、错误路径不推进版本、OR REPLACE 保留 ID 等不变量。应补充重复 ID、非 Public alter、每步删除失败不变更等回归测试。
- 扩展引用范围时，统一更新 `PlacementObject` 的表达能力、`object_uses_policy`、`depended_object_ids`、`remove_table_placement`、`handle_table_placement` 与 `collect_policy_ids`，避免“阻止删除”和“依赖枚举”覆盖范围不一致。
- 若接入真实 DDL 主链，不能只调用当前内存目录：还需对齐 Go 的 job 持久化/取消、Meta 事务、InfoSchema cache 版本判断、全局 schema version/sync、PD bundle 更新失败传播及 owner 重试语义。对应测试至少要扩展独立 Rust DDL/SQL 测试，而不是用零测试或桩替代。
- 修正 schedule 注释或加入规范化前先决定兼容目标。当前 Go 与 Rust 测试都把直接 setter 视为机械赋值；若改为大写转换，会改变现有可观察行为，应先新增失败用例再修改实现。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件和 7,032 个 Rust 文件；`files --filter pkg/ddl/placement_policy.rs` 找到唯一目标；`node --file ... --offset 1 --limit 500` 读取完整 430 行并列出 50 个符号；`query PlacementPolicyCatalog`、`query handle_table_placement`、`query check_policy_validation` 定位核心符号。`callers handle_table_placement` 在 30 秒内无结果并被终止，因此调用者结论改由精确文本搜索核验。
- Rust 源与 crate 边界：`pkg/ddl/placement_policy.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`；DDL 总体契约参考 `pkg/ddl/doc.go` 和 `docs/agents/ddl/README.md`，并以源码为最终事实。
- Rust 独立测试：`pkg/ddl/placement_policy_test.rs`、`pkg/ddl/placement_policy_ddl_test.rs`、`pkg/ddl/placement_sql_test.rs`；其中生产逻辑的集中断言位于前两者的实际 `#[test]` 区段以及 SQL 简化模型用例。
- Go 对照：`pkg/ddl/placement_policy.go`；相关真实行为测试为 `pkg/ddl/placement_policy_test.go`、`pkg/ddl/placement_policy_ddl_test.go`、`pkg/ddl/placement_sql_test.go`。它们证明完整 Go 路径还包含 DDL job、Meta/InfoSchema、PD bundles、SQL 错误码与 statement note。
- 人工复核结论：该文件存在于 Rust 迁移中，用确定性的内存结构保存并验证放置策略核心局部语义；其运行方式是显式构造目录并调用同步方法；安全扩展必须同时维护双索引、版本、状态机、引用扫描和独立测试，并不得宣称当前已经替代完整 Go DDL 链。
