# `br/pkg/task/show/cmd.rs`

## 文件定位

`cmd.rs` 是 workspace 成员 crate `astersql-br-pkg-task-show` 的核心实现文件。该 crate 由 `br/pkg/task/show/Cargo.toml` 定义为 library，`lib.rs` 通过 `pub mod cmd` 装入本文件，并用 `pub use cmd::*` 对外再导出其公开 API。文件对应 Go 包 `br/pkg/task/show` 的 `cmd.go`，目标是把备份元数据转换成适合 CLI 或 SQL 层展示的结构。

当前 Rust crate 没有外部依赖声明，存储、protobuf、上下文和 MetaReader 均来自同 crate 的 `stubs.rs`。仓库搜索未发现测试以外的 Rust 代码调用 `CreateExec` 或 `CmdExecutor::Read`；因此它目前是可测试的移植库，还没有接入 Rust 版 SQL executor。Go 的实际生产上游是 `pkg/executor/brie.go` 中 `showMetaExec::Next`，它调用 `show.CreateExec` 和 `Read` 后把 `Tables` 写入结果 chunk。

## 核心职责

本文件承担四类职责：

1. 定义 show 命令输入 `Config` 和输出模型 `TimeStamp`、`RawRange`、`Table`、`ShowResult`。
2. `CreateExec` 从配置读取 `backupmeta` 并构造 `MetaReader`，形成 `CmdExecutor`。
3. `CmdExecutor::Read` 校验备份时间窗口，并按事务备份或 RawKV 备份分支组装结果。
4. `collectResult` 负责通道聚合；`convertBasic`、`convertTable`、`convertRawRange` 负责无副作用的数据投影。

其边界很清楚：本文件不解析存储 URI、不解码 protobuf、不遍历真实 V2 schema 文件，也不渲染终端或 SQL 行；这些能力在 Go 中由 task/metautil/executor 提供，在当前 Rust crate 中由 `stubs.rs` 的 hook、`MemStorage` 和预装表数据代替。

## 主要符号

- `Config { Storage, BackendCfg, Cipher }`：show 所需的最小配置。私有方法 `lameTaskConfig` 只把这三个字段复制到 `task::Config`，避免要求调用者构造完整 BR task 配置。
- `TimeStamp(u64)`：TSO 值类型；`Display` 输出 `<tso>(<本地可读时间>)`，时间转换委托给 `oracle::format_show_ts`。
- `RawRange`：RawKV 展示投影，包含列族及拥有所有权的十六进制起止键。
- `Table`：事务备份表投影，包含库表名、KV 数量/字节数和 TiFlash 副本数。
- `ShowResult`：最终聚合结果，基础元数据之外，以 `IsRawKV` 决定主要填充 `RawRanges` 还是 `Tables`。
- `CmdExecutor { meta }`：持有 `MetaReader`。`from_reader` 是测试/注入入口；`CreateExec` 是配置驱动入口。
- `CreateExec(&Context, Config) -> Result<CmdExecutor>`：调用 `ReadBackupMeta`，为失败增加 `failed to create execution` 上下文，再以 `NewMetaReader` 组装执行器。
- `CmdExecutor::Read(&Context) -> Result<ShowResult>`：主执行入口，处理版本校验、schema 聚合或 RawKV 范围展开。
- `collectResult<T, R, M>`：消费数据通道与错误通道，应用映射函数并响应取消。
- `convertBasic`、`convertTable`、`convertRawRange`：分别转换 `BackupMeta` 基础字段、`MetaTable` 和 protobuf `RawRange`。

文件没有 trait、模块级常量或条件编译项；公开结构字段和函数沿用 Go 风格命名，crate 根通过 lint allow 接受这些名称。

## 执行流程

典型流程从 `CreateExec` 开始：

1. `Config::lameTaskConfig` 生成只含存储、后端选项和 cipher 的 `task::Config`。
2. `ReadBackupMeta(ctx, MetaFile, ...)` 返回后端、存储句柄和已解码 `BackupMeta`；后端值在本文件中不再使用。
3. `NewMetaReader(backupMeta, strg, &cfg.Cipher)` 构造 reader，随后封装为 `CmdExecutor`。
4. `Read` 先用 `convertBasic(self.meta.GetBasic())` 复制集群、BR、协议版本和起止 TSO。若 `EndVersion < StartVersion`，将其视为疑似日志备份元数据并拒绝。
5. 对事务备份，`Read` 建立容量 16 的同步数据通道和无界错误通道，克隆 reader/context 后启动后台线程。后台调用 `ReadSchemasFiles`，传入 `SkipFiles` 与 `SkipStats`，先发送最终 `Result<()>`，再通过丢弃 sender 关闭数据通道。主线程以 `collectResult(..., convertTable)` 保序聚合表投影。
6. 对 RawKV 备份，若 `RawRangeIndex` 存在则拒绝当前未支持的 Raw V2；否则逐项调用 `convertRawRange`。
7. 返回完整 `ShowResult`。

`collectResult` 每轮先检查取消，再用 `try_recv` 尽量排空数据，以免同步通道生产者受阻；随后检查错误通道，最后以 1 ms 的 `recv_timeout` 等待新数据。数据通道关闭时，它仍尝试提取一个待处理错误，再决定返回错误或已收集结果。

## 数据与状态

`ShowResult` 和其子结构均是拥有数据的普通值：字符串、整数、`Vec` 以及 `HexBytes(Vec<u8>)`，返回后不依赖 reader 或存储句柄。`convertBasic` 总是把 `Tables`、`RawRanges` 初始化为空；后续分支只填充其中与备份类型对应的一侧，但类型层面没有强制二者互斥，调用者应以 `IsRawKV` 解释结果。

`CmdExecutor` 唯一持久状态是可克隆的 `MetaReader`。当前 `stubs.rs` 的 reader 保存 `BackupMeta`、预注入表、可选读取错误、存储名和 cipher；`GetBasic` 返回副本，因此 `Read` 多次取 basic 不会借用内部状态。`collectResult` 的 `collected` 保持接收顺序；映射闭包仅要求 `Fn(T) -> R`，不能通过可变捕获保存迭代状态。

关键不变量包括：`StartVersion <= EndVersion`；Raw V2 的 `RawRangeIndex` 必须为空；库级 schema 允许 `MetaTable.Info == None`，此时 `TableName` 必须为空字符串；`TiFlashReplicas` 从 `i32` 直接转换为 `u64`，当前实现没有负值校验。

## 依赖与调用关系

上游关系如下：

- Rust：`lib.rs` 声明并再导出 `cmd`；`cmd_test.rs` 和 `parity_test.rs` 直接调用公开 API。仓库内未找到测试以外的 Rust 生产调用者。
- Go：`pkg/executor/brie.go` 的 `showMetaExec::Next` 是生产调用链，构造 `show.Config`，调用 `CreateExec`/`Read`，再把表数据和按 session 时区转换的时间写入 chunk。

主要下游关系均经 `crate::stubs`：

- `CreateExec -> ReadBackupMeta -> NewMetaReader`；当前 Rust `ReadBackupMeta` 依赖 thread-local hook，无 hook 时返回 `ReadBackupMeta not configured`，不是实际对象存储实现。
- `CmdExecutor::Read -> MetaReader::{GetBasic, ReadSchemasFiles}`。当前 `ReadSchemasFiles` 只重放 storage/hook 注入的 `MetaTable`，并不读取真实 schema 文件。
- `Read -> collectResult -> convertTable` 形成事务表链；`Read -> convertRawRange` 形成 RawKV 链；两条链都以 `convertBasic` 为公共起点。
- `TimeStamp::fmt -> oracle::format_show_ts`，后者按 TSO 高位物理毫秒和本地时区格式化。

RustCodeGraph 的文件节点显示 `cmd.rs` 含 16 个符号并被 19 个文件引用；其中列出的引用包含同 crate 测试以及若干使用共享桩形状的 BR 文件。精确 `callers CreateExec` 查询超时，因此生产接线结论以仓库符号搜索和模块/Cargo 声明交叉核验。

## 错误处理与边界

- `CreateExec` 保留底层错误并通过 `Error::Annotate` 增加 `failed to create execution`；`parity_test.rs` 覆盖无 hook 的失败路径。
- `EndVersion < StartVersion` 返回 `ErrInvalidMetaFile`，错误文案点明 start/end 并提示可能读到了 log backup；相等的起止版本合法。
- RawKV 且 `RawRangeIndex.is_some()` 返回明确的“不支持 backup meta v2”错误；空 RawRanges 合法。
- schema 读取错误通过错误通道传给 `collectResult`。错误通道的 `Ok(())` 或断开只表示生产者结束，不会清空已收集项目。
- `collectResult` 在循环入口和超时后检查 `Context`；取消时优先返回 context 错误。当前上下文桩只支持取消传播，不支持 Go context 的 deadline/value。
- 数据通道关闭后只以一次非阻塞读取检查待处理错误。如果错误消息尚未到达而 sender 已先关闭，存在返回成功结果而未等待迟到错误的理论风险；`Read` 的生产线程按“先发错误、再 drop 数据 sender”规约避免该竞态，扩展生产者时必须保持此顺序。
- `TimeStamp` 的展示依赖进程本地时区；它不是 SQL executor 最终按 session 时区格式化的替代品。Go 的 `showMetaExec::Next` 会另行用 session location 转换开始/结束时间。

## 并发与资源生命周期

事务分支每次 `Read` 创建一个后台 OS 线程。`sync_channel(16)` 提供有限背压：当消费者落后超过 16 项，`ReadSchemasFiles` 的发送会阻塞；`collectResult` 先排空可用数据以降低这种阻塞。错误通道无界，但正常仅发送一次完成结果。

后台线程不返回 `JoinHandle`，主线程靠通道关闭判断生产结束；正常协议是 `ReadSchemasFiles` 返回、发送结果、丢弃数据 sender。`collectResult` 以 1 ms 超时轮询换取取消响应，没有永久阻塞，但大量调用会产生线程与轮询开销。若主线程因取消或错误提前返回，receiver 被释放，桩 reader 的后续 send 会得到 `schema output channel closed`，线程随后退出；没有显式 join 保证其已在 `Read` 返回前结束。

RawKV 分支不启动线程。所有结果数据均拥有所有权，`CmdExecutor`、`ShowResult` 和通道离开作用域后依靠 RAII 释放；`parity_test.rs` 覆盖执行器 drop、拥有型 `HexBytes` 及后台收尾。测试用 `ReadBackupMeta` hook 是 thread-local，必须在用例结束时调用 `set_read_backup_meta_hook(None)`，否则会污染同一测试线程上的后续用例。

## 与 Go 版本的对应关系

结构和主分支基本逐项对应 `br/pkg/task/show/cmd.go`：配置裁剪、TSO wrapper、展示结构、`CreateExec`、版本窗口检查、非 Raw schema 聚合、Raw V2 拒绝以及三个转换函数均保留相同语义。Rust 将 Go 的指针/切片改为拥有型值和 `Vec`，将 `uint` 计数固定为 `u64`，并以 `Option` 表达 nil table info 与 raw range index。

并发实现不是逐语法翻译：Go `collectResult` 直接 `select` context、数据和错误通道；Rust 标准库 mpsc 无等价 select，因此采用 `try_recv + recv_timeout(1 ms)`。Rust 额外记录 `err_done`，并约定生产者先发送完成结果再关闭数据通道。两版都保持输入顺序、错误立即传播和取消语义。

当前移植存在明确能力差异。Go 通过真实 `task.ReadBackupMeta`、`metautil.MetaReader` 和对象存储运行，并由 `pkg/executor/brie.go` 接入 `SHOW BACKUP METADATA`；Rust Cargo 无外部依赖，`stubs.rs` 明示真实存储/protobuf/V2 schema walk 为 mock 边界，仓库也没有 Rust SQL executor 调用。因此 Rust 测试中的 V1/V2/加密 fixture 只验证文件字节、cipher 形状以及 hook 注入后的 show 主流程，不能证明真实对象存储读取或解密已经移植。

## 扩展指南

- 接入真实 Rust 生产链时，应优先替换 `stubs.rs` 中 `ReadBackupMeta`、`MetaReader` 和相关 protobuf/storage 类型，而不是在 `cmd.rs` 内复制 IO 逻辑；随后从 Rust executor 显式调用 `CreateExec`/`Read`。
- 支持 Raw V2 时，修改 `CmdExecutor::Read` 的 `RawRangeIndex` 分支，依据真实 MetaReader 展开索引；同步扩展独立的 `parity_test.rs`，并增加真实集成层证据，不能简单删除拒绝检查。
- 增加展示字段时，先更新 `ShowResult` 或 `Table`/`RawRange`，再更新对应 `convert*`，并同步 Go 对照、序列化/CLI/SQL 消费者和独立测试。注意 `lib.rs` 会通配再导出公开符号。
- 改变并发协议时，保持“错误结果先于数据 sender 关闭”的不变量，覆盖数据通道先关闭、错误到达、取消、生产者失败和超过 16 项背压场景。若改用 async 或可 select 的通道，应验证顺序和取消语义，并评估每次 `Read` 启动 OS 线程的性能差异。
- 修改时间格式时区语义时，同时检查 `TimeStamp::fmt`、`stubs::oracle::format_show_ts`、`cmd_test.rs` 的本地时区断言，以及 Go `showMetaExec::Next` 的 session 时区处理，避免混淆诊断字符串与 SQL 展示时间。
- Rust 单元测试继续放在独立的 `cmd_test.rs`/`parity_test.rs`，不要嵌入生产源文件；保持与 `cmd_test.go` 的全量、V2 小表、加密和 SQL 时区意图对齐。

## 验证依据

- Rust 源码：`br/pkg/task/show/cmd.rs`（全部 337 行），核对全部公开结构、impl、函数、分支、通道和错误路径。
- crate 边界：`br/pkg/task/show/Cargo.toml`、`br/pkg/task/show/lib.rs` 以及根 `Cargo.toml` workspace 成员清单；确认 library 路径、无依赖声明、模块装配与公开再导出。
- 直接依赖：`br/pkg/task/show/stubs.rs` 中 `Context`、`oracle::format_show_ts`、`MetaReader`、`ReadSchemasFiles`、`NewMetaReader`、`ReadBackupMeta`，确认取消模型、桩边界和生产者发送行为。
- Go 对照：`br/pkg/task/show/cmd.go`；生产入口 `pkg/executor/brie.go` 的 `showMetaExec::Next`；测试 `br/pkg/task/show/cmd_test.go`。
- Rust 独立测试：`br/pkg/task/show/cmd_test.rs` 覆盖真实 fixture 字节、全量/V2/加密结果与时区；`br/pkg/task/show/parity_test.rs` 覆盖投影、RawKV、版本边界、schema/CreateExec 错误、取消、通道聚合和资源释放。
- RustCodeGraph：`status` 报告索引含 7032 个 Rust 文件；`files --filter br/pkg/task/show` 确认同目录 7 个 Go/Rust 文件；`node --file br/pkg/task/show/cmd.rs --offset 1 --limit 500` 返回完整文件、16 个符号及 19 个引用文件。`query CreateExec` 与 `query collectResult` 定位到 Rust/Go 对应定义；`callers CreateExec` 在 30 秒内未返回结果，故调用者结论另以仓库搜索复核。
- 结构验收使用任务指定命令，要求文件存在且恰有 11 个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
