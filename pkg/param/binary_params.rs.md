# `pkg/param/binary_params.rs`

## 文件定位

本文件属于 `astersql-param` crate。crate 入口 [`pkg/param/lib.rs`](lib.rs) 将 `binary_params` 声明为私有模块，再用 `pub use binary_params::*` 对外导出本文件的三个公开名字：`BinaryParam`、`ErrUnknownFieldType` 和别名 `ERR_UNKNOWN_FIELD_TYPE`。因此调用者依赖的是 `astersql_param`/Cargo 别名 `param` 的公共 API，而不是直接访问该私有模块。

它位于 MySQL 预处理语句二进制参数链路的“解码结果载体”层：上游应先解析 COM_STMT_EXECUTE 的 null bitmap、类型标志和值字节，下游再把 `BinaryParam` 转成表达式。文件本身不读取网络、不解析字节，也不持有会话状态。当前 Rust 迁移尚未完全统一这条链路：[`pkg/expression/util.rs`](../expression/util.rs) 的 `ExecBinaryParam` 已直接消费本 crate 的类型，但 [`pkg/server/conn_stmt_params.rs`](../server/conn_stmt_params.rs) 和 [`pkg/server/conn_stmt.rs`](../server/conn_stmt.rs) 仍各自定义同名结构，不能把 Go 侧的端到端接线视为 Rust 侧现状。

## 核心职责

- `BinaryParam` 保存一个已经完成协议分段后的参数：MySQL 类型编号、无符号标志、NULL 标志和原始值字节。它刻意不承担类型转换，使协议解码与表达式构造解耦。
- `ErrUnknownFieldType` 延迟构造 server 错误类中的标准 8051 错误；`ERR_UNKNOWN_FIELD_TYPE` 为同一静态量提供 Rust 风格别名，兼顾 Go 标识符对齐和 Rust 调用习惯。
- `#[derive(Default)]` 保留 Go 结构体零值语义，便于先按参数个数建立空槽位再由解码器填充。

本文件不是完整的二进制协议实现。长度检查、字符集转换、日期/数字解释和表达式类型推断都属于相邻模块职责。

## 主要符号

- `pub static ErrUnknownFieldType: LazyLock<Box<terror::Error>>`：首次解引用时调用 `dbterror::ClassServer.NewStd(errno::ErrUnknownFieldType)`。错误码来自 `astersql-errno`，错误对象与 server 错误类来自 `astersql-util-dbterror` 和 `astersql-parser-terror`。
- `pub use ErrUnknownFieldType as ERR_UNKNOWN_FIELD_TYPE`：名称重导出，不创建第二个错误对象；两个名字指向同一静态量。
- `pub struct BinaryParam`：公开载体类型，字段也全部公开。
  - `Tp: u8`：MySQL 单字节字段类型编号。
  - `IsUnsigned: bool`：数值类型的 unsigned 标志；下游据此选择有符号或无符号解码。
  - `IsNull: bool`：参数值是否应按 NULL 处理。某些类型的下游转换会参考该字段。
  - `Val: Vec<u8>`：参数拥有的原始值字节；拥有所有权使载体不依赖网络缓冲区生命周期。
- `Default for BinaryParam`：由 derive 生成，得到 `Tp == 0`、两个布尔值均为 `false`、`Val` 为空向量。文件中没有手写函数、trait、`impl` 或条件编译项。

## 执行流程

典型的目标流程可分为四步：

1. 协议层读取 COM_STMT_EXECUTE 元数据和值区，判断 NULL、unsigned 和类型，并把值字节写入一个 `BinaryParam` 槽位。本文件只定义该槽位的数据契约。
2. 表达式层调用 [`pkg/expression/util.rs::ExecBinaryParam`](../expression/util.rs)，逐项匹配 `Tp`。整数和浮点按小端转换，日期/时间与 duration 经专用解析，字符串/二进制类型按各自语义生成 datum。
3. `ExecBinaryParam` 使用 `IsUnsigned` 决定整数符号解释，使用 `IsNull` 区分 decimal、blob 和字符串类的 NULL 分支，并读取或克隆 `Val`。
4. 每个 datum 经过参数类型推断后包装为常量表达式；未知 `Tp` 则通过本文件的 `ERR_UNKNOWN_FIELD_TYPE.GenWithStack(...)` 返回错误。

需要注意，以上是 `BinaryParam` 的消费流程，而不是本文件主动执行的流程。当前 Rust server 的解码函数 `parseBinaryParams` 使用 server 内部的 snake_case 同名结构，因此尚没有由该函数直接产出本文件类型的静态调用边；直接、已验证的生产下游是表达式 crate。

## 数据与状态

`BinaryParam` 是按值保存的普通结构，没有内部可变性、全局注册表或隐藏状态。`Val: Vec<u8>` 拥有数据，复制或跨层传递时的成本取决于调用者是否移动整个结构；本类型未实现 `Clone`，避免了隐式宣称便宜复制。它也没有实现协议合法性约束：例如固定宽度数值的 `Val` 长度是否足够、`Tp` 与 `Val` 是否匹配，均由生产者和消费者共同保证。

`ErrUnknownFieldType` 是文件内唯一的全局状态。`LazyLock` 保证错误对象至多初始化一次并在进程生命周期内存活；初始化参数由编译期错误码常量和静态错误类决定。别名 `ERR_UNKNOWN_FIELD_TYPE` 不增加状态。

重要不变量是：`Tp` 必须是下游支持的 MySQL 类型编号；固定宽度类型的 `Val` 必须有足够字节；NULL 参数不得要求下游解释无意义的值字节；`IsUnsigned` 只应影响适用的数值类型。本结构不自行执行这些检查。

## 依赖与调用关系

crate 边界由 [`pkg/param/Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-param`，库入口为 `lib.rs`，直接依赖如下：

- `astersql-errno`：提供 `errno::ErrUnknownFieldType == 8051`。
- `astersql-util-dbterror`：提供 `ClassServer.NewStd(...)` 标准错误工厂，经 `lib.rs` 的 `dbterror` 模块重导出。
- `astersql-parser-terror`：提供 `terror::Error` 以及错误生成能力，经 `lib.rs` 重导出。
- Rust 标准库 `std::sync::LazyLock` 和 `Vec`：分别管理一次性错误初始化和值字节所有权。

RustCodeGraph 将本文件识别为两个主要节点：`BinaryParam` 与 `ErrUnknownFieldType`。精确源码检索确认 [`pkg/expression/util.rs::ExecBinaryParam`](../expression/util.rs) 的参数类型为 `&[param::BinaryParam]`，并在未知类型分支调用 `param::ERR_UNKNOWN_FIELD_TYPE.GenWithStack(...)`。[`pkg/expression/Cargo.toml`](../expression/Cargo.toml) 以 `param = { package = "astersql-param", ... }` 引入该 crate；`pkg/server` 和 `pkg/session` 的 Cargo manifest 也声明了依赖，但仅有依赖声明不能证明它们当前直接使用本结构。

Go 的完整主链是 `pkg/server/conn_stmt_params.go::parseBinaryParams` 产生 `param.BinaryParam`，`pkg/expression/util.go::ExecBinaryParam` 消费它，并可由 `pkg/session/session.go` 进入表达式转换；Rust 当前只能确认表达式消费边，server 侧仍处于重复类型并存状态。

## 错误处理与边界

本文件不返回 `Result`，也不验证参数内容。唯一错误职责是提供标准未知字段类型错误模板。该模板的标准码为 8051、消息为 `unknown field type`、RFC 标识为 `server:8051`；实际带类型编号的上下文由 `ExecBinaryParam` 调用 `GenWithStack("stmt unknown field type %d", ...)` 时附加。

以下边界均不由 `BinaryParam` 防护：固定宽度数值的短缓冲、非法日期或 duration 长度、无效 UTF-8、协议类型与值不一致。当前 `ExecBinaryParam` 对部分固定宽度类型使用索引或切片后 `unwrap`，所以生产者满足长度不变量尤其重要；日期、duration 和未知类型有显式错误分支。扩展本类型时不能误以为增加字段即可自动强化这些验证，必须同步审查生产者和消费者。

`IsNull` 的语义也不是对所有类型完全统一：`TypeNull` 主要由 `Tp` 决定，decimal/blob/string 类分支会额外读取 `IsNull`。新增类型时应明确 NULL 是由类型号还是标志控制，并与 Go 行为保持一致。

## 并发与资源生命周期

`BinaryParam` 不含锁、任务、通道、事务或外部资源。其拥有的 `Vec<u8>` 随结构创建、移动和释放；没有借用网络输入，因此不会把包缓冲区生命周期传播到表达式层。类型没有显式 `Send`/`Sync` 实现，但全部字段由标准库可发送类型组成，编译器可自动推导相应 auto trait；是否跨线程传递由调用方决定。

`ErrUnknownFieldType` 的 `LazyLock` 使用线程安全的一次初始化：并发首次访问只发布一个完整的 `Box<terror::Error>`，之后所有调用方共享只读错误模板。文件本身没有清理阶段；静态错误存活到进程结束。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/param/binary_params.go`](binary_params.go)。字段映射为 `byte -> u8`、`bool -> bool`、`[]byte -> Vec<u8>`，字段名沿用 Go 的导出名称以保持迁移可识别性。Go 的结构体天然具有零值，Rust 通过 `Default` 明确得到同样的零值形状；独立测试覆盖了这一点。

Go 的 `ErrUnknownFieldType` 在包加载时构造；Rust 使用 `LazyLock` 推迟到首次访问，但错误类、错误码和最终标准文案保持一致。Rust 额外提供全大写别名以适应静态量命名习惯，同时保留 Go 名称供迁移代码使用。

语义上的主要迁移差异不是本结构字段，而是接线状态：Go server 的 `parseBinaryParams` 直接填充 `pkg/param.BinaryParam`，并由 expression/session 消费；Rust server 仍定义自己的 `BinaryParam`，字段名也是 snake_case，而 expression 已使用本 crate 的 PascalCase 类型。因此本文件是已实现且有直接下游的公共数据契约，但尚不能宣称 Rust server 到 expression 的类型链已经统一。

## 扩展指南

- 新增或改变参数元数据时，优先修改 `BinaryParam` 的字段契约，并同步检查 `pkg/param/binary_params.go`、`pkg/expression/util.rs::ExecBinaryParam`、server 侧两个同名结构及其解码/调用点；仅修改本文件会造成类型链漂移。
- 新增 MySQL 参数类型时，通常不需要扩大 `Tp`（它已能容纳单字节类型码），但必须同步实现协议长度/字符集解码、expression datum 转换、未知类型错误边界以及 Go 对照行为。
- 如要统一 Rust server 的重复结构，应进行显式迁移和转换审查，避免同时改变协议错误、字段命名、所有权或长数据优先级；这属于本说明任务范围外的后续工作。
- 回归测试应保持测试逻辑与生产文件分离。直接结构与错误契约测试位于 [`pkg/param/migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；协议生产端边界测试位于 [`pkg/server/conn_stmt_params_test.rs`](../server/conn_stmt_params_test.rs)；表达式转换若扩展类型，应在 expression 的独立测试文件中增加覆盖，而不要把测试内嵌进 `binary_params.rs`。
- 兼容风险集中在 Go/Rust 字段与 NULL 语义漂移、错误码或消息变化；正确性风险集中在 `Tp` 与 `Val` 长度不匹配；性能风险主要是无必要克隆 `Val` 或在层间重复转换。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/param` 列出本源文件、Go 对照、crate 入口和独立测试。
- RustCodeGraph `query BinaryParam --kind struct --json`：确认本文件结构节点位于第 31 行，并同时揭示 server 侧两个 Rust 同名结构；`query ExecBinaryParam --kind function --json` 确认 Go/Rust 消费函数；`explore` 确认 `binary_param_preserves_decoded_protocol_fields` 测试关联及参数链附近节点。对常见名直接运行 `callers/callees` 未获得可消歧的稳定结果，因此调用关系又以精确类型引用和 Cargo 声明核验。
- 已读取源码与配置：`pkg/param/binary_params.rs`、`pkg/param/lib.rs`、`pkg/param/Cargo.toml`、`pkg/expression/util.rs`、`pkg/expression/Cargo.toml`、`pkg/server/conn_stmt_params.rs`、`pkg/server/Cargo.toml`。
- 已读取 Go 对照：`pkg/param/binary_params.go`、`pkg/expression/util.go`、`pkg/server/conn_stmt_params.go`；并通过引用搜索定位 `pkg/session/session.go` 的 Go 消费入口。
- 已读取测试：`pkg/param/migration_aster_unit_test.rs` 验证字段保真、默认零值、错误码 8051、文案、RFC 标识及两个错误名字的一致性；`pkg/server/conn_stmt_params_test.rs` 提供 server 内部解码边界证据，但使用的是 server 自有结构。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求目标文档存在，且恰好具有本文的十一个固定二级标题。
