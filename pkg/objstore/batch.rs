// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 对象存储批处理包装：将写/删/重命名等副作用先入队，再一次性 `commit`。
//
// 用于备份恢复（BR）等场景：先记录 Effect，序列化为 JSON，或提交到下层 `Storage`。
// 只读操作（Read/Open/Walk 等）仍直接透传到底层存储。

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::azblob::lock_unpoisoned;
use crate::{objectio, storeapi};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 批处理队列中的单次副作用操作（写、删、批量删、重命名）。
pub enum Effect {
    Put(EffPut),
    DeleteFiles(EffDeleteFiles),
    DeleteFile(EffDeleteFile),
    Rename(EffRename),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// 写入文件的 Effect：路径与内容。
pub struct EffPut {
    pub file: String,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// 批量删除多个文件的 Effect。
pub struct EffDeleteFiles {
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// 删除单个文件的 Effect，内层为对象名。
pub struct EffDeleteFile(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// 重命名 Effect：源路径与目标路径。
pub struct EffRename {
    pub from: String,
    pub to: String,
}

/// 将 Effect 列表序列化为带换行结尾的 JSON 字节。
pub fn json_effects(effects: &[Effect]) -> Result<Vec<u8>> {
    let values = effects.iter().map(typed_json_effect).collect::<Vec<_>>();
    let mut output = serde_json::to_vec(&values)?;
    output.push(b'\n');
    Ok(output)
}

/// 把 Effect 列表的 JSON 写入任意 `Write` 实现。
pub fn write_json_effects<W: Write>(effects: &[Effect], output: &mut W) -> Result<()> {
    output.write_all(&json_effects(effects)?)?;
    Ok(())
}

/// 将 Effect JSON 落到临时文件并返回路径（前缀 `br-effects-`）。
pub fn save_json_effects_to_tmp(effects: &[Effect]) -> Result<String> {
    let mut file = tempfile::Builder::new()
        .prefix("br-effects-")
        .suffix(".json")
        .tempfile_in(std::env::temp_dir())?;
    write_json_effects(effects, &mut file)?;
    let (_persisted, path) = file.keep()?;
    Ok(path.to_string_lossy().into_owned())
}

/// 按 Go 兼容的 type/effect 结构把单个 Effect 转为 JSON 值；二进制内容用 Base64。
fn typed_json_effect(effect: &Effect) -> Value {
    match effect {
        // 与 Go 侧 JSON 类型名对齐，便于跨语言回放 Effect。
        Effect::Put(value) => json!({
            "type": "objstore.EffPut",
            "effect": {
                "file": value.file,
                "content": base64::engine::general_purpose::STANDARD.encode(&value.content),
            }
        }),
        Effect::DeleteFiles(value) => json!({
            "type": "objstore.EffDeleteFiles",
            "effect": { "files": value.files }
        }),
        Effect::DeleteFile(value) => json!({
            "type": "objstore.EffDeleteFile",
            "effect": value.0
        }),
        Effect::Rename(value) => json!({
            "type": "objstore.EffRename",
            "effect": { "from": value.from, "to": value.to }
        }),
    }
}

/// 包装下层 `Storage`，把写类操作缓存到 `effects`，读类操作透传。
pub struct Batched<S: storeapi::Storage> {
    storage: S,
    effects: Mutex<Vec<Effect>>,
}

impl<S: storeapi::Storage> Batched<S> {
    /// 用给定底层存储构造空队列的批处理包装。
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            effects: Mutex::new(Vec::new()),
        }
    }

    /// 返回底层存储引用。
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// 克隆当前已入队但尚未提交的 Effect 列表（只读快照）。
    pub fn read_only_effects(&self) -> Vec<Effect> {
        lock_unpoisoned(&self.effects).clone()
    }

    /// 清空队列，不执行任何底层写操作。
    pub fn clean_effects(&self) {
        lock_unpoisoned(&self.effects).clear();
    }

    /// 按入队顺序尝试执行全部 Effect，清空队列；对齐 Go：错误内部收集，仍返回 Ok。
    pub fn commit(&self, ctx: &objectio::Context) -> Result<()> {
        let mut effects = lock_unpoisoned(&self.effects);
        // Keep Go's observable behavior: every queued operation is attempted,
        // operational errors are combined internally, and Commit returns nil.
        let mut _errors = Vec::new();
        for effect in effects.iter() {
            let result = match effect {
                Effect::Put(value) => self.storage.WriteFile(ctx, &value.file, &value.content),
                Effect::DeleteFiles(value) => self.storage.DeleteFiles(ctx, &value.files),
                Effect::DeleteFile(value) => self.storage.DeleteFile(ctx, &value.0),
                Effect::Rename(value) => self.storage.Rename(ctx, &value.from, &value.to),
            };
            if let Err(error) = result {
                _errors.push(error);
            }
        }
        effects.clear();
        Ok(())
    }
}

/// 构造 `Batched` 包装的便捷函数。
pub fn batch<S: storeapi::Storage>(storage: S) -> Batched<S> {
    Batched::new(storage)
}

impl<S: storeapi::Storage> storeapi::Storage for Batched<S> {
    fn WriteFile(&self, _ctx: &objectio::Context, name: &str, data: &[u8]) -> Result<()> {
        lock_unpoisoned(&self.effects).push(Effect::Put(EffPut {
            file: name.to_owned(),
            content: data.to_vec(),
        }));
        Ok(())
    }

    fn ReadFile(&self, ctx: &objectio::Context, name: &str) -> Result<Vec<u8>> {
        self.storage.ReadFile(ctx, name)
    }

    fn FileExists(&self, ctx: &objectio::Context, name: &str) -> Result<bool> {
        self.storage.FileExists(ctx, name)
    }

    fn DeleteFile(&self, _ctx: &objectio::Context, name: &str) -> Result<()> {
        lock_unpoisoned(&self.effects).push(Effect::DeleteFile(EffDeleteFile(name.to_owned())));
        Ok(())
    }

    fn Open(
        &self,
        ctx: &objectio::Context,
        path: &str,
        option: Option<&storeapi::ReaderOption>,
    ) -> Result<Box<dyn objectio::Reader>> {
        self.storage.Open(ctx, path, option)
    }

    fn DeleteFiles(&self, _ctx: &objectio::Context, names: &[String]) -> Result<()> {
        lock_unpoisoned(&self.effects).push(Effect::DeleteFiles(EffDeleteFiles {
            files: names.to_vec(),
        }));
        Ok(())
    }

    fn WalkDir(
        &self,
        ctx: &objectio::Context,
        option: Option<&storeapi::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        self.storage.WalkDir(ctx, option, callback)
    }

    fn URI(&self) -> String {
        self.storage.URI()
    }

    // 批模式下暂不允许流式 Create，避免半开写入无法入队。
    fn Create(
        &self,
        _ctx: &objectio::Context,
        _path: &str,
        _option: Option<&storeapi::WriterOption>,
    ) -> Result<Box<dyn objectio::Writer>> {
        bail!("ExternalStorage.Create isn't allowed in batch mode for now.")
    }

    fn Rename(
        &self,
        _ctx: &objectio::Context,
        old_file_name: &str,
        new_file_name: &str,
    ) -> Result<()> {
        lock_unpoisoned(&self.effects).push(Effect::Rename(EffRename {
            from: old_file_name.to_owned(),
            to: new_file_name.to_owned(),
        }));
        Ok(())
    }

    fn PresignFile(
        &self,
        ctx: &objectio::Context,
        file_name: &str,
        expire: Duration,
    ) -> Result<String> {
        self.storage.PresignFile(ctx, file_name, expire)
    }

    fn Close(&self) {
        self.storage.Close();
    }
}

#[allow(dead_code)]
/// 编译期断言 `File` 可作为 `Write` 使用（死代码，仅约束类型）。
fn _assert_file_is_write(file: &mut File) -> &mut dyn Write {
    file
}
