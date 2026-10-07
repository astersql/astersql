# `br/pkg/restore/data/key.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-restore-data`。crate 入口 `br/pkg/restore/data/lib.rs` 以 `pub mod key` 装配本模块，并通过 `pub use key::*` 将其函数暴露到 crate 根；`br/pkg/restore/data/Cargo.toml` 声明该 crate 是 Go 包 `br/pkg/restore/data` 的 Rust 移植库，当前没有外部 Cargo 依赖。

它位于 BR 恢复数据规划的纯内存算法层：`br/pkg/restore/data/recover.rs` 先从各 store 的 Region peer 选择代表，再用本文件提供的字节比较和前缀编码构造有序键空间、消解重叠并检查区间连续性。本文件本身不访问 PD、TiKV、磁盘或网络，也不拥有恢复任务的生命周期。

## 核心职责

本文件提供两组基础操作：

- `keyEq`、`keyCmp` 和 `keyCmpInterface` 定义恢复键的逐字节相等关系与无符号字节字典序。它们保证有序容器、区间重叠判断和相邻边界判断使用同一套顺序。
- `PrefixStartKey`、`PrefixEndKey` 把原始 Region 边界映射到恢复算法使用的内部键空间。普通边界统一增加字节 `b'z'`；空结束键代表正无穷，因此编码成单字节 `b'z' + 1`，排在所有以 `b'z'` 开头的有限边界之后。

这些函数刻意保持简单且确定：相同输入总是得到相同输出，没有隐藏状态，也没有根据运行环境改变行为。

## 主要符号

- `pub fn keyEq(a: &[u8], b: &[u8]) -> bool`：先比较长度，再逐字节比较；只有长度和每个位置的字节都相同才返回 `true`。
- `pub fn keyCmp(a: &[u8], b: &[u8]) -> i32`：先记录公共比较长度及长度差对应的候选结果，再扫描公共前缀。首个不同字节立即决定 `-1` 或 `1`；公共部分完全相同时，短键小于长键，等长键相等。
- `pub fn keyCmpInterface(a: &[u8], b: &[u8]) -> i32`：直接委托 `keyCmp`。它保留 Go `keyCmpInterface(any, any)` 的命名和比较器入口角色，但 Rust 类型签名已在编译期限制为字节切片。
- `pub fn PrefixStartKey(key: &[u8]) -> Vec<u8>`：预分配 `key.len() + 1` 字节，写入 `b'z'` 后复制原键；空输入得到 `b"z"`。
- `pub fn PrefixEndKey(key: &[u8]) -> Vec<u8>`：空输入返回 `vec![b'z' + 1]`；非空输入复用 `PrefixStartKey`。

文件没有常量、类型、trait、`impl` 或条件编译项；五个函数均为公开 API。

## 执行流程

恢复规划中的典型流程如下：

1. `SortRecoverRegions` 在每个 Region 的 peer 中选出日志进度最靠前者。
2. 它调用 `PrefixStartKey` 和 `PrefixEndKey`，把该 peer 的原始起止键转换为 `RecoverRegionInfo` 中的内部边界；空结束键被提升为正无穷哨兵。
3. `CheckConsistencyAndValidPeer` 按已编码的 `StartKey` 将候选放入 `BTreeMap`。Rust 容器自身按 `Vec<u8>` 的字典序排列，而显式的重叠判断用 `keyEq` 与 `keyCmp`：后继起点落在当前终点之前、或前驱终点越过当前起点时，当前候选被跳过。
4. 重叠消解完成后，校验从 `PrefixStartKey(&[])`（即 `b"z"`）开始遍历；每一段的起点必须与上一段终点经 `keyEq` 完全相等，才能形成连续恢复键空间。

在本文件内部，`keyCmpInterface` 只调用 `keyCmp`；`PrefixEndKey` 的非空分支只调用 `PrefixStartKey`，其余函数没有下游函数调用。

## 数据与状态

输入统一借用 `&[u8]`，比较函数不复制也不修改输入。字节按 Rust `u8` 的数值顺序比较，因此该顺序是二进制字典序，不涉及 UTF-8、区域设置或字符串规范化。

两个前缀函数返回新建的 `Vec<u8>`，不修改调用方缓冲区。`PrefixStartKey` 的容量恰为原键长度加一；`PrefixEndKey` 对空键只分配一个字节。前缀 `b'z'` 与哨兵 `b'{'`（即 `b'z' + 1`）是恢复算法的编码约定：所有有限边界都以 `b'z'` 开头，而空结束键编码在它们之后。调用方必须同时使用对应的起点和终点编码，不能把原始键与编码键混入同一比较域。

本模块没有全局变量、缓存、锁、引用计数或可变静态状态。

## 依赖与调用关系

直接上游来自 `br/pkg/restore/data/recover.rs`：

- `SortRecoverRegions` 调用 `PrefixStartKey`、`PrefixEndKey` 构造每个 `RecoverRegionInfo`。
- `CheckConsistencyAndValidPeer` 调用 `keyEq`、`keyCmp` 判断区间重叠，调用 `PrefixStartKey(&[])` 初始化连续性检查的左边界。

测试上游来自 `br/pkg/restore/data/parity_test.rs`：`contract_normal_keys_and_sort` 直接验证相等、大小、公共前缀和前缀哨兵；`contract_boundary_prefix_and_empty_peers` 验证空起点编码。`new_recover_region_info` 也用两个前缀函数构造测试输入。`br/pkg/restore/data/key_test.rs` 通过同名辅助函数间接验证前缀编码参与 Region 排序与连续性检查的行为。

下游仅使用 Rust 标准库的切片、`Vec`、长度和索引操作；`Cargo.toml` 的 `[dependencies]` 为空。RustCodeGraph 显示目标文件被 `recover.rs` 与 `parity_test.rs` 使用，并确认上述调用边。

## 错误处理与边界

本模块没有 `Result`、错误类型或 panic 分支。比较函数接受空切片：两个空键相等且比较结果为 `0`；空键按公共前缀规则小于任意非空键。循环上界来自已验证的长度或两者最小长度，因此不会越界。

`PrefixStartKey(&[])` 返回 `b"z"`，代表恢复键空间的最小编码起点；`PrefixEndKey(&[])` 返回 `b"{"`，代表无界结束位置。非空结束键与起点使用同一种 `b'z'` 前缀编码，从而允许直接判断相邻和重叠。

资源耗尽是唯一隐含失败边界：创建 `Vec` 可能因内存分配失败而终止进程，这是 Rust 标准分配语义，本文件没有恢复策略。极端情况下 `key.len() + 1` 还受 `usize` 溢出和最大分配容量约束；实际 Region key 尺寸远小于该边界，但扩展时不应把这里误当作任意长度输入的防御性 API。

## 并发与资源生命周期

五个函数均为同步、无状态的纯计算函数，可被多个线程并发调用而无需锁。借用输入只在函数调用期间有效；比较函数不保留引用，前缀函数返回独占拥有的 `Vec<u8>`，其释放由调用方作用域和 Rust 所有权规则管理。

本模块不创建线程、异步任务、通道、事务、文件句柄或网络连接，也没有需要显式关闭或回滚的资源。主要性能成本是 `keyEq`/`keyCmp` 最坏情况下线性扫描公共长度，以及每次前缀转换的一次线性复制和一次堆分配。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/restore/data/key.go`，五个 Rust 函数与 Go 同名函数逐一对应：

- `keyEq` 保留“先长度、后逐字节”的实现和布尔结果。
- `keyCmp` 保留 `-1/0/1` 三值约定、首个差异字节优先以及公共前缀相同时按长度裁决的逻辑。
- Go 的 `keyCmpInterface(a, b any)` 会在运行时把参数断言为 `[]byte`；Rust 的 `keyCmpInterface(&[u8], &[u8])` 去掉了动态类型断言，错误类型无法通过编译，但对合法字节参数的结果一致。
- `PrefixStartKey` 都分配新缓冲区并添加字符 `'z'`；`PrefixEndKey` 都把空结束键编码为 `'z' + 1`，非空时委托起点编码。

Go 的 `recover.go` 使用 `treemap.NewWith(keyCmpInterface)` 提供容器比较器；Rust 的 `recover.rs` 改用标准库 `BTreeMap<Vec<u8>, _>`，因此当前 Rust 生产路径没有直接调用 `keyCmpInterface`，但保留该函数作为 Go API/语义镜像。显式区间判断仍调用 `keyCmp`，并由 `br/pkg/restore/data/parity_test.rs` 固定其行为。

## 扩展指南

修改键顺序或编码时，应把本文件视为一个整体契约，而不是孤立修改某个函数：

- 改比较规则时，同时检查 `keyCmp`、`keyCmpInterface` 与 `BTreeMap<Vec<u8>, _>` 的顺序是否仍一致；若二者分叉，容器遍历顺序和显式重叠判断会互相矛盾。
- 改前缀或无穷哨兵时，同时更新 `PrefixStartKey`、`PrefixEndKey`、`SortRecoverRegions` 的构造语义和 `CheckConsistencyAndValidPeer` 的初始边界，并确认新哨兵严格大于所有有限编码键。
- 新增回归测试应放在独立测试文件中，不要内嵌到 `key.rs`。直接函数契约优先扩展 `br/pkg/restore/data/parity_test.rs`；涉及排序、重叠和连续性行为时同步扩展 `br/pkg/restore/data/key_test.rs`，并核对 Go 的 `br/pkg/restore/data/key_test.go`。
- 如果目标是继续保持移植对齐，还应同步审查 `br/pkg/restore/data/key.go` 和 `recover.go`，明确记录任何有意差异。

兼容性风险集中在持久化/跨模块键序变化和空结束键含义；性能风险集中在恢复 Region 数量较大时的重复分配与复制。若要优化分配，必须先证明不会改变 `RecoverRegionInfo` 对键的独立所有权。

## 验证依据

- Rust 源码：`br/pkg/restore/data/key.rs`（五个公开函数的实现与签名）。
- crate 边界：`br/pkg/restore/data/Cargo.toml`（crate 名、`lib.rs` 入口、Go 包映射、空依赖表）；`br/pkg/restore/data/lib.rs`（模块装配与公开重导出）。
- 生产调用方：`br/pkg/restore/data/recover.rs` 中的 `SortRecoverRegions`、`CheckConsistencyAndValidPeer`。
- Go 对照：`br/pkg/restore/data/key.go`、`br/pkg/restore/data/recover.go`。
- 独立测试：`br/pkg/restore/data/key_test.rs`、`br/pkg/restore/data/parity_test.rs`；Go 测试为 `br/pkg/restore/data/key_test.go`。
- RustCodeGraph：`node --file br/pkg/restore/data/key.rs` 确认文件全貌及使用文件；针对 `keyEq`、`keyCmp`、`PrefixStartKey`、`PrefixEndKey` 的调用关系查询确认 `recover.rs` 与 `parity_test.rs` 的调用边，以及 `keyCmpInterface -> keyCmp`、`PrefixEndKey -> PrefixStartKey` 的文件内调用。
- 按任务要求未运行 Cargo；该任务只新增说明文档，结构验证命令及结果在交付检查中记录。
