# `pkg/util/util.rs`

## 文件定位

本文件是 `astersql-util` crate 的通用工具实现，由 [`pkg/util/lib.rs`](./lib.rs) 以公开模块 `pub mod util` 挂载。它直接移植自同目录的 Go 文件 [`pkg/util/util.go`](./util.go)，把字节换算、字符串集合转换、结构化 SQL 日志字段生成、非 ASCII 字节展示、TCP I/O 计数、逐行读取、标识符检查、panic 转换、protobuf 深拷贝以及 PD 地址交集判断集中在一个模块中。Cargo 清单 [`pkg/util/Cargo.toml`](./Cargo.toml) 表明该模块属于 `astersql-util`，直接依赖 `anyhow`、`prost`、`astersql-parser`（代码别名 `task_parser`）、`astersql-session-sessmgr`（`task_sessmgr`）和 `astersql-util-traceevent`（`task_traceevent`）。

RustCodeGraph 将本文件识别为 442 行的已索引 Rust 源文件，并列出直接使用文件 `pkg/util/util_test.rs`、`pkg/util/security_2_aster_unit_test.rs`、`pkg/util/codec/codec_test.rs` 和 `pkg/util/regionsplit/model_handle.rs`。经源码搜索，能明确归属于本模块 API 的 Rust 使用点集中在前两个测试文件；后两个是索引的文件级关联，未找到对本文符号的显式调用。因此，不能据此声称这些工具已经全部接入 Rust 生产主链。

## 核心职责

- 提供无状态的小型转换：`ByteToGiB`、`SliceToMap`、`StringsToInterfaces`、`Str2Int64Map`、`PrintableASCII` 和 `FmtNonASCIIPrintableCharToHex`。
- 将 `task_sessmgr::ProcessInfo` 快照转换成稳定顺序的 `Vec<LogField>`，供慢查询或通用日志消费；`GenLogFields` 同时保护语句上下文引用，并执行 SQL 规范化和长度限制。
- 用 `TCPConnWithIOCounter` 装饰 `TcpStream`，在成功读写后累加共享原子字节计数。
- 用 `ReadLine`/`ReadLines` 模拟 Go `bufio.Reader.ReadLine` 的分片及行长限制语义。
- 把 panic 载荷、protobuf 消息、两组 PD 地址和 SQL 查询结果分别转换成可传播错误、深拷贝对象、同集群判定和地址列表。

该文件没有统一的业务入口，各 API 是相互独立的通用能力；扩展时应先确认调用方需要的是 Go 兼容行为，还是更符合 Rust 习惯的新接口，避免无意改变已迁移语义。

## 主要符号

- `ByteNumOneGiB: i64` 与 `ByteToGiB(f64) -> f64`：用 1024³ 作为 GiB 基数。
- `SliceToMap(&[String]) -> HashMap<String, ()>`：克隆元素并去重；`StringsToInterfaces` 将每个字符串克隆并装箱为 `Box<dyn Any + Send + Sync>`；`Str2Int64Map` 按逗号拆分，无法解析的片段以 `0` 代替后放入集合。
- `LogValue`、`LogField`：本地结构化日志表示。`LogValue` 当前有字符串、无符号整数和有符号整数三种变体，其中本文件的私有构造器只产生前两种。
- `ReferenceGuard`：私有 RAII 守卫；成功 `TryIncrease` 后持有 `ReferenceCount`，离开 `GenLogFields` 时由 `Drop` 调用 `Decrease`。
- `GenLogFields(Duration, &ProcessInfo, bool) -> Vec<LogField>`：本文件最复杂的入口，生成执行耗时、执行详情、统计版本、连接、表/索引、事务、内存、SQL、会话别名和受影响行数字段。
- `PrintableASCII` 与 `FmtNonASCIIPrintableCharToHex`：按字节判断 0x20..0x7e，并将其他字节格式化为大写 `\xHH`；0x7f 可按调用参数隐藏。
- `TCPConnWithIOCounter` 与 `NewTCPConnWithIOCounter`：分别实现 `Read` 和 `Write`，共享 `Arc<AtomicU64>` 计数器。
- `ReadLine`、`ReadLines`：泛型约束为 `BufRead`；前者去掉 `\n` 及其前面的 `\r`，后者连续读取指定行数。
- `IsInCorrectIdentifierName`：名称为空或最后一个字节是 ASCII 空格时返回 `true`，函数名中的 “InCorrect” 表示“不正确”。
- `GetRecoverError`：先调用飞行记录器 dump，再识别 `&str`/`String` panic 载荷，否则返回固定错误。
- `ProtoV1Clone<T: Message + Default>`：通过 `prost::Message::encode_to_vec` 后重新 `decode` 完成深拷贝。
- `CheckIfSameCluster`：依次调用两个地址 getter，以精确字符串交集判断是否同集群，并原样返回两组地址。
- `PdAddressRows`、`PdAddressDatabase<C>` 与 `GetPDsAddrWithoutScheme`：定义数据库/游标抽象，并生成执行固定 `INFORMATION_SCHEMA.CLUSTER_INFO` 查询的闭包。

## 执行流程

`GenLogFields` 首先检查 `ProcessInfo.RefCountOfStmtCtx`：存在且 `TryIncrease` 失败时立即返回空列表；成功时建立 `ReferenceGuard`，保证任何正常返回路径都会配对递减。随后按固定顺序追加 `cost_time`，读取可选 `StmtCtx` 的执行详情并转换其 zap 风格值；调用可选 `StatsInfo`，按键排序后把版本 `0` 写为 `pseudo`；再按字段是否为空追加连接、用户、数据库、表、索引、事务和内存峰值。最后通过 `task_parser::digester_impl::Normalize` 规范化 SQL，在允许截断且超过 8 KiB 时寻找 UTF-8 安全边界并附加原字节长度，继而追加会话别名和受影响行数。

`ReadLine` 循环调用 `BufRead::fill_buf`，寻找本缓冲片段中的换行符，复制消费部分并调用 `consume`。只有累计到第二个及后续片段时才检查 `max_line_size`，这是对 Go `bufio.Reader.ReadLine` 可观察行为的刻意保留；发现换行后移除 `\n` 和可选 `\r`。空输入返回 `UnexpectedEof`，而 EOF 前已经累积的无换行尾行正常返回。`ReadLines` 重复上述过程；若至少已有一行后遇到 `UnexpectedEof`，返回已有内容，否则传播错误。

`CheckIfSameCluster` 先执行第一个 getter 并构建 `HashSet`，再执行第二个 getter，只要存在完全相同的地址字符串就返回 `same = true`。`GetPDsAddrWithoutScheme` 返回捕获数据库引用的闭包，执行固定 SQL 后反复调用 `PdAddressRows::next_address`，直到 `None`。

## 数据与状态

大部分函数只操作调用方传入的数据，不保留全局状态。集合转换都会拥有自己的 `String`；重复元素自然被 `HashMap`/`HashSet` 合并。`LogField` 拥有键和值字符串，生成结果不借用 `ProcessInfo`。统计映射在格式化前按键排序，因此 Rust 输出是确定的。

`TCPConnWithIOCounter` 独占一个 `TcpStream`，但通过 `Arc<AtomicU64>` 与其他实例共享计数；计数是成功调用底层 `read`/`write` 后实际返回的字节数。`ReferenceGuard` 的生命周期覆盖整个日志字段构造阶段。PD 查询闭包借用 `database`，返回闭包的生命周期不能超过数据库引用。

## 依赖与调用关系

上游模块边界是 `pkg/util/lib.rs -> pub mod util`。RustCodeGraph 的符号查询同时找到 Go `pkg/util/util.go::GenLogFields` 与 Rust `util.rs::GenLogFields`，证明它们是同路径迁移对照；精确 callers/callees 查询在本次时限内未返回，故调用结论以索引文件关系和源码搜索交叉确认。

已验证的 Rust 使用者包括：`pkg/util/util_test.rs` 调用 `GenLogFields`、`ReadLine`、`IsInCorrectIdentifierName` 和 `ProtoV1Clone`；`pkg/util/security_2_aster_unit_test.rs::utility_functions_preserve_go_edges` 调用 `ByteToGiB`、`Str2Int64Map`、`FmtNonASCIIPrintableCharToHex`、`IsInCorrectIdentifierName`、`ReadLines` 与 `CheckIfSameCluster`。当前搜索未发现本文 API 的明确 Rust 生产调用，因此 `GenLogFields` 等能力应描述为“已实现并有测试”，而不是“已在 Rust 慢查询主链启用”。Go 生产调用则可见于 `pkg/util/expensivequery/expensivequery.go`、`pkg/util/memoryusagealarm/memoryusagealarm.go`、`pkg/util/stmtsummary/v2/reader.go` 和多个 panic 恢复点。

下游依赖分别是标准库集合/I/O/网络/原子类型，`anyhow` 的统一错误，`prost` 编解码，`task_sessmgr::ProcessInfo` 及其语句上下文/内存追踪器，`task_parser::digester_impl::Normalize`，以及 `task_traceevent::traceevent::DumpFlightRecorderToLogger`。

## 错误处理与边界

- `Str2Int64Map` 刻意吞掉解析错误并插入 `0`；空字符串也因此产生包含 `0` 的集合。这与 Go 忽略 `strconv.ParseInt` 错误一致，调用方不能用它做严格输入校验。
- `GenLogFields` 在引用计数无法增加时返回空列表而非错误。`StmtCtx`、`StatsInfo` 和内存追踪器在 Rust 中允许缺失；缺失语句上下文时受影响行数为 `0`。
- SQL 截断以字节上限为准，但使用 `char_indices` 保证不会切开 UTF-8 字符；追加的 ` len(N)` 不计入 8 KiB 上限。
- `FmtNonASCIIPrintableCharToHex` 按 UTF-8 原始字节处理，因此一个非 ASCII Unicode 字符会产生多个 `\xHH`；`max_bytes_to_show` 也是字节数。隐藏 0x7f 时该字节不占输出，但仍占输入索引。
- `ReadLine` 的行长限制只在跨缓冲区分片后检查；未分片的超长行可能通过，这是与 Go 对齐的测试约束，不应“顺手修正”。
- `GetRecoverError` 不保留 Rust 非字符串 panic 载荷的具体值，也不像 Go 版本那样专门识别 `error` 接口和追加栈信息。
- `ProtoV1Clone` 会传播编码后解码失败；泛型类型必须同时实现 `Message + Default`。
- `CheckIfSameCluster` 的 `false` 只说明两次结果没有交集，不能证明一定属于不同集群；两侧地址格式必须一致。任一 getter 失败立即返回错误，第二个 getter 不会在第一个失败后执行。
- `GetPDsAddrWithoutScheme` 依赖实现方在 `next_address` 中报告扫描及游标结束错误；抽象本身没有显式 close 方法，这与 Go `rows.Close` 的资源处理存在差异。

## 并发与资源生命周期

`GenLogFields` 的 `ReferenceGuard` 是本文件最重要的并发安全机制：只有成功增加 `StmtCtx` 引用才读取执行详情，RAII 保证字段生成成功或提前返回时释放引用。它不对 `ProcessInfo` 其他字段加锁，调用方仍需保证传入快照可安全读取。

TCP 计数采用 `Ordering::Relaxed`，只保证各次加法原子且最终可汇总，不建立与网络数据或其他状态之间的 happens-before 顺序。`Arc` 管理计数器生命周期，`TCPConnWithIOCounter` 被丢弃时底层 `TcpStream` 随之关闭；本文件没有后台任务、锁、通道或事务。

`GetPDsAddrWithoutScheme` 的闭包借用数据库，行对象完全位于单次调用栈内；`PdAddressRows` 没有资源关闭契约，实现真实数据库适配器时必须在其自身类型中确保析构或显式清理。

## 与 Go 版本的对应关系

Rust 的函数集合和主流程基本对应 `pkg/util/util.go`，独立 Rust 测试 `pkg/util/util_test.rs` 也对应 `pkg/util/util_test.go`。确定的语义对齐包括：GiB 基数、集合去重、无效整数变为 `0`、日志字段顺序、SQL 规范化/截断、ASCII/DEL 处理、实际 I/O 字节计数、`ReadLine` 分片限制怪癖、空/尾空格标识符、protobuf 深拷贝和地址交集判定。

已验证的差异包括：Rust `GenLogFields` 未生成 Go 的 cop-task 详情和 `mem_arbitration` 字段，代码注释将后者归因于当前 non-arbitrator 构建；Rust 对可选 `StmtCtx`/`StatsInfo` 更防御，而 Go 直接调用；Rust 的 stats 键排序，Go map 迭代顺序不固定；Rust SQL 截断避免破坏 UTF-8，Go 直接按字节切片；Rust `GetRecoverError` 对非字符串载荷给固定消息，Go 会格式化任意值并追踪 `error`；Rust `ProtoV1Clone` 使用 prost 编解码且可失败，Go `proto.Clone` 直接返回同类消息；Rust 数据库抽象没有体现 Go 的 `rows.Close`；Rust TCP 构造器返回具体包装类型，Go 返回 `net.Conn` 接口。

测试方面，`pkg/util/util_test.rs` 覆盖日志、行读取、标识符和 protobuf；`pkg/util/security_2_aster_unit_test.rs` 补充多项小工具和同集群交集。`SliceToMap`、`StringsToInterfaces`、TCP 计数、panic 转换、PD 查询闭包以及部分错误路径尚未在这两个直接测试文件中得到同等覆盖。

## 扩展指南

新增日志字段应修改 `GenLogFields`，明确字段顺序、空值是否省略以及 Go 对照行为，并在独立的 `pkg/util/util_test.rs` 增加断言；不要把测试内嵌进 `util.rs`。涉及 `StmtCtx` 的读取必须位于 `ReferenceGuard` 生命周期内。若补齐 cop-task 或内存仲裁字段，应先确认 `task_sessmgr` 已提供对应真实状态，不能用常量或桩模拟通过测试。

修改读行逻辑时必须保留或有意更新“仅跨缓冲片段才检查上限”的兼容约束，并同步 Rust/Go 测试。扩展 `TCPConnWithIOCounter`（例如 vectored I/O、超时或 shutdown）时，应继续以底层实际完成字节数计数，并评估是否仍只需 Relaxed 顺序。为 `GetPDsAddrWithoutScheme` 接入真实驱动时，优先给行集抽象补充可验证的资源释放方案和错误测试。

通用转换若改变容错策略会有兼容风险：尤其是 `Str2Int64Map` 的错误转零、DEL 隐藏规则、地址的精确字符串比较和标识符函数的反向布尔语义。性能上应避免在日志热路径增加无界分配；当前 SQL、统计信息和字段值均会分配拥有所有权的字符串。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`node --file pkg/util/util.rs --offset 1 --limit 400` 与后续 `offset 90 --limit 360` 覆盖目标文件；`query GenLogFields --kind function --json` 定位 Rust/Go 两个实现；文件索引给出四个关联文件。精确 callers/callees 查询未在 30 秒时限内返回，因此没有把缺失结果当成调用事实。
- Rust 源与 crate 边界：`pkg/util/util.rs`、`pkg/util/lib.rs`、`pkg/util/Cargo.toml`。
- Go 对照：`pkg/util/util.go`；生产使用搜索包括 `pkg/util/expensivequery/expensivequery.go`、`pkg/util/memoryusagealarm/memoryusagealarm.go`、`pkg/util/stmtsummary/v2/reader.go`、`pkg/util/wait_group_wrapper.go`、`pkg/util/topsql/reporter/pubsub.go` 与 `single_target.go`。
- 独立测试：`pkg/util/util_test.rs`、`pkg/util/security_2_aster_unit_test.rs`，并与 `pkg/util/util_test.go` 对照。测试证明日志基本字段及脱敏/截断、EOF 和分片行长语义、标识符边界、protobuf 深拷贝、小工具容错和 PD 地址交集。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的结构命令确认文件存在且固定二级标题恰好为 11，并人工检查未把索引关联误写成生产调用。
