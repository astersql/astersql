# 任务 3: 收敛 session runtime 典型超时

批次：【批次 2】 依赖批次 1 的性能方法验证

状态：已完成

目的：对 TTL 与 statistics 的代表性超时做逐 SQL 计时，修复可复用初始化或明显算法热点。

来源任务：用户列出的 session TTL 与 combined statistics 超时。

预计会话范围：只处理基线稳定复现且不依赖外部服务的一个共同根因；没有共同根因则记录为范围外建议。

## 文件

- 可能修改：`pkg/session/runtime/*.rs`
- 测试：对应 `pkg/session/runtime/*_test.rs` 独立文件

## 上下文

- 这些测试都会构造内存 Domain，必须先分清初始化/销毁成本、数据生成成本和生产算法成本，不允许直接删减 SQL 断言。

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

- 行为：原 TTL 时区/变量恢复和 combined statistics SQL 结果保持不变。
- 失败验证测试：用户列出的精确测试。
- 失败验证命令：使用 diagnostic profile、单线程和精确 nextest filter 逐个测量。
- 预期失败原因：默认 10 秒预算下超时。
- 通过验证命令：修复目标用例以默认 profile 运行。
- 模拟策略：使用真实内存 Domain 和 SQL runtime。

## 步骤

1. 逐例测量，定位初始化、SQL 或析构阶段。
2. 选择至少一个稳定、可局部修复的共同根因；新增或保留回归断言。
3. 做不改变 SQL 行为的最小优化。
4. 格式化并运行精确用例。

## 验证

- 运行：精确默认 nextest 过滤命令。
- 预期：目标用例均通过且低于 10 秒。
- 所需证据：基线、修复后耗时、退出码、有效测试数与未处理项解释。

## 完成

四个 TTL 矩阵测试复用一个已 bootstrap 的 Domain、为各 case 创建独立 session/table，分别由 13.247/9.201/8.888/8.940 秒降至 2.876/2.789/2.330/2.893 秒。`combined_merge_sql_100010_rows_seven_partitions` diagnostic 仍超过 60 秒，未用缩减数据规避，列为后续生产算法剖析项。
