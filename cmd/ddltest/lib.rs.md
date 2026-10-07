# `cmd/ddltest/lib.rs`

## 文件定位

[`cmd/ddltest/lib.rs`](lib.rs) 是 Cargo 包 `astersql-cmd-ddltest` 的库 crate 根。`cmd/ddltest/Cargo.toml` 以 `[lib] path = "lib.rs"` 指定该入口，并把包标为 `kind = "test-only"`；根工作区 `Cargo.toml` 将 `cmd/ddltest` 列为 workspace member。它不是 TiDB 服务或 DDL 引擎的生产入口，而是 Go `cmd/ddltest` 测试迁移后的装配门面。

文件本身只有 48 行，不实现 SQL、DDL 或测试断言。实际测试支撑逻辑位于 [`stubs.rs`](stubs.rs)，测试场景位于同目录的五个 `*_test.rs` 和额外的 [`parity_test.rs`](parity_test.rs)；`lib.rs` 只确定这些代码在库构建和测试构建中的可见方式。

## 核心职责

本文件承担四项边界职责：

1. `extern crate self as astersql_cmd_ddltest` 为当前 crate 建立稳定自别名，使同一批测试源码无论作为 `[[test]]` 独立目标还是作为库内 `cfg(test)` 模块编译，都能使用 `astersql_cmd_ddltest::…` 路径。
2. `#[path = "stubs.rs"] pub mod stubs` 挂载并公开内存 DDL/SQL 测试桩。
3. `pub use stubs::*` 把桩模块的公开项再导出到 crate 根，保留接近 Go 包级符号的访问形态；现有测试主要仍显式使用 `astersql_cmd_ddltest::stubs::{…}`。
4. 五个 `#[cfg(test)] #[path = "…_test.rs"] mod …` 声明只在库的测试配置下挂载列、通用 DDL、索引、公共初始化和随机辅助测试。Cargo 另以六个 `[[test]]` 目标独立编译这些文件，其中第六个是未在本文件内挂载的 `parity_test.rs`。

crate 级 `#![allow(...)]` 允许迁移代码中的 Go 风格命名、未使用项和 Clippy 告警。该属性影响从本根模块编入的子模块，但不代表新增代码可以忽略可读性或正常 Rust 约定。

## 主要符号

- `extern crate self as astersql_cmd_ddltest`：crate 自别名；不是外部依赖，也不产生第二份 crate 实例。
- `pub mod stubs`：本文件唯一公开模块声明，真实源码为 `cmd/ddltest/stubs.rs`。其公开表面包括 `DdlSuite`/`Suite`、`create_ddl_suite`、`Datum`、`SuiteOps`、随机辅助函数以及租约、数据规模等常量。
- `pub use stubs::*`：通配再导出，把 `stubs` 的所有公开项提升至 crate 根。新增公开桩会自动扩大 crate 根 API，存在意外暴露风险。
- `mod column_test`、`mod ddl_test`、`mod index_test`、`mod main_test`、`mod random_test`：五个私有、仅 `cfg(test)` 存在的测试模块，分别映射同名 Rust 文件和 Go 测试主题。

本文件没有函数、常量、类型、trait、`impl`、宏定义、可变静态数据或 feature gate；唯一条件编译条件是 `cfg(test)`。

## 执行流程

该文件没有生产运行流程，只有编译装配流程：

1. Cargo 构建库时，以 `lib.rs` 为 crate 根，注册自别名，编入并公开 `stubs.rs`，再将桩的公开项提升到 crate 根。
2. Cargo 构建某个 `[[test]]`（例如 `ddl_test.rs`）时，测试目标通过 `use astersql_cmd_ddltest::stubs::{…}` 连接库中的共享桩；测试文件本身作为独立测试 crate 编译。
3. Cargo 以库的 `cfg(test)` 配置编译时，本文件还会把五个测试文件作为内部模块挂载。自别名使这些源码中相同的绝对 crate 路径继续成立。
4. 测试入口调用 `create_ddl_suite()`，下游 `DdlSuite::create()` 建立内存引擎、三个伪 server 和重启线程；测试执行同步 SQL或通过 `run_ddl` 的 channel 等待异步 DDL，最后调用 `teardown()` 回收线程和 server。

`parity_test.rs` 只由 Cargo 的独立 `[[test]]` 声明接入，不经过本文件的 `mod` 声明；因此“库内测试集合”和“Cargo 全部测试目标”并不完全相同。

## 数据与状态

`lib.rs` 自身不拥有或修改运行时数据。它只建立模块命名空间和再导出关系。实际状态全部在 `stubs.rs`：`DdlSuite` 以 `Arc<Mutex<Engine>>` 保存内存表状态，以 `Arc<Mutex<Vec<Option<Arc<MockServer>>>>>` 保存伪 server，以 `Arc<AtomicBool>` 发出退出信号，并用 `Mutex<Option<JoinHandle<()>>>` 管理后台重启线程；`Suite` 是 `Arc<DdlSuite>` 类型别名。

随机扰动状态是 `stubs.rs::RAND_STATE: AtomicU64`。DDL 结果由 `std::sync::mpsc::Receiver<Result<(), String>>` 返回。上述状态因 `pub mod stubs` 和 `pub use stubs::*` 可从 crate 外测试访问，但它们均不在本文件初始化。

## 依赖与调用关系

编译期模块边为 `lib.rs → stubs.rs`，测试配置下再增加 `lib.rs → {column_test.rs, ddl_test.rs, index_test.rs, main_test.rs, random_test.rs}`。Cargo 目标边还包括六个独立测试目标到库 crate，其中 `parity_test.rs` 直接导入 `create_ddl_suite`、`random_intn`、`random_num` 和 `random_string`。

关键下游调用链由测试源码核验：各场景从 `astersql_cmd_ddltest::stubs` 导入共享符号；`column_test.rs`、`ddl_test.rs`、`index_test.rs` 和 `parity_test.rs` 调用 `create_ddl_suite()`；该函数调用 `DdlSuite::create()`，后者构建内存测试环境。`main_test.rs` 调用 `setup_test_main()`；`random_test.rs` 调用五个随机辅助函数。

`cmd/ddltest/Cargo.toml` 的 `[dependencies]` 和 `[dev-dependencies]` 当前均为空；注释明确说明 arm64 Darwin 环境没有 `kv/domain/kvproto/grpcio`，由 crate 内桩替代 TiKV、session、domain、MySQL 和进程边界。因此这个 crate 当前不接入真实 DDL 子系统，也不能作为真实集群行为的直接证明。

RustCodeGraph 能索引该文件及相邻符号，但对模块再导出和独立测试 crate 的 caller/callee 边没有给出可靠精确结果；这里的直接关系同时由 `node --file` 展示的导入/调用表达式和 `rg` 的 crate 名引用核验。

## 错误处理与边界

本文件没有运行时错误分支。错误边界主要有两类：路径或模块不存在会在编译期失败；公共模块/再导出改变会在依赖该 API 的测试目标编译期失败。

下游桩同时使用 `Result<_, String>` 和 panic：`DdlSuite::exec`、`query`、`run_ddl` 传播字符串错误，`must_exec` 及部分断言辅助在失败时 panic；锁中毒由 `.unwrap()` 转为 panic。随机 helper 对非法上界或负长度 panic，`parity_test.rs` 用 `#[should_panic]` 固定这些 Go 对齐边界。该 parity 文件还验证删表后查询失败、未知投影报错、`IF NOT EXISTS` 保留已有表以及重复建表报错。

需要特别区分“桩的行为契约”和“真实 TiDB 行为”：`stubs.rs` 明确只覆盖这些迁移测试观察到的 SQL/DDL 子集，诸如真实事务、KV range GC、外部 server 进程和完整 schema 状态机均被省略或占位。

## 并发与资源生命周期

`lib.rs` 不创建线程、锁、通道或资源句柄，但它公开的 `stubs` 决定了测试可使用的生命周期接口。`DdlSuite::create()` 启动一个后台随机重启线程；`run_ddl()` 为每次 DDL 启动线程并以 channel 返回结果；测试用例还创建并发写入 worker。`DdlSuite::teardown()` 设置退出原子标志、取出并 `join` 重启线程，再清空伪 server 列表。

`column_test.rs`、`ddl_test.rs` 和 `index_test.rs` 都定义独立的 `TeardownGuard`，其 `Drop` 实现在正常退出或 panic 展开时调用 `teardown()`，并有专门测试验证该清理路径。扩展测试时必须维持这一回收方式；仅在测试末尾手工清理会在断言 panic 时泄漏后台线程。

Go 版本管理真实 `kv.Storage`、`domain.Domain`、session、数据库连接、server 子进程、`WaitGroup` 和退出 channel；Rust 桩把这些资源折叠为单进程内存对象。因此并发节奏和清理顺序是对照目标，真实进程/KV 资源语义不是当前 crate 的覆盖范围。

## 与 Go 版本的对应关系

Go `cmd/ddltest` 没有对应的非测试 `lib.go`：五个 `.go` 文件都属于同一 `package ddltest`，包级符号天然互相可见。Rust 用 `lib.rs`、公开 `stubs` 和 crate 自别名显式重建这一包级共享边界；五个同名 Rust 测试文件对应五个 Go 测试文件，`parity_test.rs` 则是 Rust 额外的迁移契约回归。

Go `ddlSuite` 持有真实 TiKV store、domain、session、MySQL 连接和多个 TiDB 子进程，并由 `createDDLSuite` 启动 server 与定期重启 goroutine。Rust `DdlSuite` 只保留内存 engine、伪 server、重启线程及相似的 suite API。Go `TestMain` 初始化公共测试环境、logger 并执行 goleak 检查；Rust `main_test.rs` 只验证 `LOG_LEVEL` 和 `setup_test_main()` 接线，明确没有覆盖全部 goleak 细节。

因此本文件对应的是 Go 的“package 级组织能力”，不是 Go DDL 实现。任何关于真实 TiDB DDL 正确性的结论都必须回到 Go 测试、canonical DDL 代码或真实 TiKV 测试验证，不能从本 crate 的内存桩外推。

## 扩展指南

- 新增共享测试支撑能力时修改 `stubs.rs`，并在独立 `*_test.rs` 或 `parity_test.rs` 中补回归；不要把实现或测试逻辑写进 `lib.rs`。
- 新增一个 Go 对照测试文件时，应在 `Cargo.toml` 增加相应 `[[test]]`；若还要让库的 `cfg(test)` 构建包含它，再在本文件增加带 `#[path]` 的私有模块。两种接线目的不同，不能只改一处后默认另一种构建也覆盖。
- 若只新增 parity 类独立测试，可沿用现状仅登记 `[[test]]`，无需扩大本文件的模块集合。
- 修改 `pub mod stubs` 或 `pub use stubs::*` 前应检查所有 `astersql_cmd_ddltest` 引用。通配再导出会让新增公开桩自动成为 crate 根 API；若要收紧表面，应先迁移使用方并保留清晰的 `stubs::` 路径。
- 若把桩替换为真实依赖，必须同步更新 Cargo 依赖、资源清理、并发错误传播和独立测试；这会改变当前 `test-only`、无外部依赖的兼容边界，不能作为本门面文件内的小改动处理。
- 相关风险集中在测试覆盖重复/遗漏、公共路径兼容以及后台线程清理；性能风险主要来自引入真实等待、外部进程或扩大并发规模，而非 `lib.rs` 当前的零运行时逻辑。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 7,032 个 Rust 文件；`files --filter cmd/ddltest` 列出 `lib.rs`、`stubs.rs`、五个同名 Rust 测试及 `parity_test.rs`。
- RustCodeGraph `node --file cmd/ddltest/lib.rs --offset 1 --limit 240`：核对完整 48 行、crate 自别名、公开模块、通配再导出和五个 `cfg(test)` 模块；索引把该文件识别为仅一个文件级符号，没有函数或类型。
- RustCodeGraph `node`：核对 `stubs.rs` 中随机状态、`DdlSuite`、`DdlSuite::create/teardown/run_ddl`、`create_ddl_suite`；核对 `column_test.rs`、`ddl_test.rs`、`index_test.rs`、`main_test.rs`、`random_test.rs` 和 `parity_test.rs` 的导入、入口、边界断言及清理方式。
- RustCodeGraph `query create_ddl_suite --kind function`：定位唯一实现 `cmd/ddltest/stubs.rs:1288`。精确 callers/callees 查询未可靠解析跨测试目标边，调用者改由上述源码节点和引用搜索确认。
- `cmd/ddltest/Cargo.toml`：核对库入口、`test-only` 元数据、六个独立测试目标、空依赖表和本地桩边界；根 `Cargo.toml` 核对 workspace 成员关系。
- Go 对照：`cmd/ddltest/ddl_test.go` 核对真实 `ddlSuite` 创建、重启和 teardown；`main_test.go` 核对 `TestMain`/goleak；`random_test.go` 核对随机 helper 的半开区间和参数语义；同目录列、索引测试提供相应场景基线。
- `rg` 引用检查：确认独立 Rust 测试通过 `astersql_cmd_ddltest::stubs` 使用库，且同目录不存在 `doc.go`。`git status` 同时显示已有的无关未跟踪 `cmd/ddltest/stubs.rs.md`，本任务未触碰该文件。

本任务是纯文档分析，按计划未运行 Cargo。最终结构由任务规定的 11 个固定二级标题命令验证。
