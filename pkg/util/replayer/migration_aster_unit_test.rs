// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// AsterSQL 迁移补充测试：Plan Replayer 文件名分支与写入转发。
//
// 用可记录的假 Storage/ObjectWriter 验证相对路径、前缀分支与 WriteCloser 行为，
// 对照 Go `pkg/util/replayer` 的命名规则。

use crate::replayer::{
    Context, Error, GeneratePlanReplayerFile, GeneratePlanReplayerFileName, GetPlanReplayerDirName,
    NewFileWriter, ObjectWriter, Storage,
};
use std::sync::{Arc, Mutex};

/// 记录写入内容与是否已 close 的测试状态。
#[derive(Default)]
struct WriterState {
    writes: Vec<Vec<u8>>,
    closed: bool,
}

/// 将每次 write/close 记入共享 WriterState 的假写入器。
struct RecordingWriter(Arc<Mutex<WriterState>>);

impl ObjectWriter for RecordingWriter {
    fn write(&mut self, _ctx: &Context, data: &[u8]) -> Result<usize, Error> {
        self.0.lock().unwrap().writes.push(data.to_vec());
        Ok(data.len())
    }

    fn close(&mut self, _ctx: &Context) -> Result<(), Error> {
        self.0.lock().unwrap().closed = true;
        Ok(())
    }
}

/// 记录 create 路径并返回 RecordingWriter 的假对象存储。
struct RecordingStorage {
    path: Arc<Mutex<Option<String>>>,
    writer_state: Arc<Mutex<WriterState>>,
}

impl Storage for RecordingStorage {
    fn create(
        &self,
        _ctx: &Context,
        path: String,
        _options: Option<()>,
    ) -> Result<Box<dyn ObjectWriter>, Error> {
        *self.path.lock().unwrap() = Some(path);
        Ok(Box::new(RecordingWriter(Arc::clone(&self.writer_state))))
    }
}

/// 断言生成名形如 `{prefix}{url-safe-key}_{timestamp}.zip`。
fn assert_generated_name(name: &str, expected_prefix: &str) {
    assert!(name.starts_with(expected_prefix), "unexpected name: {name}");
    assert!(name.ends_with(".zip"), "unexpected name: {name}");

    let remainder = &name[expected_prefix.len()..name.len() - 4];
    let (key, timestamp) = remainder.rsplit_once('_').expect("key and timestamp");
    assert_eq!(key.len(), 24, "16 bytes in padded URL-safe base64");
    assert!(key.ends_with("=="));
    assert!(
        key.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'='))
    );
    assert!(timestamp.parse::<i64>().unwrap() > 0);
}

/// 对照 Go：各 (capture, continuous, historical) 组合应对应的文件名前缀。
#[test]
fn test_generate_plan_replayer_file_name_matches_go_branches() {
    let cases = [
        ((false, false, false), "replayer_"),
        ((true, false, false), "capture_normal_replayer_"),
        ((false, true, false), "capture_replayer_"),
        ((true, true, false), "capture_replayer_"),
        ((false, false, true), "replayer_"),
        ((true, false, true), "capture_replayer_"),
        ((false, true, true), "capture_replayer_"),
        ((true, true, true), "capture_replayer_"),
    ];

    for ((capture, continuous, historical), prefix) in cases {
        let name = GeneratePlanReplayerFileName(capture, continuous, historical).unwrap();
        assert_generated_name(&name, prefix);
    }
}

/// 验证 NewFileWriter 将 write/close 原样转发到底层 ObjectWriter。
#[test]
fn test_file_writer_forwards_write_and_close() {
    let state = Arc::new(Mutex::new(WriterState::default()));
    let mut writer = NewFileWriter(
        Context::default(),
        Box::new(RecordingWriter(Arc::clone(&state))),
    );

    assert_eq!(writer.write(b"plan replayer").unwrap(), 13);
    writer.close().unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.writes, [b"plan replayer"]);
    assert!(state.closed);
}

/// 验证 GeneratePlanReplayerFile 使用 `replayer/{file}` 相对路径。
#[test]
fn test_generate_file_uses_relative_replayer_path() {
    let path = Arc::new(Mutex::new(None));
    let writer_state = Arc::new(Mutex::new(WriterState::default()));
    let storage = RecordingStorage {
        path: Arc::clone(&path),
        writer_state: Arc::clone(&writer_state),
    };

    let (mut writer, file_name) =
        GeneratePlanReplayerFile(Context::default(), &storage, true, false, false).unwrap();
    assert_generated_name(&file_name, "capture_normal_replayer_");
    assert_eq!(
        path.lock().unwrap().as_deref(),
        Some(format!("replayer/{file_name}").as_str())
    );
    assert_eq!(GetPlanReplayerDirName(), "replayer");

    writer.write(b"payload").unwrap();
    writer.close().unwrap();
    let state = writer_state.lock().unwrap();
    assert_eq!(state.writes, [b"payload"]);
    assert!(state.closed);
}
