# 任务 1: 修复统计与索引编码热点

批次：【批次 1】 无

状态：已完成

目的：消除已采样确认的二次扫描和逐行深拷贝。

来源任务：用户要求修复两个慢 SQL 测试。

预计会话范围：两个生产热点与定向测试。

## 文件

- 修改：`pkg/statistics/runtime_stats_builder.rs`
- 修改：`pkg/session/runtime/row_codec.rs`
- 修改：`pkg/tablecodec/tablecodec.rs`
- 测试：对应独立 `*_test.rs` 及 `pkg/session/runtime/statistics_test.rs`

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 共用 `target/rust-slot-1` 至 `target/rust-slot-10`，不得另建目录。
2. 用原子 `mkdir target/rust-slot-locks/slot-N.lock` 领取槽位。
3. 锁内记录计划、任务、会话和 PID；满时等待。
4. 设置绝对 `CARGO_TARGET_DIR="$PWD/target/rust-slot-N"`。
5. 所有 Cargo 命令继承变量，不绕过槽位。
6. 阶段结束删除自己的记录并 `rmdir`，保留缓存。
7. 正常、失败、中断均清理。
8. 遗留锁必须核实，不删除活跃锁。
9. 槽位不解除源码和共享资源互斥。
10. 最终记录槽位、路径、命令、退出码、测试数；先 `cargo fmt --all`。

## 测试计划

- 行为：增量内存值等于全量计算；借用式编码与原 API 字节完全一致；两个原 SQL 测试保持全部断言。
- 失败验证：53 分区约 20 秒；10 万行超过 60 秒。
- 通过验证：默认 profile 精确运行两个测试。

## 步骤

1. 添加等价性测试。
2. 实现 O(1) 增量记账和借用式编码。
3. 格式化并运行模块、原 SQL 回归与 lint。

## 验证

- 预期：两个原测试默认 profile 通过且单个低于 10 秒。
- 实际：借用式编码等价性测试通过；10 万行用例 9.012 秒，53 分区用例 2.285 秒，均在默认 nextest 的 10 秒上限内。
