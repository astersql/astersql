# `pkg/types/string.rs`

## 文件定位

[`string.rs`](string.rs) 定义一组面向字符串来源稳定性的轻量契约：一个返回拥有型字符串的 `String` trait，以及稳定来源 `PlainStr` 和可能来自可变缓冲区的 `HackedStr` 两个包装类型。文件位于 `pkg/types` 源码目录，但实际编译归属是 `astersql-types-file-group`：[`internal/file_group/lib.rs`](internal/file_group/lib.rs) 通过 `#[path = "../../string.rs"] pub mod string` 挂载它，[`internal/file_group/Cargo.toml`](internal/file_group/Cargo.toml) 为该子 crate 声明 `astersql-errors` 依赖；顶层 [`Cargo.toml`](Cargo.toml) 再依赖该子 crate，且 [`lib.rs`](lib.rs) 仅以 `pub use types_file_group as file_group` 暴露它。因此外部路径是 `astersql_types::file_group::string::*`，而不是 `astersql_types::string::*`。

本文件当前属于“已编译、已测试，但尚未接入 Rust 时间解析生产链”的兼容契约。仓库内非测试 Rust 代码没有实例化或调用 `PlainStr`、`HackedStr`；直接引用来自 [`string_test.rs`](string_test.rs) 和 [`overflow_10_aster_unit_test.rs`](overflow_10_aster_unit_test.rs)。不能据 Go 版本的使用情况推断 Rust 已完成相同接线。

## 核心职责

- `String` 将调用者提供的字符串来源统一为 `fn String(&self) -> std::string::String`，明确返回拥有值，避免把借用生命周期传播到错误或展示层。
- `PlainStr` 表示来源已经稳定的普通字符串；其 `String` 实现克隆内部 `std::string::String`。
- `HackedStr` 表示概念上可能别名可复用、可变缓冲区的字符串。它除实现本地 `String` trait 外，还同时提供固有 `FreezeStr` 方法并实现 `astersql_errors::HackedStr`，让错误系统在延迟格式化前取得快照。
- 本文件只负责包装、克隆和 trait 适配，不负责解析时间、构造错误、管理缓冲区，也不判断来源实际上是否安全；选择哪种包装由调用者负责。

## 主要符号

- `pub trait String`：单方法契约，`fn String(&self) -> std::string::String`。这里的名称遵循 Go API，因此与 Rust 标准库的 `String` 同名；方法返回值显式写成 `std::string::String` 以消除歧义。
- `pub struct PlainStr(pub std::string::String)`：公开字段的 newtype。派生 `Clone`、`Debug`、`Default`、`Eq`、`Hash`、`PartialEq`，可直接构造、比较、哈希或取出内部拥有值。
- `impl String for PlainStr`：`String()` 返回 `self.0.clone()`；原包装保持可复用。
- `pub struct HackedStr(pub std::string::String)`：与 `PlainStr` 具有相同的数据布局和派生能力，但类型身份表达“不安全来源”语义。
- `impl String for HackedStr`：提供普通展示/错误文本所需的拥有型克隆。
- `HackedStr::FreezeStr`：固有公开方法，克隆内部字符串，供直接调用者显式取得独立缓冲。
- `impl astersql_errors::HackedStr for HackedStr`：错误 crate 的适配实现，其 `FreezeStr` 同样克隆内部字符串。[`pkg/errors/core.rs`](../errors/core.rs) 中 `ErrorArg::from_hacked` 通过这个 trait 把快照存入 `ErrorArg::String`。

本文件没有模块级常量、枚举、自由函数、异步函数或条件编译项。

## 执行流程

稳定来源路径如下：调用者用拥有的标准字符串构造 `PlainStr`，再通过本地 `String::String` 取得克隆。调用前后包装中的原值不变；返回值与内部值内容相同，但拥有独立的字符串缓冲。

潜在可变来源路径有两种：

1. 普通读取调用 `HackedStr` 的本地 `String::String`，得到内部文本的拥有型克隆。
2. 错误冻结调用固有 `HackedStr::FreezeStr`，或把 `&HackedStr` 传给 `astersql_errors::ErrorArg::from_hacked`。后者依赖 `astersql_errors::HackedStr` 实现调用 trait 方法，再把结果保存为 `ErrorArg::String`。后续原始来源即使被复用，错误参数也持有自己的快照。

所有路径都是一次 `String::clone`；没有解析、验证、规范化或编码转换分支。两个同名 `FreezeStr` 分别是固有方法和外部 trait 方法，行为目前相同，但调用解析取决于调用方式和 trait 约束。

## 数据与状态

两个包装类型都只含一个公开的 `std::string::String`，自身不持有引用、裸指针或共享所有权。`Default` 产生空字符串；`Clone` 深复制字符串内容；`Eq`、`PartialEq` 和 `Hash` 均按内部字符串值工作。

“稳定”与“可能别名可变缓冲”是类型层面的来源标记，不是运行时状态：`PlainStr` 与 `HackedStr` 在 Rust 实现中都已拥有字符串，且本文件没有可变性标志或别名检查。尤其不能把 `HackedStr` 理解为真正持有外部可变缓冲的零拷贝视图；当前表示仍是拥有型 `String`，冻结操作再做一次克隆。

关键不变量是所有公开取值/冻结接口均返回拥有值，不向调用者暴露内部借用。代价是每次调用按字符串长度分配并复制；大字符串或热路径中的重复调用需要关注额外分配。

## 依赖与调用关系

上游装配链为：`pkg/types/internal/file_group/lib.rs` 挂载本文件 → `astersql-types-file-group` 编译该模块 → `pkg/types/lib.rs` 将子 crate 再导出为 `file_group`。`pkg/types/Cargo.toml` 自身没有直接声明 `astersql-errors`；真正满足本文件外部 trait 引用的是 `pkg/types/internal/file_group/Cargo.toml` 中的 `astersql_errors = { package = "astersql-errors", ... }`。

下游仅有两类操作：标准 `String::clone`，以及实现 `astersql_errors::HackedStr`。错误侧的实际消费入口是 `pkg/errors/core.rs::ErrorArg::from_hacked<T>`，它要求 `T: HackedStr` 并调用 `value.FreezeStr()`；本文件提供了满足该泛型边界的具体类型。

RustCodeGraph 对精确节点的结果显示，`PlainStr` 由 `string_test.rs::wrappers_match_go_string_contract` 实例化，`HackedStr` 还由 `string_test.rs::hacked_str_implements_errors_freeze_contract` 实例化。仓库文本引用补充确认 `overflow_10_aster_unit_test.rs::string_wrappers_match_go_string_and_freeze_contracts` 也直接验证两个包装。除这些测试外，当前没有生产 Rust 调用边。

## 错误处理与边界

本文件所有方法都返回普通 `std::string::String`，没有 `Result`、`Option`、显式错误分支或 panic 路径；在正常内存条件下唯一工作是克隆。分配失败遵循 Rust 分配器的进程级失败行为，并未在本模块单独处理。

空字符串、任意 UTF-8 内容和嵌入的 `\0` 都按 `std::string::String` 原样克隆；本文件不进行 SQL、时间格式或字符集合法性检查。它也不接受非 UTF-8 字节，因构造字段类型本身就是 Rust UTF-8 `String`。

错误冻结的边界在 trait 适配处：只有显式实现 `astersql_errors::HackedStr` 的类型才能传给 `ErrorArg::from_hacked`。仅实现本文件的 `String` trait并不足以触发冻结；调用者若把潜在不稳定来源误标为 `PlainStr`，本文件无法检测该错误分类。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、任务、通道、事务、文件句柄或网络资源。包装值的生命周期完全由 Rust 所有权决定；返回的克隆与原包装独立销毁。

由于字段是标准 `String`，两个包装在其成员满足条件时自动具备 `Send`/`Sync`，但文件没有显式声明或并发协议。冻结流程的生命周期边界是 `FreezeStr` 返回时：从此错误参数拥有独立值，不需要延长原包装的借用。当前实现虽然已经拥有内部字符串，仍通过二次克隆保持与 Go “构造错误时冻结快照”的可观察契约一致。

## 与 Go 版本的对应关系

直接对照文件是 [`string.go`](string.go)。符号一一对应：Go `String` interface 对应 Rust `String` trait，Go `type PlainStr string` / `type HackedStr string` 对应两个拥有型 newtype，三个 Go 方法对应 Rust 的两个 `String` 实现和 `HackedStr::FreezeStr`。Rust 额外实现 `astersql_errors::HackedStr`，把 Go 中依靠接口匹配的错误冻结能力显式接到 Rust trait 系统。

语义差异主要有三点：

- Go `PlainStr.String()` 和 `HackedStr.String()` 的 `string(s)` 转换通常不复制底层字节；Rust 方法签名要求返回拥有值，因此两个实现都会克隆。
- Go `HackedStr.FreezeStr()` 使用 `strings.Clone` 强制切断底层字节别名；Rust `HackedStr` 构造时已经拥有一个 `String`，`FreezeStr` 再克隆一次以保留独立快照语义。
- Go 生产链已经使用该抽象：`time.go::ParseTime`、`ParseTimeFromFloatString` 和 DST 调整路径构造 `PlainStr`，`ParseTimeWithString`、`parseTime`、`parseDatetime`、`adjustTimestampErrForDST` 接受 `String`，并把原包装传入警告/错误参数。当前 Rust 生产代码未发现对应调用，只有 file-group 装配和测试，因此 Rust 迁移状态是“契约与错误适配存在，时间解析接线未由本文件证据证明”。

Go 同目录未发现专门的 `string_test.go`；其生产使用点位于 `time.go`，相关时间行为测试集中在 `time_test.go`。Rust 的直接契约测试则独立放在 `string_test.rs` 和 `overflow_10_aster_unit_test.rs`，符合源文件与测试文件分离要求。

## 扩展指南

若新增字符串来源类型，先决定它是稳定来源还是需要冻结的来源：前者实现本地 `String` trait即可；后者除本地 trait 外，还应实现 `astersql_errors::HackedStr`，并在独立测试中验证冻结后的内容与存储独立性。不要把测试内嵌到 `string.rs`。

若改变返回类型为借用或写时复制表示，需要同步审查 `String` trait、两个包装实现、`ErrorArg::from_hacked` 的所有权边界和 Go 对照语义；这会影响 API 兼容性及延迟格式化安全。若只优化分配，也必须保留 `HackedStr` 冻结结果不随来源变化的不变量。

若将此契约接入 Rust 时间解析，应在真实时间解析入口选择 `PlainStr`/`HackedStr`，让原包装一直传到产生警告和错误的位置，而不是过早转成普通字符串；同时新增或扩展时间解析的独立测试，覆盖截断警告、错误延迟格式化和 DST 错误路径。该接线目前不属于本文件任务，不能仅凭现有类型存在宣称已支持。

修改本文件时至少同步 [`string_test.rs`](string_test.rs)；涉及深拷贝细节还应同步 [`overflow_10_aster_unit_test.rs`](overflow_10_aster_unit_test.rs)，涉及错误适配则检查 `pkg/errors/tests/normalize_generation_test.rs` 和 `pkg/errors/tests/api_parity_test.rs`。兼容风险包括公开路径、Go 风格命名和 trait 方法解析；性能风险主要是大字符串的重复克隆。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter pkg/types/string.rs` 确认目标文件已索引并识别 9 个符号。
- RustCodeGraph 源码与符号：`node --file pkg/types/string.rs --offset 1 --limit 400` 覆盖全部 74 行；`query PlainStr`、`query HackedStr`、`query FreezeStr` 及精确 `node string.rs::PlainStr`、`node string.rs::HackedStr` 核对了定义、实现和测试实例化边。通用 `FreezeStr` 图查询会命中多个同名符号，因此未把模糊结果当成本文件调用边。
- 编译与依赖边界：读取 `pkg/types/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/internal/file_group/Cargo.toml` 和 `pkg/types/internal/file_group/lib.rs`，确认 file-group 挂载、`astersql-errors` 依赖及顶层再导出关系。
- Go 对照：读取 `pkg/types/string.go`，并检查 `pkg/types/time.go` 中 `parseDatetime`、`ParseTime`、`ParseTimeWithString`、`parseTime`、`adjustTimestampErrForDST` 的实际调用链。
- 测试证据：读取 `pkg/types/string_test.rs` 与 `pkg/types/overflow_10_aster_unit_test.rs` 的字符串包装测试；读取 `pkg/errors/normalize.rs` 的 `HackedStr` trait和 `pkg/errors/core.rs::ErrorArg::from_hacked`，确认错误冻结消费者。
- 引用核验：用仓库搜索确认 `mod string` 仅由 file-group 装配，并区分生产引用与测试引用。任务是纯文档分析，按计划未运行 Cargo。
