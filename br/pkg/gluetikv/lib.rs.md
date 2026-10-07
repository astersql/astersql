# [`br/pkg/gluetikv/lib.rs`](lib.rs)

## 文件定位

`br/pkg/gluetikv/lib.rs` 是 Cargo 包 `astersql-br-pkg-gluetikv` 的 crate 根。`br/pkg/gluetikv/Cargo.toml` 用 `[lib] path = "lib.rs"` 指定该入口，并用 `package.metadata.porting.go-package = "br/pkg/gluetikv"` 声明其 Go 对照包。该文件只有 30 行，职责是编译期接线而不是实现 TiKV glue 的运行时行为。

生产构建中，本文件用 `#[path = "glue.rs"] pub mod glue` 挂载唯一实现模块，再以 `pub use glue::*` 将其公开符号提升到 crate 根（`lib.rs:17-22`）。测试构建额外挂载独立的 `parity_test.rs` 和 `glue_test.rs`（`lib.rs:24-30`），因此测试没有内嵌在生产源文件中。crate 级 `allow` 保留 Go 移植常见的公开命名，并容纳迁移期未使用项（`lib.rs:8-15`）。

## 核心职责

1. 确立纯 TiKV BR glue 的 crate 边界，使调用方可依赖 `astersql-br-pkg-gluetikv`，而不必直接把 `glue.rs` 当作模块拼装。
2. 提供扁平公共 API。`glue.rs` 中的 `Glue`、`OpenFn`、`set_open_hook_for_test` 和 `take_records_for_test` 均因 `pub use glue::*` 可从 crate 根导入；私有的 `NamedStorage`、`CounterProgress`、`NilSession` 与槽函数仍不会泄露。
3. 将生产逻辑与独立验证文件接线。普通构建只编译 `glue`；`cfg(test)` 构建还编译版本格式测试与 Go/Rust 公共契约测试。
4. 作为 TiDB glue 的下游委托点。`br/pkg/gluetidb/glue.rs` 从本 crate 根导入 `Glue as TikvGlue`，并把 `Open`、`StartProgress`、`Record`、`GetVersion`、`UseOneShotSession`、`GetClient` 与 `OwnsStorage` 委托给它。

本文件不应被描述成真实 PD/TiKV 驱动实现。`Cargo.toml` 和 `glue.rs` 均明确记录了当前 arm64/grpc 瘦身边界：默认 `Open` 返回本地命名存储替身，但会执行与 Go 驱动一致的同步路径校验；完整 TiKVDriver 与 summary wiring 仍是延期接线。

## 主要符号

`lib.rs` 自身没有常量、类型、trait、函数或 `impl`，只有以下模块级符号：

- `pub mod glue`：公开实现模块，固定映射到同目录 `glue.rs`。模块内的核心生产类型是零额外堆状态的 `Glue { StdIOGlue }`；它实现 `astersql_br_pkg_glue::Glue`。
- `pub use glue::*`：把 `glue` 的全部公开项重导出到 crate 根。当前公开集合包括 `Glue`、测试可注入的 `OpenFn`、`set_open_hook_for_test` 和 `take_records_for_test`；后两者虽然以 `_for_test` 命名，但没有 `cfg(test)` 限制，属于现有公共 API。
- `mod parity_test`：仅测试构建可见的私有模块，入口测试为 `go_rust_public_contract_matches`。
- `mod glue_test`：仅测试构建可见的私有模块，入口测试为 `test_get_version`。

真实行为由 `glue.rs` 中的 `impl GlueTrait for Glue` 提供：`GetDomain`、`CreateSession`、`Open`、`OwnsStorage`、`StartProgress`、`Record`、`GetVersion`、`UseOneShotSession`、`GetClient` 和 `AsConsoleGlue`。理解这些方法时必须沿重导出追到实现文件，不能把 crate 根的零函数图误判为功能缺失。

## 执行流程

本文件没有运行时入口；其编译期接线形成以下实际调用链：

1. 调用方从 crate 根取得 `Glue`，通常用 `Glue::new()` 构造含 `StdIOGlue` 的轻量值。`br/pkg/gluetidb/glue.rs` 是已确认的生产 Cargo 上游；同目录测试也通过 `crate::Glue` 使用根级重导出。
2. BR 上层经 `astersql_br_pkg_glue::Glue` trait 调用具体方法。`Open` 在 `CAPath` 非空时复制并更新全局 TLS 三件套，然后优先调用进程级注入 hook，否则进入 `default_open`。
3. `default_open` 先由 `validate_tikv_path` 检查 `tikv://` 前缀、非空 PD 地址及 `disableGC` 布尔值，再返回以 path 命名的本地 `Storage` 替身；当前不会建立真实 PD/TiKV 连接。
4. `StartProgress` 返回原子计数器 `CounterProgress`；`Record` 向进程级记录缓冲写入 `(name, 1, value)`；`GetVersion` 生成 `"BR\n" + BuildInfo()`。
5. 纯 TiKV 模式不建立 SQL 会话：`UseOneShotSession` 直接成功且不调用回调；`CreateSession` 返回 `NilSession`，一旦误用其 SQL/DDL 方法会返回 `gluetikv: session is nil`。
6. 测试构建时，`glue_test` 验证版本串顺序，`parity_test` 验证上述成功、边界、副作用和错误分支以及控制台能力。

## 数据与状态

crate 根本身不保存运行时数据。被它导出的实现包含三类状态：

- `Glue` 仅内嵌可复制的 `StdIOGlue`，`new` 不分配连接或会话；`AsConsoleGlue` 按调用创建其 `Arc<dyn ConsoleGlue>` 包装。
- `OPEN_HOOK: OnceLock<Mutex<Option<OpenFn>>>` 是进程级可注入打开边界。`OpenFn` 要求 `Send + Sync`，设置与读取均经互斥锁；测试必须恢复为 `None`，避免污染后续用例。
- `RECORDS: OnceLock<Mutex<Vec<(String, i32, u64)>>>` 保存 `Record` 的可观测替身数据；`take_records_for_test` 通过 `std::mem::take` 原子地取出并清空当前向量。
- 每个 `CounterProgress` 独立拥有 `AtomicI64`，`Inc` 委托 `IncBy(1)`，读写采用 `Ordering::Relaxed`；`Close` 当前无清理动作。
- TLS 配置不属于本 crate，但 `Open` 会在 `CAPath` 非空时 clone、修改并覆盖 `astersql_config` 的进程级全局配置。这是调用方可见的持久副作用。

`NamedStorage` 与 `NilSession` 是私有替身。前者只持有 path/name 字符串；后者无字段，存在的目的不是提供 SQL 能力，而是把 Go 的 `(nil, nil)` 形状映射为 Rust 可返回的 trait object，并在误用时明确报错。

## 依赖与调用关系

`Cargo.toml` 声明四个直接依赖：

- `astersql-br-pkg-glue`：提供 `Glue`、`Session`、`Storage`、`Progress`、`ConsoleGlue`、`SecurityOption` 等公共契约与 `StdIOGlue`。
- `astersql-br-pkg-version-build`：`GetVersion` 调用其 `Info`。
- `astersql-config`：`Open` 读取并更新全局 cluster SSL 配置。
- `astersql-errors`：提供 `SharedError` 与 `New`，用于路径校验和 nil-session 错误。

Cargo 搜索确认当前直接依赖本包的 manifest 是 `br/pkg/gluetidb/Cargo.toml`。RustCodeGraph 显示 `br/pkg/gluetidb/glue.rs` 对 TiKV glue 的 `Open`、`StartProgress`、`Record`、`GetVersion`、`UseOneShotSession`、`GetClient` 和 `OwnsStorage` 各有委托调用边。另一方面，同目录 `parity_test.rs` 直接调用根级 `Glue` 及测试辅助函数，构成最集中、最可追溯的行为覆盖。

Go 生产上游还包括 `br/cmd/br/backup.go` 与 `restore.go`：它们把 `gluetikv.Glue{}` 传给 raw/txn backup 或 restore 任务。Rust 侧目前能确认的真实生产 crate 消费链集中在 `gluetidb` 委托；不能仅根据 Go 入口宣称 Rust 的所有 raw/txn 命令路径都已完成同等 Cargo 接线。

## 错误处理与边界

`lib.rs` 不制造、捕获或转换错误，只决定哪些实现进入 crate 和公共命名空间。错误语义来自被重导出的 `glue.rs`：

- `Open` 的注入 hook 错误原样上抛；默认路径在外部边界之前拒绝非 `tikv://` 路径、空 PD authority 和非法 `disableGC`。未知 query key 当前被忽略。
- TLS 赋值只由非空 `CAPath` 触发；只有 cert/key 而 CA 为空时，全局配置保持不变。若随后 hook/default open 失败，已经写入的 TLS 配置不会由实现回滚。
- hook/记录互斥锁中毒时使用 `expect`，会 panic，而不是返回 `SharedError`。扩展时不能把这些辅助槽当作无失败的外部服务边界。
- `CreateSession` 成功返回 `NilSession`，但其执行、元数据、全局变量等方法统一报 `gluetikv: session is nil`；`Close` 是空操作，`GetSessionCtx` 返回占位句柄。
- `UseOneShotSession` 有意不调用回调，严格对应 Go 的直接 `return nil`。把回调执行起来会改变纯 TiKV 路径的契约。
- 根级通配重导出意味着 `glue.rs` 新增任何 `pub` 项都会自动扩大 crate 公共 API，并可能与未来根符号发生名字冲突。

## 并发与资源生命周期

`lib.rs` 不启动线程、不打开连接，也不拥有析构流程。实现层的共享资源生命周期由两个 `OnceLock<Mutex<...>>` 管理：它们首次访问时初始化，并存活到进程结束；`set_open_hook_for_test(None)` 与 `take_records_for_test()` 只是清空内容，不销毁槽本身。

`OpenFn` 使用 `Arc<dyn Fn + Send + Sync>`，允许跨线程共享注入逻辑；`Open` 在持锁期间只 clone hook，随后释放锁再调用闭包，避免让外部回调运行在槽锁内。记录缓冲在 push 或 drain 的短临界区内持锁。并行测试若同时改写全局 hook、记录缓冲或全局 TLS 配置，仍可能互相干扰；现有 `parity_test` 通过每段后清理来约束顺序，但这些进程级状态并未实现测试级隔离。

`CounterProgress` 用原子值支持多线程递增与读取，Relaxed 只保证计数原子性，不提供其他数据的 happens-before 关系。默认 `Open` 返回的 `NamedStorage` 随 `Box<dyn Storage>` 所有权释放；`NilSession` 同样随 trait object drop，且没有外部 Domain/SQL 资源需要关闭。

## 与 Go 版本的对应关系

Go 没有独立的 crate 根文件；Rust `lib.rs` 是为 Go 包 `br/pkg/gluetikv` 增加的模块边界。核心对应关系在 `glue.rs` 与 `glue.go` 之间：两侧 `Glue` 都表示“不依赖 TiDB SQL 层、只服务 TiKV 路径”的 `glue.Glue` 实现，并拥有 storage、返回 `ClientCLP`、提供 `BR\n` 版本串及内嵌控制台能力。

已保持的关键语义包括：`GetDomain`/`CreateSession` 不建立真实 TiDB 对象，`UseOneShotSession` 不调用回调，`Open` 只有在 CA 非空时同步三项 TLS 配置，`StartProgress` 提供可增量关闭的句柄，`Record(name, val)` 固定使用 unit count 1。Rust 用 `Domain`/`NilSession` trait object 表示 Go 的 nil 结果，以便满足 Rust trait 返回类型；因此“调用成功”不等于存在可用 SQL session。

当前差异必须明确：Go `Open` 调用真实 `driver.TiKVDriver{}.Open(path)`，Rust 默认只做同步路径校验并返回 `NamedStorage`；Go `StartProgress` 调用 `utils.StartProgress`，Rust 使用本地原子计数器；Go `Record` 写入 `summary.CollectSuccessUnit`，Rust写入可 drain 的内存缓冲。`Cargo.toml` 将完整 TiKVDriver/summary wiring 标为延期事项，故这些替身不能描述成完整等价实现。

Go `glue_test.go::TestGetVersion` 已由独立 `glue_test.rs::test_get_version` 保留版本串顺序意图。Rust `parity_test.rs::go_rust_public_contract_matches` 额外覆盖 storage 所有权、client 常量、空会话、进度、记录、TLS 副作用、hook 错误、默认路径校验和控制台嵌入。

## 扩展指南

- 新增生产模块时在本文件增加清晰的 `#[path] pub mod` 声明；新增测试继续放入独立 `*_test.rs` 并以 `#[cfg(test)]` 私有挂载，不能把测试逻辑写回 `lib.rs` 或 `glue.rs`。
- 修改 `glue.rs` 的公开项前先评估 `pub use glue::*` 的公共 API 扩张。若辅助函数只应供 crate 内测试使用，需同时设计可见性与测试挂载方式，而不是继续无条件重导出。
- 扩展 `GlueTrait` 行为时，应同步检查 `br/pkg/glue/glue.rs` 的 trait、`br/pkg/gluetikv/glue.rs` 的实现、`br/pkg/gluetidb/glue.rs` 的委托以及 `parity_test.rs`；对照 Go `glue.go` 保留真实分支，不能以空实现缩减语义。
- 修改 `Open` 时至少覆盖：CA 为空/非空、hook 成功/失败、非法 scheme、空 PD 地址、合法/非法 `disableGC` 和多 PD/keyspace query；特别审查全局 TLS 是否应在失败时回滚，以及并行测试隔离。
- 将默认替身替换为真实 TiKVDriver、summary 或进度实现时，必须在相应上游依赖仓库按仓库规则移植并使用发布 tag，不能 vendor、用本地 `[patch]`，也不能只更改本文件重导出后宣称完成。
- 性能风险不在 crate 根，而在全局 Mutex 争用、记录缓冲无界增长及未来真实网络打开；兼容风险集中在根级公开名称、Go nil 语义、TLS 副作用和 `UseOneShotSession` 不执行回调的不变量。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7032 个 Rust 文件；`files --filter br/pkg/gluetikv` 列出 `lib.rs`、`glue.rs`、两个独立 Rust 测试以及 Go 的 `glue.go`/`glue_test.go`。
- RustCodeGraph `node --file br/pkg/gluetikv/lib.rs --offset 1 --limit 260`：核对 30 行 crate 根、唯一生产模块、通配重导出与两个 `cfg(test)` 模块；图将该文件识别为一个模块级符号，符合其门面性质。
- RustCodeGraph `node --file br/pkg/gluetikv/glue.rs --offset 1 --limit 380`：核对 `Glue`、全局槽、路径校验、进度/会话替身及完整 trait 实现。精确 `query`/`explore` 消除同名 `Glue`/`Open` 歧义，并确认 `gluetidb` 对 `Open`、`StartProgress`、`Record`、`GetVersion`、`UseOneShotSession`、`GetClient`、`OwnsStorage` 的委托调用边。
- Cargo 与上游：读取 `br/pkg/gluetikv/Cargo.toml`，并用 `rg` 确认 `br/pkg/gluetidb/Cargo.toml` 是当前直接依赖 manifest；读取/查询 `br/pkg/gluetidb/glue.rs` 的委托证据。
- Go 对照：读取 `br/pkg/gluetikv/glue.go` 与 `glue_test.go`，并用 `rg` 核对 `br/cmd/br/backup.go`、`restore.go` 的 Go 生产入口；差异部分按 Cargo 注释与 Rust 实现如实标记为替身。
- 独立 Rust 测试：读取 `br/pkg/gluetikv/glue_test.rs` 与 `parity_test.rs`，确认由本文件在测试态挂载，并覆盖版本、成功/边界/错误、副作用、默认路径与控制台契约。
- 本任务只新增说明文档，按任务计划不运行 Cargo。结构验证命令与退出码、diff 检查和提交证据在交付报告中给出。
