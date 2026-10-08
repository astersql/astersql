# `pkg/types/json_binary.rs`

## 文件定位

本文件实现 AsterSQL 类型系统中的 MySQL/TiDB 兼容二进制 JSON 基础表示。它不是独立遗留源码：`pkg/types/internal/json_binary/lib.rs` 通过 `#[path = "../../json_binary.rs"]` 将它挂入 `astersql-types-json-binary` 子 crate，并连同 `json_constants.rs` 的布局常量一起公开；根 `pkg/types/lib.rs` 再以 `pub use types_json_binary as json_binary` 将该子 crate 暴露给上层。`pkg/types/internal/json_binary/Cargo.toml` 表明直接依赖为 `base64`、`serde_json`、`thiserror` 与 `dbterror`；本文件实际直接使用前三者中的 `base64`、`serde_json`，错误类型来自同 crate 的 `json_constants` 模块。

它位于 SQL 值与线格式之间：上层把文本或 Rust 值变为 `BinaryJSON`，行编解码、表达式、Datum 与哈希路径再读取其类型码和载荷。可见生产调用包括 `pkg/types/datum.rs` 的 JSON 转换、`pkg/session/runtime/row_codec.rs` 的文本解析、`pkg/lightning/backend/kv/canonical.rs` 的规范化，以及 `pkg/util/codec/codec.rs` 的 `HashValue` 调用。

## 核心职责

- 用 `BinaryJSON { TypeCode, Value }` 保存“单字节类型码 + 独立载荷”；对象和数组载荷遵循 `json_constants.rs` 定义的 8 字节头、key entry、value entry 和相对容器起点的偏移。
- 在 `JsonValue` 中间树与二进制表示间双向转换：`CreateBinaryJSONWithCheck`/`appendBinaryJSON` 编码，`GetValue` 递归解码。
- 提供文本入口和输出：`ParseBinaryJSONFromString`、`UnmarshalJSON`、`MarshalJSON`、`String`，包括字符串转义、有限浮点检查、opaque 的 base64 展示以及时间类值展示。
- 提供容器随机访问和元信息：`GetElemCount`、`ArrayGetElem`、内部 `objectGetKey`/`objectGetVal`、`GetKeys`、`GetElemDepth`、`Type`。
- 生成比较/聚合使用的稳定哈希字节：`HashValue` 将可无损表示的整数规范化为 float64，并递归展开容器。
- 在构造阶段执行最大深度 100、对象键最大 65535 字节和数字可解析性等检查。

## 主要符号

- `maxJSONDepth: usize = 100`：允许的容器嵌套层数上限；`CreateBinaryJSONWithCheck` 用 `value_depth(...)-1` 检查。
- `BinaryJSON`：公开核心值类型。`TypeCode` 决定 `Value` 的解释方式，`Clone` 是深拷贝字节缓冲，`Eq`/`PartialEq` 比较实际编码。
- `Opaque`：保存 MySQL 字段类型码与原始字节；二进制载荷为字段类型码、uvarint 长度和数据。
- `JsonTime`、`JsonDuration`：分别保存打包 `CoreTime`/JSON 类型/FSP，以及纳秒 duration/FSP。它们避免本 JSON 子 crate 依赖完整时间子系统。
- `JsonValue`：编码前的封闭中间表示，覆盖 null、布尔、三类数值、数字字符串、字符串、已有 `BinaryJSON`、数组、按键排序的 `BTreeMap` 对象、opaque、时间和 duration；多个 `From` 实现构成 `CreateBinaryJSON<T: Into<JsonValue>>` 的公开输入面。
- `ParseBinaryJSONFromString` 与 `UnmarshalJSON`：使用 `serde_json` 解析文本、保留数字文本，再交由受检构造路径编码。
- `CreateBinaryJSON`：便利入口；失败时 panic。`CreateBinaryJSONWithCheck`：可恢复错误入口。`CalculateBinaryJSONSize`：编码前估算载荷大小，错误同样通过 panic 暴露。
- `MarshalJSON`/`marshalTo`：按类型码分派文本输出；`String` 将错误吞为默认空串，调用方若需要区分失败必须直接用 `MarshalJSON`。
- `valEntryGet`：容器随机访问的核心。literal 直接从 entry 内联字节构造；固定宽度、字符串、opaque 和嵌套容器分别推导切片长度。
- `CalculateHashValueSize`/`HashValue`：前者报告容量，后者生成真实哈希输入；两者对可精确转 float64 的整数使用相同的 52 位判定。

## 执行流程

文本构造流程为：`ParseBinaryJSONFromString` 先拒绝空串，`serde_json::from_str` 拒绝无效或尾随内容，`from_serde_value` 将 JSON 数字保留为 `JsonValue::Number(String)`，随后 `CreateBinaryJSONWithCheck` 检查深度并调用 `appendBinaryJSON`。标量直接追加小端定长值或 uvarint 长度载荷；数组先写元素数、总长占位和 value-entry 表，再逐项写值并回填相对偏移与总长；对象 additionally 先按 `BTreeMap` 顺序写 key-entry 和所有键，再写 value-entry/值载荷。literal 不另占载荷区，而是内联到 value entry。

读取流程由 `TypeCode` 驱动。标量 getter 直接读取小端字节或长度前缀；数组经 `ArrayGetElem -> valEntryGet`，对象经 `objectGetKey`/`objectGetVal -> valEntryGet`。嵌套容器的 entry 保存相对当前容器起点的偏移，其载荷长度取容器头中的 size；因此复制出的子 `BinaryJSON` 自身仍是完整、可继续遍历的值。

文本输出流程为 `MarshalJSON -> marshalTo`。数组和对象按存储顺序递归输出并使用逗号空格规范格式；字符串处理引号、反斜杠、控制字符及 U+2028/U+2029；非有限浮点返回 `UnsupportedValue`，普通整数形式的有限浮点补 `.0`，极端量级使用规范化科学计数法；opaque 输出为 `"base64:typeN:..."`。

哈希流程中，尾数判定不超过 52 位的 `i64/u64` 被改写为 `JSONTypeCodeFloat64 + 8 字节 f64`，使 `3` 与 `3.0` 取得一致数值表示。数组哈希保留类型码与头部前四字节后递归追加元素；对象还将键编码成 binary string 后递归追加值。其它类型保留类型码和原始载荷。

## 数据与状态

`BinaryJSON` 只拥有 `Vec<u8>`，没有借用外部缓冲。根值的类型码不存入 `Value`；容器子值的类型码存于 value entry。数组/对象头为 `element_count: u32` 与 `data_size: u32`；对象 key entry 是 `offset: u32 + length: u16`，value entry 是 `type: u8 + offset/inline literal: u32`。整数、浮点和打包时间为 8 字节小端，duration 另带 4 字节 FSP；字符串和 opaque 使用最多 10 字节的 protobuf 风格 uvarint 长度。

对象内存输入使用 `BTreeMap<String, JsonValue>`，所以键在编码前按字节可比较的字符串顺序稳定排列；这与 Go `appendBinaryObject` 显式排序字段的目的相同。`GetKeys` 因而返回排序后的键数组。`Copy`/派生 `Clone` 会复制整个 `Value`；getter 对容器元素也创建新的 `Vec<u8>`，不存在共享视图。

深度定义为标量 1、空容器 1、非空容器 1 加最深子值。构造限制比较的是 `value_depth - 1 > 100`，与 Go 注释“`GetElemDepth` 总是多 1”对应。大小估算不含根类型码；字符串和 opaque 使用最坏 10 字节长度前缀，因此它是面向容量规划的估算，不保证等于最终紧凑编码长度。

## 依赖与调用关系

模块下游依赖如下：`super::json_constants::*` 提供类型码、布局常量与 `JsonError`；`serde_json` 负责文本语法解析；`base64` 仅用于 opaque 的可读文本表示；标准库 `BTreeMap` 保证对象顺序。模块没有 I/O、存储或网络依赖。

上游装配链是 `pkg/types/internal/json_binary/lib.rs -> pkg/types/json_binary.rs`，再由 `pkg/types/lib.rs` 的 `json_binary` 再导出供 workspace 使用。直接生产调用证据包括：

- `pkg/types/datum.rs`：Datum 与 JSON 间转换，时间/duration 构造，以及字符串 JSON 解析。
- `pkg/session/runtime/row_codec.rs`、`pkg/lightning/backend/kv/canonical.rs`：把输入文本解析成持久化所需 JSON 值。
- `pkg/util/codec/codec.rs`：编码 key/行时调用 `HashValue`，包括复用外部 hash buffer 的批量路径。
- `pkg/util/chunk/mutrow.rs`、`pkg/expression/builtin_cast_vec.rs`：构造 null、数值、opaque、时间与 duration JSON。

JSON 路径、比较、合并等高级操作主要在 `pkg/types/json_binary_functions.rs` 或 `types-json-functions` 子 crate 中；它们消费这里定义的 `BinaryJSON` 和访问器，本文件自身不实现 SQL JSON 函数语义。

## 错误处理与边界

可恢复入口返回 `JsonError`：空/非法文本映射为 `InvalidJsonText`，数字字符串无法解释映射为 `InvalidJsonData`，非有限浮点文本化映射为 `UnsupportedValue`，深度超限为 `DocumentTooDeep`，对象键达到 65536 字节为 `ObjectKeyTooLong`。`CreateBinaryJSON` 与 `CalculateBinaryJSONSize` 则是与 Go 便利接口一致的 panic 路径，不应直接接收不可信或未验证输入。

读取器假定 `BinaryJSON` 已由可信编码路径生成。`GetString`、`GetOpaque`、`read_u16/u32/u64`、`valEntryGet` 使用索引、切片和 `expect`，截断载荷、非法偏移、未知变长值类型或坏 uvarint 可能 panic；它们不是任意二进制校验器。未知 `TypeCode` 在 `marshalTo` 中输出空追加、在 `GetValue` 中退化为 `Null`、在 `Type` 中显示 `OPAQUE`，三者语义不同，新增类型码时必须同步更新全部分派点。

`String` 在 `MarshalJSON` 失败时返回空串，会隐藏非有限浮点等原因。`GetValue` 对非法 literal 也退化为 null。时间格式化当前始终为微秒六位格式，`JsonTime.Fsp`/`JsonDuration.Fsp` 不参与裁剪；如果上层要求严格 FSP 文本兼容，应先补齐实现与对照测试，而不能假定字段已生效。

## 并发与资源生命周期

本文件没有全局可变状态、锁、通道、线程、异步任务或事务。所有编码/解码均在调用线程同步完成，状态由拥有型 `Vec<u8>` 和局部缓冲承载；不同 `BinaryJSON` 可独立跨线程使用，具体自动 trait 由其纯值字段推导。

构造与文本输出会分配新缓冲；`HashValue` 接收并返回调用方缓冲以允许复用。容器读取目前复制子载荷，递归 `GetValue`、`GetElemDepth`、文本输出和哈希均可能随文档大小/深度产生线性分配或递归栈开销。最大构造深度限制约束正常入口，但手工构造的 `BinaryJSON` 可绕过该限制，因此处理外部原始二进制前必须有独立校验边界。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/types/json_binary.go`，测试对照为 `pkg/types/json_binary_test.go`。Rust 保留了 Go 的 `BinaryJSON` 二字段布局、类型码、容器相对偏移、literal entry 内联、对象键排序、字符串/opaque 长度前缀、深度 100、长键错误、浮点哈希规范化及公开函数命名。`JsonTime`/`JsonDuration` 是为拆分 Rust crate 而引入的轻量替身，对应 Go 的 `Time`/`Duration` 所需字段。

需要注意的当前差异：Rust 以 `JsonValue` 和泛型 `Into` 代替 Go 的 `any` 类型开关；Rust `GetValue` 能递归返回数组、对象、literal 和 opaque，而 Go 同名实现只返回若干标量并把其它类型视为不可达；Rust `serde_json` 负责语法解析，Go 使用 `encoding/json.Decoder.UseNumber`。两端都按 int64、uint64、float64 顺序解释数字字符串。Go `CalculateHashValueSize` 的容器分支虽计算局部递归大小，函数当前最终仍返回 `len(Value)+1`；Rust显式保留这一可观察结果，`pkg/types/json_binary_test.rs::TestHashValue` 也锁定了它。

Rust 独立测试 `pkg/types/json_binary_test.rs` 基本沿用 Go 测试命名，并额外以 Rust 断言覆盖。高级 Extract/Modify/Merge/Walk 测试经过 `json_binary_functions` 扩展层；本文件最直接的回归面是 `TestBinaryJSONMarshalUnmarshal`、`TestBinaryJSONCopy`、`TestGetKeys`、`TestBinaryJSONDepth`、`TestParseBinaryFromString`、`TestCreateBinary`、`TestBinaryJSONOpaque` 和 `TestHashValue`。

## 扩展指南

新增标量类型码时，至少同步检查 `json_constants.rs`、`appendBinaryJSON`、`calculate_value_size`、`valEntryGet` 的载荷长度、`marshalTo`、`GetValue`、`Type`、`HashValue`/`CalculateHashValueSize`，以及高级 JSON 比较函数；只补编码会导致值无法安全读取或产生不稳定哈希。若类型需要公开构造，还应增加 `JsonValue` 变体和相应 `From` 实现。

调整容器布局时必须保持 entry 偏移相对容器起点、literal 内联、对象键和值索引一一对应、头部 size 等于完整容器载荷这四个不变量，并同时更新 `json_constants.rs`。所有新测试应继续放在独立的 `pkg/types/json_binary_test.rs`（或对应高级功能测试文件），不要内嵌到生产源文件。

错误边界扩展优先使用 `CreateBinaryJSONWithCheck`/`MarshalJSON` 返回 `JsonError`，并在 `json_constants.rs` 增加明确 kind；不要把新校验放进只读 getter 后再依赖 panic。性能改动应重点测量深层容器的递归、`valEntryGet` 的子缓冲复制、对象重复遍历，以及大小估算是否仍满足容量上界。兼容性验证应同时比较 Go 的规范文本、精确二进制载荷与哈希结果，尤其覆盖 `2^52` 附近整数、超长键、空容器、opaque 长度跨 uvarint 边界、时间 FSP 和 100/101 层边界。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11467 个文件；`query BinaryJSON --kind struct`、`query ParseBinaryJSONFromString --kind function` 定位到本文件符号。`files --filter pkg/types/json_binary` 与路径限定 `node/callers/callees` 未返回目标，因此调用边按技能规则改用源码和仓库 `rg` 核验。
- 已读生产文件：`pkg/types/json_binary.rs`、`pkg/types/json_constants.rs`、`pkg/types/internal/json_binary/lib.rs`、`pkg/types/internal/json_binary/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`；仓库未发现 `pkg/types/doc.go`。
- 已读 Go 对照：`pkg/types/json_binary.go`；已读测试：`pkg/types/json_binary_test.rs`、`pkg/types/json_binary_test.go`。测试证据覆盖文本往返、类型、复制、排序键、65536 字节键失败、深度、空/尾随文本、数值类型、opaque/uvarint、哈希区分与容器 size 现状。
- 上游调用通过 `rg` 核验于 `pkg/types/datum.rs`、`pkg/session/runtime/row_codec.rs`、`pkg/lightning/backend/kv/canonical.rs`、`pkg/util/codec/codec.rs`、`pkg/util/chunk/mutrow.rs` 和 `pkg/expression/builtin_cast_vec.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文档存在且恰含 11 个固定二级标题，并人工复核文件定位、运行流程和安全扩展入口均有源码或测试依据。
