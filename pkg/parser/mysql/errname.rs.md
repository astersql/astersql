# `pkg/parser/mysql/errname.rs`

## 文件定位

本文件属于 `astersql-parser-mysql` crate，由 [`pkg/parser/mysql/lib.rs`](lib.rs) 通过公开模块 `pub mod errname` 暴露。它把 [`pkg/parser/mysql/errcode.rs`](errcode.rs) 中的 MySQL、MariaDB 与 TiDB 数字错误码关联到默认英文消息模板及脱敏参数位置，是协议错误构造链中的只读元数据层，而不是错误格式化器、网络协议处理器或错误码注册器。

crate 边界由 [`pkg/parser/mysql/Cargo.toml`](Cargo.toml) 确定：库入口是 `lib.rs`，包名为 `astersql-parser-mysql`。本文件本身只使用 Rust 标准库的 `HashMap`、`LazyLock` 和同模块 `errcode::*`，没有直接使用该 Cargo 清单中的外部依赖。

## 核心职责

1. `ErrMessage` 表示一条尚未格式化的默认错误消息：`Raw` 保存 Go `fmt` 风格模板，`RedactArgPos` 保存格式化前需要脱敏的 0-based 参数下标。
2. `Message` 把借用的模板和下标切片复制成拥有所有权的 `ErrMessage`；它不解释占位符、不执行脱敏，也不产生最终错误文本。
3. `MySQLErrName` 在进程内首次访问时构造并缓存 `HashMap<u16, ErrMessage>`，供错误构造路径按数字错误码查询。
4. 映射内容保持 [`pkg/parser/mysql/errname.go`](errname.go) 的错误码、英文模板和分组语义，包括经典 MySQL 错误、MariaDB 扩展以及文件尾部的 TiDB 优化器/内存配额扩展。

因此，本文件回答“某个已知错误码默认使用什么模板、哪些参数需要脱敏”；SQLSTATE 由 [`pkg/parser/mysql/state.rs`](state.rs) 管理，参数格式化与最终 `SQLError` 构造由 [`pkg/parser/mysql/error.rs`](error.rs) 完成。

## 主要符号

- `pub struct ErrMessage { pub Raw: String, pub RedactArgPos: Vec<usize> }`：公开的消息元数据值。字段名保留 Go 风格；类型由 Go 的 `string`/`[]int` 对应为 Rust 的拥有型 `String`/`Vec<usize>`。
- `pub fn Message(message: &str, redact_args: &[usize]) -> ErrMessage`：公开构造辅助函数，通过 `to_owned` 与 `to_vec` 拷贝输入。空切片表达当前 Go 表项中的 `nil` 脱敏列表语义。
- `pub fn MySQLErrName() -> &'static HashMap<u16, ErrMessage>`：公开查询入口。函数体内的 `static MYSQL_ERR_NAME: LazyLock<_>` 保证表只初始化一次，返回值的 `'static` 生命周期允许所有调用者共享同一张只读表。
- `MYSQL_ERR_NAME` 是函数局部静态值，不是模块公开 API。其初始化闭包把 `(错误码, Message(...))` 数组消费并 `collect` 为 `HashMap`；若未来出现重复数字键，后出现的值会覆盖先出现的值，最终表长也会小于声明项数。

本文件没有 trait、impl、枚举、条件编译项或可变静态状态。

## 执行流程

典型路径如下：

1. 上游以数字错误码请求默认错误，例如 `error.rs::NewErr(err_code, args)` 或 `terror.rs::ErrClass::NewStd(code)`。
2. 调用 `MySQLErrName()`。首次调用触发 `LazyLock` 初始化闭包，逐项调用 `Message`，分配每个模板的 `String` 和每个脱敏位置的 `Vec<usize>`，再收集为哈希表；后续调用直接取得同一静态表的引用。
3. 调用者用 `u16` 错误码查表。`error.rs::NewErr` 对命中项读取 `Raw` 与 `RedactArgPos`，先脱敏参数再交给共享格式化器；未命中时改为直接拼接调用参数。`terror.rs::NewStd` 对缺失项使用 `expect`，因为其契约要求传入标准且已登记的 MySQL 错误码。
4. 本文件不参与 SQLSTATE 查询；`NewErr` 另从 `state.rs::MySQLState` 取状态码，并在缺失时回退到 `HY000` 对应的默认状态。

表初始化顺序不构成对外契约：数组保留 Go 声明顺序便于审阅，但收集后的 `HashMap` 迭代顺序不稳定，调用者应始终按错误码查询。

## 数据与状态

- 键为 `u16`，与 `errcode.rs` 常量以及 MySQL 协议数字错误码宽度一致；值拥有模板和脱敏下标，不借用源文件字面量。
- 源码静态计数显示 Rust 与 Go 当前都声明 944 个映射项。最终 `HashMap` 条目数还受数字键是否重复影响，不应仅凭声明数推断运行时长度。
- 当前 `errname.rs` 的所有映射项都向 `Message` 传入 `&[]`，对应 Go 文件中的 `nil`；也就是说数据结构和下游脱敏链已经存在，但这张默认表目前没有标记任何需要脱敏的参数位置。
- 表内容只在 `LazyLock` 初始化时写入，API 只返回共享不可变引用。`ErrMessage` 字段虽然公开，但通过 `&HashMap` 取得的值不能被调用者原地修改。
- 每个 `String`/`Vec` 只在首次初始化时分配，随后存活到进程结束；查表本身不再进行模板拷贝。

## 依赖与调用关系

直接下游依赖：

- `std::collections::HashMap`：按错误码查找消息。
- `std::sync::LazyLock`：线程安全的一次性惰性初始化。
- `super::errcode::*`：提供映射键；错误码数值的权威定义不在本文件。

已核对的上游 Rust 调用：

- [`pkg/parser/mysql/error.rs`](error.rs) 的 `NewErr`：查表后读取模板和脱敏位置，构造面向 MySQL 客户端的 `SQLError`。
- [`pkg/parser/terror/terror.rs`](../terror/terror.rs) 的 `ErrClass::NewStd`：查表并委托 `NewStdErr`，后者把模板、脱敏位置、MySQL code 和 RFC code 交给共享错误基础设施。
- [`pkg/parser/mysql/errcode_2_aster_unit_test.rs`](errcode_2_aster_unit_test.rs) 与 [`pkg/parser/mysql/unit_test.rs`](unit_test.rs)：验证构造器、共享单例、代表性模板及未知码缺失行为。

RustCodeGraph 能定位 `MySQLErrName`、`ErrMessage`，并给出 `NewErr`、两个测试函数等调用者；它还把 `error.rs` 的导入边列为上游。图查询对大段表数据本身没有展开，因此条目数量、Go 对照和测试断言使用源码与 `rg` 核验。

## 错误处理与边界

- `Message` 是无失败构造器；除内存分配失败这类进程级问题外，它不返回 `Result`，也不验证模板占位符与参数数量。
- `MySQLErrName` 对未知错误码返回查找缺失，由调用者决定策略。`error.rs::NewErr` 使用参数直出作为回退；`terror.rs::NewStd` 则把缺失标准码视为程序不变量被破坏并 panic。
- 模板使用 Go `fmt` 风格（如 `%s`、`%d`、`%v`、`%-.192s`）。本文件只保存原文；兼容性取决于 `error.rs`/`astersql-errors` 格式化器。改写标点、大小写、精度或占位符类型都可能改变客户端可见文本或类型不匹配诊断。
- `collect::<HashMap<_, _>>()` 不检测重复键。新增条目时必须同时核对 `errcode.rs` 的实际数值，避免不同常量别名静默覆盖。
- 当前独立测试 `errcode_2_aster_unit_test.rs::mysql_error_names_are_complete_and_shared` 断言运行时长度为 952，但 Rust 与 Go 源码静态计数均为 944 个声明项。这是现有证据间的不一致；本次纯文档任务未运行 Cargo，不能声称该断言通过，也不在本文件范围内修复它。

## 并发与资源生命周期

`LazyLock<HashMap<...>>` 使用标准库同步原语保证并发首次访问只执行一次初始化；其他线程等待初始化完成后共享同一引用。测试 `mysql_error_names_are_complete_and_shared` 用 `std::ptr::eq` 明确要求两次调用返回同一张表。

初始化完成后没有锁保护下的持续写操作，也没有任务、通道、事务、文件句柄或网络资源。表及其拥有的字符串/向量为进程级常驻数据，不主动释放。若初始化闭包发生 panic，`LazyLock` 的失败行为属于标准库边界；当前闭包只有内存构造和收集，没有显式可恢复错误路径。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/parser/mysql/errname.go`](errname.go)：

- Go `ErrMessage` 的 `Raw string`、`RedactArgPos []int` 对应 Rust `String`、`Vec<usize>`。
- Go `Message` 返回 `*ErrMessage`；Rust 返回拥有型值，再由静态 `HashMap` 统一持有。
- Go `MySQLErrName` 是包初始化时创建的可变全局 map；Rust 改为函数形式并用 `LazyLock` 首次访问初始化，只暴露不可变共享引用。
- 两端当前都声明 944 个键值项，首部经典错误、尾部 MariaDB/TiDB 扩展及抽样模板文本一致；两端当前的映射项都没有非空脱敏位置。
- Go 消费链见 [`pkg/parser/mysql/error.go`](error.go) 的 `NewErr` 与 [`pkg/parser/terror/terror.go`](../terror/terror.go) 的 `NewStd`，分别对应 Rust 同路径实现。

语义差异主要来自初始化时机和可变性，而不是消息内容：Go map 可被包内/外代码直接改写，Rust API 没有暴露可变引用；Go 在包加载时构造，Rust 在首次使用时构造。新增或同步条目时仍应以两端错误码、模板和脱敏位置三元组为比较单位。

## 扩展指南

新增或修改默认错误消息时：

1. 先在 `errcode.rs` 与 Go `errcode.go` 核对错误码常量及实际数值，再在 Rust/Go 的 `errname` 表同步键、完整英文模板与脱敏位置。
2. 保持模板占位符的顺序、类型、宽度/精度和换行完全兼容；如果参数含敏感信息，在两端设置相同的 0-based 脱敏下标，并增加格式化后的脱敏回归测试。
3. 防止重复数字键。Rust 收集过程会静默覆盖，而 Go map 字面量对重复的同一常量键通常更早暴露问题；常量别名仍需按数值检查。
4. 测试逻辑必须放在独立文件，不要嵌入 `errname.rs`。优先扩展 `errcode_2_aster_unit_test.rs` 验证表完整性、共享性和代表性条目；扩展 `error_test.rs` 验证最终模板格式化/脱敏；需要包级联动时扩展 `unit_test.rs`。
5. 若改变缺失码策略，应修改实际消费点 `error.rs::NewErr` 或 `terror.rs::ErrClass::NewStd`，而不是在这张数据表中加入伪回退项。

兼容性风险集中在客户端可见错误文本、占位符类型、脱敏遗漏和错误码覆盖；性能风险主要是首次初始化分配量与常驻内存。常规新增单项对后续查询仍是均摊常数时间。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`query ErrMessage --kind struct`、`query MySQLErrName --kind function` 定位到本文件；`node pkg/parser/mysql/errname.rs::MySQLErrName` 给出 `Message` 下游以及 `NewErr`、`mysql_error_names_are_complete_and_shared`、`parser_mysql_error_metadata_maps_known_codes` 等上游边。
- 生产源码：完整阅读 `pkg/parser/mysql/errname.rs`，并核对 `pkg/parser/mysql/lib.rs`、`pkg/parser/mysql/Cargo.toml`、`pkg/parser/mysql/error.rs`、`pkg/parser/terror/terror.rs`、`pkg/parser/mysql/errcode.rs` 的相关入口与调用。
- Go 对照：阅读 `pkg/parser/mysql/errname.go`、`pkg/parser/mysql/error.go`、`pkg/parser/terror/terror.go` 的对应结构和消费路径。
- 独立测试：阅读 `pkg/parser/mysql/errcode_2_aster_unit_test.rs`、`pkg/parser/mysql/unit_test.rs`、`pkg/parser/mysql/error_test.rs`；确认现有覆盖包括 `Message` 拷贝语义、单例地址、抽样模板、未知码以及默认格式化。
- 静态复核：`rg` 对 Rust/Go 表均计得 944 个声明项，且未发现表项传入非空脱敏下标；这与 Rust 测试中的长度 952 断言存在待核对差异。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构验证，并人工确认没有把静态计数矛盾写成已通过的运行时结论。
