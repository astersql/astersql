# `pkg/ddl/resourcegroup/errors.rs`

## 文件定位

`errors.rs` 是 `astersql-ddl-resourcegroup` crate 的资源组转换错误定义层。crate 根模块 `pkg/ddl/resourcegroup/lib.rs` 以 `pub mod errors; pub use errors::*;` 将本文件公开到 crate 根；相邻的 `group.rs` 在将 `meta_model::ResourceGroupSettings` 转为 Resource Manager protobuf 时返回这些错误。`pkg/ddl/resourcegroup/Cargo.toml` 声明该 crate 直接依赖 `thiserror = "2"`，本文件正是该依赖的使用点。

这不是 DDL job 状态机或持久化层，而是输入校验/转换的值类型错误边界。当前 Rust 工作区中，主要实际消费者是 `pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions` 以及独立测试；虽然 `pkg/ddl/Cargo.toml` 在 Windows 目标依赖中列出该 crate，精确搜索未找到 DDL Rust 主链对这些符号的生产调用，因此不应将 Go DDL 接线当作已完成的 Rust 接线。

## 核心职责

- 用 `ResourceGroupError` 统一表示资源组设置校验中的 9 类可区分失败。
- 通过每个变体上的 `#[error("...")]` 产生稳定的 `Display` 文案并实现标准 `std::error::Error`。
- 提供 `Error` 类型别名和 9 个 Go 风格的 `Err...` 公开常量，让机械移植的调用点能保留 Go 符号名与错误分类。
- 保持与 `pkg/ddl/resourcegroup/errors.go` 中哨兵错误相同的错误文案，并允许 Rust 调用方用 `Eq`/`PartialEq` 直接比较错误类别。

## 主要符号

- `pub enum ResourceGroupError`：无载荷、可 `Clone + Copy + Debug + Eq + PartialEq` 的公开错误枚举；`thiserror::Error` 派生宏根据属性生成错误实现。
- `InvalidGroupSettings`：未传入设置；`NewGroupFromOptions` 在 `options == None` 时首先返回它。
- `TooLongResourceGroupName`：名称的 UTF-8 字节长度超过 `MAX_GROUP_NAME_LENGTH`（32）。
- `InvalidResourceGroupFormat`：为 Go 的“设置格式非法”分类保留；当前 Rust 生产源码没有返回该变体。
- `InvalidResourceGroupDuplicatedMode`：同时设置 RU 速率与 Raw CPU/IO 选项。
- `UnknownResourceGroupMode`：未选中当前唯一支持的 RU 模式。
- `DroppingInternalResourceGroup`：删除保留资源组；当前 Rust 生产源码未接线，Go 的 `pkg/ddl/executor.go::DropResourceGroup` 会返回对应哨兵错误。
- `ResourceGroupRunawayRuleIsEmpty`：Runaway 规则的执行时间、处理 key 数与 RU 阈值全为零。
- `UnknownResourceGroupRunawayAction`：Runaway 动作为 `RunawayActionNone`。
- `UnknownResourceGroupRunawaySwitchGroupName`：动作为 `SwitchGroup` 但目标组名为空。
- `pub type Error = ResourceGroupError`：与 Go 侧错误命名风格对齐，是 `NewGroupFromOptions` 的 `Result` 错误类型。
- `ErrInvalidGroupSettings` 等 9 个 `pub const`：各自是对应枚举变体的零大小常量值；它们不是动态分配的错误对象。

## 执行流程

1. 调用方调用 `group.rs::NewGroupFromOptions(groupName, options)`，返回类型为 `Result<rmpb::ResourceGroup, errors::Error>`。
2. 转换函数按固定顺序检查：设置存在性、名称长度、Runaway 阈值、Runaway 动作、SwitchGroup 目标，然后组装 Background/RU 字段并检查 RU/Raw 冲突。
3. 某项检查失败时，函数直接返回本文件的 `Err...` 常量；因为常量类型就是 `ResourceGroupError`，调用方可按变体匹配或比较。
4. 若需要向用户或日志显示，`thiserror` 产生的 `Display` 使用变体上的固定字符串；本文件本身不包装、记录或转换错误。

该流程是纯同步、立即失败的输入转换路径；它不会创建 DDL job、更新 schema version 或等待集群同步。

## 数据与状态

`ResourceGroupError` 的所有变体都没有字段，因而错误值只携带“分类”，不携带组名、非法原值或下游错误源。`Copy` 表明这些值不需要所有权转移；`Eq`/`PartialEq` 使错误判定是精确的枚举分类比较。

9 个 `Err...` 符号是编译期常量，不是可变全局状态。它们与枚举变体一一映射，不存在注册表、缓存或运行时初始化顺序。

## 依赖与调用关系

- 直接下游依赖：`thiserror::Error`，仅用于派生标准错误与 `Display` 实现。
- 模块出口：`pkg/ddl/resourcegroup/lib.rs` 公开 `errors` 并将其全部公开项重导出，因此外部 crate 可以 `astersql_ddl_resourcegroup::ResourceGroupError` 引用。
- 生产调用者：`pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions` 使用 `ErrInvalidGroupSettings`、`ErrTooLongResourceGroupName`、`ErrResourceGroupRunawayRuleIsEmpty`、`ErrUnknownResourceGroupRunawayAction`、`ErrUnknownResourceGroupRunawaySwitchGroupName`、`ErrInvalidResourceGroupDuplicatedMode` 和 `ErrUnknownResourceGroupMode`。
- 当前未接线的 Rust 变体：精确搜索表明 `InvalidResourceGroupFormat` 与 `DroppingInternalResourceGroup` 除定义和错误文案测试外没有 Rust 生产引用。
- 测试调用者：RustCodeGraph 将 `NewGroupFromOptions` 的调用者指向 `pkg/ddl/resourcegroup/migration_aster_unit_test.rs`、`pkg/resourcegroup/tests/resource_group_test.rs` 中的转换测试；后者通过 `pkg/resourcegroup/tests/Cargo.toml` 的 dev-dependency 引入该 crate。
- crate 边界：`pkg/ddl/resourcegroup/Cargo.toml` 还依赖 protobuf 和 meta model，但这些依赖由 `lib.rs`/`group.rs` 使用，`errors.rs` 不直接访问它们。

## 错误处理与边界

错误优先级由 `NewGroupFromOptions` 的检查顺序而非本枚举决定。例如，超长名称会在 RU/Raw 模式冲突之前返回；Runaway 空阈值会在动作合法性之前返回。`migration_aster_unit_test.rs::validation_errors_match_go_behavior` 和 `runaway_validation_matches_go_order` 将这个顺序固化为回归契约。

边界与限制：

- 固定文案是兼容面；`error_messages_match_go_sentinels` 对 9 个文案逐一断言。修改文案会产生可见兼容变化。
- 枚举无字段，无法保留具体非法值；若需要上下文，必须评估是否扩展变体载荷，这会使 `Copy` 和现有比较方式受影响。
- 没有 `#[source]` 字段或错误链；这些是预期的领域校验结果，不包装 I/O/protobuf 失败。
- Go 的哨兵错误依靠对象身份并可用 `errors.Is`；Rust 通过枚举值相等表达同等分类语义，不是指针身份的直译。

## 并发与资源生命周期

本文件没有锁、原子量、任务、通道、异步函数、事务或外部资源。所有错误值均是无载荷的 `Copy` 值，生命周期与普通局部值相同，无需清理。

由于该类型只由可线程安全的基本枚举组成，它不引入额外的并发协议；真正的 DDL owner、job、schema 同步和回滚生命周期不在本 crate 的这个文件中。

## 与 Go 版本的对应关系

`pkg/ddl/resourcegroup/errors.go` 定义了同名的 9 个 `errors.New(...)` 变量；Rust 的 9 个枚举变体和 `Err...` 常量与它们在名称和文案上一一对应。`pkg/ddl/resourcegroup/group.go::NewGroupFromOptions` 与 Rust 的 `group.rs::NewGroupFromOptions` 使用同样的七类转换错误，并保持空设置、名称长度、Runaway 规则以及 RU/Raw 冲突的判定顺序。

Go 回归 `pkg/resourcegroup/tests/resource_group_test.go::TestNewResourceGroupFromOptions` 覆盖空设置、RU 正常路径、Raw 未支持、重复模式、超长名称和空 SwitchGroup 目标；Rust 的 `pkg/ddl/resourcegroup/migration_aster_unit_test.rs` 与 `pkg/resourcegroup/tests/resource_group_test.rs` 对应验证错误变体、顺序和文案。

已知接线差异是：Go `pkg/ddl/executor.go::DropResourceGroup` 在删除默认保留组时直接返回 `ErrDroppingInternalResourceGroup`；当前 Rust 侧只定义了对应变体/常量并验证文案，未找到生产调用。`ErrInvalidResourceGroupFormat` 在当前 Go/Rust `group` 转换函数中也没有返回点，属于保留的兼容符号。

## 扩展指南

- 新增校验失败类别时，先确认 Go 的差异与错误文案，再在 `ResourceGroupError` 新增变体，视迁移兼容需要增加同名 `Err...` 常量。
- 必须在实际产生错误的函数（通常是 `group.rs::NewGroupFromOptions`）接线，并明确新检查在现有优先级中的位置；仅增加枚举不代表功能已支持。
- 同步扩展独立测试文件 `pkg/ddl/resourcegroup/migration_aster_unit_test.rs` 和跨 crate 的 `pkg/resourcegroup/tests/resource_group_test.rs`：至少断言精确变体、检查顺序以及 `Display` 文案。Rust 单元测试不应内嵌到 `errors.rs` 源文件。
- 如果 Go 对照也变更，同步检查 `pkg/ddl/resourcegroup/errors.go`、`group.go` 和 `pkg/resourcegroup/tests/resource_group_test.go::TestNewResourceGroupFromOptions`。
- 若变体需要携带字符串、下游错误或其他非 `Copy` 数据，需重新评估 `Copy` 派生、公开常量形式、相等性语义和上游模式匹配；这是 API 兼容风险，不应为方便附加文案而轻率改动。
- 当错误真正进入 DDL Rust 主链时，应另行验证上层如何将其映射为 SQL 错误，不能仅依赖本文件的 `Display` 文案。

## 验证依据

- RustCodeGraph 索引状态：项目已索引，目标目录列出 `errors.rs` 的 2 个顶层符号；`query ResourceGroupError` 定位到 `pkg/ddl/resourcegroup/errors.rs:30`。
- RustCodeGraph `node pkg/ddl/resourcegroup/errors.rs::ResourceGroupError` 核对了 9 个变体及其文案；该枚举无 callers/callees 函数边，符合纯数据类型的实现。
- RustCodeGraph `callers pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions` 找到 `migration_aster_unit_test.rs::ru_mode_matches_go_protobuf_shape`、`runaway_and_background_settings_match_go_fields` 以及 `pkg/resourcegroup/tests/resource_group_test.rs` 的跨 crate 测试；`callees` 列出 `runaway_action` 和 `runaway_watch_type`。常量/变体的引用边未由图查询返回，因此用精确 `rg` 补齐。
- 已读 Rust 源与边界：`pkg/ddl/resourcegroup/errors.rs`、`group.rs`、`lib.rs`、`Cargo.toml`。
- 已读 Rust 测试：`pkg/ddl/resourcegroup/migration_aster_unit_test.rs`、`pkg/resourcegroup/tests/resource_group_test.rs` 及其 `Cargo.toml`。
- 已读 Go 对照与测试：`pkg/ddl/resourcegroup/errors.go`、`group.go`、`pkg/resourcegroup/tests/resource_group_test.go::TestNewResourceGroupFromOptions`；并用 `pkg/ddl/executor.go::DropResourceGroup` 核对保留组删除错误的 Go 生产接线。
- 本任务为纯文档分析，按计划不运行 Cargo；验收依靠源码/调用边事实核对与 11 章结构检查。
