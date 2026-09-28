// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库快照（snapshot）协调逻辑。
//
// 对应 Go `pkg/util/workloadrepo/snapshot.go`。通过 etcd 上的 SNAP_ID 做 CAS
// （compare-and-swap，比较并交换）分配全局递增快照号，将 `snapshotTable` 类型表
// 的数据写入历史库，并维护 `HIST_SNAPSHOTS` 元数据行的起止时间与错误信息。

use crate::worker::worker as Worker;
use crate::*;

impl Worker {
    /// 在 etcd（或后端 KV）中创建键值；键已存在则返回错误。
    pub fn etcdCreate(&self, key: &str, value: &str) -> Result<(), String> {
        if self.backend.kv_create(key, value)? {
            Ok(())
        } else {
            Err(format!("failed to create etcd [{key}:{value}]"))
        }
    }
    /// 读取 etcd 键；缺失时返回空字符串。
    pub fn etcdGet(&self, key: &str) -> Result<String, String> {
        Ok(self.backend.kv_get(key)?.unwrap_or_default())
    }
    /// etcd CAS：仅当当前值为 `old` 时写入 `new`，失败返回错误。
    pub fn etcdCAS(&self, key: &str, old: &str, new: &str) -> Result<(), String> {
        if self.backend.kv_cas(key, old, new)? {
            Ok(())
        } else {
            Err(format!(
                "failed to update etcd [{key}:{old}] to [{key}:{new}]"
            ))
        }
    }
}

/// 查询历史快照表中当前最大 `SNAP_ID`；无行或 NULL 时返回 0。
pub fn queryMaxSnapID(backend: &dyn RepositoryBackend) -> Result<u64, String> {
    let rows = runQuery(
        backend,
        &format!("SELECT MAX(`SNAP_ID`) FROM `{workloadSchema}`.`{histSnapshotsTable}`"),
        &[],
    )?;
    match rows.first().and_then(|row| row.first()) {
        Some(Value::UInt(value)) => Ok(*value),
        Some(Value::Null) => Ok(0),
        _ => Err("no rows returned when querying max snap id".into()),
    }
}

impl Worker {
    /// 从 etcd 读取当前 SNAP_ID；键不存在时返回 `errKeyNotFound`。
    pub fn getSnapID(&self) -> Result<u64, String> {
        let value = self.etcdGet(snapIDKey)?;
        if value.is_empty() {
            return Err(errKeyNotFound.into());
        }
        value.parse::<u64>().map_err(|error| error.to_string())
    }
}

/// 向 `HIST_SNAPSHOTS` 写入或刷新指定 snapID 的开始时间。
pub fn upsertHistSnapshot(backend: &dyn RepositoryBackend, snapID: u64) -> Result<(), String> {
    runQuery(
        backend,
        &format!(
            "INSERT INTO `{workloadSchema}`.`{histSnapshotsTable}` (`BEGIN_TIME`, `SNAP_ID`) VALUES (now(), %?) ON DUPLICATE KEY UPDATE `BEGIN_TIME` = now()"
        ),
        &[Value::UInt(snapID)],
    )?;
    Ok(())
}

impl Worker {
    /// 快照结束后写回 `END_TIME` 与可选错误摘要。
    pub fn updateHistSnapshot(&self, snapID: u64, errors: &[String]) -> Result<(), String> {
        let error = (!errors.is_empty())
            .then(|| errors.join("\n"))
            .map(Value::String)
            .unwrap_or(Value::Null);
        runQuery(
            self.backend.as_ref(),
            &format!(
                "UPDATE `{workloadSchema}`.`{histSnapshotsTable}` SET `END_TIME` = now(), `ERROR` = COALESCE(CONCAT(ERROR, %?), ERROR, %?) WHERE `SNAP_ID` = %?"
            ),
            &[error.clone(), error, Value::UInt(snapID)],
        )?;
        Ok(())
    }

    /// 对单张快照表执行 INSERT…SELECT，绑定 snapID 与实例 ID。
    pub fn snapshotTable(&self, snapID: u64, tableIndex: usize) -> Result<(), String> {
        let statement = {
            let mut tables = self.workloadTables.lock().unwrap();
            let table = tables
                .get_mut(tableIndex)
                .ok_or("table index out of range")?;
            // 惰性构建快照插入语句并缓存。
            if table.insertStmt.is_empty() {
                buildInsertQuery(self.backend.as_ref(), table).map_err(|error| {
                    format!(
                        "could not generate insert statement for `{}`: {error}",
                        table.destTable
                    )
                })?;
            }
            (table.insertStmt.clone(), table.destTable.clone())
        };
        runQuery(
            self.backend.as_ref(),
            &statement.0,
            &[Value::UInt(snapID), Value::String(self.instanceID())],
        )
        .map_err(|error| {
            format!(
                "could not run insert statement for `{}`: {error}",
                statement.1
            )
        })?;
        Ok(())
    }

    /// 分配下一个全局 SNAP_ID：先 upsert 元数据，再 etcd create/CAS 提交。
    ///
    /// etcd 键缺失时从 SQL 最大 SNAP_ID 恢复；并发冲突时按 `snapshotRetries` 重试。
    pub fn takeSnapshot(&self) -> Result<u64, String> {
        let mut last = String::new();
        for _ in 0..snapshotRetries {
            // empty=true 表示 etcd 尚无 snapID，需 create 而非 CAS。
            let (snapID, empty) = match self.getSnapID() {
                Ok(value) => (value, false),
                Err(error) if error == errKeyNotFound => {
                    match queryMaxSnapID(self.backend.as_ref()) {
                        Ok(value) => (value, true),
                        Err(error) => {
                            last = format!("cannot get current snapid: {error}");
                            continue;
                        }
                    }
                }
                Err(error) => {
                    last = format!("cannot get current snapid: {error}");
                    continue;
                }
            };
            if let Err(error) = upsertHistSnapshot(self.backend.as_ref(), snapID + 1) {
                last = format!("could not insert into hist_snapshots: {error}");
                continue;
            }
            let updated = if empty {
                self.etcdCreate(snapIDKey, &(snapID + 1).to_string())
            } else {
                self.etcdCAS(snapIDKey, &snapID.to_string(), &(snapID + 1).to_string())
            };
            match updated {
                Ok(()) => return Ok(snapID + 1),
                Err(error) => last = format!("cannot update current snapid to {snapID}: {error}"),
            }
        }
        Err(last)
    }

    /// 返回对所有 `snapshotTable` 执行快照并更新元数据的闭包。
    pub fn startSnapshot<'a>(&'a self, snapID: u64) -> impl FnOnce() -> Result<(), String> + 'a {
        move || {
            let indices = self
                .workloadTables
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter_map(|(index, table)| (table.tableType == snapshotTable).then_some(index))
                .collect::<Vec<_>>();
            let errors = std::thread::scope(|scope| {
                indices
                    .into_iter()
                    .map(|index| scope.spawn(move || self.snapshotTable(snapID, index)))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .filter_map(|handle| handle.join().unwrap().err())
                    .collect::<Vec<_>>()
            });
            self.updateHistSnapshot(snapID, &errors)
        }
    }
    /// 直接写入新的快照间隔（秒），不做范围校验。
    pub fn resetSnapshotInterval(&self, newRate: i32) {
        self.state.lock().unwrap().snapshotInterval = newRate;
    }
    /// 解析快照间隔配置字符串；范围规范化由系统变量层完成。
    pub fn changeSnapshotInterval(&self, value: &str) -> Result<(), String> {
        let value = value
            .parse::<i32>()
            .map_err(|_| format!("wrong value for {repositorySnapshotInterval}: {value}"))?;
        if self.state.lock().unwrap().snapshotInterval != value {
            self.resetSnapshotInterval(value);
        }
        Ok(())
    }
}
