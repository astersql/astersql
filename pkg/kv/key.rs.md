# `pkg/kv/key.rs`

## 文件定位

`pkg/kv/key.rs` 属于 `astersql-kv` crate，是 SQL 层与存储层共同使用的“键与行标识”基础实现。`pkg/kv/lib.rs` 在 `pub mod key` 中通过 `include!("key.rs")` 装入该文件，并以 `pub use key::*` 从 crate 根导出，因此调用方通常写作 `astersql_kv::Key`、`kv::NewCommonHandle` 等，而不需要显式经过 `key` 子模块。

该文件位于存储访问主链的公共数据模型层：`Key`/`KeyRange` 描述按字节序扫描的边界，`Handle` 及其实现描述表行标识，`HandleMap` 系列按行标识保存临时状态。实际调用可见于 `pkg/tablecodec/tablecodec.rs`（编码键与行句柄互转）、`pkg/planner/util/handle_cols.rs`（构造复合或分区句柄）、`pkg/session/runtime/relational_scan.rs`（生成前缀扫描上界）和 `pkg/executor/index_merge_reader.rs`（句柄内存计量）。本文件不执行事务、RPC 或磁盘 I/O，也不拥有存储连接。

crate 边界由 `pkg/kv/Cargo.toml` 定义：包名为 `astersql-kv`，默认不启用 feature，`nextgen` 仅向 `kerneltype` 和 `keyspace` 透传；本文件本身没有条件编译分支。与本文件直接相关的 crate 内再导出包括 `codec`（`astersql-util-codec`）、`hack::MemAwareMap`（`astersql-util-hack`）、`size`（`astersql-util-size`）和 `types::Datum`。

## 核心职责

1. 提供保持字节字典序的键操作：`Key::Next` 生成单键的严格后继，`Key::PrefixNext` 生成前缀区间的排他上界，另有比较、前缀判断、深拷贝和十六进制显示。
2. 表示左闭右开区间 `[StartKey, EndKey)`，通过 `KeyRange::IsPoint` 在不分配临时后继键的情况下识别单点区间，并由 `KeyRangeSliceMemUsage` 按容量估算区间集合内存。
3. 以 `Handle` trait 统一整数主键、复合主键和分区行标识；负责编码、列切片、解码、比较、复制和内存计量。
4. 提供 `HandleMap` 与 `MemAwareHandleMap<V>`，把整数/复合、普通/分区四类键分开存储，避免把动态 trait 对象直接作为哈希键，同时保留遍历时恢复原句柄的能力。

它刻意不负责键空间前缀的业务编码、表/索引 key 的布局或 Datum 编码规则；这些分别由上游 keyspace/tablecodec 和下游 `codec` 模块决定。本文件只操作已经形成的字节序列和句柄抽象。

## 主要符号

- `pub struct Key(pub Vec<u8>)`：拥有型高层键。`Next(&self) -> Key` 在末尾追加 `0x00`；`PrefixNext(&self) -> Key` 从末字节向前做无符号进位，空键或全 `0xff` 时退化为“原键追加 `0x00`”。`Cmp` 固定返回 `-1/0/1`，`String` 输出小写十六进制。
- `pub struct KeyRange { StartKey, EndKey }`：左闭右开区间。`IsPoint` 接受两种点区间表示：`EndKey == StartKey.Next()`，或等长的 `EndKey == StartKey.PrefixNext()`；全 `0xff` 起点的等长回绕不是点区间。
- `pub fn KeyRangeSliceMemUsage(&Vec<KeyRange>) -> i64`：以 vector 容量乘 `size_of::<KeyRange>()`，再加每个起止键的底层 `Vec` 容量。它是统计口径，不是分配器的精确驻留内存。
- `pub struct Entry`：一个拥有型 `Key`/`Vec<u8>` 键值项，本文件仅定义容器。
- `pub trait Handle: Any`：行标识协议。除 `as_any` 用于 Rust 动态向下转型外，其余方法与 Go `Handle` 对齐：类型判别、下一句柄、相等/排序、编码与列访问、Datum 解码、显示、内存计量和深拷贝。
- `pub struct IntHandle(pub i64)`：整数句柄。编码委托 `codec::EncodeInt`，编码长度固定为 8；不支持 `NumCols`/`EncodedCol`，调用会 panic；`Next` 使用 wrapping 加一。
- `pub struct CommonHandle`：非整数复合句柄，私有字段 `encoded: Vec<u8>` 保存编码，`colEndOffsets: Vec<u16>` 保存每列累计结束偏移。`NewCommonHandle` 用 `codec::CutOne` 扫描列边界；短于 9 字节时只将保存的编码补零到 9 字节，列边界仍来自原输入。
- `pub struct PartitionHandle`：组合底层 `Box<dyn Handle>` 与 `PartitionID`。除比较/相等/复制/内存计量外，大多数方法直接转发给底层句柄。
- `pub struct HandleMap`：值为 `Box<dyn Any>` 的非泛型映射，支持 `Get`、`Set`、`Delete`、`Len`、`Range` 和估算式 `MemUsage`。
- `pub struct MemAwareHandleMap<V>`：泛型映射，底层使用 `hack::MemAwareMap`；支持 `Get`、返回本次内存变化量的 `Set`、以及可提前停止的 `Range`，不支持删除。
- `SizeofHandleMap`、`SizeofStrHandleVal`：当前 Rust 结构体的静态大小，用于复刻 Go 内存统计公式；`calcIntsMemUsage`、`calcStrsMemUsage` 和 `newMemAwareMap` 是文件内辅助函数。

## 执行流程

键区间的典型流程是：调用方先得到某个表、索引或字段前缀，再用 `Key::PrefixNext` 生成排他上界，最终把二者组成 `[prefix, prefix.PrefixNext())` 交给迭代器或扫描请求。`PrefixNext` 复制输入，从尾部逐字节加一；遇到未溢出的字节即停止，低位溢出字节保留为零。若没有任何可进位字节，则恢复原内容并追加零。`pkg/session/runtime/relational_scan.rs`、`pkg/structure/hash.rs` 等调用点体现了这条路径。单键边界用 `Next` 追加零，`KeyRange::IsPoint` 则直接比较长度、尾零和进位位置，以避免真正构造后继键。

复合句柄的构造流程是：上游 tablecodec/planner 先用有序 codec 生成编码，`NewCommonHandle` 保存编码（短编码补至 9 字节），随后反复调用 `codec::CutOne`，把每一列结束位置累加为 `u16` 并记录。`EncodedCol(i)` 根据相邻偏移切片并复制；`Data` 对每列调用 `codec::DecodeOne`；`String` 再逐 Datum 调用 `ToString` 并拼成 `{a, b}`。`CommonHandle::Next` 对完整编码做 `PrefixNext`，用于获得更大的最小句柄，但结果明确不保证仍可解码。

映射写入流程先通过 `as_any().downcast_ref::<PartitionHandle>()` 判断是否为分区句柄，再用 `IsInt` 选择整数或编码字节键。普通句柄写入 `ints`/`strs`；分区句柄先以 `PartitionID` 选择二级 map。复合句柄值同时保存 `h.Copy()`，因为仅凭编码字节不能在 `Range` 时恢复原来的动态句柄类型。读取和删除复用相同路由规则。遍历顺序依次为普通整数、普通复合、分区整数、分区复合；任一回调返回 `false` 即终止，但各 `HashMap` 内部顺序不稳定。

`MemAwareHandleMap<V>` 采用同一分流规则。首次写入某个分区时才通过 `newMemAwareMap` 创建二级映射；`Set` 把底层 `MemAwareMap::Set` 返回的内存增量原样交给调用方。它不提供删除，以保持与 Go 版本“只增长、由调用者累计实际尺寸”的契约。

## 数据与状态

所有键和句柄数据均由值对象拥有：`Key`、`CommonHandle` 的编码和偏移、`Entry::Value` 都是 `Vec`。公开方法中多处为保持 Go 风格 API 而返回新 `Vec` 或 `Box<dyn Handle>`，例如 `Encoded`、`EncodedCol`、`Copy` 和 `Next`；调用者不能依赖这些结果与原对象共享底层缓冲区。

`CommonHandle` 的关键不变量是：`colEndOffsets` 单调表示原始有效编码中各列的累计末端，最后一个偏移不得超过 `encoded` 长度；不足 9 字节的编码尾部可以是补零，但补零不是额外列。偏移类型为 `u16`，构造时使用 wrapping 累加，代码本身没有拒绝超过 `u16` 表示范围的总列编码长度。

`HandleMap` 把状态分为四组：`ints`、`strs`、`partitionInts[pid]`、`partitionStrs[pid]`。因此相同底层句柄在不同分区、以及分区与非分区空间中互不覆盖。`MemUsage` 只计算 map 项、键字节和包装结构体的约定大小，不递归追踪 `Any` 值指向的对象，也不等同于 Rust `HashMap` 的真实 allocator 开销。

`PartitionHandle::Equal` 在两边都是分区句柄时同时比较分区 ID 与底层句柄；另一边不是分区句柄时只比较底层句柄。`Compare` 更严格：只接受另一个 `PartitionHandle`，先按分区 ID、再按底层句柄排序。其 `Next` 直接返回底层句柄的后继，不保留分区包装，这与 Go 的匿名嵌入方法提升语义一致。

## 依赖与调用关系

模块接线为 `pkg/kv/lib.rs -> pub mod key -> include!("key.rs") -> pub use key::*`。`pkg/kv/Cargo.toml` 将 codec、hack 和 size 作为本地 workspace 路径依赖引入；`lib.rs` 再分别以 `crate::codec`、`crate::hack`、`crate::size`、`crate::types` 暴露给本文件。

主要下游调用为：`IntHandle::Encoded -> codec::EncodeInt`；`NewCommonHandle -> codec::CutOne`；`CommonHandle::Data -> codec::DecodeOne`；`IntHandle::Data -> types::NewIntDatum`；`CommonHandle::String -> Datum::ToString`；`MemAwareHandleMap::Set -> hack::MemAwareMap::Set`；内存估算使用 `size` 常量与 `std::mem::size_of`。这些调用都在内存中完成。

RustCodeGraph 对 `pkg/kv/key.rs` 报告 92 个使用文件。代表性上游边包括：`pkg/tablecodec/tablecodec.rs` 调用 `NewCommonHandle`、`NewPartitionHandle` 和 `NewHandleMap` 完成 key/handle 解码及重复检测；`pkg/planner/util/handle_cols.rs` 从行数据构造句柄；`pkg/planner/core/operator/physicalop/physical_batch_point_get.rs` 构造复合点查句柄；`pkg/session/runtime/row_codec.rs`、`pkg/lightning/backend/kv/*.rs` 解码复合句柄；`pkg/session/runtime/relational_scan.rs`、`pkg/domain/domain.rs`、`pkg/structure/hash.rs` 使用 `PrefixNext` 形成扫描边界。

RustCodeGraph 的宽泛 `explore` 会把仓库内许多同名 `Key`/`Next` 符号合并，因此本任务又以精确符号查询和限定为 `*.rs` 的调用点搜索消歧。图查询能定位 Rust/Go 两个 `NewCommonHandle`、`NewHandleMap`、`NewPartitionHandle` 定义，但 `callers` 对这些重名、方法式 API 未返回可靠的精确边；上述调用边因此由索引的“used by 92 files”信息与限定路径的直接调用点共同确认。

## 错误处理与边界

显式可恢复错误集中在 codec：`NewCommonHandle` 将 `codec::CutOne` 错误向上传播，`CommonHandle::Data` 将 `DecodeOne` 错误向上传播。`CommonHandle::String` 不返回 `Result`，而是把解码或 Datum 文本转换错误直接变为字符串；调用方不能通过返回类型区分正常文本与错误文本。

若混合比较整数和复合句柄，`IntHandle::Compare`、`CommonHandle::Compare` 会 panic；`PartitionHandle::Compare` 与非分区句柄比较也会 panic。`IntHandle::IntValue` 有效，`CommonHandle::IntValue` 会 panic；整数句柄的 `NumCols`/`EncodedCol` 会 panic。`CommonHandle::EncodedCol` 对越界列索引或不合法偏移会触发 Rust 切片边界 panic。安全扩展时应在调用前维持类型与索引不变量，而不是把这些方法当作可探测接口。

数值边界保持 Go 的溢出语义：`IntHandle::Next` 在 `i64::MAX` 上 wrapping 到 `i64::MIN`；`PrefixNext` 对全 `0xff` 不回绕为空键，而是追加零；`KeyRange::IsPoint` 明确把全 `0xff` 起点到等长零值终点判为非点区间。`NewCommonHandle` 对不足 9 字节编码补零，对首个零字节停止列扫描；无效 codec 编码由 `CutOne` 报错。

`HandleMap::Get` 返回借用的 `dyn Any`，调用方须用正确具体类型 downcast；类型不匹配得到 `None`，不会由映射自动报告错误。删除不存在的普通或分区项是无操作。设置相同逻辑键会覆盖旧值；当前实现不单独报告覆盖。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道、事务或外部资源。所有生命周期由 Rust 所有权和借用控制：`Set` 取得值所有权，映射销毁时释放键、句柄副本和值；`Get` 返回与映射借用绑定的引用；`Range` 只在不可变借用期间把临时或保存的句柄引用交给回调。分区整数项在遍历时临时构造 `PartitionHandle`，回调不得把该引用带出调用期。

`HandleMap` 和 `MemAwareHandleMap` 的变更方法需要 `&mut self`，因此单实例不会在安全 Rust 中被无同步并发修改。若上层需要跨线程共享，必须自行放入 `Mutex`/`RwLock` 等同步容器；本文件没有规定锁粒度。`Box<dyn Handle>`/`Box<dyn Any>` 的 trait 边界没有声明 `Send + Sync`，所以这些容器也不自动具备跨线程传递承诺。

内存感知映射只返回写入增量，不拥有外部内存 tracker；调用方负责将增量计入合适的 tracker，也负责计算键或值所指向对象的额外内存。普通 `HandleMap::MemUsage` 是按需重算，未缓存计数，不存在计数同步或回滚生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/key.go`，测试对照为 `pkg/kv/key_test.go` 与 `pkg/kv/key_test.rs`。Rust 基本逐项保留 Go 的公开命名和分支：`Key` 操作、`KeyRange::IsPoint`、`Handle` 方法集、三种句柄、四路 `HandleMap`、不支持删除的 `MemAwareHandleMap`，以及 `Range` 的提前终止顺序均可一一对应。

语言表示上的差异包括：Go `Key` 是 `[]byte`，Rust 是拥有型 `Vec<u8>` 新类型；Go 接口值由运行时携带动态类型，Rust 通过 `Any::downcast_ref` 识别 `PartitionHandle`；Go `any` 值对应 Rust `Box<dyn Any>`，泛型值则直接存为 `V`；Go map key 将复合编码转为 `string`，Rust 直接使用 `Vec<u8>`；Rust 返回值常为深拷贝以满足所有权规则。

Go `KeyRange` 注释与测试要求其内存布局可通过 `unsafe` 与 kvproto `coprocessor.KeyRange` 互转；Rust `KeyRange` 没有 `repr(C)`，文档和实现只保留字段语义与顺序，不承诺跨语言或 kvproto ABI 兼容。Rust 测试 `test_key_range_definition` 只验证默认值和当前内存估算，没有复制 Go 的 unsafe 布局断言。

Rust `CommonHandle::Copy` 不需要处理 Go 的 nil receiver；Rust `Encoded`/`EncodedCol` 返回新 `Vec`，而 Go 返回底层 slice；Rust `NewCommonHandle` 返回结构体而非指针。Rust `IntHandle::Next`、偏移累加和字节进位显式采用 wrapping 操作，以复现 Go 整数溢出。当前 Rust 测试覆盖 Go 的主要行为用例，但没有移植 Go 的 `BenchmarkIsPoint`、`BenchmarkMemAwareHandleMap` 和 `BenchmarkNativeHandleMap` 基准。

## 扩展指南

新增键边界算法时应优先扩展 `impl Key`，并同步检查所有使用左闭右开范围的调用方；必须为尾部进位、空键、全 `0xff` 和部分前缀写入 `pkg/kv/key_test.rs` 的独立测试。若改变 `Next`/`PrefixNext`，风险会扩散到 session 扫描、structure 迭代、DDL 分区范围和 planner 估算，不能只验证本 crate。

新增 `Handle` 实现时，必须同时决定它属于整数还是编码字节路由，并审查 `HandleMap`/`MemAwareHandleMap` 是否能保存和在 `Range` 中恢复其动态类型。若不是 `PartitionHandle` 却需要额外命名空间，现有四路 map 不足以表达，不能仅实现 trait。还要定义与 `IntHandle`、`CommonHandle`、`PartitionHandle` 的 `Equal`/`Compare` 对称性、panic 边界、编码排序、`Copy` 深度及内存口径。

改变 `CommonHandle` 编码时，应从 `NewCommonHandle`、`EncodedCol`、`Data` 三处成组修改，并与 `codec::CutOne`/`DecodeOne` 的格式契约及 `pkg/tablecodec/tablecodec.rs` 的构造点联合验证。尤其要保护短编码补零但列偏移基于原输入这一不变量，以及复合编码位于整数句柄最小/最大编码之间的排序性质。

修改内存统计时要区分三个口径：Rust 结构体静态大小、map/键的估算大小、值指向对象的外部追踪；不得把 `HashMap` 实际容量或 `V` 的深层对象无依据地混入现有 Go 兼容公式。测试应继续放在独立的 `pkg/kv/key_test.rs`，不要内嵌回生产源文件；若要补性能证据，可另行对齐 Go 的三个 benchmark，但这不属于当前文档任务。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`node --file pkg/kv/key.rs --offset 1 --limit 400` 与 `--offset 400 --limit 400` 读取了 697 行完整实现，并报告该文件被 92 个文件使用。
- RustCodeGraph 精确查询：`query NewCommonHandle --kind function`、`query NewHandleMap --kind function`、`query NewMemAwareHandleMap --kind function`、`query NewPartitionHandle --kind function`，确认目标 Rust 定义及对应 Go 定义；对 `NewCommonHandle` 的 `callees` 未形成 codec 边，故下游关系以已索引源码体核实。
- Rust 源与模块边界：`pkg/kv/key.rs`、`pkg/kv/lib.rs`、`pkg/kv/Cargo.toml`。`pkg/kv` 不存在 `doc.go`，因此包契约以 `lib.rs` 顶部说明和模块接线为准。
- 直接调用证据：`pkg/tablecodec/tablecodec.rs`、`pkg/planner/util/handle_cols.rs`、`pkg/planner/core/operator/physicalop/physical_batch_point_get.rs`、`pkg/session/runtime/row_codec.rs`、`pkg/session/runtime/relational_scan.rs`、`pkg/structure/hash.rs`、`pkg/executor/index_merge_reader.rs`。
- Go 对照：`pkg/kv/key.go`；独立测试：`pkg/kv/key_test.rs` 与 `pkg/kv/key_test.go`。两者共同验证 PrefixNext/IsPoint 边界、句柄编码比较、短编码补零、映射覆盖与删除、分区隔离、Range 提前终止、内存估算及复合编码排序。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文恰有 11 个固定二级章节，并人工复核未把 Go 的 unsafe `KeyRange` 布局承诺误写为 Rust 现状。
