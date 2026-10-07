# `pkg/executor/importer/production_size.rs`

## 文件定位

本文件属于 `astersql-executor-importer` crate。模块根 `pkg/executor/importer/lib.rs` 以 `mod production_size` 挂载它，并通过 `pub use production_size::*` 导出 `HostImportSizeEstimator`。它位于 IMPORT INTO / LOAD DATA 的文件发现与导入计划准备阶段：`LoadDataController` 扫描源文件并填充 `SourceFileMeta.real_size` 时，通过 `ImportSizeEstimator` trait 调用这里的宿主实现；该估算值随后参与总真实体量统计和调度资源估算，而不是实际的数据解码执行。

生产接线位于 `pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime`：该入口用 `HostImportSizeEstimator` 包装运行时原有的 `SizeEstimator`，从而让常见压缩文件使用本文件的采样逻辑，同时保留宿主提供的 Parquet 估算能力。crate 归属、`flate2`、`snap`、`zstd` 以及对象存储依赖由 `pkg/executor/importer/Cargo.toml` 声明。

## 核心职责

本文件有三项聚焦职责：

1. 为 Gzip、Snappy frame 和 Zstandard 源文件建立对应的流式解码器（`decode_reader`）。
2. 复刻 Go `pkg/lightning/mydump/loader.go::SampleFileCompressRatio` 的两遍前缀采样：第一遍读取一个解压缓冲区并记录底层压缩字节消费量，第二遍把底层读取严格限制在该压缩字节边界内，读尽解压流，以“解压字节数 / 压缩字节数”估算膨胀比（`calculate_file_bytes`、`EstimateRealSize`）。
3. 将 Parquet 的格式膨胀比查询透明委托给注入的 `ParquetEstimator`，使压缩流估算与 Parquet 宿主解码边界分离（`ParquetExpansionRatio`）。

该实现是估算器而非完整性校验器。无法采样时，除调用上下文已取消外，公开的 `EstimateRealSize` 会退回物理文件大小，保证单个坏样本或不支持的压缩类型不会阻断文件清单构建。

## 主要符号

- `MeasuredReader`：私有 reader 装饰器。持有对象存储 `Reader`、可取消的 `Context`、共享的已消费压缩字节计数 `Arc<AtomicU64>`，以及第二遍采样使用的可选读取上限。
- `impl Read for MeasuredReader::read`：每次底层读取前检查取消；按 `limit - consumed` 的饱和值约束本次切片长度；读取成功后以 `Relaxed` 原子加法累计实际压缩字节数。到达限制时返回 `Ok(0)`，向上层解码器呈现 EOF。
- `impl Drop for MeasuredReader::drop`：无论解码成功、出错还是提前返回，都尝试关闭底层对象 reader；关闭错误被有意忽略，因为 `Drop` 不能把新错误加入已经确定的返回路径。
- `decode_reader(reader, compression) -> Result<Box<dyn Read>, String>`：为 `Compression::Gz`、`Snappy`、`Zstd` 创建流式解码器；Zstd 构造错误转成字符串，其余压缩枚举返回明确的“不支持”错误。
- `calculate_file_bytes(context, path, compression, storage, limit) -> Result<(usize, u64), String>`：完成一次打开、解码和计数。返回值分别是本次产出的解压字节数和底层实际消费的压缩字节数。
- `HostImportSizeEstimator { ParquetEstimator }`：唯一公开类型。字段为 `Arc<dyn ImportSizeEstimator>`，用于跨服务共享并委托 Parquet 估算。
- `ImportSizeEstimator for HostImportSizeEstimator::EstimateRealSize`：公开 trait 入口。无压缩或非 Gzip/Snappy/Zstd 时直接返回 `file_size`；支持的压缩类型执行两遍采样并将比例乘以完整物理大小。
- `ImportSizeEstimator for HostImportSizeEstimator::ParquetExpansionRatio`：不自行解释 Parquet，原样转发上下文、路径、文件大小和存储对象。

## 执行流程

压缩文件的主流程如下：

1. `LoadDataController` 在 `pkg/executor/importer/import.rs` 扫描文件后调用 trait 方法 `EstimateRealSize`。
2. `EstimateRealSize` 先处理快速路径：`Compression::None` 和不属于 Gzip/Snappy/Zstd 的枚举都直接返回 `SourceFileMeta.file_size`。
3. 第一遍调用 `calculate_file_bytes(..., None)`。该函数检查上下文、通过 `Storage::Open` 打开文件、用 `MeasuredReader` 包装底层 reader，再由 `decode_reader` 建立解压层。它只向解码器请求一次最多 4096 字节的解压数据；解码器为满足这次请求实际读取了多少压缩字节，由 `consumed` 记录为 `offset`。
4. 若第一遍的 `offset == 0`，采样判定失败，避免后续除以零。
5. 第二遍重新打开同一路径并调用 `calculate_file_bytes(..., Some(offset))`。`MeasuredReader` 最多向解码器提供第一遍记录的压缩字节数；外层循环持续读取解压结果直到 EOF，并累计 `decoded`。
6. 计算 `decoded / offset` 得到样本膨胀比，再乘以完整 `file.file_size`，以 Rust 浮点转整数规则生成估算的 `i64` 大小。
7. 任一采样错误若发生在取消状态下则向上传播；否则回退为物理文件大小。成功值由调用方再乘格式膨胀比，并累加到 `TotalRealSize`。

Parquet 流程独立：`pkg/executor/importer/import.rs::estimateFormatSizeExpansionRatio` 仅在 `SourceType::Parquet` 时调用 `ParquetExpansionRatio`；本实现把调用交给 `ParquetEstimator`，调用方再将结果下限钳制为 `1.0`。

## 数据与状态

该文件没有全局可变状态。一次采样的状态全部局限在栈对象及 `MeasuredReader` 内：

- `consumed: Arc<AtomicU64>` 跨过 decoder 对 reader 的所有权包装，让 `calculate_file_bytes` 在 decoder 生命周期结束后仍能读取计数。它只负责计量，不决定文件偏移，也不执行 seek。
- `limit: Option<u64>` 为 `None` 时允许解码器按需读取；为 `Some(offset)` 时形成第二遍采样的压缩字节硬边界。`saturating_sub` 防止已消费值大于限制时发生无符号下溢。
- `total: usize` 记录当前一遍产生的解压字节数；第一遍最多记一次 4096 字节读取，第二遍累计到受限输入耗尽。
- `SourceFileMeta.file_size` 与最终结果均为 `i64`，中间比例使用 `f64`。代码没有额外检查负文件大小、浮点溢出或超过 `i64` 的估算；其前提是上游存储元数据提供有效的非负物理大小。
- `ParquetEstimator` 通过 `Arc` 共享，但本文件不缓存比例，不修改被委托对象，也不保存文件级状态。

## 依赖与调用关系

上游关系：

- `pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime` 构造 `HostImportSizeEstimator`，并将原 `services.SizeEstimator` 放入 `ParquetEstimator` 字段。
- `pkg/executor/importer/import.rs::LoadDataController` 在构建 `SourceFileMeta` 时调用 `EstimateRealSize`，把结果与 `estimateFormatSizeExpansionRatio` 相乘后写入 `real_size`。
- `pkg/executor/importer/import.rs::estimateFormatSizeExpansionRatio` 通过同一 trait 调用 `ParquetExpansionRatio`。

下游关系：

- `astersql_objstore_storeapi::{Context, Storage}` 提供取消检查和外部文件打开边界；`Storage::Open` 返回 `astersql_objstore_objectio::Reader`。
- `flate2::read::MultiGzDecoder`、`snap::read::FrameDecoder`、`zstd::stream::read::Decoder` 完成三种流式解压。
- `std::io::Read` 是解码器的统一接口；`AtomicU64` 只用于跨所有权边界读取压缩字节计数。
- Parquet 路径依赖注入的 `dyn ImportSizeEstimator`，本文件不依赖具体 Parquet reader。

RustCodeGraph 对 `calculate_file_bytes` 给出的直接调用边包括 `Context::check`、`MeasuredReader` 构造、`MeasuredReader::read` 和 `decode_reader`；对公开类型的实例化边包括 DXF `FromEncodeRuntime`、调度器测试和本文件的独立测试。trait 对象调用可能不表现为具体实现的 caller 边，因此上述动态分派关系还由构造点与 `import.rs` 的 trait 调用共同核对。

## 错误处理与边界

- 上下文在打开前、每次底层读取前、完成采样后都会检查。已取消时通常得到 `Interrupted`，经字符串化后最终由 `EstimateRealSize` 保留为错误，不触发物理大小降级。
- `Storage::Open`、底层读取、解码器读取和 Zstd decoder 构造错误统一转换为 `String`。非取消错误在 `EstimateRealSize` 边界被吞掉并返回 `file_size`，与 Go 公开估算函数的容错目标一致，但 Rust 版本在此处不记录日志。
- `UnexpectedEof` 在两遍读取中均按可接受的样本结束处理。普通 EOF 表现为 `Ok(0)`；其他 I/O 错误返回失败。
- 第一遍消费零压缩字节会显式返回 `compressed sample read zero bytes`，防止零除；若上下文未取消，该错误最终降级为 `file_size`。
- 未压缩文件、未知/不支持的压缩枚举不尝试打开存储，直接采用物理大小。`decode_reader` 自身仍防御性地拒绝非三种支持类型。
- `MeasuredReader::Drop` 会尝试关闭 reader，但忽略 close 错误。因此关闭失败既不会覆盖已有解码错误，也不会使成功估算失败；需要可观察关闭错误时必须调整接口，而不能仅修改调用方。
- 结果使用 `(ratio * file_size as f64) as i64`。新增超大文件或不可信元数据支持时，应单独评估精度、非有限浮点值和转换饱和行为。

## 并发与资源生命周期

`HostImportSizeEstimator` 满足 `ImportSizeEstimator: Send + Sync`，可通过 `Arc` 被调度服务共享；其方法本身无缓存、无锁且每次调用独立打开 reader，因此并发文件估算之间没有共享的可变文件状态。

单次压缩估算顺序执行两次 `Storage::Open`，不会并行读取同一对象。每次打开的 reader 被移动到 `MeasuredReader`，再被移动到 decoder；decoder 离开作用域时触发 `MeasuredReader::drop` 并关闭底层资源。第一遍完成后 decoder 已销毁，第二遍才重新打开文件。`consumed` 使用 `Relaxed` 是因为它承担计数而非跨线程同步协议；当前流程虽用 `Arc` 保持所有权可见性，但没有把 reader 或计数器交给后台任务。

取消状态由克隆的 `Context` 共享。读取发生期间每次进入 `MeasuredReader::read` 都重新检查，使长解压过程能够在下一次底层读取前停止；若解码器只消费已缓冲数据，取消要到下一次底层 read 或函数末尾的 `context.check()` 才被观察。文件关闭仍由 `Drop` 保证尝试执行。

## 与 Go 版本的对应关系

主要对照为 `pkg/lightning/mydump/loader.go`：

- Rust `calculate_file_bytes` 对应 Go `calculateFileBytes`。两者都以 4096 字节缓冲采样；第一遍只读一次以找出解码器实际消费的有效压缩边界，第二遍限制原始 reader 到该边界并读尽解压输出。
- Rust `EstimateRealSize` 对应 Go `EstimateRealSizeForFile` 加 `SampleFileCompressRatio` 的组合：无压缩时返回物理大小，采样成功时以膨胀比乘完整文件大小，普通采样失败时回退物理大小。
- Go 通过 `NewLimitedInterceptReader` 和 `Seek(0, io.SeekCurrent)` 得到压缩位置；Rust 对象 reader 的统一抽象虽支持 seek，但这里用 `MeasuredReader` 直接累计实际读取字节，并用第二遍的 `limit` 模拟有限 reader。这是实现手段差异，算法意图相同。
- Go 的压缩类型转换集中在 `ToStorageCompressType`；Rust 直接匹配 `Compression::{Gz, Snappy, Zstd}` 并实例化对应 crate decoder。
- Go 估算错误会写日志后回退；Rust 对非取消错误静默回退，但对已取消上下文返回错误，以便 Rust 调度链终止。这是可观察的错误语义差异。
- Go 的 Parquet 体积逻辑位于其他宿主组件；Rust 通过 `ParquetEstimator` 明确保留该边界，没有在本文件复刻 Parquet decoder。

Go 测试 `pkg/lightning/mydump/loader_test.go::TestEstimateFileSize` 验证无压缩、成功比例和失败回退；同文件的压缩采样测试验证实际 Gzip 比例。Rust 独立测试 `pkg/executor/importer/production_size_test.rs::compressed_file_estimator_samples_real_gzip_twice_and_observes_cancel` 验证真实 Gzip 的 10000 字节估算、Parquet 委托返回值以及取消传播。

## 扩展指南

- 新增压缩格式时，应同步更新 `EstimateRealSize` 的支持类型筛选和 `decode_reader` 的 decoder 分支，确认该格式在“受限压缩前缀 + 允许尾部不完整”的条件下能稳定结束；独立测试应放在 `pkg/executor/importer/production_size_test.rs`，不要嵌入生产文件。
- 修改采样窗口或两遍算法时，应保持第一遍只用于确定底层有效消费边界、第二遍用同一边界读尽解压输出这一不变量，并与 Go `calculateFileBytes` / `SampleFileCompressRatio` 对照。尤其要测试 decoder 预读、截断输入、空文件和 `UnexpectedEof`。
- 修改错误策略时，应区分取消与数据/存储错误。取消必须继续向上传播；是否记录普通失败、是否允许静默回退会影响运维可观测性和导入计划精度。
- 修改 reader 生命周期时，应保留所有返回路径的关闭保证，并为 close 失败是否可见作出明确设计。当前 `Drop` 的 best-effort 语义不能证明远端 close 成功。
- 修改 Parquet 行为应优先落在注入的宿主 estimator；若改变此处的委托接口，还需同步 `ImportSizeEstimator` trait、DXF `FromEncodeRuntime` 接线和 `estimateFormatSizeExpansionRatio` 调用方。
- 性能评估应关注每个受支持压缩文件固定两次对象打开、远端请求次数、decoder 预读量和极高压缩比下第二遍的 CPU/输出字节量；不要用缓存改变跨任务隔离，除非同时定义失效与并发规则。

## 验证依据

- 生产源码与符号：`pkg/executor/importer/production_size.rs`；RustCodeGraph 索引显示 142 行、14 个符号，并确认 `calculate_file_bytes -> Context::check / MeasuredReader / read / decode_reader` 的内部边。
- 公开契约与调用点：`pkg/executor/importer/import.rs::ImportSizeEstimator`、文件扫描阶段的 `EstimateRealSize` 调用、`estimateFormatSizeExpansionRatio`；`pkg/executor/importer/lib.rs` 的模块挂载与再导出。
- 生产构造点：`pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime`。RustCodeGraph 的 `HostImportSizeEstimator` trail 也列出该实例化边。
- crate 与依赖：`pkg/executor/importer/Cargo.toml`，确认 crate 名、模块根及 `flate2`、`snap`、`zstd`、mydump、objectio/storeapi 依赖。
- 资源与取消契约：`pkg/objstore/objectio/interface.rs::{Context, Reader}`、`pkg/objstore/storeapi/storage.rs::Storage::Open`。
- Rust 测试：`pkg/executor/importer/production_size_test.rs`；另参考 `pkg/dxf/importinto/scheduler_test.rs` 的服务接线构造。
- Go 对照与测试：`pkg/lightning/mydump/loader.go::{calculateFileBytes, SampleFileCompressRatio, EstimateRealSizeForFile}`、`pkg/lightning/mydump/loader_test.go`；`pkg/executor/importer/import.go` 还证明 Go IMPORT INTO 扫描流程调用 `mydump.EstimateRealSizeForFile`。
- 未运行 Cargo：任务是纯文档分析，计划明确禁止 Cargo。验收采用固定章节结构检查和人工事实复核。
