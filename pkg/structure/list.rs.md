# `pkg/structure/list.rs`

## 文件定位

提供列表长度、全量读取、正负逻辑下标读取或覆盖，以及整表清理。

## 核心职责

- 用 `listMeta { LIndex, RIndex }` 维护列表元素占用的半开整数区间 `[LIndex, RIndex)`；元数据和每个元素分别保存为独立 KV。
- 在同一套内部逻辑上提供左右两端推入和弹出：`LPush`/`LPop` 操作 `LIndex`，`RPush`/`RPop` 操作 `RIndex`。
- 提供列表长度、全量读取、正负逻辑下标读取或覆盖，以及整表清理。
- 复用 `TxStructure` 的 reader、可选 read-writer 和命名空间前缀；所有写入口在只读快照上拒绝执行。
- 解析并校验固定 16 字节的列表元数据，缺失元数据视为空列表。

本实现不是内存容器，也不在 `TxStructure` 内保存元素集合；它把列表状态映射到底层 `kv::Retriever`/`kv::RetrieverMutator`。键的具体布局来自 [`pkg/structure/type.rs`](type.rs) 的 `encodeListMetaKey` 与 `encodeListDataKey`。

## 主要符号

- `listMeta { LIndex: i64, RIndex: i64 }`：文件私有元数据类型。`Value(self)` 将两个下标分别按 `i64 -> u64 -> big-endian` 编成 16 字节；`IsEmpty(self)` 以 `LIndex >= RIndex` 判断空表或无效的反向区间。
- `TxStructure::LPush` / `TxStructure::RPush`：公开写入口，分别以 `left = true/false` 调用私有 `listPush`。Rust 接口用 `&[Vec<u8>]` 接收批量值，而 Go 原接口是 variadic `...[]byte`。
- `TxStructure::listPush`：逐值分配物理下标、写数据键，最后写元数据键。左推时先递减 `LIndex`，右推时先取当前 `RIndex` 再递增。
- `TxStructure::LPop` / `TxStructure::RPop`：公开写入口，分别委托 `listPop`；空列表返回 `Ok(None)`。
- `TxStructure::listPop`：确定端点下标，读取元素，删除数据键；弹出后若为空则删除元数据，否则覆盖更新后的元数据。
- `TxStructure::LLen`：返回 `RIndex.wrapping_sub(LIndex)`；不存在的列表由默认元数据得到 0。
- `TxStructure::LGetAll`：从 `RIndex - 1` 递减到 `LIndex` 读取，因此返回顺序是物理右端到左端。空列表返回 `Ok(None)`，不是空向量。
- `TxStructure::LIndex`：将逻辑下标经 `adjustIndex` 映射到物理区间；越界或空表返回 `Ok(None)`。
- `TxStructure::LSet`：按相同下标规则覆盖元素。空表静默返回 `Ok(())`，非空表越界返回 `ErrInvalidListIndex`。
- `TxStructure::LClear`：从 `LIndex` 到 `RIndex - 1` 逐键删除，再删除元数据键。
- `TxStructure::loadListMeta`：读取并反序列化元数据。`kv::ErrNotExist` 映射为 `listMeta::default()`，其他读错误向上传播，非 16 字节值返回 `ErrInvalidListMetaData`。
- `adjustIndex(index, minv, maxv)`：非负下标按 `minv + index` 定位，负下标按 `maxv + index` 定位。它在私有 `list_impl` 内声明为 `pub`，但 `lib.rs` 没有重新导出该函数；可确认的外部 API 是 `TxStructure` 上的方法。

## 执行流程

推入流程（`LPush`/`RPush` → `listPush`）：

1. 检查 `readWriter`；不存在时立即返回 `ErrWriteOnSnapshot`。该检查先于空 `values` 检查，所以只读快照上的空批量推入仍报错。
2. 可写且输入为空时直接成功，不创建元数据。
3. 由 `encodeListMetaKey(key)` 生成元数据键，`loadListMeta` 取得已有区间；键不存在时从 `[0, 0)` 开始。
4. 对每个输入值分配下标。左推连续递减 `LIndex`，因此后传入的值更靠左；右推连续递增 `RIndex`，保持输入在左到右方向上的顺序。
5. 通过 `encodeListDataKey(key, index)` 和 `writer()?.Set` 写每个元素，全部完成后将新的 16 字节元数据写回。

弹出流程（`LPop`/`RPop` → `listPop`）：

1. 先拒绝只读快照，再载入元数据；空表返回 `None`。
2. 左弹先取旧 `LIndex` 再加一，右弹先减 `RIndex` 再取新值。
3. 用 reader 读取目标元素，随后用 writer 删除对应数据键。
4. 若新区间为空则删除 meta，否则写回新 meta，最终返回读出的字节值。

读取与修改流程：`LLen` 只计算区间差；`LGetAll` 从右向左逐项 `kv::GetValue`；`LIndex`/`LSet` 先调用 `adjustIndex`，再做半开区间检查；`LClear` 顺序删除全部数据键并最后删除 meta。所有 KV 调用使用 `kv::Context::todo()`，本文件没有接收或传播调用方上下文。

## 数据与状态

列表由两类键构成，编码依据 [`pkg/structure/type.rs`](type.rs)：

- 元数据键：`prefix + EncodeBytes(business_key) + EncodeUint('L')`，值固定为 `LIndex || RIndex` 两个大端 64 位字。
- 元素键：`prefix + EncodeBytes(business_key) + EncodeUint('l') + EncodeInt(index)`，值为调用者提供的元素字节。

核心不变量是有效元素恰好对应 `[LIndex, RIndex)` 中的整数下标，长度为 `RIndex - LIndex`。一个新列表从 `[0, 0)` 开始；左侧增长使用负方向，右侧增长使用正方向。实现使用 `wrapping_add`/`wrapping_sub` 模拟 Go `int64` 的二进制回绕，而不是 Rust debug 模式的溢出 panic；这也意味着达到 `i64` 边界后不会主动报告容量溢出，区间语义可能因回绕失真。

操作顺序会影响可观察结果。例如依次 `LPush(["3", "2", "1"])` 后，物理下标从左到右是 `1, 2, 3`；`LGetAll` 按右到左读取而返回 `3, 2, 1`。再左推 `11` 后，`LPop` 首先返回 `11`。这些顺序由 Rust 测试 `list_push_pop_index_set_and_clear_match_go` 和 Go 测试 `TestList` 同时验证。

## 依赖与调用关系

上游关系：

- `lib.rs::list_impl` 负责装配，本文件依赖同 crate 根作用域中的 `TxStructure`、错误静态量和键编码方法。
- RustCodeGraph 的文件关系和符号探索显示，当前直接 Rust 调用证据集中在 `migration_aster_unit_test.rs::list_push_pop_index_set_and_clear_match_go`，另有 `structure_test.rs::TestListAndHashSnapshotWritesFail` 覆盖快照写失败。仓库搜索没有发现非测试 Rust 调用者。
- Go 对照测试 [`pkg/structure/structure_test.go`](structure_test.go) 的 `TestList` 通过真实 mock transaction 覆盖同一 API 族，但它验证的是 Go 实现而非 Rust 生产接线。

下游关系：

- `LPush`、`RPush` → `listPush` → `loadListMeta`、`encodeListMetaKey`、`encodeListDataKey`、`writer().Set`。
- `LPop`、`RPop` → `listPop` → `loadListMeta`、`kv::GetValue`、`writer().Delete/Set`。
- `LLen`、`LGetAll`、`LIndex`、`LSet`、`LClear` → `loadListMeta`；`LIndex`、`LSet` 还调用 `adjustIndex`。
- `loadListMeta` → `kv::GetValue`、`kv::IsErrNotFound`、`ErrInvalidListMetaData`。
- 错误定义和 `TxStructure::writer` 在 `structure.rs`，键编码及 `ListMeta`/`ListData` 标志在 `type.rs`；底层 trait 来自 Cargo 依赖 `astersql-kv`。

## 错误处理与边界

- 只读边界：`LPush`、`RPush`、`LPop`、`RPop`、`LSet`、`LClear` 都在执行前检查 `readWriter`，返回 `ErrWriteOnSnapshot`。`LLen`、`LGetAll`、`LIndex` 仅需 reader。
- 元数据错误：元数据存在但长度不是 16 时，`loadListMeta` 返回 `ErrInvalidListMetaData`；读取元素缺失或底层读写失败时原样传播 `errors::SharedError`。
- 空表：`LLen` 为 0；`LGetAll`、`LIndex`、弹出返回 `None`；`LSet` 和 `LClear` 静默成功。这里 `None` 对齐 Go 的 `nil`，不是“存在但内容为空”的集合。
- 下标：正数从左端计数，`0` 是 `LIndex`；负数从右端计数，`-1` 是 `RIndex - 1`。读越界返回 `None`，写越界报 `ErrInvalidListIndex`。
- 空值：本文件不预检元素值；最终行为由底层 `Mutator::Set` 契约决定。测试后端 `MemoryStore` 拒绝空值并返回 `kv::ErrCannotSetNilValue`，因此扩展测试时不能假设空字节总能落盘。
- 部分写：批量推入先逐个写数据、最后写 meta；弹出先删数据、再更新或删除 meta；清理逐项删除、最后删 meta。任一步错误都会立即返回，本文件没有补偿回滚。正确的原子提交依赖调用方所提供的事务型 `RetrieverMutator` 及其提交策略；若使用立即生效且无回滚的实现，错误可能留下孤立数据键或陈旧 meta。
- 数据损坏边界：`LGetAll` 和弹出假定 meta 覆盖的每个数据键存在；缺键会返回底层错误，而不是跳过。`IsEmpty` 将 `LIndex > RIndex` 也视为空，但不会主动报告这一元数据关系异常。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或迭代器，也不持有跨调用资源。写方法要求 `&mut self`，能在 Rust 借用层面串行化通过同一个 `TxStructure` 实例发起的修改；读方法使用 `&self`。这不等于提供跨实例并发控制：多个 `TxStructure` 若指向同一底层存储，冲突检测、隔离级别与提交原子性均由 `astersql-kv` 的具体 reader/writer 实现负责。

列表生命周期是：首次非空推入创建数据键和 meta；中间操作维护区间；最后一个元素弹出或 `LClear` 删除 meta。空批量推入、读取不存在的列表、清理不存在的列表都不会创建状态。测试用的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 以 `Arc<Mutex<BTreeMap<...>>>` 共享 reader/writer 后端；这个锁属于测试夹具，并非本文件的生产并发机制。

## 与 Go 版本的对应关系

Rust 保留了 Go 的数据模型、键编码调用、端点更新次序、从右到左的 `LGetAll`、正负下标转换、空表行为和四类错误路径。

- Go 的 `[]byte`/`[][]byte` 映射为 Rust 的 `&[u8]`、`Vec<u8>` 和 `Vec<Vec<u8>>`；Go `nil, nil` 映射为 `Ok(None)`。
- Go variadic 推入参数映射为 Rust slice；错误包装 `errors.Trace` 映射为 `?` 传播共享错误。
- Rust 显式使用 `wrapping_add/sub` 保持 Go `int64` 算术回绕语义。
- Go 的列表实现由包直接编译；Rust 版本由 `lib.rs` 的 `include!` 装配，公开的是 `TxStructure` 固有方法，而不是 `listMeta` 或内部辅助函数。

当前独立 Rust 测试未直接构造畸形 16 字节以外的 meta，也未覆盖 `i64` 回绕边界或中途写失败后的状态。

## 扩展指南

- 新增端点操作（例如批量弹出或区间读取）时，优先复用 `loadListMeta`、`adjustIndex` 和 `type.rs` 的键编码器，并保持 `[LIndex, RIndex)`、空表删除 meta、`LGetAll` 既有方向等兼容约定。
- 修改元数据布局时必须同时评估 Go `listMeta.Value/loadListMeta`、现存 KV 数据兼容性和 `ErrInvalidListMetaData`；不能只改 Rust 的编码端。若需要版本化，应先设计可区分旧 16 字节格式的迁移方案。
- 新增写路径必须在任何读取或变更前执行只读快照检查，并考虑多步写失败时的事务原子性；不要把测试 `MemoryStore` 的立即写语义误当成生产事务保证。
- 若改变批量值接口或顺序，须同步 `migration_aster_unit_test.rs::list_push_pop_index_set_and_clear_match_go` 和 Go `structure_test.go::TestList` 的对照断言。边界增强宜放在同目录独立测试文件中，不能把测试嵌入 `list.rs`。
- 推荐补充的独立测试包括：空批量推入在可写/快照上的差异、畸形 meta、数据键缺失、空值底层拒绝、`i64::MIN/MAX` 回绕，以及可注入读写失败时是否留下部分状态。
- 若把列表 API 接入 Rust 生产调用链，应新增调用方层面的事务提交/回滚测试；目前只能确认 crate 实现和单元测试，未检出非测试 Rust 调用者。

兼容风险主要是持久化键格式、返回顺序、空表 `None` 语义及错误码；性能风险主要来自 `LGetAll` 的逐元素读取和 `LClear` 的逐元素删除，二者均为 O(n) 次 KV 操作且不批处理。
