# pg_temporal.rs

提供独立于MySQL值域的PostgreSQL日期、时间、timestamp、timestamptz及interval规范载体。PgTime以私有i64微秒保留00:00到24:00的闭区间，文本、binary和精度舍入由同一codec实现。公开API与值域以源码和独立pg_temporal_test.rs测试为准。

## 任务36必要诊断纠错

PgTime::from_str在既有numeric displacement分支前区分非数值date-shaped命名时区词。未知名字沿已有PgTemporalError::UnknownZone返回22023；普通词、无效数值后缀保持22007，offset越界22009，clock越界22008。复用已有chrono_tz名单排除已知名字，不引入命名时区或POSIX求值算法。独立标量回归与time数组18.6四方向差分见docs/postgresql-pg05-array-time-evidence.md。
