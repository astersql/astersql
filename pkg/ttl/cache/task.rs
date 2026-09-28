// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// TTL 扫描任务（mysql.tidb_ttl_task）的 SQL 模板、Datum 编解码与行映射。
//
// 一个 TTL job 会拆成多个 scan task；本模块负责构造查询/插入语句，
// 并将系统表行解码为 `TTLTask`（含扫描范围与进度状态）。

// mysql.tidb_ttl_task 的 SQL 构造、任务状态和行解码逻辑。

/// 查询 mysql.tidb_ttl_task 的完整 SELECT（LOW_PRIORITY 降低对在线业务影响）。
pub const selectFromTTLTask: &str = "SELECT LOW_PRIORITY job_id,table_id,scan_id,scan_range_start,scan_range_end,expire_time,owner_id,owner_addr,owner_hb_time,status,status_update_time,state,created_time FROM mysql.tidb_ttl_task";
/// 插入 TTL task 的 SQL 模板；`%?` 为 TiDB 占位符风格。
pub const insertIntoTTLTask: &str = "INSERT LOW_PRIORITY INTO mysql.tidb_ttl_task SET job_id = %?,table_id = %?,scan_id = %?,scan_range_start = %?,scan_range_end = %?,expire_time = %?,created_time = %?";

#[derive(Clone, Debug, PartialEq)]
/// 简化 Datum：承载扫描范围编码与系统表单元格值。
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
    Time(i64),
}
/// 一行系统表结果，按 SELECT 列顺序排列。
pub type Row = Vec<Datum>;

/// 按 job_id 查询该 job 下全部 scan task。
pub fn SelectFromTTLTaskWithJobID(job_id: &str) -> (String, Vec<Datum>) {
    (
        format!("{selectFromTTLTask} WHERE job_id = %?"),
        vec![Datum::String(job_id.to_owned())],
    )
}
/// 按 job_id + scan_id 查询单条 task。
pub fn SelectFromTTLTaskWithID(job_id: &str, scan_id: i64) -> (String, Vec<Datum>) {
    (
        format!("{selectFromTTLTask} WHERE job_id = %? AND scan_id = %?"),
        vec![Datum::String(job_id.to_owned()), Datum::Int(scan_id)],
    )
}
/// 窥探可调度 task：waiting，或心跳超时仍标记 running 的任务。
pub fn PeekWaitingTTLTask(heartbeat_expire: i64) -> (String, Vec<Datum>) {
    (
        format!(
            "{selectFromTTLTask} WHERE status = 'waiting' OR (owner_hb_time < %? AND status = 'running') ORDER BY created_time ASC"
        ),
        vec![Datum::Time(heartbeat_expire)],
    )
}

/// 将 Datum 切片编码为可写入 scan_range_* 列的字节序列。
pub fn EncodeDatums(datums: &[Datum]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    fn encode_bytes(output: &mut Vec<u8>, value: &[u8]) {
        for chunk in value.chunks(8) {
            output.extend_from_slice(chunk);
            let padding = 8 - chunk.len();
            output.resize(output.len() + padding, 0);
            output.push(0xff - padding as u8);
        }
        if value.len().is_multiple_of(8) {
            output.extend_from_slice(&[0; 8]);
            output.push(0xf7);
        }
    }
    // 标志与 Go pkg/util/codec 一致；可比较数值编码同样翻转符号位。
    for datum in datums {
        match datum {
            Datum::Null => output.push(0),
            Datum::Int(value) => {
                output.push(3);
                output.extend_from_slice(&((*value as u64 ^ (1 << 63)).to_be_bytes()));
            }
            Datum::UInt(value) => {
                output.push(4);
                output.extend_from_slice(&value.to_be_bytes());
            }
            Datum::Float(value) => {
                output.push(5);
                let bits = value.to_bits();
                let encoded = if bits & (1 << 63) != 0 {
                    !bits
                } else {
                    bits ^ (1 << 63)
                };
                output.extend_from_slice(&encoded.to_be_bytes());
            }
            Datum::Bytes(value) => {
                output.push(1);
                encode_bytes(&mut output, value);
            }
            Datum::String(value) => {
                output.push(1);
                encode_bytes(&mut output, value.as_bytes());
            }
            Datum::Time(value) => {
                output.push(4);
                output.extend_from_slice(&(*value as u64).to_be_bytes());
            }
        }
    }
    Ok(output)
}

/// 解码 scan_range_* 列中的 Datum 序列；截断或不识别标签时报错。
pub fn DecodeDatums(mut input: &[u8]) -> Result<Vec<Datum>, String> {
    let mut result = Vec::new();
    while !input.is_empty() {
        let tag = input[0];
        input = &input[1..];
        match tag {
            0 => result.push(Datum::Null),
            3 | 4 | 5 => {
                if input.len() < 8 {
                    return Err("truncated datum".into());
                }
                let bits = u64::from_be_bytes(input[..8].try_into().unwrap());
                input = &input[8..];
                result.push(match tag {
                    3 => Datum::Int((bits ^ (1 << 63)) as i64),
                    4 => Datum::UInt(bits),
                    5 => {
                        let decoded = if bits & (1 << 63) != 0 {
                            bits ^ (1 << 63)
                        } else {
                            !bits
                        };
                        Datum::Float(f64::from_bits(decoded))
                    }
                    _ => unreachable!(),
                });
            }
            1 => {
                let mut value = Vec::new();
                loop {
                    if input.len() < 9 {
                        return Err("truncated datum bytes".into());
                    }
                    let padding = 0xff_u8.wrapping_sub(input[8]);
                    if padding > 8 || input[8 - padding as usize..8].iter().any(|byte| *byte != 0) {
                        return Err("invalid datum bytes padding".into());
                    }
                    value.extend_from_slice(&input[..8 - padding as usize]);
                    input = &input[9..];
                    if padding != 0 {
                        break;
                    }
                }
                result.push(Datum::Bytes(value));
            }
            _ => return Err(format!("unknown datum tag {tag}")),
        }
    }
    Ok(result)
}

/// 构造插入语句：先 EncodeDatums 编码起止范围再绑定参数。
pub fn InsertIntoTTLTask(
    job_id: &str,
    table_id: i64,
    scan_id: usize,
    start: &[Datum],
    end: &[Datum],
    expire_time: i64,
    created_time: i64,
) -> Result<(String, Vec<Datum>), String> {
    Ok((
        insertIntoTTLTask.to_owned(),
        vec![
            Datum::String(job_id.to_owned()),
            Datum::Int(table_id),
            Datum::Int(scan_id as i64),
            Datum::Bytes(EncodeDatums(start)?),
            Datum::Bytes(EncodeDatums(end)?),
            Datum::Time(expire_time),
            Datum::Time(created_time),
        ],
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 扫描任务生命周期状态（与系统表 status 字符串对应）。
pub enum TaskStatus {
    Waiting,
    Running,
    Finished,
    Other(String),
}
impl Default for TaskStatus {
    fn default() -> Self {
        Self::Other(String::new())
    }
}
impl TaskStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Waiting => "waiting",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Other(value) => value,
        }
    }
    fn from_string(value: String) -> Self {
        match value.as_str() {
            "waiting" => Self::Waiting,
            "running" => Self::Running,
            "finished" => Self::Finished,
            _ => Self::Other(value),
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 任务内部进度：总行/成功/失败，以及扫描错误与前任 owner。
pub struct TTLTaskState {
    pub TotalRows: u64,
    pub SuccessRows: u64,
    pub ErrorRows: u64,
    pub ScanTaskErr: String,
    pub PreviousOwner: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TTLTask {
    pub JobID: String,
    pub TableID: i64,
    pub ScanID: i64,
    pub ScanRangeStart: Vec<Datum>,
    pub ScanRangeEnd: Vec<Datum>,
    pub ExpireTime: i64,
    pub OwnerID: String,
    pub OwnerAddr: String,
    pub OwnerHBTime: i64,
    pub Status: TaskStatus,
    pub StatusUpdateTime: i64,
    pub State: Option<TTLTaskState>,
    pub CreatedTime: i64,
}

/// 从行中取字符串列；Bytes 按有损 UTF-8 转字符串，其它为零值。
pub(crate) fn string(row: &Row, index: usize) -> String {
    match row.get(index) {
        Some(Datum::String(value)) => value.clone(),
        Some(Datum::Bytes(value)) => String::from_utf8_lossy(value).into_owned(),
        _ => String::new(),
    }
}
/// 从行中取整型/时间列；缺失或类型不符返回 0。
pub(crate) fn int(row: &Row, index: usize) -> i64 {
    match row.get(index) {
        Some(Datum::Int(value)) | Some(Datum::Time(value)) => *value,
        Some(Datum::UInt(value)) => *value as i64,
        _ => 0,
    }
}
/// 取 Bytes 列；非 Bytes 返回 None。
fn bytes(row: &Row, index: usize) -> Option<&[u8]> {
    match row.get(index) {
        Some(Datum::Bytes(value)) => Some(value),
        _ => None,
    }
}
struct JsonCursor<'a> {
    input: &'a [u8],
    pos: usize,
}
impl JsonCursor<'_> {
    fn whitespace(&mut self) {
        while self
            .input
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn byte(&mut self, expected: u8) -> Result<(), String> {
        self.whitespace();
        if self.input.get(self.pos) != Some(&expected) {
            return Err(format!("invalid JSON at byte {}", self.pos));
        }
        self.pos += 1;
        Ok(())
    }
    fn string(&mut self) -> Result<String, String> {
        self.byte(b'"')?;
        let mut result = String::new();
        while let Some(&byte) = self.input.get(self.pos) {
            self.pos += 1;
            match byte {
                b'"' => return Ok(result),
                b'\\' => {
                    let escaped = *self.input.get(self.pos).ok_or("unterminated JSON escape")?;
                    self.pos += 1;
                    match escaped {
                        b'"' | b'\\' | b'/' => result.push(escaped as char),
                        b'b' => result.push('\u{8}'),
                        b'f' => result.push('\u{c}'),
                        b'n' => result.push('\n'),
                        b'r' => result.push('\r'),
                        b't' => result.push('\t'),
                        b'u' => {
                            let end = self.pos + 4;
                            let digits = self
                                .input
                                .get(self.pos..end)
                                .ok_or("short unicode escape")?;
                            let digits =
                                std::str::from_utf8(digits).map_err(|error| error.to_string())?;
                            let code = u32::from_str_radix(digits, 16)
                                .map_err(|error| error.to_string())?;
                            result.push(char::from_u32(code).ok_or("invalid unicode escape")?);
                            self.pos = end;
                        }
                        _ => return Err("invalid JSON escape".into()),
                    }
                }
                0..=31 => return Err("control character in JSON string".into()),
                _ if byte.is_ascii() => result.push(byte as char),
                _ => {
                    let start = self.pos - 1;
                    let tail = std::str::from_utf8(&self.input[start..])
                        .map_err(|error| error.to_string())?;
                    let character = tail.chars().next().ok_or("invalid UTF-8")?;
                    result.push(character);
                    self.pos = start + character.len_utf8();
                }
            }
        }
        Err("unterminated JSON string".into())
    }
    fn unsigned(&mut self) -> Result<u64, String> {
        self.whitespace();
        let start = self.pos;
        while self.input.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err("expected unsigned JSON integer".into());
        }
        std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|error| error.to_string())?
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())
    }
    fn skip_value(&mut self) -> Result<(), String> {
        self.whitespace();
        match self.input.get(self.pos) {
            Some(b'"') => self.string().map(|_| ()),
            Some(b'{') => {
                self.pos += 1;
                self.whitespace();
                if self.input.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    self.string()?;
                    self.byte(b':')?;
                    self.skip_value()?;
                    self.whitespace();
                    match self.input.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err("invalid JSON object".into()),
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                self.whitespace();
                if self.input.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    self.skip_value()?;
                    self.whitespace();
                    match self.input.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err("invalid JSON array".into()),
                    }
                }
            }
            Some(_) => {
                let start = self.pos;
                while self.input.get(self.pos).is_some_and(|byte| {
                    !matches!(byte, b',' | b'}' | b']') && !byte.is_ascii_whitespace()
                }) {
                    self.pos += 1;
                }
                let token = &self.input[start..self.pos];
                if token == b"true"
                    || token == b"false"
                    || token == b"null"
                    || std::str::from_utf8(token)
                        .ok()
                        .and_then(|value| value.parse::<f64>().ok())
                        .is_some()
                {
                    Ok(())
                } else {
                    Err("invalid JSON value".into())
                }
            }
            None => Err("missing JSON value".into()),
        }
    }
}
/// 按 Go encoding/json 的对象语义解析 TTLTaskState，未知字段忽略，类型错误返回失败。
fn parse_state(json: &str) -> Result<TTLTaskState, String> {
    let mut cursor = JsonCursor {
        input: json.as_bytes(),
        pos: 0,
    };
    let mut state = TTLTaskState::default();
    cursor.byte(b'{')?;
    cursor.whitespace();
    if cursor.input.get(cursor.pos) != Some(&b'}') {
        loop {
            let key = cursor.string()?;
            cursor.byte(b':')?;
            match key.as_str() {
                "total_rows" => state.TotalRows = cursor.unsigned()?,
                "success_rows" => state.SuccessRows = cursor.unsigned()?,
                "error_rows" => state.ErrorRows = cursor.unsigned()?,
                "scan_task_err" => state.ScanTaskErr = cursor.string()?,
                "prev_owner" => state.PreviousOwner = cursor.string()?,
                _ => cursor.skip_value()?,
            }
            cursor.whitespace();
            match cursor.input.get(cursor.pos) {
                Some(b',') => cursor.pos += 1,
                Some(b'}') => {
                    cursor.pos += 1;
                    break;
                }
                _ => return Err("invalid JSON object".into()),
            }
        }
    } else {
        cursor.pos += 1;
    }
    cursor.whitespace();
    if cursor.pos != cursor.input.len() {
        return Err("trailing data after JSON object".into());
    }
    Ok(state)
}
/// 将 13 列系统表行映射为 TTLTask；列数不足或范围解码失败则报错。
pub fn RowToTTLTask(row: &Row) -> Result<TTLTask, String> {
    if row.len() < 13 {
        return Err(format!(
            "TTL task row has {} columns, expected 13",
            row.len()
        ));
    }
    // Go 仅把非 NULL 的空串回退为 waiting；未知字符串原样保留，NULL 保持零值。
    let status = match row.get(9) {
        Some(Datum::String(value)) if value.is_empty() => TaskStatus::Waiting,
        Some(Datum::Bytes(value)) if value.is_empty() => TaskStatus::Waiting,
        Some(Datum::String(value)) => TaskStatus::from_string(value.clone()),
        Some(Datum::Bytes(value)) => {
            TaskStatus::from_string(String::from_utf8_lossy(value).into_owned())
        }
        _ => TaskStatus::default(),
    };
    Ok(TTLTask {
        JobID: string(row, 0),
        TableID: int(row, 1),
        ScanID: int(row, 2),
        ScanRangeStart: bytes(row, 3)
            .filter(|value| !value.is_empty())
            .map(DecodeDatums)
            .transpose()?
            .unwrap_or_default(),
        ScanRangeEnd: bytes(row, 4)
            .filter(|value| !value.is_empty())
            .map(DecodeDatums)
            .transpose()?
            .unwrap_or_default(),
        ExpireTime: int(row, 5),
        OwnerID: string(row, 6),
        OwnerAddr: string(row, 7),
        OwnerHBTime: int(row, 8),
        Status: status,
        StatusUpdateTime: int(row, 10),
        State: match row.get(11) {
            Some(Datum::String(value)) => Some(parse_state(value)?),
            _ => None,
        },
        CreatedTime: int(row, 12),
    })
}
