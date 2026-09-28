// Copyright 2026 AsterSQL.

// `meta` 测试与编译用依赖桩（harness）。
//
// 为 meta 核心提供内存版 kv/structure/model 等最小实现，使 Mutator 能在无真实 TiKV 环境下
// 验证事务语义。含错误类型、大小写不敏感字符串、表/库模型、上下文取消，以及 JSON/编解码辅助。

/// 简化错误类型：用 anyhow 承载消息，对应 Go 侧 errors 包装。
pub mod errors {
    /// 统一错误别名。
    pub type Error = anyhow::Error;

    /// 由显示消息构造错误。
    pub fn new(message: impl std::fmt::Display) -> Error {
        anyhow::anyhow!(message.to_string())
    }

    /// 将任意可转换错误提升为本模块 Error（保留因果链入口名）。
    pub fn trace<E>(error: E) -> Error
    where
        E: Into<Error>,
    {
        error.into()
    }

    /// 从其他错误类型转换。
    pub fn from<E>(error: E) -> Error
    where
        E: Into<Error>,
    {
        error.into()
    }
}

/// AST 相关轻量类型：大小写不敏感标识符等。
pub mod ast {
    use serde::{Deserialize, Deserializer, Serialize};

    /// 资源组中等优先级常量（与 Go ast.MediumPriorityValue 对齐）。
    pub const MEDIUM_PRIORITY_VALUE: u64 = 8;

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
    /// 大小写不敏感字符串：保留原文 `original` 与小写 `lower`。
    pub struct CiString {
        #[serde(rename = "O")]
        pub original: String,
        #[serde(rename = "L")]
        pub lower: String,
    }

    impl CiString {
        /// 由任意字符串构造，自动生成小写副本。
        pub fn new(value: impl Into<String>) -> Self {
            let original = value.into();
            let lower = original.to_lowercase();
            Self { original, lower }
        }
    }

    impl<'de> Deserialize<'de> for CiString {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[derive(Deserialize)]
            struct Fields {
                #[serde(default, rename = "O")]
                original: String,
                #[serde(default, rename = "L")]
                lower: String,
            }

            #[derive(Deserialize)]
            #[serde(untagged)]
            enum Representation {
                Fields(Fields),
                String(String),
            }

            match Representation::deserialize(deserializer)? {
                Representation::Fields(fields) => Ok(Self {
                    original: fields.original,
                    lower: fields.lower,
                }),
                Representation::String(value) => Ok(Self::new(value)),
            }
        }
    }
}

/// 元数据模型桩：库/表/策略/资源组/DDL Job 等序列化结构。
pub mod model {
    use serde::{Deserialize, Serialize};

    use crate::{ast, errors};

    /// 表信息版本 5：从此版本起 RowID 与 AUTO_INCREMENT 使用分离 meta field。
    pub const TABLE_INFO_VERSION_5: u16 = 5;

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// 一张表的三类 AutoID 水位：行 ID、自增 ID、随机 ID。
    pub struct AutoIdGroup {
        #[serde(rename = "RowID")]
        pub row_id: i64,
        #[serde(rename = "IncrementID")]
        pub increment_id: i64,
        #[serde(rename = "RandomID")]
        pub random_id: i64,
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// 数据库元信息。
    pub struct DbInfo {
        pub id: i64,
        #[serde(rename = "db_name", alias = "name")]
        pub name: ast::CiString,
        pub charset: String,
        pub collate: String,
    }

    impl DbInfo {
        /// 构造公开系统库描述。
        pub fn public_system(id: i64, name: &str, charset: &str, collate: &str) -> Self {
            Self {
                id,
                name: ast::CiString::new(name),
                charset: charset.to_owned(),
                collate: collate.to_owned(),
            }
        }
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// 表元信息（测试用精简字段集）。
    pub struct TableInfo {
        pub id: i64,
        #[serde(default)]
        pub name: ast::CiString,
        #[serde(default, skip_serializing)]
        pub db_id: i64,
        #[serde(default)]
        pub revision: u64,
        #[serde(default)]
        pub auto_random_bits: u64,
        #[serde(default)]
        pub version: u16,
        #[serde(default)]
        pub has_auto_increment_column: bool,
    }

    impl TableInfo {
        /// 是否已分离 AUTO_INCREMENT 与 RowID 字段。
        pub fn sep_auto_inc(&self) -> bool {
            self.version >= TABLE_INFO_VERSION_5
        }

        /// 是否存在 AUTO_INCREMENT 列（桩实现仅返回存在性）。
        pub fn get_auto_increment_col_info(&self) -> Option<()> {
            self.has_auto_increment_column.then_some(())
        }
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// 仅含 id 与名称的轻量表信息。
    pub struct TableNameInfo {
        pub id: i64,
        pub name: ast::CiString,
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// Placement Policy（放置策略）元信息。
    pub struct PolicyInfo {
        pub id: i64,
        pub name: ast::CiString,
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// Masking Policy（脱敏策略）元信息。
    pub struct MaskingPolicyInfo {
        pub id: i64,
        pub name: ast::CiString,
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// Resource Group（资源组）：RU 配额与优先级。
    pub struct ResourceGroupInfo {
        pub id: i64,
        pub name: ast::CiString,
        pub ru_per_sec: u64,
        pub burst_limit: i64,
        pub priority: u64,
    }

    impl ResourceGroupInfo {
        /// 构造默认公开资源组。
        pub fn public_default(
            id: i64,
            name: &str,
            ru_per_sec: i32,
            burst_limit: i32,
            priority: u64,
        ) -> Self {
            Self {
                id,
                name: ast::CiString::new(name),
                ru_per_sec: ru_per_sec as u64,
                burst_limit: burst_limit as i64,
                priority,
            }
        }
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// Schema 变更差异记录（按版本号索引）。
    pub struct SchemaDiff {
        pub version: i64,
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    /// DDL Job 历史条目。
    pub struct Job {
        pub id: i64,
        pub schema_name: String,
        pub table_name: String,
        #[serde(default)]
        pub raw_args: Vec<serde_json::Value>,
    }

    impl Job {
        /// 序列化为 JSON 字节。
        pub fn encode(&self, _update_raw_args: bool) -> Result<Vec<u8>, errors::Error> {
            Ok(serde_json::to_vec(self)?)
        }

        /// 从 JSON 字节反序列化。
        pub fn decode(data: &[u8]) -> Result<Self, errors::Error> {
            Ok(serde_json::from_slice(data)?)
        }
    }
}

/// 可取消的上下文桩，对应 Go context.Context 的取消检查。
pub mod context {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use crate::errors;

    #[derive(Clone, Debug, Default)]
    /// 带取消标志的上下文。
    pub struct Context {
        cancelled: Arc<AtomicBool>,
    }

    impl Context {
        /// 标记上下文已取消。
        pub fn cancel(&self) {
            self.cancelled.store(true, Ordering::Relaxed);
        }

        /// 若已取消则返回错误，否则 Ok。
        pub fn check_error(&self) -> Result<(), errors::Error> {
            if self.cancelled.load(Ordering::Relaxed) {
                Err(errors::new("context canceled"))
            } else {
                Ok(())
            }
        }
    }
}

/// 内存 KV 事务/快照/存储桩，供 structure 绑定共享 State。
pub mod kv {
    use std::sync::{Arc, Mutex};

    use crate::structure::State;

    #[derive(Clone, Debug, Default)]
    /// 内存事务：持有共享 State 与 start_ts（事务开始时间戳）。
    pub struct Transaction {
        pub(crate) state: Arc<Mutex<State>>,
        pub(crate) start_ts: u64,
    }

    impl Transaction {
        /// 设置事务优先级（桩为空操作）。
        pub fn set_option(&mut self, _priority: Priority) {}
        /// 磁盘满策略（桩为空操作）。
        pub fn set_disk_full_option(&mut self, _option: DiskFullOption) {}
        /// 返回事务开始时间戳。
        pub fn start_ts(&self) -> u64 {
            self.start_ts
        }
        /// 基于同一 State 创建只读快照视图。
        pub fn snapshot(&self) -> Snapshot {
            Snapshot {
                state: self.state.clone(),
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    /// 只读快照，共享底层 State。
    pub struct Snapshot {
        pub(crate) state: Arc<Mutex<State>>,
    }

    impl Snapshot {
        /// 快照选项设置（桩）。
        pub fn set_option<K, V>(&mut self, _key: K, _value: V) {}
    }

    #[derive(Clone, Debug, Default)]
    /// 存储入口：可按 start_ts 取快照。
    pub struct Storage {
        pub(crate) state: Arc<Mutex<State>>,
    }

    impl Storage {
        /// 返回绑定同一 State 的快照。
        pub fn get_snapshot(&self, _start_ts: u64) -> Snapshot {
            Snapshot {
                state: self.state.clone(),
            }
        }
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    /// 编码后的键字节包装。
    pub struct Key(pub Vec<u8>);

    #[derive(Clone, Copy, Debug)]
    /// 事务优先级。
    pub enum Priority {
        High,
    }

    #[derive(Clone, Copy, Debug)]
    /// 磁盘接近满载时是否仍允许写入。
    pub enum DiskFullOption {
        AllowedOnAlmostFull,
    }

    /// 内部请求来源标记常量（与 Go kv 包对齐）。
    pub const RequestSourceInternal: u8 = 1;
    pub const RequestSourceType: u8 = 2;
    pub const InternalTxnMeta: u8 = 3;
}

/// 类 Redis 的 string/hash 结构层：在共享 State 上实现 meta 所需的 HSET/HGET/INC 等。
pub mod structure {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use crate::{errors, kv};

    /// string 数据类型标记。
    pub const STRING_DATA: u8 = b's';
    const HASH_DATA: u8 = b'h';

    #[derive(Debug, Default)]
    /// 内存状态：string 映射与 hash 嵌套映射。
    pub struct State {
        strings: BTreeMap<(Vec<u8>, Vec<u8>), Vec<u8>>,
        hashes: BTreeMap<(Vec<u8>, Vec<u8>), BTreeMap<Vec<u8>, Vec<u8>>>,
    }

    #[derive(Clone, Debug)]
    /// 带前缀的事务结构访问器；所有键写入前会加 prefix。
    pub struct TxStructure {
        state: Arc<Mutex<State>>,
        prefix: Vec<u8>,
        read_only: bool,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    /// hash 的 field-value 对。
    pub struct HashPair {
        pub field: Vec<u8>,
        pub value: Vec<u8>,
    }

    /// 从事务创建带前缀的结构访问器。
    pub fn new_structure(txn: kv::Transaction, prefix: &[u8]) -> TxStructure {
        TxStructure {
            state: txn.state,
            prefix: prefix.to_vec(),
            read_only: false,
        }
    }

    /// 从快照创建带前缀的结构访问器。
    pub fn new_snapshot_structure(snapshot: kv::Snapshot, prefix: &[u8]) -> TxStructure {
        TxStructure {
            state: snapshot.state,
            prefix: prefix.to_vec(),
            read_only: true,
        }
    }

    impl TxStructure {
        fn ensure_writable(&self) -> Result<(), errors::Error> {
            if self.read_only {
                Err(errors::new("write on snapshot"))
            } else {
                Ok(())
            }
        }

        fn state_key(&self, key: &[u8]) -> (Vec<u8>, Vec<u8>) {
            (self.prefix.clone(), key.to_vec())
        }

        /// 将 string 键按十进制 int64 自增并返回新值。
        pub fn inc(&self, key: &[u8], step: i64) -> Result<i64, errors::Error> {
            self.ensure_writable()?;
            let mut state = self.state.lock().unwrap();
            let state_key = self.state_key(key);
            let current = match state.strings.get(&state_key) {
                None => 0,
                Some(value) => std::str::from_utf8(value)?.parse::<i64>()?,
            };
            let next = current.wrapping_add(step);
            state
                .strings
                .insert(state_key, next.to_string().into_bytes());
            Ok(next)
        }

        /// 读取 string 键为 int64；缺失或空视为 0。
        pub fn get_i64(&self, key: &[u8]) -> Result<i64, errors::Error> {
            match self.get(key)? {
                None => Ok(0),
                Some(value) => Ok(std::str::from_utf8(&value)?.parse()?),
            }
        }

        /// 写入 string 键。
        pub fn set(&self, key: &[u8], value: &[u8]) -> Result<(), errors::Error> {
            self.ensure_writable()?;
            self.state
                .lock()
                .unwrap()
                .strings
                .insert(self.state_key(key), value.to_vec());
            Ok(())
        }

        /// 读取 string 键。
        pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, errors::Error> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .strings
                .get(&self.state_key(key))
                .cloned())
        }

        /// 删除 string 键。
        pub fn clear(&self, key: &[u8]) -> Result<(), errors::Error> {
            self.ensure_writable()?;
            self.state
                .lock()
                .unwrap()
                .strings
                .remove(&self.state_key(key));
            Ok(())
        }

        /// 写入 hash field。
        pub fn hset(&self, hash: &[u8], field: &[u8], value: &[u8]) -> Result<(), errors::Error> {
            self.ensure_writable()?;
            self.state
                .lock()
                .unwrap()
                .hashes
                .entry(self.state_key(hash))
                .or_default()
                .insert(field.to_vec(), value.to_vec());
            Ok(())
        }

        /// 读取 hash field。
        pub fn hget(&self, hash: &[u8], field: &[u8]) -> Result<Option<Vec<u8>>, errors::Error> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .hashes
                .get(&self.state_key(hash))
                .and_then(|h| h.get(field))
                .cloned())
        }

        /// 将 hash field 解析为 int64；缺失视为 0。
        pub fn hget_i64(&self, hash: &[u8], field: &[u8]) -> Result<i64, errors::Error> {
            match self.hget(hash, field)? {
                None => Ok(0),
                Some(value) => Ok(std::str::from_utf8(&value)?.parse()?),
            }
        }

        /// hash field 十进制自增。
        pub fn hinc(&self, hash: &[u8], field: &[u8], step: i64) -> Result<i64, errors::Error> {
            self.ensure_writable()?;
            let mut state = self.state.lock().unwrap();
            let fields = state.hashes.entry(self.state_key(hash)).or_default();
            let current = match fields.get(field) {
                None => 0,
                Some(value) => std::str::from_utf8(value)?.parse::<i64>()?,
            };
            let next = current.wrapping_add(step);
            fields.insert(field.to_vec(), next.to_string().into_bytes());
            Ok(next)
        }

        /// 删除 hash field。
        pub fn hdel(&self, hash: &[u8], field: &[u8]) -> Result<(), errors::Error> {
            self.ensure_writable()?;
            if let Some(fields) = self
                .state
                .lock()
                .unwrap()
                .hashes
                .get_mut(&self.state_key(hash))
            {
                fields.remove(field);
            }
            Ok(())
        }

        /// 删除整个 hash。
        pub fn hclear(&self, hash: &[u8]) -> Result<(), errors::Error> {
            self.ensure_writable()?;
            self.state
                .lock()
                .unwrap()
                .hashes
                .remove(&self.state_key(hash));
            Ok(())
        }

        /// 返回 hash 全部 field-value。
        pub fn hget_all(&self, hash: &[u8]) -> Result<Vec<HashPair>, errors::Error> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .hashes
                .get(&self.state_key(hash))
                .into_iter()
                .flat_map(|fields| {
                    fields.iter().map(|(field, value)| HashPair {
                        field: field.clone(),
                        value: value.clone(),
                    })
                })
                .collect())
        }

        /// hash field 数量。
        pub fn hget_len(&self, hash: &[u8]) -> Result<u64, errors::Error> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .hashes
                .get(&self.state_key(hash))
                .map_or(0, |h| h.len() as u64))
        }

        /// 迭代 hash 全部 pair 并调用 visit。
        pub fn hget_iter<F>(&self, hash: &[u8], mut visit: F) -> Result<(), errors::Error>
        where
            F: FnMut(HashPair) -> Result<(), errors::Error>,
        {
            for pair in self.hget_all(hash)? {
                visit(pair)?;
            }
            Ok(())
        }

        /// 以 (field, value) 回调形式迭代 hash。
        pub fn iterate_hash<F>(&self, hash: &[u8], mut visit: F) -> Result<(), errors::Error>
        where
            F: FnMut(&[u8], &[u8]) -> Result<(), errors::Error>,
        {
            for pair in self.hget_all(hash)? {
                visit(&pair.field, &pair.value)?;
            }
            Ok(())
        }

        /// 按 hash 键字节序在 [start, end) 范围内迭代所有 hash。
        pub fn iterate_hash_bounded<F>(
            &self,
            start: Vec<u8>,
            end: Vec<u8>,
            mut visit: F,
        ) -> Result<(), errors::Error>
        where
            F: FnMut(&[u8], &[u8], &[u8]) -> Result<(), errors::Error>,
        {
            let entries: Vec<_> = self
                .state
                .lock()
                .unwrap()
                .hashes
                .iter()
                .filter(|((prefix, hash), _)| {
                    prefix == &self.prefix
                        && hash.as_slice() >= start.as_slice()
                        && hash.as_slice() < end.as_slice()
                })
                .flat_map(|((_, hash), fields)| {
                    fields
                        .iter()
                        .map(|(field, value)| (hash.clone(), field.clone(), value.clone()))
                })
                .collect();
            for (hash, field, value) in entries {
                visit(&hash, &field, &value)?;
            }
            Ok(())
        }

        /// 编码带前缀的 string 数据键。
        pub fn encode_string_data_key(&self, key: &[u8]) -> kv::Key {
            let mut out = self.prefix.clone();
            out = crate::codec::encode_bytes(&out, key);
            out = crate::codec::encode_uint(&out, STRING_DATA as u64);
            kv::Key(out)
        }

        /// 编码带前缀的 hash 数据键（EncodeBytes(hash) + HashData + EncodeBytes(field)）。
        pub fn encode_hash_data_key(&self, hash: &[u8], field: &[u8]) -> Vec<u8> {
            let mut out = self.prefix.clone();
            out = crate::codec::encode_bytes(&out, hash);
            out = crate::codec::encode_uint(&out, HASH_DATA as u64);
            crate::codec::encode_bytes(&out, field)
        }

        /// 编码 AutoID 的 hash 键与十进制值字节对。
        pub fn encode_hash_auto_id_key_value(
            &self,
            hash: &[u8],
            field: &[u8],
            value: i64,
        ) -> (Vec<u8>, Vec<u8>) {
            (
                self.encode_hash_data_key(hash, field),
                value.to_string().into_bytes(),
            )
        }
    }

    #[derive(Clone, Debug, Default)]
    /// 反向遍历 hash value 的迭代器。
    pub struct ReverseHashIterator {
        values: Vec<Vec<u8>>,
        index: usize,
    }

    impl ReverseHashIterator {
        /// 当前位置是否有效。
        pub fn valid(&self) -> bool {
            self.index < self.values.len()
        }
        /// 当前迭代到的 value 字节。
        pub fn value(&self) -> &[u8] {
            &self.values[self.index]
        }
        /// 前进到下一 value。
        pub fn next(&mut self) -> Result<(), errors::Error> {
            self.index += 1;
            Ok(())
        }
    }

    /// 创建从末尾开始的反向 hash 值迭代器。
    pub fn new_hash_reverse_iter(
        txn: &TxStructure,
        hash: &[u8],
    ) -> Result<ReverseHashIterator, errors::Error> {
        let mut pairs = txn.hget_all(hash)?;
        pairs.reverse();
        Ok(ReverseHashIterator {
            values: pairs.into_iter().map(|p| p.value).collect(),
            index: 0,
        })
    }

    /// 从 start field 起（含）反向迭代 hash 值。
    pub fn new_hash_reverse_iter_from(
        txn: &TxStructure,
        hash: &[u8],
        start: &[u8],
    ) -> Result<ReverseHashIterator, errors::Error> {
        let mut pairs = txn.hget_all(hash)?;
        if !start.is_empty() {
            pairs.retain(|pair| pair.field.as_slice() <= start);
        }
        pairs.reverse();
        Ok(ReverseHashIterator {
            values: pairs.into_iter().map(|p| p.value).collect(),
            index: 0,
        })
    }
}

/// JSON 序列化/反序列化薄封装。
pub mod json {
    use crate::errors;
    use serde::{Serialize, de::DeserializeOwned};

    /// 序列化为 JSON 字节。
    pub fn marshal<T: Serialize>(value: &T) -> Result<Vec<u8>, errors::Error> {
        Ok(serde_json::to_vec(value)?)
    }
    /// 从 JSON 字节反序列化。
    pub fn unmarshal<T: DeserializeOwned>(data: &[u8]) -> Result<T, errors::Error> {
        Ok(serde_json::from_slice(data)?)
    }
}

/// 字节切片辅助。
pub mod bytes {
    /// 在 haystack 中查找 needle 首次出现位置。
    pub fn index(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}

/// Go 兼容键编码：memcomparable 字节与大端无符号整数。
pub mod codec {
    /// 按 Go codec.EncodeBytes 的 8 字节分组 memcomparable 格式追加字节值。
    pub fn encode_bytes(prefix: &[u8], value: &[u8]) -> Vec<u8> {
        let mut out = prefix.to_vec();
        let mut offset = 0;
        while offset <= value.len() {
            let remaining = value.len() - offset;
            let take = remaining.min(8);
            out.extend_from_slice(&value[offset..offset + take]);
            let padding = 8 - take;
            out.resize(out.len() + padding, 0);
            out.push(0xff - padding as u8);
            offset += 8;
        }
        out
    }
    /// 前缀后追加大端 u64。
    pub fn encode_uint(prefix: &[u8], value: u64) -> Vec<u8> {
        let mut out = prefix.to_vec();
        out.extend_from_slice(&value.to_be_bytes());
        out
    }
}

/// 部分 JSON 解析：只提取 id/name 等顶层字段，避免完整反序列化 TableInfo。
pub mod partialjson {
    use crate::errors;

    /// 从 JSON 提取 `id` 与 `name.O`（原始大小写名）。
    pub fn extract_id_and_original_name(
        data: &[u8],
        _pattern: &str,
    ) -> Result<(i64, String), errors::Error> {
        let value: serde_json::Value = serde_json::from_slice(data)?;
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| errors::new("missing id"))?;
        let name = value
            .get("name")
            .and_then(|name| name.get("O"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| errors::new("missing original name"))?;
        Ok((id, name.to_owned()))
    }

    /// 顶层 JSON 成员访问器。
    pub struct TopLevelMembers(serde_json::Value);

    /// 解析整个 JSON 为 TopLevelMembers（fields 参数保留以对齐 Go 签名）。
    pub fn extract_top_level_members(
        data: &[u8],
        _fields: &[&str],
    ) -> Result<TopLevelMembers, errors::Error> {
        Ok(TopLevelMembers(serde_json::from_slice(data)?))
    }

    impl TopLevelMembers {
        /// 读取顶层 int64 字段。
        pub fn single_i64(&self, field: &str) -> Result<i64, errors::Error> {
            self.0
                .get(field)
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| errors::new(format!("missing {field}")))
        }
        /// 读取顶层字符串字段。
        pub fn single_string(&self, field: &str) -> Result<String, errors::Error> {
            self.0
                .get(field)
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| errors::new(format!("missing {field}")))
        }
        /// 读取 `field.O` 原始名称。
        pub fn case_insensitive_original_name(&self, field: &str) -> Result<String, errors::Error> {
            self.0
                .get(field)
                .and_then(|value| value.get("O"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| errors::new(format!("missing {field}.O")))
        }
    }
}

/// 异步运行时桩：spawn / try_join_all 的最小实现。
pub mod runtime {
    use std::future::Future;

    /// 桩：直接返回 future，不做实际调度。
    pub fn spawn<F: Future>(future: F) -> F {
        future
    }

    /// 等待全部 future 成功完成。
    pub async fn try_join_all<I>(futures: I) -> Result<Vec<()>, crate::errors::Error>
    where
        I: IntoIterator,
        I::Item: Future<Output = Result<(), crate::errors::Error>>,
    {
        futures::future::try_join_all(futures).await
    }
}

/// meta 操作直方图桩（观察调用为空操作）。
pub mod metrics {
    /// 直方图桩类型。
    pub struct Histogram;
    /// 全局 meta 直方图实例。
    pub static META_HISTOGRAM: Histogram = Histogram;
    impl Histogram {
        /// 观察获取 DDL 历史耗时（空操作）。
        pub fn observe_get_history<T, E>(&self, _result: Result<&T, &E>, _elapsed: u64) {}
        /// 观察读取 SchemaDiff 耗时（空操作）。
        pub fn observe_get_schema_diff<T, E>(&self, _result: Result<&T, &E>, _elapsed: u64) {}
        /// 观察写入 SchemaDiff 耗时（空操作）。
        pub fn observe_set_schema_diff<T, E>(&self, _result: Result<&T, &E>, _elapsed: u64) {}
    }
}

/// 时间桩：固定返回 0，避免测试依赖真实时钟。
pub mod time {
    /// 时间戳别名。
    pub type Time = u64;
    /// 当前时间（桩恒为 0）。
    pub fn now() -> u64 {
        0
    }
    /// 距 start 的时长（桩恒为 0）。
    pub fn since(_start: u64) -> u64 {
        0
    }
}

/// DXF/TTL 调度状态相关类型桩。
pub mod schstatus {
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    /// TTL 调度放大因子。
    pub struct TtlTuneFactors {
        pub amplify_factor: f64,
    }
}

/// 资源管理 protobuf 相关消费统计桩。
pub mod rmpb {
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    /// RRU/WRU 消费量。
    pub struct Consumption {
        pub rru: f64,
        pub wru: f64,
    }
}

/// MVCC 查询 helper 桩（返回预设 response）。
pub mod helper {
    use crate::errors;

    #[derive(Clone, Debug, Default)]
    /// MVCC 写记录短值。
    pub struct Write {
        pub short_value: Vec<u8>,
    }
    #[derive(Clone, Debug, Default)]
    /// MVCC 信息：写记录列表。
    pub struct MvccInfo {
        pub writes: Vec<Write>,
    }
    #[derive(Clone, Debug, Default)]
    /// MVCC 查询响应。
    pub struct MvccResponse {
        pub info: Option<MvccInfo>,
    }
    #[derive(Clone, Debug, Default)]
    /// 可注入预设响应的 helper。
    pub struct Helper {
        pub response: Option<MvccResponse>,
    }
    impl Helper {
        /// 按编码键与时间戳查询 MVCC（返回克隆的预设值）。
        pub fn get_mvcc_by_encoded_key_with_ts(
            &self,
            _key: &[u8],
            _ts: u64,
        ) -> Result<Option<MvccResponse>, errors::Error> {
            Ok(self.response.clone())
        }
    }
}

/// 元数据相关常量定义桩。
pub mod metadef {
    /// 用户可用全局 ID 上限（为系统预留尾段）。
    pub const MAX_USER_GLOBAL_ID: i64 = 0x0000_FFFF_FFFF_FFFF - 1000;
    /// 系统库固定 ID。
    pub const SYSTEM_DATABASE_ID: i64 = 0x0000_FFFF_FFFF_FFFF;
}

/// 资源组名称常量。
pub mod resourcegroup {
    /// 默认资源组名。
    pub const DEFAULT_RESOURCE_GROUP_NAME: &str = "default";
}

/// MySQL 兼容常量：系统库名与 UTF8MB4 默认字符集/排序规则。
pub mod mysql {
    pub const SYSTEM_DB: &str = "mysql";
    pub const UTF8MB4_CHARSET: &str = "utf8mb4";
    pub const UTF8MB4_DEFAULT_COLLATION: &str = "utf8mb4_bin";
}

/// 内核类型判定桩。
pub mod kerneltype {
    /// 是否 nextgen 内核（桩恒为 false，即 classic）。
    pub fn is_next_gen() -> bool {
        false
    }
}
