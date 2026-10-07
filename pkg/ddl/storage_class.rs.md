# `pkg/ddl/storage_class.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的包名），由 `pkg/ddl/lib.rs` 以 `pub mod storage_class` 暴露。它负责把 SQL 表选项或 `ENGINE_ATTRIBUTE` 中的 `storage_class` JSON 转换成规范化的 `StorageClassSettings`，再把选中的 tier 和 transition 写入 `model::TableInfo`、`model::PartitionDefinition` 等规范元数据。它处于 DDL 前端建模与持久化动作之间：`pkg/ddl/create_table.rs` 在建表信息生成时调用本模块，`pkg/ddl/persistent_actions.rs` 在 ALTER 的持久化动作中重新解析并重建表、分区属性，`pkg/ddl/schematracker/dm_tracker.rs` 复用同一套规则维护 schema tracker。

它不是 DDL job 调度器，也不直接写 `mysql.tidb_ddl_job`、推进 schema state 或执行存储层数据迁移。对 DDL 执行框架而言，本文件提供确定性的元数据校验和派生结果；真正的 job 持久化、owner 调度和 transition 观测由其他模块承担（参见 `pkg/ddl/persistent_actions.rs`、`pkg/ddl/storage_class_transition.rs`）。

## 核心职责

1. `CheckStorageClassAdmission` 根据全局 `enable_storage_class` 开关拒绝新的 storage-class DDL，同时检查显式 engine attribute、复制来的表级元数据和分区级元数据，避免 `CREATE TABLE ... LIKE` 绕过准入。
2. `BuildStorageClassSettingsFromJSON` 严格解析字符串、对象或数组形式的定义，模拟 Go `encoding/json` 在大小写字段、重复字段及 `null` 零值上的关键行为，并经 `normalize` 统一大小写和验证约束。
3. `GetEngineAttributeFromStorageClassTableOptions`、`CheckStorageClassConflictInAlterTableSpecs` 和 `GetSimpleTableStorageClassForShowCreate` 在 SQL 语法糖与 JSON 属性之间转换，同时防止 `ENGINE_ATTRIBUTE` 与 `STORAGE_CLASS` 混用或在 `SHOW CREATE` 中错误简化复杂配置。
4. `BuildStorageClassForTable` 与 `BuildStorageClassForPartitions` 根据无作用域默认项和分区作用域项派生规范元数据；分区规则按定义顺序匹配，首个命中项优先，然后回退到默认项或 `STANDARD`。
5. `normalize_partition_definitions`、`check_final_definitions`、`CheckAndUpdateAddedPartitionDefinitions` 和 `update_checked_definitions` 保证 ADD/REORGANIZE 等操作使用表达式归一化、顺序检查后的最终分区定义，而不是未经验证的输入片段。

## 主要符号

- `pub type Result<T> = Result<T, String>`：本模块的轻量错误边界。`invalid` 将规则错误包装为 `ErrStorageClassInvalidSpec`，`format_error` 将 engine-attribute JSON 格式错误包装为 `ErrEngineAttributeInvalidFormat`。
- `CheckStorageClassAdmission(engine_attribute, table)`：公开准入入口；功能关闭时只要请求或既有表/分区元数据含 storage class 就返回 `ErrGeneralUnsupportedDDL`，功能开启时直接允许。
- `StrictDef`、`StrictRule` 及其 `Deserialize`：模块私有严格解码器。字段名按 ASCII 小写比较，未知字段报错；字符串和数字原始字段的 `null` 转为零值，数组中的空元素在随后规范化阶段被拒绝。
- `BuildStorageClassSettingsFromJSON(input)`：公开 JSON 入口。`None` 生成单个无作用域 `STANDARD` 默认项；字符串变成单项定义；对象和数组使用严格解码；最终逐项调用 `normalize`。
- `normalize(def)`：把 tier 转大写、`names_in` 转小写、transition tier 转大写；仅允许 `STANDARD`/`IA`，仅允许一个持续时间大于零的 `STANDARD -> IA` transition，并禁止同时声明多个作用域类型。
- `GetEngineAttributeFromStorageClassTableOptions(options)`：扫描所有 table options、验证每个相关出现项，拒绝两种语法混用，并以最后一个同类选项作为结果；`STORAGE_CLASS` 被编码成 `{"storage_class": ...}`。
- `GetSimpleTableStorageClassForShowCreate(table)`：只有 engine attribute 恰好只有 `storage_class` 一个字段，且解析后只有一个无作用域、无 transition 的定义时，才返回可展示为简写的 tier。
- `BuildStorageClassForTable(table, settings)`：选择第一个无作用域定义作为表默认值；没有该定义时使用 `STANDARD`，并复制 transition 向量，随后记录背景日志。
- `range_value`、`compare_range`：处理 RANGE 分区边界。前者先尝试有符号/无符号整数解析，再通过 expression 解析和求值常量表达式；后者处理 `MAXVALUE`、RANGE COLUMNS 的列类型/字符集/排序规则比较，以及普通 RANGE 的有符号性。
- `BuildStorageClassForPartitions(partitions, table, settings)`：验证作用域与分区类型兼容性，按 `names_in`、`less_than`、`values_in` 匹配每个分区，并写入 tier 与 transition。
- `rebuild_partitions`：从 `TableInfo.EngineAttribute` 取设置并重建现有分区；无分区或 `PartitionTypeNone` 时保持不变。
- `normalize_partition_definitions`：对非 COLUMNS 的 RANGE/LIST 边界表达式求值并写回克隆后的 definitions；关键字和 `NULL` 不求值，成功后才整体替换，避免部分更新。
- `CheckAndUpdateAddedPartitionDefinitions`：将新增定义拼到原定义后构造最终视图，依次做归一化、最终约束检查、storage-class 重建，再按 `offset` 回填新增片段。
- `check_final_definitions`：检查分区名唯一，并对单列 RANGE 边界要求严格递增。

## 执行流程

建表路径中，`pkg/ddl/create_table.rs` 先调用 `GetEngineAttributeFromStorageClassTableOptions` 取得统一的 attribute 字符串，再由 `handle_create` 保存原始 attribute、解析 settings、派生表默认 storage class；分区 AST 转为元数据后，`normalize_partition_definitions` 先把表达式边界变成规范值，`rebuild_partitions` 再按规范边界应用分区作用域。

JSON 解析的内部顺序是：识别顶层形态 → 使用 `StrictDef`/`StrictRule` 解码 → 拒绝空定义 → `normalize` 大小写 → 检查 tier、transition 和作用域互斥 → 返回 `StorageClassSettings`。因此调用者得到的名称和 tier 已规范化，可以直接与 `CIStr.L` 及规范分区值比较。

分区派生先对每个定义做结构适用性检查：HASH/KEY 不接受分区作用域；`less_than` 只接受单列 RANGE；`values_in` 只接受单列 LIST。之后对每个分区按 settings 中的顺序查找首个作用域命中项：名称采用规范小写比较，RANGE 使用边界“小于等于”判定，LIST 对单值行做引号剥离或关键字大小写无关比较。若无命中，使用第一个无作用域默认项；仍无默认项则写入 `STANDARD` 和空 transitions。

ADD/REORGANIZE 的安全路径不是直接给新增定义赋值。`CheckAndUpdateAddedPartitionDefinitions` 构造“旧定义 + 新定义”的最终表视图，先统一常量表达式，再验证重复名称和 RANGE 严格递增，随后重新计算全部分区的 storage class，最后由 `update_checked_definitions` 按经检查的范围复制回调用者。越界或整数加法溢出均返回错误。

## 数据与状态

输入状态主要来自 `model::StorageClassSettings`、`StorageClassDef`、`StorageClassTransitRule`、`TableInfo`、`PartitionInfo` 和 `PartitionDefinition`（定义在 `pkg/meta/model/engine_attribute.rs` 及表元数据模块）。持久化相关字段包括 `TableInfo.EngineAttribute`、`TableInfo.StorageClassTier`、`TableInfo.StorageClassTransitions`，以及每个分区对应的 tier/transitions。

定义的作用域有四种语义：无作用域（表/分区默认）、`names_in`、`less_than`、`values_in`。同一定义最多只能选择后三者之一。多个定义可以同时存在，且顺序具有语义：分区使用第一个命中的有作用域定义，默认值使用第一个无作用域定义。

实现写入前普遍采用克隆：`rebuild_partitions` 克隆 definitions，`normalize_partition_definitions` 克隆 definitions，`CheckAndUpdateAddedPartitionDefinitions` 克隆表和新增 PartitionInfo。只有所有检查成功后才替换目标字段。transition 向量也通过 `clone` 写入表或分区；`pkg/ddl/storage_class_test.rs` 验证修改一个分区的 transition 不会别名影响另一个分区。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 文件使用关系及仓库调用点共同确认：

- `pkg/ddl/create_table.rs` 调用 `GetEngineAttributeFromStorageClassTableOptions`、`handle_create`、`normalize_partition_definitions`、`rebuild_partitions`，组成 CREATE TABLE 的主接线。
- `pkg/ddl/persistent_actions.rs` 调用 `get_settings`、`BuildStorageClassForTable`、`rebuild_partitions`，在持久化 ALTER 动作中更新规范表元数据，并把变更交给 storage-class transition 模块分阶段记录。
- `pkg/ddl/schematracker/dm_tracker.rs` 调用选项转换、`handle_create`、`rebuild_partitions` 和 `CheckAndUpdateAddedPartitionDefinitions`，使离线 schema tracker 与 DDL 规则一致。
- `pkg/ddl/lib.rs` 公开模块，并通过独立的 `storage_class_test.rs` 测试模块覆盖实现；测试没有内嵌在生产源文件中。

下游依赖与 `pkg/ddl/Cargo.toml` 一致：`astersql-meta-model` 提供元数据类型与 engine-attribute 解析；`astersql-parser-ast` 提供 table option/alter spec；`astersql-expression`、`astersql-expression-exprstatic`、`astersql-util-chunk` 用于分区常量表达式、类型与排序规则比较；`astersql-config` 提供功能开关；`astersql-util-dbterror` 形成兼容错误；`astersql-util-logutil` 记录派生结果；`serde`/`serde_json` 执行严格 JSON 解码。

RustCodeGraph 对 `BuildStorageClassSettingsFromJSON` 的 callee 查询确认其直接连接 `invalid`、`normalize`、`defs` 及 `StorageClassSettings` 构造；当前索引没有为多数跨模块 Rust 调用返回 callers，因此调用者部分以索引报告的文件使用关系和精确源码引用补足，并不把缺失图边解释为“没有调用者”。

## 错误处理与边界

错误最终以字符串跨越本模块边界，但关键类别保留数据库错误文本：非法定义使用 `ErrStorageClassInvalidSpec`，非法 engine-attribute 格式使用 `ErrEngineAttributeInvalidFormat`，功能关闭使用 `ErrGeneralUnsupportedDDL`，attribute 不含 `storage_class` 时 table-option 校验使用 `ErrUnsupportedEngineAttribute`。

JSON 边界包括：未知字段拒绝；尾随第二个 JSON 值拒绝；顶层或数组元素 `null` 最终因空 tier/空定义被拒绝；字段名大小写不敏感；重复字段后值覆盖前值；负数不能解码为 `u64`；`after_days`/`after_seconds` 的 `null` 取零值。错误消息中的原始定义最多保留前 192 个字符，限制异常输入对错误文本的放大。

tier 只能是 `STANDARD` 或 `IA`。transition 只能从 `STANDARD` 到 `IA`、恰好一条且总秒数大于零。`TotalSeconds` 的计算由模型类型提供；本文件按其结果验证，不自行做溢出检查。

RANGE COLUMNS 比较要求能找到首个分区列，并使用列类型、字符集和 collation 构造比较表达式；普通 RANGE 根据分区表达式的 unsigned flag 选择 `u64` 或 `i64`。无法解析、求值为 NULL、列缺失、边界不递增、分区名重复、作用域与分区类型不兼容都会停止更新并返回错误。这里只检查本文件明确承担的约束，其他完整分区约束仍由分区 DDL 模块负责。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、锁、事务或长期缓存。除读取进程级 `enable_storage_class` 配置外，函数只操作传入值和局部克隆；因此并发协调由调用者和 DDL job 框架负责。

元数据更新采用“克隆—校验—整体替换”的生命周期，降低失败时留下半更新状态的风险。表达式上下文通常由调用者提供；`compare_range` 和 `normalize_checked_partitions` 在无需会话状态的路径创建静态 expression context。日志记录使用 `BgLogger`，但本模块不管理 logger 生命周期。

storage-class transition 的持续状态、重试、取消、拓扑观测和历史记录不在本文件内；这些属于 `pkg/ddl/storage_class_transition.rs` 和调用它的持久化动作。扩展本文件时不得把后台迁移或持久化状态直接塞入这些纯元数据 helper。

## 与 Go 版本的对应关系

主对照文件是 `pkg/ddl/storage_class.go`。Rust 的 `BuildStorageClassSettingsFromJSON` 对应 Go 同名函数及 `decodeStorageClassDef`、`normalizeStorageClassDefs`、`checkStorageClassDef`；`StrictDef`/`StrictRule` 是 Rust 为复刻 Go `encoding/json` 行为而增加的局部机制。Rust 测试专门验证大小写字段、重复字段、`null` 零值、未知字段、负数和 transition 边界。

Rust 的 table-option、SHOW CREATE、表级/分区级派生函数与 Go 同名函数保持同样的主要语义。`compare_range` 汇合了 Go 的 `compareRangePartitionValues`、RANGE COLUMNS 比较、数值比较和“小于等于”helper；`values_equal` 对应 Go `partitionValueEquals`。

Rust 还包含 Go 同路径文件中没有的局部接线 helper：`CheckStorageClassAdmission` 的 Go 对照入口位于 `pkg/ddl/engine_attribute.go`；`handle_create`、`rebuild_partitions`、`normalize_partition_definitions`、`CheckAndUpdateAddedPartitionDefinitions`、`check_final_definitions` 把 Go 分散在建表/分区流程中的必要接线集中为可复用函数。这些不是简化版替代：它们明确承担最终分区视图规范化和回填，相关 Rust 测试与 Go 的 `storage_class_partition_test.go`、`storage_class_admission_test.go` 对齐。

已观察到的接口层差异包括：Go 多用指针、`(value, found, error)` 和结构化 `error`，Rust 使用引用、`Option` 与 `Result<_, String>`；Go 的日志通过 zap 字段记录，Rust 使用 `LogField`。文档没有据此推断运行时行为差异。

## 扩展指南

新增 JSON 字段时，需要同时更新模型类型、`StrictDef` 或 `StrictRule` 的字段分派及 `unknown_field` 白名单、`normalize`/约束逻辑和 `pkg/ddl/storage_class_test.rs`；还应核对 `pkg/ddl/storage_class.go` 与 `pkg/ddl/storage_class_test.go`，避免 Rust 接受集合、默认值或大小写规则偏离 Go。

新增 tier 或 transition 类型时，至少修改 `check_tier`、`normalize`、表/分区选择逻辑、展示简化条件，并检查 `pkg/ddl/storage_class_transition.rs` 对物理迁移方向和状态的假设。若需要持久化迁移，该行为必须继续走 DDL job/持久化动作，而不是从本文件直接访问存储。

新增分区作用域时，最合适的接入点是 `normalize` 的互斥校验、`BuildStorageClassForPartitions` 的适用性验证与匹配分支，以及新增/重组分区的最终视图路径。同步测试应放在独立的 `pkg/ddl/storage_class_test.rs`；涉及完整 DDL SQL 接线时还应扩展相邻的 Rust 建表/分区测试，并对照 Go 的 `storage_class_partition_test.go`。应覆盖大小写、引号、`NULL`/`MAXVALUE`、有符号/无符号边界、RANGE COLUMNS collation、多定义优先级和失败不部分写入。

兼容性风险主要是既有 JSON 接受集合或错误文案变化；正确性风险集中于作用域优先级、常量表达式规范化和 RANGE 比较；性能风险集中于对每个分区逐定义扫描以及反复解析表达式。当前复杂度近似为“分区数 × 有作用域定义数”，修改时不要在内层额外引入存储访问或不必要的全表克隆。

## 验证依据

- RustCodeGraph：`rustcodegraph status` 确认索引含 11,467 个文件；`explore 'pkg/ddl/storage_class.rs ...'` 和 `node --file pkg/ddl/storage_class.rs` 读取目标全貌；`query StorageClass --kind function/struct` 核对符号；`callers`/`callees` 查询用于检查调用边，其中 `BuildStorageClassSettingsFromJSON` 的 Rust callees 明确包含 `invalid`、`normalize`、`defs` 和模型构造。
- 生产源码：`pkg/ddl/storage_class.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/create_table.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/schematracker/dm_tracker.rs`、`pkg/meta/model/engine_attribute.rs`。
- crate 边界：`pkg/ddl/Cargo.toml`，核对了 `astersql-ddl` 包名以及 config、expression、meta-model、parser-ast、chunk、dbterror、logutil、serde/serde_json 依赖。
- Go 对照：`pkg/ddl/storage_class.go`、`pkg/ddl/engine_attribute.go`；相关回归测试为 `pkg/ddl/storage_class_test.go`、`pkg/ddl/storage_class_admission_test.go`、`pkg/ddl/storage_class_partition_test.go`。
- Rust 独立测试：`pkg/ddl/storage_class_test.rs` 覆盖准入、严格 JSON/Go 零值语义、选项冲突、SHOW CREATE 简化、表/分区默认与作用域、规范化 ADD/REORGANIZE、RANGE COLUMNS、unsigned、MAXVALUE 和非法边界；`pkg/ddl/create_table_aster_unit_test.rs`、`pkg/ddl/schematracker/dm_tracker_test.rs` 覆盖直接接线。
- DDL 框架边界：已阅读 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`，并用代码验证本文件属于元数据校验/派生 helper，不自行承担 job 持久化、schema-state 迁移、reorg checkpoint、取消回滚、schema sync 或后台资源生命周期。
