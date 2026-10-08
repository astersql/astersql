# `pkg/ttl/ttlworker/job_version_checker.rs`

## 文件定位

本文件属于 `astersql-ttl-ttlworker` crate；模块由 [`pkg/ttl/ttlworker/lib.rs`](lib.rs) 的 `pub mod job_version_checker` 对外暴露。它位于 TTL 作业创建和扫描范围生成之前，负责判断当前集群是否已经具备安全创建“索引扫描 TTL 任务”的统一版本条件。

该门禁有两条直接使用路径：[`JobManager::submit_job`](job_manager.rs) 在选择并记录 TTL 扫描索引前调用它；[`ttl_index_scan_version_check`](../../session/runtime/ttl_metadata.rs) 在 `split_ttl_scan_ranges` 真正按 TTL 索引切分范围前调用它。因此，本文件不执行扫描、删除或服务发现，只把调用者提供的时间与服务版本快照归约为一个决策。

## 核心职责

1. 用 `VersionInfo { version, git_hash }` 表示完整构建身份。版本字符串相同但 Git hash 不同仍视为不同构建。
2. 在 `version_infos_consistent` 中比较本机身份与所有真实 TiDB 服务；跳过 `ServerInfo::assumed == true` 的合成条目，但拒绝空列表、空服务信息和“只有 assumed 条目”的不可判定状态。
3. 在 `JobVersionChecker::check` 中把比较结果区分为三种语义：一致时 `AllowIndexScan`，明确不一致时 `BlockJob`，无法可靠取得或比较信息时 `FallbackToPrimaryKey`。
4. 缓存最近一次决策：普通结果 10 秒，阻塞结果 60 秒。较长的阻塞缓存降低滚动升级期间反复查询和反复尝试提交的频率。

这一三态设计保留了可用性与兼容性的区别：未知状态不证明集群存在旧 worker，所以走旧的主键扫描格式；已知混合构建则阻止本次提交，避免新索引范围被旧 worker 误解，也避免静默退化为可能昂贵得多的主键扫描。

## 主要符号

- `VersionInfo`：公开值类型，字段 `version: String` 与 `git_hash: String` 共同构成精确构建身份；派生 `Clone/Debug/Eq/PartialEq`，比较不解析版本字符串。
- `ServerInfo`：公开包装类型，包含 `version: VersionInfo` 与 `assumed: bool`。`assumed` 标识不应参与真实节点计数和一致性判断的合成服务条目。
- `JobVersionCheckResult`：公开三态枚举。默认值是 `FallbackToPrimaryKey`，保证新建 checker 在没有成功检查前采用兼容路径。
- `version_infos_consistent(current, servers) -> Result<bool, String>`：公开纯函数。`Ok(true)` 表示至少有一台真实服务且全部完整身份相同；`Ok(false)` 只表示发现明确版本不一致；`Err` 表示输入不足或损坏，不能形成可靠结论。
- `JobVersionChecker`：公开、非线程安全的有状态检查器；私有字段 `last_check_seconds` 和 `last_result` 构成缓存。
- `JobVersionChecker::check(now_seconds, local, all) -> JobVersionCheckResult`：公开入口。调用者提前完成服务发现并把失败编码为 `Result`，本方法只负责缓存、比较和策略映射。

本文件没有 trait、模块级常量、异步函数或条件编译项。10 秒和 60 秒目前是 `check` 内部字面量，而不是公开配置。

## 执行流程

`JobVersionChecker::check` 的流程如下：

1. 根据上一结果选择缓存周期：`BlockJob` 为 60 秒，其他结果为 10 秒。
2. 若 `last_check_seconds` 存在，且 `now_seconds.saturating_sub(last) < interval`，直接返回 `last_result`。`saturating_sub` 使时钟倒退或调用者传入更小时间时不会发生无符号下溢；这种情况会得到差值 0，从而继续命中缓存。
3. 缓存失效后同时检查本机与全集群输入。只有 `(Ok(Some(local)), Ok(all))` 进入版本比较；本机查询错误、本机信息为空或全集群查询错误都回退到 `FallbackToPrimaryKey`。
4. `version_infos_consistent` 先拒绝空列表，然后逐项处理：`None` 立即返回带服务 ID 的错误；`assumed` 条目被跳过；真实条目计数并与 `current` 做完整结构相等比较。首次明确不一致即返回 `Ok(false)`；遍历后若真实条目数为零则报错，否则返回 `Ok(true)`。
5. 比较结果映射为 `AllowIndexScan`、`BlockJob` 或 `FallbackToPrimaryKey`，随后无条件更新缓存时间和结果。
6. 上游依据三态继续：`JobManager::submit_job` 在允许时选择 TTL 索引、回退时不选索引、阻塞时返回错误；`split_ttl_scan_ranges` 也分别生成索引范围、继续主键范围或返回相同的混合版本错误。

## 数据与状态

检查器只持有最近检查的秒级时间戳和决策，不保存服务列表、节点 ID 或版本副本。它因此不会让旧拓扑长期驻留内存；缓存到期后的下一次调用必须由上游再次提供快照。

`last_check_seconds: None` 表示从未检查。由于 `JobVersionCheckResult::default()` 是 `FallbackToPrimaryKey`，默认构造后的状态是“没有缓存时间 + 兼容决策”，但没有时间戳时仍会立即执行首次检查。每次非缓存检查无论成功、明确不一致还是查询失败，都会写入缓存；因此暂时性查询失败也会被缓存 10 秒。

输入中的 `Vec<(String, Option<ServerInfo>)>` 保留服务 ID，ID 仅用于 `None` 错误诊断，不参与相等比较。比较要求 `version` 和 `git_hash` 两字段同时相等；不做语义版本排序、规范化或“同一发布分支”推断。

## 依赖与调用关系

本文件自身只依赖 Rust 标准库类型，没有直接依赖 `Cargo.toml` 中的其他 crate。所属 [`Cargo.toml`](Cargo.toml) 将 crate 命名为 `astersql-ttl-ttlworker`，库入口为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/ttl/ttlworker"` 声明 Go 对照包。

直接上游关系：

- [`pkg/ttl/ttlworker/job_manager.rs`](job_manager.rs) 导入 `JobVersionChecker` 与结果枚举，将 checker 作为 `JobManager` 字段，并在 `submit_job` 选择 `ScanIndex` 前检查。
- [`pkg/session/runtime/ttl_metadata.rs`](../../session/runtime/ttl_metadata.rs) 从 `astersql_ttl_ttlworker` 导入四个公开类型，用 domain infosync 的 `GetServerInfo`/`GetAllServerInfo` 构造输入，并以 `OnceLock<Mutex<JobVersionChecker>>` 共享检查缓存。
- [`pkg/ttl/ttlworker/job_version_checker_test.rs`](job_version_checker_test.rs) 直接覆盖纯比较函数与缓存决策。

直接下游只有 `version_infos_consistent`：`check` 不自行访问网络、etcd、domain 或日志。服务发现、错误字符串化和模型转换均在调用者侧完成，这使本模块可独立测试，但也意味着调用者必须确保快照代表同一检查时点所需的集群视图。

## 错误处理与边界

`version_infos_consistent` 把三类不可判定输入作为 `Err(String)`：服务列表为空；任一服务 ID 对应 `None`；过滤 assumed 条目后没有真实服务。错误文本分别明确空列表、具体空服务 ID或没有真实服务。

明确的完整构建身份不一致不是函数错误，而是 `Ok(false)`；`check` 将其升级为 `BlockJob`。相反，服务发现错误、本机信息为空或一致性函数报错都被收敛为 `FallbackToPrimaryKey`，原始错误不会由本模块保留或返回。日志责任留在较外层；Rust 版本的 `check` 本身不记录日志。

边界不变量包括：至少一个真实节点才可能得到 `Ok(true)`；assumed 节点永远不能单独证明一致；首个真实不一致即可短路；时间间隔使用严格小于，恰好达到 10 或 60 秒时会重新检查；极端时钟倒退会因饱和减法继续使用缓存，直到输入时间重新追上缓存时间。

## 并发与资源生命周期

`JobVersionChecker::check` 接收 `&mut self`，类型内部没有锁、原子量、线程或异步任务。源码注释明确它本身非线程安全，所有权应由单一作业循环持有，或由上游提供互斥保护。

两条当前调用链采用不同生命周期：`JobManager` 直接拥有 checker，生命周期随 manager；`ttl_metadata.rs` 使用进程级 `OnceLock<Mutex<_>>`，在锁内调用 `check`，中毒锁则通过 `into_inner` 恢复。服务快照和字符串均按值传入本次检查，调用结束后释放；没有通道、文件句柄、事务、后台任务或显式清理步骤。

扩展时不应在持有外层 mutex 后加入阻塞 I/O。当前设计已把服务发现放在锁外，再以准备好的值进入 `check`；若把 I/O 移入本模块，会扩大临界区并改变错误与资源生命周期。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/ttl/ttlworker/job_version_checker.go`](job_version_checker.go)，相关测试是 [`job_version_checker_test.go`](job_version_checker_test.go)。Rust 保留了 Go 的核心语义：完整 `VersionInfo` 比较、忽略 assumed 条目、空/空指针输入报错、未知状态回退主键扫描、已知混合构建阻塞，以及普通 10 秒/阻塞 1 分钟缓存。

主要结构差异如下：

- Go 的 `ttlJobVersionChecker.check(ctx)` 自行调用 infosync、记录告警并读取 `time.Now()`；Rust 将当前秒数、本机查询结果和全集群查询结果作为参数注入，使策略层成为同步、确定性的纯计算加缓存。
- Go 将缓存拆为 `cachedResult` 与 `cacheResult`；Rust 内联于 `check`。
- Go 的 server map 在 Rust 中表示为带 ID 的向量，Go 的 nil 指针对应 Rust 的 `Option<ServerInfo>`。
- Go checker 的注释要求不可并发使用；Rust 同样不内置同步，但 `ttl_metadata.rs` 的全局复用点显式包裹 `Mutex`。
- Go 能从 server model 读取 `IsAssumed()`；当前 Rust infosync 转换路径说明其 API 只返回已注册真实节点，因此写入 `assumed: false`。类型仍保留该字段，以维持比较函数语义和测试覆盖。

[`job_version_checker_test.rs`](job_version_checker_test.rs) 验证了完整构建比较、忽略 assumed、空列表报错、已知不一致阻塞 60 秒、到期后未知状态回退，以及随后一致时允许索引扫描。Go 测试还覆盖版本字符串/哈希的更多组合、nil 远端、only-assumed、查询失败调用次数与缓存刷新；这些是 Rust 后续补充测试时应保持的对照基线。

## 扩展指南

- 若改变允许索引扫描的版本规则，应优先修改 `VersionInfo` 与 `version_infos_consistent`，并同步独立测试 `job_version_checker_test.rs`；同时核对 Go 的 `tiDBServerVersionInfosConsistent`，避免只比较版本字符串或错误接受 assumed-only 集群。
- 若调整缓存策略，应把 10/60 秒提取为具名常量或配置，并新增严格边界、时钟倒退、失败缓存及状态切换测试。阻塞缓存变短会增加滚动升级期查询负载，变长会推迟升级收敛后的 TTL 作业恢复。
- 若增加决策状态，必须同步两个穷尽匹配点：`JobManager::submit_job` 与 `split_ttl_scan_ranges`。遗漏任一路径会造成作业记录和实际范围生成策略不一致。
- 若让 checker 自行执行 infosync，应保留可注入测试边界，避免在共享 mutex 内做 I/O，并明确日志、超时与快照一致性责任；当前调用者注入模式更适合单元测试。
- 测试逻辑应继续放在独立的 `job_version_checker_test.rs`，不要内嵌到生产源文件。兼容风险集中在旧 worker 无法理解新索引范围；性能风险集中在缓存周期、服务列表遍历和错误状态下的查询频率。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 Rust/Go 文件；`node --file pkg/ttl/ttlworker/job_version_checker.rs` 读取了 101 行完整源文件，并报告使用者包括 `job_manager.rs`、`ttl_metadata.rs`、集成测试与单元测试。
- RustCodeGraph `node job_version_checker.rs::JobVersionChecker`：确认结构体位于第 66 行，导入关系来自 `job_manager.rs` 与 `job_version_checker_test.rs`。精确 `callers/callees` 查询未返回方法级边，故调用点再由下列直接源码交叉核验。
- 生产源码：[`job_version_checker.rs`](job_version_checker.rs)、[`job_manager.rs`](job_manager.rs)、[`lib.rs`](lib.rs)、[`ttl_metadata.rs`](../../session/runtime/ttl_metadata.rs)。
- crate 与移植边界：[`Cargo.toml`](Cargo.toml)。
- Go 对照与测试：[`job_version_checker.go`](job_version_checker.go)、[`job_version_checker_test.go`](job_version_checker_test.go)。
- Rust 独立测试：[`job_version_checker_test.rs`](job_version_checker_test.rs)；另检查了 [`job_manager_integration_test.rs`](job_manager_integration_test.rs) 的相关导入上下文。
- 本任务仅新增说明文档，不改变运行时代码；按计划不运行 Cargo。交付前使用任务指定命令检查目标文件存在且恰有 11 个固定二级章节，并人工复核调用边、三态语义、缓存边界和 Go 对照。
