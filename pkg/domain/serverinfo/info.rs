// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 服务器信息（ServerInfo）数据结构与 JSON 编解码。
//
// 本模块定义 TiDB 实例在 etcd 中登记的静态/动态信息与拓扑信息，
// 以及轻量 JSON 解析器，供 `syncer` 读写 `/tidb/server/info` 与 `/topology/tidb`。
// Keyspace 表示多租户命名空间；AssumedKeyspace 表示跨 keyspace 假定身份。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// etcd 上存放各节点 ServerInfo 的路径前缀。
pub const ServerInformationPath: &str = "/tidb/server/info";
/// etcd 键操作默认重试次数。
pub const KeyOpDefaultRetryCnt: i32 = 5;
/// etcd 键操作默认超时。
pub const KeyOpDefaultTimeout: Duration = Duration::from_secs(1);
/// etcd 上存放拓扑（topology）信息的路径前缀。
pub const TopologyInformationPath: &str = "/topology/tidb";
/// 拓扑 session 租约 TTL（秒）。
pub const TopologySessionTTL: i32 = 45;
/// 拓扑信息周期性刷新间隔。
pub const TopologyTimeToRefresh: Duration = Duration::from_secs(30);
/// 最小启动时间戳（min start TS）上报间隔。
pub const minTSReportInterval: Duration = Duration::from_secs(30);
/// 发布版本号占位（写入拓扑 info）。
pub const TiDBReleaseVersion: &str = "v8.4.0-this-is-a-placeholder";
/// MySQL 兼容版本字符串占位（写入 ServerInfo）。
pub const ServerVersion: &str = "8.0.11-TiDB-v8.4.0-this-is-a-placeholder";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 版本与 Git 提交哈希。
pub struct VersionInfo {
    /// 版本号字符串。
    pub Version: String,
    /// 构建所用 Git 哈希。
    pub GitHash: String,
}

#[derive(Clone)]
/// 节点生命周期内基本不变的静态信息：地址、DDL ID、keyspace 等。
pub struct StaticInfo {
    /// 版本信息。
    pub VersionInfo: VersionInfo,
    /// DDL / 节点唯一 ID（uuid）。
    pub ID: String,
    /// 对外宣告的 IP / 主机名。
    pub IP: String,
    /// SQL 监听端口。
    pub Port: u32,
    /// HTTP status 端口。
    pub StatusPort: u32,
    /// DDL owner 等租约描述字符串。
    pub Lease: String,
    /// 进程启动时间戳（秒）。
    pub StartTimestamp: i64,
    /// 当前实例所属 keyspace（多租户命名空间）。
    pub Keyspace: String,
    /// 跨 keyspace 假定身份时的目标 keyspace；非空表示 IsAssumed。
    pub AssumedKeyspace: String,
    /// 惰性取 server_id 的回调（序列化时刷新 JSONServerID）。
    pub ServerIDGetter: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    /// 最近一次 Marshal 写入 JSON 的 server_id。
    pub JSONServerID: u64,
}

impl Default for StaticInfo {
    fn default() -> Self {
        Self {
            VersionInfo: VersionInfo::default(),
            ID: String::new(),
            IP: String::new(),
            Port: 0,
            StatusPort: 0,
            Lease: String::new(),
            StartTimestamp: 0,
            Keyspace: String::new(),
            AssumedKeyspace: String::new(),
            ServerIDGetter: None,
            JSONServerID: 0,
        }
    }
}

impl StaticInfo {
    /// 是否处于跨 keyspace 假定身份（AssumedKeyspace 非空）。
    pub fn IsAssumed(&self) -> bool {
        !self.AssumedKeyspace.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 可运行时变更的动态信息（目前主要为 Labels）。
pub struct DynamicInfo {
    /// 节点标签键值对。
    pub Labels: HashMap<String, String>,
}

impl DynamicInfo {
    /// 堆分配克隆，对齐 Go 侧返回指针的习惯。
    pub fn Clone(&self) -> Box<DynamicInfo> {
        Box::new(Clone::clone(self))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 供 PD / 拓扑发现使用的精简节点信息（含部署路径与标签）。
pub struct TopologyInfo {
    /// 发布版本与 Git 哈希。
    pub VersionInfo: VersionInfo,
    /// 节点 IP。
    pub IP: String,
    /// status HTTP 端口。
    pub StatusPort: u32,
    /// 可执行文件所在部署目录。
    pub DeployPath: String,
    /// 启动时间戳。
    pub StartTimestamp: i64,
    /// 节点标签。
    pub Labels: HashMap<String, String>,
}

#[derive(Clone, Default)]
/// 完整服务器信息：静态字段 + 动态字段。
pub struct ServerInfo {
    /// 静态信息。
    pub StaticInfo: StaticInfo,
    /// 动态信息。
    pub DynamicInfo: DynamicInfo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// ServerInfo 编解码错误。
pub struct ServerInfoError(pub String);

impl fmt::Display for ServerInfoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ServerInfoError {}

impl fmt::Display for ServerInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut info = self.clone();
        match info.Marshal() {
            Ok(encoded) => formatter.write_str(&String::from_utf8_lossy(&encoded)),
            Err(error) => write!(formatter, "<failed to marshal server info: {error}>"),
        }
    }
}

impl StaticInfo {
    /// Decode the static part embedded in /info using the same Go-compatible
    /// field converters as ServerInfo; dynamic labels are intentionally ignored.
    pub fn Unmarshal(&mut self, value: &[u8]) -> Result<(), ServerInfoError> {
        let text = String::from_utf8_lossy(value);
        match JsonParser::new(&text).parse()? {
            Json::Null => Ok(()),
            Json::Object(object) => {
                self.unmarshal_fields(object.iter().map(|(key, value)| (key.as_str(), value)))
            }
            _ => Err(ServerInfoError("server info JSON must be an object".into())),
        }
    }

    fn unmarshal_fields<'a>(
        &mut self,
        object: impl IntoIterator<Item = (&'a str, &'a Json)>,
    ) -> Result<(), ServerInfoError> {
        let mut first_error = None;
        for (key, value) in object {
            let result = if key.eq_ignore_ascii_case("version") {
                assign_string(value, "version", &mut self.VersionInfo.Version)
            } else if key.eq_ignore_ascii_case("git_hash") {
                assign_string(value, "git_hash", &mut self.VersionInfo.GitHash)
            } else if key.eq_ignore_ascii_case("ddl_id") {
                assign_string(value, "ddl_id", &mut self.ID)
            } else if key.eq_ignore_ascii_case("ip") {
                assign_string(value, "ip", &mut self.IP)
            } else if key.eq_ignore_ascii_case("listening_port") {
                assign_u32(value, "listening_port", &mut self.Port)
            } else if key.eq_ignore_ascii_case("status_port") {
                assign_u32(value, "status_port", &mut self.StatusPort)
            } else if key.eq_ignore_ascii_case("lease") {
                assign_string(value, "lease", &mut self.Lease)
            } else if key.eq_ignore_ascii_case("start_timestamp") {
                assign_i64(value, "start_timestamp", &mut self.StartTimestamp)
            } else if key.eq_ignore_ascii_case("keyspace") {
                assign_string(value, "keyspace", &mut self.Keyspace)
            } else if key.eq_ignore_ascii_case("assumed_keyspace") {
                assign_string(value, "assumed_keyspace", &mut self.AssumedKeyspace)
            } else if key.eq_ignore_ascii_case("server_id") {
                assign_u64(value, "server_id", &mut self.JSONServerID)
            } else {
                Ok(())
            };
            preserve_first_error(&mut first_error, result);
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl ServerInfo {
    /// 堆分配克隆。
    pub fn Clone(&self) -> Box<ServerInfo> {
        Box::new(Clone::clone(self))
    }

    /// 序列化为 etcd 存储用的 JSON 字节；必要时先通过 ServerIDGetter 刷新 server_id。
    pub fn Marshal(&mut self) -> Result<Vec<u8>, ServerInfoError> {
        // Go 侧会无条件调用 ServerIDGetter；未初始化是编程错误而不是可忽略状态。
        let server_id = self
            .StaticInfo
            .ServerIDGetter
            .as_ref()
            .expect("ServerIDGetter must be initialized")();
        self.StaticInfo.JSONServerID = server_id;
        let info = &self.StaticInfo;
        // 标签按键排序，保证 JSON 稳定。
        let mut labels: Vec<_> = self.DynamicInfo.Labels.iter().collect();
        labels.sort_by(|left, right| left.0.cmp(right.0));
        let labels = labels
            .into_iter()
            .map(|(key, value)| format!("{}:{}", json_string(key), json_string(value)))
            .collect::<Vec<_>>()
            .join(",");
        let mut fields = vec![
            format!("\"version\":{}", json_string(&info.VersionInfo.Version)),
            format!("\"git_hash\":{}", json_string(&info.VersionInfo.GitHash)),
            format!("\"ddl_id\":{}", json_string(&info.ID)),
            format!("\"ip\":{}", json_string(&info.IP)),
            format!("\"listening_port\":{}", info.Port),
            format!("\"status_port\":{}", info.StatusPort),
            format!("\"lease\":{}", json_string(&info.Lease)),
            format!("\"start_timestamp\":{}", info.StartTimestamp),
        ];
        if !info.Keyspace.is_empty() {
            fields.push(format!("\"keyspace\":{}", json_string(&info.Keyspace)));
        }
        if !info.AssumedKeyspace.is_empty() {
            fields.push(format!(
                "\"assumed_keyspace\":{}",
                json_string(&info.AssumedKeyspace)
            ));
        }
        fields.push(format!("\"server_id\":{}", info.JSONServerID));
        fields.push(format!("\"labels\":{{{labels}}}"));
        Ok(format!("{{{}}}", fields.join(",")).into_bytes())
    }

    /// 从 JSON 字节反序列化并回填 StaticInfo / DynamicInfo。
    pub fn Unmarshal(&mut self, value: &[u8]) -> Result<(), ServerInfoError> {
        // encoding/json 会把字符串中的无效 UTF-8 替换为 U+FFFD，而不是拒绝整个文档。
        let text = String::from_utf8_lossy(value);
        let Json::Object(object) = JsonParser::new(&text).parse()? else {
            return Err(ServerInfoError("server info JSON must be an object".into()));
        };
        let mut first_error = None;
        for (key, value) in &object {
            let result = if key.eq_ignore_ascii_case("labels") {
                assign_labels(value, "labels", &mut self.DynamicInfo.Labels)
            } else {
                self.StaticInfo
                    .unmarshal_fields(std::iter::once((key.as_str(), value)))
            };
            preserve_first_error(&mut first_error, result);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        // 反序列化后用固定闭包复现 ServerIDGetter。
        let server_id = self.StaticInfo.JSONServerID;
        self.StaticInfo.ServerIDGetter = Some(Arc::new(move || server_id));
        Ok(())
    }

    /// 转为拓扑信息：版本用 TiDBReleaseVersion，部署路径取当前可执行文件目录。
    pub fn ToTopologyInfo(&self) -> TopologyInfo {
        let deploy_path = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.parent()
                    .map(|parent| parent.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| ".".into());
        TopologyInfo {
            VersionInfo: VersionInfo {
                Version: TiDBReleaseVersion.into(),
                GitHash: self.StaticInfo.VersionInfo.GitHash.clone(),
            },
            IP: self.StaticInfo.IP.clone(),
            StatusPort: self.StaticInfo.StatusPort,
            DeployPath: deploy_path,
            StartTimestamp: self.StaticInfo.StartTimestamp,
            Labels: self.DynamicInfo.Labels.clone(),
        }
    }
}

impl TopologyInfo {
    /// 将拓扑信息编码为 JSON 字节。
    pub fn Marshal(&self) -> Vec<u8> {
        let mut labels: Vec<_> = self.Labels.iter().collect();
        labels.sort_by(|left, right| left.0.cmp(right.0));
        let labels = labels
            .into_iter()
            .map(|(key, value)| format!("{}:{}", json_string(key), json_string(value)))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"version\":{},\"git_hash\":{},\"ip\":{},\"status_port\":{},\"deploy_path\":{},\"start_timestamp\":{},\"labels\":{{{}}}}}", json_string(&self.VersionInfo.Version), json_string(&self.VersionInfo.GitHash), json_string(&self.IP), self.StatusPort, json_string(&self.DeployPath), self.StartTimestamp, labels).into_bytes()
    }

    /// 从 JSON 字节解析拓扑信息。
    pub fn Unmarshal(value: &[u8]) -> Result<Self, ServerInfoError> {
        let text = String::from_utf8_lossy(value);
        let Json::Object(object) = JsonParser::new(&text).parse()? else {
            return Err(ServerInfoError("topology JSON must be an object".into()));
        };
        let mut info = Self::default();
        let mut first_error = None;
        for (key, value) in &object {
            let result = if key.eq_ignore_ascii_case("version") {
                assign_string(value, "version", &mut info.VersionInfo.Version)
            } else if key.eq_ignore_ascii_case("git_hash") {
                assign_string(value, "git_hash", &mut info.VersionInfo.GitHash)
            } else if key.eq_ignore_ascii_case("ip") {
                assign_string(value, "ip", &mut info.IP)
            } else if key.eq_ignore_ascii_case("status_port") {
                assign_u32(value, "status_port", &mut info.StatusPort)
            } else if key.eq_ignore_ascii_case("deploy_path") {
                assign_string(value, "deploy_path", &mut info.DeployPath)
            } else if key.eq_ignore_ascii_case("start_timestamp") {
                assign_i64(value, "start_timestamp", &mut info.StartTimestamp)
            } else if key.eq_ignore_ascii_case("labels") {
                assign_labels(value, "labels", &mut info.Labels)
            } else {
                Ok(())
            };
            preserve_first_error(&mut first_error, result);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(info)
    }
}

/// 将字符串转义为 JSON 字符串字面量。
fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '<' => output.push_str("\\u003c"),
            '>' => output.push_str("\\u003e"),
            '&' => output.push_str("\\u0026"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[derive(Clone, Debug)]
/// 轻量 JSON AST（仅覆盖 ServerInfo 所需子集）。
enum Json {
    Object(Vec<(String, Json)>),
    Array(Vec<Json>),
    String(String),
    Number(String),
    Bool,
    Null,
}

/// 手工 JSON 解析器状态：输入字节与当前偏移。
struct JsonParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> JsonParser<'a> {
    /// 从字符串构造解析器。
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            offset: 0,
        }
    }
    /// 解析完整 JSON 值，不允许尾随多余数据。
    fn parse(mut self) -> Result<Json, ServerInfoError> {
        let value = self.value()?;
        self.whitespace();
        if self.offset != self.input.len() {
            return Err(ServerInfoError("trailing JSON data".into()));
        }
        Ok(value)
    }
    /// 跳过 ASCII 空白。
    fn whitespace(&mut self) {
        while self
            .input
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }
    /// 按首字节分派解析 object / string / number / bool / null。
    fn value(&mut self) -> Result<Json, ServerInfoError> {
        self.whitespace();
        match self.input.get(self.offset) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(Json::Number),
            Some(b't') => {
                self.keyword(b"true")?;
                Ok(Json::Bool)
            }
            Some(b'f') => {
                self.keyword(b"false")?;
                Ok(Json::Bool)
            }
            Some(b'n') => {
                self.keyword(b"null")?;
                Ok(Json::Null)
            }
            _ => Err(ServerInfoError("invalid JSON value".into())),
        }
    }
    /// 解析 JSON object。
    fn object(&mut self) -> Result<Json, ServerInfoError> {
        self.offset += 1;
        let mut object = Vec::new();
        self.whitespace();
        if self.input.get(self.offset) == Some(&b'}') {
            self.offset += 1;
            return Ok(Json::Object(object));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            if self.input.get(self.offset) != Some(&b':') {
                return Err(ServerInfoError("missing JSON colon".into()));
            }
            self.offset += 1;
            object.push((key, self.value()?));
            self.whitespace();
            match self.input.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    break;
                }
                _ => return Err(ServerInfoError("invalid JSON object separator".into())),
            }
        }
        Ok(Json::Object(object))
    }
    /// 解析 JSON array；未知字段也必须先通过完整语法校验。
    fn array(&mut self) -> Result<Json, ServerInfoError> {
        self.offset += 1;
        let mut array = Vec::new();
        self.whitespace();
        if self.input.get(self.offset) == Some(&b']') {
            self.offset += 1;
            return Ok(Json::Array(array));
        }
        loop {
            array.push(self.value()?);
            self.whitespace();
            match self.input.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    break;
                }
                _ => return Err(ServerInfoError("invalid JSON array separator".into())),
            }
        }
        Ok(Json::Array(array))
    }
    /// 解析 JSON 字符串（含转义与 UTF-8）。
    fn string(&mut self) -> Result<String, ServerInfoError> {
        if self.input.get(self.offset) != Some(&b'"') {
            return Err(ServerInfoError("expected JSON string".into()));
        }
        self.offset += 1;
        let mut output = String::new();
        while let Some(byte) = self.input.get(self.offset).copied() {
            self.offset += 1;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = *self
                        .input
                        .get(self.offset)
                        .ok_or_else(|| ServerInfoError("unfinished JSON escape".into()))?;
                    self.offset += 1;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let high = self.unicode_code_unit()?;
                            let character = if (0xd800..=0xdbff).contains(&high) {
                                let low_start = self.offset;
                                if self.input.get(low_start..low_start + 2) == Some(b"\\u") {
                                    self.offset += 2;
                                    let low = self.unicode_code_unit()?;
                                    if (0xdc00..=0xdfff).contains(&low) {
                                        let code = 0x10000
                                            + (((high as u32 - 0xd800) << 10)
                                                | (low as u32 - 0xdc00));
                                        char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
                                    } else {
                                        self.offset = low_start;
                                        char::REPLACEMENT_CHARACTER
                                    }
                                } else {
                                    char::REPLACEMENT_CHARACTER
                                }
                            } else if (0xdc00..=0xdfff).contains(&high) {
                                char::REPLACEMENT_CHARACTER
                            } else {
                                char::from_u32(high as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
                            };
                            output.push(character);
                        }
                        _ => return Err(ServerInfoError("invalid JSON escape".into())),
                    }
                }
                byte if byte < 0x20 => {
                    return Err(ServerInfoError("control character in JSON string".into()));
                }
                byte if byte.is_ascii() => output.push(byte as char),
                _ => {
                    self.offset -= 1;
                    let remaining = std::str::from_utf8(&self.input[self.offset..])
                        .map_err(|error| ServerInfoError(error.to_string()))?;
                    let character = remaining
                        .chars()
                        .next()
                        .ok_or_else(|| ServerInfoError("invalid UTF-8".into()))?;
                    self.offset += character.len_utf8();
                    output.push(character);
                }
            }
        }
        Err(ServerInfoError("unterminated JSON string".into()))
    }
    /// 读取一个 UTF-16 `\uXXXX` code unit。
    fn unicode_code_unit(&mut self) -> Result<u16, ServerInfoError> {
        let end = self.offset + 4;
        let digits = std::str::from_utf8(
            self.input
                .get(self.offset..end)
                .ok_or_else(|| ServerInfoError("short unicode escape".into()))?,
        )
        .map_err(|error| ServerInfoError(error.to_string()))?;
        self.offset = end;
        u16::from_str_radix(digits, 16).map_err(|error| ServerInfoError(error.to_string()))
    }
    /// 解析完整 JSON number 语法，具体整数范围在字段赋值时检查。
    fn number(&mut self) -> Result<String, ServerInfoError> {
        let start = self.offset;
        if self.input.get(self.offset) == Some(&b'-') {
            self.offset += 1;
        }
        match self.input.get(self.offset) {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
                    self.offset += 1;
                }
            }
            _ => return Err(ServerInfoError("invalid JSON number".into())),
        }
        if self.input.get(self.offset) == Some(&b'.') {
            self.offset += 1;
            let digits = self.offset;
            while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
                self.offset += 1;
            }
            if self.offset == digits {
                return Err(ServerInfoError("invalid JSON number fraction".into()));
            }
        }
        if matches!(self.input.get(self.offset), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.input.get(self.offset), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let digits = self.offset;
            while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
                self.offset += 1;
            }
            if self.offset == digits {
                return Err(ServerInfoError("invalid JSON number exponent".into()));
            }
        }
        Ok(std::str::from_utf8(&self.input[start..self.offset])
            .map_err(|error| ServerInfoError(error.to_string()))?
            .to_owned())
    }
    /// 匹配字面量关键字（true/false/null）。
    fn keyword(&mut self, keyword: &[u8]) -> Result<(), ServerInfoError> {
        if self.input.get(self.offset..self.offset + keyword.len()) != Some(keyword) {
            return Err(ServerInfoError("invalid JSON keyword".into()));
        }
        self.offset += keyword.len();
        Ok(())
    }
}

/// encoding/json 会保留最早的类型错误，同时继续尽力解码后续成员。
fn preserve_first_error(
    first_error: &mut Option<ServerInfoError>,
    result: Result<(), ServerInfoError>,
) {
    if first_error.is_none() {
        if let Err(error) = result {
            *first_error = Some(error);
        }
    }
}

/// null 对 Go 标量无影响，其余类型必须与目标字段一致。
fn assign_string(value: &Json, key: &str, target: &mut String) -> Result<(), ServerInfoError> {
    match value {
        Json::Null => Ok(()),
        Json::String(value) => {
            target.clone_from(value);
            Ok(())
        }
        _ => Err(ServerInfoError(format!("field {key} must be a string"))),
    }
}

fn assign_u32(value: &Json, key: &str, target: &mut u32) -> Result<(), ServerInfoError> {
    match value {
        Json::Null => Ok(()),
        Json::Number(value) => {
            *target = value
                .parse()
                .map_err(|error: std::num::ParseIntError| ServerInfoError(error.to_string()))?;
            Ok(())
        }
        _ => Err(ServerInfoError(format!(
            "field {key} must be an unsigned integer"
        ))),
    }
}

fn assign_u64(value: &Json, key: &str, target: &mut u64) -> Result<(), ServerInfoError> {
    match value {
        Json::Null => Ok(()),
        Json::Number(value) => {
            *target = value
                .parse()
                .map_err(|error: std::num::ParseIntError| ServerInfoError(error.to_string()))?;
            Ok(())
        }
        _ => Err(ServerInfoError(format!(
            "field {key} must be an unsigned integer"
        ))),
    }
}

fn assign_i64(value: &Json, key: &str, target: &mut i64) -> Result<(), ServerInfoError> {
    match value {
        Json::Null => Ok(()),
        Json::Number(value) => {
            *target = value
                .parse()
                .map_err(|error: std::num::ParseIntError| ServerInfoError(error.to_string()))?;
            Ok(())
        }
        _ => Err(ServerInfoError(format!("field {key} must be an integer"))),
    }
}

fn assign_labels(
    value: &Json,
    key: &str,
    target: &mut HashMap<String, String>,
) -> Result<(), ServerInfoError> {
    match value {
        Json::Null => {
            target.clear();
            Ok(())
        }
        Json::Object(labels) => {
            let mut first_error = None;
            for (label, value) in labels {
                let result = match value {
                    Json::String(value) => {
                        target.insert(label.clone(), value.clone());
                        Ok(())
                    }
                    Json::Null => {
                        target.insert(label.clone(), String::new());
                        Ok(())
                    }
                    _ => Err(ServerInfoError(format!("label {label} must be a string"))),
                };
                preserve_first_error(&mut first_error, result);
            }
            first_error.map_or(Ok(()), Err)
        }
        _ => Err(ServerInfoError(format!("field {key} must be an object"))),
    }
}
