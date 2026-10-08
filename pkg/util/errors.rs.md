# `pkg/util/errors.rs`

## 文件定位

`pkg/util/errors.rs` 属于 `astersql-util` crate；crate 根 `pkg/util/lib.rs` 通过 `pub mod errors` 将它公开为 `astersql_util::errors`。文件只依赖 Rust 标准库的 `std::error::Error`，不使用 `pkg/util/Cargo.toml` 中列出的第三方依赖。它是 Go `pkg/util/errors.go` 的 Rust 对照实现，提供从错误包装链取得最深根因的通用辅助函数。

当前仓库的 Rust 生产代码没有检索到 `OriginError` 调用；已确认的调用只在 `pkg/util/errors_test.rs` 和 `pkg/util/cpu_posix_1_aster_unit_test.rs`。因此它目前是已经公开接线、已有测试覆盖，但尚无生产调用证据的工具 API，而不是 SQL 请求主链上的必经节点。

## 核心职责

本文件只有一项职责：让调用方把一个可空的动态错误引用规约为同一条 `source` 链的最深错误。`OriginError` 保留 `None`，对非空输入反复调用 `Error::source`，直到当前错误不再报告下层来源。

该函数只做链遍历，不改变错误对象，不生成新错误，不补充上下文，也不判断错误类别。返回值仍是借用引用，因而根因对象的所有权和生命周期继续由调用方持有。

## 主要符号

- `pub fn OriginError<'a>(err: Option<&'a (dyn Error + 'static)>) -> Option<&'a (dyn Error + 'static)>`：唯一公开函数。显式生命周期 `'a` 表明返回引用来自输入错误或其 `source` 链，不能比输入借用活得更久；`'static` 约束的是错误对象内部不得借用非静态数据，并不表示返回引用本身具有静态生命周期。
- `std::error::Error`：唯一导入的 trait，提供 `source()`。文件没有模块级常量、结构体、枚举、额外 trait、`impl` 或条件编译项。

名称沿用 Go 的导出命名 `OriginError`，因此不符合 Rust 常见的 snake_case 风格；crate 根 `pkg/util/lib.rs` 通过 `#![allow(non_snake_case)]` 接受这一迁移期 API 形状。

## 执行流程

1. `let mut current = err?` 使用 `?` 解包输入。输入为 `None` 时立即返回 `None`，不会访问错误链。
2. 对非空输入执行 `current.source()`。
3. 如果得到 `Some(source)`，把 `current` 更新为该下层错误并继续循环；外层包装错误不会被返回。
4. 如果得到 `None`，说明已到当前链的叶子，返回 `Some(current)`。

`pkg/util/errors_test.rs::test_origin_error` 分别验证空输入、无 `source` 的叶子、一层包装和两层包装；`pkg/util/cpu_posix_1_aster_unit_test.rs::origin_error_follows_the_complete_source_chain` 另用 `thiserror` 派生的 `#[source]` 字段验证真实 trait 链能被穿透。

## 数据与状态

函数的全部可变状态是栈上的局部引用 `current`。每次循环只把它重新指向下一层 `dyn Error`，没有复制或移动错误值，也没有堆分配、缓存、全局变量或持久化状态。

返回值保持对象身份：叶子输入会原样返回该引用。`test_origin_error` 用 `std::ptr::eq` 验证这一点；包装输入则以最终错误文本 `"err1"` 验证已落到预期叶子。时间复杂度是 O(d)，其中 d 是 `source` 链深度；额外空间是 O(1)。

## 依赖与调用关系

- 上游模块接线：`pkg/util/lib.rs` 的 `pub mod errors` 编译并公开本文件；同一文件在 `cfg(test)` 下通过 `#[path = "errors_test.rs"] mod errors_test` 接入独立测试。
- 已确认 Rust 调用者：`pkg/util/errors_test.rs::test_origin_error` 与 `pkg/util/cpu_posix_1_aster_unit_test.rs::origin_error_follows_the_complete_source_chain`。RustCodeGraph 对 `pkg/util/errors.rs` 给出的反向使用证据指向后者，文本检索补齐了独立测试中的导入和调用。
- 下游调用：`OriginError` 只调用标准库 trait 方法 `Error::source()`；没有项目内函数调用或 I/O。
- crate 边界：`pkg/util/Cargo.toml` 声明包名 `astersql-util`、库入口 `lib.rs`、`autotests = false`。本实现无需 Cargo 依赖，但聚合测试依靠 crate 根的显式测试模块接线。
- Go 对照：`pkg/util/errors.go::OriginError` 调用 `github.com/pingcap/errors.Cause` 逐层取因；其调用生态比当前 Rust 版本广。不能据此推断 Rust 侧已经承接所有 Go 调用场景。

## 错误处理与边界

- `None` 映射为 `None`，对应 Go 测试中的 `OriginError(nil) == nil`。
- 无来源的错误直接返回自身；有来源的错误返回最深可见来源。函数本身不返回 `Result`，因为遍历过程没有声明可恢复失败。
- 函数不会保留外层错误的显示文本、类型上下文或回溯；调用方若仍需这些信息，应同时保留原始错误引用。
- 实现信任 `Error::source` 链最终终止，没有循环检测或最大深度。若自定义错误违反通常约定并形成自引用/循环来源，循环不会终止；新增通用防护前需要先明确与 Go 行为的兼容要求。
- 接口只接受 `dyn Error + 'static`。携带非静态借用的错误不能直接传入，这是 Rust 版本相对 Go 动态 `error` 接口的类型边界。

## 并发与资源生命周期

文件没有锁、原子变量、线程、异步任务、通道、事务、文件句柄或网络资源。函数只进行不可变借用读取，本身不引入共享可变状态；能否在线程间传递仍取决于具体错误类型是否实现 `Send`/`Sync`，本签名没有附加这两个约束。

生命周期 `'a` 将输出绑定到输入错误链的借用期。函数不取得所有权，也不负责释放资源；调用结束后局部引用消失，错误对象仍按其原所有者的生命周期销毁。

## 与 Go 版本的对应关系

Go `pkg/util/errors.go::OriginError` 从输入 `error` 开始，反复调用 `errors.Cause(err)`，直到 `Cause` 返回与当前错误相同的接口值。Rust 版本把这一协议映射为标准库 `Error::source()`：`Some` 表示还可解包，`None` 表示到达叶子。

两侧测试意图一致：`pkg/util/errors_test.go::TestOriginError` 和 `pkg/util/errors_test.rs::test_origin_error` 都覆盖 nil/`None`、叶子、一层包装、两层包装，并要求最终得到最初的 `err1`。差异在于 Go 依赖 `pingcap/errors` 的 `Cause` 约定并返回拥有接口值；Rust 依赖标准库 source 链并返回借用的 trait object。Rust 版本没有复刻 `pingcap/errors` 的全部包装、堆栈或类型匹配能力。

## 扩展指南

- 若要改变根因判定或增加停止条件，修改点集中在 `OriginError` 的循环；必须同步扩展独立测试 `pkg/util/errors_test.rs`，不要把测试内嵌回生产文件。
- 新增边界测试时优先覆盖自定义多层 `source`、叶子对象身份以及计划支持的特殊链形；若改变 `None` 或 `'static` 语义，还应对照更新 `pkg/util/errors_test.go` 所表达的兼容契约，或明确记录有意差异。
- 若生产代码开始使用该 API，应从 `astersql_util::errors::OriginError` 接入，并确认丢弃外层上下文是否符合日志、重试或分类需求。
- 若计划增加循环保护、深度限制或返回拥有值，应先评估 API 兼容性和 O(d) 遍历成本；当前无分配、O(1) 额外空间是值得保持的性质。
- 不要仅因 Go 调用很多就机械迁移调用点；应逐个确认相应 Rust 错误类型确实用 `source()` 表达与 Go `Cause` 相同的包装关系。

## 验证依据

- Rust 源与符号：`pkg/util/errors.rs`；RustCodeGraph `node --file` 确认文件共 33 行且唯一函数为 `OriginError`，`query OriginError --kind function` 区分出 Go/Rust 同名定义。
- 模块与 crate：`pkg/util/lib.rs` 的 `pub mod errors`、独立测试模块接线，以及 `pkg/util/Cargo.toml` 的包名、库入口和依赖声明。
- Rust 测试：`pkg/util/errors_test.rs::test_origin_error`；补充回归 `pkg/util/cpu_posix_1_aster_unit_test.rs::origin_error_follows_the_complete_source_chain`。
- Go 对照：`pkg/util/errors.go::OriginError` 与 `pkg/util/errors_test.go::TestOriginError`。
- 调用关系：RustCodeGraph 的文件使用关系和 `OriginError` 节点 trail，加上 `rg` 对 Rust 源的直接引用核验；未发现生产 Rust 调用者，未把 Go 侧广泛调用误记为 Rust 调用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证检查文档存在且恰有 11 个规定二级标题，并人工复核路径、符号、边界与扩展说明均可回溯到上述证据。
