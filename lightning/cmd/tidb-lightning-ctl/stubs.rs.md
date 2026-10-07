# `lightning/cmd/tidb-lightning-ctl/stubs.rs`

## 文件定位

源码入口：[`stubs.rs`](./stubs.rs)。它是 `astersql-lightning-cmd-tidb-lightning-ctl` crate 的本地兼容层。crate 由同目录 `Cargo.toml` 定义为同时具有库入口 `lib.rs` 和二进制入口 `bin_main.rs` 的工具；`lib.rs` 以 `pub mod stubs` 声明本文件并重导出其公开符号，`main.rs` 再通过 `use crate::stubs::*` 使用这些符号。二进制调用链为 `bin_main.rs::main` → `lib.rs::main` → `main.rs::main`/`main_with_args`。

本文件服务于 arm64-safe 的裁剪构建：Cargo 只直接依赖 `astersql-lightning-pkg-importer` 和 `astersql-lightning-pkg-server`，本文件从 server crate 重导出 `Error`、`Result`、`config`、`context`、`common`、`pdhttp`、`tls`、`CheckpointControl`、`SwitchMode` 等接口，并在本地补足 Go 控制程序所需但精简依赖未提供的 flag、配置装载、PD/TiKV 边界。它是会进入生产 crate 的兼容实现，不是测试文件；但其中 PD/TiKV 网络行为明确是可观测 mock，不能当作完整生产网络客户端。

## 核心职责

本文件承担四组职责：

1. 提供 Go 风格错误身份与栈展示适配：`CheckpointNotFound`、`ErrCheckpointTableNotFound`、`StackError`、`ErrorStack`。
2. 用 `FlagSet` 复现 ctl 使用到的 `flag.FlagSet` 子集，并由 `LoadGlobalConfigWithCtl` 合并默认值、极小配置文件子集和命令行覆盖，产出 `GlobalConfig` 与 `CtlActionFlags`。
3. 将上述中间配置投影到 server crate 的 `config::Config`，并提供 `ToTLS`/`TlsExt`、退出钩子以及 PD client 生命周期适配，使 `main.rs::run_loaded` 能保持 Go 主流程形状。
4. 在不建立真实网络连接的前提下，保留 ctl 可观察的 PD/TiKV 控制协议：store 状态过滤与并行遍历、compact 请求记录、fetch-mode 结果注入，以及 Prometheus 指标解析。

边界必须明确：`PdClient::GetStores` 只读取内存快照，`Compact` 只记录调用，`FetchMode` 只读取 mock map，`TlsExt::TLSConfig` 返回默认空配置，`PdClient::as_server_client` 只投影成 server stub 类型。注释和同目录 parity 测试均把这些行为定义为兼容桩，而不是实际 PD HTTP、TiKV gRPC 或 TLS 实现。

## 主要符号

- `metapb::{StoreState_Up, StoreState_Offline, StoreState_Tombstone}`：显式保留 Go/kvproto 的数值顺序 `0/1/2`，供 `ForAllStores` 做包含上界比较；不能替换为 server stub 的不同哨兵值。
- `CheckpointNotFound::{GenWithStackByArgs, Equal}` 与静态量 `ErrCheckpointTableNotFound`：通过 `Error.class == "Lightning:Checkpoint:ErrCheckpointTableNotFound"` 保留 RFC 错误身份；`main.rs::formatFatalError` 依赖身份而非文案相似度来决定是否附加恢复提示。
- `StackError::{new, Error}`、`ErrorStack`：利用 `#[track_caller]` 保存调用文件和行号，并稳定输出“错误正文 + 调用点”，仅用于复现 Go `errors.ErrorStack` 的测试可见部分。
- `FlagVal`、`Flag`、`FlagRef`、`FlagSet`：分别表示受支持的 bool/string/int 值、注册元数据、共享可变句柄和完整 flag 注册表。`FlagSet::Parse` 支持 `-k=v`、`-k v`、裸布尔 flag、`--` 终止和遇首个位置参数后停止解析。
- `GlobalTiDB`、`GlobalMydumper`、`GlobalImporter`、`GlobalCheckpoint`、`GlobalLightning`、`GlobalConfig`：ctl 可见的全局配置中间模型；`NewGlobalConfig` 提供默认值。
- `CtlActionFlags`：保存 compact、fetch mode、switch mode、四类 checkpoint 操作和本地中间文件检查动作，另以 `usage_invoked: Arc<AtomicBool>` 暴露 Usage 是否发生。
- `LoadGlobalConfigWithCtl`：本文件的主要输入入口；注册基础 flag 与 ctl flag，修改 `-d` 默认值为 `noop://`，解析输入并按“默认值 → 配置文件 → 非空命令行值”合并。
- `LoadFromGlobal`、`ToTLS`、`TlsExt`：把中间配置映射到 server `config::Config`，构造 ctl 所需的 `common::TLS` 调用面。
- `set_exit_fn`、`reset_exit_fn`、`call_exit`、`Must`：可替换的进程退出边界；`Must` 将 help 映射为退出 0、其他装载错误映射为退出 2。
- `MetaStore`、`StoreInfo`、`StoresInfo`、`PdClient`、`ClientOption`、`WithTLSConfig`、`NewClient`：PD HTTP 边界的精简数据模型与客户端。`PdClient` 保存端点、共享关闭标记和可注入 store 列表。
- `ForAllStores`：读取 store 快照，筛选 `State <= max_state` 的节点，在 scoped threads 中并发执行 action，记录第一个错误并取消派生 context。
- `Compact`、`FetchMode`、`FetchModeFromMetrics`：前两者是网络边界 mock；后者是真实保留的纯文本解析算法。
- `reset_tikv_mocks`、`mock_fetch_mode`、`take_compact_calls`、`clear_last_client_closed`、`take_last_client_closed`、`take_last_client_endpoints`：独立测试使用的可观测状态管理接口。

## 执行流程

主流程由 `main.rs::run` 或 `run_main` 调用 `LoadGlobalConfigWithCtl(args)` 开始。装载函数先调用 `NewGlobalConfig`，建立 `FlagSet` 并注册全局参数；随后将 `d` 的当前值和展示默认值都改为 `noop://`，再注册 ctl 动作参数并执行 `FlagSet::Parse`。`-V` 被转换为 `flag: help requested`，`-config` 与 `-c` 则通过再次按 argv 顺序扫描实现“最后出现者生效”。若指定配置文件，只解析 ctl 当前需要的简单 `pd-addr` 与 `backend` 行；之后非空命令行字段覆盖默认/文件值，并验证 server mode 必须带 status address。

成功时返回 `(GlobalConfig, CtlActionFlags, FlagSet)`。`main.rs::run_loaded` 用 `LoadFromGlobal` 填充 server `config::Config`，调用其 `Adjust`，再通过 `ToTLS` 和 `BuildTLSConfig` 准备安全配置。PD 地址按逗号原样切分后传给 `NewClient`；这意味着空地址和连续逗号产生的空片段也是可观察输入。

随后 `main.rs::dispatch` 按固定优先级只执行第一个命中的动作：compact → fetch mode → switch mode → checkpoint remove/ignore/destroy/dump → 本地存储检查 → Usage。compact 与 fetch mode 分别由 `compactCluster`/`fetchMode` 调用本文件 `ForAllStores`，上界均为 `StoreState_Offline`。前者对每个合格 store 调用 `Compact(FullLevelCompact, "")`；后者调用 `FetchMode`，单节点错误只打印而不会令整个遍历失败。`run_loaded` 在 dispatch 返回后显式 `cli.Close()`，同时 `Drop for PdClient` 提供兜底。

`ForAllStores` 先拒绝已关闭 client，然后克隆 store 列表与派生 context。每个状态不高于上界的 store 各占一个 scoped thread；首个 action 错误被写入 `first_err` 并触发取消，但已启动线程仍由 scope 等待结束。无论成功或失败，等待结束后都会取消派生 context，最后返回首个错误或成功。

`FetchModeFromMetrics` 按行查找固定指标名，并要求指标名前存在字符串开头或非字母数字/下划线边界。它保留指标值到行尾的完整文本：精确值 `0` 为 `import`，其他非空值（包括 `0 `）为 `normal`，未找到指标则返回 `import mode status is not exposed`。

## 数据与状态

`FlagSet.flags` 是 `HashMap<String, Arc<Mutex<Flag>>>`。`Arc<Mutex<_>>` 使 `Lookup` 返回的 `FlagRef` 能在注册后原地修改 `d` 的值与 `DefValue`，同时 `FlagSet` 仍持有同一单元；`args` 保存解析终止后的所有位置参数。getter 遇类型不匹配时返回零值，但会对未知 flag 名使用 `unwrap`，调用约束是只能读取已注册且类型已知的参数。

`GlobalConfig` 是装载期中间状态，`config::Config` 是执行期配置；两者刻意分离。`LoadFromGlobal` 只复制 ctl 会读取的字段，空 backend/sorted-kv-dir 不覆盖 server 默认值。`NewGlobalConfig` 明确设置 `CheckRequirements=true`、checkpoint 开启、TiDB host 为 `127.0.0.1`、用户为 `root`、status port 为 `10080`、日志级别为 `error`。

进程级可变状态包括 `EXIT_FN: Mutex<fn(i32)>`、两个记录最近 PD client 的 `OnceLock<Mutex<Option<_>>>`，以及 TiKV mock 的 `COMPACT_CALLS` 和 `FETCH_MODE_RESULTS`。take 类函数采用取出即清空语义，测试必须在每个场景前调用对应 clear/reset，避免把前一用例的状态当成本次证据。

`PdClient` 的 `closed` 是共享 `Arc<AtomicBool>`，stores 是 `Arc<Mutex<StoresInfo>>`；client 的 clone 会共享生命周期标志与 store 快照。`NewClient` 会把端点向量与关闭标志登记到全局观测槽，`Close` 和 `Drop` 都以 `SeqCst` 写入关闭状态。

## 依赖与调用关系

上游直接调用者是 `main.rs`：`run`/`run_main` 调用 `LoadGlobalConfigWithCtl`，`run_loaded` 调用 `LoadFromGlobal`、`ToTLS`、`WithTLSConfig`、`NewClient`，`compactCluster`/`fetchMode` 调用 `ForAllStores`、`Compact`、`FetchMode`。`main.rs::formatFatalError` 和 `formatFatalErrorStacked` 使用本文件的错误身份与栈适配。`lib.rs` 将整个模块公开重导出，测试因此可直接复用同一接口。

下游依赖主要来自 `astersql-lightning-pkg-server`：错误类型、context、TLS、执行期配置、checkpoint 控制器和 switch-mode 实现都从该 crate 复用；`FullLevelCompact` 来自 `astersql-lightning-pkg-importer`。标准库依赖集中在 `HashMap`、`Arc/Mutex/OnceLock`、`AtomicBool`、scoped threads、文件读取和进程退出。

RustCodeGraph 文件查询显示该文件有 107 个符号，并被 `main.rs`、`parity_test.rs` 等索引文件使用。精确符号查询定位了 `LoadGlobalConfigWithCtl`（第 520 行）、`ForAllStores`（第 933 行）、`FetchModeFromMetrics`（第 998 行）和 `NewClient`（第 878 行）；直接引用搜索进一步确认主流程与两份同目录独立 Rust 测试的调用现场。

## 错误处理与边界

`FlagSet::Parse` 对 `-`、三个及以上连字符开头的参数、未知 flag、缺失参数、非法 bool/int 立即返回 `Error`；未注册的 `-h`/`--help` 会先调用 Usage，再返回固定错误 `flag: help requested`。bool 只接受 Go `strconv.ParseBool` 在这里列出的大小写拼写。解析遇到首个普通位置参数后不再处理后续 flag。

配置文件读取失败会携带文件路径；文件内容只支持逐行提取简单 `key = "value"` 的 `pd-addr` 与 `backend`，不是通用 TOML/YAML 解析器。`-config`/`-c` 二次扫描依赖前一次 `Parse` 已保证参数存在，不能脱离当前调用顺序单独复用。server mode 缺少 status address 会在网络边界之前失败。

`CheckpointNotFound::Equal` 只比较错误 class，因此相同文案的普通错误不会获得 checkpoint 恢复提示。`StackError` 不是完整 backtrace，只保证调用点可辨识。`Must` 在触发替换后的 exit hook 后仍返回默认配置；真实默认 hook 会直接退出，而测试 hook 下调用方必须认识到返回值只是占位。

PD client 关闭后 `GetStores` 返回 `pd client closed`。`ForAllStores` 返回 GetStores 错误或并发 action 的首个已记录错误；由于线程调度，若多个 action 同时失败，“首个”是竞争中先获得互斥锁者，不保证 store 顺序。取消信号不会强制中断已经运行的闭包，闭包需自行观察 context。

`Compact` 永远在记录成功后返回 `Ok`，不会模拟网络失败；`FetchMode` 未配置目标地址时返回 `fetch mode unavailable for ...`。因此这两个接口只能证明调度参数与错误分流，不能证明真实 PD/TiKV 连接、认证、超时、重试或协议兼容性。

## 并发与资源生命周期

`ForAllStores` 使用 `std::thread::scope`，借用 action 并为每个合格 store 启动线程；scope 退出前会 join 全部线程，所以函数返回时不存在遗留 worker。`first_err: Arc<Mutex<Option<Error>>>` 串行化首次错误写入；派生 context 的 cancel handle 被各线程共享，并在等待结束后再次调用，保证成功路径也处于已取消状态。这对应 Go `errgroup.WithContext(...).Wait()` 返回后 context 被取消的契约。

`PdClient` 具有显式和隐式两层释放：`main.rs::run_loaded` 在 dispatch 的所有正常 Result 返回路径之后调用 `Close`，`Drop` 又在对象销毁时调用 `Close`。关闭是幂等的原子写入。这里没有真实 socket 或 HTTP transport 可释放，生命周期断言针对的是 Go `defer cli.Close()` 的可观察时机。

全局退出 hook 和 mock 记录使用进程级同步原语。它们不会因测试结束自动复位；`main_test.rs` 通过子进程隔离进程全局 exit hook，`parity_test.rs` 则在契约场景前后显式 reset。新增并行测试必须遵循相同隔离方式，否则不同用例可能竞争最近客户端、compact 调用和 fetch-mode map。

## 与 Go 版本的对应关系

主要对照入口是 `lightning/cmd/tidb-lightning-ctl/main.go`。Go `config.LoadGlobalConfig(... extraFlags ...)` 对应 `LoadGlobalConfigWithCtl`；修改 `-d` 为 `noop://`、ctl flag 名称、动作分派优先级、PD 地址 `strings.Split`、checkpoint 控制器的延迟创建、无动作时 `fs.Usage()` 都在 Rust 主流程中保留。Go `defer cli.Close()` 对应 Rust 的显式 `Close` 加 `Drop` 兜底。

Go `common.ErrCheckpointTableNotFound.Equal` 对应 `CheckpointNotFound::Equal` 的 class 匹配；Go `errors.ErrorStack` 只被局部模拟为稳定调用点字符串。Go 的真实全局配置解析功能远多于本文件；Rust 仅覆盖 ctl 当前读取字段，且只解析两个简单配置键。

`pkg/lightning/tikv/tikv.go::ForAllStores` 使用 `errgroup.WithContext` 并并发处理 `State <= maxState` 的 store；本文件用 scoped threads、首错槽和派生 context 复刻相同控制语义。`tikv.go::Compact` 会建立真实 ImportSST gRPC 连接并发送 `CompactRequest`，本文件仅记录 `(address, level, resource_group)`。`tikv.go::FetchMode` 会连接 debug service 获取 metrics，本文件改为地址到结果的 mock map；两者共同依赖的纯算法 `FetchModeFromMetrics` 则按 Go 正则的边界与整行 capture 语义实现。

同目录 Go `main_test.go` 覆盖入口可运行、checkpoint 错误不打印栈、普通错误保留栈；Rust `main_test.rs` 进一步验证 flag/config 错误码 2、运行错误码 1 和 client 关闭时机。`pkg/lightning/tikv/tikv_test.go` 验证 Up/Offline 被遍历而 Tombstone 被过滤，以及 metrics 的 import/normal/缺失三类结果；同目录 Rust `parity_test.rs` 还覆盖 Go bool 拼写、并发性、派生 context 取消、配置别名顺序、端点空片段和 mock 生命周期。

## 扩展指南

新增 ctl flag 时，应在 `LoadGlobalConfigWithCtl` 注册并解析，按用途放入 `GlobalConfig` 或 `CtlActionFlags`，再在 `main.rs::dispatch` 的明确优先级位置接线。若 flag 属于运行配置，还需同步 `LoadFromGlobal`；同时在独立的 `parity_test.rs` 或 `main_test.rs` 增加正常、边界、错误和资源释放断言，不要把测试嵌入本文件。

扩大配置文件支持时，不应继续堆叠脆弱的逐行 `strip_prefix`；应优先复用 server/config 的真实解析能力，并用 Go `config.LoadGlobalConfig` 的覆盖顺序作为契约。特别要保持 `-config`/`-c` 最后出现者生效、命令行非空值覆盖文件值，以及 ctl 的 `noop://` 默认值。

若要将 PD/TiKV mock 替换为生产实现，应在独立上游 crate 中补齐并发布带 tag 的依赖，再统一更新 Cargo manifest；不要把外部依赖复制进 vendor/third_party，也不要用本地 `[patch]`。迁移时重点替换 `PdClient`/`NewClient`/`as_server_client`、`TlsExt`、`Compact`、`FetchMode`，同时保留 `ForAllStores` 的状态上界、并发、首错和取消契约。真实网络实现还需要新增独立测试覆盖连接失败、TLS、超时、重试与资源释放。

修改全局 mock 或退出钩子时，应优先封装场景级 guard 或保持子进程隔离，避免并行测试互相污染。任何对 `StoreState_*` 数值、metrics marker、错误 class 或用户可见文案的改变，都必须先核对 Go 文件与 parity 测试，因为这些值是兼容协议而非内部细节。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning-ctl` 定位到本 crate 的 Rust/Go 入口和测试。
- RustCodeGraph `node --file lightning/cmd/tidb-lightning-ctl/stubs.rs`：完整读取本文件 1–1042 行；精确 query 定位 `LoadGlobalConfigWithCtl`、`ForAllStores`、`FetchModeFromMetrics`、`NewClient`。
- RustCodeGraph `node`：读取 `lightning/cmd/tidb-lightning-ctl/main.rs`、`parity_test.rs`、`main_test.rs`，确认直接调用链、动作分派、并发/资源生命周期及独立测试覆盖。
- Cargo/入口：`lightning/cmd/tidb-lightning-ctl/Cargo.toml`、`lib.rs`、`bin_main.rs`，确认 crate 边界、仅有的两个直接 Rust 依赖和模块装配。
- Go 对照：`lightning/cmd/tidb-lightning-ctl/main.go`、`main_test.go`，确认 ctl 入口、flag、错误格式化、动作顺序与 PD client 关闭语义。
- TiKV 对照：`pkg/lightning/tikv/tikv.go`、`tikv_test.go`，确认 store 状态包含上界、并发遍历、真实网络职责和 metrics 解析规则。
- 直接引用搜索：`rg` 核对关键符号在目标源码、`main.rs`、`main_test.rs`、`parity_test.rs` 及 Go 对照中的引用位置；未发现同名 `stubs` 独立测试，相关 Rust 测试按仓库约定位于同目录独立测试文件。
- 本任务只新增说明文档，不修改运行时代码，按计划不运行 Cargo。结构验证以固定 11 个二级标题和目标文件存在性为准。
