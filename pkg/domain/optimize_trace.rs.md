# `pkg/domain/optimize_trace.rs`

## 文件定位

本文件属于 `astersql-domain` crate，由 [`pkg/domain/lib.rs`](lib.rs) 以 `pub mod optimize_trace` 公开装配。它负责生成 Optimizer Trace 在外部存储中的**相对目录**，结果形如 `optimizer_trace/<instance_id>`；文件本身不创建目录、不写 trace 文件，也不负责下载或垃圾回收。

RustCodeGraph 的当前索引只找到 [`pkg/domain/optimize_trace_test.rs`](optimize_trace_test.rs) 对本文件 API 的 Rust 调用，没有找到 Rust 生产调用者。因此这里是已经实现并公开的 Domain 边界能力，但尚不能据此声称 Rust 的 trace 生成、HTTP 下载或 GC 主链已经接入。Go 对照实现则已由 `pkg/executor/trace.go`、`pkg/server/handler/optimizor/optimize_trace.go` 和 `pkg/domain/domain.go` 使用。

## 核心职责

1. 用公开常量 `OPTIMIZER_TRACE_DIR` 统一根目录名 `optimizer_trace`。
2. `get_optimizer_trace_dir_name()` 从 `astersql-domain-infosync` 的全局 `InfoSyncer` 查询本节点 `ServerInfo`，优先采用非空 `ServerInfo.ID`。
3. 当全局 syncer 未初始化、查询失败或 ID 为空时，用当前进程 PID 作为实例标识，保证返回路径始终含有非空末级组件。
4. 将“选择实例标识”和“访问全局状态”分离：内部函数负责纯路径构造，公开函数负责运行时查询，测试辅助入口只暴露纯逻辑给同 crate 测试。

该设计的直接不变量是：返回值的首级目录由 `OPTIMIZER_TRACE_DIR` 决定，末级目录要么是非空 server ID，要么是十进制 PID。

## 主要符号

- `pub const OPTIMIZER_TRACE_DIR: &str = "optimizer_trace"`：外部存储中的固定根目录名。它是唯一的模块级常量。
- `fn optimizer_trace_dir_for_server_id(server_id: Option<&str>) -> PathBuf`：私有纯构造函数。它用 `Option::filter` 排除空字符串；非空值经 `str::to_owned` 成为自有字符串，否则惰性计算 `std::process::id().to_string()`，最后执行 `PathBuf::from(OPTIMIZER_TRACE_DIR).join(instance)`。
- `pub fn get_optimizer_trace_dir_name() -> PathBuf`：公开运行时入口。它调用 `infosync::GetServerInfo()`，用 `.ok()` 丢弃错误，再把可用的 `info.ID.as_str()` 交给私有构造函数。
- `#[cfg(test)] pub(crate) fn optimizer_trace_dir_for_server_id_for_test(...) -> PathBuf`：仅测试编译时存在的 crate 内入口，原样转发到私有构造函数，使测试不必操纵全局 `InfoSyncer`。

本文件没有类型、trait、`impl`、异步函数或额外条件编译分支；唯一的条件编译项是测试辅助函数。

## 执行流程

调用 `get_optimizer_trace_dir_name()` 时，流程如下：

1. 调用 `astersql_domain_infosync::GetServerInfo()`。
2. 将 `Result<ServerInfo>` 通过 `.ok()` 转成 `Option<ServerInfo>`：成功保留快照，失败变为 `None`。
3. 成功时临时借用 `ServerInfo.ID`；失败时传入 `None`。
4. `optimizer_trace_dir_for_server_id` 过滤空 ID。
5. 若仍有 ID，则复制该 ID；否则读取当前进程 PID 并转成十进制字符串。
6. 把实例标识拼到 `optimizer_trace` 后，返回相对 `PathBuf`。

测试路径 `optimizer_trace_dir_for_server_id_for_test()` 跳过步骤 1–3，直接把显式输入送入步骤 4–6。RustCodeGraph 验证到的文件内调用边为 `get_optimizer_trace_dir_name -> optimizer_trace_dir_for_server_id`，测试辅助入口也调用同一私有函数。

## 数据与状态

本模块自身不保存可变状态。`OPTIMIZER_TRACE_DIR` 是静态字符串；每次调用都会新建并返回一个自有 `PathBuf`。

运行时状态来自两个外部来源：

- `infosync::GetServerInfo()` 读取全局 `InfoSyncer`。其实现位于 `pkg/domain/infosync/info.rs`，先从全局 `RwLock<Option<Arc<InfoSyncer>>>` 克隆 syncer，再由 `ServerInfoSyncer()` 读取并克隆 `server_info` 快照。
- `std::process::id()` 提供当前进程 PID，只在 server info 不可用或 ID 为空时读取。

函数返回相对路径而非绝对路径；真正使用哪个外部存储、如何解释路径分隔符以及何时清理文件，都属于调用方职责。由于使用平台原生 `PathBuf::join`，路径表示遵循运行平台的路径规则。

## 依赖与调用关系

直接标准库依赖是 `std::path::PathBuf` 和 `std::process::id`。直接 workspace 依赖是 `astersql-domain-infosync`；[`pkg/domain/Cargo.toml`](Cargo.toml) 将其声明为路径依赖 `path = "infosync"`。本模块无 feature gate，且通过 [`pkg/domain/lib.rs`](lib.rs) 的公开模块声明进入 `astersql-domain`。

下游调用链为：

`get_optimizer_trace_dir_name` → `infosync::GetServerInfo` → `getGlobalInfoSyncer` / `InfoSyncer::ServerInfoSyncer`，随后回到 `optimizer_trace_dir_for_server_id` → `PathBuf::join`。

RustCodeGraph 和全仓库 Rust 文本检索均未发现测试之外的 `get_optimizer_trace_dir_name`、`OPTIMIZER_TRACE_DIR` 或测试辅助函数引用。与之相对，Go 的 `domain.GetOptimizerTraceDirName()` 当前参与三条生产路径：

- `pkg/executor/trace.go::generateOptimizerTraceFile`：生成随机 trace zip 文件名，并用返回目录作为外部存储创建路径的父目录。
- `pkg/server/handler/optimizor/optimize_trace.go::OptimizeTraceHandler.ServeHTTP`：把返回目录与路由文件名拼接，供下载处理器定位文件。
- `pkg/domain/domain.go::NewDomainWithEtcdClient`：把该目录加入 `dumpFileGcChecker.paths`，参与 dump 文件回收。

这三条边是理解该 API 在完整 Go 应用中的位置的直接证据，也是未来 Rust 接线时需要分别核对的消费者；它们不等同于当前 Rust 已接线。

## 错误处理与边界

本文件的公开函数不返回 `Result`。`infosync::GetServerInfo()` 的任意错误（包括全局 syncer 未初始化）都被 `.ok()` 转为缺失值，并统一回退到 PID；这是刻意对齐 Go “查询失败则使用 PID”的行为。空字符串 ID 也被视为缺失，`None`、`Some("")` 和查询失败最终走同一兜底路径。

非空 ID 不做清洗、规范化或合法性验证，而是直接作为一个 `PathBuf::join` 参数。因而调用方/注册信息必须保证 ID 适合作为单个路径组件；若未来允许包含分隔符、`.`、`..` 或平台前缀，需先明确兼容与安全契约并增加测试，不能在这里静默改变既有目录布局。

模块不验证外部存储是否存在、不可写或发生名称冲突，也不处理 I/O 错误，因为这里只构造路径。PID 回退在同一进程内稳定，但跨进程重启可能变化，且它不是集群范围内的持久身份。

## 并发与资源生命周期

本模块不创建线程、任务、通道、文件句柄或事务，也不持有锁。`get_optimizer_trace_dir_name()` 同步完成，返回的 `PathBuf` 与全局状态解耦。

并发同步由 infosync 层拥有：`getGlobalInfoSyncer()` 在全局槽位上取得读锁并克隆 `Arc`，`ServerInfoSyncer()` 再在 `server_info` 上取得读锁并克隆 `ServerInfo`。本模块只消费该快照，借用 `ID` 的生命周期不越过函数调用。若锁中毒，infosync 内的 `unwrap()` 可能 panic；本模块的 `.ok()` 只能处理 `Result::Err`，不能捕获 panic。

PID 路径不会预留或创建资源，所以并发调用只会各自构造等值路径，不存在本模块内部竞态；实际同名文件竞争由写入端解决。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/domain/optimize_trace.go`](optimize_trace.go) 的 `GetOptimizerTraceDirName() string`。两版共同语义为：尝试读取本节点 server info；只有非空 ID 才优先使用；否则采用当前进程 PID；最后返回 `optimizer_trace/<instance_id>` 相对路径。

主要表示差异如下：

- Go 返回 `string` 并使用 `filepath.Join`；Rust 返回类型更强的 `PathBuf` 并使用 `PathBuf::join`。
- Go 同时检查 `err == nil && info != nil`；Rust 的 `GetServerInfo()` 返回 `Result<ServerInfo>` 而非可空指针，因此 `.ok()` 已覆盖错误分支，不存在成功但 `ServerInfo` 为空的分支。
- Go 先把 `instanceID` 初始化为空字符串再覆写；Rust 用 `Option<&str>` 和 `filter` 显式表达“缺失或空”的同一语义。
- Rust 增加了 `#[cfg(test)]` 的纯逻辑转发入口，用于独立测试；Go 文件没有对应辅助函数。

Go 的生产消费者已接入 trace 创建、下载和 GC；当前 Rust 索引没有对应生产调用边，这是迁移状态差异，不应在文档中描述为功能已经端到端可用。

## 扩展指南

- 修改根目录名或目录层级时，应改 `OPTIMIZER_TRACE_DIR` / `optimizer_trace_dir_for_server_id`，并同步检查 Go 兼容、已有外部存储对象、下载路径和 GC 路径，避免旧文件不可发现或不可回收。
- 修改实例 ID 选择策略时，应把确定性规则保留在 `optimizer_trace_dir_for_server_id` 一类纯函数中，把全局查询留在 `get_optimizer_trace_dir_name`；同时扩展独立文件 [`pkg/domain/optimize_trace_test.rs`](optimize_trace_test.rs)，不要把 Rust 测试嵌回生产源文件。
- 接入 Rust 生产链时，应分别在 Rust 的 trace 文件生成端、HTTP 下载 runtime 的 `optimizer_trace_directory()` 实现和 Domain dump-file GC 初始化处建立调用，并为每条接线增加所在模块的独立测试。现有 `pkg/server/handler/optimizor/optimize_trace.rs` 通过 trait 向 runtime 索要目录，本文件当前没有自动连接该 trait。
- 若需要保留或暴露 infosync 错误，必须重新评估公开签名以及 Go 的静默 PID 兜底兼容性；直接把签名改为 `Result<PathBuf>` 会影响消费者契约。
- 若要接受非受信任的 server ID，必须增加路径组件验证和跨平台用例，并评估改变已有合法 ID 行为的兼容风险。
- 此路径函数位于可能频繁访问的生成/下载边界，扩展时应避免网络 I/O 或长时间持锁；当前只执行内存快照读取和少量字符串/路径分配。

## 验证依据

本说明基于以下直接证据完成，未运行 Cargo（任务明确为纯文档分析）：

- Rust 主文件：[`pkg/domain/optimize_trace.rs`](optimize_trace.rs)，核对全部 67 行及四个符号。
- crate 装配与依赖：[`pkg/domain/lib.rs`](lib.rs) 的 `pub mod optimize_trace`、测试模块声明，以及 [`pkg/domain/Cargo.toml`](Cargo.toml) 的 `astersql-domain` 包信息和 `astersql-domain-infosync` 路径依赖。目标包不存在 `doc.go`。
- RustCodeGraph：索引状态为 11,467 个文件；`query/node/explore/callers/callees` 确认文件内调用边、infosync 下游调用，以及 Rust 侧仅测试调用的当前事实。
- infosync 实现：`pkg/domain/infosync/info.rs` 的 `getGlobalInfoSyncer()`、`GetServerInfo()` 和 `InfoSyncer::ServerInfoSyncer()`，核对错误来源、读锁和快照克隆。
- 独立 Rust 测试：[`pkg/domain/optimize_trace_test.rs`](optimize_trace_test.rs)。`optimizer_trace_directory_owns_server_info_lookup_like_go` 检查根目录与非空末级组件；`optimizer_trace_directory_prefers_non_empty_server_id_and_falls_back_to_pid` 覆盖非空 ID、空 ID 和 `None`。
- Go 对照：[`pkg/domain/optimize_trace.go`](optimize_trace.go)；生产调用证据来自 `pkg/executor/trace.go::generateOptimizerTraceFile`、`pkg/server/handler/optimizor/optimize_trace.go::OptimizeTraceHandler.ServeHTTP` 和 `pkg/domain/domain.go::NewDomainWithEtcdClient`。全仓库未检索到直接覆盖 `GetOptimizerTraceDirName` 的 Go 测试。
- 相邻 Rust 下载边界：`pkg/server/handler/optimizor/optimize_trace.rs` 的 `OptimizeTraceRuntime::optimizer_trace_directory` 表明目录由 runtime 注入，未直接调用本文件；其测试仅验证下载路径拼接，不构成本文件的生产调用证据。

人工复核结论：本文区分了当前实现、Go 应用位置和尚未接线的 Rust 生产链；没有把注释中的预期设计当作已经发生的调用，也没有建议把测试写入生产源文件。
