# 任务 1: 消除 lockstore 固定十秒测试

批次：【批次 1】 无

状态：已完成

目的：保留单写多读和数据完整性覆盖，将固定十秒墙钟循环改成确定且足量的并发操作。

来源任务：用户提供的 `test_mem_store_concurrent` 超时。

预计会话范围：只修改 lockstore Rust 测试文件并运行单 crate 定向验证。

## 文件

- 修改：`pkg/store/mockstore/unistore/lockstore/lockstore_test.rs`
- 测试：`pkg/store/mockstore/unistore/lockstore/lockstore_test.rs`

## 上下文

- Go 测试用十秒墙钟制造并发；Rust 默认超时恰为十秒，因此必然超时。必须保留 10 个读者、随机 Put/Delete、数据一致性与读写均实际发生。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得另建构建目录。
2. 构建前用 `mkdir target/rust-slot-locks/slot-N.lock` 原子领取 1–10 中的空闲槽位。
3. 在锁目录记录计划、任务、会话和 PID；全部占用时等待，不删除活跃锁。
4. 设置绝对路径 `CARGO_TARGET_DIR="$PWD/target/rust-slot-N"`。
5. 所有 Cargo 命令和子进程继承该变量，不用 `--target-dir` 绕过。
6. 阶段结束后删除自己的记录并 `rmdir` 锁目录，保留编译缓存。
7. 正常、失败和中断都清理自己的锁；使用产物的进程结束前不释放。
8. 遗留锁先核实持有者已结束；不能确认则不删除。
9. 槽位不改变源码冲突和验证资源互斥关系。
10. 最终记录槽位、绝对路径、命令、退出码和有效测试数；Rust 修改后先 `cargo fmt --all`。

## 测试计划

- 行为：十个读者与一个写者交错访问，读值无损坏，插入、删除和读取均发生。
- 失败验证测试：现有 `lockstore_test::tests::test_mem_store_concurrent`。
- 失败验证命令：`cargo nextest run --locked --package astersql-store-mockstore-unistore-lockstore -E 'test(=lockstore_test::tests::test_mem_store_concurrent)'`
- 预期失败原因：固定运行约 10.01 秒，被默认 10 秒预算终止。
- 通过验证命令：同上。
- 模拟策略：使用真实 `MemStore` 和线程，不 mock。

## 步骤

1. 保存当前默认超时与 diagnostic 约 10 秒通过的证据。
2. 用确定操作预算替换十秒墙钟终止条件，并确保读线程已启动后再写。
3. 断言读、插入、删除均发生且值无损坏。
4. `cargo fmt --all` 后运行默认 profile 定向测试。

## 验证

- 运行：上述默认 nextest 命令。
- 预期：1 个测试通过，耗时显著低于 10 秒。
- 所需证据：修改前超时/10 秒 diagnostic、修改后退出码 0、1 个有效测试及耗时。

## 完成

使用固定 50000 轮 Put/Delete 和启动屏障保留 10 读者/1 写者覆盖；默认 profile 1 个测试通过，测试体 4.183 秒（修改前 diagnostic 为 10.020 秒，默认会超时）。
