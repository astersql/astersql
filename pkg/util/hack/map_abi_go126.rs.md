# `pkg/util/hack/map_abi_go126.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-hack`（`pkg/util/hack/Cargo.toml` 的 `[lib]` 指向 `lib.rs`），是 Go 1.26 Swiss map 的 Rust 对照实现。`pkg/util/hack/lib.rs` 通过 `pub mod map_abi_go126` 声明模块，并用 `pub use map_abi_go126::*` 将该版本作为 crate 根的当前公开 API；Go 1.25 兼容实现仍保留在独立的 `map_abi` 模块中。

它同时承担两类职责，但两类对象不能混用：`mapTable`、`mapData`、`abiType`、`mapType`、`SwissMapWrap` 镜像 Go runtime ABI，供布局核验或由 Go/FFI 提供真实裸指针时读取；业务可运行的 `MemAwareMap<K, V>` 则持有 `crate::swiss_map::SwissMap<K, V>`，不把 Rust `HashMap` 内存冒充为 Go runtime map。`pkg/util/hack/hack.rs::init` 调用本文件的 `checkMapABI`，但当前 Rust 函数为空，不会查询 Go runtime 版本。

## 核心职责

1. 用 `#[repr(C)]` 类型保存 Go 1.26 `internal/runtime/maps` 与 `internal/abi` 的关键字段顺序，支持目录、table、group、key/elem 地址和容量/字节数公式的复核。
2. 提供 `MemAwareMap<K, V>` 的创建、读写、存在性检查、元素计数与内存记账 API。真实存储、扩容、split、tombstone 和 clear 行为由 `swiss_map.rs::SwissMap` 实现。
3. 在写入数量到达 checkpoint 时，用 `approxSize` 更新单调不减的 `Bytes`，避免每次写入都执行较昂贵的 `RealBytes` 容量遍历。
4. 保留 Go unsafe API 的形状并明确失败边界：普通 Rust `HashMap` 不能经 `ToSwissMap` 转成 Go ABI 视图，`MemAwareMap::unwrap` 也不能产生 Go map header；二者当前都会 panic。

## 主要符号

- 常量 `maxTableCapacity = 1024`、`mapGroupSlotsBits = 3`、`mapGroupSlots = 8`：分别控制 table split/checkpoint 上限和每 group 的固定 slot 数；`mapSize`、`mapTableSize`、`sizeofPtr` 从 Rust ABI 镜像类型计算字节数。
- `mapTable` 与 `groupsReference`：描述 table 的使用量、容量、剩余增长空间、局部深度、目录索引，以及连续 group 区域的地址和 `lengthMask`。
- `mapData`：描述 Go map header。`directoryAt` 按指针宽度读取第 `i` 个 table 指针；`Size` 计算 header、目录、唯一 table 和 groups 的字节数；`Cap` 累加唯一 table 的容量；`MockSeedForTest` 仅允许空 map 改 seed。
- `abiType` 与 `mapType`：镜像 Go 类型元数据和 map 专属的 key/elem/group 类型、hasher、`GroupSize`、`SlotSize`、`ElemOff`、flags。
- `groupReference` 与 `groupsReference::group`：执行 group、key、elem 的裸指针偏移；`groupCap(groupSize, slotSize)` 使用 `(groupSize - groupSlotsOffset) / slotSize` 计算 slot 数。
- `SwissMapWrap::{from_raw_parts, Size}`：前者是显式 FFI 裸指针构造入口并拒绝空指针，后者通过 `mapData::Size` 读取 Go ABI 数据。调用方必须履行其 `unsafe` 前置条件。
- `ToSwissMap<K, V>`：保留 Go API 名称，但对 Rust `HashMap` 无合法转换，必定 panic，并提示改用 `SwissMapWrap::from_raw_parts`。
- `MemAwareMap<K, V>`：字段 `M` 保存安全的 `SwissMap`，`groupSize` 保存 Go 布局公式得到的 group 字节数，`nextCheckpoint` 控制估算更新频率，`Bytes` 保存累计近似内存。
- `MemAwareMap::{Init, Set, SetExt, Get, Exist, Count, Empty, Len, RealBytes, MockSeedForTest}`：构成业务 API；`NewMemAwareMap` 创建并初始化实例。
- `approxSize`：按 `groupSize * maxLen * 204 / 1000` 估算内存；`checkMapABI` 是当前无运行时检查逻辑的版本升级提醒占位。

## 执行流程

创建路径从 `NewMemAwareMap(capacity)` 开始：先构造空 `SwissMap` 和零值记账字段，再调用 `Init(SwissMap::with_capacity(capacity))`。`Init` 通过 `swiss_map.rs::group_size::<K, V>()` 计算 Go 风格 group 布局，通过 `SwissMap::size(mapSize, mapTableSize, sizeofPtr, groupSize)` 得到初始 `Bytes`；元素数不超过 8 时把首个 checkpoint 设为 16，否则设为 `used + min(used, 1024)`。

写入路径由 `Set` 调用 `SwissMap::insert`。插入或覆盖完成后读取当前长度；仅当 `used >= nextCheckpoint` 时计算 `max(Bytes, approxSize(groupSize, used))`，把差值作为 `i64` 返回，再把下一 checkpoint 推进 `min(used, 1024) + used`。因此 1024 个元素以内 checkpoint 随当前规模近似翻倍，之后每 1024 个元素更新一次。`SetExt` 先用 `contains_key` 判断 key 是否已经存在，再调用 `Set`，返回 `(内存增量, 是否新插入)`。

读取路径不触发记账：`Get` 返回 `(Option<&V>, bool)`，`Exist`、`Count`、`Empty`、`Len` 分别转发到 `SwissMap`。`RealBytes` 调用 `real_bytes_for_capacity`，按 `SwissMap` 当前保留的 table/directory 分配历史计算 Go ABI 口径容量，因此 clear 后元素数可归零但容量字节数不必下降。

独立 ABI 读取路径从 `SwissMapWrap::from_raw_parts` 开始。`mapData::Size` 对 small map（`dirLen == 0`）只计一个 group；普通 map 遍历目录，跳过连续重复的 table 指针，避免同一 split table 重复计数。`Cap` 使用同样的去重规则累加 capacity。该路径不由 `MemAwareMap` 的安全存储路径调用。

## 数据与状态

`MemAwareMap` 的核心状态不变量是：`M.len()` 是实际元素数；`Bytes` 是 checkpoint 驱动的近似值且通过 `max` 保证不下降；`RealBytes()` 是按已分配 capacity 计算的实时值，两者不承诺相等。覆盖已有 key 不增加长度，`SetExt` 应返回 `insert = false`；clear 后 `SwissMap` 保留分配、递增 `clear_seq` 并更换 seed，所以 `RealBytes` 可保持不变。

ABI 镜像中，`mapData::Used` 必须位于首字段；`dirLen == 0` 表示 small-map 单 group 优化，否则 `dirPtr` 指向 table 指针目录。目录相邻项可以引用同一 table，故 `Size`/`Cap` 用 `lastTab` 去重。`groupsReference::lengthMask + 1` 是 group 数，并假设其为 2 的幂。group 首部是 `ctrlGroup`，slots 从 `groupSlotsOffset = size_of::<u64>()` 开始，每个 slot 的 elem 地址为 key slot 地址再加 `ElemOff`。

类型布局按当前目标平台的 `usize`/指针宽度计算。`map_abi_go126_test.rs` 的断言明确覆盖 64 位布局，例如 `mapData` 48 字节、`mapTable` 32 字节、`mapType` 112 字节；这些数值不是跨架构保证。

## 依赖与调用关系

下游主要依赖是 `crate::swiss_map::SwissMap`：`MemAwareMap` 的 `insert/get/contains_key/len/is_empty/set_seed/size` 均由它提供，`Init` 还调用 `swiss_map::group_size`。其余依赖仅来自标准库：`HashMap` 只出现在禁止重解释的 `ToSwissMap` 参数中，`c_void` 表示 ABI 裸指针，`Hash`/`Eq` 约束 key。

模块入口 `pkg/util/hack/lib.rs` 将本模块 API 再导出到 crate 根；`pkg/util/hack/hack.rs::init -> map_abi_go126::checkMapABI` 是初始化调用边。RustCodeGraph 将 `pkg/util/hack/map_abi_test.rs` 和 `pkg/util/hack/migration_aster_unit_test.rs` 标为本文件使用方：前者引用 Go 1.26 常量和布局类型，后者直接验证 Go 1.26 `MemAwareMap`、合成 ABI `Cap/Size` 以及 crate 根再导出。

仓库当前未搜索到 crate 外生产 Rust 代码通过 `astersql_util_hack::NewMemAwareMap` 或 `astersql_util_hack::MemAwareMap` 直接使用本模块的 crate 根再导出；直接行为使用者集中在本 crate 测试。相反，`pkg/util/set/lib.rs`、`pkg/kv/lib.rs`、`pkg/tablecodec/lib.rs` 等入口明确再导出 `hack_crate::map_abi` 或 `hack_dependency::map_abi`，即 Go 1.25 兼容模块，不能据此视为本文件的直接调用者。因此本文件在完整应用中的已接线位置是 crate 根 API 和 `hack.rs::init`，而非已经覆盖所有上层业务 map。

## 错误处理与边界

本文件不返回 `Result`。输入或 ABI 前置条件错误通过 panic 或 unsafe 未定义行为边界表达：`SwissMapWrap::from_raw_parts` 对任一空指针执行 `assert!`；`ToSwissMap` 和私有 `MemAwareMap::unwrap` 无条件 panic；空 map 以外调用 `MockSeedForTest` 会由 `SwissMap::set_seed`（或 ABI 镜像的同名方法）panic。

所有 `mapData`、`mapTable`、`groupsReference`、`groupReference` 的裸指针读取都要求地址有效、对齐、仍存活且确实符合 Go 1.26 对应布局。`directoryAt` 不检查 `i < dirLen`，`group/key/elem` 不检查索引范围，`Size`/`Cap` 也假设目录和 table 指针非空且有效。`groupCap` 假设 `groupSize >= groupSlotsOffset` 且 `slotSize != 0`，否则可能下溢或除零。

Go 的 `Init(nil)` 会 panic；Rust 的 `Init` 接受 `Into<SwissMap<K, V>>`，类型系统和转换实现不会产生 nil map。另一方面，Go `checkMapABI` 会检查 `runtime.Version()` 包含 `go1.26`，Rust `checkMapABI` 当前为空，因此版本安全只能靠布局测试和人工复核，不能把它描述为已实现的运行时防线。

`approxSize`、checkpoint 加法以及从 `u64` 到 `i64` 的 delta 转换没有显式溢出处理；常规 map 规模下测试覆盖了预期值，但极端容量仍需单独评估。`ToSwissMap`、`unwrap`、`checkMapABI` 是明确的未接线/占位接口，不应在新增业务路径中调用。

## 并发与资源生命周期

`MemAwareMap` 的修改 API 需要 `&mut self`，本文件不提供锁、原子变量、线程或通道；跨线程共享时必须由上层加锁。ABI 镜像中的 `mapData::writing` 只是 Go runtime 字段镜像，本文件不会用它协调 Rust 并发，也不会让裸指针读取变得线程安全。对正在被 Go runtime 修改、扩容或释放的 map 执行 `SwissMapWrap` 读取会破坏安全前置条件。

安全路径的所有权由 `SwissMap` 持有：table、directory、key/value 随 `MemAwareMap` 生命周期释放。clear 清空 slot 内容但保留 table/directory 分配供复用；`Bytes` 也不会因 clear 自动减少。FFI 路径的 `SwissMapWrap` 仅保存裸指针，不拥有、不延长也不释放底层 Go 对象；调用方必须保证 `Type` 和 `Data` 在整个读取期间匹配且存活。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/hack/map_abi_go126.go`，其 build tag 为 `go1.26 && !go1.27`。Rust 保留了常量、ABI 字段、`Size`/`Cap`、group 地址公式、近似比例、checkpoint 算法以及 `MemAwareMap` API 名称。Go 1.26 将早期变体的 table/Map 命名对应为 `mapTable`/`mapData`；本文件据此与 `pkg/util/hack/map_abi.rs` 的 Go 1.25 变体并存。

关键迁移差异是存储实现：Go `ToSwissMap` 和 `unwrap` 可以借助 `unsafe.Pointer` 直接解释真实 Go map；Rust 标准 `HashMap` 布局不兼容，因此这两个入口被改为明确 panic，日常 `MemAwareMap` 改用安全的 `SwissMap` 模拟 Go Swiss-table 的分配历史并按 Go ABI 口径计费。Go `NewMemAwareMap` 返回指针，当前 Go 1.26 Rust 变体返回值类型 `MemAwareMap`；调用方无需空值检查。Go 的 `SetExt` 通过 `Used` 变化判断插入，Rust 在写入前用 `contains_key` 得出等价业务语义。

Go 测试 `pkg/util/hack/map_abi_test.go` 验证真实 runtime 的目录扫描、seed、精确字节值、clear 与 benchmark；Rust 将可移植部分拆到 `map_abi_go126_test.rs`、`map_abi_test.rs`、`migration_aster_unit_test.rs`，其中安全存储的主要 workload 仍通过共享/Go 1.25 API 测试覆盖，并额外对 Go 1.26 镜像做字段偏移与合成目录验证。

## 扩展指南

- Go 版本或 runtime 字段变化时，先同步 `mapTable`、`mapData`、`abiType`、`mapType` 及常量，再更新独立的 `map_abi_go126_test.rs` 字段 offset/size 断言和 `map_abi_test_type_go126_test.rs` 别名；不得只改 Rust 注释或凭旧布局继续读取裸指针。
- 修改容量、split、group 布局或内存公式时，必须同时核对 `swiss_map.rs::{with_capacity, grow, size, group_size}` 与本文件的 `approxSize`、`Init`、`Set`、`groupCap`，并同步 `map_abi_test.rs` 和 `migration_aster_unit_test.rs` 的精确字节/checkpoint/clear 回归。
- 新增 `MemAwareMap` 行为优先接在 `SwissMap` 的安全 API 上；不要调用 `unwrap` 或尝试实现对 Rust `HashMap` 的内存重解释。若确需读取 Go runtime 对象，应只在明确 FFI 边界使用 `SwissMapWrap::from_raw_parts`，为所有权、线程隔离、版本和生命周期建立外部保证并增加独立测试文件。
- 若要让 `checkMapABI` 成为真实防线，需要先定义可获得 Go runtime 版本的 FFI/构建契约；在此之前应保留“占位”描述，不能声称运行时版本已校验。
- 测试逻辑须继续放在独立文件，不嵌入本生产文件。至少同步 `map_abi_go126_test.rs`（ABI 布局）、`map_abi_test.rs`（行为/workload）和 `migration_aster_unit_test.rs`（Go 1.26 直接 API、合成布局与 crate 根导出）。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/hack` 找到目标、Go 对照、模块入口与测试；`node --file pkg/util/hack/map_abi_go126.rs` 读取 562 行完整源码，并报告直接使用方 `map_abi_test.rs`、`migration_aster_unit_test.rs`；`query MemAwareMap`/`query NewMemAwareMap` 确认 Go 1.25、Go 1.26 同名变体，故调用关系按模块路径消歧。
- 源码与模块边界：`pkg/util/hack/map_abi_go126.rs`、`pkg/util/hack/swiss_map.rs`、`pkg/util/hack/lib.rs`、`pkg/util/hack/hack.rs`、`pkg/util/hack/Cargo.toml`。
- Go 对照：`pkg/util/hack/map_abi_go126.go`；真实 runtime 行为测试：`pkg/util/hack/map_abi_test.go`。
- Rust 独立测试：`pkg/util/hack/map_abi_go126_test.rs`（64 位字段大小/偏移）、`pkg/util/hack/map_abi_test.rs`（布局公式、增长、clear、checkpoint、精确字节与 workload）、`pkg/util/hack/migration_aster_unit_test.rs`（Go 1.26 CRUD、合成 `Cap/Size`、第九项扩容、crate 根导出和混合对齐）。
- 仓库引用搜索：`rg` 确认 `hack.rs::init` 调用 `checkMapABI`，crate 根重导出 Go 1.26 API，并区分业务侧对 crate 根当前版本和 `map_abi` 兼容版本的引用。任务为纯文档分析，按计划不运行 Cargo。
