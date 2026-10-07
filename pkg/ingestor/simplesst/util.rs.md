# `pkg/ingestor/simplesst/util.rs`

## 文件定位

该文件属于 `astersql-ingestor-simplesst` crate（见同目录 `Cargo.toml` 的 `package.name` 与 `package.metadata.porting.go-package`），由 `lib.rs` 以 `pub mod util` 暴露。它不负责 SST 编解码本身，而是为简单 SST 写入、按统计属性定位读取起点，以及全局排序任务清理对象提供通用算法。当前 Rust 实现的存储边界是 crate 内的 `MemoryStorage`；跨 crate 的生产调用证据是 `pkg/ingestor/globalsort/util.rs::CleanUpFilesInDirectories` 调用 `GetAllFileNamesFromScan`，写入侧则由 `pkg/ingestor/simplesst/writer.rs` 调用 `get_max_overlapping`。

## 核心职责

- `get_max_overlapping` 对带权区间端点进行原地排序和扫描，计算任一点的最大累计权重，供 writer 汇总 SST 文件重叠度。
- `remove_duplicates` 与 `remove_duplicates_more_than_two` 对“已按 key 排序”的输入按连续同 key 分组：前者移除整个重复组，后者每组保留前两项。
- `get_read_range_from_props` 批量读取统计文件，为多个升序 job key 生成 `[job_key][stat_path]` 偏移矩阵，使数据读取者能从不晚于目标 key 的位置开始 seek。
- `GetAllFileNamesFromScan` 以一次对象列表扫描筛选普通任务目录和随机分区前缀下的文件；`GetAllFileNamesInDirectories` 与单目录包装函数把它接到 `MemoryStorage`。

这些职责都是无持久状态的辅助计算；唯一外部状态来自传入的存储快照和统计文件内容。

## 主要符号

- `EndpointTp::{ExclusiveEnd, InclusiveStart, InclusiveEnd}`：枚举判别值同时定义同 key 排序顺序。开区间右端先扣减，闭区间左端随后增加，闭区间右端最后扣减，从而表达端点是否计入重叠。
- `Endpoint { Key, Tp, Weight }`：扫描线端点；代码约定 `Weight` 为正数，开始端点增加、结束端点减少。
- `get_max_overlapping(&mut [Endpoint]) -> i64`：Rust 风格核心入口；会改变切片顺序。`GetMaxOverlapping` 是 Go 命名兼容包装。
- `remove_duplicates`：以 `keep = 0` 调用私有核心 `remove_duplicates_with_keep`；`record_removed = false` 时仍统计重复组总元素数，但不保存被移除项。
- `remove_duplicates_more_than_two`：以 `keep = 2`、`record = true` 调用同一核心。第三返回值是所有大小至少为 2 的重复组的元素总数，包含保留下来的前两项，不等同于 `removed.len()`。
- `get_read_range_from_props`：固定使用 64 个统计文件的批次并发预算。`get_read_range_from_props_with_limit` 是可测试的参数化实现，传入 0 也会被提升为 1。
- `read_offsets_for_path`：单统计文件扫描器，使用 `StatsReader::from_storage(..., 250 * 1024)` 和 `next_prop()` 得到 `FirstKey`/`Offset`。
- `get_all_file_names`、`GetAllFileNames`：单目录包装；`GetAllFileNamesInDirectories` 支持批量目录；`GetAllFileNamesFromScan` 将目录匹配算法与具体对象存储扫描解耦。

## 执行流程

重叠度流程从 `(Key, Tp)` 排序开始。扫描每个端点时，`InclusiveStart` 加权，两个结束类型减权，并在每步更新最大值。类型顺序使同一 key 上的 `ExclusiveEnd` 在新起点之前生效，而 `InclusiveEnd` 在新起点之后生效；因此 `[a,b)` 与 `[b,c)` 不重叠，而在 `b` 闭合的区间仍计入该点。

去重流程以游标寻找每个连续同 key 区间 `[cursor, end)`。单元素组直接复制到输出；重复组将组大小累加到 `duplicates`，复制前 `keep` 项，并按 `record` 决定是否复制剩余项到 `removed`。算法只比较相邻元素，不会合并未排序输入中分散出现的相同 key。

偏移计算先为空 job key 直接返回，避免打开任何路径；随后创建零值矩阵，并把 paths 按并发上限分批。每批使用 `std::thread::scope` 为每个路径启动 worker，收集结果后按原路径索引写回矩阵。单文件 worker 顺序读取属性：当属性首 key 大于当前 job key 时，向后推进 job key 并继承上一偏移；否则把当前属性偏移记给当前 key。统计 EOF 时，剩余 key 继承最后偏移；所有 key 已确定时立即停止读取，因而不会解析无关尾部。

文件发现先把请求目录去重成 `HashSet`，再执行一次 `scan`。路径去除开头的 `/` 后，若第一段就是目标目录，则要求至少还有一段；否则第一段必须通过 `writer::IsValidPartition`，第二段必须是目标目录且还要存在第三段。命中路径保持原字符串，最后按字典序排序。

## 数据与状态

`Endpoint.Key`、去重回调返回的 key 和统计属性 `FirstKey` 都按字节字典序比较。去重函数虽然接收 `&mut Vec<T>`，当前 Rust 实现不在原向量内压缩，而是克隆元素生成 `output`/`removed`；因此要求 `T: Clone`，且调用者应使用返回值而不是期待 `input` 已被改写。偏移结果的外层顺序严格对应 `job_keys`，内层顺序严格对应 `paths`，空文件或创建 reader 时的 EOF 对应全零列。

目录集合仅在一次调用内存在；重复目录不会导致重复结果。`GetAllFileNamesFromScan` 保留扫描器返回的完整路径（包括可能的前导 `/`），仅用去除前导斜杠后的视图做分段匹配。

## 依赖与调用关系

下游依赖包括 `stat_reader::StatsReader`（打开和迭代范围属性）、`writer::IsValidPartition`（校验 `p` 加八位二进制的分区目录）、crate 的 `MemoryStorage`/`Result`/`Error`，以及标准库排序、`HashSet` 和 scoped threads。

RustCodeGraph 将该文件标为被 `pkg/ingestor/globalsort/util.rs` 与 `pkg/ingestor/simplesst/util_test.rs` 使用。源码补充显示 `writer.rs::MultipleFilesStat` 的统计更新以及 `GetMaxOverlappingTotal` 调用 `get_max_overlapping`；`globalsort/util.rs::CleanUpFilesInDirectories` 将真实 `Storage::list_prefix("")` 作为闭包传给 `GetAllFileNamesFromScan`，再把结果交给 `delete_files`。当前仓库搜索未发现 `remove_duplicates*` 和 `get_read_range_from_props*` 在独立测试之外的 Rust 生产调用，因此文档不把它们描述成已接通的完整生产链。

## 错误处理与边界

- 空 `job_keys` 返回空矩阵且不读取存储；空 `dirs` 返回空列表且不调用扫描闭包。
- `StatsReader::from_storage` 返回 EOF 时，该文件的所有偏移为 0；其他打开错误原样返回。读取中 EOF 是正常结束，其他错误在尽力 `close` 后返回。
- worker panic 被转换为 `Error::InvalidData("stats reader worker panicked")`；任一 worker 出错会使整个批次失败，尚未处理的后续批次不会启动。
- reader 的 `close` 错误被明确忽略，主读取错误优先；提前确定全部 key 时同样尽力关闭。
- 文件扫描闭包的泛型错误 `E` 不包装、不改写地向上传播；无分隔段、非法分区前缀、目录本身而非目录内文件都会被跳过。
- 输入契约未由运行时检查：job key 必须升序，去重输入必须按回调 key 排序，端点权重应为正且端点应成对。违反这些前提会得到无意义或不完整结果，而不是显式错误。

## 并发与资源生命周期

偏移扫描的并发单位是统计文件。`get_read_range_from_props_with_limit` 逐批创建 scoped threads，作用域保证借用的 `job_keys`、path 和 `MemoryStorage` 在线程退出前有效，也保证进入下一批前当前批已全部 join。默认最多同时处理 64 个路径；批处理而非全量任务队列意味着发生错误前，同批其他 worker 仍会完成 join。

每个 worker 独占一个 `StatsReader`，在 EOF、提前完成或错误路径上调用 `close`；没有跨 worker 共享 reader 或偏移列。共享 `MemoryStorage` 自身通过 crate 内锁实现并发读取。其余算法只使用调用栈和局部 `Vec`/`HashSet`，没有全局可变状态、异步任务或后台线程遗留。

## 与 Go 版本的对应关系

直接对照为 `pkg/ingestor/simplesst/util.go`。端点枚举的 iota 顺序、原地端点排序及加减顺序与 Go `GetMaxOverlapping` 一致。去重的分组语义、`keep = 0/2` 和第三返回值含义一致，但 Go `doRemoveDuplicates` 在输入切片内压缩并复用元素，Rust 为满足所有权安全而克隆到新向量；此外 Go 用 `intest.Assert` 限制 keep 值，Rust 私有核心只由两个固定包装调用。

Go `GetReadRangeFromProps` 使用带 context 的 error group、并发上限 64、真实 `storeapi.Storage` 和日志任务；Rust 保留 64 上限、每路径并发和偏移算法，但使用同步 scoped threads、`MemoryStorage`，没有 context 取消与任务日志。这是接口/运行环境差异，不应推断 Rust 已具备 Go 的取消传播能力。

Go `GetAllFileNames` 直接 `WalkDir` 并接受可变数量目录；Rust 将匹配算法抽为 `GetAllFileNamesFromScan`，以便 `globalsort` 的通用 `Storage` 复用。二者都匹配 `dir/...` 与 `pXXXXXXXX/dir/...`、忽略目录自身并排序结果；Rust 额外容忍内存 fixture 的前导斜杠。

## 扩展指南

新增端点类型或改变开闭语义时，必须同时审查 `EndpointTp` 的派生排序顺序、`get_max_overlapping` 的更新时机，以及 `writer.rs` 中构造端点的位置，并在独立的 `util_test.rs` 增加同 key 开闭组合回归。

扩展去重策略应修改私有 `remove_duplicates_with_keep` 或新增明确包装，并保持“重复总数是否包含保留项”的契约；不要把测试内嵌回生产源文件。若需要避免克隆，可重新设计所有权接口，但要核对 Go 的原地压缩语义、元素顺序和内存成本。

把偏移扫描接到真实对象存储前，需要在调用边界补足 Go 版本已有的 context 取消、日志和通用 storage 抽象，同时保持结果矩阵索引、EOF、提前停止和 64 并发预算。新增错误分支应同步 `pkg/ingestor/simplesst/util_test.rs`，尤其验证 reader 关闭和部分批次失败语义。

改变目录规则时，应以 `GetAllFileNamesFromScan` 为唯一匹配核心，并同步检查 `writer::IsValidPartition`、`globalsort::CleanUpFilesInDirectories` 及扫描一次/错误透传测试；宽化匹配可能误删其他任务对象，是最高兼容性风险。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file pkg/ingestor/simplesst/util.rs` 读取 23 个符号及“被 2 个文件使用”的文件级关系；对核心函数执行 `query`、`callers`、`callees`，函数级 callers 多数未建边，因此又以精确仓库搜索补齐直接调用证据。
- Rust 源码：`pkg/ingestor/simplesst/util.rs`、`lib.rs`、`writer.rs`、`stat_reader.rs`，以及跨 crate 调用 `pkg/ingestor/globalsort/util.rs::CleanUpFilesInDirectories`。
- crate 配置：`pkg/ingestor/simplesst/Cargo.toml` 和根 `Cargo.toml` 的 workspace/facade 条目，确认 crate 名、Go 包映射和工作区归属。
- 对照与测试：`pkg/ingestor/simplesst/util.go`、`util_test.go`、独立 Rust 测试 `util_test.rs`。Rust 测试覆盖端点语义、首/中/尾重复组、偏移矩阵、空 key 不读取、并发上限为 2、全部 key 确定后不解析损坏尾部、目录去重、单次扫描和错误原样传播。
- 本任务只生成文档，按总计划不运行 Cargo；交付前使用任务指定命令确认目标文件存在且恰有 11 个固定二级章节，并人工检查未声称尚无生产调用证据的接口已完成接线。
