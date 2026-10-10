# 任务 2: 优化 indexusage 会话增量热点

批次：【批次 1】 无

状态：已完成

目的：让并发聚合测试在默认预算内完成，并使 Rust delta 所有权与 Go 实现一致。

来源任务：两个 indexusage 并发测试各约 37 秒。

预计会话范围：修改 indexusage 生产实现和同目录独立测试文件。

## 文件

- 修改：`pkg/statistics/handle/usage/indexusage/collector.rs`
- 修改：`pkg/statistics/handle/usage/indexusage/collector_test.rs`
- 修改：`pkg/statistics/handle/usage/indexusage/migration_aster_unit_test.rs`

## 上下文

- Go 会话 collector 独占普通 map，Report/Flush 转移 map；Rust 当前每次 Update 获取外层和内层两个 Mutex。两个 Rust 测试还各自生成两遍 640 万操作。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库共用 `target/rust-slot-1` 至 `target/rust-slot-10`，不得另建目录。
2. 用原子 `mkdir target/rust-slot-locks/slot-N.lock` 领取空闲槽位。
3. 锁内记录计划、任务、会话和 PID；满时等待。
4. 设置绝对 `CARGO_TARGET_DIR="$PWD/target/rust-slot-N"`。
5. 所有 Cargo 命令继承变量，不绕过槽位。
6. 阶段结束删除自己的记录并 `rmdir`，保留缓存。
7. 正常、失败、中断均清理，进程结束前不释放。
8. 遗留锁必须核实，不删除他人活跃锁。
9. 槽位不解除源码和共享资源互斥。
10. 最终记录槽位、路径、命令、退出码、测试数；先 `cargo fmt --all`。

## 测试计划

- 行为：64 个会话并发 Report/Flush 后逐索引结果等于串行聚合。
- 失败验证测试：现有两个精确测试已在 diagnostic profile 分别耗时 37.1 秒和 37.6 秒。
- 失败验证命令：`cargo nextest run --locked --profile diagnostic --package astersql-statistics-handle-usage-indexusage -E 'test(=collector_test::test_flush_concurrent_index_collector) | test(=migration_aster_unit_test::concurrent_flush_matches_serial_aggregation)'`
- 预期失败原因：默认 profile 在 10 秒终止。
- 通过验证命令：去掉 diagnostic 后运行同一过滤器。
- 模拟策略：使用真实 worker/channel/map。

## 步骤

1. 将 delta 改为 worker 可直接拥有的 map；会话状态只保留一层互斥。
2. Report/Flush 用替换方式转移 map，保持发送失败时不丢增量。
3. 将同路径 Go 对齐测试改成一次生成、串行与并发复用同一操作序列。
4. 保留迁移补充测试的语义但避免重复完整压力规模。
5. 格式化并运行两个定向测试及 crate 全部测试。

## 验证

- 运行：上述默认 nextest 命令和该 package 全测试。
- 预期：有效测试均通过且单测不超过 10 秒。
- 所需证据：前后耗时、退出码、测试数和结果逐项相等断言仍存在。

## 完成

delta 改为单层会话锁和所有权转移，拒绝发送会归还原 map；完整 64×100000 用例保留并从 35.56 秒降至 4.36 秒，补充用例 0.987 秒，默认 profile 均通过。
