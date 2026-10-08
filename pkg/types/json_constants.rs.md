# `pkg/types/json_constants.rs`

## 文件定位

`pkg/types/json_constants.rs` 是 AsterSQL 类型系统中 MySQL/TiDB Binary JSON 协议常量与标准 JSON 错误的集中定义。它不由根 `pkg/types/lib.rs` 直接声明为普通模块，而是由 `pkg/types/internal/json_binary/lib.rs` 通过 `#[path = "../../json_constants.rs"] pub mod json_constants` 编入 `astersql-types-json-binary`，随后以 `pub use json_constants::*` 导出；根 `astersql-types` crate 再通过 `pub use types_json_binary as json_binary` 暴露该子 crate。因此常见访问路径是 `astersql_types::json_binary::*` 或其 `json_constants` 子模块。

所属 crate 边界由 `pkg/types/Cargo.toml` 确认：根包名为 `astersql-types`，依赖内部包 `types-json-binary`；本文件直接使用 `thiserror::Error` 和 `dbterror`，二者都列在根 manifest 的依赖中。本文件没有 Cargo feature 分支，但 `JSON_ERRORS_INIT` 针对 Unix、Apple 和 Windows 使用不同链接段属性，承担跨平台的模块初始化接线。

## 核心职责

1. 定义 Binary JSON 的单字节类型码、null/true/false 字面量编码以及对象/数组头和 entry 的固定布局尺寸，供编码、解码、随机访问、类型转换与落盘逻辑共享。
2. 提供 JSON 字符串转义所需的 ASCII 安全字符表 `jsonSafeSet` 和十六进制字符表 `jsonHexChars`。
3. 以 `json_type_precedence` 表达 MySQL 5.7 JSON 类型比较优先级，并对未知或大小写不匹配的类型名返回 `None`。
4. 定义 `JSONModifyInsert`、`JSONModifyReplace`、`JSONModifySet` 三种路径修改模式，以及 `JSONContainsPathAll`/`One` 两种 contains-path 模式字符串。
5. 同时提供 Rust 内部的 `JsonError`/`JsonErrorKind` 和可解引用为 `dbterror::terror::Error` 的 `JsonErrorDefinition`，让调用者既能保留 Rust 错误分类，也能沿用 Go/TiDB 标准错误码、错误类和消息生成 API。

该文件只定义协议词汇、不可变表和错误描述符，不负责解析 JSON 文本、验证二进制载荷或执行 JSON 路径修改；这些行为主要在 `pkg/types/json_binary.rs`、`pkg/types/json_binary_functions.rs` 及其内部子 crate 中实现。

## 主要符号

- `pub type JSONTypeCode = u8` 及 `JSONTypeCodeObject` 到 `JSONTypeCodeDuration`：定义 on-wire 类型码 `0x01`、`0x03`、`0x04`、`0x09..=0x11`。`BinaryJSON::TypeCode`、`JsonTime::TypeCode` 和多个转换分支都使用这一别名。
- `JSONLiteralNil`、`JSONLiteralTrue`、`JSONLiteralFalse`：定义 literal 载荷首字节的三个有效值 `0x00..=0x02`。
- `unknownTypeCodeErrorMsg`、`unknownTypeErrorMsg`：保留 Go 风格 `%d`/`%s` 模板；当前独立 Rust 测试验证其文本，本文件不负责格式化这些模板。
- `const fn make_json_safe_set() -> [bool; 128]` 与 `pub(crate) const jsonSafeSet`：在编译期生成 ASCII 查表；仅 `0x20..=0x7f` 可候选直出，其中双引号和反斜杠为 `false`，控制字符 `0x00..=0x1f` 也保持 `false`。
- `pub(crate) const jsonHexChars: &[u8; 16]`：为 `\uXXXX` 等转义提供小写十六进制数字。
- `headerSize`、`dataSizeOff`、`keyEntrySize`、`keyLenOff`、`valTypeSize`、`valEntrySize`：分别固定容器头、数据长度偏移、对象 key entry 和 value entry 布局。数值是协议的一部分，不能当作普通调优参数修改。
- `pub fn json_type_precedence(&str) -> Option<i32>`：将 `BLOB` 到 `NULL` 映射到 `-1` 到 `-12`；`INTEGER`、`UNSIGNED INTEGER`、`DOUBLE` 同为 `-11`。匹配区分大小写，未知输入返回 `None`。当前精确检索只发现独立测试调用，尚未发现生产调用点。
- `pub type JSONModifyType = u8` 与三个 `JSONModify*` 常量：值分别为 `0x01`、`0x02`、`0x03`，其中 SET 在位值上等于 INSERT 与 REPLACE 的组合。
- `JsonErrorKind`：枚举 15 种 Rust 侧分类；除 13 个标准错误描述符覆盖的类型外，还包含 `UnsupportedValue`、`UnknownType`、`InvalidBinaryData`，供非标准错误文本使用。
- `JsonError::new` 与 `JsonError::kind`：构造包含分类和自定义消息的 `thiserror` 错误，并只读取回分类；显示文本完全由传入 `message` 决定。
- `JsonErrorDefinition`：保存 `JsonErrorKind` 和一个返回静态标准错误的函数指针；`From<JsonErrorDefinition> for JsonErrorKind` 支持直接传给 `JsonError::new`，`Deref<Target = dbterror::terror::Error>` 则开放 `Code`、`GenWithStackByArgs` 等标准错误 API。
- `standard_json_error!`：为每个 `ErrInvalidJSON*`/`ErrJSON*` 常量生成上述双重描述符。除 `ErrJSONObjectKeyTooLong` 使用 `ClassTypes` 外，其余均使用 `ClassJSON`。
- `JSON_ERRORS_INIT`：平台初始化函数表项，启动时逐个解引用 13 个标准错误描述符，从而触发对应 `LazyLock` 注册/构造。
- `JSONContainsPathAll`、`JSONContainsPathOne`：分别为字面串 `"all"`、`"one"`，是 SQL JSON_CONTAINS_PATH 模式的共享值。

## 执行流程

普通 Binary JSON 流程从上游构造或读取 `BinaryJSON { TypeCode, Value }` 开始。`pkg/types/json_binary.rs` 按本文件的 `JSONTypeCode*` 分派标量、容器、时间和 opaque 数据；数组/对象读取和写入再依据 `headerSize`、`keyEntrySize`、`valEntrySize` 等计算条目位置。literal 分支读取 `JSONLiteral*`，字符串序列化通过 `jsonSafeSet` 判断是否可直出，否则使用 `jsonHexChars` 生成转义。`pkg/types/convert.rs` 也按相同类型码决定 JSON 到整数/浮点的转换或截断错误。

错误路径分成两层。需要 Rust 内部轻量错误时，调用者使用 `JsonError::new(kind_or_definition, message)`；`Into<JsonErrorKind>` 会把 `JsonErrorDefinition` 转成对应分类，之后 `Display` 仅输出消息。需要 MySQL/TiDB 兼容错误码时，调用者直接对 `ErrInvalidJSONCharset` 等描述符做解引用或调用其标准错误方法；首次解引用执行宏生成的闭包，在函数局部 `LazyLock<Box<dbterror::terror::Error>>` 中调用 `dbterror::<Class>.NewStd(errno::<Name>)`，后续复用同一对象。

模块装载时，受支持平台把 `JSON_ERRORS_INIT` 放入对应初始化段。其 `initialize` 依次解引用全部 13 个标准错误常量，主动触发惰性对象创建。这条路径不处理业务输入，也不返回错误；它用于尽早完成与 Go 包初始化相当的标准错误接线。

`json_type_precedence` 是独立的纯匹配流程：收到类型名后在 `match` 中返回优先级，未命中立即返回 `None`。它不规范化大小写，也不从类型码反推类型名。

## 数据与状态

所有协议常量和查表均不可变。`make_json_safe_set` 在编译期构造恰好 128 项的布尔数组；索引只覆盖 ASCII，调用方在索引前必须确保字节小于 128。Binary JSON 布局的重要不变量包括：容器头为 8 字节（元素数 4 字节加总长度 4 字节）；key entry 为 6 字节；value entry 为 5 字节；value entry 首字节是类型码，其余位置用于偏移或内联 literal。

唯一具有惰性初始化状态的是宏为每个标准错误生成的函数局部 `std::sync::LazyLock<Box<dbterror::terror::Error>>`。每个描述符持有无捕获函数指针，而不是直接持有可变错误对象；同一描述符多次解引用返回同一静态实例，独立测试用指针相等验证了这一点。`JsonError` 自身拥有一个 `String`，克隆时复制消息；`JsonErrorKind` 和 `JsonErrorDefinition` 均为 `Copy`。

`json_type_precedence` 不再使用 Go 版本的可变 map，而用函数中的静态 `match` 表达同一集合，避免全局可变映射和查表初始化。该函数的负数方向是现有兼容数据，不应仅凭数值直觉反转或重编号。

## 依赖与调用关系

装配链为 `pkg/types/internal/json_binary/lib.rs` 引入本文件并再导出，`pkg/types/Cargo.toml` 将该内部 crate 作为 `types-json-binary` 依赖，`pkg/types/lib.rs` 再以 `json_binary` 名称导出它。测试 `pkg/types/json_constants_test.rs` 通过 `crate::json_binary::json_constants::*` 访问本文件；这也证明生产定义属于 JSON binary 子 crate，而不是根 crate 的私有测试模块。

已核实的直接或代表性生产消费者如下：

- `pkg/types/json_binary.rs` 直接 `use super::json_constants::*`，消费类型码、literal、布局、转义表和 `JsonError`，完成二进制 JSON 编解码及文本序列化。
- `pkg/types/convert.rs` 按类型码和 literal 值实现 JSON 数值转换。
- `pkg/types/datum.rs` 在 JSON 字符集不合法时使用 `ErrInvalidJSONCharset`；`pkg/types/internal/datum/lib.rs` 从 JSON binary crate 再导入相关 JSON 类型与错误。
- RustCodeGraph 的文件节点还报告本文件被 `pkg/types/time.rs`、`pkg/util/chunk/row_in_disk.rs` 等共 8 个文件使用，说明这些常量同时约束时间型 JSON 和 chunk 落盘表示。

下游依赖包括标准库的 `LazyLock`、`Deref`、链接段属性和字符串所有权，`thiserror` 的 `Error` 派生，以及 `dbterror` 的错误类、错误号和标准错误构造器。本文件不调用解析器、存储层、网络或 SQL executor。`json_type_precedence` 的精确 RustCodeGraph callers/callees 查询没有产生可用调用边，精确文本检索仅发现本文件和 `pkg/types/json_constants_test.rs`，因此不能将 Go 侧比较流程推断成当前 Rust 生产接线。

## 错误处理与边界

- `json_type_precedence` 对空串、小写 `null`、`TIMESTAMP` 和未知名称返回 `None`；调用者必须显式处理缺失值。函数只接受列出的 MySQL 比较类型名，不接受任意 Binary JSON 类型码名称。
- `JsonError::new` 不校验消息是否匹配错误分类，也不附加 SQL 错误码；它适合内部错误传播。需要客户端可见的标准错误码和参数化消息时，应使用相应 `Err*` 描述符的 `GenWithStackByArgs`。
- `JsonErrorDefinition` 的 `Deref` 会触发惰性构造，但构造闭包没有可恢复的 `Result` 分支；标准错误号或错误类若配错，会成为静态兼容性缺陷，而非运行时可处理输入错误。
- 13 个标准描述符中 `ErrJSONObjectKeyTooLong` 必须保持 `ClassTypes`/8129，其余测试覆盖的描述符属于 `ClassJSON`。修改类或 errno 会改变 SQL 错误身份、客户端消息和上层分支判断。
- `jsonSafeSet` 的索引域只有 ASCII。它特意将 DEL (`0x7f`) 设为可直出，与 Go 表逐项一致；不要在没有跨语言兼容证据时按一般控制字符直觉改写。
- 布局常量直接参与切片和偏移计算。错误值可能造成解码错位、越界或不兼容数据，边界校验责任位于实际编解码函数，而不在本常量文件。
- `JSONModifyType` 是 `u8` 别名而非封闭 enum，本文件不能阻止调用者传入 `0` 或其他非法值；实际修改入口必须继续校验或穷尽处理合法模式。

## 并发与资源生命周期

类型码、布局值、字符串常量和静态查表只读，可被任意线程并发访问。`json_type_precedence` 是无分配、无副作用的常数时间匹配；`make_json_safe_set` 只在编译期执行。文件没有锁、线程、异步任务、通道、事务、文件句柄或网络连接。

标准错误对象由 `std::sync::LazyLock` 保证线程安全的一次初始化，成功后存活至进程结束。`JSON_ERRORS_INIT` 又在平台模块初始化阶段主动触发这些锁；业务线程随后通常只读取静态对象。描述符内部的函数指针不捕获环境，`JsonErrorDefinition` 可复制；`JsonError` 的消息 `String` 由错误值自行拥有并按普通 Rust 所有权释放。

性能敏感点主要是热路径上的数组索引和布局算术，均为 O(1)；本文件没有运行时 map 查找。扩展类型码或错误时，避免为单纯常量查询引入锁、堆分配或重复注册。标准错误的 `Box` 仅在每类错误首次初始化时分配一次。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/types/json_constants.go`。Rust 逐项保留了 `JSONTypeCode`、12 个类型码、3 个 literal、两个未知类型模板、6 个布局常量、三种修改模式、13 个标准错误和两个 contains-path 字符串。Go 的 `jsonSafeSet [utf8.RuneSelf]bool` 对应 Rust `[bool; 128]`；Go 的 `jsonHexChars` 字符串对应 Rust 16 字节数组引用。小端序 `jsonEndian` 位于 Go 常量文件，但 Rust 版本没有在本文件定义同名全局，而由具体二进制读写实现负责小端转换。

Go 的 `jsonTypePrecedences map[string]int` 在 Rust 中改为 `json_type_precedence(&str) -> Option<i32>`，键和值集合一致；Rust 用 `None` 明确表达未知键。Go 的包级错误变量直接保存 `dbterror` 标准错误；Rust 增加 `JsonErrorKind`、`JsonError` 和 `JsonErrorDefinition` 适配层，并以 `Deref` 保留标准错误 API。Rust 还增加平台链接段初始化 `JSON_ERRORS_INIT`，显式模拟 Go 包初始化时标准错误已存在的生命周期。

Rust 独立测试 `pkg/types/json_constants_test.rs` 覆盖未知类型模板、全部 precedence 项和未知项、13 个错误码/错误类、标准消息生成、`JsonErrorDefinition -> JsonErrorKind` 转换以及惰性对象身份。Go 仓库没有同路径 `json_constants_test.go`；相关常量在 `pkg/types/json_binary_test.go`、`pkg/types/convert_test.go` 等行为测试中被间接覆盖。因此不能宣称每个常量都有一一对应的 Go 单元测试。

## 扩展指南

- 新增 Binary JSON 类型时，应先确认 MySQL/TiDB on-wire 编码，再成组更新 `JSONTypeCode*`、`pkg/types/json_binary.rs` 的编码/解码/序列化分支、`pkg/types/convert.rs` 的转换策略，以及独立 Rust 测试；还要检查 `pkg/types/internal/json_functions/lib.rs` 中当前独立维护的类型码副本，防止两个子 crate 漂移。
- 修改对象/数组布局时，应同步所有使用 `headerSize`、`keyEntrySize`、`valEntrySize` 的读写路径和落盘兼容测试。已有持久化或网络数据可能受影响，这类变更必须按协议迁移处理，不能只调整常量让当前测试通过。
- 新增标准 JSON 错误时，应同时增加 `JsonErrorKind`（若需要 Rust 分类）、`standard_json_error!` 调用和 `JSON_ERRORS_INIT::initialize` 解引用，并核对 `dbterror::errno` 名称、错误类与 Go `pkg/types/json_constants.go`。测试应继续放在独立 `pkg/types/json_constants_test.rs`，覆盖 code、class、消息和 kind 转换，不要嵌入生产源文件。
- 扩展比较类型集合时，应同步 Go `jsonTypePrecedences` 与 Rust `json_type_precedence`，覆盖大小写和未知输入，并先确认生产比较入口是否已经接线；当前只有测试调用证据，不能把扩展该函数等同于改变全部 Rust JSON 比较行为。
- 修改字符串安全表时，应用 0..127 全域测试核对双引号、反斜杠、控制字节和 `0x7f`，同时检查 `pkg/types/json_binary.rs` 的转义逻辑。若要处理非 ASCII，应扩展调用流程而不是直接用大于 127 的字节索引当前表。
- 修改公开常量名或导出路径会影响 `astersql-types-json-binary` 和根 crate 的使用者；应保持 `pkg/types/internal/json_binary/lib.rs` 的模块/re-export 边界，并同步直接消费者。主要风险是协议兼容、SQL 错误身份和持久化数据解释；性能风险则来自把常量查表改成运行时分配或锁定。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/types/json_constants.rs` 确认文件已索引且含 61 个符号；两次 `node --file` 读取了完整 268 行源码，并报告 8 个使用文件；`query` 确认 `json_type_precedence`、`JsonError`、`JSONTypeCode`、`JSON_ERRORS_INIT` 的定义。精确 `callers/callees json_type_precedence` 未返回可用边，已用精确符号检索补证，没有把泛化 `explore` 的同名噪声作为结论。
- 源码与装配：`pkg/types/json_constants.rs`、`pkg/types/internal/json_binary/lib.rs`、`pkg/types/lib.rs`。
- Cargo 边界：`pkg/types/Cargo.toml`。
- Go 对照：`pkg/types/json_constants.go`；间接行为测试 `pkg/types/json_binary_test.go`、`pkg/types/convert_test.go`。
- Rust 测试：`pkg/types/json_constants_test.rs`；代表性消费测试 `pkg/types/json_binary_test.rs`、`pkg/types/json_binary_functions_test.rs`。
- 生产调用证据：`pkg/types/json_binary.rs`、`pkg/types/convert.rs`、`pkg/types/datum.rs`、`pkg/types/internal/datum/lib.rs`、`pkg/util/chunk/row_in_disk.rs`。
- 按任务约束未运行 Cargo。交付前使用任务给定命令验证文档存在且恰有 11 个固定二级章节，并人工复核本文能回答文件存在理由、执行路径、状态边界和安全扩展位置。
