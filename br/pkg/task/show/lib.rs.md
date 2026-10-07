# `br/pkg/task/show/lib.rs`

## 文件定位

[`lib.rs`](./lib.rs) 是 Cargo 包 `astersql-br-pkg-task-show` 的 crate 根。`br/pkg/task/show/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定该入口，并用 `package.metadata.porting.go-package = "br/pkg/task/show"` 声明其 Go 对照包；根 `Cargo.toml` 将 `br/pkg/task/show` 列为 workspace member。

该文件是装配与兼容门面，不承载 `SHOW BACKUP METADATA` 的业务算法。实际 Rust 行为位于 [`cmd.rs`](./cmd.rs)，本地替代依赖位于 [`stubs.rs`](./stubs.rs)。当前仓库中除 workspace member 声明外，没有其他 `Cargo.toml` 引用包名 `astersql-br-pkg-task-show`，也没有其他 Rust 文件引用 crate 路径；因此它目前是可独立构建、由 crate 内测试验证的迁移单元，不能据此声称已经接入 Rust CLI 或 SQL 执行主链。生产系统中的完整调用链仍可在 Go 的 `pkg/executor/brie.go` 中看到：`showMetaExec.Next` 调用 `show.CreateExec`，再调用 `CmdExecutor.Read`。

## 核心职责

本文件只有四类职责：

1. 用 `#[path = "stubs.rs"] pub mod stubs` 和 `#[path = "cmd.rs"] pub mod cmd` 固定两个生产模块的源码位置并公开模块命名空间。
2. 通过 `pub use cmd::*` 扁平导出 `cmd.rs` 的 show API，使使用者既可写 `crate::cmd::Config`，也可从 crate 根取得 `Config`、`CreateExec`、`CmdExecutor`、`ShowResult` 等符号。
3. 从 `stubs` 精确再导出业务 API 需要的兼容类型和模块，包括 `Context`、`Error`、`Result`、`MetaReader`、`backuppb`、`objstore` 与 `task`；未列入该清单的 stub 细节仍只能经 `stubs::...` 访问。
4. 在 `cfg(test)` 下挂载 [`parity_test.rs`](./parity_test.rs) 与 [`cmd_test.rs`](./cmd_test.rs)，保证测试逻辑与生产源文件分离。

crate 根的 `#![allow(...)]` 对整个 crate 放宽命名、未使用项、死代码及 Clippy 检查。这是为了容纳 Go 风格的公开名称（如 `CreateExec`、`Read`、`ClusterID`）和迁移期 stub；它也会降低静态检查发现问题的能力，扩展时不应把它理解为可以任意忽略代码质量。

## 主要符号

- `pub mod stubs`：公开本地兼容层。其关键定义包括 `Context`/`CancelFunc`、`Error`/`Result`、`backuppb` 与 `encryptionpb` 数据结构、`Storage`/`MemStorage`、`MetaReader`、`ReadBackupMeta` 及测试注入 hook。
- `pub mod cmd`：公开实际 show 实现。主要 API 是 `Config`、`TimeStamp`、`RawRange`、`Table`、`ShowResult`、`CmdExecutor`、`CreateExec`、`collectResult` 以及三个转换函数 `convertBasic`、`convertTable`、`convertRawRange`。
- `mod parity_test`：仅测试构建可见的 crate 内契约测试。它能访问私有/内部路径，但不会成为公共 API。
- `mod cmd_test`：仅测试构建可见的 Go 测试移植，读取同目录 `testdata` fixture，并用 hook 替代尚未引入的真实 kvproto/metautil/对象存储边界。
- `pub use cmd::*`：通配再导出 `cmd.rs` 当前及未来的所有公开项。新增 `pub` 符号会自动扩大 crate 根 API，属于兼容性变化。
- `pub use stubs::{...}`：受控的 stub 再导出列表。`CIStr`、`DBInfo`、`TableInfo`、`MetaTable` 用于 schema 投影；`MetaReader`/`NewMetaReader` 用于执行器；`set_read_backup_meta_hook`、`MemStorage` 主要服务注入和测试；`backuppb`、`encryptionpb`、`objstore`、`task` 模拟 Go 依赖的命名边界。

## 执行流程

`lib.rs` 没有函数、初始化器或全局构造过程，运行时不会按源码声明顺序执行模块。其作用发生在编译和名称解析阶段：

1. 编译器以 `lib.rs` 为 crate 根，加载 `stubs.rs` 与 `cmd.rs`。
2. `cmd.rs` 通过 `crate::stubs` 使用兼容类型；随后 crate 根把 `cmd` 的公开 API 与选定的 stub API 再导出。
3. 普通构建不包含测试模块；`cargo test` 一类测试构建才加载 `parity_test.rs` 和 `cmd_test.rs`。
4. API 被调用时，真正的数据流从 `CreateExec(&Context, Config)` 开始：`ReadBackupMeta` 读取元数据，`NewMetaReader` 构造 reader，`CmdExecutor::Read` 校验版本窗口，然后选择事务备份的 schema 通道聚合或 RawKV range 投影。此流程属于 `cmd.rs`，不是 `lib.rs` 自身的执行逻辑。
5. Go 的完整 SQL 路径由 `pkg/executor/brie.go::showMetaExec.Next` 提供：组装 `show.Config`，调用 `show.CreateExec`/`Read`，再把表信息和按会话时区转换的时间写入结果 chunk。当前没有证据表明 Rust crate 根已连接到等价上游。

## 数据与状态

本文件自身不定义数据字段、静态变量、锁、缓存或持久状态。它定义的是可见性边界：`cmd::*` 公开全部业务类型，`stubs::{...}` 只公开白名单类型。

实际状态由下游符号持有：`Config` 保存存储 URI、后端配置和密钥；`CmdExecutor` 持有可克隆的 `MetaReader`；`ShowResult` 拥有集群信息、版本窗口、表列表或 RawKV ranges。`stubs.rs` 的 `set_read_backup_meta_hook` 管理测试注入状态，`parity_test.rs` 明确要求用例结束后恢复为 `None`，避免状态污染。上述状态之所以可从 crate 根触达，是再导出策略的结果，但其生命周期规则由各实现文件负责。

## 依赖与调用关系

`br/pkg/task/show/Cargo.toml` 的 `[dependencies]` 为空，所以 Rust crate 不链接真实的 BR、kvproto、对象存储或 TiDB crate；`cmd.rs` 的所有下游依赖都来自同 crate 的 `stubs`。直接关系如下：

- `lib.rs` → `stubs.rs`：模块装配，并再导出兼容类型/模块。
- `lib.rs` → `cmd.rs`：模块装配，并通配再导出 show 公共 API。
- `cmd.rs` → `stubs.rs`：使用 `Context`、`MetaReader`、`ReadBackupMeta`、protobuf 形状和错误类型完成本地实现。
- 测试构建中的 `lib.rs` → `parity_test.rs` / `cmd_test.rs`：验证公开契约、fixture 读取、错误与取消语义。
- Go 生产链 `pkg/executor/brie.go` → Go 包 `br/pkg/task/show/cmd.go`：这是当前可验证的应用调用方，不是 Rust crate 的静态调用边。

RustCodeGraph 对 `lib.rs` 的文件节点显示 37 行且只有模块/再导出结构；文件引用结果仅报告 `tools/tazel/parity_test.rs`，精确的仓库文本搜索也未发现 Rust 消费者。由于通配再导出本身不是函数调用，调用图没有为 `lib.rs` 产生业务调用路径是正常现象。

## 错误处理与边界

本文件没有可失败操作，也不创建 `Result`；错误语义全部由再导出的实现提供。`CreateExec` 会给 `ReadBackupMeta` 失败增加 `failed to create execution` 上下文，`CmdExecutor::Read` 会拒绝 `StartVersion > EndVersion` 和带 `RawRangeIndex` 的 RawKV V2 元数据，schema 读取错误及 context 取消则由 `collectResult` 传播。

门面层有三个重要边界：

- `pub use cmd::*` 会无条件公开 `cmd.rs` 中所有 `pub` 项，新增或重命名符号需要评估 crate 根 API 兼容性。
- stub 类型只是本地迁移替身。空依赖清单和 `cmd_test.rs` 的说明都明确指出真实 kvproto、metautil schema walk、TiDB SQL 执行与对象存储边界被模拟；不能把测试通过解释为真实后端已接线。
- `#![allow(clippy::all)]` 等 crate 级豁免会覆盖子模块。新增代码若能使用 Rust 风格命名或真实依赖，应主动收窄豁免，而不是继续扩大静态检查盲区。

## 并发与资源生命周期

`lib.rs` 本身不启动线程、不创建通道，也不拥有需要释放的资源。并发行为来自它导出的 `CmdExecutor::Read`：事务备份路径创建容量 16 的 `sync_channel` 和错误通道，克隆 `MetaReader`/`Context` 后启动后台线程执行 `ReadSchemasFiles`；主线程由 `collectResult` 排空数据、传播错误并轮询取消。发送端 drop 后输出通道关闭，收集循环结束。

测试侧的资源边界也独立于 crate 根：`cmd_test.rs::TempBackup` 用 `Drop` 清理临时目录；测试 hook 需要显式 `clear_hook`/`set_read_backup_meta_hook(None)`；`parity_test.rs` 验证父 context 的后续取消能被子 context 观察，并验证执行器和 owned `ShowResult` 可安全释放。若将此 crate 接入并行测试或真实服务，hook 隔离与后台线程退出仍是需要重点复核的风险。

## 与 Go 版本的对应关系

Go 包没有 `lib.go` 式门面；`br/pkg/task/show/cmd.go` 直接定义包级 API，Go 编译器天然聚合同包文件。Rust 的 `lib.rs` 用显式模块声明和再导出模拟这一包级命名空间：`pub use cmd::*` 对应 Go 包公开的 `Config`、`TimeStamp`、`RawRange`、`Table`、`ShowResult`、`CmdExecutor` 与 `CreateExec`。

Rust 与 Go 当前并非同等依赖形态。Go `BUILD.bazel` 显式依赖真实 `br/pkg/errors`、`br/pkg/metautil`、`br/pkg/task`、`pkg/objstore`、kvproto 和 client-go oracle；Rust `Cargo.toml` 没有依赖，转而公开 `stubs.rs` 中的替身。Rust 特有的 `CmdExecutor::from_reader`、`collectResult` 和若干转换函数被公开，主要便于注入与契约测试，并不都是 Go 包的导出 API。

行为对照证据来自 `cmd.go` 与测试：Go `TestFull`、`TestV2AndSmallTables`、`TestV2Encrypted` 验证 v1、500 张小表和 AES-256-CTR fixture；`TestShowViaSQL` 验证 `+08:00`/`-08:00` 会话时区输出。Rust `cmd_test.rs` 保留对应测试名称与 fixture，但通过本地解析和 hook 模拟外部边界；`parity_test.rs` 另验证字段投影、版本窗口、RawKV V2 拒绝、schema 错误、取消和资源释放。

## 扩展指南

- 新增 show 业务行为应修改 `cmd.rs` 中最接近的符号，并同步独立测试文件 `cmd_test.rs` 或 `parity_test.rs`；不要把测试模块或测试函数写入 `lib.rs`/`cmd.rs`。
- 新增公开业务 API 前先决定它是否应从 crate 根暴露。由于 `cmd::*` 是通配导出，任何 `pub` 项都会自动成为根 API；若该行为不合适，应改为显式导出清单并做兼容性审查。
- 新增 stub 能力时，默认经 `stubs::...` 使用；只有调用方确实需要根级兼容名称时才加入 `pub use stubs::{...}`。真实外部依赖的移植必须遵守仓库的上游仓库、tag 与 Git 依赖规则，不能把依赖源码复制进本 crate。
- 若要接入真实 Rust 应用链，需要同时补齐使用方的 Cargo 依赖和入口调用，并用真实存储/kvproto/metautil 验证；现有 hook 测试只能作为逻辑契约证据，不能替代集成验证。
- 涉及通道或 context 的修改应重点覆盖：输出关闭前的待处理错误、取消优先级、容量 16 的背压、后台线程退出，以及 hook 在并行测试中的隔离。
- 保持 Go 语义：版本窗口校验、事务/RawKV 分支、V2 RawKV 限制、时间格式与错误上下文都应与 `cmd.go` 及 `cmd_test.go` 对照，不能为让 Rust 测试通过而简化。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/task/show` 列出 `lib.rs`、`cmd.rs`、`stubs.rs`、两份 Rust 测试及 Go 对照；`node --file br/pkg/task/show/lib.rs --offset 1 --limit 200` 确认本文件共 37 行及全部模块/再导出；`node --file br/pkg/task/show/cmd.rs --offset 1 --limit 360` 核对公开符号和 `CreateExec → CmdExecutor::Read → collectResult/转换函数` 数据流；对两份测试的 `node --file` 查询核对边界与错误断言。对门面文件的 explore 未得到静态调用路径，与其不含可调用函数且尚无 Rust 消费者的事实一致。
- Rust 源与配置：`br/pkg/task/show/lib.rs`、`cmd.rs`、`stubs.rs`、`Cargo.toml`、根 `Cargo.toml`、`parity_test.rs`、`cmd_test.rs`。
- Go 与构建对照：`br/pkg/task/show/cmd.go`、`cmd_test.go`、`BUILD.bazel`，以及生产调用方 `pkg/executor/brie.go::showMetaExec.Next`。
- 仓库引用搜索：包名/路径只出现在 workspace、crate 自身与 Go Bazel/Go 调用处；未发现其他 Cargo manifest 或 Rust 源消费该 crate。该结论只覆盖当前工作树的静态文本与 RustCodeGraph 索引，不证明未来或仓库外调用方不存在。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的 11 标题结构命令检查文档存在且章节数精确，并人工检查没有把 stub 当作真实外部集成。
