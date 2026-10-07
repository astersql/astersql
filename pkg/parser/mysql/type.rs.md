# `pkg/parser/mysql/type.rs`

## 文件定位

本文件属于 `astersql-parser-mysql` crate。crate 入口 `pkg/parser/mysql/lib.rs` 通过 `pub mod r#type` 公开该模块；使用原始标识符是因为 `type` 是 Rust 关键字。它位于 parser 的 MySQL 协议与列元数据基础层，集中定义字段类型编号、列属性位掩码、`MEDIUMINT` 数值边界，以及读取标志位的纯函数。

这里不定义完整的字段类型对象，也不负责 SQL 类型推导、值转换或协议编解码；上层 `FieldType`、planner、executor、DDL、table 和 codec 等模块把本文件的数值常量与标志位作为共同契约使用。`pkg/parser/mysql/Cargo.toml` 声明 crate 名为 `astersql-parser-mysql`、edition 为 2024；本文件只使用整数与按位运算，不直接使用该 manifest 中的任何外部依赖，也没有 feature、平台分支或条件编译项。

## 核心职责

1. 用 `u8` 常量固定 MySQL 二进制协议的字段类型编号，包括 `TypeTiny`、`TypeLong`、`TypeTimestamp`、字符串/BLOB 家族，以及 TiDB 扩展 `TypeTiDBVectorFloat32`。
2. 用 `usize` 的单比特常量表达列与表达式属性，例如非空、键、无符号、自增、二进制比较、JSON 转换和内部 DDL 状态。
3. 用 `HasFlag` 实现统一的非破坏性位检测，并用 16 个语义明确的 `Has*Flag` 函数封装常用标志。
4. 提供 `MaxUint24`、`MaxInt24`、`MinInt24`，让 `TypeInt24` 的有符号和无符号边界在 Rust 侧保持明确的 `i32` 表示。

该文件是常量和纯函数集合：没有 struct、enum、trait、impl、宏、可变全局状态、I/O 或运行时注册过程。

## 主要符号

- 类型编号 `TypeUnspecified` 至 `TypeBit` 使用连续值 `0..=16`；其中 `TypeDuration = 11` 对应 MySQL `TIME`，沿用 Go 为避免与 `Time` 类型冲突而采用的名称。
- 扩展类型编号 `TypeJSON = 0xf5`、`TypeNewDecimal = 0xf6`、`TypeEnum = 0xf7`、`TypeSet = 0xf8`、四个 BLOB 相关编号 `0xf9..=0xfc`、`TypeVarString = 0xfd`、`TypeString = 0xfe`、`TypeGeometry = 0xff`。`TypeTiDBVectorFloat32 = 0xe1` 是 TiDB 的 float32 向量扩展编号。
- 协议/列属性位 `NotNullFlag`、`PriKeyFlag`、`UniqueKeyFlag`、`MultipleKeyFlag`、`BlobFlag`、`UnsignedFlag`、`ZerofillFlag`、`BinaryFlag`、`EnumFlag`、`AutoIncrementFlag`、`TimestampFlag`、`SetFlag`、`NoDefaultValueFlag`、`OnUpdateNowFlag`、`PartKeyFlag`、`NumFlag` 占用第 0 至 15 位。
- 内部位 `GroupFlag`、`UniqueFlag`、`BinCmpFlag`、`ParseToJSONFlag`、`IsBooleanFlag`、`PreventNullInsertFlag`、`EnumSetAsIntFlag`、`DropColumnIndexFlag`、`GeneratedColumnFlag`、`UnderScoreCharsetFlag` 覆盖第 15 至 24 位。`GroupFlag` 与 `NumFlag` 有意共享第 15 位，不能把二者当成可独立共存的状态。
- `MaxUint24: i32 = (1 << 24) - 1`、`MaxInt24: i32 = (1 << 23) - 1`、`MinInt24: i32 = -(1 << 23)` 分别是 `16_777_215`、`8_388_607`、`-8_388_608`。
- `pub fn HasFlag(flag: usize, flagItem: usize) -> bool` 执行 `(flag & flagItem) > 0`。它实际接受任意掩码：只要两者有至少一个公共位就返回 `true`，并不要求 `flagItem` 恰好只有一个比特。
- 16 个专用包装器为 `HasDropColumnWithIndexFlag`、`HasNotNullFlag`、`HasNoDefaultValueFlag`、`HasAutoIncrementFlag`、`HasUnsignedFlag`、`HasZerofillFlag`、`HasBinaryFlag`、`HasPriKeyFlag`、`HasUniKeyFlag`、`HasMultipleKeyFlag`、`HasTimestampFlag`、`HasOnUpdateNowFlag`、`HasParseToJSONFlag`、`HasIsBooleanFlag`、`HasPreventNullInsertFlag`、`HasEnumSetAsIntFlag`；每个都把固定的单比特常量交给 `HasFlag`。

## 执行流程

类型编号没有本地执行流程：解析器、字段元数据或协议层选择一个 `Type*` 常量，将其保存在上层类型对象或按字节传输；本文件只规定编号，不验证编号是否适用于某个 SQL 值。

标志检测的主流程是：调用方持有组合后的 `usize` 掩码，调用语义化的 `Has*Flag`；包装器选择对应常量并调用 `HasFlag`；`HasFlag` 做按位与；结果非零即表示目标位已设置。函数不清除位、不规范化相互关系，也不检查例如“主键是否同时非空”之类的上层约束。

RustCodeGraph 显示这些 helper 已进入多个业务层。例如 `pkg/executor/typed_index_reader.rs::append_index_row` 与 `pkg/executor/typed_kv_scan.rs::append_decoded_row_for_table` 使用 `HasPriKeyFlag` 识别主键列；`pkg/server/pg_catalog.rs::native_column_type` 使用 `HasUnsignedFlag` 区分数值属性；`pkg/ddl/index.rs::set_global_index_version` 使用非空/防止空值插入相关标志。调用方依据检测结果选择业务分支，本文件本身不拥有这些流程。

## 数据与状态

全部数据均为编译期 `const`，函数参数按值传入，调用过程不分配内存。类型编号固定为 `u8`，与 Go `byte` 和协议单字节编号范围一致；标志位固定为 `usize`，对应 Go 的 `uint` 位掩码风格。当前最高使用第 24 位，因此受支持 Rust 平台的 `usize` 均可容纳现有标志。

关键不变量包括：协议类型编号不能随意改值；除明确复用的 `NumFlag`/`GroupFlag` 外，各标志应占不同的单比特；专用 helper 必须选择与函数名相同语义的常量；24 位边界必须保持 `[-2^23, 2^23-1]` 与无符号 `[0, 2^24-1]`。常量没有运行时可变状态，因此文件也不会保存列值、连接状态或解析上下文。

## 依赖与调用关系

- 模块入口：`pkg/parser/mysql/lib.rs` 的 `pub mod r#type` 使所有 `pub const` 和 `pub fn` 可由 `astersql_parser_mysql::r#type` 访问；同文件还以 `#[cfg(test)] mod type_test` 接入独立 Rust 测试。
- 下游依赖：16 个专用 `Has*Flag` 全部调用同文件 `HasFlag`；除此之外，本文件不调用仓库函数或外部 crate。
- 上游类型层：`pkg/parser/types/field_type.rs`、`pkg/types/field_type.rs` 等消费类型编号与 `HasUnsignedFlag`、`HasBinaryFlag`、`HasZerofillFlag`，将底层位契约接入字段描述、显示与类型兼容逻辑。
- 上游规划与执行层：RustCodeGraph 找到 `pkg/planner/core/operator/physicalop/base_physical_plan.rs::collect_scan_handles -> HasPriKeyFlag`、`pkg/executor/typed_index_reader.rs::append_index_row -> HasPriKeyFlag`、`pkg/executor/typed_kv_scan.rs::append_decoded_row_for_table -> HasPriKeyFlag` 等调用边。
- 上游 DDL、会话与服务层：图中可见 `pkg/ddl/index.rs::set_global_index_version`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/system_query.rs`、`pkg/server/pg_catalog.rs` 等读取相应标志。这说明本文件是跨子系统的低层兼容契约，而不是 parser 内部私有实现。
- RustCodeGraph 的 `HasFlag` callers 明确解析出本文件 16 个包装器；个别 `callees`/`callers` 查询同时混入同名 Go 符号，本文只把文件路径为 `.rs` 且能定位到实际调用点的结果作为 Rust 调用证据。

## 错误处理与边界

本文件没有 `Result`、`Option`、panic、日志或错误码。任何 `u8` 都能在类型编号层面存在；未知编号、类型组合是否合法、协议版本是否支持某个扩展，都由调用方处理。类似地，任意 `usize` 都可传给标志 helper，未知高位会被忽略，除非 `flagItem` 本身包含该位。

`HasFlag(0, x)` 和 `HasFlag(x, 0)` 都返回 `false`。若 `flagItem` 是多个位的组合，函数采用“任一位相交”语义，而不是“所有位均已设置”；新增需要全包含判断的 API 时不能直接复用这一契约。专用 helper 使用单比特常量，因此没有该歧义。

`NumFlag` 和 `GroupFlag` 数值相同，检测其中一个等价于检测另一个；这是与 Go 对齐的既有兼容事实。`usize` 与 Go `uint` 都是平台字宽整数，但将标志写入固定宽度协议或持久格式时，调用方必须显式转换并保证不截断。`MinInt24` 写成 `-(1 << 23)`，语义与 Go 的 `-1 << 23` 数值一致。

## 并发与资源生命周期

常量在编译期确定，所有 helper 都是无副作用纯函数，只读取按值传入的整数。多线程可并发调用而无需锁、原子量、channel、任务或事务；没有惰性初始化、缓存竞争和全局可变状态。

函数不创建堆对象、不持有借用、不打开网络或文件资源，也没有显式清理阶段。单次检测是常数时间 `O(1)`、常数空间 `O(1)`；类型和标志常量在整个程序生命周期内可用，不存在所有权转移或释放问题。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mysql/type.go`。Rust 保留了 Go 的全部类型编号、`TypeTiDBVectorFloat32`、第 0 至 24 位标志、`NumFlag`/`GroupFlag` 同位关系、三个 `TypeInt24` 边界，以及 16 个专用检测函数和通用 `HasFlag`。两边的检测条件均为按位与后大于零。

表示层面的主要差异是：Go 类型编号用 `byte`、Rust 用等宽的 `u8`；Go 标志用 `uint`、Rust 用 `usize`；Go 的无类型整型边界常量在 Rust 中显式标为 `i32`。Rust 的专用 helper 统一委托 `HasFlag`，而 Go 的专用函数直接重复按位表达式；对单比特常量而言行为一致。

独立测试 `pkg/parser/mysql/type_test.rs::test_flags` 对照 `pkg/parser/mysql/type_test.go::TestFlags`，保留相同断言顺序，包括重复验证 `HasNotNullFlag(NotNullFlag)`。两侧当前都覆盖 12 个基础 helper 的真值路径，没有在这两个文件中覆盖零值、组合掩码、错误常量、`HasDropColumnWithIndexFlag`、`HasParseToJSONFlag`、`HasIsBooleanFlag`、`HasPreventNullInsertFlag`、`HasEnumSetAsIntFlag` 或类型编号/边界常量；不能把现有测试解释为所有公开符号均已逐项验证。

## 扩展指南

新增或修改协议类型时，应先核对 MySQL/TiDB 的实际编号，再同步 `pkg/parser/mysql/type.go`、Rust 常量及消费该编号的类型/协议代码；不得为方便而重排已有值。新增标志应选择未占用位，明确它是协议可见属性还是内部状态，并检查固定宽度序列化边界；若有意复用已有位，必须像 `NumFlag`/`GroupFlag` 一样记录不可区分语义。

新增常用标志 helper 时，优先沿用“专用函数委托 `HasFlag`”的结构，并在独立测试文件 `pkg/parser/mysql/type_test.rs` 增加自身位为真、零值为假、无关位为假的断言；同步核对 Go 测试意图。Rust 单元测试不要放回 `type.rs`。若需求是“组合掩码全部存在”，应新增语义明确的实现与测试，而不是误用当前的任一位相交逻辑。

兼容性风险高于实现复杂度：类型编号改变会影响协议和跨组件类型解释；标志位碰撞会让元数据属性不可区分；更改 helper 语义会波及 planner、executor、DDL、table、server 与 codec。性能风险很低，当前所有访问均为常量读取或单次按位运算；扩展时应维持无分配和纯函数属性。

## 验证依据

- 源码与符号：RustCodeGraph `node --file pkg/parser/mysql/type.rs --offset 1 --limit 500` 显示完整 221 行，确认类型常量、标志常量、三个边界常量、16 个专用 helper 和 `HasFlag`，且没有条件编译、类型定义或 I/O。
- crate 边界：RustCodeGraph 读取 `pkg/parser/mysql/lib.rs`，确认 `pub mod r#type` 与 `#[cfg(test)] mod type_test`；`pkg/parser/mysql/Cargo.toml` 确认 crate 名、edition、入口和依赖。本文件没有直接导入项。
- 调用图：`rustcodegraph callers HasFlag --file pkg/parser/mysql/type.rs --json` 返回 16 个本文件包装器；针对 `HasNotNullFlag`、`HasUnsignedFlag`、`HasPriKeyFlag` 等查询并筛选 `.rs` 路径，定位到 `pkg/ddl/index.rs`、`pkg/executor/typed_index_reader.rs`、`pkg/executor/typed_kv_scan.rs`、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`、`pkg/server/pg_catalog.rs`、`pkg/session/runtime/*` 等调用者。
- Go 对照：`pkg/parser/mysql/type.go`；逐项核对数值、位位置、边界和 helper 语义。Rust 的 `usize`/`i32` 是显式类型适配，未改变当前常量数值。
- 测试依据：RustCodeGraph 读取 `pkg/parser/mysql/type_test.rs::test_flags`，并与 `pkg/parser/mysql/type_test.go::TestFlags` 对照；两者均是独立测试文件，覆盖范围及缺口已在文中明确列出。
- 本任务为纯文档分析，按任务约束不运行 Cargo。交付检查使用任务指定命令验证目标存在且恰有 11 个固定二级标题，并人工复核文档能够说明文件存在理由、执行方式、边界、真实调用关系与安全扩展位置。
