# `pkg/util/intest/assert_common.rs`

源码链接：[assert_common.rs](./assert_common.rs)。

## 文件定位

本文件是 `astersql-util-intest` crate 的断言公共内核，归属 `pkg/util/intest`。crate 入口 `pkg/util/intest/lib.rs` 无条件声明 `assert_common`，再根据 `test`、`intest`、`enableassert` 条件编译 `assert.rs` 或 `no_assert.rs`；因此，本文件不决定一次断言是否执行，而负责保存公共开关、承载断言参数、执行已经获准的检查并生成失败消息。

`pkg/util/intest/Cargo.toml` 表明该 crate 没有外部依赖，声明了空的 `intest` 与 `enableassert` feature。`lib.rs` 对外再导出 `AssertArg` 和 `EnableInternalCheck`，但 `doAssert`、`doAssertNoError`、`doAssertNotNil`、`doAssertFunc`、`assertionFailedMsg` 与 `sprintf` 都不会成为 crate 的公共 API。除正式 crate 外，`pkg/util/mathutil/lib.rs` 还通过 `#[path = "../intest/assert_common.rs"]` 复用本文件作为其内部模块，这是修改可见性、条件编译或模块相对路径时必须考虑的第二个编译上下文。

## 核心职责

1. `EnableInternalCheck` 保存进程内内部检查总开关；其初值由编译配置决定，而运行期读写由外层模块和调用方完成。
2. `AssertArg` 把 Rust 的字符串、整数、浮点和布尔值收敛为有限的消息参数集合，并为这些值提供显示、Go 类型名和格式动词兼容性判断。
3. `doAssert*` 把四类断言统一收敛到失败消息和 `panic!` 路径，同时保留 `AssertFunc` 的函数调用行为及 `AssertNoError` 的错误上下文。
4. `assertionFailedMsg` 与 `sprintf` 模拟本模块实际使用到的 Go `fmt.Sprintf` 子集，使移植后的 panic 文本保持可比；它不是通用格式化库。

本文件只处理“检查被执行以后”的语义。启用版 `pkg/util/intest/assert.rs` 在 `EnableAssert` 或 `EnableInternalCheck` 为真时进入这里；普通版 `pkg/util/intest/no_assert.rs` 仅在 `EnableInternalCheck` 为真时进入这里。

## 主要符号

- `pub static EnableInternalCheck: AtomicBool`：对外导出的全局开关。初值为 `cfg!(any(test, feature = "intest", feature = "enableassert"))`，即测试构建或两个断言 feature 任一开启时为真；该表达式生成布尔初值，本身不是条件编译掉符号。
- `pub enum AssertArg`：消息值的封闭表示，包含 `String(String)`、`Int(i64)`、`Uint(u64)`、`Float(f64)`、`Bool(bool)`。它实现 `Clone`、`Debug`、`PartialEq` 和 `Display`。
- `impl From<&str>`、`impl From<String>`、`impl From<bool>` 与 `impl_assert_arg!`：提供调用侧转换。宏为常见有符号整数、无符号整数和浮点类型生成 `From`；窄整数及平台整数通过 `as _` 归一到 `i64`/`u64`，`f32` 归一到 `f64`。
- `AssertArg::go_type_name`：给诊断文本返回固定 Go 风格类型名 `string`、`int64`、`uint64`、`float64` 或 `bool`。
- `AssertArg::accepts_verb`：规定 `%s` 只接受字符串、`%d` 只接受有/无符号整数、`%v` 接受任意变体；`%+v` 在 `sprintf` 的独立分支中接受任意变体。
- `pub(crate) fn doAssert(bool, &[AssertArg])`：条件为假时调用 `doPanic`，为真时无副作用返回。
- `pub(crate) fn doAssertNoError(Option<&dyn Display>, &[AssertArg])`：`Some(err)` 时把 `error is not nil: {err}` 作为额外说明；`None` 时返回。
- `pub(crate) fn doAssertNotNil<T>(Option<T>, &[AssertArg])`：只检查 `Option::is_some()`，借此表达 Rust API 中的 nil。
- `pub(crate) fn doAssertFunc(Option<fn() -> bool>, &[AssertArg])`：先断言函数存在，再调用函数并断言返回值。参数是函数指针，不接受捕获环境的闭包类型。
- `fn doPanic(&str, &[AssertArg]) -> !`：构造消息后 `panic!`，返回类型 `!` 明确表示失败路径不返回。
- `pub(crate) fn assertionFailedMsg(&str, &[AssertArg]) -> String`：组合固定前缀、用户消息、额外说明，并调用 `sprintf` 消费格式参数。
- `fn sprintf(&str, &[AssertArg]) -> String`：单遍扫描格式串，处理 `%%`、`%s`、`%d`、`%v`、`%+v` 以及缺参、类型不匹配和多余参数诊断。

## 执行流程

以启用版 `Assert(false, args)` 为例，流程如下：

1. `pkg/util/intest/assert.rs::Assert` 以 `Ordering::Relaxed` 读取 `EnableAssert` 与 `EnableInternalCheck`；任一为真才调用 `doAssert`。普通版 `no_assert.rs::Assert` 只读取后者。
2. `doAssert` 检查条件；真值立即返回，假值以空的额外说明调用 `doPanic`。
3. `doPanic` 调用 `assertionFailedMsg`。后者先建立 `assert failed`；没有用户参数时可直接追加非空 `extra_msg` 并返回。
4. 有用户参数时，第一个 `AssertArg` 无论变体都通过 `Display` 转成格式串/消息主体；非空 `extra_msg` 在它之后以 `, ` 拼接。剩余参数交给 `sprintf`。
5. `sprintf` 从左到右扫描 Unicode 字符：普通字符原样写入；`%%` 写一个 `%`；受支持动词消费一个参数；缺参写 `%!<verb>(MISSING)`；类型不匹配写 `%!<verb>(<go-type>=<value>)`；格式串结束后仍有参数则追加 `%!(EXTRA ...)`。
6. 最终字符串成为 `panic!` payload。

其他入口只改变进入该公共尾部前的判断：`doAssertNoError` 为 `Some(err)` 生成额外说明；`doAssertNotNil` 将 `Option` 映射为布尔条件；`doAssertFunc` 先阻止空函数指针被调用，再执行函数。若函数本身 panic，该 panic 不被捕获，会原样向上传播，`pkg/util/intest/assert_test.rs::test_assert_func` 对此有明确覆盖。

## 数据与状态

唯一持久共享状态是 `EnableInternalCheck: AtomicBool`。本文件只定义及初始化它，不主动修改它；`assert.rs` 和 `no_assert.rs` 读取它，测试及少数外部调用方可以通过再导出的静态量执行 `store`/`swap`。初值由构建配置固定：普通无 feature 构建为假，测试、`intest` 或 `enableassert` 构建为真。

`AssertArg` 拥有其字符串，数值和布尔值按值保存；断言函数只借用参数切片，不保留引用。`assertionFailedMsg` 和 `sprintf` 每次创建新的 `String`，没有缓存或全局格式状态。`doAssertNotNil<T>` 按值接收 `Option<T>`，因此调用后值会被消费；其语义是 Rust 的 `Some`/`None`，并不进行 Go `reflect.Value.IsNil` 那种对接口内 typed-nil 的二次反射检查。

## 依赖与调用关系

直接标准库依赖只有 `std::fmt::{self, Display}` 与 `std::sync::atomic::AtomicBool`。Cargo 清单没有第三方依赖。

上游直接调用边由源码与局部搜索确认：

- `pkg/util/intest/assert.rs::{Assert, AssertNoError, AssertNotNil, AssertFunc}` 分别调用同名 `doAssert*`；该模块用于测试、`intest` 或 `enableassert` 变体。
- `pkg/util/intest/no_assert.rs` 暴露相同 API，也调用相同 `doAssert*`，但只在内部检查开关开启时下沉。
- `pkg/util/intest/assert_common_test.rs` 直接调用 crate 私有的 `assertionFailedMsg` 验证格式诊断。
- `pkg/util/intest/lib.rs` 将 `AssertArg`、`EnableInternalCheck` 再导出给依赖 crate。实际业务调用者通常调用外层 `Assert*`，例如 `pkg/expression/scalar_function.rs` 的非空检查、`pkg/util/chunk/chunk_util.rs` 的条件检查和 `pkg/table/tblctx/buffers.rs` 的非空检查，随后才间接进入本文件。
- `pkg/util/mathutil/lib.rs` 直接以路径复用本文件；`pkg/util/mathutil/math.rs::Divide2Batches` 使用其中的 `AssertArg` 组装断言消息。

下游调用关系完全在本文件内：`doAssert`、`doAssertNotNil` 和 `doAssertFunc` 形成条件检查链；所有失败最终到 `doPanic -> assertionFailedMsg -> sprintf`。`Display::fmt`、`go_type_name` 和 `accepts_verb` 为格式化提供值文本及诊断元数据。

## 错误处理与边界

本模块没有 `Result` 返回路径；违反断言即 panic。`doAssertNoError` 只要求错误实现 `Display`，不保留具体错误类型或错误链。用户未提供消息时，普通条件失败文本恰为 `assert failed`；错误断言则为 `assert failed, error is not nil: <显示文本>`。

格式化边界如下：

- 只支持 `%%`、`%s`、`%d`、`%v`、`%+v`。宽度、精度、索引参数以及其他 Go `fmt` 动词均未实现，不能据此声称完整兼容 `fmt.Sprintf`。
- 单独的 `%` 或未知动词不会消费参数；当前实现先保留 `%`，后续字符在下一轮按普通字符处理。`%+` 后不是 `v` 时写出 `%+`，且检查过程中读到的下一个字符已被消费。这是当前实现事实，新增格式能力时需要专门回归。
- `%s` 遇到非字符串、`%d` 遇到非整数会输出 Go 风格 `%!` 诊断；缺参和多余参数也生成诊断而非二次 panic。`assert_common_test.rs` 覆盖了这些情况。
- `%+v` 当前与 `%v` 都只调用 `Display`，不会产生 Rust `Debug` 的结构展开。
- `AssertArg` 不支持任意用户类型；调用方必须先转成现有变体或扩展枚举及其全部匹配分支。
- `doAssertFunc` 的 `expect("assert function checked")` 位于已完成 `is_some()` 断言之后；在当前同步控制流中 `Some` 不会在两步之间改变。若函数返回假则产生断言 panic，若函数自身 panic则直接透传。

## 并发与资源生命周期

`EnableInternalCheck` 使用原子量，因此并发读写不会产生数据竞争；具体读操作位于外层 `assert.rs`/`no_assert.rs`，使用 `Ordering::Relaxed`。该开关只表达是否执行检查，不承担跨线程数据发布或顺序同步，所以当前实现没有更强内存序。多个线程可同时改变开关，单次调用看到哪一个时刻的值取决于原子读取，模块不提供作用域守卫或自动恢复机制。

`pkg/util/intest/assert_test.rs` 使用进程内 `TEST_LOCK: Mutex<()>` 串行化测试对 `EnableAssert` 和 `EnableInternalCheck` 的修改，并在相关测试结尾恢复开启状态；这说明直接修改全局开关的测试必须避免互相干扰。格式化和断言执行没有锁、后台任务、通道、文件句柄或网络资源，所有临时 `String` 与迭代器在调用结束或 panic 展开时释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/intest/assert_common.go`。Rust 保留了 Go 的 `EnableInternalCheck`、`doAssert`、`doAssertNoError`、`doAssertNotNil`、`doAssertFunc`、`doPanic` 和 `assertionFailedMsg` 职责，并以 `AssertArg` 加局部 `sprintf` 替代 `...any` 与 `fmt.Sprintf`。

主要一致点：失败前缀为 `assert failed`；用户参数首项充当格式串/消息；错误说明插在用户消息后；函数断言先检查 nil 再调用；函数内部 panic 不拦截；常用 `%s`、`%d`、`%v`、`%+v` 以及缺参、错型、多参诊断对齐目标测试。

已确认的差异与迁移边界：

- Go 的 `EnableInternalCheck` 初始为假，并在 `init()` 中结合 `InTest`、`EnableAssert` 和 failpoint 修改；Rust 直接用编译期 `cfg!` 设置原子初值，本文件没有 failpoint 初始化逻辑。
- Go 变参接受任意类型，且首参非字符串时用 `%+v` 转换；Rust 首参已经是有限的 `AssertArg`，始终可显示。
- Go `doAssertNotNil` 通过反射识别接口中函数、通道、映射、指针、切片等 typed nil；Rust API 用 `Option<T>` 表示空值，只检查 `None`。
- Go 接受任意 `func() bool` 值；Rust 签名是 `Option<fn() -> bool>` 函数指针，不直接容纳捕获闭包。
- Rust `sprintf` 只实现当前断言所需子集。Go `fmt.Sprintf` 的全部格式语义不属于现有 Rust 实现。

Go 行为测试 `pkg/util/intest/assert_test.go::TestAssert` 覆盖条件、消息插值、非空、函数及错误断言；Rust 对应覆盖分布在 `pkg/util/intest/assert_test.rs` 和 `pkg/util/intest/assert_common_test.rs`，测试逻辑与源文件分离。

## 扩展指南

新增消息参数类型时，应同时修改 `AssertArg` 变体、`Display`、`go_type_name`、`accepts_verb` 与相应 `From` 实现，并在 `pkg/util/intest/assert_common_test.rs` 增加该类型的正常格式、错动词、缺参及多参用例。不要只让类型可转换而遗漏 `%!` 诊断类型名。

新增格式动词或标志时，接入点是 `sprintf` 的 `%` 分支。修改前应先用 Go `fmt.Sprintf` 明确目标文本，再扩充独立测试；尤其要覆盖未知动词、尾随 `%`、参数是否被消费以及 Unicode 格式串。若目标是完整 Go 格式兼容，应评估使用成熟格式化实现，而不是无边界扩张当前小型解析器。

新增断言种类时，推荐保持现有分层：在 `assert.rs` 与 `no_assert.rs` 提供同签名外层 API 并处理开关，在本文件增加 crate 私有的 `doAssert*` 公共行为，再由 `lib.rs` 按两个变体一致地导出。相关测试放在独立的 `*_test.rs` 文件，不嵌入生产源文件；同时核对 Go `assert_common.go` 与 `assert_test.go` 的语义。

修改 `EnableInternalCheck` 时要评估默认 feature、运行期原子访问以及测试隔离。修改本文件还必须验证 `astersql-util-intest` 和 `pkg/util/mathutil/lib.rs` 的路径复用上下文。兼容性风险集中在 panic 文本、公开 `AssertArg` 枚举和开关默认值；性能风险较低，但失败路径会分配字符串，成功路径应继续保持只做条件判断而不格式化消息。

## 验证依据

- 生产源码：`pkg/util/intest/assert_common.rs`（全部 230 行）、`pkg/util/intest/assert.rs`、`pkg/util/intest/no_assert.rs`、`pkg/util/intest/lib.rs`。
- crate 与复用边界：`pkg/util/intest/Cargo.toml`、`pkg/util/mathutil/lib.rs`、`pkg/util/mathutil/math.rs::Divide2Batches`。
- Rust 独立测试：`pkg/util/intest/assert_common_test.rs` 验证格式缺参、类型错误与多余参数；`pkg/util/intest/assert_test.rs` 验证四类断言、panic 透传与双开关行为；`pkg/util/intest/not_in_unittest_test.rs` 提供普通构建开关状态的补充证据。
- Go 对照：`pkg/util/intest/assert_common.go` 与 `pkg/util/intest/assert_test.go::TestAssert`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/intest` 找到 16 个相关源文件；`node --file pkg/util/intest/assert_common.rs --offset 1 --limit 260` 返回完整源码并报告该文件被 9 个文件使用；`query` 分别定位 Rust 的 `doAssert*`、`assertionFailedMsg` 和 `EnableInternalCheck`。精确 `callers/callees` 命令在 30 秒窗口内未返回，因此调用边进一步由上述模块源码及 `rg` 局部搜索交叉核对，未把超时结果当作调用事实。
- 人工复核结论：该文件存在的原因是让启用/禁用断言变体共享检查和消息语义；运行路径是外层开关判断后进入 `doAssert*`，最终统一格式化并 panic；安全扩展必须同步公共参数表示、格式诊断、两个外层变体、路径复用上下文及独立测试。
