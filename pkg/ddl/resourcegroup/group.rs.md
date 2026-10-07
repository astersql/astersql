# `pkg/ddl/resourcegroup/group.rs`

源文件：[`group.rs`](group.rs)

## 文件定位

本文件属于 `astersql-ddl-resourcegroup` crate，是资源组 DDL 模型到 Resource Manager protobuf 的纯转换与校验层。crate 入口 `pkg/ddl/resourcegroup/lib.rs` 将 `group` 模块公开再导出，并将 `astersql-meta-model` 的 `group_3` 模型与 AST 枚举公开为 `model`、`ast`；`build.rs` 生成本文件使用的 `rmpb` protobuf 类型。`pkg/ddl/resourcegroup/Cargo.toml` 表明其直接运行时依赖仅有固定版本 `protobuf = 2.8.0`、`astersql-meta-model` 和 `thiserror`。

当前 Rust 接线应与 Go 主链区分：RustCodeGraph 和全仓 `rg` 找到的 Rust 调用者均在 `pkg/ddl/resourcegroup/migration_aster_unit_test.rs` 与 `pkg/resourcegroup/tests/resource_group_test.rs`，没有发现 Rust 生产调用点。因此它是已经移植且有测试覆盖的转换库，但不能据此宣称 Rust DDL 创建/修改资源组主链已经调用它。Go 对照实现则由 `pkg/ddl/resource_group.go` 的 `onCreateResourceGroup`、`onAlterResourceGroup` 和 `checkResourceGroupValidation` 调用，处于 DDL job 的参数校验和提交 Resource Manager 之前。

## 核心职责

- `NewGroupFromOptions` 校验资源组名称和配置组合，把 `model::ResourceGroupSettings` 转为 `rmpb::ResourceGroup`。
- 它复制名称、优先级、Runaway 阈值/动作/观察规则、Background 任务设置，并为 RU 模式构造嵌套的 token bucket。
- 它拒绝空设置、超长名称、空 Runaway 规则、无效 Runaway 动作、缺少切换目标组名、RU/Raw 同时配置，以及当前不支持的非 RU 模式。
- `runaway_action` 与 `runaway_watch_type` 将 AST 枚举显式映射为 protobuf 枚举，避免依赖不同生成类型之间的数值强转。
- 本文件只做内存转换和同步校验，不写 DDL 元数据、不调用 PD/Resource Manager、不推进 schema state，也不检查 SwitchGroup 指向的资源组是否真实存在；后一点与 Go 文件中的 TODO 一致。

## 主要符号

- `pub const MAX_GROUP_NAME_LENGTH: usize = 32`：资源组名称的最大字节数。检查使用 Rust `String::len()`，与 Go `len(string)` 一样按 UTF-8 字节计数，而不是按字符数计数。
- `pub const MaxGroupNameLength`：为移植期保留的 Go 风格公开别名，值与 `MAX_GROUP_NAME_LENGTH` 完全相同。
- `pub fn NewGroupFromOptions(groupName: String, options: Option<&model::ResourceGroupSettings>) -> Result<rmpb::ResourceGroup, Error>`：唯一公开行为入口。它取得名称所有权、只借用设置，成功时返回拥有所有 protobuf 子对象的资源组，失败时返回 `crate::errors::ResourceGroupError` 的别名 `Error`。
- `fn runaway_action(ast::RunawayActionType) -> rmpb::RunawayAction`：私有动作映射，覆盖 None、DryRun、Cooldown、Kill、SwitchGroup；未知枚举回落到 `NoneAction`。入口函数会先拒绝显式 `RunawayActionNone`，但其他未来未知值目前仍会被此函数降级为 `NoneAction`。
- `fn runaway_watch_type(ast::RunawayWatchType) -> rmpb::RunawayWatchType`：私有观察类型映射，覆盖 None、Exact、Similar、Plan；未知值回落到 `NoneWatch`。只有 `WatchType != WatchNone` 时入口才创建 protobuf `RunawayWatch`。

## 执行流程

1. `NewGroupFromOptions` 先将 `None` 转为 `ErrInvalidGroupSettings`，再以 `groupName.len()` 检查 32 字节上限；因此名称错误优先于后续模式错误。
2. 创建空 `rmpb::ResourceGroup`，设置名称与 `options.Priority as u32`。模型中的优先级是 `u64`，这里沿用 Go 的 `uint32` 转换语义；函数本身不做范围校验。
3. 若存在 `options.Runaway`，先要求 `ExecElapsedTimeMs`、`ProcessedKeys`、`RequestUnit` 至少一个非零，再构造 `RunawayRule`。随后拒绝 `RunawayActionNone`；若动作是 `RunawayActionSwitchGroup`，还要求 `SwitchGroupName` 非空。动作和可选 watch 经私有映射函数转换，最后整体挂到资源组。
4. 若存在 `options.Background`，复制 `JobTypes` 到新的 `protobuf::RepeatedField`，并写入 `ResourceUtilLimit`。这里不校验任务类型字符串或利用率范围；这些约束若需要，应由更上游解析/校验层负责。
5. 仅当 `RURate > 0` 时选择 `GroupMode::RuMode`，依次构造 `TokenLimitSettings { fill_rate, burst_limit }`、`TokenBucket` 和 `GroupRequestUnitSettings`，形成 protobuf 的 RU 嵌套结构。
6. RU 结构构造后检查 `CPULimiter`、`IOReadBandwidth`、`IOWriteBandwidth`：任一非空即返回 `ErrInvalidResourceGroupDuplicatedMode`；全部为空则返回完成的资源组。
7. `RURate == 0` 时无论 Raw 字段是否存在都返回 `ErrUnknownResourceGroupMode`，因为当前实现只支持 RU 模式。Runaway/Background 校验发生在模式判定之前，所以无效的前置子配置可能先于未知模式报错。

## 数据与状态

输入模型定义在 `pkg/meta/model/resource_group.rs`：`ResourceGroupSettings` 保存 `RURate`、`Priority`、三项 Raw 限流字符串、`BurstLimit`，以及用 `Option<Arc<_>>` 表示的 Runaway 和 Background 子设置。本函数只读这些输入；字符串和列表被克隆到 protobuf，输入中的共享 `Arc` 不会进入输出。

输出 `rmpb::ResourceGroup` 的关键形状为：顶层 `name`、`priority`、`mode`；可选 `runaway_settings` 和 `background_settings`；RU 模式下还有 `r_u_settings.r_u.settings.fill_rate/burst_limit`。`BurstLimit` 原值（包括测试覆盖的 `0`、`-1`、`-2`）被直接保留，本文件不调用模型的 `GetBurstLimitAdjusted`。

函数没有全局可变状态或缓存。局部 protobuf 对象只有在完整组装后才随 `Ok(group)` 移交给调用者；任一错误都会丢弃已构造的局部对象。`MaxGroupNameLength` 只是兼容别名，不形成独立状态。

## 依赖与调用关系

- 上游类型依赖：`crate::{ast, model, rmpb}` 由 `lib.rs` 聚合；`model`/`ast` 来自 `astersql-meta-model`，`rmpb` 由 protobuf 构建脚本生成。
- 下游函数调用：RustCodeGraph 给出 `NewGroupFromOptions -> runaway_action` 与 `NewGroupFromOptions -> runaway_watch_type` 两条本文件调用边；其余工作是 protobuf 生成类型的 `new`/`set_*` 方法以及 `protobuf::RepeatedField::from_vec`。
- 错误依赖：所有返回值来自 `pkg/ddl/resourcegroup/errors.rs` 中的 `ResourceGroupError` 变体和 Go 风格常量别名。
- 已验证的 Rust 调用者：`pkg/ddl/resourcegroup/migration_aster_unit_test.rs` 直接覆盖转换、校验顺序和错误文本；`pkg/resourcegroup/tests/resource_group_test.rs` 覆盖 Go 对齐的 RU、burst、Runaway 和非法配置用例。RustCodeGraph 未找到生产调用者。
- Go 应用主链：`pkg/ddl/resource_group.go` 在 `onCreateResourceGroup` 与 `onAlterResourceGroup` 中先调用 Go 同名函数得到 protobuf，再分别调用 `infosync.AddResourceGroup`/`infosync.ModifyResourceGroup`；`checkResourceGroupValidation` 复用同一转换函数做校验。该关系是 Rust 未来接线的语义参照，不是当前 Rust 调用事实。
- workspace 边界：`pkg/ddl/Cargo.toml` 声明了 `astersql-ddl-resourcegroup` 路径依赖，测试 crate `pkg/resourcegroup/tests/Cargo.toml` 则将它列为 dev-dependency；依赖声明本身不等于生产代码已调用该 API。

## 错误处理与边界

错误按源码顺序短路：空 `options` → 名称过长 → Runaway 空规则 → Runaway 无动作 → SwitchGroup 空目标 → RU/Raw 重复模式 → 未知模式。`pkg/ddl/resourcegroup/migration_aster_unit_test.rs::runaway_validation_matches_go_order` 明确验证 Runaway 内部顺序，`validation_errors_match_go_behavior` 与外部测试文件验证其余主要分支。

名称限制按字节判断，因此多字节 UTF-8 名称可能在少于 32 个字符时超过限制。空名称在本函数内不报错。阈值规则仅判断三个值是否同时为零：`ProcessedKeys` 或 `RequestUnit` 为负数也会被视为“非空”；Background 字段和 `Priority as u32` 同样没有额外范围检查。文档只记录当前事实，不把这些行为解释成完整的业务合法性保证。

`runaway_action`/`runaway_watch_type` 对未知枚举采用 None 回退，而公开入口仅显式拦截已知的 `RunawayActionNone`。扩展 AST 枚举而未同步映射可能静默生成 None protobuf 值，这是兼容性风险。SwitchGroup 只要求非空，未验证目标存在；Go 对照源也明确保留此 TODO。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务、网络连接或持久化句柄。`NewGroupFromOptions` 是同步纯计算：借用 `ResourceGroupSettings` 仅持续到返回，输出拥有克隆后的字符串、任务列表与 protobuf 子消息，可独立于输入生命周期使用。

在 Go DDL 参照链中，转换结果随后可能跨越 DDL job 的元数据更新和 infosync RPC，但这些超时、取消、重试、owner failover 与 schema version 生命周期均由 `pkg/ddl/resource_group.go` 及 DDL 框架管理，不属于本 Rust 文件。当前没有 Rust 生产调用证据，因此也不能为其宣称任何 DDL 事务或故障恢复保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/resourcegroup/group.go::NewGroupFromOptions`。Rust 版本保持了 Go 的检查顺序、32 字节名称上限、优先级窄化、Runaway/Background 字段填充、RU token bucket 形状、RU/Raw 冲突以及仅支持 RU 模式的结论。`pkg/resourcegroup/tests/resource_group_test.go::TestNewResourceGroupFromOptions` 是主要 Go 表驱动证据。

实现差异主要来自语言和生成 API：Go 使用指针与结构体字面量，Rust 使用 `Option<&...>`、拥有值和逐层 `set_*`；Go 通过枚举数值转换 Runaway/Watch，Rust 用两个显式映射函数；Go 的 `[]string` 在 Rust 输出中通过 `RepeatedField::from_vec` 构造。Rust 的 `Option<Arc<_>>` 输入模型用于对齐 Go 指针子配置的共享语义，但转换结果会克隆字段。

Go 的生产调用点已存在于 DDL job 处理器；Rust 目前只有测试调用证据。外部 Rust 测试中同时存在 TestKit 的资源组 SQL 生命周期用例，但这些 SQL 用例并不直接调用 `NewGroupFromOptions`，不能单独证明 SQL 路径已接入本 crate。

## 扩展指南

- 新增资源组模式时，修改入口末段的模式选择和互斥规则，并同步 protobuf 构造；不要仅让新模式绕过 `ErrUnknownResourceGroupMode`。应在 `pkg/ddl/resourcegroup/migration_aster_unit_test.rs` 和 `pkg/resourcegroup/tests/resource_group_test.rs` 的独立测试中加入成功形状、模式冲突与错误优先级用例，同时核对 Go 同路径实现。
- 新增 Runaway action/watch 枚举时，必须同步 `runaway_action` 或 `runaway_watch_type`，并为每个新值断言 protobuf 结果，避免落入 None 回退。若需要严格拒绝未知值，应改变映射函数返回 `Result`，并明确 Go 兼容策略。
- 增加 SwitchGroup 存在性、Background job type、优先级或数值范围校验时，应先确认这些规则属于本纯转换层还是上游 SQL/DDL 校验层；保持与 Go 的错误类型、错误顺序和生产调用时点一致。
- 若把本 crate 接入 Rust DDL 生产链，需要在真实调用点补充针对 create/alter/validation 的独立集成测试，验证转换错误会取消或拒绝作业、protobuf 会被提交到对应资源管理接口，而不是用现有直接单元测试替代接线证据。
- Rust 测试不得内嵌回 `group.rs`；沿用同目录 `migration_aster_unit_test.rs` 或独立测试 crate。改动生产 Rust 后按仓库协议执行 `cargo fmt --all`，但本次纯文档任务未修改 Rust 且按计划不运行 Cargo。

## 验证依据

- 源码与边界：`pkg/ddl/resourcegroup/group.rs`、`lib.rs`、`errors.rs`、`Cargo.toml`，以及模型定义 `pkg/meta/model/resource_group.rs`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ddl/resourcegroup` 定位 7 个相关文件；`explore`/`node NewGroupFromOptions` 确认公开函数、两个私有映射调用边及测试调用者。全仓 `rg` 进一步确认未发现 Rust 生产调用点。
- Go 对照：`pkg/ddl/resourcegroup/group.go`、`pkg/ddl/resource_group.go` 和 `pkg/resourcegroup/tests/resource_group_test.go::TestNewResourceGroupFromOptions`。
- Rust 测试：`pkg/ddl/resourcegroup/migration_aster_unit_test.rs`（转换形状、错误顺序、错误文本）与 `pkg/resourcegroup/tests/resource_group_test.rs`（RU/burst、Runaway 和非法配置）；本任务按计划只读取测试，不运行 Cargo。
- 结构验收使用任务文件给定命令，要求本文存在且恰有 11 个固定二级标题；最终交付前另检查变更范围只包含本文，并人工复核无 Rust/Go/Cargo/`plan.md` 修改。
