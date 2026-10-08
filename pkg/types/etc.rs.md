# [`pkg/types/etc.rs`](./etc.rs)

## 文件定位

`pkg/types/etc.rs` 是 `pkg/types` 类型系统中的“杂项类型语义”实现：它把 MySQL 类型码、`FieldType`、Datum Kind、字符集/校对规则和公共错误类型连接成一组轻量查询及错误构造 API。源码本身没有 `mod` 声明；实际编译入口是 `pkg/types/internal/metadata/lib.rs` 的私有模块 `etc_defs`，该模块先把 `ast`、`mysql`、`FieldType`、Datum Kind 常量、`collate`、`errors`、`opcode` 等名字引入作用域，再以 `include!("../../etc.rs")` 纳入本文件，并通过 `pub use etc_defs::*` 对外再导出。

因此，这些函数的运行时归属是 `astersql-types-metadata` crate，而根 `astersql-types` crate 又在 `pkg/types/lib.rs` 以 `pub use types_group_4 as metadata` 暴露该子 crate。调用方通常通过依赖别名的 `metadata`/`types` 门面访问函数，而不是直接引用本文件模块。文件没有模块级常量、类型、trait、`impl` 或条件编译项；当前 28 个定义均为公开函数（包括小写的 `overflow`）。

## 核心职责

本文件承担四类职责，依据均可落到 `pkg/types/etc.rs` 中的具体符号：

1. 对 MySQL 类型码做集合分类：`IsTypeBlob`、`IsTypeChar`、`IsTypeVector`、`IsTypeVarchar`、`IsTypeUnspecified`、`IsTypePrefixable`、`IsTypeFractionable`、`IsTypeTime`、`IsTypeFloat`、`IsTypeInteger`、`IsTypeStoredAsInteger`、`IsTypeNumeric`、`IsTypeTemporal`。
2. 结合字段元数据判断字符串语义：`IsTypeBit`、`IsTemporalWithDate`、`IsBinaryStr`、`IsNonBinaryStr`、`IsString`、`NeedRestoredData`、`NeedRestoredDataWithCollate`。其中 restored data 判定直接影响新校对规则下索引值是否需要携带可恢复原值的信息。
3. 把内部编码转换为诊断或展示文本：`IsStringKind`、`KindStr`、`TypeStr`、`TypeToStr`。
4. 统一边界错误：`EOFAsNil` 归一化短读终止，`InvOp2` 构造二元操作类型不匹配错误，`overflow` 基于 `ErrOverflow` 构造带类型名和栈信息的溢出错误。

这些函数不拥有 SQL 请求、Datum 或索引状态；它们是类型系统底层的纯判定/格式化层，供表编解码、表达式推断和上层错误报告复用。

## 主要符号

- `IsTypeBlob(tp)`、`IsTypeChar(tp)`、`IsTypeVector(tp)` 直接委托 `ast`（即 `parser_types::types`）的同名定义，避免在 types metadata 层复制 parser 的基础分类。
- `IsTypeVarchar(tp)` 接受 `TypeVarString` 与 `TypeVarchar`；`IsTypeUnspecified(tp)` 只接受 `TypeUnspecified`；`IsTypeFloat(tp)` 只接受 `TypeFloat`。
- `IsTypePrefixable(tp)` 是 Blob 与 Char 集合的并集。它表达“允许索引前缀”的类型能力，不表示所有字符串类型都必然走同一种物理编码。
- `IsTypeFractionable(tp)` 接受 `Datetime`、`Duration`、`Timestamp`；`IsTypeTime(tp)` 接受 `Datetime`、`Date`、`Timestamp`；`IsTemporalWithDate(tp)` 只是后者的语义别名。`IsTypeTemporal(tp)` 的集合更宽，额外含 `Duration` 与 `NewDate`。
- `IsTypeInteger(tp)` 接受 Tiny/Short/Int24/Long/Longlong/Year。`IsTypeStoredAsInteger(tp)` 在这组类型之上再接受 Datetime/Date/Timestamp/Duration，但刻意不含虽可按整数表达、却不能按同样规则下推 TiFlash 的 Enum/Set；这一排除意图来自同路径 Go 实现的注释与 Rust 对照测试。
- `IsTypeNumeric(tp)` 接受 Bit、整数族（不含 Year）、NewDecimal、Float、Double；它是明确枚举，不会因未知类型码而误判为数值。
- `IsTypeBit(ft)` 从 `FieldType::GetType` 判断 Bit。`IsBinaryStr(ft)`/`IsNonBinaryStr(ft)` 同时要求类型属于 `IsString` 集合，并分别要求 collation 等于/不等于 `charset::CollationBin`；仅有 binary collation 不能把 Bit 变成字符串。
- `NeedRestoredData(ft)` 使用进程当前的 `collate::NewCollationEnabled()`，再委托 `NeedRestoredDataWithCollate(ft, useNewCollate)`。显式版本仅在新校对开启、字段为非 binary 字符串、满足“非 bin collation 或 Varchar”且 collation 不是 `utf8mb4_0900_bin` 时返回 `true`。
- `IsString(tp)` 是 Char、Blob、Varchar、Unspecified 的并集；`IsStringKind(kind)` 只识别 Datum 的 `KindString` 与 `KindBytes`，两者属于不同抽象层，不能互换使用。
- `KindStr(kind)` 用穷举 `match` 映射当前 20 个 Datum Kind；未知值返回空字符串。`TypeStr(tp)` 与 `TypeToStr(tp, charset)` 分别委托 parser AST 的类型名及“类型码 + charset”显示名转换，后者会产生诸如 Blob→text、Varchar→varbinary 的结果。
- `EOFAsNil(err)` 接受可空的 `errors::SharedError`。它沿错误 cause 尝试向下转型 `std::io::Error`，只把 `ErrorKind::UnexpectedEof` 变成 `None`，其余输入经 `errors::Trace` 透传。
- `InvOp2<X, Y>(x, y, o)` 要求两侧可 `Debug` 且为 `'static`，固定返回 `Err`；消息包含两侧调试值、`opcode::Op::String()` 和 Rust 静态类型名。返回类型保留 `Option<Box<dyn Any>>` 的占位成功形状，以对应 Go 的 `(any, error)`。
- `overflow(v, tp)` 用调试格式记录值、用 `TypeStr` 记录目标类型，并调用 `ErrOverflow.GenWithStack` 生成 MySQL 数据越界类错误。

## 执行流程

类型分类调用通常是单步常量集合判断：调用方传入 `u8` 类型码，函数通过相等比较、`matches!` 或 parser AST 委托立即返回布尔值。组合谓词按由基础到派生的顺序复用，例如 `IsTypePrefixable` 调用 Blob/Char 判定，`IsString` 聚合四个字符串类型集合，`IsTemporalWithDate` 委托 `IsTypeTime`。

restored data 主流程位于 `NeedRestoredDataWithCollate`：先检查调用方捕获的新校对开关；再通过 `IsNonBinaryStr` 排除非字符串和通用 binary collation；随后接受非 `_bin` 校对，或对 Varchar 保留特例；最后明确排除 `utf8mb4_0900_bin`。`NeedRestoredData` 只是用全局校对开关进入该流程。生产侧的 `pkg/tablecodec/tablecodec.rs::buildRestoredColumn` 遍历列时调用显式版本，跳过无需恢复数据的列，并对 bin collation 的恢复列改造成 unsigned longlong 来承载截断空格数；同文件的索引编码/解码路径也调用该判定。

名称转换流程没有缓存或分配型全局表：`KindStr` 在栈上匹配后创建一个 `String`；`TypeStr` 返回 parser 层静态字符串；`TypeToStr` 委托 parser 层根据 charset 生成 `String`。例如 `pkg/types/datum.rs::invalidConv` 同时调用 `KindStr(d.Kind())` 与 `TypeStr(tp)`，形成“源 Datum kind → 目标 MySQL type”的转换错误文本。

错误流程中，`EOFAsNil` 先观察 cause 链的首个可用 cause，成功识别 `UnexpectedEof` 即吞掉该终止信号，否则交给 `errors::Trace`。`InvOp2` 无成功分支，直接格式化并创建共享错误。`overflow` 把值和目标类型包装为 `ErrorArg` 后交给 `ErrOverflow` 模板产生带栈错误。

## 数据与状态

输入数据主要有三种：MySQL 类型码 `u8`、Datum Kind `u8`、借用的 `FieldType`。类型码与 Kind 都是开放的整数输入，因此各映射对未知值必须有稳定兜底：布尔谓词返回 `false`，`KindStr`/`TypeStr` 返回空名称（`TypeStr` 的行为由 parser 层定义）。`FieldType` 只被读取，涉及 `GetType()` 与 `GetCollate()`，不会在本文件中被修改。

文件自身不保存可变状态。唯一读取的外部动态状态是 `NeedRestoredData` 调用的 `collate::NewCollationEnabled()`；需要保证一次表/索引操作使用固定设置的调用方，应使用 `NeedRestoredDataWithCollate` 并传入自己捕获的布尔值。其余依赖是类型常量、Datum Kind 常量或静态错误模板。

返回值的所有权有意不同：多数分类函数返回 `bool`；`KindStr`/`TypeToStr` 返回自有 `String`；`TypeStr` 返回 `&'static str`；错误函数返回共享错误或可空共享错误。`InvOp2` 的成功类型虽存在于签名中，但实现永远不构造成功值。

## 依赖与调用关系

编译接线由 `pkg/types/internal/metadata/lib.rs::etc_defs` 完成。该模块提供：parser 类型能力 `ast_types as ast`、`FieldType`、`mysql`/`charset` 常量、`collate`、共享 `errors`、`ErrOverflow`、Datum Kind 常量和 `opcode`。`pkg/types/internal/metadata/Cargo.toml` 对应依赖为 `astersql-parser-types`、`astersql-util-collate`、`astersql-util-dbterror`，并通过本地路径依赖固定在同一 workspace；根 `pkg/types/Cargo.toml` 再以 `types-group-4 = { package = "astersql-types-metadata", ... }` 纳入它。

已核对的内部调用边包括：`IsTypePrefixable → IsTypeBlob/IsTypeChar`，`IsTemporalWithDate → IsTypeTime`，`IsBinaryStr/IsNonBinaryStr → IsString`，`NeedRestoredData → NeedRestoredDataWithCollate`，`NeedRestoredDataWithCollate → IsNonBinaryStr/IsTypeVarchar`，`IsString → IsTypeChar/IsTypeBlob/IsTypeVarchar/IsTypeUnspecified`，以及 `overflow → TypeStr`。

RustCodeGraph 报告本文件被 36 个文件使用；精确源码检索确认的代表性生产调用有：

- `pkg/tablecodec/tablecodec.rs`、`pkg/table/raw_row.rs`、`pkg/lightning/backend/kv/base.rs` 与 `pkg/session/runtime/system_session.rs` 使用 `NeedRestoredDataWithCollate`，服务于行/索引恢复数据布局和编码选择。
- `pkg/expression/planner_bridge.rs::ResolveType4Between` 使用 `IsTypeTemporal`，在 BETWEEN 公共比较类型原为字符串时决定是否提升为 datetime。
- `pkg/types/datum.rs::invalidConv` 使用 `KindStr` 与 `TypeStr` 生成类型转换错误。
- `pkg/session/runtime/mview_ddl.rs` 使用 parser 层同源的 `IsTypeBlob`；`pkg/types/field_type.rs` 与内部 field crate 还有相近分类实现。扩展类型集合时必须辨别调用的是 metadata、本地 field 还是 parser 门面，不能只改一个副本便假设全仓生效。

## 错误处理与边界

所有分类谓词都是总函数：未知 `u8` 不会报错或 panic，而是落入 `false`。`KindStr` 对未知 Kind 返回 `""`，`TypeStr`/`TypeToStr` 对未知类型同样由 parser 实现返回空名称；调用方若要求拒绝未知类型，必须在更高层显式校验，不能把空字符串当作有效类型名。

`NeedRestoredDataWithCollate` 的边界由 Rust 测试固定：开关为 `false` 时一律为假；通用 `binary` 字符集/校对为假；`utf8mb4_bin` 的定长 String 为假但 VarString 为真；`utf8mb4_0900_bin` 两者都为假；utf8mb4 非 bin 校对及当前列出的 gbk/gb18030 校对为真。该逻辑关系到索引兼容性和尾随空格恢复，新增 collation 不应仅凭名称直觉修改条件。

Go 的 `EOFAsNil` 以 `terror.ErrorEqual(err, io.EOF)` 判断 EOF；Rust 当前实现及测试明确使用 `std::io::ErrorKind::UnexpectedEof`。这不是字面一一映射，调用者必须按 Rust 已测试的错误种类理解。普通错误（以及 `None`）经 `errors::Trace` 保留；当前代码只对 cause 转型结果检查一种 kind，不泛化吞掉其他 I/O 错误。

`InvOp2` 的错误文本依赖 `Debug` 输出和 `type_name`，可用于诊断但不应作为稳定机器协议解析。`overflow` 使用 `ErrOverflow`，其 MySQL errno 对应由 `pkg/types/enum_4_aster_unit_test.rs` 验证为数据越界错误；错误模板或参数顺序变化会影响兼容文案。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接，也没有静态可变容器。所有借用仅持续一个函数调用；返回的 `String` 与 `SharedError` 由调用方拥有，`TypeStr` 返回的静态字符串无需释放。

并发相关的唯一注意点是校对开关读取的一致性：`NeedRestoredData` 在每次调用时读取 `NewCollationEnabled()`，而表编解码等长流程使用 `NeedRestoredDataWithCollate` 接受调用方保存的快照，可避免同一操作的多个步骤隐式读取不同配置。文件本身不负责配置同步，也不保证跨多次 `NeedRestoredData` 调用的快照一致。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/etc.go`，Rust 保留了相同函数集合和主要集合语义。Go 的三个函数变量 `IsTypeBlob`、`IsTypeChar`、`IsTypeVector` 在 Rust 中改成委托函数；Go 的 `kind2Str` map 在 Rust 中改为 `KindStr` 的穷举 `match`，未知 key 仍得到空字符串；Go 的 `TypeStr`/`TypeToStr` 函数变量在 Rust 中也是委托函数。`InvOp2` 用 Rust 泛型、`Debug` 与 `type_name` 模拟 Go `%v`/`%T`，成功占位从 Go `any` 映射成 `Option<Box<dyn Any>>`。

类型集合与 restored data 条件和 Go 当前实现一致，包括 `IsTypeStoredAsInteger` 不含 Enum/Set、`IsTypeNumeric` 不含 Year、Varchar 在普通 `_bin` 校对下的恢复数据特例，以及 `utf8mb4_0900_bin` 排除项。Rust `NeedRestoredDataWithCollate` 的显式开关也对应 Go 同名函数，使编码/解码路径复用一次捕获的校对模式。

已确认的差异是 EOF 表示：Go 测试传入 `io.EOF`，Rust `pkg/types/etc_test.rs::TestEOFAsNil` 构造 `UnexpectedEof`。另外 Rust 的 `pkg/types/etc_test.rs::TestIsNonBinaryStr` 直接调用目标函数，修正了 Go 同名测试中实际误调用 `IsBinaryStr` 的覆盖缺口。Rust 独立测试还比 Go 同路径测试更明确覆盖 `NeedRestoredData` 的 charset/collation 矩阵；`pkg/types/enum_4_aster_unit_test.rs` 则验证更完整的分类、错误文案和 errno 对应。

## 扩展指南

新增 MySQL 类型时，先确认权威类型码与 parser 分类是否应扩展，再逐一审查 `IsTypeInteger`、`IsTypeStoredAsInteger`、`IsTypeNumeric`、`IsString`、`IsTypeTemporal` 等集合；不要因为物理存储相似就同时加入所有集合。若 parser 的 Blob/Char/Vector 或类型名映射已负责该语义，应修改权威 parser 实现并验证本文件委托结果，而不是在委托后叠加不一致判断。

新增 Datum Kind 时，需要同步 `pkg/types/internal/metadata/lib.rs` 的 Kind 常量来源/再导出、`KindStr` 映射及 Datum 的编码、显示与转换调用方。未知值返回空字符串是现有兼容边界；若要改成错误返回，会改变公开签名并波及 `pkg/types/datum.rs::invalidConv` 等调用者。

新增或调整 collation 时，应优先修改 `NeedRestoredDataWithCollate`，并同步独立 Rust 测试 `pkg/types/etc_test.rs::TestNeedRestoredData` 与 `pkg/types/enum_4_aster_unit_test.rs::type_helpers_match_go_classification_and_restored_data_rules`；还要核对 `pkg/tablecodec/tablecodec.rs` 的 restored column 编解码契约以及 `pkg/table/raw_row.rs` 等直接调用点。这里的错误会带来索引兼容性或数据还原风险，不能只验证布尔函数本身。

修改错误行为时，同步 `pkg/types/etc_test.rs::TestEOFAsNil` 和 `pkg/types/enum_4_aster_unit_test.rs::error_helpers_preserve_eof_invalid_operation_and_mysql_codes`。测试逻辑必须继续放在独立测试文件，不嵌入 `etc.rs`。若追求 Go 对齐，需明确记录 `io.EOF` 与 Rust `UnexpectedEof` 的语义选择。性能方面，这些函数处于编码和类型推断热路径，应维持无锁、常量时间判定；避免在分类函数中引入分配，`KindStr`/错误路径以外尤其如此。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/types/etc.rs` 找到目标，`node --file pkg/types/etc.rs --offset 1 --limit 280` 读取了 234 行及 28 个符号，并报告 36 个使用文件。
- RustCodeGraph 符号查询：查询了 `NeedRestoredDataWithCollate`、`EOFAsNil`、`InvOp2`、`IsTypeStoredAsInteger`、`KindStr`；图中确认本文件内部调用边及 Go/Rust 同名定义。限定名 callers 输出存在多定义混合，因此上游调用点另用精确 Rust 源码检索复核，未把歧义输出当作单一调用结论。
- 编译与 crate 边界：读取 `pkg/types/internal/metadata/lib.rs`、`pkg/types/internal/metadata/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`，确认 `include!`、再导出和 workspace 路径依赖链；`pkg/types` 下不存在 `doc.go`，因此无更近的包契约文件可读。
- Go 对照：完整读取 `pkg/types/etc.go`，并读取 `pkg/types/etc_test.go` 中与本文件相关的分类、类型名、EOF、时间/字符串边界测试。
- Rust 测试：完整读取 `pkg/types/etc_test.rs`；另读取 `pkg/types/enum_4_aster_unit_test.rs` 中 `type_helpers_match_go_classification_and_restored_data_rules` 与 `error_helpers_preserve_eof_invalid_operation_and_mysql_codes`。
- 生产调用证据：读取 `pkg/tablecodec/tablecodec.rs::buildRestoredColumn`、`pkg/expression/planner_bridge.rs::ResolveType4Between`、`pkg/types/datum.rs::invalidConv`，并精确检索 table/raw-row/lightning/session 等直接调用点。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工检查只新增本文档、未修改 Rust/Go/Cargo/`plan.md`，且文中没有把未验证设计写成已支持事实。
