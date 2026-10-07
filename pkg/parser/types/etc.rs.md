# `pkg/parser/types/etc.rs`

## 文件定位

`pkg/parser/types/etc.rs` 属于 Cargo 包 `astersql-parser-types`（`pkg/parser/types/Cargo.toml`），由 crate 根 `pkg/parser/types/lib.rs` 的 `types::etc` 模块通过 `include!("etc.rs")` 装入，并经 `pub use etc::*`、`pub use types::*` 对外再导出。它位于 parser 的字段类型基础层：不负责解析 SQL token，而是为 `FieldType` 等上层类型提供 MySQL/TiDB 类型码分类、名称转换、十进制存储参数和标准类型错误。

该文件的直接 Go 对照是 `pkg/parser/types/etc.go`。Rust 源文件已经带有 AsterSQL 处理标记，并保留了原有 ql BSD 与 PingCAP Apache License 注释。

## 核心职责

文件有四组职责，均以 `u8` MySQL 类型码为边界：

1. `IsTypeBlob`、`IsTypeChar`、`IsTypeVector` 对类型码做无状态分类，供字段类型显示、字符集输出和向量类型识别使用。
2. `type2Str`/`str2Type` 以及 `TypeStr`、`TypeToStr`、`StrToType` 维护类型码与规范名称的双向转换，并实现 binary 字符集下 `text -> blob`、`char -> binary` 的显示别名。
3. `dig2bytes`、`digitsPerWord`、`wordSize` 描述 MySQL DECIMAL 以每 9 位一个 4 字节 word 编码时，完整 word 和余数位所需的字节数；实际计算在 `pkg/parser/types/field_type.rs` 的 `FieldType::StorageLength` 中完成。
4. `ErrInvalidDefault`、`ErrDataOutOfRange`、`ErrTruncatedWrongValue`、`ErrIllegalValueForType` 建立 parser types 的标准错误对象；`initialize_standard_errors` 配合 crate 启动钩子保持 Go 包级变量的注册时序。

## 主要符号

- `pub fn IsTypeBlob(tp: u8) -> bool`：只接受 `TypeTinyBlob`、`TypeMediumBlob`、`TypeBlob`、`TypeLongBlob`。RustCodeGraph 显示它由本文件 `TypeToStr` 以及 `field_type.rs` 的 `FieldType::String`、`FieldType::Restore` 调用。
- `pub fn IsTypeChar(tp: u8) -> bool`：只将 `TypeString`（CHAR）和 `TypeVarchar` 归为字符类型；`TypeVarString` 不在集合内。调用边与 `IsTypeBlob` 相同。
- `pub fn IsTypeVector(tp: u8) -> bool`：当前只识别 `TypeTiDBVectorFloat32`。索引未显示该 Rust 函数的直接调用者，因此它目前主要是对齐 Go 的公开辅助 API，不能据此推断支持其他向量编码。
- `static type2Str: LazyLock<HashMap<u8, &'static str>>`：包含 28 个类型码到规范小写名称的映射，例如 `TypeBlob -> "text"`、`TypeString -> "char"`、`TypeTiDBVectorFloat32 -> "vector"`。
- `static str2Type: LazyLock<HashMap<&'static str, u8>>`：从 `type2Str` 迭代生成反向表，使两表的已登记项天然一致；若未来出现重复名称，`HashMap` 收集时只能保留其中一个类型码，因此扩展映射必须保持名称唯一。
- `pub fn TypeStr(tp: u8) -> &'static str`：返回规范名称；未登记的类型码返回空字符串，以对应 Go map 查询的字符串零值。
- `pub fn TypeToStr(tp: u8, cs: &str) -> String`：先取规范名称；只有 `cs == "binary"` 才做显示改写。BLOB 家族把首个 `text` 改为 `blob`，CHAR/VARCHAR 把首个 `char` 改为 `binary`，`TypeNull` 直接显示为 `binary`。
- `pub fn StrToType(ts: &str) -> u8`：依次把首个 `blob` 改为 `text`、首个 `binary` 改为 `char`，再查反向表；未命中返回 `mysql::TypeUnspecified`。该 API 是精确、区分大小写的字符串匹配，不会自动 trim 或统一大小写。
- `dig2bytes`、`digitsPerWord`、`wordSize`：分别是十进制余数位字节查表、每 word 9 位数字和每 word 4 字节；类型使用 `isize`，与 `FieldType` 的长度/小数位计算保持一致。
- 四个 `LazyLock<Box<terror::Error>>`：分别以对应的 `mysql::Err*` 错误码调用 `terror::ClassTypes.NewStd` 构造。
- `pub(crate) fn initialize_standard_errors()`：按 Go 声明顺序对四个 `LazyLock` 执行 `force`，仅供本 crate 初始化接线调用，不作为外部 API 再导出。

## 执行流程

类型显示主流程是：`FieldType::CompactStr` 或 `FieldType::Restore` 取得字段类型码和字符集，调用 `TypeToStr`；`TypeToStr` 先经 `TypeStr` 查询 `type2Str`，非 binary 字符集直接返回，binary 字符集则根据 `IsTypeBlob`、`IsTypeChar` 或 `TypeNull` 选择一次别名替换。`FieldType::String`/`Restore` 随后还会再次用 `IsTypeChar`、`IsTypeBlob` 决定是否输出字符集与排序规则。

反向解析流程是：`StrToType` 先将显示侧的 BLOB/BINARY 别名还原成表内的 TEXT/CHAR 名称，再查懒加载的 `str2Type`。因此 `"longblob"` 会先变成 `"longtext"` 并解析为 `TypeLongBlob`，`"binary"` 会变成 `"char"` 并解析为 `TypeString`；未知值走 `TypeUnspecified`，而不是返回错误。

DECIMAL 存储长度流程不在本文件执行：`FieldType::StorageLength` 将整数位数和小数位数分别按 `digitsPerWord` 拆成完整 word 与余数，再用 `wordSize` 和 `dig2bytes` 累加字节数。本文件只提供这套编码规则的参数。

错误初始化流程由 `pkg/parser/types/lib.rs` 的平台启动段 `PARSER_TYPES_PACKAGE_INIT` 触发，调用 `types::etc::initialize_standard_errors`，依次强制构造并注册四个标准错误。`pkg/parser/types/etc_test.rs::standard_errors_are_registered_during_package_initialization` 用子进程在 `terror::RegisterFinish` 前后验证这个时序。

## 数据与状态

本文件没有请求级、会话级或可变业务状态。两个名称映射和四个错误对象都是进程级 `LazyLock`：第一次访问或启动钩子强制初始化后保持不变。`type2Str` 的值是静态字符串切片；`TypeStr` 因此可以返回 `&'static str`。`TypeToStr` 和 `StrToType` 为了执行别名替换会分配新的 `String`，但不会修改全局表。

类型码和名称表共同构成兼容性数据：规范名称用于字段类型显示和 SQL 恢复，改变现有名称可能影响 information schema 文本、恢复后的 SQL 及名称往返。`dig2bytes` 的索引契约是余数范围 `0..=9`；当前调用方以 `% digitsPerWord` 产生 `0..=8`，不会越界。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashMap` 和 `std::sync::LazyLock`。通过 `lib.rs` 模块作用域注入的 `mysql` 提供类型码、字符集相关常量和错误码，`terror` 提供错误类别、错误码包装及标准错误构造。`Cargo.toml` 将它们分别绑定到工作区路径依赖 `../mysql` 与 `../terror`；该 crate 还依赖 charset、format、util 等，但这些不是 `etc.rs` 的直接引用。

RustCodeGraph 的直接调用边为：

- `FieldType::CompactStr -> TypeToStr -> TypeStr`，并在 binary 分支调用 `IsTypeBlob`/`IsTypeChar`；
- `FieldType::Restore -> TypeToStr`，同时直接调用 `IsTypeBlob`/`IsTypeChar` 来决定字符集与排序规则输出；
- `FieldType::String -> IsTypeBlob`/`IsTypeChar`；
- `FieldType::StorageLength -> digitsPerWord`、`wordSize`、`dig2bytes`；
- `PARSER_TYPES_PACKAGE_INIT -> initialize_standard_errors -> ErrInvalidDefault/ErrDataOutOfRange/ErrTruncatedWrongValue/ErrIllegalValueForType`。

`lib.rs` 通过 `pub use etc::*` 再导出公开函数、常量和错误对象，使其他 crate 可从 `parser_types::types::*` 或 crate 根使用它们；`initialize_standard_errors` 保持 `pub(crate)`，只服务内部启动接线。

## 错误处理与边界

名称转换采用哨兵值而非 `Result`：`TypeStr` 对未知类型码返回 `""`，`StrToType` 对未知、大小写不符或带额外空白的名称返回 `TypeUnspecified`。`TypeToStr` 继承 `TypeStr` 的空串行为；binary 分支即使对未知类型执行，也不会报错。调用者若需要区分“显式 unspecified”与“无法识别”，必须在进入本 API 前保留原始输入或额外校验。

别名替换使用 `replacen(..., 1)`，有意对齐 Go `strings.Replace(..., 1)`，不是词法解析：它只替换第一个匹配子串。当前公开规范名称使其得到预期的 `tinytext -> tinyblob`、`longtext -> longblob` 等结果；添加包含 `blob`、`binary`、`text` 或 `char` 子串的新名称时必须检查是否产生意外归一化。

四个错误对象只定义错误身份和标准码，不在本文件中产生或传播业务错误；真正的报错条件位于使用这些静态对象的调用方。错误码转换为 `isize` 后包装为 `terror::ErrCode`，测试校验对外 `Code()` 与 MySQL 错误码一致。

## 并发与资源生命周期

`LazyLock` 保证映射和错误对象在并发首次访问时只初始化一次；初始化完成后只通过共享不可变引用读取，不需要业务层锁。`str2Type` 初始化时会读取 `type2Str`，形成确定的单向初始化依赖，不存在反向依赖或循环。

本文件不创建线程、异步任务、通道、文件、网络连接或事务。`HashMap` 和 `terror::Error` 的生命周期为整个进程；`TypeToStr`/`StrToType` 产生的临时字符串由调用栈所有权管理。启动钩子在程序或 libtest 入口前强制错误初始化，其目的是完成全局错误注册，而不是管理可释放资源。

## 与 Go 版本的对应关系

`pkg/parser/types/etc.rs` 按 `pkg/parser/types/etc.go` 的声明顺序保留三类判定、双向映射、三个 DECIMAL 参数和四个标准错误。`u8` 对应 Go `byte`；Rust `matches!`/布尔表达式对应 Go `switch`/比较；`LazyLock<HashMap<...>>` 对应 Go 包级 map；`replacen(..., 1)` 对应 `strings.Replace(..., 1)`。

可观察语义保持一致：未知类型码在 `TypeStr` 中得到空字符串，未知名称在 `StrToType` 中得到 `TypeUnspecified`，BLOB/BINARY 只是显示与输入别名，binary 字符集下 `TypeNull` 显示为 `binary`。`pkg/parser/types/etc_test.go::TestStrToType` 验证 Go 表项往返及两个基础别名；Rust `etc_test.rs::test_str_to_type` 通过扫描完整 `u8` 域复现同一测试意图，避免为测试暴露私有表。

Rust 的额外接线是 `initialize_standard_errors` 与 `lib.rs` 平台启动段：Go 包级变量天然在导入时构造，Rust 用显式启动钩子和 `LazyLock::force` 模拟该时序。`etc_test.rs::standard_errors_are_registered_during_package_initialization` 是对应的 Rust 独立回归测试。`migration_aster_unit_test.rs::type_names_and_eval_types_match_go` 还覆盖 binary 显示、`longblob`、`TypeNull`、向量名称、未知输入和四个错误码。

## 扩展指南

新增 MySQL/TiDB 类型码时，先判断它是否属于 BLOB、CHAR 或 VECTOR 分类，再在 `type2Str` 添加唯一、规范的小写名称。由于 `str2Type` 自动派生，不应另建第二份手工映射；同时必须验证新名称经过 `StrToType` 的 `blob`/`binary` 子串归一化后仍唯一且正确。若新类型在 binary 字符集下需要特殊显示，应扩展 `TypeToStr` 的分支，并同步检查 `FieldType::CompactStr`、`String`、`Restore` 的输出契约。

修改 DECIMAL 编码参数时，应在 `FieldType::StorageLength` 的独立测试中覆盖 word 边界、余数边界、precision/scale 组合，不能只改查表。新增标准类型错误时，应在错误静态量旁定义、按 Go 声明顺序加入 `initialize_standard_errors`，并在 `etc_test.rs` 的子进程注册测试和错误码断言中同步覆盖。

测试必须继续放在独立文件：直接的名称/别名/初始化测试放入 `pkg/parser/types/etc_test.rs`；跨 `FieldType` 显示和存储长度的行为放入 `field_type_test.rs` 或现有 `migration_aster_unit_test.rs`。兼容风险主要是 SQL 显示文本与错误码身份；性能风险主要来自热路径中新增长字符串分配或扩大懒加载表，但当前查询为常数规模 HashMap 读取。

## 验证依据

- Rust 源与装配：`pkg/parser/types/etc.rs`、`pkg/parser/types/lib.rs`。
- crate 边界：`pkg/parser/types/Cargo.toml`，包名为 `astersql-parser-types`，直接路径依赖包含 `mysql` 与 `terror`。
- Go 对照：`pkg/parser/types/etc.go`；Go 测试：`pkg/parser/types/etc_test.go::TestStrToType`。
- Rust 独立测试：`pkg/parser/types/etc_test.rs::test_str_to_type`、`standard_errors_are_registered_during_package_initialization`；补充迁移证据：`pkg/parser/types/migration_aster_unit_test.rs::type_names_and_eval_types_match_go`。
- 直接实现调用方：`pkg/parser/types/field_type.rs` 中的 `CompactStr`、`String`、`Restore`、`StorageLength`。
- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/parser/types/etc.rs` 核对了 161 行完整源码；对带路径符号执行 `node`，确认 `TypeToStr` 调用 `TypeStr`/`IsTypeBlob`/`IsTypeChar` 且由 `CompactStr`/`Restore` 调用，确认 BLOB/CHAR 判定还由 `String` 调用；`IsTypeVector`、`StrToType` 和 `initialize_standard_errors` 未显示额外直接调用边。
- 本任务是纯文档分析，按计划不运行 Cargo；结构由任务指定的 11 标题检查验证，内容则通过上述源码、调用图、Go 对照和独立测试交叉复核。
