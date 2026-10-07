# `pkg/ingestor/globalsort/testutil.rs`

## 文件定位

[`testutil.rs`](./testutil.rs) 属于 `astersql-ingestor-globalsort` crate（见 [`Cargo.toml`](./Cargo.toml)），由 [`lib.rs`](./lib.rs) 以 `pub mod testutil` 暴露。它不是全局排序的生产入口，而是同 crate 独立 Rust 测试共用的辅助层：[`reader_test.rs`](./reader_test.rs) 用它验证多文件读回，[`split_test.rs`](./split_test.rs) 用它构造切分器输入，[`testutil_test.rs`](./testutil_test.rs) 专门验证辅助函数的前置条件。包级 [`../doc.go`](../doc.go) 将 globalsort 放在 Ingestor 的“借助外部存储完成全局排序和归并”职责内；本文件只为这条路径提供测试装配和往返断言。

## 核心职责

- `mockOneMultiFileStat` 将按索引对应的 data/stat 路径包装成一个 `MultipleFilesStat`，使测试可以直接喂给 `NewRangeSplitter`。它不读取文件，也不生成范围属性；每个 `FilePair.properties` 初始为空。
- `testReadAndCompare` 驱动一次完整的测试读回：按 `RangeSplitter` 产生的 group 逐段确定半开区间，读取各组文件，展平并按键排序，最后把累计结果与期望 `kvs` 做精确相等比较。
- 两个函数都返回 crate 统一的 `Result`，把原 Go 测试辅助函数中的 `require` 断言转化为可传播、可单测的错误。

## 主要符号

- `pub fn mockOneMultiFileStat(data: &[String], stat: &[String]) -> Result<Vec<MultipleFilesStat>>`：要求两组路径等长；按 `zip` 顺序生成 `FilePair { data_file, stat_file, properties: Vec::new() }`，外层始终只有一个 `MultipleFilesStat`。
- `pub fn testReadAndCompare(token: &CancellationToken, kvs: &[KvPair], store: &dyn Storage, data_files: &[String], stat_files: &[String], start_key: Vec<u8>, memory_size_limit: usize) -> Result<()>`：公共测试入口。它组合 [`split.rs`](./split.rs) 的 `NewRangeSplitter`/`SplitOneRangesGroup`/`Close`、[`reader.rs`](./reader.rs) 的 `read_all_data`/`MemKvsAndBuffers::build`，以及 [`lib.rs`](./lib.rs) 的 `Storage`、`KvPair` 和 `next_key`。
- 本文件没有模块级常量、类型、trait、`impl` 或条件编译项；可观察状态全部局限于函数栈帧及下游对象。

## 执行流程

1. `testReadAndCompare` 先拒绝空 `kvs`，因为末组需要以最后一个期望键计算最终上界。
2. 它调用 `mockOneMultiFileStat` 校验路径数并构造一个文件统计组，再以 `memory_size_limit` 作为 group 大小阈值创建 `RangeSplitter`；其他 key/size 阈值设置为 `i64::MAX`，range-job 大小固定为 4 GiB。
3. 循环调用 `SplitOneRangesGroup`。非末组使用切分器返回的 `end_key_of_group`；末组以 `next_key(kvs.last().key)` 构造严格大于最后期望键的半开上界。
4. 当前实现对每个 data 文件使用起始偏移 `0`，并通过 `Storage::read(file).len()` 求结束偏移。随后以 `[current_start, current_end)` 调用 `read_all_data`；stat 文件列表在这里用于长度对齐，范围属性则由切分器处理。
5. `MemKvsAndBuffers::build` 将逐文件缓冲展平；本函数再按 `KvPair.key` 排序，把该组追加到 `actual`，并把组上界推进为下一组下界。
6. 当切分器返回空的 `end_key_of_group` 时结束循环，显式 `Close` 切分器；最后要求 `actual == kvs`，同时验证行数、键、值和顺序，成功返回 `Ok(())`。

## 数据与状态

- `MultipleFilesStat.filenames` 保留调用方路径顺序；data/stat 通过同一索引配对。空的 `properties` 会让 legacy fixture 路径在 `NewRangeSplitter` 中从 data 文件解码并逐 KV 派生 `RangeProperty`。
- `current_start` 表示下一组的包含式下界，`current_end` 表示排除式上界；每轮令前者等于后者，形成连续的半开区间。
- `loaded` 每轮新建，只保存当前 group 的逐文件缓冲、展平 KV 和尺寸计数；`actual` 跨 group 累计所有 KV，最终与传入的 `kvs` 做 `Eq` 比较。
- `starts` 全为零，`ends` 是完整对象长度。因此该辅助函数验证的是完整 fixture 文件经过键范围过滤后的内容，不复刻 Go 版本按 stat 属性计算精确读偏移的优化。
- `memory_size_limit` 同时影响切分器 group 大小及 `read_all_data` 的内存检查；但 `actual` 的跨组累计不计入该下游内存限制。

## 依赖与调用关系

- 已确认上游 Rust 调用者只有测试：`reader_test.rs` 两次调用 `testReadAndCompare` 覆盖多文件和单文件往返；`split_test.rs` 两次调用 `mockOneMultiFileStat` 构造切分输入；`testutil_test.rs` 覆盖辅助函数自身的参数边界。
- `mockOneMultiFileStat → FilePair/MultipleFilesStat` 仅做元数据装配；随后 `NewRangeSplitter` 消费这些结构，并可能通过 `Storage::read` 补建空属性。
- `testReadAndCompare → NewRangeSplitter → SplitOneRangesGroup` 决定每组键范围和活跃文件；`testReadAndCompare → Storage::read` 获取对象长度；`testReadAndCompare → read_all_data → read_one_file` 完成取消检查、解码、范围过滤和内存限制检查。
- crate 的直接依赖由 `Cargo.toml` 声明；本文件实际通过 crate 内部公共抽象间接使用 `astersql-ingestor-errdef` 等能力，没有自己引入外部 crate。
- RustCodeGraph 的文件关系显示 `testutil.rs` 被 `reader_test.rs`、`split_test.rs`、`testutil_test.rs` 使用；没有生产文件调用它，因此不能把本辅助流程描述为线上请求路径。

## 错误处理与边界

- data/stat 数量不等时，`mockOneMultiFileStat` 返回 `Error::InvalidArgument("data and stat file counts differ")`，避免 `zip` 静默截断。
- `kvs` 为空时，`testReadAndCompare` 返回 `Error::InvalidArgument("expected KVs must not be empty")`，既保留 Go 辅助函数对非空输入的隐含前置条件，也避免对 `last()` 解包失败。
- `NewRangeSplitter`、`SplitOneRangesGroup`、`Storage::read`、`read_all_data` 和 `Close` 的错误均由 `?` 原样传播。`read_all_data` 自身保证任一文件读取失败会清空当前 `loaded`，调用方不会比较半成品。
- 最终内容不相等时返回 `Error::InvalidData`，消息只报告实际与期望行数；具体键值差异需由调用测试或调试器进一步定位。
- 当前控制流只在所有 group 成功后调用 `Close`；此前任一 `?` 返回都会直接离开函数。`RangeSplitter` 不持有外部线程或文件句柄，但若未来为其增加必须执行的清理，应改为 RAII/作用域守卫而不能继续依赖末尾显式关闭。

## 并发与资源生命周期

- `CancellationToken` 以 `Arc<AtomicBool>` 共享取消状态；本文件不创建线程，令牌由调用者拥有，并由 `read_all_data` 在逐文件读取前检查。已开始的同步 `Storage::read` 不能被本层中途打断。
- `Storage` 要求 `Send + Sync`，但本函数只借用 `&dyn Storage` 并顺序调用。`read_all_data` 当前也逐文件处理，计算出的 concurrency 参数在 `read_one_file` 中尚未形成真实并行读取。
- `RangeSplitter` 从构造延续到循环结束，成功路径由 `Close` 置为关闭并释放属性迭代器；每轮 `MemKvsAndBuffers` 在迭代末尾析构，`actual` 保留至最终比较。
- 每个 group 内先展平再排序，排序为进程内可变切片排序；跨组只追加，不再执行全局排序。因此正确性依赖切分器输出的 group 边界单调且不重叠，相关单调性由 `split_test.rs` 覆盖。

## 与 Go 版本的对应关系

- 直接对照文件是 [`testutil.go`](./testutil.go)：函数名、单一 `MultipleFilesStat` 包装、`NewRangeSplitter` 阈值、逐 group 读回、末键 `Next()` 上界、组内按键排序和关闭切分器的总体意图一致。
- Go `mockOneMultiFileStat` 直接按 `data` 索引访问 `stat[i]`；Rust 先验证等长并返回 `Result`，这是安全化差异，`test_mock_one_multi_file_stat_rejects_mismatched_counts` 固化了该行为。
- Go 在空 `kvs` 时会因访问最后元素失败；Rust 将该前置条件显式化为 `InvalidArgument`，由 `test_read_and_compare_rejects_empty_expected_kvs` 覆盖。
- Go 通过 `simplesst.GetReadRangeFromProps` 从 stat 文件计算读偏移，并复用 `membuf` pool；Rust fixture 路径以 `0..完整文件长度` 读取后按键过滤，且每组新建 `MemKvsAndBuffers`。二者验证目标相同，但 I/O 精度、缓冲复用和性能特征不等价。
- Go 逐组逐 KV 立即断言并清空缓冲；Rust 累计全部 `actual` 后一次比较。因此 Rust 能检测额外/缺失行且错误可传播，但峰值辅助内存更高，错误消息也不含首个差异位置。

## 扩展指南

- 增加文件配对元数据时，优先修改 `mockOneMultiFileStat` 构造的 `FilePair`，并在独立的 `testutil_test.rs` 增加字段与不匹配输入测试；不要把测试嵌入生产源文件。
- 改变切分阈值或 group 读取流程时，应同步核对 `split.rs::NewRangeSplitter`/`SplitOneRangesGroup` 与 Go `testutil.go`，并运行 `reader_test.rs` 的单、多文件往返和 `split_test.rs` 的边界单调性用例。
- 若要对齐 Go 的范围读取性能，应接入与 stat 属性等价的 offset 计算，而不是只调整 `starts`/`ends` 常量；同时验证 `RecordFormat::GoBigEndian64` 与 legacy fixture 两条路径。
- 若允许空期望集，需要先定义是否仍应访问/校验文件、末组上界如何确定，再同时修改前置检查和回归测试，不能简单删除错误分支。
- 若辅助数据规模增大，应评估 `actual` 全量累计和完整对象长度探测的内存/I/O 成本；正确性风险集中在半开区间、重复/遗漏 KV、路径错配及失败时资源关闭。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`query mockOneMultiFileStat --kind function --json` 与 `query testReadAndCompare --kind function --json` 同时定位 Rust 和 Go 定义；文件节点确认 `testutil.rs` 的三个使用文件。精确 `callers`/`callees` 查询在 30 秒内未返回，具体调用点随后用源码搜索核实。
- 已读 Rust 源：`testutil.rs`、`testutil_test.rs`、`reader_test.rs`、`split_test.rs`、`reader.rs`、`split.rs`、`lib.rs`；包契约为 `pkg/ingestor/doc.go`。
- 已读边界声明：`pkg/ingestor/globalsort/Cargo.toml`（crate 名、`lib.rs` 入口、Go 包映射与依赖）；Go 语义对照为 `pkg/ingestor/globalsort/testutil.go`。
- 独立测试证据：`testutil_test.rs` 验证等长路径配对、错配拒绝和空期望拒绝；`reader_test.rs` 验证单/多文件读回；`split_test.rs` 验证辅助统计能驱动切分器及 group 边界行为。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证为固定 11 节结构检查及人工事实复核。
