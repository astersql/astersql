// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这个文件负责生成 dumpling 导出目录里的 `metadata` 文件，
// 记录导出开始/结束时间，以及与恢复相关的全局位点信息。
// 对不同数据库类型，metadata 的重点略有不同：
// MySQL/MariaDB 关注 `SHOW MASTER STATUS`；
// TiDB 在快照导出场景下还可能直接记录 `snapshot` 值。

// globalMetadata 把 metadata 文件的构建过程包装成一个可累计写入的对象。
pub struct globalMetadata {
    // tctx 主要用于记录“不支持的 server type”等警告日志。
    // buffer 是最终写入 `metadata` 文件的文本内容。
    pub tctx: tcontext::Context,
    // storage 为空时表示只在内存里构建，不实际落盘。
    pub buffer: Vec<u8>,
    pub after_conn_buffer: Vec<u8>,
    // snapshot 在 TiDB 快照导出路径里会被额外写进 metadata。
    pub storage: Option<std::sync::Arc<dyn Storage>>,
    pub snapshot: String,
}
// 输出文件名固定为 `metadata`，与 Go dumpling 导出目录保持一致。
pub const metadataFileName: &str = "metadata";

pub fn newGlobalMetadata(
    tctx: tcontext::Context,
    s: Option<std::sync::Arc<dyn Storage>>,
    snapshot: impl Into<String>,
) -> globalMetadata {
    // 构造时只初始化容器，不主动查询数据库。
    globalMetadata {
        tctx,
        buffer: Vec::new(),
        after_conn_buffer: Vec::new(),
        storage: s,
        snapshot: snapshot.into(),
    }
}

impl std::fmt::Display for globalMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 测试或调试场景可以直接把当前 buffer 当成字符串查看。
        f.write_str(std::str::from_utf8(&self.buffer).unwrap_or(""))
    }
}

impl globalMetadata {
    pub fn recordStartTime(&mut self, t: SystemTime) {
        // 开始/结束时间都写成人类可读的一行文本，格式与 Go 侧一致。
        let s = format_time(t);
        self.buffer
            .extend_from_slice(format!("Started dump at: {s}\n").as_bytes());
    }
    pub fn recordFinishTime(&mut self, t: SystemTime) {
        self.buffer.extend_from_slice(&self.after_conn_buffer);
        let s = format_time(t);
        self.buffer
            .extend_from_slice(format!("Finished dump at: {s}\n").as_bytes());
    }
    pub fn recordGlobalMetaData(
        &mut self,
        db: &Conn,
        server_info: &ServerInfo,
        after_conn: bool,
    ) -> Result<()> {
        // 真正的数据库查询逻辑委托给自由函数，便于测试和复用。
        let buffer = if after_conn {
            self.after_conn_buffer.clear();
            &mut self.after_conn_buffer
        } else {
            &mut self.buffer
        };
        recordGlobalMetaData(
            &self.tctx,
            db,
            buffer,
            server_info,
            after_conn,
            &self.snapshot,
        )
    }
    pub fn writeGlobalMetaData(&self) -> Result<()> {
        // storage 为空时允许静默跳过，方便纯内存测试。
        if let Some(store) = &self.storage {
            store.WriteFile(metadataFileName, &self.buffer)?;
        }
        Ok(())
    }
}

fn format_time(t: SystemTime) -> String {
    let seconds = match t.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub fn recordGlobalMetaData(
    tctx: &tcontext::Context,
    db: &Conn,
    buffer: &mut Vec<u8>,
    server_info: &ServerInfo,
    after_conn: bool,
    snapshot: &str,
) -> Result<()> {
    let write_master_status = |buffer: &mut Vec<u8>, log: &str, pos: &str, gtid: &str| {
        if log.is_empty() {
            return;
        }
        buffer.extend_from_slice(b"SHOW MASTER STATUS:");
        if after_conn {
            buffer.extend_from_slice(b" /* AFTER CONNECTION POOL ESTABLISHED */");
        }
        buffer
            .extend_from_slice(format!("\n\tLog: {log}\n\tPos: {pos}\n\tGTID:{gtid}\n").as_bytes());
    };
    match server_info.ServerType {
        ServerType::ServerTypeMySQL | ServerType::ServerTypeTiDB => {
            let status = ShowMasterStatus(db, server_info)?;
            let log = getValidStr(&status, 0);
            let pos =
                if server_info.ServerType == ServerType::ServerTypeTiDB && !snapshot.is_empty() {
                    snapshot.to_string()
                } else {
                    getValidStr(&status, 1)
                };
            let gtid = getValidStr(&status, 4);
            write_master_status(buffer, &log, &pos, &gtid);
        }
        ServerType::ServerTypeMariaDB => {
            let status = ShowMasterStatus(db, server_info)?;
            let log = getValidStr(&status, 0);
            let pos = getValidStr(&status, 1);
            let mut gtid = String::new();
            match db.QueryContext("SELECT @@global.gtid_binlog_pos") {
                Ok(mut rows) => {
                    if rows.Next() {
                        let mut value = [RawBytes(None)];
                        if rows.Scan(&mut value).is_ok() {
                            gtid = String::from_utf8_lossy(value[0].as_opt().unwrap_or(b"")).into();
                        }
                    }
                    let _ = rows.Close();
                }
                Err(error) => tctx.L().Warn(
                    "fail to get gtid for MariaDB",
                    [Field::string("error", error.msg)],
                ),
            }
            write_master_status(buffer, &log, &pos, &gtid);
        }
        _ => {
            return Err(errors_errorf(format!(
                "unsupported serverType {} for recordGlobalMetaData",
                server_info.ServerType.String()
            )));
        }
    }
    buffer.push(b'\n');
    if server_info.ServerType == ServerType::ServerTypeTiDB || after_conn {
        return Ok(());
    }

    let is_mariadb_multi_source = match db.QueryContext("SELECT @@default_master_connection") {
        Ok(mut rows) => {
            let has_row = rows.Next();
            rows.Close()?;
            has_row
        }
        Err(_) => false,
    };
    let follower_query = if is_mariadb_multi_source {
        "SHOW ALL SLAVES STATUS"
    } else if server_info.ServerType == ServerType::ServerTypeMySQL
        && server_info
            .ServerVersion
            .as_ref()
            .is_some_and(|version| !version.LessThan(&parse_semver("8.4.0")))
    {
        "SHOW REPLICA STATUS"
    } else {
        "SHOW SLAVE STATUS"
    };
    let mut rows = db.QueryContext(follower_query)?;
    let columns = rows.Columns()?;
    while rows.Next() {
        let mut values = vec![RawBytes(None); columns.len()];
        rows.Scan(&mut values)?;
        let mut connection_name = String::new();
        let mut pos = String::new();
        let mut log = String::new();
        let mut host = String::new();
        let mut gtid = String::new();
        for (column, value) in columns.iter().zip(values.iter()) {
            let value = String::from_utf8_lossy(value.as_opt().unwrap_or(b""));
            match column.to_ascii_lowercase().as_str() {
                "connection_name" => connection_name = value.into_owned(),
                "exec_master_log_pos" | "exec_source_log_pos" => pos = value.into_owned(),
                "relay_master_log_file" | "relay_source_log_file" => log = value.into_owned(),
                "master_host" | "source_host" => host = value.into_owned(),
                "executed_gtid_set" | "gtid_io_pos" => gtid = value.into_owned(),
                _ => {}
            }
        }
        if !host.is_empty() {
            buffer.extend_from_slice(b"SHOW SLAVE STATUS:\n");
            if is_mariadb_multi_source {
                buffer.extend_from_slice(
                    format!("\tConnection name: {connection_name}\n").as_bytes(),
                );
            }
            buffer.extend_from_slice(
                format!("\tHost: {host}\n\tLog: {log}\n\tPos: {pos}\n\tGTID:{gtid}\n\n").as_bytes(),
            );
        }
    }
    rows.Close()?;
    Ok(())
}

pub fn getValidStr(str_arr: &[String], idx: usize) -> String {
    // `SHOW MASTER STATUS` 字段不足时回空串，避免 metadata 构建直接 panic。
    str_arr.get(idx).cloned().unwrap_or_default()
}
