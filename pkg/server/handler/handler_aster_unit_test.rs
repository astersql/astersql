// Copyright 2026 AsterSQL.

//! 本文件补充 `pkg/server/handler` 的跨文件行为回归测试。
//!
//! 每个场景都直接对应 Go 实现的可观察契约：数字 handle 使用 strconv 的
//! base-0 解析，临时索引 key 改写 index ID，而 WriteData 成功写出 JSON。

use std::collections::HashMap;

use crate::tikv_handler::{
    Handle, NewTikvHandlerTool, PhysicalTable, Storage, UrlValues, get_handle,
    index_key_to_temp_index_key,
};
use crate::upgrade_handler::{NewClusterUpgradeHandler, Storage as UpgradeStorage};
use crate::util::{ResponseWriter, WriteData};

#[test]
fn get_handle_accepts_go_base_zero_integer() {
    let tool = NewTikvHandlerTool(Storage);
    let table = PhysicalTable;
    let mut params = HashMap::new();
    params.insert("handle".to_owned(), "0x10".to_owned());

    let result = get_handle(&tool, &table, &params, &UrlValues);
    assert!(result.is_ok());
    let handle = result.ok().expect("base-0 handle");
    assert!(matches!(handle, Handle::Int(16)));
}

#[test]
fn temp_index_key_rewrites_index_id_in_place() {
    // A table/index key has `t{table_id}_i{index_id}` before the encoded values.
    let mut key = vec![0u8; 19];
    key[0] = b't';
    key[9..11].copy_from_slice(b"_i");
    key[11..19].copy_from_slice(&0x8000_0000_0000_002a_u64.to_be_bytes());

    let temporary = index_key_to_temp_index_key(&key);
    assert_eq!(&temporary[..11], &key[..11]);
    assert_eq!(&temporary[11..19], &0xffff_0000_0000_002a_u64.to_be_bytes());
}

#[test]
fn write_data_writes_json_success_response() {
    let mut writer = ResponseWriter::default();
    WriteData(&mut writer, "ok");

    assert_eq!(writer.status_code(), Some(200));
    assert_eq!(writer.body_bytes(), br#""ok""#);
}

#[test]
fn cluster_upgrade_start_and_finish_follow_state_machine() {
    let handler = NewClusterUpgradeHandler(UpgradeStorage::new());

    assert_eq!(handler.StartUpgrade().ok(), Some(false));
    assert_eq!(handler.StartUpgrade().ok(), Some(true));
    assert_eq!(handler.FinishUpgrade().ok(), Some(false));
    assert_eq!(handler.FinishUpgrade().ok(), Some(true));
}
