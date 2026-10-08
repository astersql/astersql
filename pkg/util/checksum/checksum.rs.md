# `pkg/util/checksum/checksum.rs`

## 文件定位

本文件是 `astersql-util-checksum` crate 的核心实现，由同目录 `lib.rs` 声明为 `checksum` 模块并整体再导出。crate 在 `pkg/util/checksum/Cargo.toml` 中仅以 `crc32fast = "1"` 作为运行时外部依赖，同时由根 `Cargo.toml` 的 `facade_util_checksum` 和 `pkg/lib.rs` 的 `pkg::util::checksum` 门面暴露给工作区其他模块。

它位于临时落盘 I/O 的数据完整性层：`pkg/util/chunk/chunk_util.rs` 的 `diskFileReaderWriter::initWithFileName` 用 `checksum::NewWriter` 包装临时文件（可先叠加 CTR 加密），`getReader` 则用 `checksum::NewReader` 和 `NewReaderWithCache` 拼接已刷盘前缀与 Writer 尚未刷出的尾部缓存。更底层的适配接线位于 `pkg/util/chunk/internal/group1/lib.rs`：该文件把项目的 `io::WriteCloser`/`io::ReaderAt` 转换为这里的草稿 trait，再将字符串错误映射回项目错误类型。

## 核心职责

- `Writer<W>` 将用户字节切成最多 1020 字节的载荷块；每块前置 4 字节小端 IEEE CRC-32，形成最多 1024 字节的编码块后写给底层 `W`。
- `Reader<R>` 把用户逻辑偏移换算为编码流的块偏移，读取并校验每个涉及的完整编码块，只把载荷字节复制给调用者。
- `Writer::GetCache` 与 `Writer::GetCacheDataOffset` 暴露未刷盘尾部及其用户逻辑起点，供 `pkg/util/chunk/chunk_util.rs::getReader` 在文件仍打开写入时构造一致的读取视图。
- `ReaderBuffer` 和 `checksumReaderBufPool` 复用 1024 字节读缓冲；`Writer` 则持有自己的编码缓冲和载荷缓冲。

该层只做块完整性检测，不提供保密性、认证加密、恢复或纠错；加密由相邻的 `pkg/util/encrypt` 层承担。

## 主要符号

- `GoError = String`：模拟 Go `error` 的当前迁移边界；错误分类依赖字符串，而不是 Rust 错误枚举。
- `WriteCloserDraft::{Write, Close}` 与 `ReaderAtDraft::ReadAt`：分别模拟 Go `io.WriteCloser` 和 `io.ReaderAt`。对 `&mut T`、`&T` 的转发实现允许包装器嵌套。
- `checksumBlockSize = 1024`、`checksumSize = 4`、`checksumPayloadSize = 1020`：定义磁盘格式。前 4 字节是小端 `u32` CRC，后面是实际载荷；末块可以短于 1024 字节。
- `Writer<W>`：保存底层 writer、持久错误 `err`、编码缓冲 `buf`、独立载荷缓冲 `payload`、已用长度 `payloadUsed` 和已成功刷出的用户字节数 `flushedUserDataCnt`。
- `NewWriter`：分配固定大小缓冲并建立初始状态。Rust 版的 `payload` 是独立 `Vec<u8>`；Go 版让 payload 直接引用 `buf[4:]`，但可观察编码结果相同。
- `Writer::{AvailableSize, Write, Buffered, Flush, GetCache, GetCacheDataOffset, Close}`：组成写入、显式刷出、缓存查询和关闭 API；`Writer` 自身也实现 `WriteCloserDraft`，因而可以像测试中那样多层嵌套。
- `Reader<R>`、`NewReader`、`Reader::ReadAt`：组成校验读取 API；`Reader` 自身实现 `ReaderAtDraft`，也支持多层嵌套。
- `errChecksumFail = "error checksum"`：块头过短或 CRC 不匹配时的兼容错误文案。
- `ReaderBuffer::{acquire, bytes}` 与 `Drop`：从全局池取得缓冲并在作用域结束时归还；互斥锁中毒时通过 `into_inner` 继续使用池。
- `copy_to`：以源、目标较短者为上限复制字节，是写缓冲填充和读结果提取的共同辅助函数。

## 执行流程

写入流程以 `Writer::Write` 为入口：

1. 只要输入长度严格大于当前剩余空间，先用 `copy_to` 填满 `payload`，然后调用 `Flush`。等于剩余空间时只填满而不立即刷出，后续 `Flush`/`Close` 或下一次非空写入才会落盘。
2. `Flush` 对空载荷直接成功；否则用 `crc32fast::hash` 计算载荷的 IEEE CRC-32，将 `to_le_bytes()` 结果写入 `buf[0..4]`，再复制载荷并一次调用底层 `Write`。
3. 底层成功后，`flushedUserDataCnt` 只累计用户载荷字节，不计 4 字节块头，并把 `payloadUsed` 清零。发生错误或按 Go 规则判定 short write 时，错误被写入 `err` 并永久锁存。
4. `Close` 先刷出末块；刷出失败时不调用底层 `Close`，否则把底层关闭结果直接返回。

读取流程以 `Reader::ReadAt(p, off)` 为入口：

1. 空目标切片立即返回 `(0, None)`，不访问底层。
2. 用 `off % 1020` 得到首块载荷内偏移，用 `off / 1020 * 1024` 得到编码流游标，并从池中取得一个 1024 字节缓冲。
3. 每轮调用底层 `ReadAt(buffer, cursor)`。普通错误或零字节 EOF 立即返回；“读到部分数据并同时返回 `EOF`”则仍把该数据视为可能的合法末块并继续校验。
4. 少于 4 字节的块返回 `errChecksumFail`；否则将前 4 字节按小端解析为期望 CRC，并对 `[4..n]` 载荷重新哈希。不匹配时停止并返回此前已复制的用户字节数。
5. 首轮从 `4 + offsetInPayload` 开始复制，后续轮从块载荷起点复制；游标按底层实际返回的编码字节数推进。目标填满后返回 `(nn, None)`；若请求越过文件尾，下一轮底层 EOF 会连同已复制长度返回。

## 数据与状态

编码格式是不带全局头的块序列：`[crc32_le | payload]...`。完整块固定为 1024 字节，末块为 `4 + 剩余载荷长度`；因此用户偏移 `u` 所在编码块从 `(u / 1020) * 1024` 开始，块内载荷偏移是 `u % 1020`。CRC 只覆盖载荷，不覆盖校验字段自身。

`Writer` 的关键不变量是 `payloadUsed <= checksumPayloadSize`，且 `GetCache()` 恰为 `payload[..payloadUsed]`；`flushedUserDataCnt` 仅在整个待写块被本实现视为成功后增加。`err` 一旦为 `Some` 就不会清除，后续 `Write`/`Flush`/`Close` 返回同一字符串错误，避免在已不确定的编码流后继续追加。

`Reader` 本身只保存底层 `R`，没有可变游标，所以每次 `ReadAt` 的位置完全由 `off` 决定。全局池保存可复用的 `Vec<u8>`；缓冲内容不会在归还时清零，但每次参与校验和复制的范围只到本轮底层返回的 `n`。

## 依赖与调用关系

上游生产调用链以临时 spill 文件为主：

- `pkg/util/chunk/chunk_util.rs::diskFileReaderWriter::initWithFileName` → `checksum::NewWriter` → `Writer::Write`/`Close`。
- `pkg/util/chunk/chunk_util.rs::diskFileReaderWriter::getReader` → `checksum::NewReader` → `Reader::ReadAt`，并用对应 Writer 的 `GetCache`、`GetCacheDataOffset` 补齐未刷盘尾部。
- `pkg/util/chunk/internal/group1/lib.rs::checksum::{NewWriter, NewReader}` → 本 crate 的 `NewWriter`/`NewReader`；`ChecksumWriteAdapter`、`ChecksumReadAdapter` 负责 trait 与错误边界转换。

下游依赖很窄：CRC 计算仅调用 `crc32fast::hash`，字节序使用标准库 `u32::{to_le_bytes, from_le_bytes}`，池同步使用 `std::sync::Mutex`。`pkg/util/encrypt/aes_layer_test.rs::test_read_at` 还验证 checksum 与 CTR 层以两种顺序组合时均可正确读取，说明本层依赖的是随机访问/关闭接口契约，而不是具体文件类型。

RustCodeGraph 的文件查询显示本文件有 29 个符号；精确符号查询同时定位到 `pkg/util/checksum/checksum.rs::{NewWriter, NewReader}` 和对应 Go 符号。调用图命令未在限定时间返回完整 callers，因此上述生产调用边由局部文本检索和调用点源码复核补足，而非把未返回的图结果当作事实。

## 错误处理与边界

- 底层 `Write` 的显式错误直接锁存在 `Writer::err`。若底层未报错但返回长度小于 `payloadUsed`，则合成 `"short write"`；这个阈值有意与 Go 当前实现一致，只比较用户载荷长度，而不是实际传入底层的 `payloadUsed + 4`。
- `Close` 只有在 `Flush` 成功后才触达底层，因此刷出失败优先于关闭错误；底层关闭错误不被锁存到 `Writer::err`。
- `Reader::ReadAt` 原样传播非 EOF 读取错误和零字节 EOF；块不足 4 字节、CRC 不匹配统一返回 `errChecksumFail`。成功校验的早先块已经复制后，后续失败会返回部分计数 `nn`。
- API 没有显式拒绝负偏移；其余运算和切片逻辑以非负偏移为前提，负值可能在转换/切片时失败。调用者应遵守 Go `ReaderAt` 的有效偏移契约，不应传负数。
- 本实现信任底层 `ReaderAtDraft` 返回的 `n` 不超过目标缓冲长度，并假设编码流除末块外按 1024 字节边界保存；违反接口契约可能导致切片越界或错误的块边界推进。
- 末块只要至少包含 4 字节且 CRC 正确就可被接受；文件格式不单独记录总用户长度。插入或删除字节通常会破坏当前及后续块边界，修改载荷只破坏所在块，这些行为由独立测试覆盖。

## 并发与资源生命周期

`Reader::ReadAt` 只借用 `&self`，局部游标和缓冲互不共享；当底层 `R` 自身支持并发随机读时，同一 Reader 可按 Rust 类型约束并发使用。缓冲池由全局 `Mutex<Vec<Vec<u8>>>` 保护，每个调用持有独占租约，但锁只在取出和 `Drop` 归还时短暂持有，不覆盖底层 I/O 或 CRC 计算。即使发生提前返回，RAII 也会归还缓冲；锁中毒不会永久禁用池。

`Writer` 的修改操作需要 `&mut self`，本文件不提供内部并发写保护。生产适配层 `pkg/util/chunk/internal/group1/lib.rs::checksum::Writer` 额外使用 `Arc<Mutex<_>>` 提供可克隆的同步门面，这不是本文件 `Writer` 自身的保证。

Writer 拥有底层 `W`，但没有 `Drop` 自动关闭语义；调用者必须显式调用 `Close`，否则尾部 `payload` 不会写入底层。Reader 不拥有需要显式释放的本地资源，临时缓冲在每次调用结束时归池，底层对象的生命周期由泛型 `R` 的所有权和实现决定。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/checksum/checksum.go`，测试对照为 `checksum_test.go` 与 Rust 的 `checksum_test.rs`、`migration_aster_unit_test.rs`。常量、块布局、IEEE CRC-32、小端字段、写满/刷出循环、错误锁存、缓存偏移、部分 EOF 校验和读偏移换算均按 Go 控制流保留；`errChecksumFail` 和 `"short write"` 文案也保持兼容。

主要语言差异是：Go 直接使用 `io.WriteCloser`/`io.ReaderAt` 和 `error`，Rust 暂以 `WriteCloserDraft`/`ReaderAtDraft` 及 `Option<String>` 表示；Go 使用 `zeropool.New` 管理读缓冲，Rust 使用 `Mutex<Vec<Vec<u8>>>` 与 `ReaderBuffer::Drop`；Go Writer 的 payload 借用编码缓冲尾部，Rust 为避免自引用结构而使用两个独立 `Vec<u8>`，在 Flush 时复制。Go 构造函数返回指针，Rust 返回拥有泛型底层对象的值。

`checksum_test.rs` 对齐 Go 的嵌套层、插入/删除/修改损坏、空文件、跨块读写、缓存偏移和加密组合；`migration_aster_unit_test.rs` 额外明确验证小端 CRC 磁盘布局、尾部 EOF、short write 锁存及空切片不触达底层。生产接线 `pkg/util/chunk/chunk_util.rs` 与 Go 同路径 `chunk_util.go` 的包装顺序和缓存桥接一致。

## 扩展指南

- 修改块大小、校验字段或算法属于磁盘格式变更，应同时修改三个常量、`Writer::Flush`、`Reader::ReadAt` 的偏移换算，并评估旧 spill 文件兼容性；至少扩展 `migration_aster_unit_test.rs` 的字节级布局断言和 `checksum_test.rs` 的跨块/损坏用例。
- 增加结构化错误时，优先在 `GoError`、两个 Draft trait 与 `pkg/util/chunk/internal/group1/lib.rs` 的适配器处统一设计，避免只改错误字符串而破坏 Go 兼容断言和上层映射。
- 改变 Writer 缓冲策略时，必须保持 `GetCache`/`GetCacheDataOffset` 的组合语义；`chunk_util.rs::getReader` 依赖它们在未 Close 状态下读取逻辑连续的数据。
- 优化池或并发行为时，应保留提前返回也归还缓冲、锁中毒恢复和每次调用独占缓冲的性质；新增并发测试应放在独立测试文件中，不应内嵌到 `checksum.rs`。
- 若要支持负偏移、异常底层返回长度或更严格 short write 判定，应先确定是否继续逐字对齐 Go；这些改变可能是行为兼容变更，需在 Rust 独立测试和 Go 对照测试中明确期望，而不能仅靠编译通过。
- 新增正常/异常读写行为优先扩展同目录 `checksum_test.rs` 或 `migration_aster_unit_test.rs`；涉及与加密层组合时同步检查 `pkg/util/encrypt/aes_layer_test.rs`，涉及 spill 在线读取时检查 `pkg/util/chunk` 的独立测试。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter pkg/util/checksum` 找到目标、Go 对照、模块入口和三份相关测试；`node --file pkg/util/checksum/checksum.rs --offset 1 --limit 400` 读取了目标全部 296 行；`query NewWriter`、`query NewReader` 精确确认 Rust/Go 同路径符号。callers/callees 查询在 30 秒窗口内未返回内容，生产调用边改由下列源码检索验证。
- 核心源码与 crate 边界：`pkg/util/checksum/checksum.rs`、`pkg/util/checksum/lib.rs`、`pkg/util/checksum/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- 生产调用与适配：`pkg/util/chunk/chunk_util.rs`、`pkg/util/chunk/internal/group1/lib.rs`；对应 Go 调用为 `pkg/util/chunk/chunk_util.go`。
- Go 语义：`pkg/util/checksum/checksum.go`、`pkg/util/checksum/checksum_test.go`。
- Rust 独立测试：`pkg/util/checksum/checksum_test.rs`、`pkg/util/checksum/migration_aster_unit_test.rs`；组合层证据为 `pkg/util/encrypt/aes_layer_test.rs::test_read_at`。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前只执行任务指定的 11 章节结构检查，并人工复核标题、符号、调用边、错误边界和独立测试位置。
