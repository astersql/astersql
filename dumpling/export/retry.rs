// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//
// 这个文件集中定义导出阶段几类“失败后还能不能继续”的退避策略。
// 重点不是通用重试框架，而是对 dumpling 当前两条关键路径做语义约束：
// 1. dump chunk/重建连接时，哪些错误值得指数退避；
// 2. LOCK TABLES 时，如果只是某张表不存在，如何跳过该表继续尝试。
// 因此这里的 backoffer 都非常小，只服务 export 包自己的恢复流程。

// dump chunk 与 lock tables 使用不同的重试上限，保持和 Go 侧经验值一致。
pub const dumpChunkRetryTime: i32 = 3;
pub const lockTablesRetryTime: i32 = 5;
// 连接重建的等待窗口采用指数退避，但被限制在一个较小上限内。
pub const dumpChunkWaitInterval: Duration = Duration::from_millis(50);
pub const dumpChunkMaxWaitInterval: Duration = Duration::from_millis(200);
// MySQL 1146 表示“表不存在”，是 lock tables 路径唯一会特殊处理的错误码。
pub const ErrNoSuchTable: u16 = 1146;

// Reset 让同一个 backoffer 可以在一次成功后重新回到初始预算。
pub trait backOfferResettable: BackoffStrategy {
    fn Reset(&mut self);
}

pub fn newRebuildConnBackOffer(should_retry: bool) -> Box<dyn backOfferResettable> {
    // 不允许重试时直接退化成 noop，实现上仍保留统一接口以简化调用方逻辑。
    if !should_retry {
        return Box::new(noopBackoffer { attempt: 1 });
    }
    // 允许重试时使用指数退避，既给数据库一点恢复时间，也避免等待无限膨胀。
    Box::new(dumpChunkBackoffer {
        attempt: dumpChunkRetryTime,
        delay_time: dumpChunkWaitInterval,
        max_delay_time: dumpChunkMaxWaitInterval,
    })
}

pub struct dumpChunkBackoffer {
    // attempt 表示剩余预算；delay_time 是下一轮等待；max_delay_time 用于封顶。
    pub attempt: i32,
    pub delay_time: Duration,
    pub max_delay_time: Duration,
}

impl BackoffStrategy for dumpChunkBackoffer {
    fn NextBackoff(&mut self, err: &Error) -> Duration {
        let err = errors_cause(err);
        // 对已知不可重试的 MySQL 错误，立即耗尽预算，避免无意义地重连。
        if err.mysql.is_some() && !IsRetryableError(err) {
            self.attempt = 0;
            return Duration::ZERO;
        }
        // 其余情况按 2 倍指数退避推进，但返回值仍受 max_delay_time 限制。
        self.delay_time = self.delay_time * 2;
        self.attempt -= 1;
        if self.delay_time > self.max_delay_time {
            return self.max_delay_time;
        }
        self.delay_time
    }
    fn RemainingAttempts(&self) -> i32 {
        self.attempt
    }
}
impl backOfferResettable for dumpChunkBackoffer {
    fn Reset(&mut self) {
        // reset 后重新回到初始节奏，确保下一次失败不会继承上一次的衰减状态。
        self.attempt = dumpChunkRetryTime;
        self.delay_time = dumpChunkWaitInterval;
    }
}

pub struct noopBackoffer {
    // noop 只允许调用方走一轮统一流程，然后立刻结束。
    pub attempt: i32,
}
impl BackoffStrategy for noopBackoffer {
    fn NextBackoff(&mut self, _err: &Error) -> Duration {
        // 返回 0 表示不等待，调用方会根据 attempt=0 终止后续重试。
        self.attempt -= 1;
        Duration::ZERO
    }
    fn RemainingAttempts(&self) -> i32 {
        self.attempt
    }
}
impl backOfferResettable for noopBackoffer {
    fn Reset(&mut self) {
        self.attempt = 1;
    }
}

pub fn newLockTablesBackoffer(
    tctx: tcontext::Context,
    block_list: HashMap<String, HashMap<String, ()>>,
    conf: &Config,
) -> lockTablesBackoffer {
    // 指定了具体表时不做多轮尝试，因为重复锁同一组显式表通常不会带来新结果。
    let attempt = if conf.SpecifiedTables {
        1
    } else {
        lockTablesRetryTime
    };
    lockTablesBackoffer {
        tctx,
        attempt,
        block_list,
    }
}

pub struct lockTablesBackoffer {
    // block_list 会逐步记录“本轮不再尝试加锁”的表，供上层重建 LOCK TABLES SQL。
    pub tctx: tcontext::Context,
    pub attempt: i32,
    pub block_list: HashMap<String, HashMap<String, ()>>,
}

impl BackoffStrategy for lockTablesBackoffer {
    fn NextBackoff(&mut self, err: &Error) -> Duration {
        let err = errors_cause(err);
        if let Some(mysql_err) = &err.mysql {
            if mysql_err.Number == ErrNoSuchTable {
                // 只有“表不存在”会被当成可恢复错误：记下它，然后立刻重试剩余表。
                self.attempt -= 1;
                match getTableFromMySQLError(&mysql_err.Message) {
                    Ok((db, table)) => {
                        self.block_list.entry(db).or_default().insert(table, ());
                        return Duration::ZERO;
                    }
                    Err(e) => {
                        // 连失败表名都解析不出来时，就没法安全跳过，直接终止重试。
                        self.tctx.L().Error(
                            "fail to retry lock tables",
                            [Field::string("error", e.msg.clone())],
                        );
                        self.attempt = 0;
                        return Duration::ZERO;
                    }
                }
            }
        }
        // 其他任何错误都视为不可恢复，避免带锁状态下反复试错。
        self.attempt = 0;
        Duration::ZERO
    }
    fn RemainingAttempts(&self) -> i32 {
        self.attempt
    }
}

pub fn getTableFromMySQLError(msg: &str) -> Result<(String, String)> {
    // 输入期望形如 `Table 'db.tbl' doesn't exist`，先剥掉固定前后缀。
    let msg = msg.strip_prefix("Table '").unwrap_or(msg);
    let msg = msg.strip_suffix("' doesn't exist").unwrap_or(msg);
    let fail_part: Vec<&str> = msg.split('.').collect();
    if fail_part.len() != 2 {
        // 解析失败时明确报 unsupported，让调用方不要误以为还能继续跳表重试。
        return Err(errors_errorf(format!(
            "doesn't support retry lock table {msg}"
        )));
    }
    Ok((fail_part[0].to_string(), fail_part[1].to_string()))
}
