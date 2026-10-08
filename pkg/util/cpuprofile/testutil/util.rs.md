# `pkg/util/cpuprofile/testutil/util.rs`

## 文件定位

本文件是 CPU 剖析测试负载生成器：它主动创建持续占用 CPU 的线程，并为每个线程保存一组预期的 pprof 标签，供剖析器、采集器和 TopSQL 相关测试构造稳定的采样输入。它不参与数据库线上请求处理，也不负责启动或解析 CPU profile；实际使用者先启动剖析器，再用这里的 worker 制造可采样负载。

同一实现有两条装配路径。`pkg/util/cpuprofile/testutil/Cargo.toml` 以 `lib.rs` 为入口建立独立 crate `astersql-util-cpuprofile-testutil`，`lib.rs` 公开重导出本文件的全部公共符号；`pkg/util/cpuprofile/lib.rs` 又以 `#[path = "testutil/util.rs"]` 将它编入 `astersql-util-cpuprofile` 主 crate 并重导出。因此扩展公共 API 时必须同时考虑独立 testutil crate 和主 crate 内嵌模块两种消费者。

## 核心职责

1. `mock_cpu_load` 按传入的标签名创建“每个标签一组”的 worker，并追加一个包含全部标签的合并 worker；标签值按 Go 约定取 `hex("{label} value")`。
2. `mock_cpu_load_v2` 固定使用键 `sql_global_uid`，按每个传入值创建单独 worker，再追加全值合并 worker。
3. `CancellationToken` 在调用者与全部 worker 之间共享单调取消状态；`CpuLoad::join_timeout` 给调用者一个有总截止时间的回收入口。
4. `spawn_workers`、`thread_name` 和 `mock_cpu_load_by_thread_with_labels` 分别负责线程创建、可识别命名和不可被优化消除的忙循环。

这里保存的 `LabelSet` 是测试可观察元数据。Rust 标准库没有对应 Go `runtime/pprof.SetGoroutineLabels` 的线程标签 API，所以当前实现并未把这些键值写入原生 pprof 运行时；`black_box(labels)` 只保证标签参数参与执行、不会被优化器直接删除。涉及“采样结果实际携带标签”的结论必须由剖析器测试中的注入 profile 另行证明，不能仅由本文件推断。

## 主要符号

- `pub type LabelSet = Vec<(String, String)>`：保持标签的输入顺序，允许同名键或重复值，不做去重与合法性校验。
- `pub struct CancellationToken(Arc<AtomicBool>)`：可克隆的同步取消句柄。`new`/`Default` 创建 `false` 状态；`cancel` 以 `Release` 写入 `true`；`is_cancelled` 以 `Acquire` 读取。状态只会由未取消变为取消。
- `pub struct CpuLoad`：拥有 `label_sets` 与每个 worker 的 `(JoinHandle<()>, Receiver<()>)`。字段私有，调用者通过 `label_sets()` 只读观察分组，并通过消费 `self` 的 `join_timeout` 回收线程。
- `CpuLoad::join_timeout(self, Duration) -> bool`：以一个共享 deadline 等待所有完成信号，随后逐个 `join`；超时、channel 异常或线程 panic 均返回 `false`。
- `mock_cpu_load<I, S>`：接受任意 `IntoIterator<Item = S>` 且 `S: AsRef<str>`，构造 Go `MockCPULoad` 对应分组。
- `mock_cpu_load_v2<I, S>`：构造 Go `MockCPULoadV2` 对应的 `sql_global_uid` 分组。
- `MockCPULoad`、`MockCPULoadV2`：接受 `Vec<String>` 的 Go 风格兼容别名，仅转发到对应 snake_case API。
- `spawn_workers`：内部统一线程工厂；每个标签组恰好创建一个命名 OS 线程和一个完成通知 channel。
- `mock_cpu_load_by_thread_with_labels`：内部忙循环；每轮对 `0..1_000_000` 做 wrapping 累加，直至取消。
- `encode_hex`：内部小写十六进制编码器，每个输入字节输出两个字符。
- `thread_name`：空标签组命名为 `cpuprofile-load`，非空组命名为 `cpuprofile-load:key=value,...`。

本文件没有 trait、条件编译项或模块级可变全局状态。

## 执行流程

以 `mock_cpu_load(cancel, ["sql", "plan_digest"])` 为例：

1. 函数依输入顺序构造 `("sql", hex("sql value"))` 与 `("plan_digest", hex("plan_digest value"))`。
2. 每个键值对各自形成一个单元素 `LabelSet`，同时被追加到 `all`；循环结束后 `all` 作为第三组加入。因此 worker 数始终是输入项数加一。
3. `spawn_workers` 克隆每组标签和共享取消令牌，为每组建立 channel，并通过 `thread::Builder` 以 `thread_name` 的结果启动线程。
4. worker 在 `mock_cpu_load_by_thread_with_labels` 中反复计算；每一百万次加法后重新读取取消状态。退出循环后发送 `()` 完成信号并结束线程。
5. 调用者先调用 `cancel.cancel()`，再调用 `load.join_timeout(timeout)`。该方法把 timeout 转成一次性的绝对 deadline；前面 worker 的等待会消耗后面 worker 可用的时间，而不是每个 worker 各获得完整 timeout。

`mock_cpu_load_v2` 的线程创建与回收流程相同，差异只在标签构造：键总是 `sql_global_uid`，值原样保留而不做十六进制编码。空输入仍把空的 `all` 加入分组，所以会启动一个无标签 worker；这是对 Go “始终启动 all-label goroutine”行为的保留。

## 数据与状态

`CancellationToken` 的唯一共享状态是 `Arc<AtomicBool>`。克隆 token 只增加同一原子值的所有者，不复制取消状态；任一克隆调用 `cancel` 后，所有 worker 最终都会观察到 `true`。`Acquire`/`Release` 足以建立取消发布与观察关系，但本实现不借取消信号发布其他数据。

`CpuLoad::label_sets` 与 worker 一一对应，次序等于输入的单项分组次序，最后是合并分组。`workers` 保存 `JoinHandle` 以拥有线程回收权，保存 receiver 以支持标准库 `JoinHandle` 本身不提供的超时等待。完成 channel 仅传递空值，不承载计算结果。

忙循环的 `sum` 是线程局部 `u64`，使用 `wrapping_add` 避免 debug/release 溢出行为差异；`black_box(sum)` 阻止整段计算被优化掉。标签与线程名在创建后不再修改。文件不访问数据库、网络、磁盘、锁、异步 runtime 或事务。

## 依赖与调用关系

下游全部来自 Rust 标准库：`Arc<AtomicBool>` 实现共享取消，`mpsc` 实现完成通知，`thread::Builder`/`JoinHandle` 管理 OS 线程，`Instant`/`Duration` 实现总超时，`black_box` 保留负载计算。`encode_hex` 自行实现编码，因此独立 testutil crate 的 `Cargo.toml` 不需要第三方依赖。

RustCodeGraph 将本文件识别为含 16 个符号、被 13 个文件使用。经直接引用核验，核心调用边包括：

- `mock_cpu_load` / `mock_cpu_load_v2` → `spawn_workers` → `thread_name` 与 `mock_cpu_load_by_thread_with_labels`。
- worker 闭包 → `mock_cpu_load_by_thread_with_labels` → `CancellationToken::is_cancelled`。
- `MockCPULoad` → `mock_cpu_load`；`MockCPULoadV2` → `mock_cpu_load_v2`。
- `pkg/util/cpuprofile/cpuprofile_test.rs::TestGetCPUProfile` 调用 `mock_cpu_load`，制造三类标签对应的 CPU 负载，并在测试结束时取消、限时 join。
- `pkg/util/cpuprofile/migration_aster_unit_test.rs::global_profiler_matches_go_start_stop_registration_and_delivery` 调用 `mock_cpu_load`，为全局剖析器真实采样与投递提供负载。
- `pkg/util/cpuprofile/testutil/migration_aster_unit_test.rs` 直接验证两种构造函数、`label_sets`、`cancel` 和 `join_timeout`。

仓库根 `Cargo.toml` 通过 `facade_util_cpuprofile_testutil` 指向独立 crate，`pkg/lib.rs` 再把它暴露为 util/cpuprofile/testutil 门面；主 cpuprofile crate 则直接按路径包含本文件。这些是装配关系，不改变本文件的运行逻辑。

## 错误处理与边界

公共构造函数不返回 `Result`。线程创建失败时，`spawn_workers` 的 `expect("failed to start CPU-load worker")` 会 panic；此处选择快速失败，因为没有 worker 就无法形成有效测试负载。

`join_timeout` 用 `bool` 合并三类失败：总 deadline 已经过期、等待完成通知失败、或 `JoinHandle::join` 发现 worker panic。它不暴露具体失败来源。该方法不会主动取消 worker；若调用者忘记先调用 `cancel`，通常会等待到超时并返回 `false`。更重要的是，一旦在等待阶段提前返回，`self` 被析构，尚未 join 的 `JoinHandle` 会被 detach，而 worker 仍可能继续运行，直到某个 token 克隆被取消。因此安全调用顺序是“取消，再 join”。

若所有完成信号已收到，随后某个线程 join 失败，函数立即返回 `false`；尚未 drain 的 handle 同样随 `self` 析构而 detach，不过这些线程按完成信号语义已经退出或即将完成。发送完成信号的错误被显式忽略，因为 receiver 被提前丢弃只表示调用者不再等待。

空输入不是错误：两种构造函数都创建一个空标签合并 worker。重复标签、空字符串、包含逗号或等号的字符串也不会被拒绝；它们可能使线程名产生歧义，但 `label_sets` 中的原始键值仍保持不变。极长标签还可能让 OS 拒绝线程名/线程创建，并触发上述 panic。

## 并发与资源生命周期

每个标签组对应一个独立 OS 线程，CPU 与线程成本为 O(n+1)，其中 n 是输入项数；每个 worker 在取消前持续忙等，不让出线程，也没有速率限制。该工具只适合短时、受控测试，不能作为生产后台任务使用。

取消检查位于每轮一百万次迭代之前，因此取消响应不是即时的；延迟取决于机器速度与调度。原子布尔值避免数据竞争，且重复 `cancel` 幂等。`CpuLoad` 没有实现 `Drop` 自动取消或 join，资源生命周期由调用者显式管理。仅丢弃 `CpuLoad` 会 detach 线程；只要 worker 持有 token 的 `Arc`，原子状态仍存活，若没有外部句柄再触发取消，线程可无限运行。

`join_timeout` 消费 `CpuLoad`，确保同一批 handle 不能被重复 join。单一 deadline 保证整批等待总时长大致受 timeout 限制。线程先发送完成通知、后从闭包返回，receiver 成功后紧接的 `join` 仍用于确认线程完全退出并捕获 panic。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/cpuprofile/testutil/util.go`：

- Go `MockCPULoad` 对每个标签启动一个 goroutine，并启动一个包含全部标签的 goroutine；Rust `mock_cpu_load` 保留相同分组数量和顺序。两者都把 `"{label} value"` 的字节编码成小写十六进制作为预期标签值。
- Go `MockCPULoadV2` 固定键 `sql_global_uid`，对每个值与全值组合启动 goroutine；Rust 版本保持相同标签结构。
- Go 通过 `context.Context` 取消，通过 `pprof.WithLabels` 与 `pprof.SetGoroutineLabels` 把标签实际绑定到 goroutine；Rust 以自定义 `CancellationToken` 取消，并只保留 `LabelSet`、线程名和 `black_box` 引用，没有等价的运行时 pprof 标签绑定。这是当前移植的重要能力差异。
- Go API 启动后不返回 worker 容器，也不提供 join；Rust 返回 `CpuLoad`，使测试能够检查标签分组并有界回收线程。
- 两端忙循环均按一百万项执行 `sum += i * 2`。Rust 用 `u64::wrapping_add` 明确溢出语义，并用 `black_box` 防止优化；Go 代码没有对应显式屏障。

Go 侧调用证据包括 `pkg/util/cpuprofile/cpuprofile_test.go::TestGetCPUProfile`、`pkg/util/topsql/collector/main_test.go::TestPProfCPUProfile` 和 `TestProcessProfCPUProfile`。Rust 目前在 cpuprofile 测试中直接消费本工具；TopSQL 的 Go 场景说明了 V2 标签的原始用途，但不能据此声称 Rust TopSQL 已经接入本工具。

## 扩展指南

新增标签构造策略时，优先增加新的公共构造函数并复用 `spawn_workers`，不要复制线程生命周期逻辑；若只是兼容 Go 命名，可像现有别名一样薄转发。任何改变分组顺序、是否保留合并组、十六进制规则或空输入行为的修改，都需要同步更新独立测试 `pkg/util/cpuprofile/testutil/migration_aster_unit_test.rs`，并核对 Go `util.go` 及其调用测试。

若要让 Rust 的真实 pprof 样本携带标签，修改点不能只限于 `LabelSet` 或线程名；必须先确认所用 `pprof` crate/平台是否支持线程标签，再在 `mock_cpu_load_by_thread_with_labels` 或剖析器采集边界接入，并在 `pkg/util/cpuprofile/cpuprofile_test.rs` 增加真实采样回归。要避免把“测试期 profile 注入”误当成运行时标签支持。

若改进回收语义，可考虑让超时错误可区分、为 `CpuLoad` 增加显式取消所有权，或设计安全的 `Drop` 策略；但自动 join 可能阻塞析构，必须先定义清晰契约。若改变取消检查粒度或忙循环规模，应评估测试稳定性、CPU 占用和采样命中率。公共 API 变更还要同时验证独立 crate、`pkg/util/cpuprofile/lib.rs` 的路径包含以及根 facade 的重导出。

Rust 测试逻辑应继续放在同目录独立的 `migration_aster_unit_test.rs`，不要内嵌进本源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件并已覆盖本路径；`files --filter pkg/util/cpuprofile/testutil` 列出 `lib.rs`、`migration_aster_unit_test.rs`、`util.go`、`util.rs`；`node --file pkg/util/cpuprofile/testutil/util.rs --offset 1 --limit 260` 返回完整 198 行、16 个符号及“used by 13 files”。精确 callers 查询因仓库大量同名符号未在合理时间内返回，直接调用关系随后用限定路径的 `rg` 补齐。
- 源码与装配：`pkg/util/cpuprofile/testutil/util.rs`、`pkg/util/cpuprofile/testutil/lib.rs`、`pkg/util/cpuprofile/testutil/Cargo.toml`、`pkg/util/cpuprofile/lib.rs`、根 `Cargo.toml` 与 `pkg/lib.rs`。
- Rust 独立测试：`pkg/util/cpuprofile/testutil/migration_aster_unit_test.rs` 覆盖单项/合并分组、十六进制值、V2 固定键、空输入、取消与成功 join；`pkg/util/cpuprofile/cpuprofile_test.rs::TestGetCPUProfile` 和 `pkg/util/cpuprofile/migration_aster_unit_test.rs::global_profiler_matches_go_start_stop_registration_and_delivery` 证明其被真实剖析流程测试消费。
- Go 对照：`pkg/util/cpuprofile/testutil/util.go` 定义原始 goroutine、context 和 pprof 标签行为；`pkg/util/cpuprofile/cpuprofile_test.go::TestGetCPUProfile`、`pkg/util/topsql/collector/main_test.go::TestPProfCPUProfile`、`TestProcessProfCPUProfile` 给出直接调用场景。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文没有把 Rust 当前未实现的运行时标签绑定描述为已支持。
