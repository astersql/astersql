# `pkg/lightning/config/const.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-config`（见 `pkg/lightning/config/Cargo.toml`），集中保存 Lightning 配置层使用的容量阈值、比例默认值和 gRPC keepalive 参数描述。模块入口 `pkg/lightning/config/lib.rs` 以 `pub mod r#const` 声明它，并通过 `pub use r#const::*` 把公开符号再导出到 crate 根；使用方通常写 `astersql_lightning_config::READ_BLOCK_SIZE` 或在 crate 内写 `crate::READ_BLOCK_SIZE`，无需直接引用原始标识符 `r#const`。

文件只声明值和一个数据结构，不加载配置、不建立网络连接，也不执行 Region 切分。真正把部分默认值装配进任务配置的是 `pkg/lightning/config/config.rs::new_config` 与 `MydumperRuntime::adjust`；Go 版本中其余常量的运行时消费者分布在 Lightning importer、TiKV 客户端、mydump parser、DDL ingest 和 IMPORT INTO 等路径。

## 核心职责

1. 用私有的 `KIB`、`MIB`、`GIB` 按 1024 进制构造容量常量，避免公开值中重复书写换算表达式。
2. 为 mydumper 配置提供默认读取块大小、批大小、最大 Region 大小和批导入比例；`config.rs` 会在构造配置或修正非法输入时使用这些值。
3. 保存预切分 Region 所需的大小、键数及最大放大倍率。这些符号对外公开，但当前 Rust 仓库中没有目标文件之外的消费点；它们仍是 Go 对照 API 的移植声明，而不是已经接入 Rust 导入链的证据。
4. 用 `GrpcKeepaliveParams` 表达 keepalive 三元组。它是纯数据描述；当前 crate 没有 gRPC 依赖，且 Rust 侧没有消费者，因此不会像 Go 的 `grpc.WithKeepaliveParams` 那样直接生成拨号选项。
5. 保存兼容 Go 配置契约的 MySQL `max_allowed_packet` 缺省值。该值刻意是 `pub(crate)`，只供配置 crate 内部使用。

## 主要符号

- `KIB: i64 = 1024`、`MIB: i64 = 1024 * KIB`、`GIB: i64 = 1024 * MIB`：模块私有的二进制容量单位。所有乘数均在编译期求值。
- `DEFAULT_BATCH_IMPORT_RATIO: f64 = 0.75`：`MydumperRuntime::adjust` 在批导入比例不属于半开区间 `[0, 1)` 时采用的回退值。
- `READ_BLOCK_SIZE: ByteSize = 64 KiB`：`new_config` 的 `mydumper.read_block_size` 默认值；`MydumperRuntime::adjust` 也用它替换小于等于零的值。
- `SPLIT_REGION_SIZE: ByteSize = 96 MiB`、`SPLIT_REGION_KEYS: i32 = 960_000`：默认 Region 预切分阈值。Go 的 `const.go` 说明 96 MiB 对齐 TiKV 8.4.0 之前的 `coprocessor.region-split-size` 默认量级，并通过降低键阈值避免等待 TiKV 自动切分；Rust 注释仅保留了简化说明。
- `MAX_SPLIT_REGION_SIZE_RATIO: i32 = 10`：Region 切分大小相对基准值的倍率上限。当前 Rust 路径尚未使用；Go importer 会按任务数放大并以该值封顶。
- `DEFAULT_MAX_ALLOWED_PACKET: u64 = 64 MiB`：crate 内私有默认值。`new_config` 将其写入 TiDB 连接配置，`Checkpoint::adjust` 为 MySQL checkpoint 构造连接参数时也使用它。
- `GrpcKeepaliveParams { time, timeout, permit_without_stream }`：公开、可复制且支持调试和相等比较的参数结构；三个字段分别表示 ping 周期、响应超时以及空闲连接是否允许 ping。
- `DEFAULT_GRPC_KEEPALIVE_PARAMS`：`time = 60s`、`timeout = 120s`、`permit_without_stream = false` 的结构值。目前仅被定义和再导出。
- `BUFFER_SIZE_SCALE: i64 = 5`：公开不可变 `static`。Go mydump parser 以此放大块缓冲；Rust `pkg/lightning/mydump/parser.rs` 使用的是自己定义的同名常量 `2`，没有引用这里的 `5`。
- `DEFAULT_BATCH_SIZE: ByteSize = 100 GiB`、`MAX_REGION_SIZE: ByteSize = 256 MiB`：`new_config` 分别写入 `mydumper.batch_size` 和 `mydumper.max_region_size`。Rust mydump 的批大小计算另有一个数值相同、类型为 `f64` 的局部 `DEFAULT_BATCH_SIZE`，两者不是同一符号。

除 `DEFAULT_MAX_ALLOWED_PACKET` 与三个单位常量外，其余声明均从 crate 根公开。`ByteSize` 的真实定义在 `pkg/lightning/config/bytesize.rs`，是保存字节数的 `pub struct ByteSize(pub i64)`；因此本文件中的容量值最终都是精确整数，而非带单位的运行时对象。

## 执行流程

本文件没有函数调用流程；它参与的是配置装配流程：

1. crate 加载时，编译器构造各个 `const`/不可变 `static` 值；该过程没有 I/O 或惰性初始化。
2. `pkg/lightning/config/config.rs::new_config` 创建 `Config`，把 `DEFAULT_MAX_ALLOWED_PACKET` 写入 `tidb.max_allowed_packet`，把 `READ_BLOCK_SIZE`、`DEFAULT_BATCH_SIZE`、`MAX_REGION_SIZE` 写入 `mydumper` 对应字段。
3. 配置加载和调整后，`MydumperRuntime::adjust` 检查 `batch_import_ratio`：不在 `[0, 1)` 时写回 `DEFAULT_BATCH_IMPORT_RATIO`；检查 `read_block_size.0 <= 0` 时写回 `READ_BLOCK_SIZE`。显式且合法的用户值不会被这些默认值覆盖。
4. `Checkpoint::adjust` 在 checkpoint 驱动为 MySQL、且未提供 DSN 时构造 `MySqlConnectParam`，将 `DEFAULT_MAX_ALLOWED_PACKET` 放入连接参数。
5. 其他公开声明当前只经 `lib.rs` 暴露；代码搜索未发现 Rust 调用链，因此不能把 Go 侧的 Region 切分、buffer 扩容或 gRPC 拨号行为视作已在 Rust 中运行。

## 数据与状态

容量单位使用 `i64`，公开容量包装为 `ByteSize(i64)`；`DEFAULT_MAX_ALLOWED_PACKET` 因下游连接字段要求而转换为 `u64`。当前乘法结果远低于整数上限：最大公开容量是 100 GiB。`SPLIT_REGION_KEYS` 和倍率使用 `i32`，与 Rust 配置结构预期类型一致，但 Go 对照中的 `int` 会随平台字宽变化。

`const` 在使用处内联且没有独立可变状态；三个 `pub static` 同样未声明 `mut`，调用方不能安全地修改它们。`GrpcKeepaliveParams` 的 `Duration` 字段使用标准库类型，结构派生 `Clone`、`Copy`、`Debug`、`Eq` 和 `PartialEq`，可按值传递和比较，不持有 socket、任务句柄或回调。

默认值的权威性需按接线范围理解：`READ_BLOCK_SIZE`、`DEFAULT_BATCH_SIZE`、`MAX_REGION_SIZE`、`DEFAULT_BATCH_IMPORT_RATIO` 和私有 packet 值已经进入 Rust 配置流程；`SPLIT_REGION_*`、`MAX_SPLIT_REGION_SIZE_RATIO`、`DEFAULT_GRPC_KEEPALIVE_PARAMS`、`BUFFER_SIZE_SCALE` 当前只是公开声明。尤其不能仅凭同名或同数值，推断 `pkg/lightning/mydump` 的局部常量与本文件共享状态。

## 依赖与调用关系

直接下游依赖只有：

- `std::time::Duration`：构造 keepalive 时间值。
- `crate::ByteSize`：为容量常量提供配置域类型；其定义在 `pkg/lightning/config/bytesize.rs`。

Cargo 边界由 `pkg/lightning/config/Cargo.toml` 确认：库入口是 `lib.rs`，Go 包映射为 `pkg/lightning/config`。本文件本身不需要 Cargo 清单中的 `serde`、`toml`、`url`、`fail` 等依赖，也没有 `tonic`、`grpc` 或网络客户端依赖，这与 keepalive 仅为描述结构的事实一致。

Rust 直接上游为 `pkg/lightning/config/config.rs`：导入并使用 `DEFAULT_BATCH_IMPORT_RATIO`、`DEFAULT_BATCH_SIZE`、`DEFAULT_MAX_ALLOWED_PACKET`、`MAX_REGION_SIZE`、`READ_BLOCK_SIZE`。RustCodeGraph 能识别 `GrpcKeepaliveParams` 和 `ByteSize` 结构，但其常量查询/调用边未覆盖这些声明；因此对常量调用点采用精确 `rg` 补证。全仓 Rust 搜索没有发现其他消费点。

Go 对照的调用范围更广：`pkg/lightning/mydump/parser.go` 使用 `BufferSizeScale`；`lightning/pkg/importer/table_import.go` 和 `import.go` 使用 Region 三个阈值；`pkg/lightning/tikv/tikv.go`、`lightning/pkg/importer/precheck_impl.go` 与 `br/pkg/restore/split/client.go` 使用 keepalive 拨号选项。这些是设计对应关系和迁移参照，不是 Rust 已接线的调用边。

## 错误处理与边界

本文件没有返回 `Result`、没有 panic 分支，也不自行校验常量。错误或修正发生在消费者：`MydumperRuntime::adjust` 对非法比例和非正读取块大小静默回退，`Checkpoint::adjust` 只负责组装默认连接参数。

批导入比例的 Rust 边界是精确的半开区间 `[0.0, 1.0)`：负数、`1.0`、正无穷和 `NaN` 都因 `Range::contains` 返回 false 而回退到 `0.75`。Go `config.go` 使用 `value < 0 || value >= 1`，普通有限值边界一致，但两个比较对 `NaN` 都为 false，因此 Go 会保留 `NaN`；这是当前可观察的语义差异。

`READ_BLOCK_SIZE` 的回退边界与 Go 一致，均为小于等于零。`ByteSize` 内部允许表达有符号值，但这些声明全为正数。若修改容量常量，应同时检查向 `usize`、`u64` 或浮点数转换的消费路径，避免在其他平台或大值下发生截断、溢出或精度损失。

Rust 尚无同名独立 `const_test.rs`。现有间接测试 `pkg/lightning/config/config_test.rs::test_adjust_will_batch_import_ratio_invalid` 验证负比例回退为 `0.75`；`pkg/lightning/config/toml_codec_test.rs::load_toml_decodes_all_go_config_fields_owned_by_the_codec` 验证 64 MiB packet 字段可正确解码，但没有逐项锁定本文件全部常量。Go 的 importer、mydump 与 ingestor 测试为未接线符号提供行为参照。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件、连接或内存池。所有值都可只读共享；不可变 `static` 具有进程全生命周期，但没有初始化顺序、清理或竞争条件。

`GrpcKeepaliveParams` 也不拥有 gRPC 连接。即使调用方复制 `DEFAULT_GRPC_KEEPALIVE_PARAMS`，它得到的只是三个值；真正创建 channel、配置 keepalive 以及关闭连接的生命周期必须由未来的网络客户端接线负责。`BUFFER_SIZE_SCALE` 只表达倍率，不分配缓冲区；实际分配及溢出检查属于 parser 消费者。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/config/const.go`。数值上，Rust 的批导入比例、64 KiB 读取块、96 MiB Region 大小、960000 键、10 倍上限、64 MiB packet、60/120 秒 keepalive、倍率 5、100 GiB 批大小和 256 MiB Region 上限均与 Go 一致。

表示方式存在三类差异：

- Go 使用 `github.com/docker/go-units` 构造容量，Rust 用私有 `KIB/MIB/GIB`；结果同为 1024 进制字节数。
- Go 的 `DefaultGrpcKeepaliveParams` 已是 `grpc.DialOption`，可直接传给 `grpc.DialContext`；Rust 常量只是 `GrpcKeepaliveParams` 数据，且没有客户端适配代码。
- Go 的可变包级 `var` 可被测试或运行时代码替换；Rust 的 `pub static` 不可变，默认值不能在安全代码中重赋。Rust 把 keepalive 设为 `const`，也没有 Go 接口值的动态类型。

迁移完整度并不等同于数值完整度。Rust 已接入配置构造和调整的默认值，但 Region 切分、keepalive、`BUFFER_SIZE_SCALE` 仍未接入对应业务链。Rust parser 的局部倍率为 `2`，与 Go 通过本模块取得的 `5` 不一致；在没有进一步性能和内存行为验证前，不应把它们强行统一。

## 扩展指南

- 新增配置默认值时，先判断它属于通用配置契约还是具体算法内部常量。前者放在这里并从 `lib.rs` 再导出；后者应留在实际消费者模块，避免出现两个同名但未共享的值。
- 修改已接线的默认值时，同步检查 `config.rs::new_config`、`MydumperRuntime::adjust`、`Checkpoint::adjust`，并扩展独立的 `pkg/lightning/config/config_test.rs`；Rust 测试不得嵌入生产源文件。
- 接入 `SPLIT_REGION_SIZE`、`SPLIT_REGION_KEYS` 或倍率时，应按 Go 的 importer/ingestor 消费点验证大小与键数联动、倍率封顶和整数转换，而不是仅验证常量相等。
- 接入 keepalive 时，应在具体 gRPC 客户端层显式把三个字段映射为客户端选项，并新增独立测试覆盖默认值、覆盖值、无活跃流行为和连接关闭；不要让配置 crate 为此反向依赖具体网络实现，除非 crate 边界经过评审。
- 若决定让 Rust parser 使用 `BUFFER_SIZE_SCALE`，需先解释当前局部值 `2` 与 Go 值 `5` 的差异，并验证峰值内存和吞吐；不要保留两个看似权威的同名常量。
- 修改 Go/Rust 共有契约时同步检查 `pkg/lightning/config/const.go` 及相关 Go 测试。兼容风险主要是默认资源占用、Region 数量、连接保活策略和 MySQL packet 上限；性能风险集中在批大小、块大小、Region 阈值与缓冲倍率。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11467 个文件；`files --filter pkg/lightning/config` 确认目标、模块入口、Go 对照和独立测试集合；`node --file pkg/lightning/config/const.rs --offset 1 --limit 240` 读取完整 68 行源码；`query/node/callers/callees` 核对 `GrpcKeepaliveParams`；`node bytesize.rs::ByteSize` 核对容量包装类型及 `new_config` 实例化关系。常量符号未被索引查询命中，按技能规则用精确文本搜索补充。
- Rust 源码与边界：`pkg/lightning/config/const.rs`、`pkg/lightning/config/lib.rs`、`pkg/lightning/config/config.rs`、`pkg/lightning/config/bytesize.rs`、`pkg/lightning/config/Cargo.toml`。
- Rust 测试：`pkg/lightning/config/config_test.rs`、`pkg/lightning/config/toml_codec_test.rs`；同目录不存在 `const_test.rs`，也未发现测试逐项覆盖全部常量。
- Go 对照与消费者：`pkg/lightning/config/const.go`、`pkg/lightning/config/config.go`、`pkg/lightning/config/config_test.go`、`pkg/lightning/mydump/parser.go`、`lightning/pkg/importer/table_import.go`、`lightning/pkg/importer/import.go`、`pkg/lightning/tikv/tikv.go`、`lightning/pkg/importer/precheck_impl.go`、`br/pkg/restore/split/client.go`，以及相邻 importer/mydump/ingestor 测试中的引用。
- 全仓精确搜索确认 Rust 侧只有 `config.rs` 消费上述 5 个默认值，且 `SPLIT_REGION_*`、`MAX_SPLIT_REGION_SIZE_RATIO`、`DEFAULT_GRPC_KEEPALIVE_PARAMS`、本文件的 `BUFFER_SIZE_SCALE` 没有 Rust 消费者。本文不运行 Cargo；最终仅执行任务规定的 11 章节结构检查并人工复核结论可追溯性。
