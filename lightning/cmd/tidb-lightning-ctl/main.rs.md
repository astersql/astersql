# [`lightning/cmd/tidb-lightning-ctl/main.rs`](main.rs)

## 文件定位

本文件是 `tidb-lightning-ctl` Rust crate 的控制面主流程，而不是最外层可执行文件。进程从 `bin_main.rs::main` 进入 `astersql_lightning_cmd_tidb_lightning_ctl::main`，经 `lib.rs::main` 转发到本文件的 `entry::main`。`Cargo.toml` 将该目录同时声明为以 `lib.rs` 为入口的库和以 `bin_main.rs` 为入口的二进制；移植元数据把它标记为 Go 包 `lightning/cmd/tidb-lightning-ctl` 对应的 binary。

该文件负责把命令行解析结果变成集群压缩、TiKV 模式查询/切换或 checkpoint 管理动作。实际参数、PD/TiKV 边界和部分配置适配统一由同目录 `stubs.rs` 导入；checkpoint 控制和模式切换则由 `astersql-lightning-pkg-server` 提供，完整压缩级别常量来自 `astersql-lightning-pkg-importer`。因此它是命令编排层，不承载 checkpoint 数据库或真实网络协议的具体实现。

## 核心职责

- `main`、`main_with_args`、`run_main` 划分进程入口、可注入参数入口和可测试退出码边界；FIPS 钩子在任何参数处理之前调用。
- `run`/`run_loaded` 完成配置装载、配置调整、TLS 初始化、PD client 创建和确定性关闭。
- `dispatch` 按固定优先级选择且只执行一个动作：`compact`、`fetch-mode`、`switch-mode`、checkpoint remove、ignore error、destroy error、dump、检查本地中间文件，最后才显示 usage。
- `formatFatalError` 和 `formatFatalErrorStacked` 区分“checkpoint 表不存在”这种可由用户纠正的错误与一般错误；前者追加可执行示例并抑制栈样式噪声。
- `compactCluster` 和 `fetchMode` 把集群级动作映射到状态不高于 `Offline` 的各个 store。

当前实现必须结合 `stubs.rs` 理解：PD client、TiKV compact/fetch 和 CLI 解析是为 arm64/迁移验证准备的本地兼容层，部分路径不会发真实网络请求；不能仅凭本文件宣称所有生产网络能力已经接通。

## 主要符号

- `pub fn main()`：收集去掉 argv[0] 后的进程参数并调用 `main_with_args`。
- `pub fn main_with_args(args: Vec<String>)`：先调用 `fips::enable_fips_only`，再执行 `run_main`；只有非零结果才调用可替换的 `call_exit`。
- `pub const checkpointTableNotFoundUsage`：checkpoint 缺失错误附带的三类操作示例。
- `pub fn formatFatalError(&Error) -> String`：按 `ErrCheckpointTableNotFound.Equal` 的错误类别身份进行特判；普通 `Error` 路径附加当前命令路径作为栈样式后缀。
- `pub fn formatFatalErrorStacked(&StackError) -> String`：供带调用位置的兼容错误使用；普通分支委托 `ErrorStack`。
- `pub fn run(Vec<String>) -> Result<()>`：库/测试可复用的错误返回式入口，依次调用 `LoadGlobalConfigWithCtl` 和 `run_loaded`。
- `pub fn run_main(Vec<String>) -> i32`：进程可观察的退出码适配层。帮助为 `0`，参数/配置装载错误为 `2`，动作执行错误为 `1`，成功为 `0`。
- `fn run_loaded((GlobalConfig, CtlActionFlags, FlagSet)) -> Result<()>`：把全局配置转换成 server 配置、建立 TLS 和 PD client、调用 `dispatch`，并在返回前显式 `Close`。
- `fn dispatch(...) -> Result<()>`：唯一的动作选择器。它是私有函数，动作 flags、配置、TLS、client 与 usage 都由上层注入。
- `pub fn compactCluster(...) -> Result<()>`：通过 `ForAllStores` 对 `Up`、`Offline` store 调用 `Compact(..., FullLevelCompact, "")`，跳过 `Tombstone`。
- `pub fn fetchMode(...) -> Result<()>`：并行遍历同一 store 集合并打印各节点模式；单节点 `FetchMode` 失败只打印，不让回调失败。

文件自身没有定义 struct、enum、trait 或 `impl`，也没有条件编译项；主要公开面为上述进程/测试入口、错误格式化函数和两个集群动作。`run_loaded`、`dispatch` 则刻意保持内部实现。

## 执行流程

1. `bin_main.rs::main` 调用库的 `main`，`lib.rs::main` 再调用 `entry::main`；后者收集参数并进入 `main_with_args`。
2. `main_with_args` 先触发 FIPS 初始化，然后用 `run_main` 获取退出码。成功/帮助不触发退出钩子，错误才调用 `call_exit`。
3. `run_main` 调用 `LoadGlobalConfigWithCtl`。帮助请求直接返回 `0`；其他 flag 或配置错误打印后返回 `2`。装载成功后转入 `run_loaded`，业务错误写入 stderr 并返回 `1`。
4. `run_loaded` 创建后台 context 和 `config::Config`，经 `LoadFromGlobal` 拷贝 ctl 所需配置，调用 `Adjust` 补默认值并校验，再分别构造 ctl TLS 和 TiDB security TLS 配置。
5. PD 地址按逗号原样切分后交给 `NewClient("lightning-ctl", ...)`；空地址和连续逗号形成的空端点不会在本层过滤。
6. `dispatch` 采用“首个命中立即返回”的互斥优先级。集群动作直接复用 PD client；checkpoint 动作只在命中相应 flag 时延迟创建 `CheckpointControl`。
7. `check-local-storage` 将 `None` 或空 map 都视为没有丢失中间文件，否则仅输出表名集合；无动作时调用 `FlagSet::Usage` 并成功返回。
8. 无论 `dispatch` 成功或报错，`run_loaded` 都在返回结果前显式调用 `cli.Close`。

优先级意味着同时传入多个动作参数时不会组合执行，例如 `--compact` 会遮蔽之后的 switch-mode 与 checkpoint flags。新增动作时必须明确其插入位置，否则会改变既有多 flag 输入的可观察行为。

## 数据与状态

主流程持有三组输入状态：`GlobalConfig` 表示通用 Lightning 配置，`CtlActionFlags` 表示本次控制动作，`FlagSet` 保存 usage 行为。它们由 `LoadGlobalConfigWithCtl` 一次性返回；`run_loaded` 随后把全局配置投影为 server `config::Config`。当前投影只覆盖 ctl 实际访问字段，不是完整 Lightning 配置克隆。

`CtlActionFlags` 中 bool 控制 compact、fetch-mode 和 local-storage 检查，字符串非空控制 mode 与四类 checkpoint 操作。`usage_invoked` 是共享原子标记；`dispatch` 在调用 `Usage` 后以 `SeqCst` 读取一次，但不根据结果分支，实际用途主要是兼容层中的可观察测试状态。

PD client 保存名称、按原序切分的 endpoints、关闭原子标记和 store 快照。`run_loaded` 对它只借用，不把 client 泄漏到函数外。checkpoint controller 也只在单一动作分支内创建和使用。

本文件没有长期全局业务状态。可替换退出函数、最近 client 状态和 TiKV mock 记录位于 `stubs.rs`，属于进程级测试状态；相应测试通过子进程隔离退出钩子，避免 Rust 并行测试相互污染。

## 依赖与调用关系

上游调用链为 `bin_main.rs::main → lib.rs::main → entry::main → main_with_args → run_main → run_loaded → dispatch`。此外，`main_test.rs` 直接调用 `main_with_args`、`run_main` 和错误格式化函数；`parity_test.rs` 直接调用 `run`、`run_main`、`compactCluster`、`fetchMode` 及格式化函数。RustCodeGraph 的文件关系也记录 `main.rs` 被 `parity_test.rs` 使用。

下游关系主要包括：

- 参数与配置：`LoadGlobalConfigWithCtl`、`LoadFromGlobal`、`config::Config::NewConfig`、`Adjust`。
- 安全与连接：`fips::enable_fips_only`、`ToTLS`、`BuildTLSConfig`、`WithTLSConfig`、`NewClient`、`PdClient::Close`。
- 集群动作：`ForAllStores → Compact` 或 `FetchMode`；mode 切换调用 server 的 `SwitchMode`。
- checkpoint：`NewCheckpointControl` 根据 backend 选择 import-into 或 legacy controller，再调用 `Remove`、`IgnoreError`、`DestroyError`、`Dump` 或 `GetLocalStoringTables`。
- 错误：`errors::Trace` 包装动作错误，`ErrCheckpointTableNotFound.Equal` 做类别匹配，`ErrorStack` 输出带位置的错误。

`Cargo.toml` 直接依赖 `astersql-lightning-pkg-importer` 和 `astersql-lightning-pkg-server`。`stubs.rs` 从 server 重导出错误、配置、context、TLS、checkpoint 与 mode-switch 接口，并在本地实现精简的 flag/PD/TiKV 边界，这正是本文件只需 `use crate::stubs::*` 的原因。

## 错误处理与边界

错误边界分三层。参数/配置装载错误由 `run_main` 转换成退出码 `2`；帮助请求字符串 `flag: help requested` 被视为正常退出 `0`。进入 `run_loaded` 后的配置调整、TLS、client 下游或动作错误都传播为 `Result`，最终由 `run_main` 格式化到 stderr 并映射为 `1`。`run` 则保留原始 `Result`，适合库调用和测试。

checkpoint 缺失必须按错误 class 精确识别，而不是按文本相似性匹配。命中后输出原错误和 `checkpointTableNotFoundUsage`，不附加栈；普通错误仍保留栈样式信息。`parity_test.rs::config_alias_order_and_checkpoint_error_identity_match_go` 明确验证同文案但无该 class 的错误不会得到恢复提示。

`fetchMode` 有意吞掉单 store 查询错误：它将错误写到 stderr，回调仍返回 `Ok(())`，使其继续巡检其他节点。与之不同，`compactCluster` 的 compact 错误会由 `ForAllStores` 保留首个错误并向上传播。

重要输入边界包括：PD 地址 split 不清洗空片段；store 状态上限 `Offline` 是包含式；`None` 与空表集合在 local-storage 输出中等价；无任何动作时打印 usage 并返回成功。checkpoint remove/destroy 等破坏性语义由 server controller 负责，本层只在相应字符串非空时分派，不增加二次确认。

## 并发与资源生命周期

本文件本身不创建线程、async task、通道、锁或事务。并发出现在下游 `ForAllStores`：它为每个状态不高于 `Offline` 的 store 创建 scoped thread，共享派生 context；首个回调错误触发取消，并在所有线程结束后返回首个错误，成功路径等待结束后同样取消派生 context。完成顺序不稳定，因此测试按 store 集合而非调用顺序断言。

`compactCluster` 的各 store 回调可能并行执行。`fetchMode` 也并行查询；其回调把单点错误转换成打印后成功，所以这类错误不会取消其他节点。

PD client 生命周期由 `run_loaded` 集中管理：构造后调用一次 `dispatch`，随后在返回结果之前显式 `Close`，覆盖成功、usage 和动作失败路径；`PdClient::Drop` 还提供兜底关闭。`main_test.rs::run_main_worker` 和 `parity_test.rs::contract_resource_cleanup` 分别验证业务失败/正常返回之前已经关闭，以及显式关闭和 Drop 兜底。

checkpoint controller 是分支局部值，操作返回后即释放。legacy controller 的数据库打开/关闭细节在 `lightning/pkg/server/checkpoint_control.rs`，不属于本文件；扩展时不要在 `dispatch` 中复制该资源管理逻辑。

## 与 Go 版本的对应关系

直接对照文件是同目录 `main.go`，总体结构保持一致：配置装载与调整、TLS、PD client、互斥动作顺序、checkpoint controller 的按需创建、usage fallback，以及 `compactCluster`/`fetchMode` 的 store 遍历参数均有明确对应。`checkpointTableNotFoundUsage` 的文本和特殊错误格式也来自 Go 实现。

Rust 为可测试性拆出了 Go 中没有的层次：`main_with_args` 显式接收 argv，`run_main` 返回数字退出码，`run_loaded` 接收已解析三元组，`dispatch` 单独承担动作选择。Go 的 `config.Must`/`exit` 行为由这些层和 `call_exit` 共同表达。Go 的 `defer cli.Close()` 在 Rust 中变为 `dispatch` 后显式 `Close` 加 `Drop` 兜底。

另一个重要差异是依赖接线。Go 直接使用 PD HTTP、TiKV、Lightning config/common/server/importer；当前 Rust crate 为 arm64 开发环境使用本地 `stubs.rs` 模拟 CLI、PD/TiKV 网络边界，仅 server checkpoint/mode-switch 与 importer 常量通过直接 crate 依赖复用。因此文档和扩展评审应区分“与 Go 可观察语义对齐”与“已接入同等真实基础设施”。

测试对应关系为 `main_test.go::TestRunMain` 对 `main_test.rs::test_run_main`，涵盖入口执行、checkpoint 缺失不打印栈和普通错误保留栈；额外的 `parity_test.rs` 覆盖 Go 布尔 flag 语法、alias 后值覆盖、动作分派、store 状态边界、并行取消、非法 mode、端点 split 和 client 释放。

## 扩展指南

新增控制动作时，先在参数层 `stubs.rs::CtlActionFlags` 与 `LoadGlobalConfigWithCtl` 注册并解析 flag，再在 `dispatch` 中选择明确优先级。若动作需要新配置字段，还要同步 `GlobalConfig` 到 `config::Config` 的 `LoadFromGlobal` 投影；不要假设当前投影完整。网络或存储行为应放入 canonical crate，由本层调用，不应继续把真实业务堆入本地 stub。

新增 PD/TiKV 集群动作可仿照 `compactCluster`/`fetchMode`，但必须先决定单节点错误是 fail-fast/取消还是打印后继续，并为并行无序完成编写集合式断言。调整资源创建位置时，应保持 `run_loaded` 对 client 的所有返回路径关闭；如引入新的 client、文件或 checkpoint DB，应由拥有者建立同样明确的释放边界。

改变错误类型或文案时，需要同时检查 `formatFatalError` 的 class 身份判断和退出码分层，避免把用户可纠正错误误报为带栈内部错误。改变多 flag 行为会影响 `dispatch` 的首命中契约，应当显式记录兼容性风险。

测试应继续放在独立文件，不嵌入 `main.rs`：入口/退出码与错误格式同步更新 `main_test.rs`，Go/Rust 外部契约和边界同步更新 `parity_test.rs`；若语义源自 Go，还应核对 `main.go` 与 `main_test.go`。涉及 checkpoint controller 的内部行为应在 `lightning/pkg/server/checkpoint_control_test.rs` 等所属模块测试，而不是让 ctl 入口测试承担全部覆盖。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning-ctl` 列出本 crate 的 Rust/Go 入口、测试和 stub。
- RustCodeGraph `node --file lightning/cmd/tidb-lightning-ctl/main.rs`：读取完整 237 行源码，确认 15 个索引符号、文件被 `parity_test.rs` 使用，以及入口、分派、格式化和两个集群动作的实现。
- RustCodeGraph `query`：确认 `run_loaded`、本文件 `dispatch`、`compactCluster`、`fetchMode`、`run_main`，以及 `stubs.rs` 中 `LoadGlobalConfigWithCtl`、`NewClient`、`ForAllStores`、`call_exit` 和 server 的 `NewCheckpointControl` 的真实位置。
- RustCodeGraph `node`：读取 `lib.rs`、`bin_main.rs`、`stubs.rs` 相关片段、`main_test.rs`、`parity_test.rs`、Go `main.go` 与 `main_test.go`，并读取 `lightning/pkg/server/checkpoint_control.rs` 的 trait、factory 和 legacy 资源边界。
- `lightning/cmd/tidb-lightning-ctl/Cargo.toml`：核对 library/binary 入口、Go 包映射、binary 移植类型和两个直接依赖。
- 精确图命令 `rustcodegraph callers run_loaded` 在本地索引上超过 60 秒未返回并被中止；调用关系改由完整已索引源码、文件使用关系和直接测试调用交叉验证，不据此推断缺失调用边。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级章节的结构命令和人工事实复核验收。
