# `pkg/util/dbutil/types.rs`

## 文件定位

提供三个纯布尔分类函数，让调用方不必重复枚举类型码。

## 核心职责

本文件承担两项窄职责：

1. 以 `u8` 常量保存本模块分类所需的 MySQL 类型码，保持与 Go `byte` 和 MySQL 协议单字节类型码一致。
2. 提供三个纯布尔分类函数，让调用方不必重复枚举类型码。

生产代码中的直接用途目前只有时间分类：[`common.rs`](./common.rs) 的 `AnalyzeValuesFromBuckets` 调用 `IsTimeTypeAndNeedDecode`，仅当列属于 `DATETIME`、`TIMESTAMP` 或 `DATE` 且桶值不是可读时间字符串时，才进入 `DecodeTimeInBucket`。RustCodeGraph 的文件级关系也只列出 `common.rs` 和独立测试 `common_test.rs` 为本文件使用方；`IsNumberType`、`IsFloatType` 当前没有查到生产调用者，属于已公开但尚未在本 crate 生产路径消费的辅助 API。

## 主要符号

- `TypeDecimal: u8 = 0`：本地保留的旧 DECIMAL/占位类型码。它不在三个分类函数的任何集合中；同值在 [`pkg/parser/mysql/type.rs`](../../parser/mysql/type.rs) 中命名为 `TypeUnspecified`。
- 整数相关常量：`TypeTiny = 1`、`TypeShort = 2`、`TypeLong = 3`、`TypeLonglong = 8`、`TypeInt24 = 9`、`TypeYear = 13`。
- 浮点/定点数相关常量：`TypeFloat = 4`、`TypeDouble = 5`、`TypeNewDecimal = 246 (0xf6)`。
- 时间相关常量：`TypeTimestamp = 7`、`TypeDate = 10`、`TypeDatetime = 12`。
- `pub fn IsNumberType(tp: u8) -> bool`：只接受上述六种整数类型；`YEAR` 被视为整数，`FLOAT`、`DOUBLE` 和两种 DECIMAL 不在此集合。
- `pub fn IsFloatType(tp: u8) -> bool`：接受 `FLOAT`、`DOUBLE`、`NEWDECIMAL`；不接受 `TypeDecimal = 0`。
- `pub fn IsTimeTypeAndNeedDecode(tp: u8) -> bool`：接受 `DATETIME`、`TIMESTAMP`、`DATE`；不包含协议中的 `TIME/TypeDuration`。

所有常量和函数都是 `pub`，但 crate 根只声明 `pub mod types`，没有把这些名字平铺再导出；外部调用者应通过 `astersql_util_dbutil::types::...` 访问。

## 执行流程

三个函数都执行固定集合成员判断，没有分配、循环或 I/O：

1. 调用方传入一个 `u8` 类型码。
2. `matches!` 将该字节与函数内列出的常量逐一匹配。
3. 命中任一常量即返回 `true`，其他值（包括未知、保留或未列出的合法 MySQL 类型）返回 `false`。

实际桶解析链路为：`common::AnalyzeValuesFromBuckets` 拆分桶边界值并校验值数量 → 对每个值调用 `IsTimeTypeAndNeedDecode` → 时间类型且值不是可读时间格式时调用 `common::DecodeTimeInBucket` → 将 packed `u64` 转成时间字符串。分类函数只决定是否进入解码分支，不负责判断字符串形态，也不传播解码错误。

## 数据与状态

本文件的数据全部是编译期 `u8` 常量。函数只读取入参并返回 `bool`，没有全局可变状态、缓存、配置、环境变量或持久化数据。

关键不变量是类型码必须与权威协议定义保持一致。当前枚举值与 [`pkg/parser/mysql/type.rs`](../../parser/mysql/type.rs) 以及 Go 的 [`pkg/parser/mysql/type.go`](../../parser/mysql/type.go) 对应值一致；例外是本地 `TypeDecimal = 0` 的命名，它与解析器模块的 `TypeUnspecified = 0` 同值，且当前未被分类函数使用。

## 依赖与调用关系

- 上游模块装配：[`lib.rs`](./lib.rs) 的 `pub mod types` 使该模块成为 crate 公共子模块。
- 生产调用者：[`common.rs`](./common.rs) 导入 `IsTimeTypeAndNeedDecode`，并在 `AnalyzeValuesFromBuckets` 中保护 packed 时间解码分支。
- 测试调用者：[`common_test.rs`](./common_test.rs) 导入三个函数；`query_scanners_and_mysql_type_classification_match_go` 验证全部正例集合和每类一个负例。
- 下游依赖：三个分类函数仅引用同文件常量；没有可继续追踪的函数 callee。时间分类产生的布尔结果间接决定 `common::DecodeTimeInBucket` 是否执行，但该调用属于 `AnalyzeValuesFromBuckets`，不是本文件发起。
- Go 对照：[`types.go`](./types.go) 直接使用 `pkg/parser/mysql` 常量实现同名函数；Rust 为避免该小模块额外依赖，在本文件中保存所需数值。

RustCodeGraph 对三个精确 Rust 函数执行 `callers`/`callees` 没有返回函数级边；文件级索引给出的两个使用文件与文本引用一致。因此这里不宣称存在索引未证明的跨 crate 调用链。

## 错误处理与边界

这些 API 不返回 `Result`，也不会 panic。所有未命中的字节统一返回 `false`，所以未知类型码和“合法但不属于该类别”的类型码在本层不可区分。

边界尤其包括：

- `TypeDecimal = 0` 不被 `IsFloatType` 接受，只有 `TypeNewDecimal = 246` 被接受。
- `TypeYear = 13` 被 `IsNumberType` 接受。
- `TIME/TypeDuration = 11` 不被 `IsTimeTypeAndNeedDecode` 接受；函数名所表达的是“需要按 TiDB 桶 packed 时间解码”的子集，而不是所有时间语义类型。
- 分类结果为 `true` 不保证输入值一定是 packed 数字；`AnalyzeValuesFromBuckets` 还会用 `is_time_string` 排除已格式化的时间值，真正的解析失败由 `DecodeTimeInBucket` 以 `Err(String)` 报告。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、文件句柄、网络连接或事务。常量是不可变静态数据，三个纯函数可被任意线程并发调用，不存在初始化或清理阶段。

在调用链中，桶值 `String` 的分配与原地替换由 `AnalyzeValuesFromBuckets` 管理；本文件只按值接收一个字节，不借用或持有调用方资源。

## 与 Go 版本的对应关系

实现形式不同但语义等价：Go 用 `switch`/条件表达式并引用 `pkg/parser/mysql`；Rust 用 `matches!` 和本地 `u8` 常量。

## 扩展指南

新增或调整类型分类时，应按以下顺序处理：

1. 先核对 [`pkg/parser/mysql/type.rs`](../../parser/mysql/type.rs) 与 Go [`pkg/parser/mysql/type.go`](../../parser/mysql/type.go) 的协议值，避免在此创造新的类型码来源。
2. 修改对应常量或 `matches!` 集合，并明确该类型是否真的属于调用方所需语义。例如把 `TIME/TypeDuration` 加入时间集合会使统计桶路径尝试按 packed 日期时间解码，不能只凭“它是时间类型”决定。
3. 在独立测试文件 [`common_test.rs`](./common_test.rs) 中同步正例、相邻负例和未知值；不要把测试嵌入 `types.rs`。若分类改变桶处理行为，还需扩充 `AnalyzeValuesFromBuckets` 的用例，并与 [`common_test.go`](./common_test.go) 的预期核对。
4. 若新增常量与解析器模块重复，优先评估改为复用 `astersql-parser-mysql` 是否合适；这会涉及 [`Cargo.toml`](./Cargo.toml) 的依赖边界，不能只修改本文件。

主要兼容风险是错误类型码导致值走错解析分支；主要正确性风险是扩大时间集合后对普通字符串执行 packed 数字解析。分类操作本身是常数时间，性能风险很低；不要用动态集合替代当前固定匹配，除非确有运行时配置需求。
