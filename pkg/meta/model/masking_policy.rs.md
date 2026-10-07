# `pkg/meta/model/masking_policy.rs`

## 文件定位

本文件定义列级数据脱敏策略的持久化元数据模型，不负责解析 SQL、校验脱敏表达式或执行结果改写。它实际编译在 `astersql-meta-model-group3` crate 中：`pkg/meta/model/internal/group3/lib.rs` 的 `masking_policy` 子模块用 `include!("../../masking_policy.rs")` 引入本文件，并以 `pub use masking_policy::*` 对外再导出；顶层 `astersql-meta-model` crate 再通过 `pkg/meta/model/lib.rs` 暴露 group3。

`pkg/meta/model/Cargo.toml` 表明顶层 crate 是 group1–group4 的聚合门面；实际编译单元 `pkg/meta/model/internal/group3/Cargo.toml` 依赖 group1 和 `serde`。本文件从 group1 使用 `Time`，并通过 group3 模块注入的 `ast`、`SchemaState` 使用标识符、限制操作位图和 DDL schema 状态。

## 核心职责

- `MaskingPolicyStatus` 固定启用状态的持久化数值，并提供与 Go 展示语义一致的 `String`。
- `MaskingPolicyType` 及常量固定五种脱敏类型的元数据字符串，并保留增量迁移期间的旧名称别名。
- `MaskingPolicyInfo` 汇总策略身份、目标库表列、表达式、类型、限制操作、审计字段和 DDL 状态，并规定 JSON 形状。
- `Default` 构造可安全反序列化缺失字段的 Go 零值，其中时间字段显式使用公元 1 年，而不是 chrono 的其他默认值。
- `Clone` 提供与 Go 同名的值拷贝入口；本文件只描述和传输状态，不判断策略是否适用于某次查询。

## 主要符号

- `pub struct MaskingPolicyStatus(pub u8)`：透明序列化的状态新类型。`MaskingPolicyStatusDisable` 为 `0`，`MaskingPolicyStatusEnable` 为 `1`；`MaskingPolicyStatusDisabled`/`Enabled` 是同值兼容别名。公开元组字段也允许保留未知字节值。
- `MaskingPolicyStatus::String(self) -> &'static str`：`0`、`1` 分别返回 `DISABLED`、`ENABLED`，其他值返回空串，不报错。
- `pub type MaskingPolicyType = &'static str`：常量使用的静态字符串别名。正式值为 `MASK_FULL`、`MASK_PARTIAL`、`MASK_NULL`、`MASK_DATE`、`CUSTOM`；四个 `MaskingPolicyTypeMask*` 名称是兼容别名。持久化结构中的 `MaskingType` 则是拥有所有权的 `String`。
- `pub struct MaskingPolicyInfo`：完整策略记录。`ID` 和 `TableID`/`ColumnID` 保存稳定身份；`Name`、`DBName`、`TableName`、`ColumnName` 为 `ast::CIStr`；`Expression` 保存表达式文本；`Status`、`MaskingType`、`RestrictOps` 描述行为；`CreatedAt`/`UpdatedAt` 与创建更新者保存审计信息；`State` 保存 DDL 中间态。
- `is_zero`：仅供 serde 的 `skip_serializing_if` 使用，在 `RestrictOps == 0` 时省略 JSON 字段。
- `default_go_time`：用 Unix 时间戳 `-62135596800` 构造 `0001-01-01T00:00:00Z`，其 `expect` 记录该常量必须是 chrono 可表示时间的不变量。
- `Default for MaskingPolicyInfo`：数值、字符串、CIStr、状态和 schema 状态采用零值，两个时间采用 `default_go_time`。
- `MaskingPolicyInfo::Clone(&self) -> Self`：委托派生的 `Clone`，返回拥有型副本。

## 执行流程

1. DDL 或其他上游构造 `MaskingPolicyInfo`，填入策略目标、表达式、类型和状态；创建/修改/删除任务还可把它装入 `pkg/meta/model/job_args.rs` 的 `MaskingPolicyArgs`。
2. meta 层的 `create_masking_policy`/`update_masking_policy`（`pkg/meta/meta.rs`）接收该模型，用 JSON 编码后附加 magic byte，写入 `MASKING_POLICIES` hash；ID 有效性、存在或不存在检查属于 meta 层而非本文件。
3. `get_masking_policy`/`list_masking_policies` 剥离 magic byte 后反序列化 JSON。结构体的 `#[serde(default)]` 令缺失字段回落到 `MaskingPolicyInfo::default()`，字段上的 `rename` 和省略条件维持 Go 元数据形状。
4. infoschema、DDL 和 Job 参数代码读取或复制该记录；真正的表达式验证、缓存组织、策略匹配及查询结果脱敏均发生在本文件之外。
5. 展示状态时，调用 `MaskingPolicyStatus::String` 将已知字节映射成大写文本；未知值保持可反序列化，但展示为空串。

## 数据与状态

`MaskingPolicyInfo` 是拥有型快照：字符串和 `CIStr` 均随实例持有，`Clone` 后可独立修改。`Status` 的默认值由透明新类型的 `Default` 得到 `0`，即禁用；`State` 使用 `SchemaState::default()`，具体 DDL 推进由外部状态机负责。

JSON 字段全部使用 Go 的 snake_case 名称。`masking_type`、`created_by`、`updated_by` 在空字符串时省略，`restrict_ops` 在零位图时省略；`created_at`、`updated_at` 在 Rust 中不会省略，默认序列化为 `0001-01-01T00:00:00Z`。`MaskingPolicyStatus` 使用 `#[serde(transparent)]`，因此 JSON 是数字而不是对象或名称。`RestrictOps` 的底层类型在 `pkg/parser/ast/lib.rs` 中是 `u8`，各操作以独立 bit 表示，本文件只保存位图，不解释组合。

该类型没有内部校验：零 ID、空名称、任意表达式、未知状态字节或任意脱敏类型字符串都能存在于内存对象中。持久化入口对创建时的零 ID和对象存在性另行检查，语义合法性应由 DDL/解析层保证。

## 依赖与调用关系

下游依赖很小：`serde::{Serialize, Deserialize}` 决定兼容编码；`group_1::time::Time` 是 group1 再导出的 `chrono::DateTime<Utc>`；模块注入的 `ast::CIStr`、`ast::MaskingPolicyRestrictOps` 和 `SchemaState` 提供共享模型身份。`default_go_time` 调用 `Time::from_timestamp`，`Clone` 调用派生的 `clone`，除此之外没有 I/O 或服务调用。

已核对的直接上游包括：

- `pkg/meta/meta.rs` 的 create/update/get/list masking-policy 方法负责 JSON 持久化与恢复。
- `pkg/meta/model/job_args.rs` 的 `MaskingPolicyArgs` 在 create/alter 动作中携带 `Option<Box<MaskingPolicyInfo>>`。
- `pkg/ddl/masking_policy.rs` 构造策略记录，`pkg/infoschema/infoschema.rs` 查询、缓存和克隆策略记录。
- `pkg/meta/reader.rs` 在只读接口中返回单个或多个 `model::MaskingPolicyInfo`。

RustCodeGraph 对 `MaskingPolicyInfo` 的符号查询同时定位到本定义，以及 DDL 构造、infoschema 加载/查询/克隆和 Go 对照；对宽泛调用边的输出不足以区分同名 Go/Rust 类型，因此上述具体接线又以限定路径搜索核验。

## 错误处理与边界

本文件没有业务 `Result` 返回值。唯一可能 panic 的位置是 `default_go_time` 的 `expect`，其输入为编译期固定的 Go 零时间时间戳；如果底层时间库未来不再接受该范围，任何 `MaskingPolicyInfo::default()` 或依赖 serde 默认补字段的反序列化都会失败。

未知 `MaskingPolicyStatus` 不被拒绝：透明反序列化接受任意 `u8`，`String` 对未知值返回 `""`，与 Go switch 的 default 分支一致。`MaskingPolicyType` 也没有枚举封闭性，结构字段可包含常量集合外的字符串。JSON 编解码错误、无效 ID、重复创建、不存在对象和存储错误由 `pkg/meta/meta.rs` 等调用层传播。

Rust 的 `MaskingPolicyInfo::Clone` 只能在有效引用上调用，返回值而非指针；它不复刻 Go `(*MaskingPolicyInfo)(nil).Clone() == nil` 的 nil 接收者分支。当前所有字段都是值或拥有型字段，派生深拷贝满足现有语义；若未来加入共享可变句柄，必须重新评估这一结论。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。模型没有内部可变性和析构协议；实例生命周期完全由调用者所有权决定，克隆产生另一份拥有型值。派生类型没有显式 `Send`/`Sync` 实现，其并发能力由所有字段的自动 trait 推导。

持久化事务生命周期属于 `pkg/meta/meta.rs` 中持有的事务对象；infoschema 缓存可能用 `Arc` 共享策略快照，但共享方式不在本文件中定义。时间字段只是 UTC 值，不持有计时器或系统时钟资源。

## 与 Go 版本的对应关系

直接对照 `pkg/meta/model/masking_policy.go`：状态的 `byte` 对应 Rust 的 `u8` 新类型，数值 `0/1`、旧名称别名和未知状态空字符串行为一致；五个类型字符串及四个旧别名逐项一致；`MaskingPolicyInfo` 的字段集合和 JSON 名称逐项对应。

需要注意三处语言适配：

- Go 的 `MaskingPolicyType` 是拥有值语义的命名 `string`，Rust 常量别名是 `&'static str`，但结构字段使用 `String`，因此可保存动态或未知值。
- Go `time.Time` 的零值由语言提供；Rust 用 `default_go_time` 显式构造同一时刻。尽管 Go 标签对时间写有 `omitempty`，结构体零值通常仍被编码；Rust 测试固定要求两个时间字段存在。
- Go `Clone` 对 nil 接收者返回 nil，非 nil 时进行结构体浅拷贝；Rust 不存在 nil 引用并返回 `Self`。当前字段没有指针式共享内容，所以派生 `Clone` 得到等价的独立值快照。

`pkg/meta/model/masking_policy_test.rs` 验证 Rust JSON 使用小写字段名、状态编码为数字、零限制操作被省略以及零时间文本；`pkg/meta/model/job_3_aster_unit_test.rs` 验证状态字符串、类型常量、克隆和 `id` JSON 字段。Go 侧模型本身没有同名独立测试，CRUD 行为由 `pkg/meta/meta_test.go::TestMaskingPolicy` 覆盖。

## 扩展指南

新增持久化字段时，应同时修改 `MaskingPolicyInfo`、`Default`、Go 同路径结构和独立 Rust 测试，并明确 JSON 名称、旧数据缺字段时的默认值及是否允许省略；不要把测试嵌入生产 `.rs`。若字段进入 meta JSON，需验证旧元数据反序列化和 Rust/Go 双向兼容。

新增状态值时，固定新的数值判别并扩展 `String` 与状态 round-trip 测试；不可重排既有 `0/1`。新增脱敏类型时，同步正式常量、必要的兼容别名、Go 常量，以及解析/DDL/执行层的语义处理；仅在本文件加字符串不会自动使查询执行支持该类型。

改变 `RestrictOps`、时间字段或省略规则会改变持久化 JSON，需同步检查 `pkg/meta/meta.rs` 的 CRUD、`pkg/meta/model/masking_policy_test.rs` 和 Go 兼容数据。改变 `Clone` 前应搜索 infoschema 缓存与 DDL Job 参数调用点，防止引入共享可变状态或破坏快照语义。性能上该模型的主要成本是拥有型字符串克隆和 JSON 编解码；扩展大字段时应评估缓存复制与元数据存储开销。

## 验证依据

- 源文件：`pkg/meta/model/masking_policy.rs`，核对全部常量、类型、serde 属性、辅助函数及 impl；文件无条件编译分支。
- 编译边界：`pkg/meta/model/Cargo.toml`、`pkg/meta/model/internal/group3/Cargo.toml`、`pkg/meta/model/internal/group3/lib.rs`、`pkg/meta/model/lib.rs`。
- Go 对照：`pkg/meta/model/masking_policy.go`；限制操作底层定义：`pkg/parser/ast/lib.rs`。
- Rust 调用与持久化：`pkg/meta/meta.rs`、`pkg/meta/reader.rs`、`pkg/meta/model/job_args.rs`、`pkg/ddl/masking_policy.rs`、`pkg/infoschema/infoschema.rs`。
- 独立测试：`pkg/meta/model/masking_policy_test.rs`、`pkg/meta/model/job_3_aster_unit_test.rs`、`pkg/meta/meta_test.rs`；Go 行为参考 `pkg/meta/meta_test.go::TestMaskingPolicy`。
- RustCodeGraph：`status` 确认索引含 Rust/Go 文件；`query MaskingPolicyInfo --kind struct` 定位 Rust/Go 定义及 DDL、infoschema 相关符号；随后按限定路径搜索消除同名符号歧义。
- 按任务约束未运行 Cargo；本纯文档任务以事实复核和固定十一章节结构检查作为验收。
