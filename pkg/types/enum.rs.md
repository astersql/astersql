# `pkg/types/enum.rs`

## 文件定位

`pkg/types/enum.rs` 实现 MySQL `ENUM` 值的内存表示，以及“成员名 ↔ 从 1 开始的序号”的解析规则。它位于 `pkg/types` 源码目录，但实际由 `pkg/types/internal/metadata/lib.rs` 中的 `enum_defs` 通过 `include!("../../enum.rs")` 编译进 `astersql-types-metadata` crate；该模块随后 `pub use enum_defs::*`，而上层 `pkg/types/lib.rs` 又把该 crate 重导出为 `astersql_types::metadata`。因此调用方通常通过 `types::ParseEnum*` 或 `astersql_types::metadata::ParseEnum*` 使用这里的 API，而不是直接声明本文件为模块。

`pkg/types/Cargo.toml` 声明根 crate `astersql-types`，并以路径依赖 `types-group-4 = { package = "astersql-types-metadata", path = "internal/metadata" }` 连接上述实现。名称比较依赖 `astersql-util-collate`，错误包装依赖 types 错误设施；这些名称在 `enum_defs` 的 `use crate::{ErrTruncated, collate, errors}` 中被显式带入 `include!` 的作用域。

## 核心职责

本文件有三项职责：

1. 用 `Enum { Name, Value }` 同时保存 SQL 可见的成员文本和 MySQL 使用的成员序号。
2. 用 `ParseEnumName` 按指定 collation 比较成员名，用 `ParseEnumValue` 校验并解析序号；`ParseEnum` 组合二者并提供数值文本回退。
3. 对解析失败统一生成以 `ErrTruncated` 为 cause 的共享错误，使上层类型转换、表达式求值、行编解码等路径能够按 types 错误语义继续处理。

这里不保存列定义，也不决定有哪些成员；调用方从 `FieldType::GetElems()` 提供有序的 `&[String]`。成员顺序就是序号语义的一部分：第一个成员为 1，最后一个成员为 `elems.len()`。

## 主要符号

- `pub struct Enum { pub Name: String, pub Value: u64 }`：一个已解析的 ENUM 值。派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`；默认值为“空名称、序号 0”，但正常的 `ParseEnumValue` 不会产生序号 0。
- `Enum::Copy(&self) -> Enum`：显式深拷贝 `Name`，按值复制 `Value`。`pkg/types/enum_4_aster_unit_test.rs` 通过指针不等验证字符串缓冲区互不共享。
- `Enum::String(&self) -> String`：克隆并返回成员名，保持 Go `String() string` 的拥有语义。
- `Enum::ToNumber(&self) -> f64`：把序号转换为数值上下文使用的 `f64`。
- `impl Display for Enum`：直接把 `Name` 写入 formatter，不额外分配返回字符串；显示结果与 `String()` 一致。
- `ParseEnum(elems, name, collation)`：公开组合入口。先尝试名称匹配，失败后尝试按 Go `strconv.ParseUint(name, 0, 64)` 风格解析数值文本，最后按序号取成员。
- `ParseEnumName(elems, name, collation)`：通过 `collate::GetCollator(collation).Compare(element, name)` 线性扫描，命中时返回列表中的原始成员文本以及 `index + 1`。
- `ParseEnumValue(elems, number)`：只接受闭区间 `1..=elems.len()`，用 `number - 1` 索引并克隆成员名。
- `enum_name_error`、`wrap_truncated`：内部错误构造器。前者形成包含输入名称和候选成员的诊断文本，后者以 `ErrTruncated` 为 cause 包装文本。
- `parse_uint_base_zero`：内部数值文本解析器，识别 `0x`/`0X`、`0b`/`0B`、`0o`/`0O` 和传统前导零八进制；允许合法位置的下划线分隔符，拒绝空串、正负号、非法/空数字段、末尾下划线及连续下划线。

本文件没有 trait、模块级常量或条件编译项；公开面由一个结构体、三个方法、`Display` 实现和三个解析函数组成，其余函数均为私有实现细节。

## 执行流程

`ParseEnum` 的主流程如下：

1. 调用 `ParseEnumName`。后者取得指定 collation 的 collator，按 `elems` 原顺序比较；第一个比较结果为 0 的成员胜出，并保留列表中原始拼写。
2. 若名称解析成功，立即返回，因而纯数字字符串若恰好也是一个成员名，会优先被当作名称，而不是序号。
3. 若名称解析失败，调用 `parse_uint_base_zero(name)`。它根据前缀选择 2、8、10 或 16 进制，去掉允许的下划线后调用 `u64::from_str_radix`；溢出或语法非法返回 `None`。
4. 数值解析成功时调用 `ParseEnumValue`。序号为 0 或大于成员数时返回边界错误，否则以 `number - 1` 读取成员。
5. 输入既不是成员名，也不是合法无符号数值文本时，`ParseEnum` 返回名称不存在错误。若文本可解析成数字但数字越界，则保留 `ParseEnumValue` 的“number ... overflow”错误，而不是改写成名称错误。

直接调用 `ParseEnumName` 的范围构造和表达式路径不会获得数值回退；直接调用 `ParseEnumValue` 的编解码路径也不会执行 collation 比较。这一区别是选择扩展入口时的重要契约。

## 数据与状态

`Enum` 是拥有数据的普通值对象：`Name` 拥有一个 `String`，`Value` 是 `u64`。函数只借用候选列表和输入文本，在成功返回时仅克隆被选中的成员；没有全局可变状态、缓存或内部引用生命周期。

重要不变量包括：

- 由 `ParseEnumName` 或 `ParseEnumValue` 成功构造的值满足 `1 <= Value <= elems.len()`，且 `Name == elems[Value - 1]`。
- 名称比较可能按 collation 忽略大小写或尾空格，但返回的 `Name` 始终是 `elems` 中的原始字符串，而不是调用者输入的规范化结果。`enum_test.rs` 对 `"A     "` 匹配 `"a"` 的用例证明这一点。
- `Enum::default()` 的 `Value == 0` 是上层用于空/零值的表示，不属于本文件解析函数接受的正常成员区间。`pkg/types/datum.rs::convertToMysqlEnum` 对已有 ENUM 且内部整数为 0 的情况会显式使用此默认值。
- `ToNumber` 使用 `u64 as f64`；对现实中的 ENUM 成员数量没有精度问题，但如果有人绕过解析器手工构造超大 `Value`，超过 `f64` 精确整数范围后不会保持逐整数精度。

## 依赖与调用关系

下游依赖：

- `collate::GetCollator` 和 `Collator::Compare` 决定 `ParseEnumName` 的等价关系；`pkg/util/collate/collate.rs` 中的 `GetCollator` 再委托给 `GetCollatorWithCollate`。
- `ErrTruncated` 在 `pkg/types/errors.rs` 由 `standard_error!(ErrTruncated, ClassTypes, WarnDataTruncated)` 定义；`errors::SharedError`、`errors::Wrap` 承载 cause 和上下文文本。
- Rust 标准库提供字符串所有权、`Display` 和 `u64::from_str_radix`。

RustCodeGraph 对 `pkg/types/enum.rs` 识别到 15 个使用文件。代表性的上游调用链包括：

- `pkg/types/datum.rs::convertToMysqlEnum`：字符串、字节、ENUM 和 SET 输入走 `ParseEnum`，其他数值类型先转无符号整数再走 `ParseEnumValue`，最后写入新的 Datum。
- `pkg/tablecodec/tablecodec.rs::Unflatten`、`pkg/util/rowcodec/decoder.rs` 与 `pkg/util/codec/codec.rs`：把存储层序号恢复为带名称的 ENUM，直接使用 `ParseEnumValue`。
- `pkg/expression/scalar_function.rs::Eval`：将表达式文本结果解析成 ENUM，使用组合入口 `ParseEnum`。
- `pkg/expression/chunk_executor.rs`、`pkg/expression/constant_propagation.rs`：根据值的物理形态分别选择名称或序号入口。
- `pkg/planner/core/operator/physicalop/physical_batch_point_get.rs` 和 `pkg/util/ranger/points.rs`：用 `ParseEnumName` 将谓词中的成员名映射到稳定序号，用于键/范围构造。
- `pkg/table/column.rs`：用 `ParseEnumValue(..., 1)` 构造首个 ENUM 成员的列值。

因此该文件处于“字段类型元数据 → 类型转换/表达式 → 行与键编解码”的共享边界；解析规则变化会同时影响 SQL 语义和存储键的一致性。

## 错误处理与边界

所有公开解析失败都返回 `errors::SharedError`。成员名不存在、数值文本非法、序号为 0、序号超过成员数、整数溢出最终都表现为以 `ErrTruncated` 为 cause 的错误，但诊断文本分为名称错误和数字边界错误。`wrap_truncated` 先克隆 `ErrTruncated` 建立 cause，再调用 `errors::Wrap(Some(cause), message)`；传入 `Some` 后的 `expect` 只断言包装器不会丢失已存在错误。

边界行为：空 `elems` 无法成功解析；重复或按 collation 等价的成员由线性扫描决定“最先者优先”；`ParseEnumValue` 在做减一索引前先拒绝 0，因此不会发生无符号下溢；上界检查也保证索引安全。数值回退拒绝 `+1`、`-1`、空前缀数字、尾下划线和连续下划线；`0x_2` 与 `0_2` 被接受，独立测试明确覆盖其 Go base-0 兼容意图。

调用者不得把“名称匹配失败”单独视为最终失败，因为 `ParseEnum` 会继续进行数值回退；若业务只允许名称，应调用 `ParseEnumName`。反之，从磁盘或协议得到的序号应调用 `ParseEnumValue`，避免把数字解释为同名枚举成员。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。每次解析只在当前调用栈运行；输入均为共享只读借用，因此同一成员列表可被并发调用者安全读取，前提是调用方本身以 Rust 类型系统允许的方式共享它。

资源成本主要是名称解析的 O(n) collation 比较，以及成功时一次成员字符串克隆。数值解析会在去除下划线时创建一个临时 `String`；错误路径还会通过 `elems.join(" ")` 生成候选列表文本。这里没有缓存，生命周期在函数返回后由 Rust 所有权自动结束。扩大成员规模或在热点循环中调用时，应评估线性扫描、克隆和错误文本拼接成本，而不是在本文件中引入未经全局语义验证的共享缓存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/enum.go`，测试对照是 `pkg/types/enum_test.go`。Rust 保留了 Go 的字段名、公开函数名和主流程：`Enum{Name, Value}`、深拷贝、字符串/数值表示、名称优先、`strconv.ParseUint(..., base=0, bitSize=64)` 风格回退、1 起始序号以及 `ErrTruncated` 包装。

实现语言差异包括：Go 的值接收者天然复制结构体，Rust 用 `&self` 加显式克隆；Go 通过 `fmt.Stringer` 暴露格式化，Rust 同时保留 Go 风格 `String()` 并实现标准 `Display`；Go 用 `error`，Rust 用 `Result<Enum, errors::SharedError>`。Go 的 `fmt.Sprintf("... %v", elems)` 显示为方括号列表，Rust 用 `elems.join(" ")` 得到对应的空格分隔内容。

`parse_uint_base_zero` 是 Rust 为复刻 `strconv.ParseUint` base-0 行为增加的私有适配层。`pkg/types/enum_test.rs` 在 Go 原测试基础上显式补充 `0x_2`、`0_2` 和 `+1`，`pkg/types/enum_4_aster_unit_test.rs` 还覆盖十六进制序号、上下界、错误文本和 `Copy` 深拷贝。当前直接 Go 测试验证默认、`utf8_unicode_ci`、`utf8_general_ci`、中文名称、数字回退和序号 0；Rust 两份独立测试合计覆盖这些语义并补足迁移边界。

## 扩展指南

- 修改名称等价规则时，应从 `ParseEnumName` 与 `astersql-util-collate` 的契约入手，并同步验证大小写、尾空格、Unicode 和重复等价成员；不要在 `ParseEnum` 中另做字符串规范化，否则直接调用名称入口与组合入口会分叉。
- 修改数值文本语法时，应集中调整 `parse_uint_base_zero`，逐项对照 Go `strconv.ParseUint(..., 0, 64)`，在 `pkg/types/enum_test.rs` 增加独立边界测试；需特别保护名称优先和 `ParseEnumValue` 的范围错误传播。
- 修改序号或零值语义时，应同时审查 `ParseEnumValue`、`pkg/types/datum.rs::convertToMysqlEnum`、table/row codec、ranger 与 planner 调用点。ENUM 序号会进入行数据和键编码，兼容性风险高于普通显示逻辑。
- 增加公开能力时，要在 `pkg/types/internal/metadata/lib.rs` 的 include/re-export 结构下确认符号可见性，并评估根 `astersql-types` 的 `metadata` 重导出；不要把测试内嵌到 `enum.rs`，应扩展同目录独立测试 `enum_test.rs`，迁移契约类用例可扩展 `enum_4_aster_unit_test.rs`。
- 性能优化应优先测量调用场景。若考虑索引或缓存，需要证明 collation 配置、成员顺序和重复等价成员的“首个命中”语义不变，并处理缓存所有权与并发生命周期。

## 验证依据

- 源码事实：`pkg/types/enum.rs`（`Enum`、`Copy`、`String`、`ToNumber`、`Display`、`ParseEnum`、`ParseEnumName`、`ParseEnumValue`、三个私有辅助函数）。
- 装配与依赖：`pkg/types/internal/metadata/lib.rs`（`enum_defs` 的 include 和重导出）、`pkg/types/internal/metadata/Cargo.toml`（metadata crate 依赖）、`pkg/types/lib.rs` 与 `pkg/types/Cargo.toml`（根 crate 的 metadata 重导出和路径依赖）、`pkg/types/errors.rs`（`ErrTruncated` 对应 `WarnDataTruncated`）。
- Go 对照：`pkg/types/enum.go`、`pkg/types/enum_test.go`。
- Rust 测试：`pkg/types/enum_test.rs`、`pkg/types/enum_4_aster_unit_test.rs`；补充调用用例见 `pkg/types/convert_test.rs` 和 `pkg/planner/core/tests/pointget/point_get_plan_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/types/enum.rs` 核对完整 152 行实现及 15 个使用文件；`node ParseEnum` 核对 `convertToMysqlEnum → ParseEnum → {ParseEnumName, parse_uint_base_zero, ParseEnumValue}` 与两组测试调用；`node GetCollator` 核对名称入口委托 `GetCollatorWithCollate`。
- 直接调用检索：`rg` 核对 tablecodec、rowcodec、codec、expression、planner、ranger、table 与 datum 等生产调用点。本文只做静态文档分析，按任务约束未运行 Cargo 或代码测试；结构验证命令在交付前单独执行。
