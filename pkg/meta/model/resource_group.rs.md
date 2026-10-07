# `pkg/meta/model/resource_group.rs`

## 文件定位

`resource_group.rs` 定义资源组在元数据层的 Go 兼容数据模型：资源配额、调度优先级、失控查询规则、后台任务限制，以及资源组的 ID、名称和 schema 状态。它只负责保存、克隆、序列化和格式化这些数据，不负责校验 DDL、写事务或向 Resource Manager 下发配置。

该文件不是独立模块入口。`pkg/meta/model/internal/group3/lib.rs` 在私有 `resource_group` 模块中以 `include!("../../resource_group.rs")` 编译它，并通过 `pub use resource_group::*` 导出；顶层 `pkg/meta/model/lib.rs` 再以 `group_3` 暴露该 crate。实际 Rust API 因而通常写作 `astersql_meta_model::group_3::{ResourceGroupInfo, ResourceGroupSettings, ...}`。

crate 边界由 `pkg/meta/model/internal/group3/Cargo.toml` 定义为 `astersql-meta-model-group3`，直接依赖 `group-1`、`serde`（启用 `derive` 和 `rc`）、`serde_repr`、`serde_json`；顶层 `pkg/meta/model/Cargo.toml` 的 `astersql-meta-model` 仅聚合 group1–group4。`SchemaState` 和 `ast` 的真实类型身份由 group3 从 group1 复用，并非本文件的本地替代类型。

## 核心职责

1. 用 `ResourceGroupSettings` 表示 RU、优先级、CPU/IO、burst、runaway 和 background 设置，并维持与 `pkg/meta/model/resource_group.go` 相同的 JSON 字段名。
2. 用 `String` 把设置稳定地格式化为 SQL/SHOW 风格文本，包括 `QUERY_LIMIT=(...)` 与 `BACKGROUND=(...)` 两个嵌套片段。
3. 用 `Adjust` 和 `GetBurstLimitAdjusted` 统一有限 RU、无限 RU 与特殊 burst 值的解释。
4. 用 `ResourceGroupInfo` 把设置与持久化身份（`ID`、`Name`、`State`）组合；`#[serde(flatten)]` 使设置字段保持在 JSON 顶层，与 Go 的匿名嵌入字段一致。
5. 用 `Arc` 保存可选的 runaway/background 子设置，使派生 `Clone` 和显式 `Clone` 方法都保留 Go 指针字段的浅拷贝语义。

本文件不执行业务合法性校验。例如“runaway 至少有一个阈值”“切换动作必须有目标组”“当前只支持 RU 模式”等检查位于 `pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions`，不能把模型对象可构造等同于配置一定可下发。

## 主要符号

- `unlimitedRURate: u64 = i32::MAX as u64`：与 Go `math.MaxInt32` 对齐的无限 RU 哨兵。它会覆盖普通 `BurstLimit` 的读取解释。
- `ResourceGroupRunawaySettings`：保存 `ExecElapsedTimeMs`、`ProcessedKeys`、`RequestUnit` 三类触发阈值，`Action`/`SwitchGroupName` 处置，以及 `WatchType`/`WatchDurationMs` 观察策略。时间字段的存储单位都是毫秒。
- `ResourceGroupBackgroundSettings`：保存允许的 `JobTypes` 和 `ResourceUtilLimit`。JSON 键明确为 `job_types` 与 `utilization_limit`。
- `ResourceGroupSettings`：设置主体。`RURate`、`Priority`、三个 CPU/IO 字符串、`BurstLimit` 按值保存；`Runaway` 与 `Background` 是 `Option<Arc<...>>`。
- `ResourceGroupSettings::GetBurstLimitAdjusted(&self) -> i64`：当 `RURate == unlimitedRURate` 时无条件返回 `-1`，否则返回原 `BurstLimit`。
- `ResourceGroupSettings::String(&self) -> String`：按固定次序输出非零/非空字段；优先级始终输出。
- `ResourceGroupSettings::Adjust(&mut self)`：仅当 RU 不是无限哨兵且 burst 非负时，把 `BurstLimit` 更新为 `RURate as i64`；`-1`、`-2` 和无限 RU 分支保持原值。
- `ResourceGroupSettings::Clone(&self) -> Self`：Go 风格命名的浅拷贝门面，内部调用 Rust `Clone`；两个 `Arc` 子对象仍共享同一分配。
- `NewResourceGroupSettings() -> ResourceGroupSettings`：构造零值设置，但把 `Priority` 设为 `ast::MediumPriorityValue`。
- `format_go_duration(milliseconds: u64) -> String`：把毫秒转换为 `Duration`，再委托 `placement::formatGoDuration` 生成 Go `time.Duration.String` 风格文本。
- `ResourceGroupInfo`：以 `#[serde(flatten)]` 嵌入 `ResourceGroupSettings`，再加入 `ID: i64`、`Name: ast::CIStr` 和 `State: SchemaState`。
- `ResourceGroupInfo::Clone(&self) -> Self`：显式克隆设置外层值；设置内部的 `Arc` 仍是浅拷贝。

所有结构均派生 `Clone`、`Debug`、`Default`、`Serialize` 和 `Deserialize`。公开命名保留 Go 风格，group3 crate 在根部允许 `non_snake_case` 与 `non_upper_case_globals`。

## 执行流程

典型的创建/持久化/使用链如下：

1. 调用方通过 `NewResourceGroupSettings` 获得 medium 优先级的设置，或从 JSON 反序列化 `ResourceGroupInfo`。
2. DDL 或会话代码填充 RU、burst、runaway/background 字段。对普通 RU 配额，调用 `Adjust` 把非负 burst capacity 同步为 RU rate；特殊负值不被覆盖。
3. `pkg/meta/meta.rs::add_resource_group` / `update_resource_group` 将 `ResourceGroupInfo` 序列化为 JSON，加上 magic byte 后写入 `ResourceGroups` hash；`list_resource_groups` / `get_resource_group` 做逆向反序列化。`pkg/meta/reader.rs::SnapshotReader::get_resource_group` 也从 `ResourceGroups/ResourceGroup:<id>` 读取，并兼容有无 magic byte 的数据。
4. `pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions` 消费 `ResourceGroupSettings`：复制优先级、runaway/background，设置 RU token bucket 的 `fill_rate` 与 `burst_limit`，并在这里执行模式和规则校验。
5. 展示或日志路径调用 `String`。顶层字段以 `", "` 分隔；runaway 括号内的阈值和 action/watch 依赖 placement helper 的空格分隔；background 的两个字段以 `", "` 分隔。

`String` 的具体分支顺序是：可选 `RU_PER_SEC`、必有 `PRIORITY`、可选 CPU/读 IO/写 IO、可选特殊 `BURSTABLE`、可选 `QUERY_LIMIT`、可选 `BACKGROUND`。runaway 阈值只在大于零时输出；`SWITCH_GROUP` 动作额外带目标组名；watch 启用时，正时长格式化为 duration，非正时长写 `DURATION=UNLIMITED`。background 的任务类型按原顺序用逗号连接。

## 数据与状态

- `RURate == 0` 表示格式化时不输出 RU 配额；`RURate == unlimitedRURate` 表示无限 RU，并使 `GetBurstLimitAdjusted` 返回 `-1`。
- `BurstLimit == -2` 格式化为 `BURSTABLE(MODERATED)`，`-1` 格式化为 `BURSTABLE(UNLIMITED)`；其他值不产生 `BURSTABLE` 文本。非负值经 `Adjust` 后通常等于 RU rate。
- `Priority` 即使为默认/零值也始终通过 `ast::PriorityValueToName` 输出；构造器的默认值是 medium，而派生的 `Default::default()` 不会自动调用构造器，因此二者不能互换理解。
- runaway 三个阈值中，格式化只展示正值；模型本身仍可保存零值或负的 `i64` 阈值。合法性由下游转换层判断。
- `WatchDurationMs <= 0` 在 `String` 中仅当 watch 已启用时解释为无限；watch 为 `WatchNone` 时不输出 watch/duration。
- `Runaway`、`Background` 为 `None` 时对应片段完全缺席；为 `Some` 但内部均为空时，模型仍可生成空括号或仅 action 等文本，模型层不拒绝它。
- `ResourceGroupInfo` 序列化时 settings 被 flatten，所以 `ru_per_sec`、`priority`、`runaway`、`background` 与 `id`、`name`、`state` 同处顶层。该布局是元数据兼容契约。

## 依赖与调用关系

直接下游依赖：

- `serde::{Serialize, Deserialize}` 提供 Go 兼容 JSON；`serde` 的 `rc` feature 使 `Arc` 子对象可序列化。
- `std::sync::Arc` 表示共享的可选子设置；`std::time::Duration` 承接毫秒到格式文本的转换。
- group3 注入的 `ast` 提供优先级名称、`CIStr`、runaway action/watch 枚举；注入的 `SchemaState` 表示 DDL schema 生命周期。
- `placement::{SeparatorFn, formatGoDuration, writeSetting*ToBuilder}` 负责一致的分隔、引用、整数和时长格式。`writeSettingItemToBuilder` 在没有自定义 separator 时，对非空 builder 写一个空格。

RustCodeGraph 将目标文件标记为被 17 个文件使用。已核对的直接应用点包括：

- `pkg/meta/meta.rs` 与 `pkg/meta/reader.rs`：资源组元数据的事务写入、列表/单项读取和 JSON 解码。
- `pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions`：把模型转换成 Resource Manager protobuf，并执行模型层没有的有效性检查。
- `pkg/session/runtime/control.rs`：构造/反序列化默认 `ResourceGroupInfo`，持久化和读取 `Background`。
- `pkg/session/runtime/paging.rs::resource_group_allows_paging_size_bytes`：构造最小设置并通过 `GetBurstLimitAdjusted` 判断是否为有限 burst。
- `pkg/meta/model/job_args.rs`：DDL job 参数以 `Option<Box<ResourceGroupInfo>>` 携带资源组元数据。

仓库中还有 `pkg/ddl/resource_group.rs` 等同名本地结构，不能仅凭类型名视为本文件类型；判断调用关系时必须确认导入路径是否是 `astersql_meta_model::group_3` 或对应 `model` 别名。

## 错误处理与边界

本文件的公开方法均返回普通值，没有 `Result`，也不主动报错。边界行为主要表现为条件省略或特殊值解释：

- `String` 不验证未知优先级、空 runaway 规则、空 switch 目标、CPU/IO 与 RU 同时存在等问题；这些错误由 `NewGroupFromOptions` 返回 `ErrInvalidGroupSettings`、`ErrResourceGroupRunawayRuleIsEmpty`、`ErrUnknownResourceGroupRunawayAction`、`ErrUnknownResourceGroupRunawaySwitchGroupName`、`ErrInvalidResourceGroupDuplicatedMode` 或 `ErrUnknownResourceGroupMode`。
- 序列化/反序列化错误出现在调用层，例如 `pkg/meta/meta.rs` 的 `json::marshal/unmarshal` 或 `pkg/meta/reader.rs` 的 `serde_json::from_slice`，而不是模型方法内。
- `Adjust` 保留 Go 的 `u64 -> i64` 转换语义，但调用者仍应把正常有限 RU 限制在可表示范围；本文件不做溢出范围校验。
- `WatchDurationMs` 只有在大于零时才转换成 `u64 Duration`，因此负值不会发生负数转换，而是按 unlimited 文本处理。
- `String` 是格式化表示，不是可逆 parser，也不是下发成功的证明。新增校验不应偷偷放进该方法改变历史展示行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络资源，所有方法都是同步的内存操作。`String` 的 builder 和 separator 闭包仅在一次调用栈内存在；`Duration` 也是临时值。

唯一的共享所有权来自 `Option<Arc<ResourceGroupRunawaySettings>>` 与 `Option<Arc<ResourceGroupBackgroundSettings>>`。这些结构内部字段不是 interior mutable，公开 API 也只通过共享引用读取它们；`Clone` 增加强引用计数而不复制子对象。若调用者需要修改共享子设置，应构造新的值/`Arc`（或显式使用写时复制策略），不能假设克隆后已经深拷贝。外层 `ResourceGroupSettings`、字符串、向量、标量和 `ResourceGroupInfo` 的身份字段仍按 Rust `Clone` 规则拥有各自值。

持久化事务、快照和 Resource Manager 请求的生命周期分别属于 `pkg/meta/meta.rs`、`pkg/meta/reader.rs` 和 DDL/resourcegroup 层；本文件不持有这些资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/model/resource_group.go`，当前 Rust 版本逐项保留其主要语义：

- `unlimitedRURate` 都取 `math.MaxInt32`/`i32::MAX`。
- 三个设置结构和 `ResourceGroupInfo` 的 JSON 键、字段含义与 Go 一致；Rust 用 `#[serde(flatten)]` 模拟 Go 的匿名嵌入。
- Go 的 `*ResourceGroupRunawaySettings`、`*ResourceGroupBackgroundSettings` 对应 Rust `Option<Arc<...>>`，从而让浅克隆共享子对象。
- Go 构造器返回指针，Rust 构造器返回拥有所有权的值；默认优先级都为 medium。
- `GetBurstLimitAdjusted`、`Adjust`、`String` 的分支及输出顺序与 Go 对齐。时长最终委托 `placement::formatGoDuration`，覆盖诸如 1500ms -> `1.5s`、2000ms -> `2s` 的 Go 风格输出。
- Go 的 `Clone` 返回新指针；Rust 返回新值。两者都复制外层结构并共享原本的指针型子设置。

需要注意，Rust 的派生 `Default` 与 `NewResourceGroupSettings` 是两个不同入口：只有后者设置 medium 优先级。这是 Rust API 形态差异，不应在文档或新代码中把它们描述成等价构造。

## 扩展指南

新增或修改资源组字段时，应按以下边界同步：

1. 在对应结构中增加字段并明确 `serde` 键；若 Go 字段嵌入/省略规则不同，先以 `resource_group.go` 和已持久化 JSON 为兼容依据。
2. 判断该字段是否影响 `String` 的稳定顺序、引用方式或零值省略，并复用 `placement.rs` helper，避免产生另一套分隔/时长格式。
3. 若字段影响默认值或 burst 归一化，分别检查 `NewResourceGroupSettings`、`Adjust` 与 `GetBurstLimitAdjusted`；不要用修改派生 `Default` 的方式无意改变反序列化/结构更新语义。
4. 若字段要下发给 Resource Manager，同步 `pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions` 及其 protobuf 映射和错误分支；模型层不应替代该层的业务校验。
5. 若字段需要持久化，检查 `pkg/meta/meta.rs`、`pkg/meta/reader.rs` 以及默认资源组构造路径。保留 flatten 后的顶层 JSON 布局和 magic-byte 兼容。
6. 测试必须放在独立文件。优先扩展 `pkg/meta/model/resource_group_test.rs`：JSON 契约、每个格式化分支、特殊值和 clone 指针身份均应有断言；涉及下发校验时同步 `pkg/ddl/resourcegroup/migration_aster_unit_test.rs` 或 `pkg/resourcegroup/tests/resource_group_test.rs`，不要把测试内嵌进生产源文件。

兼容风险主要是已持久化 JSON 键/默认值、SHOW/日志字符串的精确格式以及 Go/Rust 浅拷贝差异；性能风险主要来自把当前 `Arc` 浅拷贝改成深拷贝，或在频繁格式化路径增加不必要分配。涉及 action/watch 枚举扩展时，还必须同步 AST 到 protobuf 的映射，避免未知枚举被静默降级。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/meta/model` 确认目标、Go 对照和独立测试均被索引；`node --file pkg/meta/model/resource_group.rs` 核对全部 295 行及“used by 17 files”；`query ResourceGroupInfo`、`query ResourceGroupSettings`、`query writeSettingItemToBuilder` 用于消除同名符号歧义；对 `placement.rs`、`meta.rs`、`reader.rs`、`ddl/resourcegroup/group.rs`、`session/runtime/control.rs` 和 `session/runtime/paging.rs` 的文件节点核对直接调用/数据流。
- 源文件：`pkg/meta/model/resource_group.rs`，核对常量、三个设置/信息结构、方法、serde 属性、特殊值和格式化分支。
- crate 与模块入口：`pkg/meta/model/Cargo.toml`、`pkg/meta/model/internal/group3/Cargo.toml`、`pkg/meta/model/internal/group3/lib.rs`、`pkg/meta/model/lib.rs`，核对 group3 编译归属、依赖、`include!` 与再导出关系。
- Go 对照：`pkg/meta/model/resource_group.go`，核对字段、JSON tag、构造器、调整、格式化和浅克隆语义。同目录没有专用 `resource_group_test.go`；Go 测试中的相关使用主要散见 `job_args_test.go`，本文件自身的直接行为由独立 Rust 测试补齐。
- Rust 测试：`pkg/meta/model/resource_group_test.rs` 验证 `utilization_limit` 等 Go JSON 键、完整格式化字符串、有限/无限 RU 调整和 `Arc::ptr_eq` 浅拷贝；`pkg/meta/model/job_3_aster_unit_test.rs` 另验证 job 使用场景中的 `Adjust`/`GetBurstLimitAdjusted`。
- 应用证据：`pkg/meta/meta.rs::{list_resource_groups,get_resource_group,add_resource_group,update_resource_group}`、`pkg/meta/reader.rs::SnapshotReader::get_resource_group`、`pkg/ddl/resourcegroup/group.rs::NewGroupFromOptions`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/paging.rs::resource_group_allows_paging_size_bytes`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级标题，并人工检查未把模型层能力、下游校验或同名结构混为一谈。
