# `br/pkg/utils/misc.rs`

## 文件定位

`br/pkg/utils/misc.rs` 是 `astersql-br-pkg-utils` library crate 的公开杂项模块。`br/pkg/utils/Cargo.toml` 用 `[lib] path = "lib.rs"` 指定 crate 根，`br/pkg/utils/lib.rs` 再以 `#[path = "misc.rs"] pub mod misc;` 暴露本文件；crate 根没有 `pub use misc::*`，因此外部调用者需要经过 `astersql_br_pkg_utils::misc` 路径访问其中符号。文件对应 Go `br/pkg/utils/misc.go`，把不适合归入重试、schema、进度等专门模块的 BR 共用逻辑集中在一起。

当前 Rust 文件已经实现列类型兼容判定、Store 存活检查、限时清理、退出信号监听、Map 值收集、分区查找和备份文件汇总，并公开两个常量及 gRPC 抽象。不过迁移尚未完整接线：`GRPCConn` 固定使用 `NoopGrpcDialer` 返回错误；RustCodeGraph 对主要符号的调用边为空，全仓 Rust 精确搜索也只找到本目录独立测试对这里的直接调用。若干 Rust 生产模块仍使用各自 `stubs.rs` 中的同名实现。Go 侧则已有备份、恢复、流任务和命令入口等真实调用者，本文将其作为语义与迁移目标证据，而不当作 Rust 已接线事实。

## 核心职责

1. `IsTypeCompatible` 判定备份列类型能否安全装入目标列，并把 collation 是否一致作为独立结果返回。
2. `CheckStoreLiveness` 用 PD Store 状态和最近心跳时间过滤不可服务或长时间失联的 Store。
3. `GrpcDialer`/`GrpcConn` 定义连接边界，但默认 `GRPCConn` 是明确失败的 Noop 桩，防止误认为已有真实 gRPC 连接。
4. `WithCleanUp` 为必须执行但可能失败的清理操作提供超时取消上下文，并把清理错误排在既有业务错误之前合并。
5. `StartExitSingleListener` 监听进程退出信号：首次信号触发诊断输出与子上下文取消，第二次信号强制以状态码 1 退出。
6. `Values`、`FlattenValues` 和 `GetPartitionByName` 提供集合与元数据小工具；`SummaryFiles` 汇总备份文件校验和、KV 数、字节数并写 summary 统计。

## 主要符号

- `storeDisconnectionDuration: Duration = 100s`：Store 最长无心跳阈值；`last_heartbeat == 0` 不应用该阈值。
- `LabelRuleBatchSize: usize = 50`：与 Go 相同的 PD placement label rule 批量大小。当前 Rust 备份模块使用自己桩中的 `64`，不是本常量的调用证据，也显示主链尚未统一接入此模块。
- `IsTypeCompatible(src, target) -> (bool, bool)`：第一个布尔值表示类型兼容，第二个只表示 collation 相等。私有 `flen_and_decimal` 将 `UnspecifiedLength` 替换为 MySQL 类型默认长度与小数位。
- `GrpcDialer` 与 `GrpcConn`：均要求 `Send + Sync`。拨号器返回共享连接句柄；连接只约定 `target()` 与 `close()`。`NoopGrpcDialer` 是无状态默认实现。
- `GRPCConn(ctx, store_addr, tls_conf)`：保留 Go 风格入口签名，但忽略 context，TLS 只透传给 Noop 拨号器，当前必定返回带地址的 `ErrUnknown` 注解错误。
- `CheckStoreLiveness(&Store)`：只接受 `Up` 和 `Offline`；后者在 PD 语义中是“正在下线”，仍视为存活。
- `WithCleanUp(&mut Option<SharedError>, timeout, fn_)`：同步调用清理闭包，同时由后台线程在超时后取消传入闭包的本地 context。
- `AllStackInfo() -> Vec<u8>` 与 `DumpGoroutineWhenExit: AtomicBool`：前者捕获当前 Rust backtrace，后者控制收到退出信号时是否打印。它们近似而非完全复现 Go 的全 goroutine 栈。
- `StartExitSingleListener(Context) -> (Context, Context)`：返回 child context 和作为取消句柄使用的 clone；父 context 不因子取消而取消。
- `Values`、`FlattenValues`：克隆 `HashMap` 的值，输出顺序不稳定；后者预先计算总长度后展平各 `Vec`。
- `GetPartitionByName(&TableInfo, CIStr)`：用 `name.L` 做大小写归一化查找，只有大于零的分区 ID 算成功。
- `SummaryFiles(&[File]) -> (crc, kvs, bytes)`：CRC64 逐项 XOR，KV 和字节数逐项相加；每个文件还更新全局 summary，最后按 CF 名记录文件数。

## 执行流程

`IsTypeCompatible` 先计算 `collate_eq`，之后依次比较 NOT NULL 标志、UNSIGNED 标志和 `EvalType`；任一不一致都立即返回 `(false, collate_eq)`。随后源、目标的未指定 `flen`/`decimal` 被换成类型默认值，要求源值均不大于目标值。元素检查要求源元素数量不多于目标，并要求源的每个 enum/set 元素都存在于目标集合。最后 charset 相等才使第一个返回值为真；collation 不参与第一个布尔值，所以“类型兼容但 collation 不同”是合法结果。

`CheckStoreLiveness` 先验证状态。若既不是 `Up` 也不是 `Offline`，立即返回 `ErrKVStorage` 注解错误；心跳为零则跳过时间检查。非零心跳按 Unix epoch 纳秒构造 `SystemTime`，仅在当前时间确实晚于心跳且间隔超过 100 秒时失败。如果心跳位于未来，`duration_since` 返回错误而本函数忽略它，Store 仍被视为存活，这与 Go `time.Since` 不完全等价，属于应谨慎处理的边界。

`WithCleanUp` 创建独立 context 和 `AtomicBool stop`，后台线程每 5ms 检查一次：清理尚未结束且到达 timeout 时调用 `cancel()`。主线程同步执行闭包，取得错误后设置 stop，然后将 `cleanup_err` 与既有错误按该顺序交给 `Join`。函数不会等待计时线程退出；计时线程在下次轮询看到 stop 后自行结束。

`StartExitSingleListener` 从传入 context 创建 child token，注册 `SIGHUP`、`SIGINT`、`SIGTERM`、`SIGQUIT`。注册成功后线程阻塞等待首个信号，打印分隔信息，按原子开关选择性输出 `AllStackInfo`，记录 Warn，取消 child 并提示可再次强退；第二个信号到来后调用 `process::exit(1)`。信号注册失败只记录 Warn，函数仍返回未自动取消的 child 与取消句柄。

`SummaryFiles` 单次遍历文件：按 CF 累加数量、分别调用 `CollectSuccessUnit` 记录每个文件的 KV/字节、XOR CRC 并累加总量；遍历结束再为每个 CF 调用 `CollectInt`。空切片返回三个零，但仍不产生 CF 统计。

## 数据与状态

本文件唯一自身拥有的可变全局状态是 `DumpGoroutineWhenExit: AtomicBool`。监听线程以 `Ordering::Relaxed` 读取它；该标志只控制诊断输出，不承担其他内存状态的发布/同步。`SummaryFiles` 会通过 `astersql-br-pkg-summary` 修改 crate 外部的全局 summary，因此它并非纯聚合函数：重复调用会重复累计观测数据，即使返回三元组相同。

`IsTypeCompatible` 按值接收两个 `FieldType`，不会改写调用方对象。元素集合在调用内构造成 `HashSet<&str>`；时间、计数与 CF Map 都是单次调用的局部状态。`Values` 和 `FlattenValues` 要求 `V: Clone`，输出拥有独立值，但 HashMap 迭代顺序没有保证。

`WithCleanUp` 的 context、stop 位和线程跨越闭包执行期。闭包返回后 stop 被设置，后台线程最终释放所持 `Arc` 与 context clone。`StartExitSingleListener` 的线程持有 signal iterator 和取消句柄，可持续存活到两次信号完成；调用方主动调用返回的 cancel 只取消 child，不会停止监听线程或注销信号。

## 依赖与调用关系

模块装配与 crate 边界由 `br/pkg/utils/Cargo.toml`、`br/pkg/utils/lib.rs` 确认。直接依赖包括：parser 的 `FieldType`/MySQL 标志与默认长度工具、meta model 的 `TableInfo`、本 crate `kvproto` 与 context 桩、BR errors/logutil/summary、通用 `astersql-errors`、TLS 类型以及 `signal-hook`。Cargo 注释明确该 crate 为 darwin arm64 精简掉 kv/domain/kvproto/grpcio/tablecodec/sqlexec 的版本，本地 stub 与 Noop gRPC 边界正是这一限制的体现。

RustCodeGraph 文件节点报告目标文件被 21 个文件关联，但对精确 Rust 符号 ID 执行 `callers`/`callees` 没有返回静态调用边。全仓精确引用补查得到：`br/pkg/utils/misc_test.rs` 直接覆盖 `IsTypeCompatible`、`WithCleanUp`、`StartExitSingleListener`、`SummaryFiles`；未找到本模块这些符号的 Rust 生产调用。`br/pkg/backup/client.rs`、`br/cmd/br/main.rs`、`br/pkg/checksum/executor.rs` 等出现的同名调用分别解析到各自 `stubs.rs` 导入或局部 utils 边界，不能算作这里的上游调用者。

Go 主链提供了真实语义背景：`br/pkg/backup/client.go` 使用 `CheckStoreLiveness` 与 `SummaryFiles`；`br/pkg/restore/snap_client/systable_restore.go` 使用 `IsTypeCompatible`；`br/pkg/restore/data/data.go`、`br/pkg/task/backup_ebs.go` 使用 `GRPCConn`；`br/pkg/task/backup_ebs.go`、`stream.go` 使用 `WithCleanUp`；`br/cmd/br/main.go` 使用 `StartExitSingleListener`；`br/pkg/task/restore.go` 使用 `Values`；checksum 与 snap-client 通过 `GetPartitionByName` 展开分区。它们说明迁移目标，但不证明 Rust 已完成这些接线。

## 错误处理与边界

`IsTypeCompatible` 不返回错误；任何不满足条件都编码为 `false`，而 collation 结果即使在早期类型失败时仍独立有效。它把 enum/set 元素当集合处理，不比较顺序或重复次数。新增校验不能随意把第二个布尔值并入第一个，否则会破坏 Go 调用方分别处理类型与 collation 的契约。

`GRPCConn` 当前不是可用拨号实现。它不遵循 Go 的 5 秒阻塞拨号、TLS/insecure credentials、额外 dial options、context 取消或连接关闭生命周期；任何生产代码接入前必须替换/注入真实 `GrpcDialer`，不能把现有 `ErrUnknown` 当作网络连接失败重试证据。

`CheckStoreLiveness` 的状态和心跳错误均以 `ErrKVStorage` 为根错误并添加上下文。零心跳明确跳过；未来时间也会因 `duration_since` 错误被放行。将 `i64` 心跳强转 `u64` 意味着负值会变成极大纳秒数并落入未来时间路径，当前没有显式拒绝。

`WithCleanUp` 的 `err_out` 在 Rust 中不能为 nil，因此没有 Go `errOut == nil` 时“记录并忽略 cleanup 错误”的分支。`Join` 保持 cleanup 错在前；清理闭包必须自己观察 context 取消，超时不会抢占或强制终止它。闭包若永久阻塞，本函数也永久阻塞。timeout 线程采用 5ms 轮询，取消时间存在粒度误差。

`StartExitSingleListener` 第二个信号直接结束进程，析构与常规清理不保证执行。`writeln!`/stdout 写入错误被忽略；信号注册错误只记日志。`AllStackInfo` 仅捕获调用线程的 Rust backtrace，不等价于 Go `runtime.Stack(..., true)` 的全部 goroutine dump。

`GetPartitionByName` 区分无分区信息的 `InvalidInput` 与找不到名称的 `NotFound`，并刻意保留 Go 错误文本中的 `parition` 拼写。`SummaryFiles` 的整数运算在 debug 构建可能溢出 panic、release 构建可能回绕，函数未做 checked arithmetic；它也不校验 CF 或文件元数据。

## 并发与资源生命周期

`IsTypeCompatible`、集合函数、分区查找和本地汇总计算本身没有锁。`SummaryFiles` 的外部 summary 收集器并发语义由 `astersql-br-pkg-summary` 负责；本文件没有为多次调用建立事务性边界。

`WithCleanUp` 每次调用创建一个 OS 线程，线程用共享原子 stop 与 context clone 协作。使用 `SeqCst` 简化 stop 可见性，但没有保存或 join 线程句柄。调用者不应把它用于大量短清理而忽略线程创建成本，也不应假设函数返回时计时线程已经完全退出。

`StartExitSingleListener` 同样创建常驻 OS 线程。`signal_hook::iterator::Signals` 在该线程内持续等待；首个信号取消 child，第二个信号结束整个进程。返回的两个 `Context` 实际是同一 child token 的 clone，而不是 Go 的“context + 函数”不同类型。主动 cancel 不负责关闭 Signals，若进程长期不收到信号，监听线程将持续存在。

`GrpcConn` 用 `Arc<dyn GrpcConn>` 表示共享所有权，真实实现未来必须定义最后一个句柄释放与显式 `close()` 的关系，并保证 `close()` 在并发调用下安全。当前 Noop 路径不创建资源。

## 与 Go 版本的对应关系

`IsTypeCompatible`、`CheckStoreLiveness`、集合函数、分区查找和 `SummaryFiles` 的核心分支及常量基本逐项对应 `br/pkg/utils/misc.go`。独立 Rust 测试 `misc_test.rs` 复刻 Go 的类型矩阵，包括 flag、EvalType、默认 flen/decimal、enum 超集、charset/collation、Blob 扩容和 Timestamp 默认精度；它还保留 Go 测试中 `SetFlag(99)` 的笔误场景。`WithCleanUp` 测试覆盖只有 cleanup 错、cleanup 与既有错合并、完全成功；`SummaryFiles` 固定验证 `0xF ^ 0xF0 ^ 0xF00 == 0xFFF` 及 KV/字节算术和。

关键迁移差异如下：Go `GRPCConn` 是带 5 秒超时和 TLS 选择的真实阻塞拨号，Rust 是固定失败桩；Go `WithCleanUp` 支持 nil `errOut` 并写 Warn，Rust 参数类型排除了该分支；Go 用 timeout context 的 runtime timer，Rust 每次创建轮询线程；Go `AllStackInfo` 收集所有 goroutine 栈，Rust 只捕获当前 backtrace；Go 返回 `context.CancelFunc`，Rust 返回 child context clone；Go `time.Since` 对未来心跳产生负 duration 而不超过阈值，Rust通过忽略 `duration_since` 错误达到相似“放行”结果但机制不同。

Rust 测试比 Go 同目录测试多出退出监听用例：一个验证手动 cancel 只影响 child，Unix 用例实际 raise `SIGTERM` 验证首个信号取消 child。当前没有独立 Rust 测试覆盖 `GRPCConn`、`CheckStoreLiveness`、`Values`、`FlattenValues`、`GetPartitionByName`、`AllStackInfo` 或常量，也没有证明 Go 的生产调用链已迁移。

## 扩展指南

- 接入真实 gRPC 时，优先实现可注入的 `GrpcDialer` 并在实际调用者持有/关闭 `Arc<dyn GrpcConn>`，同步覆盖 TLS 与明文、5 秒/调用方取消、临时/永久错误和 close 行为；不要在调用点绕开该抽象复制连接代码。
- 修改类型兼容规则时同时更新 `IsTypeCompatible`、私有 `flen_and_decimal`、`br/pkg/utils/misc_test.rs` 与 Go 对照测试。尤其保持 `(type_eq, collate_eq)` 两项独立，并评估 schema restore 对 charset/collation 的兼容风险。
- 修改 Store 存活判定时应为状态、零/过期/未来/负心跳分别增加独立 Rust 测试，再接入备份客户端；100 秒阈值关系到错误剔除可用 Store与误向失联节点发请求的权衡。
- 扩展清理工具时不能依赖强制抢占。若要消除每次调用创建线程或确保线程回收，应在不改变错误顺序、同步返回与 context 观察契约的前提下设计 timer/guard，并为超时及快速完成竞态增加测试。
- 扩展退出处理时要保留“一次优雅取消、二次强退”的进程契约，同时明确 signal 注册注销、主动 cancel 后线程退出以及测试之间的全局信号干扰；测试继续放在独立 `misc_test.rs`，不要嵌入生产文件。
- `Values`/`FlattenValues` 的调用方不得依赖顺序；需要确定性输出时应在调用侧排序或新增语义明确的 API。`SummaryFiles` 若用于并行流水线，应先核对全局 summary 是否允许并发/重复累计，不能只并行化局部循环。
- 若统一目前散落在 `backup/checksum/snap_client` 桩中的同名实现，应逐个核对类型与错误契约后改用 canonical crate，避免仅凭名称替换。兼容风险集中在 label batch size 的 50/64 差异、错误类型和 stub 数据模型。

## 验证依据

- RustCodeGraph `status`：索引包含 11467 个文件、7032 个 Rust 文件；`node --file br/pkg/utils/misc.rs --offset 1 --limit 500` 完整读取目标 329 行，并报告该文件被 21 个文件关联。
- RustCodeGraph `query --json`：确认 `IsTypeCompatible`、`GRPCConn`、`SummaryFiles` 等 Rust 精确符号 ID；对主要函数执行 `callers`/`callees` 未返回静态边，因此再用全仓精确 `rg` 补查，没有把空图结果直接解释为不存在迁移背景。
- 目标源码与模块边界：`br/pkg/utils/misc.rs`、`br/pkg/utils/lib.rs`、`br/pkg/utils/Cargo.toml`。
- Go 语义与测试：`br/pkg/utils/misc.go`、`br/pkg/utils/misc_test.go`；Go 生产调用补查覆盖 `br/pkg/backup/client.go`、`br/pkg/restore/data/data.go`、`br/pkg/restore/snap_client/systable_restore.go`、`br/pkg/task/{backup_ebs,restore,stream}.go`、`br/pkg/checksum/executor.go`、`br/cmd/br/main.go`。
- Rust 独立测试：`br/pkg/utils/misc_test.rs`，由 `br/pkg/utils/lib.rs` 的 `#[cfg(test)] #[path = "misc_test.rs"] mod misc_test;` 挂载。另核对了 `br/pkg/backup/client.rs`、`br/cmd/br/main.rs`、`br/pkg/checksum/executor.rs` 等同名 Rust 引用，确认它们当前走局部 stubs 而非本模块。
- 人工复核：本文区分公开实现、Noop/桩边界、当前 Rust 接线状态与 Go 迁移目标，说明了正常流程、错误与并发生命周期以及安全扩展所需的独立测试；按总计划未运行 Cargo。
