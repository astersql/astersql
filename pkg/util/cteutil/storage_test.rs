// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// CTE 临时存储（`Storage`）单元测试。
//
// 覆盖引用计数开闭、Chunk 读写、内存超限落盘（spill）、
// `Reopen` 清空与 `SwapData` 交换数据等行为，对应 Go `storage_test.go`。

use std::sync::Arc;

use cteutil::{NewStorageRowContainer, Storage, chunk, memory, mysql, types};

/// 构造仅含单一列类型 `tp` 的字段元数据切片。
fn fields(tp: u8) -> Vec<types::FieldType> {
    vec![*types::NewFieldType(tp)]
}

/// 生成 `size` 行、第 0 列为 0..size-1 的整型 Chunk。
fn int_chunk(field_types: &[types::FieldType], size: usize) -> Box<chunk::Chunk> {
    let mut input = chunk::NewChunkWithCapacity(field_types.to_vec(), size);
    for value in 0..size {
        input.AppendInt64(0, value as i64);
    }
    input
}

/// 提取 Chunk 第 0 列全部 Int64 值。
fn int_values(input: &chunk::Chunk) -> Vec<i64> {
    (0..input.NumRows())
        .map(|row| input.GetRow(row).GetInt64(0))
        .collect()
}

/// 提取 Chunk 第 0 列全部字符串值。
fn string_values(input: &chunk::Chunk) -> Vec<String> {
    (0..input.NumRows())
        .map(|row| input.GetRow(row).GetString(0))
        .collect()
}

/// Adapter for the already-integrated chunk spill action. It supplies the
/// memory tracker's Go-style fallback interface while retaining the Arc that
/// the asynchronous Rust action needs.
///
/// 将已集成的 Chunk 落盘动作适配为 Go 风格的 `ActionOnExceed` 接口，
/// 同时保留异步落盘所需的 `Arc` Tracker。
struct SpillOnExceed {
    action: chunk::SpillDiskAction,
    tracker: Arc<memory::Tracker>,
    fallback: Option<Box<dyn memory::ActionOnExceed>>,
    finished: bool,
}

impl memory::ActionOnExceed for SpillOnExceed {
    fn Action(&mut self, _tracker: &mut memory::Tracker) {
        self.action.Action(Arc::clone(&self.tracker));
    }

    fn SetFallback(&mut self, action: Option<Box<dyn memory::ActionOnExceed>>) {
        self.fallback = action;
    }

    fn GetFallback(&mut self) -> Option<Box<dyn memory::ActionOnExceed>> {
        self.fallback.take()
    }

    fn GetPriority(&self) -> i64 {
        self.action.GetPriority()
    }

    fn SetFinished(&mut self) {
        self.finished = true;
        self.action.SetFinished();
    }

    fn IsFinished(&self) -> bool {
        self.finished
    }
}

#[test]
/// 验证未打开时关闭失败，以及 Open/Deref 引用计数配对。
fn TestStorageBasic() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 1);

    assert_eq!(
        storage.DerefAndClose().unwrap_err().to_string(),
        "Storage not opend yet"
    );

    storage.OpenAndRef().unwrap();
    storage.DerefAndClose().unwrap();
    assert_eq!(
        storage.DerefAndClose().unwrap_err().to_string(),
        "Storage not opend yet"
    );

    storage.OpenAndRef().unwrap();
    storage.OpenAndRef().unwrap();
    storage.DerefAndClose().unwrap();
    storage.DerefAndClose().unwrap();
    assert_eq!(
        storage.DerefAndClose().unwrap_err().to_string(),
        "Storage not opend yet"
    );
}

#[test]
/// 多次 Open 后按次数 Deref，最后一次关闭底层存储。
fn TestOpenAndClose() {
    let mut storage = NewStorageRowContainer(fields(mysql::TypeLong), 1);

    for _ in 0..10 {
        storage.OpenAndRef().unwrap();
    }
    for _ in 0..9 {
        storage.DerefAndClose().unwrap();
    }
    storage.DerefAndClose().unwrap();
    assert_eq!(
        storage.DerefAndClose().unwrap_err().to_string(),
        "Storage not opend yet"
    );
}

#[test]
/// 未打开时 Add 失败；打开后 Add/GetChunk 往返一致。
fn TestAddAndGetChunk() {
    let field_types = fields(mysql::TypeLong);
    let mut storage = NewStorageRowContainer(field_types.clone(), 10);
    let input = int_chunk(&field_types, 10);

    assert_eq!(
        storage.Add(&input).unwrap_err().to_string(),
        "Storage is not valid"
    );
    storage.OpenAndRef().unwrap();
    storage.Add(&input).unwrap();

    let output = storage.GetChunk(0).unwrap();
    assert_eq!(int_values(&input), int_values(&output));
}

#[test]
/// 内存超限触发 spill：第二次 Add 后数据落到磁盘 Tracker。
fn TestSpillToDisk() {
    let field_types = fields(mysql::TypeLong);
    let mut storage = NewStorageRowContainer(field_types.clone(), 10);
    let input = int_chunk(&field_types, 10);
    storage.OpenAndRef().unwrap();

    let mem_tracker = storage.GetMemTracker();
    mem_tracker.SetBytesLimit(input.MemoryUsage() + 1);
    let action = storage.ActionSpillForTest();
    mem_tracker.FallbackOldAndSetNewAction(Some(Box::new(SpillOnExceed {
        action: action.clone(),
        tracker: Arc::clone(&mem_tracker),
        fallback: None,
        finished: false,
    })));
    let disk_tracker = storage.GetDiskTracker();

    storage.Add(&input).unwrap();
    let output = storage.GetChunk(0).unwrap();
    assert_eq!(int_values(&input), int_values(&output));
    assert!(mem_tracker.BytesConsumed() > 0);
    assert!(mem_tracker.MaxConsumed() > 0);
    assert_eq!(disk_tracker.BytesConsumed(), 0);
    assert_eq!(disk_tracker.MaxConsumed(), 0);

    // 再次写入使内存用量超限，触发落盘动作并等待完成。
    storage.Add(&input).unwrap();
    action.WaitForTest();
    assert_eq!(mem_tracker.BytesConsumed(), 0);
    assert!(mem_tracker.MaxConsumed() > 0);
    assert!(disk_tracker.BytesConsumed() > 0);
    assert!(disk_tracker.MaxConsumed() > 0);

    let output0 = storage.GetChunk(0).unwrap();
    let output1 = storage.GetChunk(1).unwrap();
    assert_eq!(int_values(&input), int_values(&output0));
    assert_eq!(int_values(&input), int_values(&output1));
}

#[test]
/// `Reopen` 清空已有 Chunk，可再次写入；可重复调用。
fn TestReopen() {
    let field_types = fields(mysql::TypeLong);
    let mut storage = NewStorageRowContainer(field_types.clone(), 10);
    storage.OpenAndRef().unwrap();
    let input = int_chunk(&field_types, 10);

    storage.Add(&input).unwrap();
    assert_eq!(storage.NumChunks(), 1);
    storage.Reopen().unwrap();
    assert_eq!(storage.NumChunks(), 0);

    storage.Add(&input).unwrap();
    assert_eq!(storage.NumChunks(), 1);
    assert_eq!(
        int_values(&input),
        int_values(&storage.GetChunk(0).unwrap())
    );

    for _ in 0..100 {
        storage.Reopen().unwrap();
    }
    storage.Add(&input).unwrap();
    assert_eq!(storage.NumChunks(), 1);
    assert_eq!(
        int_values(&input),
        int_values(&storage.GetChunk(0).unwrap())
    );
}

#[test]
/// `SwapData` 交换两存储的数据与 schema，元数据（引用计数等）不变。
fn TestSwapData() {
    let int_fields = fields(mysql::TypeLong);
    let mut storage1 = NewStorageRowContainer(int_fields.clone(), 10);
    storage1.OpenAndRef().unwrap();
    let input1 = int_chunk(&int_fields, 10);
    let expected1 = int_values(&input1);
    storage1.Add(&input1).unwrap();

    let string_fields = fields(mysql::TypeVarString);
    let mut storage2 = NewStorageRowContainer(string_fields.clone(), 10);
    storage2.OpenAndRef().unwrap();
    let mut input2 = chunk::NewChunkWithCapacity(string_fields, 10);
    for value in 0_i64..10 {
        input2.AppendString(0, &value.to_string());
    }
    let expected2 = string_values(&input2);
    storage2.Add(&input2).unwrap();

    storage1.SwapData(&mut storage2).unwrap();

    let output1 = storage1.GetChunk(0).unwrap();
    let output2 = storage2.GetChunk(0).unwrap();
    assert_eq!(expected1, int_values(&output2));
    assert_eq!(expected2, string_values(&output1));
}
