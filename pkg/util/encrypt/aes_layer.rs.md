# `pkg/util/encrypt/aes_layer.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-encrypt`，crate 入口是 [`pkg/util/encrypt/lib.rs`](lib.rs)，其中以 `pub mod aes_layer` 声明本模块并通过 `pub use aes_layer::*` 再导出公开 API。依赖边界由 [`pkg/util/encrypt/Cargo.toml`](Cargo.toml) 定义：本层直接使用 `aes`、`cipher`、`ctr` 和 `rand`；同一 crate 中的 `cbc`、`cfb-mode`、`ofb` 并非本文件的执行依赖。

它是一个可组合的 AES-128-CTR I/O 层，而不是通用密钥管理组件：创建时在进程内生成随机 128 位密钥和 nonce，写侧把明文加密后交给抽象的 `WriteCloser`，读侧从抽象的 `ReaderAt` 按明文偏移随机读取并解密。当前可见的生产接线位于 [`pkg/util/chunk/internal/group1/lib.rs`](../chunk/internal/group1/lib.rs)：`encrypt::NewWriter`、`encrypt::NewReader` 将本层包装到 spill 临时文件读写链；对应 Go 接线在 [`pkg/util/chunk/chunk_util.go`](../chunk/chunk_util.go)，仅在 spilled-file 加密配置不是 `plaintext` 时启用。

## 核心职责

1. `NewCtrCipher` / `NewCtrCipherWithBlockSize` 创建一次读写会话共享的 `CtrCipher`，保存随机密钥、随机 nonce、逻辑加密块大小以及每个逻辑块包含的 AES 块数。
2. `Writer` 聚合小写入；每次 `Flush` 对当前缓冲区原地施加连续 CTR 密钥流，再将密文写入下层，因而输出长度与明文长度完全相同，不添加头部、padding 或认证标签。
3. `Reader::ReadAt` 把任意明文偏移换算成逻辑块起点和 CTR 计数器，从底层读取整个逻辑块并只复制调用方请求的明文范围。
4. `WriteCloser`、`ReaderAt` 以及 `GoError = String` 提供与 Go `io.WriteCloser`、`io.ReaderAt` 和 `error` 形态相近的移植边界，便于与 checksum 层和 chunk I/O 适配器组合。

本文件只提供机密性，没有完整性或真实性保证；CTR 模式复用同一 `(key, nonce, counter)` 会复用密钥流，因此同一 `CtrCipher` 的字节位置只能对应同一条逻辑密文流。这一约束可由 `CtrCipher::stream` 的 IV 构造直接推出。

## 主要符号

- `GoError = String`：跨移植层传递的错误文本；本模块依赖精确字符串 `"EOF"` 判断可继续处理的部分读取。
- `errInvalidBlockSize`、`defaultEncryptBlockSize`：非法逻辑块大小错误文本与默认 1024 字节块大小。常量采用 Go 风格命名，并由 crate 级 lint 许可保留。
- `WriteCloser`：要求可变写入与关闭；为 `&mut T` 提供转发实现，使借用的下层也能嵌套。
- `ReaderAt`：以 `u64` 偏移执行不可变随机读；为 `&T` 提供转发实现。
- `CtrCipher { key, nonce, encryptBlockSize, aesBlockCount }`：私有字段保证调用者不能构造不满足块大小不变量的实例；`Clone` 使 reader/wrapper 可持有同一参数副本，`Debug` 会包含私有密钥字段，调用方不应将该值写入日志。
- `NewCtrCipher()`：以 1024 字节逻辑块调用 `NewCtrCipherWithBlockSize`。
- `NewCtrCipherWithBlockSize(i64)`：仅接受正数且为 AES 块大小 16 的倍数；使用 `OsRng` 生成 16 字节密钥和最高位清零的 `u64` nonce，计算 `aesBlockCount = encryptBlockSize / 16`。
- `CtrCipher::stream(counter)`：构造 16 字节大端 IV，高 8 字节为 nonce，低 8 字节为调用方给定的 AES 块计数器，并创建 `ctr::Ctr128BE<Aes128>`。
- `Writer<W>` / `NewWriter`：从计数器 0 初始化连续密钥流，并分配一个逻辑块大小的缓冲区。公开观察方法为 `AvailableSize`、`Buffered`、`GetCache`、`GetCacheDataOffset`。
- `Writer::Write`、`Flush`、`Close`：分别负责分块缓存、加密落盘和“先刷新、后关闭”。小写 `WriteCloser::write/close` 只转发到这组 Go 风格公开方法。
- `Reader<R>` / `NewReader`：拥有底层 reader 和一份 `CtrCipher`；`ReadAt` 执行偏移映射与解密。小写 `ReaderAt::read_at` 将 `u64` 偏移转成公开方法所需的 `i64`。

## 执行流程

写路径以 `NewWriter(w, cipher)` 开始。`Write` 先检查粘性错误；当输入长度大于缓冲剩余空间时，先填满缓冲再调用 `Flush`，随后继续消费输入。恰好能装入剩余空间的数据只留在缓存，直到后续写入、显式 `Flush` 或 `Close`。`Flush` 对 `buf[..n]` 原地加密，调用下层 `write`，按实际 `written` 增加 `flushedUserDataCnt`；完整写入后令 `n = 0`。`Close` 只有在刷新成功后才调用下层 `close`。RustCodeGraph 的直接边为 `Write -> AvailableSize/Flush`、`Close -> Flush`、`Flush -> WriteCloser::write`。

读路径以 `NewReader(r, cipher)` 开始。对非空请求，`ReadAt(p, off)` 计算：

- `offset = off % encryptBlockSize`：请求在首个逻辑块内的偏移；
- `counter = (off / encryptBlockSize) * aesBlockCount`：首个逻辑块对应的 AES 计数器；
- `cursor = off - offset`：底层密文的逻辑块对齐读取位置。

随后它用 `cipher.stream(counter)` 创建新密钥流，循环读取最多一个逻辑块、原地解密本轮实际读到的字节、首轮跳过 `offset` 后复制到调用方缓冲。后续轮次把块内偏移清零并延续同一个 CTR stream。因此随机读无需扫描此前密文，同时仍与从计数器 0 连续写出的密文对齐。RustCodeGraph 确认 `ReadAt -> ReaderAt::read_at/CtrCipher::stream`，trait 实现再由 `ReaderAt::read_at -> ReadAt` 转发。

在 chunk spill 主链中，[`pkg/util/chunk/internal/group1/lib.rs`](../chunk/internal/group1/lib.rs) 的 `EncryptWriteAdapter`/`EncryptReadAdapter` 把该模块的元组式错误协议桥接到 chunk 的 I/O trait；其 `encrypt::Writer` 再用 `Arc<Mutex<_>>` 提供可克隆、串行化访问的外层句柄。Go 侧 [`diskFileReaderWriter::initWithFileName`](../chunk/chunk_util.go) 展示了层次顺序：文件外包 AES writer，再外包 checksum writer；读取时按相反语义组合 reader，并用 writer 的 cache 与 cache offset 补足尚未落盘的数据。

## 数据与状态

`CtrCipher` 是只读配置快照。`key` 与 `nonce` 共同确定一条密钥流；`encryptBlockSize` 只控制 I/O 缓冲和随机定位粒度，`aesBlockCount` 保证逻辑块编号能换算为 AES 的 16 字节计数器步数。密文不携带这些参数，解密必须复用写入时的同一个 `CtrCipher`；当前 API 没有序列化、恢复或轮换密钥的能力。

`Writer` 是有状态的顺序写入器：`cipherStream` 必须随成功写出的逻辑字节单调前进，`buf[..n]` 表示当前尚未完成落盘的缓存，`flushedUserDataCnt` 是下层报告已写入的用户数据字节数，`err` 是首次刷新失败后的粘性错误。`GetCache` 返回借用切片，调用者只能观察未刷新明文，不能跨后续可变调用长期持有；chunk 层会复制成 `Vec<u8>`。由于 CTR 不改变长度，成功路径上的用户数据偏移也等于密文偏移。

`Reader` 自身不保存游标；每次 `ReadAt` 分配一个逻辑块大小的临时 `Vec<u8>` 和独立 CTR stream，因此不同调用之间没有共享的解密进度。空目标缓冲直接成功，不触碰下层。

## 依赖与调用关系

- crate 内部：[`lib.rs`](lib.rs) 声明并再导出本模块；没有 feature gate 或条件编译分支。测试模块仅在 `cfg(test)` 下从独立文件挂载，生产源与测试保持分离。
- 密码实现：`aes::Aes128` 定义分组密码，`ctr::Ctr128BE` 定义 128 位大端 CTR 计数器模式，`cipher::{KeyIvInit, StreamCipher}` 提供初始化和原地密钥流操作。
- 随机源：`rand::rngs::OsRng` 生成进程内 key 和 nonce。
- 上游生产调用：[`pkg/util/chunk/internal/group1/lib.rs`](../chunk/internal/group1/lib.rs) 的 `encrypt::{NewCtrCipher, NewWriter, NewReader}`；其 Cargo manifest 通过 `encrypt-crate = { package = "astersql-util-encrypt", path = "../encrypt" }` 建立依赖。
- 组合层：[`pkg/util/encrypt/aes_layer_test.rs`](aes_layer_test.rs) 验证纯 AES、checksum→AES、AES→checksum、双层 AES 四种 reader/writer 组合，证明 trait 边界允许透明嵌套；`checksum` 仅为 dev-dependency，不是本文件的运行时依赖。
- RustCodeGraph 能识别本文件内部调用边和 `aes_layer_test.rs::test_read_at -> NewWriter/NewReader`；对 `encrypt_crate` 别名后的 chunk 生产调用未给出精确 symbol caller，因此该接线由源码搜索和 `pkg/util/chunk/Cargo.toml` 交叉核验。

## 错误处理与边界

构造阶段对 `encryptBlockSize <= 0` 或非 16 倍数返回 `io::ErrorKind::InvalidInput`，文本为 `invalid encrypt block size`。这里的正数检查比当前 Go 文件更严格：Go 仅检查取模，Rust 明确拒绝 0 和负数，避免零长缓冲导致写循环无法前进或负数转 `usize`。

写阶段采用粘性错误：一旦 `Flush` 从下层得到错误，或发现 `written < n` 且下层未报错而合成 `"short write"`，就保存到 `Writer::err`；后续 `Write`、`Flush`、`Close` 返回同一文本，不再调用下层关闭。无论成功与否，`flushedUserDataCnt` 都只累计下层声明实际写入的字节；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `migration_ctr_layer_preserves_partial_write_count_with_error` 固定了“部分写 3 字节并返回 disk full”这一行为。错误发生前，缓冲区已经被原地变为密文且 CTR stream 已前进；粘性错误禁止重试这一内部缓冲，调用方应放弃 writer。

读阶段把精确字符串 `"EOF"` 当成可识别的结束信号：若本轮同时读到字节和 EOF，仍解密并复制这些字节；若没有读到字节，返回已累计数量和 EOF。任何其他错误立即连同已复制数量返回。Rust 还显式拒绝负偏移、在下层返回 0 字节且无错误时终止，并在首块偏移大于实际读取量时返回 `"invalid reader offset"`；Go 当前实现没有这些保护，相关异常输入可能落入底层错误、无限循环或切片越界。`ReaderAt for Reader` 接收 `u64` 后以 `as i64` 转换，超过 `i64::MAX` 的 trait 偏移会变为负数并得到 `negative offset`，因此本 API 的有效偏移范围实际上是 `0..=i64::MAX`。

本层没有认证标签，错误 key、nonce 或被篡改密文通常只会产生错误明文而不会报错；完整性要求必须由外层 checksum 或认证加密方案承担。

## 并发与资源生命周期

`Writer::Write`、`Flush`、`Close` 都需要 `&mut self`，类型本身不包含锁，也没有后台任务或通道；共享写入必须由调用方串行化。生产 wrapper 在 [`pkg/util/chunk/internal/group1/lib.rs`](../chunk/internal/group1/lib.rs) 使用 `Arc<Mutex<Writer<_>>>` 完成这一点。`Close` 不具备幂等承诺：首次成功关闭后再次调用会再次执行下层 `close`；调用方应只关闭一次。

`Reader::ReadAt` 只需要 `&self`，每次调用的临时缓冲与 stream 均为局部变量；只要泛型下层 `R` 可安全并发随机读，同一个 reader 的各次读取没有内部游标竞争。本模块未显式声明 `Send`/`Sync`，这些 auto trait 取决于泛型下层以及密码类型。

`CtrCipher` 的 key 和 nonce 只驻留内存，Drop 时没有显式清零；`Debug` 派生也意味着调试格式可能暴露 key。随机源失败没有通过本 API 的 `io::Result` 显式传播：`RngCore::fill_bytes/next_u64` 使用 `OsRng` 的直接方法，而不是可返回错误的 `try_fill_bytes`。资源关闭只由 `Writer::Close` 驱动；Rust `Drop` 不会自动 flush 或 close，忘记关闭会丢失尾部缓存。

## 与 Go 版本的对应关系

主要结构和算法逐项对应 [`pkg/util/encrypt/aes_layer.go`](aes_layer.go)：`CtrCipher` 四项状态、默认 1024 字节块、nonce+counter 的大端 IV、writer 的缓冲/粘性错误/短写规则、reader 的块对齐计数器换算，以及 `GetCache`/`GetCacheDataOffset` 给未落盘读取层补缓存的协议均保持一致。Rust 使用泛型 trait 代替 Go interface，并以 `(usize, Option<String>)` 表达 Go 的 `(n, error)`。

已确认的差异如下：

- Rust 构造器拒绝零和负块大小；Go 只拒绝非 16 倍数。
- Go 的 key/nonce 随机读取可返回 `error`；Rust 函数签名虽为 `io::Result`，但当前 `OsRng` 直接方法没有把随机源错误映射到该结果。
- Go nonce 由 `[0, math.MaxInt64)` 采样；Rust 清除最高位后范围可包含 `i64::MAX`。
- Rust `ReadAt` 增加负偏移、零进展和非法首块偏移保护；正常非负偏移、部分读取加 EOF 的语义与 Go 保持一致。
- Rust `NewReader` 拥有 `CtrCipher` 值，Go reader 保存指针；生产 wrapper 因此对 cipher 执行 `clone`。
- Go benchmark 实际运行三条性能管线；Rust 独立测试只用 `benchmark_read_at` 保留名称清单，没有稳定版 benchmark harness，因此 Rust 侧尚无等价性能验证。

[`pkg/util/encrypt/aes_layer_test.rs`](aes_layer_test.rs) 对应 Go `aes_layer_test.go`，用内存文件替代真实临时文件，保留随机偏移和四类组合管线的行为断言；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 额外覆盖非法块大小、缓存观察、尾部 EOF 和部分写失败后的偏移/粘性错误。

## 扩展指南

- 调整块大小或偏移算法时，应同步修改 `NewCtrCipherWithBlockSize`、`CtrCipher::stream` 和 `Reader::ReadAt`，并保持 `encryptBlockSize` 为 16 的正倍数以及 `counter = logical_block * aesBlockCount` 这一写读不变量。至少扩展 [`aes_layer_test.rs`](aes_layer_test.rs) 的跨块/尾部偏移用例与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的小块回归。
- 新增 I/O 下层时，实现本文件的 `WriteCloser`/`ReaderAt` 适配器；不要把适配代码塞回生产源中的测试模块。若错误协议不使用精确文本 `"EOF"`，应先建立结构化错误映射，否则部分尾读语义会变化。
- 修改缓存或 flush 行为时，要同时验证 `Buffered`、`GetCache`、`GetCacheDataOffset`，以及 chunk spill 的 `NewReaderWithCache` 接线；尾部未关闭时的可读性依赖这三个值。
- 增加并发共享时，应在外层像 chunk wrapper 一样加锁，而不是让多个 writer 共享同一 CTR stream；并发写乱序会破坏计数器与文件偏移的一一对应。
- 若要持久化或跨进程恢复，应设计显式、安全的 key/nonce 元数据格式和密钥生命周期；仅序列化 nonce 不足以解密。若安全目标包含防篡改，应采用带认证的格式，不能把 checksum 当作密码学认证。
- 修改公开 API 或 Go 对齐语义时，同步检查 [`aes_layer.go`](aes_layer.go)、[`aes_layer_test.go`](aes_layer_test.go)、[`aes_layer_test.rs`](aes_layer_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 以及 [`pkg/util/chunk/internal/group1/lib.rs`](../chunk/internal/group1/lib.rs)。性能相关变更还应补充可运行的 Rust benchmark，而不是只更新当前名称占位。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，目标文件被完整索引为 266 行。
- RustCodeGraph `node --file pkg/util/encrypt/aes_layer.rs`：核对了全部常量、trait、结构、函数和 impl；精确节点查询确认 `NewCtrCipher -> NewCtrCipherWithBlockSize`、`NewWriter -> CtrCipher::stream`、`Write -> AvailableSize/Flush`、`Flush -> WriteCloser::write`、`Close -> Flush/WriteCloser::close`、`ReadAt -> ReaderAt::read_at/CtrCipher::stream`。
- 读取的实现与边界文件：[`pkg/util/encrypt/aes_layer.rs`](aes_layer.rs)、[`pkg/util/encrypt/Cargo.toml`](Cargo.toml)、[`pkg/util/encrypt/lib.rs`](lib.rs)、[`pkg/util/encrypt/aes_layer.go`](aes_layer.go)。
- 读取的测试证据：[`pkg/util/encrypt/aes_layer_test.rs`](aes_layer_test.rs)、[`pkg/util/encrypt/aes_layer_test.go`](aes_layer_test.go)、[`pkg/util/encrypt/migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。
- 读取的生产接线：[`pkg/util/chunk/internal/group1/lib.rs`](../chunk/internal/group1/lib.rs)、[`pkg/util/chunk/chunk_util.go`](../chunk/chunk_util.go)、`pkg/util/chunk/Cargo.toml`。
- 人工复核结论：文件存在是为了给顺序 spill 写入和随机读取提供长度不变、可与 checksum/缓存层组合的 AES-CTR 变换；安全扩展必须维持 key/nonce/counter、逻辑块与明文偏移的对应关系，并同步独立测试与 chunk 适配层。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构检查要求本文恰好包含规定的 11 个二级标题。
