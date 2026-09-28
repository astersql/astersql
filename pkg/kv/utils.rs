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

// KV 层常用工具函数：整型读写、遍历内存缓冲，以及 nextgen keyspace 判定。

use crate::{Context, Error, IsErrNotFound, Key, Retriever, RetrieverMutator, Storage, errors};

/// 将 key 对应的十进制整数字节加 step 后写回；键不存在时写入 step。
///
/// 使用 wrapping_add，与 Go 侧溢出回绕语义一致。
pub fn IncInt64(rm: &mut dyn RetrieverMutator, key: &Key, step: i64) -> Result<i64, Error> {
    let value = match rm.Get(&Context::todo(), key.clone(), &[]) {
        Ok(value) => value,
        Err(error) if IsErrNotFound(&error) => {
            // 键不存在：直接以 step 作为初值写入。
            rm.Set(key.clone(), step.to_string().into_bytes())?;
            return Ok(step);
        }
        Err(error) => return Err(error),
    };

    let text = std::str::from_utf8(&value.Value).map_err(|error| errors::New(error.to_string()))?;
    let int_value = text
        .parse::<i64>()
        .map_err(|error| errors::New(error.to_string()))?
        .wrapping_add(step);
    rm.Set(key.clone(), int_value.to_string().into_bytes())?;
    Ok(int_value)
}

/// 读取 key 对应的十进制整数；键不存在时返回 0。
pub fn GetInt64(ctx: &Context, retriever: &dyn Retriever, key: &Key) -> Result<i64, Error> {
    let value = match retriever.Get(ctx, key.clone(), &[]) {
        Ok(value) => value,
        Err(error) if IsErrNotFound(&error) => return Ok(0),
        Err(error) => return Err(error),
    };

    let text = std::str::from_utf8(&value.Value).map_err(|error| errors::New(error.to_string()))?;
    text.parse::<i64>()
        .map_err(|error| errors::New(error.to_string()))
}

/// 遍历内存缓冲（mem buffer）中全部键值，对每一对调用 callback。
///
/// 迭代结束后调用 Close；与 Go 一致，忽略 Close 返回的错误。
pub fn WalkMemBuffer<F>(mem_buf: &dyn Retriever, mut callback: F) -> Result<(), Error>
where
    F: FnMut(&Key, &[u8]) -> Result<(), Error>,
{
    let mut iterator = mem_buf.Iter(Key::default(), None)?;
    let result = (|| {
        while iterator.Valid() {
            let key = iterator.Key();
            let value = iterator.Value();
            callback(&key, &value)?;
            iterator.Next()?;
        }
        Ok(())
    })();

    // Go deliberately ignores the error returned by the deferred Close call.
    // 与 Go 一致：defer Close 的错误被刻意忽略。
    iterator.Close();
    result
}

/// 判断存储是否为 nextgen 下的用户 keyspace（非 SYSTEM）。
pub fn IsUserKS(store: &dyn Storage) -> bool {
    kerneltype::IsNextGen() && store.GetKeyspace() != keyspace::System
}

/// 判断存储是否为 nextgen 下的 SYSTEM keyspace（系统保留键空间）。
pub fn IsSystemKS(store: &dyn Storage) -> bool {
    kerneltype::IsNextGen() && store.GetKeyspace() == keyspace::System
}
