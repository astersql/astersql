// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// 延迟回调调度器（deferFn）。
//
// 对应 Go `issyncer.deferFn`：登记带触发时刻的回调，在 `check` 时执行已到期项并保留未到期项。
// 用于 InfoSchema 同步器中推迟执行的清理或通知逻辑。

use std::sync::Mutex;
use std::time::Instant;
/// 单条延迟记录：触发时刻与一次性回调。
struct deferFnRecord {
    fire: Instant,
    callback: Box<dyn FnOnce() + Send>,
}
/// 延迟回调集合；到期项在 `check` 中执行。
#[derive(Default)]
pub struct deferFn {
    records: Mutex<Vec<deferFnRecord>>,
}
impl deferFn {
    /// 登记回调，在 `fire` 之后由 `check` 触发（与 Go `time.After` 一致）。
    pub fn add<F: FnOnce() + Send + 'static>(&self, callback: F, fire: Instant) {
        self.records.lock().unwrap().push(deferFnRecord {
            fire,
            callback: Box::new(callback),
        });
    }
    /// 执行所有已到期回调，未到期的写回 `records`。
    pub fn check(&self) {
        let now = Instant::now();
        let mut pending = Vec::new();
        // Go holds deferFn's mutex throughout iteration and callback execution,
        // so concurrent add/check operations cannot mutate the queue midway.
        let mut records = self.records.lock().unwrap();
        for record in records.drain(..) {
            if record.fire < now {
                (record.callback)()
            } else {
                pending.push(record)
            }
        }
        *records = pending;
    }
    /// len exposes the number of pending (not-yet-fired) callbacks. Go's
    /// `deferFn` names this field `data`; the Rust port calls it `records`,
    /// so tests use this accessor instead of reaching into the field.
    /// 返回尚未触发的回调数量（Go 字段名为 `data`，Rust 为 `records`）。
    pub fn len(&self) -> usize {
        self.records.lock().unwrap().len()
    }
    /// 是否没有待触发回调。
    pub fn is_empty(&self) -> bool {
        self.records.lock().unwrap().is_empty()
    }
}
/// 公开类型别名，与 Go 侧 `DeferFn` 命名对齐。
pub type DeferFn = deferFn;
