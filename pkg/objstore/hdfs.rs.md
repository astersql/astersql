# `pkg/objstore/hdfs.rs`

## 文件定位

`hdfs.rs` 是 `astersql-objstore` crate 中的 HDFS 后端实现，由 `pkg/objstore/lib.rs` 的 `pub mod hdfs` 纳入模块树。它把统一存储接口适配到本机的 Hadoop 命令行程序，而不是链接 HDFS 客户端库：运行时通过 `$HADOOP_HOME/bin/hdfs dfs ...` 子进程访问远端。

完整构造链是 `parse::ParseBackend` / `parseBackend` 将 `hdfs:` URI 原样保存为 `StorageBackend::Hdfs(Hdfs { remote })`，随后 `storage::New` 在该枚举分支调用 `hdfs::NewHDFSStorage`，得到 `Arc<dyn storage::Storage>`。因此本文件位于“后端 URI 解析”与“统一对象存储调用者”之间；`pkg/objstore/storage.rs:256-288` 和 `pkg/objstore/parse.rs:250-329` 是它的直接入口证据。

## 核心职责

- `HDFSStorage` 保存 HDFS 根 URI，并为统一接口暴露根 URI。依据：`HDFSStorage::new`、`HDFSStorage::remote`、两个 `URI` 实现。
- 将对象名简单拼成 `{remote}/{name}`，以 `hdfs dfs -put - <path>` 完整写入对象，以 `hdfs dfs -ls <path>` 判断对象是否存在。依据：`file_path`、`write_file`、`file_exists`。
- 从环境变量解析命令执行方式：`HADOOP_HOME` 决定 `hdfs` 二进制路径，存在 `HADOOP_LINUX_USER` 时改为 `sudo -u USER <hdfs-bin> dfs ...`。依据：`get_hdfs_bin`、`get_linux_user`、`dfs_command_spec`。
- 同时实现 `storeapi::Storage` 与 crate 内的 `storage::Storage`，让两套迁移期间并存的接口复用同一写入/存在性逻辑。当前能力刻意受限于 rawkv 备份：除完整写入、存在性检查、URI 和空 `Close` 外，其余对象操作均拒绝。依据：两个 trait impl 与 `unsupported_hdfs_operation`。

本文件不是通用 HDFS 文件系统客户端，也不提供读取、删除、列举、流式读写、重命名或预签名能力。

## 主要符号

- `pub struct HDFSStorage { remote: String }`：后端实例的全部持久状态；`Clone + Debug`，无内部客户端、锁或连接池。
- `HDFSStorage::new(remote: impl Into<String>) -> Self`：Rust 风格构造器；只保存字符串，不检查 URI，也不访问环境或网络。
- `NewHDFSStorage(remote: String) -> HDFSStorage`：保留 Go 命名形状的公开兼容入口；`storage::New` 使用它。
- `HDFSStorage::remote(&self) -> &str`：借用根路径；两套 trait 的 `URI` 则克隆并返回所有权字符串。
- `file_path(&self, name: &str) -> String`：私有路径拼接器，固定插入一个 `/`，不清理重复斜杠、`.`、`..`，也不转义命令参数；参数通过 `Command` 的独立 argv 传递，不经过 shell 展开。
- `write_file(&self, name, data) -> Result<()>`：私有共享实现，将数据写入子进程 stdin，等待进程退出，并在非零退出时合并 stdout/stderr 形成错误。
- `file_exists(&self, name) -> Result<bool>`：私有共享实现；成功退出为 `true`，任意非零退出为 `false`，启动命令失败才返回错误。
- `get_hdfs_bin() -> Result<PathBuf>`：要求 `HADOOP_HOME` 已设置，返回其 `bin/hdfs` 子路径，但不提前验证文件是否存在或可执行。
- `get_linux_user() -> Option<OsString>`：读取可选 `HADOOP_LINUX_USER`，保留非 UTF-8 环境值直到命令规格构建时以 lossy 方式转换。
- `pub struct CommandSpec { program, args }`：可测试、可比较的命令规格；私有 `command` 将其转换为 `std::process::Command`。
- `dfs_command_spec(args) -> Result<CommandSpec>` / `dfs_command(args) -> Result<Command>`：分别构造可检查的命令描述和可执行命令。
- `unsupported_hdfs_operation() -> anyhow::Error`：为大多数不支持的方法生成统一的 rawkv 限制错误。
- `impl storeapi::Storage` 与 `impl crate::storage::Storage`：两套接口适配；后一实现额外提供 `as_any`，供工厂测试和运行时类型识别。

本文件没有模块级常量、条件编译项或异步函数。

## 执行流程

构造流程：

1. `parseBackend` 识别 `hdfs` scheme，将原始 URI 保存到 `StorageBackend::Hdfs`，不剥离 host、path 或 query。
2. `storage::New` 命中 HDFS 分支，调用 `NewHDFSStorage(hdfs.remote.clone())`；这里不读取 `HADOOP_HOME`，也不受传入 `Context` 的取消状态影响。
3. `NewHDFSStorage` 委托 `HDFSStorage::new`，实例只持有 `remote`。

完整写入流程（两套 trait 的 `WriteFile` 最终都进入 `write_file`）：

1. `file_path` 拼接根 URI 与逻辑对象名。
2. `dfs_command(&["-put", "-", path])` 构造 HDFS 命令；无切换用户时为 `$HADOOP_HOME/bin/hdfs dfs -put - <path>`，有用户时为 `sudo -u <user> $HADOOP_HOME/bin/hdfs dfs -put - <path>`。
3. 子进程的 stdin/stdout/stderr 都设为管道；父进程把整个 `data` 写入 stdin，随后 `wait_with_output` 等待退出并收集输出。
4. 零退出码返回 `Ok(())`；非零退出码把 stdout 后接 stderr，以 lossy UTF-8 转成消息，并连同退出状态返回错误。

存在性检查流程（两套 `FileExists` 都进入 `file_exists`）：构造 `hdfs dfs -ls <path>`，用 `Command::output` 等待完成；仅以退出状态判断结果，零为 `true`、非零为 `false`。该逻辑不会区分“不存在”、权限失败、HDFS 服务端错误等不同非零原因。

其他 trait 方法不启动命令：`URI` 返回根 URI；`Close` 为空；其余方法立即返回不支持错误。

## 数据与状态

实例状态只有拥有所有权的 `remote: String`。构造后本文件不修改它；克隆 `HDFSStorage` 会复制该字符串。对象路径不是规范化后的结构，而是每次调用 `file_path` 临时分配出的字符串，所以调用方必须保证根路径和名称的拼接语义正确。

进程级配置来自两个环境变量：

- `HADOOP_HOME`：必需，但延迟到第一次需要构造命令时才检查；缺失时产生 `please specify environment variable HADOOP_HOME`。
- `HADOOP_LINUX_USER`：可选；只要变量存在就启用 `sudo -u` 路径，包括值为空的情况。

`CommandSpec` 是一次性命令描述，`program` 与 `args` 均拥有其数据。写入数据本身不会存入 `HDFSStorage`，而是同步送入一次性子进程。两个传入的 context 类型均未读取，因此取消标志不属于本后端的状态机。

## 依赖与调用关系

crate 边界由 `pkg/objstore/Cargo.toml` 确认：库入口为 `lib.rs`、`autotests = false`，本文件直接使用外部依赖 `anyhow`；`objectio` 与 `storeapi` 是同仓库路径依赖并由 crate 根再导出。进程管理、环境、路径和同步写入均来自标准库。

上游直接关系：

- `parse::ParseBackend` / `parseBackend`：把 HDFS URI 建模成 `StorageBackend::Hdfs`。
- `storage::New`、`NewWithDefaultOpt`、`NewFromURL`：统一工厂最终构造 `HDFSStorage`；HDFS 不走 `Options::external_factory`。
- 统一接口调用者通过 `storeapi::Storage` 或 `crate::storage::Storage` 动态分派到本文件；`storage_test.rs::test_new_hdfs_storage` 验证了工厂结果类型和 URI。

下游直接关系：

- `get_hdfs_bin`、`get_linux_user` 读取进程环境。
- `dfs_command_spec` 和 `CommandSpec::command` 生成 `std::process::Command`。
- `write_file`、`file_exists` 调用外部 `hdfs`（可经 `sudo`）并同步等待子进程。
- 两套 trait 的返回类型分别来自 `objectio`/`storeapi` 与 `crate::storage`；不支持的方法只需满足这些接口的类型边界，不会创建 Reader/Writer。

RustCodeGraph 的目标文件节点显示该模块被 21 个文件使用，并准确索引出 48 个符号；对重名 trait 方法执行精确 `callers/callees` 未产生可辨识结果。因此这里仅把源码和工厂分支能直接确认的关系列为调用边，不据此推断所有间接业务调用者。

## 错误处理与边界

- 缺少 `HADOOP_HOME`：`get_hdfs_bin` 立即返回 `anyhow` 错误；路径不存在、不可执行或 `sudo` 不可用则在 `spawn`/`output` 阶段返回带 `failed to start ...` 上下文的 I/O 错误。
- 写 stdin 失败、等待失败使用 `?` 原样向上传播底层错误。HDFS 非零退出时同时保留 stdout、stderr 和 `ExitStatus`；`hdfs_test.rs::write_file_error_includes_combined_output_and_exit_status_like_go` 固定验证这三部分。
- `file_exists` 把所有已启动进程的非零退出都解释为“不存在”，不会上报 stderr。与 Go 实现相比，这保留了普通 `ExitError -> false` 的主路径，但 Rust 当前没有 Go 中对非 `ExitError` 等待错误附加输出的分支；这是扩展诊断能力时必须留意的语义边界。
- `ReadFile`、`DeleteFile`、`DeleteFiles`、`Open`、`WalkDir`、`Create`、`Rename` 都返回 `currently HDFS backend only support rawkv backup`。`PresignFile` 使用单独消息 `HDFS backend does not support PresignFile`。
- 路径仅字符串拼接；本文件不阻止绝对名称、父目录片段、空名称、根 URI 尾随 `/` 或名称前导 `/`。
- 两套接口的 context 参数均被命名为 `_ctx` 并忽略。测试明确验证，即使 context 已取消，写入和存在性检查仍继续执行。

## 并发与资源生命周期

`HDFSStorage` 只含不可变 `String`，满足 `storage::Storage` 所要求的 `Send + Sync`，不同线程可并发调用；每次写入或检查都会创建独立子进程，没有共享连接、缓存、锁或后台任务。本文件本身不串行化同一路径的并发写入，也不提供原子覆盖保证，冲突行为由 `hdfs dfs -put` 和 HDFS 决定。

`write_file` 同步持有子进程：取得 stdin 后写完全部数据，再调用 `wait_with_output` 回收进程并收集 stdout/stderr。函数返回时 child 已被等待，不遗留由本代码管理的后台任务。由于 stdout/stderr 从一开始即为管道、但在 stdin 写完前没有主动排空，若子进程在消费完输入前产生超过管道容量的大量输出，理论上存在双方阻塞风险；修改大数据或高输出场景时应针对这一生命周期设计回归测试。

`file_exists` 使用 `output()` 完成启动、等待和输出回收。两个 `Close` 都是空操作，因为实例没有长寿命外部资源。环境变量是进程全局可变状态；现有 Rust 测试用 `#[serial]` 串行化环境修改，并在用例结束前清理 `HADOOP_HOME`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/hdfs.go`。主要对应如下：

- Go `HDFSStorage.remote`、`NewHDFSStorage` 对应 Rust 同名结构和兼容构造器；Rust 另提供惯用的 `new`、`remote`。
- Go `getHdfsBin`、`getLinuxUser`、`dfsCommand` 对应 Rust `get_hdfs_bin`、`get_linux_user`、`dfs_command_spec`/`dfs_command`。Rust 引入 `CommandSpec` 将“命令是什么”与“启动命令”分开。
- Go `WriteFile` 用 `bytes.Buffer` 作为 stdin 并调用 `CombinedOutput`；Rust 直接向 piped stdin 写入，再以 `wait_with_output` 合并 stdout 和 stderr。独立测试要求错误文本仍包含两路输出与退出状态。
- Go `FileExists` 把 `*exec.ExitError` 当作不存在，其他错误带输出返回；Rust 以退出状态成功与否直接返回布尔值，启动失败带上下文返回。通常的非零退出行为对齐，但错误分类和诊断细节并非完全等价。
- Go 只实现一套存储接口；Rust 为迁移中的 `storeapi::Storage` 和 crate 内 `storage::Storage` 提供平行实现，并共用私有核心逻辑，避免两套行为漂移。
- Go 与 Rust 都忽略传入 context，并都把读取、删除、遍历、流式创建、重命名限制为不支持；`PresignFile` 有独立错误，`Close` 为空。

仓库中没有同路径 `hdfs_test.go`；当前 HDFS 专项回归位于独立文件 `pkg/objstore/hdfs_test.rs`，工厂构造回归位于 `pkg/objstore/storage_test.rs`。因此不能声称存在 Go 专项测试覆盖，只能以 Go 生产实现和 Rust 独立测试交叉核对。

## 扩展指南

- 新增真正的读、删、遍历、流式 I/O、重命名能力时，应分别替换两个 trait impl 中对应的不支持分支，并优先抽取像 `write_file`/`file_exists` 一样的私有共享实现，防止两套接口行为分叉。测试必须放在独立的 `pkg/objstore/hdfs_test.rs`，不要内嵌进生产文件。
- 修改命令行拼装时先改 `dfs_command_spec`，同时覆盖无 `HADOOP_LINUX_USER` 与 `sudo -u` 两条 argv；不要改成 shell 字符串，否则会引入路径/用户名转义与注入差异。
- 改善存在性错误分类时应以 Go `FileExists` 为兼容基线，区分“命令已运行但目标不存在”和权限、连接、启动/等待失败，并增加 stderr/退出码回归；不能只把所有非零退出继续吞成 `false`。
- 引入取消支持时要同时处理两种 context 类型，并明确取消后是否终止已启动子进程；现有测试把“忽略已取消 context”固定为当前兼容行为，语义变化必须同步更新测试和调用方预期。
- 优化大对象写入或高输出命令时应审视 stdin 与 stdout/stderr 的并发排空，避免管道背压死锁；同时保留失败时两路输出和退出状态。
- 若修改 URI/路径拼接，需联动 `parse.rs` 中 HDFS 原始 URI 保留策略、`storage.rs` 的工厂分支，以及 `storage_test.rs` 的 URI 断言，评估重复斜杠、特殊字符和父路径片段的兼容风险。
- 若增加长寿命客户端、线程或缓存，必须重新实现 `Close` 的资源释放并审查 `Clone`、`Send + Sync` 和并发写同一路径的语义；当前空 `Close` 只因没有持久资源才安全。

## 验证依据

本说明基于以下直接证据：

- 目标实现：`pkg/objstore/hdfs.rs` 全部 297 行；关键符号包括 `HDFSStorage`、`write_file`、`file_exists`、`dfs_command_spec` 及两套 `Storage` impl。
- crate 与模块边界：`pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`。
- 构造入口：`pkg/objstore/parse.rs` 的 `StorageBackend::Hdfs` 分支和 `pkg/objstore/storage.rs` 的 `storage::New` HDFS 分支。
- Go 对照：`pkg/objstore/hdfs.go` 全部 161 行。
- 独立 Rust 测试：`pkg/objstore/hdfs_test.rs` 的错误输出/退出状态回归与取消 context 回归；`pkg/objstore/storage_test.rs::test_new_hdfs_storage`、`test_new_hdfs_storage_ignores_cancelled_context` 的工厂回归。
- RustCodeGraph：`status` 确认本地索引包含 11,467 文件、307,296 节点、1,848,419 边；`files --filter pkg/objstore` 确认目标、Go 对照和测试均已索引；`node --file pkg/objstore/hdfs.rs` 返回完整源码、48 个符号及 21 个文件的使用关系；`query HDFS`、`query NewHDFSStorage --kind function --json`、`query dfs_command_spec --kind function --json`、`query write_file --kind function --json` 用于消歧关键符号。精确 `callers/callees` 未返回可辨识边，调用关系因此由已索引源码中的直接工厂和委托语句复核。
- 人工边界检查：确认没有条件编译、没有内部锁/任务/通道/事务，没有 `hdfs_test.go`，且生产文件未嵌入测试。

按任务约束，本次是纯文档分析，未运行 Cargo 或代码测试。交付结构检查要求本文恰有上述 11 个固定二级标题。
