# `pkg/util/column-mapping/column.rs`

## 文件定位

本文件是 `astersql-util-column-mapping` crate 的核心实现，负责把按 schema/table pattern 配置的列映射规则应用到一行值上。crate 入口 `pkg/util/column-mapping/lib.rs` 通过 `pub mod column` 和 `pub use column::*` 再导出这里的公开 API；`pkg/util/column-mapping/Cargo.toml` 声明它只直接依赖同仓库的 `astersql-util-table-rule-selector`，根 `Cargo.toml` 将该 crate 列为 workspace member 并登记为 `facade_util_column_mapping`。

当前 Rust 仓库中，`NewMapping`、`Mapping::HandleRowValue`、`Mapping::HandleDDL` 和 `SetPartitionRule` 的实际调用只见于本 crate 的独立测试；未发现其他 Rust 生产文件依赖 `facade_util_column_mapping`。因此它是已经移植并具备单元测试的独立能力，但尚不能据此认定已接入 Rust 应用主链。Go 对照实现 `pkg/util/column-mapping/column.go` 则是现有 Go 列映射能力的实现。

## 核心职责

文件承担四组职责：

1. 用 `Rule`、`Expr` 和 `Value` 表达匹配条件、变换类型与可处理的行值。
2. 用 `Mapping` 管理 trie selector 中的规则，并缓存某个 schema/table 的匹配结果与列位置。
3. 实现 `addPrefix`、`addSuffix`、`partitionID` 三种值变换；其中 partition ID 把 instance/schema/table 标识编码进有符号 64 位整数的高位。
4. 校验、规范化、增删改规则，并为匹配失败、冲突规则、缺列、非法参数或非法值返回字符串错误。

`HandleDDL` 不是完整 DDL 重写器：无匹配规则时原样返回语句，有匹配规则时明确返回“not implemented”错误（`Mapping::HandleDDL`）。

## 主要符号

- `SetPartitionRule(instanceIDSize, schemaIDSize, tableIDSize)`：更新四个进程级原子变量 `instanceIDBitSize`、`schemaIDBitSize`、`tableIDBitSize`、`maxOriginID`。默认位宽为 4/7/8，原始 ID 默认占 44 位。
- `Expr`：内建 `AddPrefix`、`AddSuffix`、`PartitionID`，并用 `Other(String)` 保留未知配置值；`expr_handler` 只为前三者返回处理函数。
- `Value`：Rust 对 Go `[]any` 的显式枚举替代，支持若干有/无符号整数、字符串和 `Other`。partition ID 成功后，字符串输入仍输出十进制字符串，其他整数输入统一输出 `Value::Int`。
- `Rule`：保存 schema/table pattern、源/目标列、表达式、参数和建表语句。`Valid` 检查表达式、目标列及参数个数；`Adjust` 为三参数 partition 规则补空分隔符；`ToLower` 只归一化 schema/table pattern。
- `mappingInfo`：内部缓存值，保存 ignore 标记、源/目标列位置、命中的 `Arc<Rule>`，以及预计算的三段 partition ID。
- `Mapping`：组合 `Box<dyn Selector>`、大小写策略和 `RwLock<HashMap<String, Arc<mappingInfo>>>`。公开规则操作为 `AddRule`、`UpdateRule`、`RemoveRule`，公开处理入口为 `HandleRowValue` 与 `HandleDDL`。
- `NewMapping`：创建 trie selector 和空缓存，再逐条调用 `AddRule`；任一初始规则失败即返回带规则上下文的错误。
- `queryColumnInfo`：规则匹配、优先级选择、列位置计算与缓存写入的核心内部入口。
- `computePartitionID` / `computeID` / `parse_u64_with_bits`：解析规则参数和名称后缀，执行位宽检查并生成待 OR 合并的高位片段。

## 执行流程

构造阶段由 `NewMapping` 创建 `NewTrieSelector()`，然后 `AddRule` 进入 `addOrUpdateRule`：忽略 `None`，执行 `Rule::Valid`，在大小写不敏感模式下降低 pattern 大小写，调用 `Adjust` 补齐 partition 参数，清空缓存，再以 `SelectorInsert` 或 `SelectorReplace` 写入 selector。`RemoveRule` 同样按大小写策略处理 pattern、清缓存后调用 selector 的 `Remove`。

行处理从 `Mapping::HandleRowValue` 开始：

1. 按 `caseSensitive` 决定是否将传入的 schema/table 转成小写。
2. 调用 `queryColumnInfo`。后者以 `` `schema`.`table` `` 为 key 查缓存；未命中时调用 `Selector::Match`。
3. 无规则时缓存 `ignore = true` 并原样返回；有规则时区分 schema 级规则（空 `PatternTable`）与 table 级规则。存在 table 级匹配时优先使用它，否则使用 schema 级匹配；所选层级必须恰好一条规则。
4. 线性查找 `SourceColumn` 和 `TargetColumn`。源列允许不存在并以 `-1` 返回，目标列不存在则报错。partition 规则还会由 `computePartitionID` 预计算三段高位值。
5. 根据 `Expr` 调用处理函数。前/后缀处理要求目标值为字符串；partition 处理解析目标 ID、检查 `0 <= id < maxOriginID`，将四段按位 OR 后写回。
6. 返回改写后的整行，以及 `[sourcePosition, targetPosition]`；未匹配时位置为 `None`。

DDL 路径复用相同的匹配与列定位流程；只有未匹配时成功透传。匹配后会清缓存并返回未实现错误。

## 数据与状态

规则的长期存储位于 selector；`Mapping.cache` 只保存派生的 `mappingInfo`。缓存 key 只包含 schema/table，不包含 `columns`，所以相同表后续即使传入不同列清单，也会复用第一次计算的列位置；`column_test.rs::TestHandle` 明确覆盖了这个与 Go 一致的行为。规则增删改会调用 `resetCache`，避免旧规则结果继续生效。

partition ID 从 bit 63 以下开始依次放置 instance、schema、table 片段，最高符号位不用，剩余低位留给 origin ID。名称等于 prefix 时对应片段为 0；否则必须满足 `prefix + separator + 十进制后缀`。某段位宽为 0或相应参数为空时跳过该段。`Rule::Adjust` 保证经正常加规则路径进入的 partition 规则总有第四个分隔符参数。

`SetPartitionRule` 修改的是整个进程共享的位宽，而既有 `mappingInfo` 已经缓存了旧位宽计算出的片段。调用方若在已有 `Mapping` 存活期间修改位宽，必须使相关缓存失效；当前公开 API 不会替所有实例自动清缓存。

## 依赖与调用关系

直接下游只有 `crate::table_rule_selector` 再导出的 `Selector`、`Rule`、`NewTrieSelector`、`Insert` 和 `Replace`。主要内部调用边为：

- `NewMapping -> Mapping::AddRule -> Mapping::addOrUpdateRule -> Selector::Insert`。
- `Mapping::HandleRowValue -> Mapping::queryColumnInfo -> Selector::Match`，随后经 `expr_handler` 到 `addPrefix`、`addSuffix` 或 `partitionID`。
- `Mapping::queryColumnInfo -> computePartitionID -> computeID -> parse_u64_with_bits`。
- `Mapping::UpdateRule` 与 `Mapping::RemoveRule` 分别落到 selector 的替换和删除能力。

RustCodeGraph 将 `column.rs` 标为被许多文件“使用”，但精确符号查询与仓库文本检索没有找到本目录测试之外的 Rust 生产调用点；宽泛文件关联中混有同名 `column` 符号，不能作为应用接线证据。根 workspace 的 `facade_util_column_mapping` 也未被其他 `Cargo.toml` 引用。

## 错误处理与边界

本文件统一用 `ColumnResult<T> = Result<T, String>`，没有结构化错误类型。规则错误包括未知表达式、空目标列、前/后缀参数不为 1 个、partition 参数不为 3 或 4 个；匹配错误包括同层级规则不恰好为 1 条、selector 中保存了非 `Rule` 类型、目标列不存在；值错误包括目标不是字符串、ID 不是十进制数、名称前缀/分隔符不匹配、后缀超位宽，以及 origin ID 为负或达到上限。

需要特别注意以下边界：

- `SetPartitionRule` 不校验三个位宽是否非负、总和是否小于 63；非法配置可能在减法或移位处触发 panic，而不是返回 `Result`。
- `computePartitionID` 直接索引 `Arguments[0..=3]`。通过 `AddRule` 的规则会先校验并 `Adjust`，但测试或未来内部调用若绕过该路径，短参数会 panic；现有测试也用短参数确认该函数报错/失败边界。
- `Value::Uint64` 通过 `as i64` 转换；超过 `i64::MAX` 的值会变成负数并被 origin ID 范围检查拒绝。
- `RwLock::read/write` 使用 `expect`，锁中毒会 panic；`mappingInfo.rule` 的内部不变量也由 `expect` 维护。
- `HandleDDL` 的匹配分支始终失败；`CreateTableQuery` 当前仅存储，不参与执行。

## 并发与资源生命周期

四个位宽/上限变量使用 `AtomicI32`、`AtomicI64` 且全部采用 `SeqCst`，单次读写在线程间可见；测试通过 `PARTITION_RULE_TEST_LOCK` 串行修改全局规则。不过 `SetPartitionRule` 的四次 store 不是一个原子事务，并发查询可能观察到新旧位宽混合状态。生产调用若动态修改位宽，需要在外层提供停机点或互斥协议。

每个 `Mapping` 的缓存由 `RwLock` 保护，命中路径持读锁，填充和重置持写锁；缓存项和规则用 `Arc` 共享，离开 map 后仍可被当前调用安全持有。代码没有后台任务、通道、文件句柄或显式事务；锁 guard 依靠作用域自动释放。selector 的并发保证来自 `table_rule_selector` 实现，本文件没有额外包装，因此扩展规则热更新前应同时核对该依赖的线程安全契约。

## 与 Go 版本的对应关系

Rust 主要逐段对应 `pkg/util/column-mapping/column.go`：规则验证和调整、schema/table 大小写处理、table 级优先级、缓存 key、列位置、三种表达式以及 partition 位布局均保持一致。`pkg/util/column-mapping/column_test.rs` 对应 Go 的 `column_test.go`，覆盖 `Valid`、行处理与缓存、DDL 未实现、匹配/忽略、位宽、ID 解析、值类型和大小写；`migration_aster_unit_test.rs` 额外覆盖 selector pattern、table 优先级、更多整数形态以及 Update/Remove。

可见差异包括：Go 的 `Expr` 是字符串别名及全局 `Exprs` map，Rust 使用 enum 和 `expr_handler`；Go 接受 `[]any`，Rust 使用封闭的 `Value` enum；Go 返回 `*Mapping` 并允许 nil receiver 静默透传，Rust 返回拥有值且方法不能在空 receiver 上调用；Go 用 `errors` 包保留 NotFound/NotValid/NotSupported 分类，Rust 目前只有字符串；Go 的规则带 YAML/JSON/TOML tag，Rust `Rule` 未派生序列化能力，这些 tag 只在注释语义中出现。Rust 使用原子变量避免 Go 包级普通变量的数据竞争，但没有让多字段更新成为事务。

## 扩展指南

新增表达式时，应同时扩展 `Expr`、`Expr::as_str`、`expr_handler` 和具体处理函数，并更新 `Rule::Valid`/`Adjust` 的参数契约；测试应放在独立的 `column_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌进生产文件。若表达式需要新的 `Value` 形态，还要明确输入输出类型保持规则，并与 Go 的 `[]any` 行为逐项对照。

若实现 DDL 映射，入口是 `Mapping::HandleDDL`，需要决定如何使用 `CreateTableQuery`、返回哪些列位置，并补充匹配与不匹配的独立测试。若改变 selector 优先级或允许同表多表达式，修改点集中在 `queryColumnInfo`，同时要重新设计缓存值和冲突规则。

性能或正确性调整应优先处理两个现存契约：缓存 key 不含列布局，以及全局 partition 位宽与实例缓存不联动。改变任何一项都会偏离 Go 行为，必须先添加 Go/Rust 对照用例并评估调用者兼容性。把 crate 接入 Rust 生产链时，还需在消费方 `Cargo.toml` 引用 workspace dependency，并用真实行 schema 验证列位置和 `Value` 转换；当前没有生产接线可作为回归保障。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/util/column-mapping` 确认本模块的 Rust/Go 实现与测试集合。
- RustCodeGraph 源码与符号查询：读取 `column.rs` 全部 685 行；查询 `Mapping`、`NewMapping`、`SetPartitionRule`，并核对 `NewMapping -> AddRule/resetCache` 等内部调用边。宽泛 `explore` 结果存在大量同名 `column` 噪声，因此生产接线结论另以精确仓库检索复核。
- 读取的实现/边界文件：`pkg/util/column-mapping/column.rs`、`lib.rs`、`Cargo.toml`、根 `Cargo.toml`。
- 读取的对照与测试：`pkg/util/column-mapping/column.go`、`column_test.go`、`column_test.rs`、`migration_aster_unit_test.rs`。
- 精确检索：除本实现、本模块测试及 `lib.rs` 注释外，Rust 文件中未找到 `NewMapping`、`HandleRowValue`、`HandleDDL`、`SetPartitionRule` 的生产调用；除根 workspace 登记外，其他 Cargo manifest 未引用 `facade_util_column_mapping`。
- 按任务约束未运行 Cargo；最终仅执行固定十一章节的结构验证，并人工核对本说明没有把未接线或未实现能力描述为已支持的应用行为。
