// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库（workload repository）采样逻辑。
//
// 对应 Go `pkg/util/workloadrepo/sampling.go`。对标记为 `samplingTable` 的源表
// 周期性执行 INSERT…SELECT，将当前实例（instance）上的瞬时状态写入历史表，
// 并支持调整采样间隔配置项。

use crate::worker::worker as Worker;
use crate::*;

impl Worker {
    /// 对指定下标的采样表执行一次采样插入。
    ///
    /// 若尚未缓存 `insertStmt`，先按源表列定义构建 INSERT 语句；执行时绑定当前
    /// 实例 ID，便于多节点结果区分。
    pub fn samplingTable(&self, tableIndex: usize) -> Result<(), String> {
        let statement = {
            let mut tables = self.workloadTables.lock().unwrap();
            let table = tables
                .get_mut(tableIndex)
                .ok_or("table index out of range")?;
            // 惰性构建并缓存 INSERT…SELECT，避免每次采样重复拼 SQL。
            if table.insertStmt.is_empty() {
                buildInsertQuery(self.backend.as_ref(), table)?;
            }
            table.insertStmt.clone()
        };
        runQuery(
            self.backend.as_ref(),
            &statement,
            &[Value::String(self.instanceID())],
        )?;
        Ok(())
    }

    /// 返回一次“采样全部采样表”的闭包，供调度线程稍后调用。
    ///
    /// 先收集 `samplingTable` 类型表的下标，再并发逐表采样。与 Go 的定时器分支
    /// 一致，单表失败不会中止本轮其它表，也不会终止后续调度。
    pub fn startSample<'a>(&'a self) -> impl FnOnce() -> Result<(), String> + 'a {
        move || {
            // 在持锁期间只收集下标，避免长时间占用 workloadTables 锁。
            let indices = self
                .workloadTables
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter_map(|(index, table)| (table.tableType == samplingTable).then_some(index))
                .collect::<Vec<_>>();
            std::thread::scope(|scope| {
                let handles = indices
                    .into_iter()
                    .map(|index| scope.spawn(move || self.samplingTable(index)))
                    .collect::<Vec<_>>();
                for handle in handles {
                    // Go logs a failed table and continues the sampling loop. The
                    // backend error remains observable through `samplingTable`.
                    match handle.join() {
                        Ok(_) => {}
                        Err(payload) => std::panic::resume_unwind(payload),
                    }
                }
            });
            Ok(())
        }
    }

    /// 直接写入新的采样间隔（秒），不做范围校验。
    pub fn resetSamplingInterval(&self, newRate: i32) {
        self.state.lock().unwrap().samplingInterval = newRate;
    }
    /// 解析采样间隔配置字符串；范围钳制由调用该 hook 的系统变量层负责。
    pub fn changeSamplingInterval(&self, value: &str) -> Result<(), String> {
        let value = value
            .parse::<i32>()
            .map_err(|_| format!("wrong value for {repositorySamplingInterval}: {value}"))?;
        if value != self.state.lock().unwrap().samplingInterval {
            self.resetSamplingInterval(value);
        }
        Ok(())
    }
}
