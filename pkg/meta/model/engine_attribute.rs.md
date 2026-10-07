# `pkg/meta/model/engine_attribute.rs`

## 文件定位

本文件定义表和分区 `ENGINE_ATTRIBUTE` 中存储类（storage class）部分的 Rust 元数据模型及少量无状态辅助逻辑。它不是 DDL 校验器，也不直接读写元数据：DDL 对输入的严格校验、规范化和把定义投影到表/分区的工作位于 `pkg/ddl/storage_class.rs`；本文件负责保留顶层原始 JSON、表达规范化后的结构，以及生成对外展示用的存储类字符串。

编译边界并不只是表面上的 `pkg/meta/model/Cargo.toml`。`pkg/meta/model/internal/group1/lib.rs:468-470` 通过 `#[path = "../../engine_attribute.rs"]` 把本文件编入 `astersql-meta-model-group1` 并公开再导出；`pkg/meta/model/lib.rs` 又以 `pub use ::group_1::*` 从聚合 crate `astersql-meta-model` 导出这些符号。聚合 crate 的 `Cargo.toml` 只依赖四个内部 group，而实际所需的 `serde` 和带 `raw_value` feature 的 `serde_json` 声明在 `pkg/meta/model/internal/group1/Cargo.toml`。

## 核心职责

1. `EngineAttribute` 保存 `storage_class` 字段的原始 JSON 片段，使 DDL 层能在不提前限定该字段形状的情况下继续解析；未知顶层字段被忽略。
2. `ParseEngineAttributeFromString` 把持久化或 SQL 选项中的字符串转为上述顶层模型，并为未设置属性的空串和顶层 `null` 提供零值语义。
3. `StorageClassDef`、`StorageClassSettings` 和 `StorageClassTransitRule` 表达存储层级、分区作用域和迁移规则，供 `pkg/ddl/storage_class.rs` 做严格解析、规范化、校验与分配。
4. `HasNoScopeDef` 判定一个定义是否为表级默认定义；`TotalSeconds` 统一迁移等待时间；`buildStorageClassString` 为 `TableInfo::StorageClassString` 和 `PartitionDefinition::StorageClassString` 生成紧凑表示。

本文件刻意不负责合法 tier、互斥作用域、迁移方向或正等待时间等业务约束。这些约束由 `pkg/ddl/storage_class.rs` 的 `check_tier`、`normalize`、`BuildStorageClassSettingsFromJSON` 和 `BuildStorageClassForPartitions` 承担。

## 主要符号

- `EngineAttribute { StorageClass: Option<Box<serde_json::value::RawValue>> }`：顶层 `ENGINE_ATTRIBUTE` 模型。`RawValue` 保留 `storage_class` 值自身的词法 JSON，例如测试证明对象内部空格仍保留；`Option::None` 表示字段缺失，`Some(raw)` 也可以承载 JSON `null`。
- `impl PartialEq for EngineAttribute`：比较 `RawValue::get()` 返回的原始文本，而非按 JSON 语义归一化。因此空白或键顺序不同但语义相同的对象仍可能不相等。
- `impl Deserialize for EngineAttribute`：自定义 visitor 以 ASCII 大小写不敏感方式识别 `storage_class`；同名大小写变体重复出现时后值覆盖前值；未知键通过 `IgnoredAny` 消耗。它只实现 unit 和 map 路径，其他 JSON 类型由反序列化器报错。
- `ParseEngineAttributeFromString(&str) -> Result<EngineAttribute, serde_json::Error>`：空串或去除首尾空白后等于 `null` 时返回默认值，其余输入交给 `serde_json::from_str`。
- `StorageClassTierStandard`、`StorageClassTierIA`、`StorageClassTierDefault`：当前层级名分别为 `STANDARD`、`IA`，默认层级引用 `STANDARD`。
- `StorageClassDef`：含 `Tier`、`NamesIn`、`LessThan`、`ValuesIn`、`Transitions`。可选容器让 Rust 区分 JSON 缺失/`null` 与显式数组；后续 DDL 逻辑负责收敛业务语义。
- `StorageClassDef::HasNoScopeDef`：仅当 `names_in` 缺失或为空、`less_than` 缺失、`values_in` 缺失或为空时返回真。特别地，`LessThan: Some("")` 仍是“有作用域”。
- `StorageClassSettings { Defs: Option<Vec<Option<StorageClassDef>>> }`：容纳一组可选定义。双层 `Option` 保留 Go 指针切片/JSON `null` 的形状，DDL 严格解析会拒绝定义数组内的 `null`。
- `StorageClassTransitRule`：记录目标 `Tier`、`AfterDays` 与 `AfterSeconds`；字段均参与序列化，零秒不会被省略。
- `StorageClassTransitRule::TotalSeconds`：以 `AfterDays.wrapping_mul(86400).wrapping_add(AfterSeconds)` 计算 `u64` 秒数，明确使用回绕算术。
- `buildStorageClassString`：无迁移规则时原样返回 tier；有规则时序列化局部 `StorageClassInfo { tier, transitions }`。序列化失败会经 `unwrap_or_default` 降级为空串，但当前成员均为字符串、切片和整数，正常内存条件下没有数据相关的序列化失败分支。

## 执行流程

解析路径从 DDL 进入：`pkg/ddl/storage_class.rs` 的 `CheckStorageClassAdmission`、`GetEngineAttributeFromStorageClassTableOptions`、`get_settings`、`handle_create`，以及 `pkg/ddl/persistent_actions.rs` 的 `modify_engine_attribute` 调用 `ParseEngineAttributeFromString`。解析器先处理空串/顶层 `null`，否则自定义 visitor 遍历顶层对象；遇到大小写不敏感的 `storage_class` 就保存其原始 JSON，遇到其他字段就跳过。DDL 层随后对 `raw.get()` 调用 `BuildStorageClassSettingsFromJSON`，完成严格字段解码、tier 大写化、分区名小写化、作用域互斥及迁移规则校验。

元数据落地路径中，`BuildStorageClassForTable` 用第一个 `HasNoScopeDef()` 的定义设置表级 tier 和迁移规则；`BuildStorageClassForPartitions` 先借助该方法区分默认定义与分区范围定义，再按名称、RANGE 上界或 LIST 值匹配分区。由此可见，本文件的“无作用域”判定是 DDL 分配行为的关键分界，但具体匹配和冲突处理不在本文件。

展示路径中，`pkg/meta/model/table.rs` 的 `TableInfo::StorageClassString` 与 `PartitionDefinition::StorageClassString` 调用 `buildStorageClassString`：没有迁移时只返回 `STANDARD`/`IA` 等 tier；有迁移时返回含 `tier` 与 `transitions` 的紧凑 JSON。该输出也是 Rust/Go 对照测试校验的稳定格式。

## 数据与状态

所有公开结构均为拥有所有权的值对象，没有全局可变状态。`EngineAttribute` 的 `RawValue` 通过 `Box` 拥有输入片段；克隆会生成独立的拥有值。`StorageClassDef` 的三个作用域字段具有不同判定规则：空 `NamesIn`/`ValuesIn` 与缺失等价，但任何 `Some(LessThan)`（包括空字符串）都表示范围已定义。

`StorageClassSettings::Defs` 可以表达缺失定义列表，列表元素也可以是 `None`；这是数据模型的可表示状态，不等于业务有效状态。`pkg/ddl/storage_class.rs:210-255` 的严格构建器会为未提供输入建立默认 `STANDARD` 定义，并拒绝数组中的空定义。类似地，结构体派生的 `Deserialize` 本身不会验证 tier 或作用域组合，调用方不能把“成功反序列化”等同于“可执行的存储类配置”。

迁移时间使用 `u64`。`TotalSeconds` 对极大天数采用回绕而非报错或饱和；正常 DDL 路径目前只检查结果大于零，没有在本文件中设置最大值。序列化会保留 `after_days` 和 `after_seconds` 的零值；`pkg/meta/model/table_test.go::TestStorageClassTransitionsJSONIncludesAfterSeconds` 说明 CSE 依赖 `after_seconds` 即使为零也存在。

## 依赖与调用关系

直接下游依赖只有 `serde` 与 `serde_json`：derive 提供普通结构的序列化/反序列化，自定义 visitor 使用 `MapAccess`、`IgnoredAny`，原始字段使用启用了 `raw_value` feature 的 `RawValue`。文件没有 I/O、事务、日志、配置或异步运行时依赖。

RustCodeGraph 的文件节点将直接使用者列为 `pkg/ddl/persistent_actions.rs`、`pkg/ddl/storage_class.rs`、`pkg/ddl/storage_class_test.rs` 和 `pkg/meta/model/index_test.rs`。其中可由源码引用确认的生产边包括：

- `pkg/ddl/persistent_actions.rs:614` -> `ParseEngineAttributeFromString`，在修改 engine attribute 的持久化动作前验证顶层 JSON；失败时取消 job。
- `pkg/ddl/storage_class.rs` -> `ParseEngineAttributeFromString`、`StorageClassDef`、`StorageClassSettings`、`TotalSeconds`、`HasNoScopeDef`，负责 admission、严格配置解析、规范化及表/分区分配。
- `pkg/meta/model/table.rs:294,1274` -> `buildStorageClassString`，为表和分区模型公开 `StorageClassString`。

`pkg/meta/model/index_test.rs` 通过 `use crate::group_1::*` 导入整个 group，图索引把它视为文件使用者，但未发现它对本文件具体符号的引用，因此不能将其当作本文件的行为测试。精确符号调用应以以上源码位置为准。

## 错误处理与边界

`ParseEngineAttributeFromString` 的错误类型是原始 `serde_json::Error`。空字符串和顶层 `null` 是显式成功零值；非法 JSON、数组、字符串、数字或布尔等不符合 visitor 接受形状的输入会失败。对象中的未知顶层字段不会失败；`storage_class` 的内部格式也不在此处验证，而是在 DDL 层读取 `RawValue` 后处理。

大小写不敏感和“最后一个匹配键获胜”是当前兼容行为：`pkg/ddl/storage_class_test.rs::storage_class_go_json_zero_values_duplicates_and_transition_boundaries` 用 `Storage_Class` 与 `STORAGE_CLASS` 验证结果为后者 `"IA"`。新增字段时应谨慎避免引入仅大小写不同的歧义，并明确重复字段策略。

`TotalSeconds` 不返回错误，溢出按 Rust `wrapping_*` 明确定义回绕。`buildStorageClassString` 同样不暴露错误，序列化错误被折叠为空串。若未来字段引入可失败的自定义序列化器，应重新评估静默空串是否仍安全；不能仅沿用当前签名掩盖新增失败。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、任务、通道、事务、文件句柄或网络资源。全部函数只读取借用参数或构造拥有值，因此同一不可变实例可由调用方并发读取；结构体是否跨线程共享由其成员的自动 trait 和调用方容器决定，本文件不创建共享所有权。

生命周期只在 `buildStorageClassString` 的局部 `StorageClassInfo<'a>` 中显式出现：它在一次 `serde_json::to_string` 调用期间借用 tier 与 transitions，函数返回前即销毁，不会把引用逸出。`RawValue` 使用 `Box` 而非借用输入，因此 `ParseEngineAttributeFromString` 返回值不依赖输入字符串的生命周期。DDL 的事务与 job 生命周期属于调用者，尤其是 `persistent_actions.rs::modify_engine_attribute`，不能归因于本文件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/model/engine_attribute.go`。Rust 保留了相同的公开概念、JSON 字段名、tier 常量、无作用域判定、秒数换算以及“无 transitions 时返回裸 tier，否则返回 JSON”的输出策略。`StorageClassTransitRule` 的 Rust `u64` 对应 Go 在当前平台使用的 `uint`；Rust 以回绕操作固定了 release/debug 一致性，而 Go 无符号算术本身按机器字宽回绕，因此极值宽度并非跨架构完全相同。

有几项实现层差异需要在扩展时保留意识：

- Go `ParseEngineAttributeFromString` 只特判空串，`json.Unmarshal("null", &attr)` 也会成功且保持零值；Rust 显式把去除首尾空白后的 `null` 当零值，效果覆盖了 Go 的常见结果，并额外接受带周围空白的 `null`。
- Go `json.RawMessage` 保存字段原始字节；Rust `Box<RawValue>`承担相同职责。Rust 自定义反序列化器显式落实 Go JSON 字段名大小写不敏感的行为。
- Go 的切片和指针天然表示 `nil`；Rust 用 `Option<Vec<_>>`、`Option<String>` 和 `Vec<Option<_>>` 保留相应形状。`HasNoScopeDef` 将 nil 与空切片视为相同，但不把非 nil 的空 `LessThan` 当成缺失，与 Go `*string != nil` 一致。
- Go `buildStorageClassString` 忽略 `json.Marshal` 错误；Rust 用 `unwrap_or_default` 对应地降级为空串。

Rust 回归证据位于独立测试文件 `pkg/meta/model/go_merge_15_test.rs`、`pkg/meta/model/go_merge_18_test.rs` 和 `pkg/ddl/storage_class_test.rs`；Go 对照证据包括 `pkg/meta/model/table_test.go` 以及 DDL 的 `pkg/ddl/storage_class_test.go`。测试没有内嵌到生产源文件，符合仓库约束。

## 扩展指南

新增顶层 engine attribute 字段时，应修改 `EngineAttribute` 及其自定义 visitor，并决定是否需要像 `storage_class` 一样保留原始 JSON、是否大小写不敏感、重复键如何处理；同时在独立的 `pkg/meta/model/*_test.rs` 增加空值、未知字段、重复字段和原始文本保真测试。若字段参与 DDL，严格业务解析与错误映射应放在拥有该行为的 DDL 模块，而不是把所有策略塞进本文件。

扩展存储类作用域时，需要同步修改 `StorageClassDef`、`HasNoScopeDef`、`pkg/ddl/storage_class.rs::normalize` 与 `BuildStorageClassForPartitions`。必须明确新作用域与 `names_in`、`less_than`、`values_in` 的互斥关系，并补充 DDL 独立测试；否则新字段可能被误判成默认表级定义。

新增迁移规则字段时，应同时检查 `StorageClassTransitRule` 的 Go 对照、序列化零值要求、`TotalSeconds` 的溢出语义、`normalize` 的合法迁移限制以及 `table.rs` 的展示输出。若改变 `buildStorageClassString` 的格式，会影响表/分区 `StorageClassString` 的外部表示，存在元数据兼容和字符串比较风险；应更新 Rust 的 `go_merge_15_test.rs`、`go_merge_18_test.rs` 与 Go 的 `table_test.go` 对照用例。

性能上，`ParseEngineAttributeFromString` 会分配顶层键字符串并拥有化原始字段，展示函数也会分配新字符串；当前属性很小，这比引入共享生命周期更简单。若将来配置显著增大，应先用真实元数据负载度量，再考虑优化，不能破坏 `RawValue` 的原文保真和聚合 crate 的公开类型身份。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/meta/model/engine_attribute.rs` 报告目标文件含 20 个符号。
- RustCodeGraph 源码/符号查询：`node --file pkg/meta/model/engine_attribute.rs --offset 1 --limit 400`；`query EngineAttribute`、`query HasNoScopeDef`、`query TotalSeconds`、`query buildStorageClassString`；以及对 `ParseEngineAttributeFromString`、`HasNoScopeDef`、`TotalSeconds`、`buildStorageClassString` 的精确 `node` 查询。精确 `callers` 查询在 60 秒内无结果后中止，因此调用边另由以下直接源码引用核验。
- 已读生产与装配路径：`pkg/meta/model/engine_attribute.rs`、`pkg/meta/model/engine_attribute.go`、`pkg/meta/model/Cargo.toml`、`pkg/meta/model/internal/group1/Cargo.toml`、`pkg/meta/model/internal/group1/lib.rs`、`pkg/meta/model/lib.rs`、`pkg/meta/model/table.rs`、`pkg/ddl/storage_class.rs`、`pkg/ddl/persistent_actions.rs`。
- 已读测试路径：`pkg/meta/model/go_merge_15_test.rs`、`pkg/meta/model/go_merge_18_test.rs`、`pkg/meta/model/table_test.go`、`pkg/ddl/storage_class_test.rs`、`pkg/ddl/storage_class_test.go`；并检查 `pkg/meta/model/index_test.rs` 后确认其没有具体调用目标符号。
- 关键测试事实：非法 JSON 报错；空串和顶层 `null` 为零值；`storage_class` 原始文本保真；空/缺失列表与 `LessThan: Some("")` 的范围判定不同；两天三秒等于 172,803 秒；无 transitions 返回裸 tier；有 transitions 输出紧凑 JSON；`after_seconds: 0` 仍序列化；表和分区克隆不会共享迁移规则容器。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 标题结构命令，并人工检查唯一生产物、源码链接、无行为修改和无无依据的支持性结论。
