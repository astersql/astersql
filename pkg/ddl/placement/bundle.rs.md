# `pkg/ddl/placement/bundle.rs`

## 文件定位

`bundle.rs` 是 `astersql-ddl-placement` crate 中把元数据层的 `model::PlacementSettings` 转换成 PD placement rule group 的核心实现。crate 入口 `pkg/ddl/placement/lib.rs` 以私有模块 `mod bundle` 装载本文件，再通过 `pub use bundle::*` 暴露其 API；`pkg/ddl/placement/Cargo.toml` 则声明该 crate 直接依赖 `astersql-store-pdtypes`、`astersql-meta-model` 和 `astersql-tablecodec`，分别提供 PD 规则类型、表/分区/策略模型和 key 编码能力。

在完整 DDL 链路中，本文件负责“计算规则”，而不负责提交 DDL job 或直接调用 PD。当前可确认的 Rust 生产入口有两处：`pkg/ddl/persistent_create_table.rs::create_table` 在事务中以 `Policies` 实现 `PolicyGetter`，调用 `NewFullTableBundles` 后通过 `JobExecutionContext::put_create_table_bundles` 写入 PD；`pkg/ddl/persistent_masking_actions.rs` 的表 ID 重建路径采用同样流程。因而它属于 job 驱动 DDL 的一个局部计算/接线组件：没有自己的 schema state machine、reorg/backfill、checkpoint、回滚状态或系统表写入，错误由上层 DDL job 路径决定是否取消。

## 核心职责

- 用 `NewBundleFromOptions` 将两套互斥输入语法转为 `Bundle`：显式的 leader/follower/learner 约束语法由 `NewBundleFromConstraintsOptions` 处理，`PRIMARY_REGION`/`REGIONS`/`SCHEDULE` 糖语法由 `NewBundleFromSugarOptions` 处理（`bundle.rs:68-345`）。
- 用 `Bundle::Tidy` 删除无效规则、按约束指纹和角色合并规则，并在不丢失 leader 优先级时把可转换角色规约成 voter（`bundle.rs:353-377,496-580`）。
- 用 `Bundle::Reset` 或 `Bundle::RebuildForRange` 把抽象规则绑定到表、分区或系统 key range，补齐 group ID、rule ID、优先级以及经过 codec 编码的十六进制起止键（`bundle.rs:379-450,582-598`）。
- 提供 bundle 身份、复制、判空、JSON 表示和 leader DC 查询等辅助行为（`Bundle::{String,Clone,IsEmpty,ObjectID,GetLeaderDC}`）。
- 通过 `PolicyGetter` 隔离策略元数据来源，并用 `NewTableBundle`、`NewPartitionBundle`、`NewPartitionListBundles`、`NewFullTableBundles` 组合表级继承规则和分区独立规则（`bundle.rs:611-697`）。

## 主要符号

- `pub struct Bundle { ID, Index, Override, Rules }`：与 PD rule group JSON 对齐；serde 字段名是 `group_id`、`group_index`、`group_override` 和 `rules`。`ID` 为空时序列化会省略，其余字段保留（`bundle.rs:42-53`）。
- `NewBundle(id)`：仅用 `GroupID(id)` 初始化组 ID。它不验证 ID 正负；有效对象 ID 的检查延后到 `Bundle::ObjectID`（`bundle.rs:55-61,462-478`）。
- `NewBundleFromConstraintsOptions`：解析通用、leader、follower、learner 约束；支持通用约束的 YAML 数组和字典映射两种形态，并把 survival preferences 写入所有规则的 `LocationLabels`（`bundle.rs:68-199`）。
- `NewBundleFromSugarOptions`：解析 primary region、region 集合、followers 和调度方式，生成 leader/voter 规则；默认 followers 为 2（总 voter 数 3），支持 `even` 与 `majority_in_primary`（`bundle.rs:201-298`）。
- `newBundleFromOptions` / `NewBundleFromOptions`：前者限制 followers 不超过 8 并选择语法分支；后者是公开规范化入口，会额外执行 `Tidy`（`bundle.rs:300-345`）。
- `Bundle::Tidy`、`ConstraintsGroup::{MergeRulesByRole,MergeTransformableRoles}`、`transformableLeaderConstraint`：规则规约实现。`ConstraintsGroup` 是文件私有状态，不越过调用边界（`bundle.rs:353-377,496-580`）。
- `Bundle::{Reset,RebuildForRange}` 与 `GetRangeStartAndEndKeyHex`：将规则绑定到物理 key range；`encode_table_prefix` 是文件私有编码辅助函数（`bundle.rs:379-450,582-598`）。
- `pub trait PolicyGetter`：唯一方法 `GetPolicy(policy_id) -> Result<model::PolicyInfo, Error>`，使规则构建不依赖具体事务/infoschema 实现（`bundle.rs:611-614`）。
- `NewTableBundle` / `NewPartitionBundle` / `NewPartitionListBundles` / `NewFullTableBundles`：面向表与分区元数据的组合 API；`newBundleFromPolicy` 是它们共用的私有策略解析桥（`bundle.rs:616-697`）。

本文件没有条件编译项；测试模块的 `#[cfg(test)]` 声明位于 `pkg/ddl/placement/lib.rs`，不在生产文件内。

## 执行流程

1. 上层把表/分区元数据和 `PolicyGetter` 传给 `NewFullTableBundles`。它先尝试构建表级 bundle，再遍历分区构建其独立 bundle；没有 policy reference 的对象直接跳过（`bundle.rs:658-697`）。
2. `newBundleFromPolicy` 调用 `PolicyGetter::GetPolicy`，把取得的 `PolicyInfo.PlacementSettings` 交给 `NewBundleFromOptions`。当前建表实现 `persistent_create_table.rs::Policies::GetPolicy` 从 DDL 元数据事务读取策略，缺失策略会返回 `[schema:8249]` 错误。
3. `newBundleFromOptions` 先拒绝 `Followers > 8`，再依据是否出现显式约束/learner 字段选择糖语法或约束语法；两套语法禁止混用（`bundle.rs:300-325`）。
4. 约束语法先尝试把 `Constraints` 当 YAML 数组解析，失败时改按字典约束生成 voter 规则；随后合入 leader/follower/learner 约束并检测冲突。糖语法则排序 regions、确认 primary region 在其中，再计算主 region 副本数和剩余 voter（`bundle.rs:68-298`）。
5. 公开入口 `NewBundleFromOptions` 调用 `Tidy`：过滤 `Count <= 0`，按当前顺序重新编号，以 `ConstraintsFingerPrint` 分组，同角色累加 count；只有一个组能竞选 leader 时，才把该 leader 组中的 leader/follower/voter 合为 voter，learner 始终独立（`bundle.rs:335-377,504-580`）。多个约束组显式包含 leader 会报错。
6. `NewTableBundle` 收集表 ID 以及全部分区 ID，用 `Reset(RuleIndexTable, ids)` 让表策略覆盖表及其分区；`NewPartitionBundle` 只为分区自己的策略调用 `Reset(RuleIndexPartition, &[partition_id])`。分区独立 bundle 与表级继承 bundle 都会出现在完整列表中，由 PD 的 rule index/group 语义决定覆盖关系（`bundle.rs:616-684`）。
7. `Reset` 优先选已有的表级规则作为模板，按每个物理 ID 深拷贝规则，生成互不重复的 `table_rule_*`/`partition_rule_*` ID，并以 `[GenTablePrefix(id), GenTablePrefix(id+1))` 作为 key range（`bundle.rs:404-450`）。上层随后把结果交给 PD；该 I/O 不在本文件内。

## 数据与状态

`Bundle` 是拥有所有数据的可克隆值类型，`Rules` 也是拥有所有权的 `Vec<pd::Rule>`。构建阶段多返回 `Result<Option<Bundle>, Error>`：`None` 表示元数据没有引用 placement policy，不等于错误，也不等于含零规则的 `Some(Bundle)`。`NewFullTableBundles` 把这些可选值压缩成 `Vec<Bundle>`，因此空向量表示表和分区都没有独立策略。

`Tidy` 是破坏式但就地的规范化操作：它通过 `drain` 消耗原规则、重写 rule ID 和 count，并最终按字符串 ID 排序。它依赖 `ConstraintsFingerPrint` 将相同 label constraints 放入同组；不同约束组不会合并。测试 `bundle_test.rs::tidy_matches_go_merge_matrix_and_is_stable` 还验证重复调用稳定、零 count 被删除、同角色 count 相加。

`Reset` 同样就地改写 bundle：首个 `new_ids` 元素既决定 bundle group ID，又被视为表级/当前对象规则；后续 ID 总被标为 `RuleIndexPartition`。当输入规则中已混有表级与分区级副本时，只保留 `RuleIndexTable` 模板，避免再次 Reset 时指数复制旧分区规则。`Reset` 对空 `new_ids` 使用 `assert!`，这是调用方必须维护的不变量，而非可恢复错误。

`RebuildForRange` 目前只为 global/meta 两个已知名称设置特殊 group ID 和 index；`GetRangeStartAndEndKeyHex` 只有 meta group 返回非空边界，global 和未知 group 返回空字符串。它仍会对所有规则设置 `Override=true`、group ID、顺序 index 和由小写 policy name 生成的 rule ID（`bundle.rs:379-401,588-598`）。

## 依赖与调用关系

内部主要调用链为：

`NewFullTableBundles` → `NewTableBundle` / `NewPartitionListBundles` → `NewPartitionBundle` → `newBundleFromPolicy` → `PolicyGetter::GetPolicy` → `NewBundleFromOptions` → `newBundleFromOptions` → `NewBundleFromSugarOptions` 或 `NewBundleFromConstraintsOptions` → `Bundle::Tidy`。

规则构建向下依赖同 crate 的 `rule.rs::{NewRule,NewRuleBuilder}`、`constraint.rs::NewConstraintDirect`、`constraints.rs::{NewConstraintsFromYaml,AddConstraint,ConstraintsFingerPrint}` 和 `common.rs` 中的 group/rule index/key-range 常量。物理范围编码依赖 `tablecodec::GenTablePrefix` 与 `codec::EncodeBytes`，序列化依赖 `serde_json`，survival preferences 解析依赖 `serde_yaml`。

RustCodeGraph 对 `bundle.rs` 的文件索引和内部边可用，并确认 `NewBundleFromOptions` 调用 `newBundleFromOptions`、后者分派到两套构建函数，`NewFullTableBundles` 调用表/分区构建函数。不过本次索引的精确 `callers` 查询没有返回两个跨 crate Rust 调用点；使用仓库文本检索补证后，确认生产调用位于 `pkg/ddl/persistent_create_table.rs:152` 和 `pkg/ddl/persistent_masking_actions.rs:662`。这两处都在 DDL 事务读取策略、在事务外调用 `put_create_table_bundles`。Go 版还被 `pkg/ddl/table.go`、`pkg/ddl/partition.go`、`pkg/ddl/create_table.go` 和 GC worker 等更多路径调用；不能据此推断 Rust 已接齐同等上游。

## 错误处理与边界

- `None` options、两套语法混用、followers 超过 8、primary region 不在 regions、未知 schedule、多个 leader constraint group 等均返回 placement `Error`，不修改外部状态。
- 显式约束构建会为 leader/follower/learner 的解析或冲突添加上下文；底层错误来自 `NewConstraintsFromYaml`、`NewRuleBuilder` 或 `AddConstraint`。字典约束只在通用 `Constraints` 的 YAML 数组解析失败后尝试。
- `newLocationLabelsFromSurvivalPreferences` 对空串返回空列表，非空值必须能反序列化为字符串数组，否则返回 `ErrInvalidSurvivalPreferenceFormat`（`bundle.rs:327-333`）。
- `Bundle::ObjectID` 要求 ID 以 `BundleIDPrefix` 开头、后缀是正整数；格式、解析和非正数分别映射到相应错误（`bundle.rs:462-478`）。注意 `NewBundle` 自身允许负数，测试也固定了这一 Go 兼容行为。
- `Bundle::String` 把 JSON 序列化错误压缩为空字符串，不暴露错误对象；Rust 版没有 Go 版 `MockMarshalFailure` failpoint，但 `bundle_test.rs::string_and_new_bundle_match_go_cases` 验证正常 JSON 形态。
- `GetLeaderDC` 只接受 role=leader、count=1、且存在指定 key 的单值 `In` 约束的规则。实现先用任意约束判断规则有效，随后读取该规则的第一条约束值；这与 Go 当前实现的索引行为对齐，但调用者若构造“目标 DC 约束不是第一条”的 leader rule，返回值可能不是目标 key 对应值。扩展或修复时必须先在 Go/Rust 两侧补同构回归测试。
- `Reset` 的空 ID 切片会 panic；所有公开组合构建函数当前都传入至少一个表或分区 ID。新调用者必须在调用前保证非空。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、channel、网络连接或事务句柄。所有构建函数只读取借用的配置/元数据并返回拥有所有权的值；`&mut self` 方法由调用者独占修改 `Bundle`，并不提供共享可变状态。

事务与 PD I/O 生命周期属于上层。`persistent_create_table.rs::create_table` 在短事务内构造 `TransactionMutator` 和 bundle，事务闭包结束后才调用 `put_create_table_bundles`；表 ID 重建路径也遵循这一形态。本文件只接收同步 `PolicyGetter`，不会缓存 getter 或让借用逸出。错误逐层用 `?` 返回，上层负责将错误映射为 DDL job 取消/失败；本文件没有清理钩子或补偿动作。

从 DDL 协议看，这些规则计算处于 job 执行的局部步骤，但它本身不改变 schema version、不等待 follower schema sync、不产生 reorg checkpoint，也不定义 rollback。修改本文件不能假定规则写入 PD 与元数据事务天然原子；相关失败语义必须在调用它的 DDL job/context 测试中验证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/placement/bundle.go`。Rust 文件保留了 Go 的主要 API 名称、字段名和流程：两套 options 构建、followers 上限、survival preferences、`Tidy` 合并矩阵、range 重建、`Reset` 规则命名与 key 编码、对象 ID 解析、leader DC 查询以及表/分区 bundle 组合。`pkg/ddl/placement/Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向 `pkg/ddl/placement`。

已确认的表示差异包括：Go `Bundle` 包装 `pd.GroupBundle` 且规则使用指针，Rust `Bundle` 直接拥有 `Vec<pd::Rule>`；Go `PolicyGetter` 返回 `*PolicyInfo`，Rust 返回拥有所有权的 `PolicyInfo`；Go 的 policy helper 接收完整 `PolicyRefInfo`，Rust 只把可选 policy ID 传入私有 helper。这些差异没有改变当前组合语义。

需谨慎对待的行为差异是 `String`：Go 可借助 failpoint 强制造成 marshal 失败，Rust 仅通过 `unwrap_or_default` 保留“失败返回空串”的接口语义，当前测试未能注入序列化失败。另一个验证边界是接线覆盖：Go 的 RustCodeGraph 调用图显示 placement bundle 被建表、表/分区变更和 GC 等多处使用；Rust 仓库检索目前只确认建表和表 ID 重建路径调用 `NewFullTableBundles`。这说明文件级逻辑已经较完整，但不能宣称所有 Go 上游都已迁移。

测试对应关系也很明确：`bundle_test.rs` 后半段包含可执行 Rust 回归，覆盖 Go 的 options 表、Reset、Tidy、range、身份和 JSON；文件前半还保留大量注释化的 Go 迁移参考，不应把注释用例计作可执行覆盖。`bundle_1_aster_unit_test.rs` 提供较小的端到端行为组；`meta_bundle_test.rs` 可执行测试覆盖 `PolicyGetter` 与表/分区组合。

## 扩展指南

- 新增 placement option 或调度算法时，优先修改 `newBundleFromOptions` 的语法选择规则以及对应的 `NewBundleFrom*Options`；同时扩展 `pkg/ddl/placement/bundle_test.rs::new_bundle_from_options_matches_go_table`，并与 `bundle.go` 同名逻辑/测试逐项核对。不能为了 Rust 测试通过而简化 Go 的校验或默认值。
- 新增可合并角色或改变 leader 优先级规则时，修改 `ConstraintsGroup` 与 `transformableLeaderConstraint`，同步扩展 `tidy_matches_go_merge_matrix_and_is_stable`。必须验证不同约束组、多个 leader group、learner 不可转换、零 count 和重复调用稳定性。
- 新增 key range 时，集中扩展 `RebuildForRange`、`GetRangeStartAndEndKeyHex` 以及 `common.rs` 常量，并更新 `range_keys_and_rebuild_match_go_cases`。编码必须继续使用 `codec::EncodeBytes(GenTablePrefix(...))`，不要手工拼接裸 key。
- 改变表/分区继承语义时，修改 `NewTableBundle`、`NewPartitionBundle`、`NewFullTableBundles` 或 `Reset`，并同步 `meta_bundle_test.rs` 的四组可执行测试。还应检查 `persistent_create_table.rs` 与 `persistent_masking_actions.rs` 的 DDL 失败映射和 PD 写入时序。
- 若修正 `GetLeaderDC` 的“验证任意约束但读取第一条约束”限制，应同时修改 Go/Rust，并增加目标 DC 约束不在首位的独立测试；单边更改会破坏版本对齐。
- Rust 单元测试继续放在同目录独立测试文件，由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入，不要嵌入 `bundle.rs`。任何生产 Rust 修改完成后按仓库规则先执行 `cargo fmt --all`；本次仅新增文档，没有运行 Cargo。
- 兼容风险主要是 PD JSON 字段、group/rule ID、rule index 和 key range；正确性风险集中在副本计数、约束冲突和继承/覆盖；性能风险主要来自规则数约为“模板规则数 × 物理 ID 数”，以及构建过程中多次克隆规则和 label。扩展时应避免无界增加规则组合。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter pkg/ddl/placement` 找到目标、Go 对照和独立测试；`node --file pkg/ddl/placement/bundle.rs` 分段读取全部 697 行；`explore` 核对内部调用链和 Go 上游；对 `NewFullTableBundles`、`NewBundleFromOptions` 执行了 `query`、`callers`、`callees`。精确 Rust 跨 crate callers 未返回结果，因此又用 `rg` 定位实际生产调用，没有把图的缺边解释成“无调用”。
- 生产源码：`pkg/ddl/placement/bundle.rs`、`pkg/ddl/placement/lib.rs`、`pkg/ddl/persistent_create_table.rs`、`pkg/ddl/persistent_masking_actions.rs`；DDL 包契约读取自 `pkg/ddl/doc.go` 和 `docs/agents/ddl/README.md`。
- crate/依赖：`pkg/ddl/placement/Cargo.toml`、`pkg/ddl/Cargo.toml`，以及引用 `astersql-ddl-placement` 的 workspace manifests。
- Go 对照：`pkg/ddl/placement/bundle.go` 全部 705 行；RustCodeGraph 还显示其 Go 上游包括 `pkg/ddl/create_table.go`、`pkg/ddl/table.go`、`pkg/ddl/partition.go` 等。
- 测试证据：`pkg/ddl/placement/bundle_test.rs`（可执行测试自约 1600 行开始）、`pkg/ddl/placement/bundle_1_aster_unit_test.rs`、`pkg/ddl/placement/meta_bundle_test.rs`，并对照 `bundle_test.go`、`meta_bundle_test.go` 的测试符号。按任务约束，本次没有运行 Cargo，因此这里记录的是测试源码所表达的边界，而不是新的测试执行结果。
- 文档交付只要求结构验证：确认目标文件存在且恰有“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”十一个固定二级标题；该命令的实际退出码在任务完成时记录。
