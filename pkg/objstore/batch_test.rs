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

// `batch` 模块测试：验证 Effect 入队顺序与 JSON 序列化格式。
//
// 使用内存存储包装，断言写/删/重命名只进入队列，以及临时 JSON 文件内容。

use std::fs;

use objstore::azblob::MemoryStorage;
use objstore::batch::{
    EffDeleteFile, EffDeleteFiles, EffPut, EffRename, Effect, batch, save_json_effects_to_tmp,
};
use objstore::objectio::Context;
use objstore::storeapi::Storage;

#[test]
/// 覆盖单次与连续操作入队，并与期望 Effect 列表比对。
fn test_batched() {
    enum Operation {
        DeleteFiles,
        DeleteFile,
        WriteFile,
        Rename,
        Sequence,
    }

    let ctx = Context::default();
    let batched = batch(MemoryStorage::default());
    let cases = [
        (
            "DeleteFiles",
            Operation::DeleteFiles,
            vec![Effect::DeleteFiles(EffDeleteFiles {
                files: vec!["file1.txt".into(), "file2.txt".into()],
            })],
        ),
        (
            "DeleteFile",
            Operation::DeleteFile,
            vec![Effect::DeleteFile(EffDeleteFile("file3.txt".into()))],
        ),
        (
            "WriteFile",
            Operation::WriteFile,
            vec![Effect::Put(EffPut {
                file: "file4.txt".into(),
                content: b"content".to_vec(),
            })],
        ),
        (
            "Rename",
            Operation::Rename,
            vec![Effect::Rename(EffRename {
                from: "oldName.txt".into(),
                to: "newName.txt".into(),
            })],
        ),
        (
            "SequenceOfOperations",
            Operation::Sequence,
            vec![
                Effect::DeleteFile(EffDeleteFile("file5.txt".into())),
                Effect::Put(EffPut {
                    file: "file6.txt".into(),
                    content: b"new content".to_vec(),
                }),
                Effect::Rename(EffRename {
                    from: "file6.txt".into(),
                    to: "fileRenamed.txt".into(),
                }),
            ],
        ),
    ];

    // 每种操作执行后立刻核对队列，再清空以便下一条用例独立。
    for (name, operation, expected) in cases {
        match operation {
            Operation::DeleteFiles => {
                batched.DeleteFiles(&ctx, &["file1.txt".into(), "file2.txt".into()])
            }
            Operation::DeleteFile => batched.DeleteFile(&ctx, "file3.txt"),
            Operation::WriteFile => batched.WriteFile(&ctx, "file4.txt", b"content"),
            Operation::Rename => batched.Rename(&ctx, "oldName.txt", "newName.txt"),
            Operation::Sequence => batched
                .DeleteFile(&ctx, "file5.txt")
                .and_then(|_| batched.WriteFile(&ctx, "file6.txt", b"new content"))
                .and_then(|_| batched.Rename(&ctx, "file6.txt", "fileRenamed.txt")),
        }
        .unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert_eq!(batched.read_only_effects(), expected, "{name}");
        batched.clean_effects();
    }
}

#[test]
/// 校验 `save_json_effects_to_tmp` 产出的 JSON 与 Go 兼容字段（含 Base64 内容）。
fn test_json_effects() {
    let effects = vec![
        Effect::Put(EffPut {
            file: "example.txt".into(),
            content: b"Hello, world".to_vec(),
        }),
        Effect::DeleteFiles(EffDeleteFiles {
            files: vec!["old_file.txt".into(), "temp.txt".into()],
        }),
        Effect::DeleteFile(EffDeleteFile("obsolete.txt".into())),
        Effect::Rename(EffRename {
            from: "old_name.txt".into(),
            to: "new_name.txt".into(),
        }),
    ];
    let path = save_json_effects_to_tmp(&effects).unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fs::remove_file(path).unwrap();
    let expected = serde_json::json!([
        {"type":"objstore.EffPut","effect":{"file":"example.txt","content":"SGVsbG8sIHdvcmxk"}},
        {"type":"objstore.EffDeleteFiles","effect":{"files":["old_file.txt","temp.txt"]}},
        {"type":"objstore.EffDeleteFile","effect":"obsolete.txt"},
        {"type":"objstore.EffRename","effect":{"from":"old_name.txt","to":"new_name.txt"}}
    ]);
    assert_eq!(actual, expected);
}
