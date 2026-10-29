# `pkg/types/pg_numeric.rs`

规范PostgreSQL numeric值实现；独立测试为同目录pg_numeric_test.rs。


## numeric数组开放（任务30）

新增共享from_binary/to_binary，逐字段验证PG numeric_recv，保持完整精度、尾零、wire dscale截断与NaN/±Infinity身份。from_binary的08P01/22P03独立于文本22P02；不经过MySQL decimal或浮点。生产算法扩展由用户明确授权，标量与数组共用。

当前精确18.6差分与验证记录见 `docs/postgresql-pg05-array-numeric-evidence.md`；只承诺记录范围。


## numeric ROUND 局部接线（任务4）

`round_scale` 复用声明modifier的精确十进制舍入，ties away from zero；先将int32 scale限到numeric表示域，保留尾零和NaN/Infinity。`apply_modifier`继续单独验证声明precision/scale并检查溢出。函数解析不经过MyDecimal或浮点；PG会话使用精确文本载体输出numeric和binary编码。
