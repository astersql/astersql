# `pkg/types/errors.rs`

## 文件定位

`pkg/types/errors.rs` 是 `astersql-types` crate 的类型系统错误目录。`pkg/types/lib.rs` 以 `#[path = "errors.rs"] pub mod errors` 挂载它，随后用 `pub use errors::*` 将本文件的符号重导出到 crate 根；因此类型转换、时间、枚举、JSON 和统计等上层代码可以通过 `types::ErrWrongValue` 或 `types::errors::ErrWrongValue` 使用同一错误模板。

crate 边界由 `pkg/types/Cargo.toml` 确定：包名为 `astersql-types`，本文件直接依赖 `astersql-util-dbterror` 和 `astersql-parser-types`，前者提供错误类、MySQL errno 与 `terror::Error`，后者提供需保留对象身份的 `ErrInvalidDefault`。本文件不执行 SQL，而是为实际转换和求值路径提供可稳定识别、生成参数化消息并转为 MySQL 协议错误的模板。

## 核心职责

1. 统一定义类型层的 MySQL 兼容错误模板，包括截断、越界、精度/标度、时间、枚举、JSON 路径和统计缺失等类别。
2. 通过 `standard_error!` 把“Rust 公开名称—`dbterror` 错误类—MySQL errno”映射收敛为同一种 `LazyLock<Box<dbterror::terror::Error>>` 声明。
3. 对“协议错误码”与“消息模板名称”不相同的四个对象使用 `NewStdErr` 显式组合，避免被 `NewStd(code)` 的默认消息表绑定。
4. 重导出 `parser_types::types::ErrInvalidDefault`，而不重建一个同码错误，以保留 Go 版本直接赋值所表达的单一实例语义。
5. 提供 `DateTimeStr` / `DateStr` / `TimeStr` / `TimestampStr` 四个稳定的类型名参数，供 `ErrWrongValue` 等模板生成与 Go 一致的文案。

## 主要符号

- `DateTimeStr` 、`DateStr`、`TimeStr`、`TimestampStr`: 值分别为 `"datetime"`、`"date"`、`"time"`、`"timestamp"` 的 `&'static str`。`pkg/types/datum.rs` 在时间戳和 duration 转换错误中把它们传给 `ErrWrongValue.GenWithStackByArgs` 。
- `standard_error!($name, $class, $code)`: 文件内部宏，生成公开的 `LazyLock<Box<terror::Error>>`，其初始化闭包执行 `dbterror::$class.NewStd(dbterror::errno::$code)`。宏在本文件中调用 29 次，本身不对 crate 外导出。
- `ErrInvalidDefault`: 使用 `pub use parser_types::types::ErrInvalidDefault` 原样重导出；`pkg/types/errors_test.rs::invalid_default_reuses_parser_types_error` 用 `std::ptr::eq` 验证两个路径指向同一错误对象。
- 29 个标准错误模板：其中 28 个归入 `ClassTypes`，覆盖 `ErrDataTooLong`、`ErrIllegalValueForType`、`ErrTruncated`、`ErrOverflow`、`ErrDivByZero`、宽度/精度/标度、年份/日期时间、分区统计、JSON 参数/路径等错误；`ErrTimestampInDSTTransition` 单独归入 `ClassExecutor`，映射 `ErrTimeStampInDSTTransition`。
- `ErrSyntax`: 错误码使用 `ErrParse`，消息取 `MySQLErrName[ErrSyntax]`。
- `ErrWrongValue`: 错误码使用 `ErrTruncatedWrongValue`，消息取 `MySQLErrName[ErrWrongValue]`。
- `ErrWrongValue2`: 错误码和消息都使用 `ErrWrongValue`对应项。
- `ErrWrongValueForType`: 错误码和消息都使用 `ErrWrongValueForType`对应项。

合计为 34 个错误模板（29 个宏生成、4 个显式 `LazyLock`、1 个重导出）。本文件没有 trait、struct、enum、普通函数、`impl` 或条件编译项。

## 执行流程

1. `pkg/types/lib.rs` 编译时挂载 `errors.rs` 并重导出其公开符号。此时 `LazyLock` 只建立静态容器，不执行错误构造闭包。
2. 上层转换路径首次解引用某个错误模板（例如 `pkg/types/datum.rs` 中的 `ErrDataTooLong.FastGen(...)`）时，该静态量的 `LazyLock` 执行且仅执行一次初始化闭包。
3. 对宏生成项，闭包调用 `dbterror::ErrClass::NewStd`；该方法从 `errno::MySQLErrName` 取标准消息，再委托 `NewStdErr`。对四个特殊项，本文件直接调用 `NewStdErr` 选定 errno 和消息模板。
4. `parser/terror::ErrClass::NewStdErr` 登记错误类与错误码，为错误附加 MySQL code、RFC code 和脱敏参数位置，然后返回装箱的 `terror::Error`。
5. 调用者通常不直接返回模板，而是用 `FastGen`、`FastGenByArgs` 或 `GenWithStackByArgs` 以运行时值填充模板，再把生成的错误传播或记录为 warning。`pkg/types/datum.rs` 同时展示了错误返回、warning 生成以及用 `terror::ErrorEqual` 匹配 `ErrOverflow` 的三种用法。
6. 在协议边界，`terror::ToSQLError` 根据已登记的错误类/码还原 MySQL code，并将格式化后的消息放入 `SQLError`。`pkg/types/errors_test.rs::test_error` 直接验证了常用子集的这条转换链。

## 数据与状态

- 四个类型名是编译期字符串常量，无可变状态。
- 每个本地错误模板是一个独立的 `LazyLock<Box<terror::Error>>`。完成初始化后，静态容器保有模板对象，调用者借用它生成具体错误，而不转移或替换模板。
- `ErrInvalidDefault` 不拥有新状态：它是 `parser_types` 已有静态对象的公开别名。
- 错误对象携带两类稳定身份：`ErrClass` 表示子系统，errno 表示 MySQL 错误码。除 `ErrTimestampInDSTTransition` 使用 `ClassExecutor` 外，本文件新建的模板都使用 `ClassTypes`。
- `parser/terror` 还维护全局的错误类—错误码注册表；它不在本文件中定义，但会在 `NewStd` / `NewStdErr` 初始化每个模板时被写入，并在 `ToSQLError` 时被读取。

## 依赖与调用关系

**上游。** `pkg/types/lib.rs` 是公开装配入口。RustCodeGraph 对目标文件的索引列出 `pkg/types/datum.rs`、`pkg/types/enum_4_aster_unit_test.rs`、`pkg/types/truncate_12_aster_unit_test.rs`、`pkg/expression/errors_35_aster_unit_test.rs` 和 `pkg/errctx/migration_aster_unit_test.rs` 五个直接使用文件。其中生产路径 `pkg/types/datum.rs` 在值截断、时间转换和溢出分支中生成 `ErrDataTooLong`、`ErrWrongValue`、`ErrOverflow`、`ErrTruncated`、`ErrMBiggerThanD` 等具体错误或 warning。crate 根重导出也使其他包可经 `types::*` 间接引用这些符号。

**下游。**

- `std::sync::LazyLock` 负责线程安全的延迟初始化。
- `dbterror::ClassTypes` / `ClassExecutor` 选择 RFC 错误分类，`dbterror::errno::*` 提供 MySQL code，`dbterror::errno::MySQLErrName` 提供消息及脱敏元数据。
- `dbterror::ErrClass::NewStd` 和 `NewStdErr` 最终委托 `pkg/parser/terror/terror.rs::ErrClass::NewStdErr`，完成注册和 `terror::Error` 构造。
- `parser_types::types::ErrInvalidDefault` 是唯一从另一 crate 原样重导出的错误模板。

RustCodeGraph 能正确索引文件和四个显式静态量，但 `standard_error!` 产生的符号没有全部展开为独立节点；因此宏生成项的上游证据由精确符号搜索和上述直接文件源码补充，不把全局同名 errno 当成本文件的调用者。

## 错误处理与边界

- 本文件定义“错误模板”，不决定具体路径将错误返回、降级为 warning 还是容错。该策略由 `datum.rs` 等调用者及其 `Context` 决定。
- `NewStd` 要求 errno 在 `MySQLErrName` 中存在标准消息；底层 `parser/terror::NewStd` 在缺失映射时会 `expect` 失败。新增宏项时不能只添加一个 errno 数字。
- `ErrSyntax`、`ErrWrongValue`、`ErrWrongValue2` 和 `ErrWrongValueForType` 是显式 errno/消息组合，其中前两个的 code 与消息索引故意不同。将它们改写为 `standard_error!` 会改变对外文案或协议码。
- `ErrTimestampInDSTTransition` 故意归于 `ClassExecutor` 而非 `ClassTypes`；错误码仍是 `ErrTimeStampInDSTTransition`。错误类参与 RFC code 和注册表匹配，不是可随意整理的分组标签。
- `ErrPartitionStatsMissing` 和 `ErrPartitionColumnStatsMissing` 被放在 types 包是 Go 源码为避免 import cycle 的边界选择；文件本身不实现统计构建逻辑。
- `terror::ToSQLError` 对未知类、未注册 code 回退为 `ErrUnknown`。因此不应用临时 `terror::Error` 替代这些已注册的公开模板。

## 并发与资源生命周期

`LazyLock` 保证每个模板的初始化闭包在并发首次访问中只执行一次，模板随进程生命周期存活，无显式释放。本文件不创建任务、通道、事务或 I/O 资源，也没有可变的业务数据。

初始化闭包本身并非纯构造：`dbterror::NewStd` 最终会写入 `parser/terror` 的全局错误注册表。`pkg/util/dbterror/terror.rs` 明确保留了 Go 的约束：`NewStd` 通常用于全局初始化，底层注册不是任意时刻可执行的操作；`pkg/parser/terror/terror.rs::initError` 在注册表已冻结时会 panic。Rust 的 `LazyLock` 将 Go 的包初始化时机改为首次解引用时机，所以安全扩展时必须确保新模板在全局注册冻结前被初始化，或由现有包初始化机制显式触发；本文件中没有额外的预热函数。

## 与 Go 版本的对应关系

`pkg/types/errors.go` 是逐项语义对照：四个类型名常量的名称与值相同；34 个错误符号的错误类、errno 与特殊消息选择相同；`ErrInvalidDefault = parser_types.ErrInvalidDefault` 在 Rust 中对应为 `pub use`。

主要实现差异如下：

- Go 的 `var` 块在包初始化时按顺序调用 `NewStd` / `NewStdErr`；Rust 因非 `const` 构造而使用每项独立的 `LazyLock`，在首次访问时构造。对外模板语义一致，初始化/注册时机不同。
- Go 通过重复的包级赋值声明各错误；Rust 用 `standard_error!` 去除 29 个同形声明的重复，特殊 errno/消息组合仍显式保留。
- Go 错误变量是 `*terror.Error`；Rust 是 `LazyLock<Box<terror::Error>>`，使用处通过 deref 获得 `terror::Error` 引用。

`pkg/types/errors_test.go::TestError` 和 `pkg/types/errors_test.rs::test_error` 覆盖相同的常用错误子集，都校验 `ToSQLError(...).Code == error.Code()`。Rust 额外的 `invalid_default_reuses_parser_types_error` 校验指针身份；`pkg/types/enum_4_aster_unit_test.rs` 还对所有 34 个错误与四个字符串执行了全量对照。

## 扩展指南

1. 新增类型错误前，先在 Go `pkg/types/errors.go` 和 errno/消息表中确认真实类别、协议码、消息占位符及脱敏位置；不要仅依符号名推断映射。
2. errno 与标准消息是同一项时，在 `pkg/types/errors.rs` 添加 `standard_error!(Name, ClassTypes, Errno)`；二者不同时仿照 `ErrSyntax` / `ErrWrongValue` 用 `NewStdErr` 显式构造。只有真实归属执行器时才使用 `ClassExecutor` 等其他类别。
3. 如果 Go 是对其他 crate 静态错误的直接别名，Rust 优先 `pub use` 原对象，并像 `invalid_default_reuses_parser_types_error` 一样添加身份测试，不要构造一个只是 code 相同的副本。
4. 同步更新独立测试 `pkg/types/errors_test.rs`；若是新公开模板，还应纳入 `pkg/types/enum_4_aster_unit_test.rs` 的全量 code 映射表。不应把测试内嵌到 `errors.rs`。
5. 在真实调用者的独立测试中验证消息参数、strict/warning 分支和 `ErrorEqual` 身份；仅检查编译或 `Code()` 不能证明格式化语义完整。
6. 兼容风险集中在 MySQL code、RFC class、消息模板及全局注册时机；性能风险很小，但新增 `LazyLock` 仍有一次性初始化和注册开销。新错误必须确认在 `terror` 注册表冻结前完成首次初始化。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/types/errors.rs` 确认目标文件存在且有 11 个索引符号。
- RustCodeGraph 源码/符号查询：`node --file pkg/types/errors.rs --offset 1 --limit 400`；`query ErrSyntax`、`query ErrWrongValue`、`query ErrWrongValue2`、`query ErrWrongValueForType`、`query DateTimeStr`；索引确认四个显式静态量与类型名常量的定义位置。
- RustCodeGraph 下游查询：`query NewStd`、`query NewStdErr`、`query ToSQLError`；`node --file pkg/util/dbterror/terror.rs --offset 1 --limit 164` 和 `node --file pkg/parser/terror/terror.rs --offset 280 --limit 135` 核对错误类、标准消息构造、注册冻结边界与 SQL 错误转换。
- 已读 Rust 源码/配置：`pkg/types/errors.rs`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`、`pkg/types/datum.rs` 的精确引用点。目标包下未找到 `doc.go`，因此以 crate 根 `lib.rs` 作为最近的模块合约。
- 已读 Rust 测试：`pkg/types/errors_test.rs`、`pkg/types/enum_4_aster_unit_test.rs`；并用精确搜索核对 `pkg/types/truncate_12_aster_unit_test.rs`、`pkg/expression/errors_35_aster_unit_test.rs`、`pkg/errctx/migration_aster_unit_test.rs` 中的相关边界证据。
- 已读 Go 对照：`pkg/types/errors.go` 和 `pkg/types/errors_test.go`，确认字符串、符号、错误类、errno、特殊消息以及常用 code 转换测试的对应关系。
- 文档结构按任务规定的 11 个二级标题组织；本任务是纯文档分析，按总计划不运行 Cargo。
