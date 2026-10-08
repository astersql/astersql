# [`pkg/util/profile/profile.rs`](profile.rs)

## 文件定位

本文件是 `astersql-util-profile` crate 的剖析数据入口，负责把 CPU/runtime profile 转成上层可消费的 `DatumRows`。crate 根 `pkg/util/profile/lib.rs` 通过 `pub use profile::*` 公开本文件 API，而树构建和六列火焰图行的具体展开委托给同 crate 的 `flamegraph.rs`。`pkg/util/profile/Cargo.toml` 将 Go 对照包标为 `pkg/util/profile`，并声明 `cpuprofile`、`pprof`、`flate2`、`texttree`、`types` 与 `thiserror` 依赖。

在完整系统的设计位置上，它对应 performance schema 本地 TiDB profile 表的数据采集器：`pkg/infoschema/perfschema/tables.rs` 会把六张 `tidb_profile_*` 表分别路由为 `cpu`、`heap`、`mutex`、`allocs`、`block`、`goroutine`，但路由停在 `RowSource::local_profile` 抽象。当前 Rust 生产代码中未检索到该抽象直接调用 `Collector::ProfileGraph` 的接线；`pkg/util/profile/profile_test.rs::test_profiles` 直接模拟这层分发。因此，本文件的采集能力已经实现并由测试覆盖，但不能仅凭现有证据宣称 Rust SQL 主链已经把它投入生产使用。

## 核心职责

- `Collector::ProfileReaderToDatums` 接收任意 `Read`，识别 gzip 或原始 protobuf，解码并验证 `pprof::protos::Profile`，然后生成火焰图行。
- `Collector::ProfileGraph` 统一按名字分发采集：规范化后的 `cpu` 走 `cpuprofile` 采样；其他名称交给进程级 `RuntimeProfileProvider`；`goroutine` 使用文本解析，其余名称使用 protobuf 解析。
- `Collector::ParseGoroutines` 解析 Go `runtime/pprof` 的 goroutine 文本格式，输出函数、goroutine ID、状态和源码位置四列。
- `validate_profile` 在火焰图代码按下标和 ID 访问数据前完成结构一致性检查，复刻 Go `profile.CheckValid` 在本模块所依赖的关键约束。
- `CPUProfileInterval` 以及读写函数提供进程级 CPU 采样窗口；runtime provider 槽位与 guard 提供可替换、可恢复的运行时 profile 边界。

本文件不负责火焰图排序、百分比格式化和文本树缩进；这些行为位于 `pkg/util/profile/flamegraph.rs`。它也不负责 performance schema 表注册或 SQL 行迭代；那部分位于 `pkg/infoschema/perfschema/tables.rs`。

## 主要符号

- `CPUProfileInterval: AtomicU64`：以毫秒保存的全局采样窗口，初值为 30,000；`set_cpu_profile_interval(Duration)` 使用饱和到 `u64::MAX` 的毫秒值写入，`cpu_profile_interval()` 读取并还原为 `Duration`。两者均使用 `Ordering::SeqCst`。
- `ProfileError { message: String }`：本模块统一错误类型；`thiserror` 的显示实现直接返回消息，便于保持 Go 侧可观察错误文本。
- `RuntimeProfileProvider`：`Send + Sync` trait，`write_profile(name, debug, writer)` 同时表达 Go `pprof.Lookup(name)` 是否命中以及 `WriteTo` 是否成功。返回 `Ok(false)` 表示名称不存在，`Err` 表示写出失败。
- `runtime_profile_provider()`：惰性初始化的 `OnceLock<RwLock<Option<Arc<dyn RuntimeProfileProvider>>>>`，保存进程级 provider。
- `RuntimeProfileProviderGuard` / `install_runtime_profile_provider`：安装新 provider 时保存旧值，guard `Drop` 时恢复；这是测试和宿主运行时接入的生命周期边界。
- `Collector`：无字段的公开收集器。公开方法是 `ProfileReaderToDatums`、`ProfileGraph`、`ParseGoroutines`；`profile_to_flamegraph_node`、`profile_to_datums`、`cpu_profile_graph` 为内部阶段。
- `parse_profile_data`：私有解码入口；处理空输入、gzip 解压、protobuf decode 和完整校验。
- `validate_string_index` / `validate_profile`：私有结构校验器，保护字符串表、mapping/function/location ID 和 sample 引用。

## 执行流程

`ProfileReaderToDatums` 的流程如下：

1. 将 reader 全量读入 `Vec<u8>`；读取错误转换为 `ProfileError`。
2. `parse_profile_data` 拒绝空输入；若前两字节是 gzip 魔数 `1f 8b`，先用 `GzDecoder` 解压。
3. 用 `Profile::decode` 解码 protobuf，并立即调用 `validate_profile`。
4. `profile_to_flamegraph_node` 再次验证传入 profile，构造 `ProfileIndex` 与空根节点，对每个 sample 调用 `FlamegraphNode::add` 累加。
5. `new_flamegraph_collector(profile).collect(&root)` 按累计值排序并展开为六列 `DatumRows`。重复校验使内部直接调用 `profile_to_datums` 时也不会绕过安全前置条件。

`ProfileGraph` 先对名字执行 `trim().to_lowercase()`，仅用结果判断 CPU，所以例如 `" CPU "` 会进入 CPU 路径。CPU 路径创建共享输出缓冲，调用 `cpuprofile::NewCollector().StartCPUProfile`，等待当前原子间隔，再停止采集，复制缓冲内容并交给 `ProfileReaderToDatums`。`cfg(test)` 分支会在等待前注入真实 pprof fixture，以隔离大型测试二进制中原生采样和符号化的时序不确定性；这不是生产分支行为。

非 CPU 路径不会规范化传给 provider 的原始名称。它从全局槽位克隆 provider；无 provider 时返回 `cannot retrieve {name} profile`。名称严格等于 `goroutine` 时使用 `debug = 2`，其他名称使用 0。provider 未命中也返回相同错误；命中后，goroutine 输出交给 `ParseGoroutines`，其他输出交给 protobuf 流程。

`ParseGoroutines` 以双换行分块。每块必须含冒号；冒号前内容按“ID + 状态”切成两段，ID 解析为 `i64`，状态去掉方括号。冒号后的栈按两行一帧解释为函数名和文件位置，奇数个尾行会因 `stack.len() / 2` 被忽略。第一帧不加树前缀，中间帧和末帧分别使用 `texttree` 的中间/末节点标记；每帧输出四个 datum。

## 数据与状态

`Collector` 本身无状态，单次解析的 profile、索引、火焰图树和结果行均为调用栈内所有。profile 输入会先被完整缓冲，因此内存峰值包含输入、可能的 gzip 解压结果、protobuf 对象、ID 索引、树和输出行；本文件没有流式解码或输入大小上限。

进程级可变状态有两处。`CPUProfileInterval` 是原子毫秒数，修改会影响后续 CPU 采样调用；已经进入 `sleep` 的调用不会因后续修改而缩短。runtime provider 是单一全局槽位，值为共享 trait object；安装是替换语义而非栈容器语义，guard 保存安装瞬间的前值。

合法 profile 的关键不变量由 `validate_profile` 保证：`string_table[0]` 必须为空字符串；所有使用到的字符串下标必须非负且在范围内；mapping、function、location ID 均非零且各自唯一；location 引用的 mapping/function 必须存在；有 sample 时必须有 sample type；每条 sample 的值数量必须等于 sample type 数量；每个 sample location 必须存在。通过这些检查后，`flamegraph.rs` 才能安全使用索引访问和 sample 最后一个 value。

## 依赖与调用关系

下游直接调用边如下：

- `ProfileReaderToDatums -> parse_profile_data -> Profile::decode / GzDecoder / validate_profile`。
- `ProfileReaderToDatums -> profile_to_datums -> profile_to_flamegraph_node -> ProfileIndex::new / FlamegraphNode::add`。
- `profile_to_datums -> new_flamegraph_collector -> FlamegraphCollector::collect`。
- `cpu_profile_graph -> cpuprofile::{NewCollector, shared_buffer_writer, StartCPUProfile, StopCPUProfile} -> ProfileReaderToDatums`。
- `ProfileGraph -> runtime_profile_provider -> RuntimeProfileProvider::write_profile -> ParseGoroutines | ProfileReaderToDatums`。
- `ParseGoroutines -> types::datum::{NewStringDatum, NewIntDatum}` 和 `texttree` 树标记。

RustCodeGraph 将 `profile.rs` 标为被 `pkg/util/profile/profile_test.rs`、`migration_aster_unit_test.rs` 及 `pkg/util/memory/heap_profile.rs` 使用；逐项源码核验表明实际可观察的本 API 调用集中在两个独立测试文件和 `flamegraph_test.rs`，而 `heap_profile.rs` 自己使用 `rpprof` 写堆 profile，并未调用本文件公开符号。Cargo 层面，工作区 facade 在根 `Cargo.toml` 将本 crate 命名为 `facade_util_profile`，`pkg/lib.rs::util::profile` 再导出它；`pkg/infoschema/perfschema/Cargo.toml` 也声明依赖，但当前 `tables.rs` 通过 `RowSource` 抽象获取本地 profile，没有直接引用 `Collector`。因此图的“used by”文件关系不能等同于已确认的静态调用边。

## 错误处理与边界

所有可恢复错误都汇总为 `ProfileError`，并尽量保留底层错误文本。输入读取、gzip 解压、protobuf 解码、CPU profiler 启停、provider 写出以及输出 mutex 中毒都会返回错误，不会静默降级。profile 结构错误统一增加 `malformed profile:` 前缀；空输入和 gzip 解压后的空输入返回 `parsing profile: empty input file`。

锁中毒策略有意不同：provider 的 `RwLock` 读写使用 `PoisonError::into_inner` 继续恢复/读取槽位；CPU 输出 `Mutex` 中毒则返回 `profile output mutex poisoned`。未知 runtime profile、未安装 provider、或 provider 返回 `false` 都折叠为 `cannot retrieve {name} profile`。

已验证的解析边界包括空输入、坏 gzip、sample 值数不匹配、location 引用不存在的 function、goroutine 缺冒号和非法 ID。还需注意三个当前语义：CPU 名称比较不区分大小写且忽略两端空白，而 `goroutine` 比较严格；goroutine 奇数尾行被忽略而非报错；函数只检查被引用字符串和 ID，不试图实现 Go pprof 库所有可能的语义验证。扩展校验时应先对照 Go `profile.CheckValid`，避免无意接受或拒绝不同输入。

## 并发与资源生命周期

原子采样间隔可被并发读写，`SeqCst` 提供全序；它不串行化多次 CPU profile 请求。`cpu_profile_graph` 依赖 `cpuprofile::Collector` 自己处理全局 CPU profiler 的并发限制，本文件只保证输出缓冲由 `Arc<Mutex<Vec<u8>>>` 共享。采样线程会同步 `sleep` 整个窗口，调用期间占用当前执行线程。

provider 读取时只在 `RwLock` 下克隆 `Arc`，随后释放锁再执行外部 `write_profile`，避免在潜在慢 I/O 期间占用全局锁。安装和 guard 恢复持有写锁。guard 适合严格嵌套、逆序销毁；若多个线程交错安装并非逆序释放，较早 guard 的 `Drop` 可能覆盖较新的 provider，当前 API 没有所有权令牌校验。生产接入或并行测试必须把 guard 生命周期限制在明确作用域，并避免交错替换。

reader 数据、provider 输出和 CPU 输出全部驻留内存；`write_profile` 使用借用的 `&mut dyn Write`，provider 不应保留 writer。`RuntimeProfileProvider: Send + Sync` 和 `Arc` 允许多线程共享 provider，但 provider 自身必须保证内部并发安全。

## 与 Go 版本的对应关系

Go 权威对照是 `pkg/util/profile/profile.go`。主要一一对应关系为：Go `CPUProfileInterval` ↔ Rust 原子毫秒值及 getter/setter；Go `Collector` 三个公开方法 ↔ Rust 同名方法；Go `cpuProfileGraph` ↔ Rust `cpu_profile_graph`；Go `pprof.Lookup + WriteTo` ↔ Rust `RuntimeProfileProvider::write_profile`；Go `profile.Parse` / `CheckValid` ↔ Rust gzip/protobuf 解码与本地 `validate_profile`；Go `profileToFlamegraphNode/profileToDatums` ↔ Rust 私有转换阶段和 `flamegraph.rs`。

已保持的行为包括：CPU 名规范化、默认 30 秒采样、goroutine 使用 debug 2、未知名称错误、火焰图 sample 累加、goroutine 四列输出和树标记。Rust 额外引入 provider 抽象，因为 Rust 运行时没有 Go `runtime/pprof.Lookup` 的同构全局 API；也用 guard 恢复槽位、用原子值替代 Go 可直接赋值的全局 `Duration`。

Rust `parse_profile_data` 显式检查 gzip 魔数并用 prost 解码，Go 则由 `github.com/google/pprof/profile.Parse` 统一处理。Rust `validate_profile` 是针对本模块访问模式的移植实现，不应在没有新增对照测试时被描述为 Go 库 `CheckValid` 的完整逐行复刻。Go `TestProfiles` 通过真实 session/domain 和 performance schema SQL 查询六张表；Rust `profile_test.rs::test_profiles` 说明 SQL 栈尚未在此 util crate 内完整接线，改为直接验证等价的 `ProfileGraph` 名称分发。

## 扩展指南

新增一种 protobuf runtime profile 时，优先在宿主 `RuntimeProfileProvider` 增加名称支持；只要输出仍是 pprof protobuf，通常不需修改 `ProfileGraph`。若新增文本格式，则在 `ProfileGraph` 增加明确分支和独立解析函数，同时补充 `pkg/util/profile/profile_test.rs` 的 provider 调用参数、成功路径和未知名称回归。修改名称规范化必须同时考虑 CPU 与非 CPU 当前不对称语义以及 Go `profile.go`。

扩展 profile 校验应修改 `validate_profile`/`validate_string_index`，并在独立的 `migration_aster_unit_test.rs` 中加入合法与非法最小 profile；不要把 Rust 测试内嵌到 `profile.rs`。修改火焰图行、排序或百分比不应堆进本文件，应落在 `flamegraph.rs` 并同步 `flamegraph_test.rs`。修改 goroutine 文本解析则同步 Go 文本格式、树标记、奇数行/空块行为与错误文案测试。

要完成生产 SQL 接线，应在实现 `pkg/infoschema/perfschema/tables.rs::RowSource::local_profile` 的宿主层调用 `Collector::ProfileGraph`，并在独立集成测试中从 `performance_schema.tidb_profile_*` 验证实际行，而不是只新增 Cargo 依赖。还必须提供生产 `RuntimeProfileProvider`，明确其安装时机、进程级生命周期和并发替换策略。

性能上应重点评估全量缓冲和索引/树的峰值内存、CPU 采样对调用线程的同步阻塞，以及恶意或超大 profile 的输入限制。兼容性上应保留六列火焰图/四列 goroutine datum 形状、Go 可观察错误和稳定排序。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；本次查询时可用。
- RustCodeGraph `files --filter pkg/util/profile`：确认 `profile.rs`、`flamegraph.rs`、crate 入口、Go 对照及三个 Rust 测试文件均在索引中。
- RustCodeGraph `node --file pkg/util/profile/profile.rs`：读取完整 398 行，确认所有常量、类型、trait、函数、impl、`cfg(test)` 分支及直接下游调用。
- RustCodeGraph `query`：定位 Rust/Go `ProfileGraph`、`ProfileReaderToDatums`、`ParseGoroutines`，以及私有 `validate_profile`、`install_runtime_profile_provider`；宽泛 `explore/callers/callees` 结果存在同名噪声或未返回精确边，因此调用关系又用目标源码与精确 `rg` 交叉核验，未把噪声当作证据。
- 已读实现/边界：`pkg/util/profile/lib.rs`、`flamegraph.rs`、`Cargo.toml`、根 `Cargo.toml` facade 声明、`pkg/lib.rs::util::profile` 再导出，以及 `pkg/infoschema/perfschema/tables.rs`/`Cargo.toml` 的路由与依赖。
- 已读 Go 对照：`pkg/util/profile/profile.go`、`profile_test.go`。Go 测试验证经 session/domain 查询六张 performance schema 表。
- 已读 Rust 测试：`pkg/util/profile/profile_test.rs`、`migration_aster_unit_test.rs`、`flamegraph_test.rs`。它们分别覆盖名称分发/provider 参数与恢复、raw/gzip/profile 校验和 goroutine 错误、fixture 行序与百分比格式。
- 结构验证要求：文档必须存在，且固定十一个二级标题各出现一次；本任务不改变运行时代码，按计划不运行 Cargo。
