# `pkg/meta/model/placement.rs`

## 文件定位

本文件定义 placement policy（放置策略）的 Rust 元数据模型，以及一组把设置项渲染成 SQL 风格文本的公共 helper。它只负责数据表示、Serde 编解码、克隆和文本格式化，不访问元数据存储，也不向 PD 下发调度规则；这一边界由文件头说明和文件内没有 I/O、网络或调度依赖共同确认。

实际编译边界不是根 `pkg/meta/model/lib.rs` 直接声明本模块，而是 `pkg/meta/model/internal/group3/lib.rs` 的 `placement` 子模块用 `include!("../../placement.rs")` 引入，再以 `pub use placement::*` 导出。根 crate `astersql-meta-model` 依赖 `astersql-meta-model-group3` 并通过 `group_3` 暴露它。`pkg/meta/model/Cargo.toml` 只依赖四个内部 group crate；本文件实际所需的 `serde` 和 `group-1`（提供 `ast::CIStr`、`SchemaState`）声明在 `pkg/meta/model/internal/group3/Cargo.toml`。

## 核心职责

- `PolicyRefInfo` 保存对策略的稳定引用：数值 `ID` 与大小写保留/归一名称 `ast::CIStr`。
- `PlacementSettings` 保存区域、副本数量、各角色约束、调度方式和存活偏好，并由 `String` 产生稳定顺序的设置文本。
- `PolicyInfo` 把 `PlacementSettings` 与策略 `ID`、`Name`、`SchemaState` 合成完整元数据记录。
- `writeSetting*ToBuilder` 统一实现字符串、整数、时长和普通设置项的追加与分隔规则；`resource_group.rs` 也直接复用这些 helper，因此它们不只是 placement 私有实现细节。
- `formatGoDuration` 为非负 `std::time::Duration` 提供接近 Go `time.Duration.String` 的格式，供 duration helper 和资源组展示使用。

本文件不解析约束语法，也不验证区域、副本数或策略状态是否合法。调用方必须在更高层完成这些业务校验。

## 主要符号

- `PolicyRefInfo { ID: i64, Name: ast::CIStr }`：可序列化的轻量引用。结构使用 `snake_case`，`ID` 显式映射为 JSON `id`。
- `PlacementSettings`：十二个公开字段。字符串字段默认空串，`Learners`、`Followers`、`Voters` 默认零；Serde 写出 snake_case 名，读取时还接受对应 Go 导出字段名（如 `PrimaryRegion`、`LeaderConstraints`）。
- `PlacementSettings::String(&self) -> String`：按固定顺序输出非零设置。顺序是 `PRIMARY_REGION`、`REGIONS`、`SCHEDULE`、`CONSTRAINTS`、`LEADER_CONSTRAINTS`、`VOTERS`、`VOTER_CONSTRAINTS`、`FOLLOWERS`、`FOLLOWER_CONSTRAINTS`、`LEARNERS`、`LEARNER_CONSTRAINTS`、`SURVIVAL_PREFERENCES`。
- `PlacementSettings::Clone(&self) -> Self`：返回拥有独立 `String` 的值克隆。
- `SeparatorFn<'a> = Box<dyn FnMut(&mut String) + 'a>`：允许调用者为非首项注入逗号等分隔行为。
- `writeSettingStringToBuilder`：写入 `ITEM="value"`；仅把值中的双引号替换为 `\"`。
- `writeSettingIntegerToBuilder`：写入无引号的 `ITEM=<u64>`。
- `writeSettingDurationToBuilder`：先调用 `formatGoDuration`，再按字符串设置项写入。
- `formatGoDuration`：零值为 `0s`；不足一秒时选择 `ms`、`µs` 或 `ns`；一秒以上组合 `h`、`m`、`s`，小数尾零被移除。
- `writeSettingItemToBuilder`：builder 非空时先执行所有 separator；未传 separator 时插入一个空格，最后追加 item。首项从不运行 separator。
- `PolicyInfo { PlacementSettings, ID, Name, State }`：Serde flatten 使设置字段与 `id`、`name`、`state` 位于同一 JSON 对象；读取字段同时兼容 `ID`/`Name`/`State` aliases。
- `PolicyInfo::Clone(&self) -> Self`：显式通过 `PlacementSettings::Clone` 复制嵌入设置，再复制其余字段。

文件没有 trait 定义、条件编译项或异步入口；所有类型和函数均为公开 API，只有 `formatGoDuration` 内部的 `decimal` 是局部函数。

## 执行流程

`PlacementSettings::String` 从空 `String` 开始，逐字段检查：空字符串和零副本数被跳过；字符串字段交给 `writeSettingStringToBuilder`，整数交给 `writeSettingIntegerToBuilder`。两个 helper 先构造完整 item，再进入 `writeSettingItemToBuilder`。由于 `String` 传入空 separator slice，第二项及以后自动以单个空格连接。最终字符串的顺序由函数中的条件顺序决定，与结构体字段声明顺序不完全相同，不能随意重排。

字符串设置先执行 `value.replace('"', "\\\"")`，所以例如约束中的 JSON 双引号在外层设置字符串中被反斜杠转义；函数不转义反斜杠、换行或其他字符。整数直接用十进制表示。

duration 流程先取得 `dur.as_nanos()`。零值立即返回 `0s`；小于一秒时选取不大于数值尺度的单位并用 `decimal` 输出可选小数；至少一秒时依次拆出小时、分钟和剩余秒。整小时省略分钟和秒，整分钟在已经输出分钟时补 `0s`，例如 `1m0s`；有纳秒余数的秒会输出去尾零的小数。

外部复用路径见 `resource_group.rs`：资源组的逗号分隔设置通过单元素 `SeparatorFn` 数组写入，而 runaway 子句内的空格分隔项使用空 slice；时长字段通过 `writeSettingDurationToBuilder` 输出。该路径证明 separator 和 duration helper 属于当前生产格式化链的一部分。

## 数据与状态

所有模型都是普通拥有型值。`PlacementSettings` 的字符串与标量没有共享可变状态；其派生 `Default` 表示全空/全零设置，`String` 因而返回空串。`PolicyInfo` 的 `PlacementSettings` 是内嵌值而非 Go 版本的指针，因此 Rust 中不存在 nil settings；Serde 的 `#[serde(flatten)]` 配合各设置字段的 `default`，允许缺少设置字段的 Go JSON 解码成默认值。

`PolicyInfo::State` 使用 group1 导出的 `SchemaState`，名称使用同一类型身份的 `ast::CIStr`。本文件只保存这些状态，不执行状态迁移。`Clone` 返回新值；修改克隆中的字符串、标量或嵌入设置不会影响原值。

`SeparatorFn` 在调用期间以可变 slice 借用，闭包可修改同一个 builder，但不会被本文件保存。`formatGoDuration` 使用 `u128` 纳秒计算，覆盖 `std::time::Duration` 的非负范围。

## 依赖与调用关系

向下依赖只有 `serde::{Serialize, Deserialize}`、`std::time::Duration`，以及由 `internal/group3/lib.rs` 注入作用域的 `ast::CIStr` 和 `SchemaState`。`serde_json` 不被本文件直接调用，但 group3 crate 和测试中的 `ast::metadata_json` 用它验证 wire format。

装配链为：`internal/group3/lib.rs::placement` → `include!(../../placement.rs)` → `pub use placement::*` → 根 `pkg/meta/model/lib.rs::group_3`。RustCodeGraph 对目标文件给出的直接使用文件是 `placement_test.rs` 与 `job_args_test.rs`；此外源码级直接依赖 `resource_group.rs` 通过 `use placement::{...}` 复用五个格式化符号，并随 group3 一起编译。`job_args.rs` 的多个参数结构保存 `PolicyRefInfo` 或 `PolicyInfo`，相关 round-trip 在 `job_args_test.rs` 覆盖。

Go 侧 `placement.go` 被 DDL、schema tracker、restore 等多个生产文件引用，说明这些模型位于元数据/DDL 交界处；但不能由此推断同名 Rust DDL 路径都使用本类型。当前 RustCodeGraph 中还存在 executor、DDL 等模块自己的同名 `PlacementSettings` 镜像，跨 crate 类型统一程度应以具体调用点为准。

## 错误处理与边界

本文件的公开函数均不返回 `Result`，也不主动 panic：格式化和克隆是确定性的内存操作。Serde 解码错误由派生实现和上层 codec 返回，本文件不捕获或改写错误。

重要边界如下：零数值和空字符串不会出现在 placement 设置文本中；传给 `writeSettingItemToBuilder` 的首项不触发 separator；非首项若有多个 separator 会按 slice 顺序全部执行；字符串只转义双引号，不能把输出当成通用 SQL 字符串转义器。`formatGoDuration` 只能接收非负 `std::time::Duration`，因此不覆盖 Go `time.Duration` 的负数表示；当前直接业务调用由非负毫秒转换而来。该函数模拟常用 Go 输出格式，但测试证据目前明确覆盖的是 `1500ms -> 1.5s`，其他极端值应新增独立测试后再宣称兼容。

Go `PolicyInfo` 的 settings 是指针，理论上可为 nil；Rust 改为非可选内嵌值，将“缺少 settings”规范化为默认设置。`placement_test.rs::crossks_align_policy_decode_go_missing_settings` 明确验证了该兼容行为。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。所有 builder 都由调用者以 `&mut String` 独占借用，Rust 借用规则阻止同一时刻的并发修改。separator 闭包的生命周期受 `SeparatorFn<'a>` 和传入 slice 限制，只在同步调用栈中执行并随调用者容器释放。

克隆产生独立拥有的数据，没有 `Arc`、`Rc` 或内部可变性；因此模型本身可安全地按 Rust 类型能力在线程间转移，但本文件不提供共享或同步策略。格式化期间仅分配局部 `String` 和临时 item，返回后没有后台工作或清理步骤。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/model/placement.go`，Rust 保留了三个模型的字段、`String` 顺序、双引号转义、整数格式、默认空格分隔和 Clone 隔离语义。`placement_test.go` 的三组断言在 `placement_test.rs` 中有对应覆盖：基础顺序、角色副本/约束顺序、约束内引号转义，以及两个 Clone 的修改隔离。

主要表示差异是：Go 方法返回指针克隆，Rust 返回拥有型值；Go `PolicyInfo` 匿名嵌入 `*PlacementSettings`，Rust 使用非可选值并通过 `#[serde(flatten)]` 保持 JSON 平铺；Go variadic `func()` separator 由 Rust 的 `&mut [SeparatorFn]` 表示，并显式把 builder 传给闭包；Go duration 可为负，Rust duration 不可为负。Rust 还为设置字段和顶层元数据字段增加 Go 导出名 aliases，以兼容旧 wire 名称，相关断言位于 `placement_test.rs::crossks_align_policy_go_wire_names_and_partial_settings`。

`PolicyRefInfo` 没有显式 Clone 方法，但派生的 `Clone` 覆盖值复制需求。Rust 的 `formatGoDuration` 是为替代 Go 标准库 `Duration.String` 而新增的局部实现，不能简单等同于已经覆盖 Go 标准库的全部输入空间。

## 扩展指南

新增 placement 设置字段时，应同时更新 `PlacementSettings` 字段及 Serde wire 名、`String` 中的非零判断和历史输出位置，并同步 `placement.go` 的字段/格式语义。输出顺序是兼容契约；新字段不能只按结构体末尾机械追加，应先核对 Go 实现和 SHOW/DDL 预期。同步扩展独立的 `pkg/meta/model/placement_test.rs`，覆盖默认省略、非默认输出、引号转义、Go 名称 JSON 解码和 clone 隔离；测试逻辑不要嵌入生产文件。

若调整通用 builder 或 duration，应同时检查 `pkg/meta/model/resource_group.rs` 及 `job_3_aster_unit_test.rs::placement_rendering_matches_go_order_escaping_and_duration`，因为分隔符、括号内空格和时长文本会影响资源组展示。新增 duration 能力时至少覆盖零、纳秒/微秒/毫秒、小数秒、整分钟、小时组合和最大值；若要支持负数，需要更换输入表示并明确与现有非负调用方的兼容策略。

若把这些类型接入新的 Rust DDL/executor crate，先确认对方是否已有同名镜像，避免静默引入不兼容的第二套类型。公共 API 命名当前为 Go 风格并由 crate 级 allow 接受；除非进行全调用链迁移，不应仅在本文件改名。涉及 wire format 的修改应保持旧 aliases 或提供迁移方案。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/meta/model` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file pkg/meta/model/placement.rs`：读取完整 305 行，确认全部结构、方法、helper、字段顺序和文件直接使用关系。
- RustCodeGraph `query PlacementSettings`、`query PolicyRefInfo`：确认本文件符号以及仓库中存在的 Go/Rust 同名镜像；没有把同名结果误认成调用边。
- RustCodeGraph `node`：读取 `pkg/meta/model/internal/group3/lib.rs` 的 include/re-export 装配、`pkg/meta/model/lib.rs` 的根再导出、`pkg/meta/model/resource_group.rs` 的 helper 复用、`pkg/meta/model/placement.go`、`placement_test.go`、`placement_test.rs` 和 `job_3_aster_unit_test.rs` 的对应行为。
- Cargo 证据：`pkg/meta/model/Cargo.toml` 确认根 crate 只聚合 group crates；`pkg/meta/model/internal/group3/Cargo.toml` 确认实际 crate 对 `group-1`、`serde`、`serde_json` 的依赖。
- 测试证据：`placement_test.rs` 验证固定输出顺序、零值省略、双引号转义、Clone 隔离、缺失 settings 解码、snake_case 写出和旧 Go 字段名读取；`job_3_aster_unit_test.rs` 验证 `1500ms` 渲染为 `1.5s`；`job_args_test.rs` 提供策略类型进入 Job 参数序列化路径的证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的命令验证本文恰含 11 个固定二级章节，并人工复查唯一新增生产物为本文件。
