# `pkg/ddl/label/attributes.rs`

## 文件定位

`attributes.rs` 位于 `astersql-ddl-label` crate，负责 DDL Label Rule 进入 PD 规则模型之前的属性文本处理。crate 入口 `pkg/ddl/label/lib.rs` 通过 `pub mod attributes` 和 `pub use attributes::*` 暴露本文件 API；`pkg/ddl/label/Cargo.toml` 将该 crate 定义为 `astersql-ddl-label`，默认使用 Classic 内核语义，并通过 `nextgen` feature 联动 `kerneltype-dependency/nextgen`。

它不是 DDL job 执行器，也不负责 job 持久化、schema state 迁移、reorg、回滚或 schema version 同步。它是 Label Rule 构造链上的纯内存辅助层：`pkg/ddl/label/rule.rs` 中的 `Rule::ApplyAttributesSpec` 先把 AST 的 attributes 字段按 YAML 字符串数组解析，再调用本文件的 `NewLabels` 生成 `pd::RegionLabel`。后续规则 ID、内部标签和 key range 的组装由 `rule.rs` 完成。

## 核心职责

- `NewLabel` 严格解析一条 `key=value`：必须且只能有一个 `=`，并裁剪 key/value 两侧空白；空 key、空 value、缺少等号或多个等号均报错。
- `CompatibleWith` 把两条标签的关系归为三态：键不同为兼容、键和值都相同为重复、同键不同值为冲突。
- `Add` 在保持输入顺序的前提下向 `Vec<RegionLabel>` 添加标签：重复项静默忽略，冲突项报错且不修改集合，其余项追加到末尾。
- `NewLabels` 串联解析与添加，因此批量输入会在解析阶段拒绝非法格式，在合并阶段去重并拒绝同键异值。
- `RestoreRegionLabel` 和 `RestoreRegionLabels` 生成面向 SQL attributes 展示的文本；批量恢复会隐藏系统内部注入的 `db`、`table`、`partition` 标签，并在 NextGen 模式下额外隐藏 `keyspace`。

这些职责可由 `attributes.rs` 的六个公开函数、`AttributesCompatibility` 枚举以及四个内部键名常量直接复核。

## 主要符号

- `keyspaceKey`、`dbKey`、`tableKey`、`partitionKey`：`pub(crate)` 常量，既供 `RestoreRegionLabels` 判断可见性，也被 `pkg/ddl/label/rule.rs` 用于注入系统标签。它们不是 crate 外 API。
- `AttributesCompatibility`：公开的无数据枚举，包含 `AttributesCompatible`、`AttributesIncompatible`、`AttributesDuplicated`。它只表达比较结果，不持有标签。
- `NewLabel(attr: &str) -> Result<pd::RegionLabel, Error>`：公开解析入口。成功时只设置 `RegionLabel.Key` 与 `RegionLabel.Value`，`TTL`、`StartAt` 保持 `Default` 值。
- `RestoreRegionLabel(l: &pd::RegionLabel) -> String`：把当前字段直接拼成 `key=value`，不加引号、不转义，也不校验空字段。
- `CompatibleWith(l, o) -> AttributesCompatibility`：对 key/value 做区分大小写的精确字符串比较；TTL 和 StartAt 不参与兼容性判断。
- `NewLabels(attrs: Vec<String>) -> Result<Vec<pd::RegionLabel>, Error>`：预分配与输入等长的容量，按输入顺序调用 `NewLabel` 和 `Add`。
- `RestoreRegionLabels(labels: &[pd::RegionLabel]) -> String`：过滤内部标签，把其余标签逐个格式化为带双引号的 `"key=value"`，以逗号连接且不添加额外空格。
- `Add(labels: &mut Vec<pd::RegionLabel>, label: pd::RegionLabel) -> Result<(), Error>`：线性扫描现有集合，遇到第一个重复或冲突结果即返回；只有扫描结束仍兼容时才执行 `push`。

本文件没有 trait、impl、宏或条件编译项；NextGen 分支由运行时调用 `kerneltype::IsNextGen()` 决定，具体构建 feature 在 `pkg/ddl/label/Cargo.toml` 中声明。

## 执行流程

生产主链可概括为：AST `AttributesSpec` → `Rule::ApplyAttributesSpec`（`rule.rs`）→ YAML 得到 `Vec<String>` → `NewLabels` → 对每项执行 `NewLabel` → `Add` → 写入 `Rule.Labels`。

单项解析时，`NewLabel` 使用 `split('=')` 收集全部片段。片段数不是 2 时立即返回 `Error::InvalidAttributesFormat`；随后分别 `trim` key 与 value，任一为空也返回同一错误；全部通过后构造标签。因此 `a=b=c` 不会被理解为 key 为 `a`、value 为 `b=c`，而是非法输入。

批量合并时，`NewLabels` 保留已经成功添加的标签顺序。每个新标签进入 `Add` 后与已有元素逐一比较：键不同时继续；同键同值时直接成功返回但不追加；同键异值时先将新旧标签恢复为文本，再返回 `Error::ConflictingAttributes`。由于 `push` 位于扫描之后，冲突路径不会改变传入集合。

展示时，`RestoreRegionLabels` 按原顺序遍历，不排序。`db`、`table`、`partition` 总是跳过；`keyspace` 仅在 `kerneltype::IsNextGen()` 为真时跳过。`written` 计数只统计实际输出项，因此过滤发生在首项或中间项时都不会留下多余逗号。

## 数据与状态

主要数据对象是 `pkg/ddl/label/lib.rs` 定义的 `pd::RegionLabel { Key, Value, TTL, StartAt }`。本文件创建的标签只填充 Key/Value；兼容性和冲突规则也只查看这两个字段。这意味着 Key/Value 相同但 TTL/StartAt 不同的标签仍会被视为重复，这是当前实现的明确语义。

`NewLabels` 返回拥有所有权的 `Vec<RegionLabel>`，其中标签顺序等于首次出现顺序；完全相同的后续标签不占新位置。空输入成功返回空向量。`Add` 原地修改调用者提供的向量，但只在所有已有标签均兼容时追加；重复和错误路径均不追加。

文件没有全局可变状态、缓存、持久化状态或隐式事务。四个键名常量是静态只读字符串；`AttributesCompatibility` 是一次比较的值类型；所有 `String` 与 `Vec` 都归当前调用栈所有。

## 依赖与调用关系

直接下游依赖只有 crate 内部接口：`crate::pd::RegionLabel` 提供标签数据结构，`crate::errors::Error` 提供 `InvalidAttributesFormat` 与 `ConflictingAttributes`，`crate::kerneltype::IsNextGen` 控制 keyspace 展示过滤。具体 serde、YAML 和 key-range 编码不在本文件内发生。

RustCodeGraph 对 `pkg/ddl/label/attributes.rs` 的索引列出八个符号，并显示直接使用文件包括 `pkg/ddl/label/rule.rs` 与相关测试。关键调用边是：`Rule::ApplyAttributesSpec → NewLabels`；`NewLabels → NewLabel`、`NewLabels → Add`；`Add → CompatibleWith`、`Add → RestoreRegionLabel`；`RestoreRegionLabels → RestoreRegionLabel`。其中真正的生产入口是 `rule.rs` 的 `ApplyAttributesSpec`，其余直接调用主要来自 `attributes_test.rs` 和 `migration_aster_unit_test.rs`。

crate 边界方面，`pkg/ddl/label/Cargo.toml` 声明 `kerneltype-dependency`，并通过 `nextgen` feature 传播内核类型选择；`pkg/ddl/Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/domain/infosync/Cargo.toml` 等依赖该 label crate。不过这些上层 crate 通常消费完整 Rule/Label Rule API，不等于都直接调用本文件函数。

## 错误处理与边界

`NewLabel` 的所有格式失败都保留原始输入到 `Error::InvalidAttributesFormat { attribute }`。`pkg/ddl/label/errors.rs` 将其显示为 `attributes should be in format 'key=value': <原输入>`。已验证的非法集合包括空串、无等号、空 key、空 value、多等号；空白裁剪后为空也会走相同分支。

`Add` 的冲突错误是 `Error::ConflictingAttributes { new_label, existing_label }`，显示顺序固定为“新标签在前、已有标签在后”。它返回遇到的第一个同键异值冲突，不汇总全部冲突。重复不是错误，而是幂等成功。

函数不转义 key/value 中的逗号、引号或其他字符；`RestoreRegionLabel` 只做直接拼接，`RestoreRegionLabels` 只包一层双引号。因此输入语法的上游约束很重要，不能把恢复函数当作通用安全序列化器。另一个边界是 `NewLabels` 接收拥有所有权的 `Vec<String>`，Rust 没有 Go `nil` slice 的独立状态；Go 的 nil 与空 slice 都映射为 Rust 空向量行为。

## 并发与资源生命周期

所有函数都是同步、无锁、无 I/O 的纯内存操作；没有线程、异步任务、通道、文件句柄、网络连接或数据库事务。只要调用者不给同一向量建立非法别名，Rust 的借用规则即可保证 `Add` 修改期间的独占访问。

时间复杂度方面，`NewLabel` 与单条恢复和字符串长度线性相关；`Add` 最多扫描现有标签一次；`NewLabels` 逐项调用 `Add`，最坏为标签数的二次复杂度。正常 attributes 列表很短，当前实现优先保持 Go 顺序与错误行为；若未来为了大集合改用 map，必须同时保留首次出现顺序、首冲突选择和错误文案顺序。

资源生命周期局限于临时分配：解析会构造 split 片段向量和新的 Key/Value 字符串，恢复会分配结果字符串。错误或重复路径由 Rust 自动释放临时值，不需要显式清理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/label/attributes.go`，Rust 保留了同名常量、类型和六个函数的控制流。`str::split('=').collect()` 对齐 Go `strings.Split(attr, "=")` 的“恰好两个片段”规则；`trim()` 对齐 `strings.TrimSpace`；三态枚举对齐 Go 底层为 `byte` 的 `AttributesCompatibility` 常量；线性 `Add` 与过滤恢复逻辑也逐分支对应。

表达形式上的差异包括：Go 使用指针与 slice，Rust 使用引用和 `Vec`；Go 通过 `fmt.Errorf` 包装哨兵错误，Rust 使用 `thiserror` 的结构化 `Error`；Go 的 `nil`/空 slice 在 Rust 中统一为空 `Vec`；Go 的 `pd.RegionLabel` 来自 PD client，Rust 对应结构定义在本 crate 的 `lib.rs` 并保持 JSON 字段形状。以上差异没有改变当前被测试的 Key/Value 行为。

`pkg/ddl/label/attributes_test.go` 与 `attributes_test.rs` 均验证解析裁剪、单条恢复、空/多项批量解析、重复去重、冲突以及内部标签过滤。Rust 的 `migration_aster_unit_test.rs` 进一步覆盖 Go 测试未显式列出的 `""`、`"key"`、`"=value"`、`"key="`、`"a=b=c"` 非法输入，并断言冲突不修改原集合及完整错误文案。

## 扩展指南

新增或改变属性语法时，首要修改点是 `NewLabel`，并应在独立文件 `pkg/ddl/label/attributes_test.rs` 增加合法、非法、空白与分隔符边界用例，同时核对 `attributes.go` 和 `attributes_test.go`，避免 Rust 单方面改变 Go 语义。不要把测试内嵌回生产源文件。

改变去重或冲突规则时，应同步审查 `CompatibleWith`、`Add` 和 `errors.rs`，并验证冲突前后向量不变、重复仍幂等、错误中新旧标签顺序稳定。若引入 TTL/StartAt 参与比较，这是兼容性行为变化，必须先明确 PD 与 Go 侧契约。

新增系统内部标签键时，需要同时考虑两处：`rule.rs` 是否注入该键，以及 `RestoreRegionLabels` 是否应隐藏该键。若规则依赖内核类型，还需同步 `Cargo.toml` feature 和 Classic/NextGen 两套测试期望。改变输出格式则要评估 SQL SHOW/恢复文本的兼容性，特别是顺序、引号、逗号和转义。

性能优化应保留当前可观察语义。用哈希结构替换线性扫描可能改变顺序或首个冲突；只有在真实大列表证据存在并补足回归测试后才适合进行。该文件本身不应承担 DDL job、PD 网络提交或 key-range 编码，这些分别属于 DDL 执行层、infosync/label manager 与 `rule.rs`/codec 边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点且目标目录已索引；`files --filter pkg/ddl/label` 确认 Rust/Go 源与测试文件；`node --file pkg/ddl/label/attributes.rs --offset 1 --limit 260` 返回完整 167 行源码及八个符号；`explore 'pkg/ddl/label/attributes.rs symbols callers callees label attributes'` 给出 `NewLabels`、`Add`、`RestoreRegionLabels` 与 `ApplyAttributesSpec` 的调用关系及测试使用点。
- 已读生产与边界文件：`pkg/ddl/label/attributes.rs`、`pkg/ddl/label/lib.rs`、`pkg/ddl/label/errors.rs`、`pkg/ddl/label/rule.rs`、`pkg/ddl/label/Cargo.toml`、`pkg/ddl/Cargo.toml`，以及 DDL 包契约 `pkg/ddl/doc.go`。
- 已读 Go 对照与测试：`pkg/ddl/label/attributes.go`、`pkg/ddl/label/attributes_test.go`。
- 已读 Rust 独立测试：`pkg/ddl/label/attributes_test.rs`、`pkg/ddl/label/migration_aster_unit_test.rs`；其中后者直接验证严格格式、去重、冲突不修改集合、错误文本和内核类型相关过滤。
- 本任务是纯文档分析，按任务要求不运行 Cargo；结构验证单独检查目标文件存在且恰有规定的十一个二级标题。
