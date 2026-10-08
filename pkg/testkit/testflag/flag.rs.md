# `pkg/testkit/testflag/flag.rs` 逻辑说明

## 文件定位

`flag.rs` 是 `astersql-testkit-testflag` crate 的实现文件，为 Rust 测试工具提供与 Go `pkg/testkit/testflag` 同名的 `-long` 开关查询能力。crate 边界由 `pkg/testkit/testflag/Cargo.toml` 定义：库入口是同目录 `lib.rs`，没有声明第三方依赖，并通过 `package.metadata.porting.go-package` 标记其 Go 来源为 `pkg/testkit/testflag`。

`pkg/testkit/testflag/lib.rs` 以 `pub mod flag` 装入本文件，并用 `pub use flag::*` 将公开函数提升到 crate 根；根工作区的 `pkg/lib.rs::testkit::testflag` 又通过 `facade_testkit_testflag` 暴露该 crate。该文件不是数据库 SQL 请求主链的一部分，而是测试执行策略的基础辅助模块。

## 核心职责

本文件只负责一件事：从命令行参数中判断 `long` 布尔标志是否启用。

- `Long` 面向实际进程参数，是对 Go 同名 API 的兼容入口。
- `long_from_args` 将解析逻辑与进程全局参数分离，允许独立 Rust 测试用构造的参数序列确定性验证所有分支。
- `parse_go_bool` 识别 Go `strconv.ParseBool`/`flag` 语义所采用的有限布尔拼写集合。

它不注册参数、不修改环境、不缓存结果，也不负责跳过测试；是否跳过由上层 `pkg/util/skip/skip.rs::NotUnderLong` 决定。

## 主要符号

- `fn parse_go_bool(value: &str) -> Option<bool>`：私有纯函数。`1`、`t`、`T`、`true`、`TRUE`、`True` 映射为 `Some(true)`；`0`、`f`、`F`、`false`、`FALSE`、`False` 映射为 `Some(false)`；其他字符串返回 `None`。
- `pub fn long_from_args<I, S>(args: I) -> bool`：公开、泛型、可注入参数的核心解析器。`I` 只需实现 `IntoIterator<Item = S>`，元素只需实现 `AsRef<OsStr>`，因此测试可传字符串数组，实际入口可传 `std::env::ArgsOs`。
- `pub fn Long() -> bool`：公开兼容 API；保留 Go 风格大写命名并以 `#[allow(non_snake_case)]` 局部放宽 lint。函数把 `std::env::args_os()` 原样交给 `long_from_args`。

文件没有常量、结构体、枚举、trait、`impl` 或条件编译项。公开 API 是 `long_from_args` 和 `Long`；`parse_go_bool` 仅为文件内部实现。

## 执行流程

`Long` 的调用流程是：读取当前进程的 OS 参数 → 调用 `long_from_args` → 返回最终布尔值。

`long_from_args` 的具体流程如下：

1. 以 `false` 初始化局部状态 `long`，并用 `skip(1)` 跳过约定为可执行文件名的 `argv[0]`。
2. 每个 `OsStr` 通过 `to_string_lossy` 转为可比较文本。
3. 遇到裸 `-long` 或 `--long` 时把状态设为 `true`，继续扫描，因此后续合法同名标志可以覆盖它。
4. 遇到 `-long=<value>` 或 `--long=<value>` 时调用 `parse_go_bool`。合法值覆盖当前状态并继续；非法值将状态重置为 `false`，随后停止解析。
5. 遇到位置参数、`--`、未知标志或其他不匹配文本时立即停止，后续参数不再生效。
6. 参数耗尽或提前停止后返回当前 `long` 状态。

因此，多个合法 `long` 参数遵循“最后一个已解析值生效”；但只有出现在第一个停止点之前的参数才属于已解析范围。

## 数据与状态

全部状态都局限于单次 `long_from_args` 调用：一个初始为 `false` 的局部 `bool` 和当前参数的临时借用文本。函数不持有静态可变变量、全局注册表或堆上长期对象。

`long_from_args` 消费传入的迭代器；它不保存任何参数引用。`Long` 每次调用都会重新读取 `std::env::args_os()` 并重新解析，不会记忆上一次结果。该设计使 `long_from_args` 易测，但也意味着它不是 Go 包级 `*bool` 状态的逐字数据模型复刻。

## 依赖与调用关系

下游依赖仅来自 Rust 标准库：

- `std::ffi::OsStr` 作为参数元素的抽象边界；
- `std::env::args_os` 为 `Long` 提供当前进程参数；
- `IntoIterator`、`AsRef`、`Option`、`bool` 和字符串前缀操作完成解析。

RustCodeGraph 的精确符号边显示：`Long` 调用 `long_from_args`，`long_from_args` 调用 `parse_go_bool`。仓库引用搜索进一步确认，生产 Rust 侧的直接消费者是 `pkg/util/skip/skip.rs::NotUnderLong`：它调用 `testflag::Long()`，再把结果交给 `not_under_long_with`，在未启用长测试时调用测试上下文的 `skip`。

测试调用者位于 `pkg/testkit/testflag/flag_test.rs` 和 `pkg/testkit/testflag/migration_aster_unit_test.rs`。`pkg/testkit/testflag/lib.rs` 负责装配这些测试模块，并将本文件 API 再导出到 crate 根。

## 错误处理与边界

API 不返回 `Result`，也不 panic 来报告参数错误。无法识别的显式布尔值由 `parse_go_bool` 表示为 `None`；`long_from_args` 将其转换为 `false` 并停止。未知标志、位置参数、独立 `--` 和其他格式同样作为解析停止边界，但保留停止前已得到的状态（非法显式布尔值是例外，会先重置为 `false`）。

必须满足“首项是程序名”的调用约定；即使调用者没有提供真正的 `argv[0]`，首个元素仍会无条件被跳过。空序列和只有程序名的序列自然返回默认值 `false`。

非 UTF-8 OS 参数使用 `to_string_lossy` 转换；这避免转换失败，但可能用替换字符改变不可解码内容。只有转换后精确匹配受支持拼写的参数会生效。解析器不接受空格分离的显式值（例如 `-long false`）；裸布尔标志本身已经表示 `true`，下一项会按新的参数/停止边界处理。

## 并发与资源生命周期

三个函数都不创建线程、异步任务、锁、通道、文件、网络连接或数据库事务。`parse_go_bool` 和 `long_from_args` 只操作调用栈上的局部值；`Cow<str>` 形式的 `to_string_lossy` 临时值在当前循环迭代结束时释放。

由于没有共享可变状态，多个线程可并发调用 `long_from_args`；每次解析彼此独立。`Long` 依赖进程启动参数这一进程级只读输入，但本文件既不改变它，也不提供运行期注入接口；需要隔离测试时应调用 `long_from_args`，不要尝试改写进程环境。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/testkit/testflag/flag.go` 在包初始化时执行 `flag.Bool("long", false, "run long tests")`，保存返回的 `*bool`，`Long()` 仅解引用该指针。Rust 保留了 `Long() -> bool` 的名称、默认值和用户可见目的，但把“注册并由 Go 测试框架解析全局 FlagSet”改成了“每次从 `args_os` 解析”。

Rust 额外公开 `long_from_args`，这是为了在不修改进程全局参数的情况下覆盖解析行为，不是 Go 公共 API 的直接对应物。其布尔拼写、单双短横线、重复标志覆盖顺序和停止规则由源码注释及独立测试明确约束。

Go 侧当前没有同目录 `flag_test.go`；Rust 的迁移测试承担对照证据。Go 仓库中的 `pkg/util/skip/skip.go::NotUnderLong` 调用 `testflag.Long()`，对应 Rust 的 `pkg/util/skip/skip.rs::NotUnderLong` 调用链。其他 Go 测试（例如 `pkg/ttl/ttlworker/*_integration_test.go` 与 `pkg/ttl/cache/split_test.go`）也使用 Go `testflag.Long()`，但不能据此推断存在等量的 Rust 调用点。

## 扩展指南

- 新增或调整布尔字面量时，修改 `parse_go_bool`，并同步扩展 `pkg/testkit/testflag/migration_aster_unit_test.rs` 的真值和假值覆盖；不要让它与 Go 接受集合无依据地分叉。
- 修改参数扫描、重复覆盖或停止规则时，修改 `long_from_args`，并在 `pkg/testkit/testflag/flag_test.rs` 为位置参数、`--`、未知标志、非法值等边界补充回归用例。
- 修改实际进程入口时，优先保持 `Long` 只是薄封装，避免把不可注入的环境读取扩散到解析核心。
- 若要增加其他测试标志，应先判断是否应抽取共享解析器；同时检查 `pkg/util/skip/skip.rs` 中已有的 `short_from_args`/`parse_go_bool`，防止两份 Go 布尔语义继续漂移。
- 公共 API 变化需同步检查 `pkg/testkit/testflag/lib.rs`、根工作区 `pkg/lib.rs::testkit::testflag` 再导出，以及 `pkg/util/skip/skip.rs::NotUnderLong` 的兼容性。

主要正确性风险是偏离 Go `flag` 的停止和非法值行为；兼容性风险是更改 `Long` 名称、返回类型或 crate 根再导出；性能风险很低，但 `Long` 每次都会遍历进程参数并进行有损字符串转换，若未来进入高频路径应先测量再缓存。测试逻辑应继续放在独立的 `flag_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。

## 验证依据

- 目标实现：`pkg/testkit/testflag/flag.rs`；RustCodeGraph 文件节点列出 86 行源码及三个函数，精确节点确认调用边 `Long → long_from_args → parse_go_bool`。
- crate 与导出边界：`pkg/testkit/testflag/Cargo.toml`、`pkg/testkit/testflag/lib.rs`、根 `Cargo.toml` 的 workspace member/`facade_testkit_testflag` 声明，以及 `pkg/lib.rs::testkit::testflag`。
- Go 对照：`pkg/testkit/testflag/flag.go`；直接上游语义对照为 `pkg/util/skip/skip.go::NotUnderLong`。
- Rust 上游：`pkg/util/skip/skip.rs::NotUnderLong` 与 `not_under_long_with`；其 Cargo manifest 以路径依赖引用 `astersql-testkit-testflag`。
- 独立测试：`pkg/testkit/testflag/migration_aster_unit_test.rs` 验证默认值、裸标志、单双短横线、真/假字面量和后者覆盖前者；`pkg/testkit/testflag/flag_test.rs` 验证位置参数、`--`、未知标志和非法值停止行为。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；验收使用任务文件规定的十一章节结构命令，并人工复核所有行为结论均能回指上述符号或文件。
