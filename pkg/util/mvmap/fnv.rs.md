# `pkg/util/mvmap/fnv.rs`

## 文件定位

本文件是 `astersql-util-mvmap` crate 内部的 FNV-1 64 位哈希实现。crate 入口 `pkg/util/mvmap/lib.rs` 通过 `include!("mvmap.rs")` 引入主体，主体在 `pkg/util/mvmap/mvmap.rs:23-26` 以私有 `mod fnv` 声明本模块并导入 `fnv_hash64`。因此它不是对 workspace 公开的通用哈希 API，而是 `MVMap` 把字节 key 映射到 `HashMap<u64, entryAddr>` 桶头时使用的内部组件。

`pkg/util/mvmap/Cargo.toml` 将该 crate 命名为 `astersql-util-mvmap`，库入口为 `lib.rs`，且没有为本文件声明 feature 或外部依赖；其移植元数据明确对应 Go 包 `pkg/util/mvmap`。

## 核心职责

`fnv_hash64(data: &[u8]) -> u64` 为任意字节切片计算确定性的 FNV-1 64 位值：从固定偏移量开始，对每个字节依次先乘 FNV 素数、再与该字节异或。它只负责产生桶键，不负责判定 key 相等、处理碰撞或存取 value。

碰撞正确性由调用方保证：`MVMap::Put` 将同一哈希值的条目串成链，`MVMap::Get` 遍历链后由 `dataStore::get` 比较原始 key 字节。因而本哈希会影响桶分布和性能，但不同 key 得到相同哈希不会直接造成错误命中（见 `pkg/util/mvmap/mvmap.rs:202-245`）。

## 主要符号

- `OFFSET64: u64`：私有常量，值为 `14695981039346656037`，是 FNV-1 64 位初始偏移量；与 `pkg/util/mvmap/fnv.go` 的 `offset64` 相同。
- `PRIME64: u64`：私有常量，值为 `1099511628211`，是每轮混合使用的 FNV 素数；与 Go 的 `prime64` 相同。
- `pub(crate) fn fnv_hash64(data: &[u8]) -> u64`：crate 内可见的唯一函数。输入是只读字节切片，输出是 64 位哈希；不分配、不返回借用，也不暴露给 crate 外部。

文件没有类型、trait、`impl`、条件编译项或可变静态状态。RustCodeGraph 将本文件识别为两个符号（常量索引未返回精确名称时，以文件节点源码复核），并将 `fnv_hash64` 定位在 `pkg/util/mvmap/fnv.rs:37`。

## 执行流程

1. 将局部变量 `hash` 初始化为 `OFFSET64`。
2. 按切片顺序遍历 `data` 的每个 `u8`。
3. 用 `hash.wrapping_mul(PRIME64)` 计算乘积，显式按模 `2^64` 回绕。
4. 将乘积与当前字节提升后的 `u64` 做异或，作为下一轮状态。
5. 遍历结束后返回 `hash`。

这是 FNV-1（multiply-then-XOR），不是 FNV-1a（XOR-then-multiply）。空输入不会进入循环，结果就是 `OFFSET64`；该结论直接来自函数控制流，当前独立测试没有单列空输入用例。

在应用内，写路径 `MVMap::Put` 先对 key 调用本函数，再读取同桶旧头、追加数据和 entry，最后更新桶头；读路径 `MVMap::Get` 先计算同一哈希，再沿 entry 链过滤真实 key 并收集 values（`pkg/util/mvmap/mvmap.rs:202-245`）。

## 数据与状态

函数状态仅有栈上的局部 `u64 hash` 和对输入切片的只读迭代。每一轮状态完全由前一轮哈希和当前字节决定；字节顺序不同通常产生不同结果，重复调用相同输入则得到相同结果。

显式 `wrapping_mul` 是跨语言一致性的关键不变量：Go 的 `uint64` 乘法自然按 `2^64` 回绕，而 Rust 调试构建中的普通溢出乘法可能 panic。这里不保存 key、哈希缓存或随机种子，因此结果跨进程稳定，也不具备抗恶意碰撞哈希的随机化性质。

## 依赖与调用关系

上游直接调用边由 RustCodeGraph 的 `fnv_hash64` 节点确认：

- `MVMap::Put`（`pkg/util/mvmap/mvmap.rs:202`）调用它选择写入桶。
- `MVMap::Get`（`pkg/util/mvmap/mvmap.rs:225`）调用它选择读取桶。
- `test_fnv_hash`（`pkg/util/mvmap/mvmap_test.rs:85`）验证已知向量。
- `fnv_hash_matches_go_test_vector`（`pkg/util/mvmap/migration_aster_unit_test.rs:109`）再次验证 Go 对齐向量。

RustCodeGraph 的 callees 查询返回“无被调用者”：实现只使用语言内建的切片迭代、整数转换、回绕乘法与异或，没有调用仓库函数或外部 crate。模块装配链为 `lib.rs` → `include!("mvmap.rs")` → `mod fnv`；`Cargo.toml` 也未列出第三方依赖。

## 错误处理与边界

函数没有 `Result`/`Option` 返回值和显式错误分支。任意有效的 `&[u8]`（包括空切片和包含零字节的切片）都能得到一个 `u64`；输入借用保证读取期间内存有效。

算术溢出不是错误，而是 FNV 算法的一部分，由 `wrapping_mul` 明确定义。哈希碰撞也不是本函数的错误条件：调用方必须继续比较原始 key，本 crate 的 `dataStore::get` 正是该防线。新增调用者若只凭哈希值认定 key 相等，将破坏现有边界约定。

该算法不是密码学哈希，也没有针对对抗性输入的安全承诺。修改常量、运算顺序、整数宽度或回绕规则会改变所有桶键，并破坏与 Go 实现及现有测试向量的兼容性。

## 并发与资源生命周期

`fnv_hash64` 无共享可变状态，仅只读访问调用者提供的切片，因此多个线程可并发调用本函数；这与 `pkg/util/mvmap/fnv.go` 中“thread-safe”的注释一致。函数不持有锁、不启动任务、不使用通道、不打开文件或网络资源，也没有需要清理的堆资源。

这种函数级线程安全不等于 `MVMap` 整体线程安全。`MVMap::Put` 会修改哈希表和分片存储，主体文件明确要求在单执行流中使用；是否同步 `MVMap` 是调用方责任，与本无状态函数的生命周期不同。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/util/mvmap/fnv.go`：`OFFSET64`/`PRIME64` 分别对应 `offset64`/`prime64`，`fnv_hash64` 对应 `fnvHash64`。两者均从相同偏移量开始，对每个字节执行“乘素数后异或”，返回 64 位无符号值。

可见性表达不同但边界一致：Go 的小写符号限于包内，Rust 的常量为模块私有、函数为 `pub(crate)`；后者供同 crate 中由 `include!` 组装的 `mvmap.rs` 和独立测试模块调用。Rust 用 `wrapping_mul` 显式表达 Go `uint64` 的回绕语义，是语义保真而不是算法变化。

Go 测试 `pkg/util/mvmap/mvmap_test.go:53` 将结果与标准库 `fnv.New64()` 比较。Rust 的 `pkg/util/mvmap/mvmap_test.rs:85` 和 `migration_aster_unit_test.rs:109` 使用同一输入 `{cb f2 9c e4 84 22 23 25}`，期望 `0x51af634308c212fc`，为当前移植结果提供直接对照。

## 扩展指南

若只需在 MVMap 内继续使用 FNV-1，应复用 `fnv_hash64`，不要在调用点复制算法。若要改变算法或支持另一种哈希，应优先在本模块新增清晰命名的独立函数，并在 `mvmap.rs` 的桶选择位置显式接线；必须同步评估写入和读取两条路径，避免两者使用不同算法。

任何算法调整都应同步独立测试文件，而不要把测试嵌入生产源文件。最低限度应更新或扩展 `pkg/util/mvmap/mvmap_test.rs`，并按迁移目的决定是否同步 `pkg/util/mvmap/migration_aster_unit_test.rs`；Go 对齐变化还应核对 `pkg/util/mvmap/fnv.go` 与 `mvmap_test.go`。建议覆盖空输入、单字节、含零字节、产生多次回绕的长输入及与 Go/标准库一致的固定向量。

扩展时需重点检查三类风险：改变稳定哈希结果的兼容风险、桶分布退化或额外分配带来的性能风险，以及绕过原始 key 比较导致碰撞误命中的正确性风险。若要引入随机化或密码学性质，那已超出当前 Go 对齐的 FNV-1 合同，应作为显式架构变更处理。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 `11467` 个文件；`files --filter pkg/util/mvmap` 覆盖本模块的 Rust/Go 实现与测试。
- RustCodeGraph 文件节点：`node --file pkg/util/mvmap/fnv.rs --offset 1 --limit 120`，核对完整 45 行源码、常量、函数签名和算法。
- RustCodeGraph 符号与调用查询：`query fnv_hash64 --kind function`、`node pkg/util/mvmap/fnv.rs::fnv_hash64`、`callees pkg/util/mvmap/fnv.rs::fnv_hash64`；节点轨迹确认四个直接调用者，callees 确认没有仓库级下游调用。独立 `callers` 命令曾无输出挂起并被终止，调用方证据改由同一索引的 node trail 及下列精确节点交叉验证。
- RustCodeGraph 精确节点：`pkg/util/mvmap/mvmap.rs::Put`、`pkg/util/mvmap/mvmap.rs::Get`、`pkg/util/mvmap/mvmap_test.rs::test_fnv_hash`、`pkg/util/mvmap/migration_aster_unit_test.rs::fnv_hash_matches_go_test_vector`。
- crate 与装配证据：`pkg/util/mvmap/Cargo.toml`、`pkg/util/mvmap/lib.rs`、`pkg/util/mvmap/mvmap.rs:16-26`。
- Go 对照与测试证据：`pkg/util/mvmap/fnv.go`、`pkg/util/mvmap/mvmap.go:130-163`、`pkg/util/mvmap/mvmap_test.go:53-64`。
- Rust 独立测试证据：`pkg/util/mvmap/mvmap_test.rs:85-90`、`pkg/util/mvmap/migration_aster_unit_test.rs:109-112`。本任务只新增文档，按计划不运行 Cargo；验证以源码、调用图、跨语言测试向量和文档结构检查为准。
