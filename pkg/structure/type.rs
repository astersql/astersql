// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 数据结构类型标志与 KV 键编码/解码。
//
// 每种结构用大写标志表示元数据（meta）、小写表示数据（data）；编码格式为
// `prefix + EncodeBytes(key) + EncodeUint(flag) [+ field/index]`，保证同前缀下
// 可按类型扫描并正确区分 string/hash/list。

/// 数据结构元数据/数据键上的类型标志字节。
// TypeFlag is for data structure meta/data flag.
pub type TypeFlag = u8;

/// 字符串元数据标志（大写 S）。
pub const StringMeta: TypeFlag = b'S';
/// 字符串数据标志（小写 s）。
pub const StringData: TypeFlag = b's';
/// 哈希元数据标志（大写 H）。
pub const HashMeta: TypeFlag = b'H';
/// 哈希数据标志（小写 h）。
pub const HashData: TypeFlag = b'h';
/// 列表元数据标志（大写 L）。
pub const ListMeta: TypeFlag = b'L';
/// 列表数据标志（小写 l）。
pub const ListData: TypeFlag = b'l';

impl TxStructure {
    /// 编码字符串数据键：`prefix + key + StringData`。
    // EncodeStringDataKey will encode string key.
    pub fn EncodeStringDataKey(&self, key: &[u8]) -> kv::Key {
        let mut encoded = Vec::with_capacity(self.prefix.len() + key.len() + 24);
        encoded.extend_from_slice(&self.prefix);
        encoded = codec::EncodeBytes(encoded, key);
        kv::Key(codec::EncodeUint(encoded, StringData as u64))
    }

    /// 解码字符串数据键，校验前缀与 `StringData` 标志后返回业务键。
    pub fn decodeStringDataKey(&self, encoded: kv::Key) -> Result<Vec<u8>, errors::SharedError> {
        if !encoded.0.starts_with(&self.prefix) {
            return Err(errors::New("invalid encoded hash data key prefix"));
        }
        let (remaining, key) = codec::DecodeBytes(&encoded.0[self.prefix.len()..], None)
            .map_err(|error| errors::New(error.to_string()))?;
        let (_, flag) =
            codec::DecodeUint(remaining).map_err(|error| errors::New(error.to_string()))?;
        if flag as TypeFlag != StringData {
            return Err(ErrInvalidHashKeyFlag.GenWithStack(
                &format!(
                    "invalid encoded string data key flag {}",
                    flag as u8 as char
                ),
                &[],
            ));
        }
        Ok(key)
    }

    /// 编码哈希元数据键（导出供测试；v5.1 及更早版本使用）。
    // EncodeHashMetaKey exports for tests. It's used in version v5.1 and earlier.
    pub fn EncodeHashMetaKey(&self, key: &[u8]) -> kv::Key {
        let mut encoded =
            Vec::with_capacity(self.prefix.len() + codec::EncodedBytesLength(key.len()) + 8);
        encoded.extend_from_slice(&self.prefix);
        encoded = codec::EncodeBytes(encoded, key);
        kv::Key(codec::EncodeUint(encoded, HashMeta as u64))
    }

    /// 编码哈希字段数据键：`prefix + key + HashData + field`。
    pub(crate) fn encodeHashDataKey(&self, key: &[u8], field: &[u8]) -> kv::Key {
        let mut encoded = Vec::with_capacity(
            self.prefix.len()
                + codec::EncodedBytesLength(key.len())
                + 8
                + codec::EncodedBytesLength(field.len()),
        );
        encoded.extend_from_slice(&self.prefix);
        encoded = codec::EncodeBytes(encoded, key);
        encoded = codec::EncodeUint(encoded, HashData as u64);
        kv::Key(codec::EncodeBytes(encoded, field))
    }

    /// 编码哈希数据键（导出供测试）。
    // EncodeHashDataKey exports for tests.
    pub fn EncodeHashDataKey(&self, key: &[u8], field: &[u8]) -> kv::Key {
        self.encodeHashDataKey(key, field)
    }

    /// 解码哈希数据键，返回 `(业务键, 字段名)`。
    pub fn decodeHashDataKey(
        &self,
        encoded: kv::Key,
    ) -> Result<(Vec<u8>, Vec<u8>), errors::SharedError> {
        if !encoded.0.starts_with(&self.prefix) {
            return Err(errors::New("invalid encoded hash data key prefix"));
        }
        // 依次解码：业务键 → 类型标志 → 字段名。
        let (remaining, key) = codec::DecodeBytes(&encoded.0[self.prefix.len()..], None)
            .map_err(|error| errors::New(error.to_string()))?;
        let (remaining, flag) =
            codec::DecodeUint(remaining).map_err(|error| errors::New(error.to_string()))?;
        if flag as TypeFlag != HashData {
            return Err(ErrInvalidHashKeyFlag.GenWithStack(
                &format!("invalid encoded hash data key flag {}", flag as u8 as char),
                &[],
            ));
        }
        let (_, field) =
            codec::DecodeBytes(remaining, None).map_err(|error| errors::New(error.to_string()))?;
        Ok((key, field))
    }

    /// 构造某哈希键下所有字段数据的公共前缀（不含 field）。
    pub(crate) fn hashDataKeyPrefix(&self, key: &[u8]) -> kv::Key {
        let mut encoded = Vec::with_capacity(self.prefix.len() + key.len() + 24);
        encoded.extend_from_slice(&self.prefix);
        encoded = codec::EncodeBytes(encoded, key);
        kv::Key(codec::EncodeUint(encoded, HashData as u64))
    }

    /// 编码列表元数据键。
    pub(crate) fn encodeListMetaKey(&self, key: &[u8]) -> kv::Key {
        let mut encoded = Vec::with_capacity(self.prefix.len() + key.len() + 24);
        encoded.extend_from_slice(&self.prefix);
        encoded = codec::EncodeBytes(encoded, key);
        kv::Key(codec::EncodeUint(encoded, ListMeta as u64))
    }

    /// 编码列表元素数据键：在标志后附加元素下标。
    pub(crate) fn encodeListDataKey(&self, key: &[u8], index: i64) -> kv::Key {
        let mut encoded = Vec::with_capacity(self.prefix.len() + key.len() + 36);
        encoded.extend_from_slice(&self.prefix);
        encoded = codec::EncodeBytes(encoded, key);
        encoded = codec::EncodeUint(encoded, ListData as u64);
        kv::Key(codec::EncodeInt(encoded, index))
    }
}
