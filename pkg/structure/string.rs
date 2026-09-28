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

// 事务内字符串（String）数据结构操作。
//
// 在 `TxStructure` 上提供类似 Redis String 的 Set/Get/Inc/Iterate/Clear，
// 键经 `EncodeStringDataKey` 编码后写入底层 KV（键值存储）。

impl TxStructure {
    /// 设置键对应的字符串值。
    // Set sets the string value of the key.
    pub fn Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), errors::SharedError> {
        let encoded = self.EncodeStringDataKey(key);
        self.writer()?.Set(encoded, value.to_vec())
    }

    /// 读取键对应的字符串值；不存在时返回 `None`。
    // Get gets the string value of a key.
    pub fn Get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, errors::SharedError> {
        let encoded = self.EncodeStringDataKey(key);
        match kv::GetValue(&kv::Context::todo(), self.reader.as_ref(), encoded) {
            Ok(value) => Ok(Some(value)),
            Err(error) if kv::IsErrNotFound(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 将键的值按十进制解析为 `i64`；键不存在时返回 0。
    // GetInt64 gets the int64 value of a key.
    pub fn GetInt64(&self, key: &[u8]) -> Result<i64, errors::SharedError> {
        let Some(value) = self.Get(key)? else {
            return Ok(0);
        };
        let text = std::str::from_utf8(&value).map_err(|error| errors::New(error.to_string()))?;
        text.parse::<i64>()
            .map_err(|error| errors::New(error.to_string()))
    }

    /// 将键的整数值按 `step` 自增，返回自增后的值。
    // Inc increments the integer value of a key by step, returns the value after the increment.
    pub fn Inc(&mut self, key: &[u8], step: i64) -> Result<i64, errors::SharedError> {
        let encoded = self.EncodeStringDataKey(key);
        kv::IncInt64(self.writer()?, &encoded, step)
    }

    /// 在同一前缀下迭代 `[key, upperBound)` 区间内的全部字符串键值对。
    // Iterate iterates all keys in the same prefix.
    pub fn Iterate<F>(
        &self,
        key: &[u8],
        upperBound: &[u8],
        mut function: F,
    ) -> Result<(), errors::SharedError>
    where
        F: FnMut(&[u8], &[u8]) -> Result<(), errors::SharedError>,
    {
        // 将业务键编码为 KV 范围上下界后扫描，解码后再回调。
        let lower = self.EncodeStringDataKey(key);
        let upper = self.EncodeStringDataKey(upperBound);
        let mut iterator = self.reader.Iter(lower, Some(upper))?;
        let result = (|| {
            while iterator.Valid() {
                let decoded = self.decodeStringDataKey(iterator.Key())?;
                let value = iterator.Value();
                function(&decoded, &value)?;
                iterator.Next()?;
            }
            Ok(())
        })();
        iterator.Close();
        result
    }

    /// 删除键对应的字符串值；键不存在时视为成功。
    // Clear removes the string value of the key.
    pub fn Clear(&mut self, key: &[u8]) -> Result<(), errors::SharedError> {
        let encoded = self.EncodeStringDataKey(key);
        match self.writer()?.Delete(encoded) {
            Err(error) if kv::IsErrNotFound(&error) => Ok(()),
            result => result,
        }
    }
}
