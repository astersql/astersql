// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL 命令客户端（etcd / mock）往返与错误路径的单元测试。
//
// 覆盖成功响应、错误响应、上下文取消、缺失 request id，以及 JSON 编解码。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::command::{
    ClientContext, ClientError, CmdRequest, CommandClient, CommandResult, EtcdStore, JsonValue,
    TTL_CMD_KEY_RESPONSE_PREFIX, TriggerNewTtlJobResponse, new_command_client,
    new_mock_command_client,
};
use crate::notification::{
    NotificationClient, TTL_NOTIFICATION_PREFIX, new_mock_notification_client,
    new_notification_client,
};

/// 构造测试用请求 JSON 对象。
fn request_payload() -> JsonValue {
    JsonValue::object([
        ("v_1", JsonValue::from("1")),
        ("v_2", JsonValue::from(2_i64)),
    ])
}
/// 构造测试用响应 JSON 对象。
fn response_payload() -> JsonValue {
    JsonValue::object([
        ("v_3", JsonValue::from("3")),
        ("v_4", JsonValue::from(4_i64)),
    ])
}
/// 从 JSON 对象取出命名字段，缺失则 panic。
fn object_field<'a>(value: &'a JsonValue, name: &str) -> &'a JsonValue {
    let JsonValue::Object(fields) = value else {
        panic!("expected a JSON object, got {value:?}")
    };
    fields
        .get(name)
        .unwrap_or_else(|| panic!("missing field {name}"))
}

// 对应 Go TestCommandClient：对 etcd-backed client 和 mock client 跑同一套发送/watch/take/response
// 流程，覆盖成功响应、重复 take 返回 false、以及错误响应两条路径。
fn run_command_client_round_trip(client: Arc<dyn CommandClient>) {
    let ctx = ClientContext::new().with_timeout(Duration::from_secs(30));

    // ---- 成功路径 ----
    let watcher = client.watch_command(ctx.clone());
    let (send_tx, send_rx) = mpsc::channel();
    {
        let client = Arc::clone(&client);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let (request_id, result) = client.command(&ctx, "type1", request_payload());
            let _ = send_tx.send((request_id, result));
        });
    }

    let cmd = watcher
        .recv_timeout(Duration::from_secs(10))
        .expect("expected the mocked TTL job command to be observed by the watcher");
    assert_eq!(cmd.cmd_type, "type1");
    assert_eq!(
        object_field(&cmd.data, "v_1"),
        &JsonValue::String("1".into())
    );
    assert_eq!(
        object_field(&cmd.data, "v_2"),
        &JsonValue::Number("2".into())
    );

    let taken = client
        .take_command(&ctx, &cmd.request_id)
        .expect("take_command should not fail for a pending request");
    assert!(taken);
    client
        .response_command(
            &ctx,
            &cmd.request_id,
            CommandResult::data(response_payload()),
        )
        .expect("response_command should not fail while the sender is still waiting");

    let (send_request_id, result) = send_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("sender thread should report back the command result");
    assert!(!send_request_id.is_empty());
    assert_eq!(send_request_id, cmd.request_id);
    let response = result.expect("command should have succeeded with the mocked response");
    assert_eq!(
        object_field(&response, "v_3"),
        &JsonValue::String("3".into())
    );
    assert_eq!(
        object_field(&response, "v_4"),
        &JsonValue::Number("4".into())
    );

    // 同一个 request 被 take 后不能重复获取。
    let taken_again = client
        .take_command(&ctx, &cmd.request_id)
        .expect("take_command should not fail for an already-taken request");
    assert!(!taken_again);

    // ---- 错误路径 ----
    let watcher = client.watch_command(ctx.clone());
    let (send_tx, send_rx) = mpsc::channel();
    {
        let client = Arc::clone(&client);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let (request_id, result) = client.command(&ctx, "type1", request_payload());
            let _ = send_tx.send((request_id, result));
        });
    }

    let cmd = watcher
        .recv_timeout(Duration::from_secs(10))
        .expect("expected the second command to be observed by the watcher");
    client
        .take_command(&ctx, &cmd.request_id)
        .expect("take_command should not fail for the second request");
    client
        .response_command(
            &ctx,
            &cmd.request_id,
            CommandResult::Error("mockErr".to_owned()),
        )
        .expect("response_command should not fail while delivering an error");

    let (send_request_id, result) = send_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("sender thread should report back the error result");
    assert!(!send_request_id.is_empty());
    let err = result.expect_err("command should have failed with the mocked error");
    assert_eq!(err, ClientError::Response("mockErr".to_owned()));
}

#[test]
fn test_command_client_round_trip_with_etcd_backed_client() {
    let store = Arc::new(EtcdStore::default());
    run_command_client_round_trip(new_command_client(store));
}

#[test]
fn test_command_client_round_trip_with_mock_client() {
    run_command_client_round_trip(new_mock_command_client());
}

// 对应 Go 中 ctx.Done() 分支：ClientContext 取消后，Command 应立即返回取消错误而不是继续等待。
#[test]
fn test_command_client_command_fails_fast_when_context_cancelled() {
    let store = Arc::new(EtcdStore::default());
    let client = new_command_client(store);
    let ctx = ClientContext::new();
    ctx.cancel();
    let (_, result) = client.command(&ctx, "type1", request_payload());
    assert_eq!(result.unwrap_err(), ClientError::Cancelled);
}

// Go cmdResponse uses ordinary encoding/json struct decoding: a missing request_id stays at its
// zero value, and waitCmdResponse ignores it when returning the response payload.
#[test]
fn etcd_response_accepts_missing_request_id_like_go_json_unmarshal() {
    let store = Arc::new(EtcdStore::default());
    let client = new_command_client(Arc::clone(&store));
    let ctx = ClientContext::new().with_timeout(Duration::from_secs(2));
    let watcher = client.watch_command(ctx.clone());
    let (send_tx, send_rx) = mpsc::channel();
    {
        let client = Arc::clone(&client);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = client.command(&ctx, "type1", request_payload());
            let _ = send_tx.send(result);
        });
    }

    let request = watcher.recv_timeout(Duration::from_secs(1)).unwrap();
    store
        .put(
            &ctx,
            format!("{TTL_CMD_KEY_RESPONSE_PREFIX}{}", request.request_id),
            br#"{"error_message":"","data":{"v_3":"3","v_4":4}}"#.to_vec(),
            None,
        )
        .unwrap();

    let (_, result) = send_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(result.unwrap(), response_payload());
}

// 对应 Go：向不存在的 request id 发送响应应报告未找到，而不是静默成功。
#[test]
fn test_command_client_response_command_missing_request_returns_error() {
    let client = new_mock_command_client();
    let ctx = ClientContext::new();
    let err = client
        .response_command(&ctx, "does-not-exist", CommandResult::data(JsonValue::Null))
        .expect_err("responding to an unknown request id must fail");
    assert_eq!(
        err,
        ClientError::ResponseKeyNotFound("does-not-exist".to_owned())
    );
}

// 对应 Go：对不存在的 request id 调用 TakeCommand 应返回 false、无错误。
#[test]
fn test_command_client_take_command_missing_request_returns_false() {
    let client = new_mock_command_client();
    let ctx = ClientContext::new();
    let taken = client.take_command(&ctx, "does-not-exist").unwrap();
    assert!(!taken);
}

// JSON 编解码是 command client 传输层的关键组成部分：确认对象/数组/字符串转义可以来回还原。
#[test]
fn test_json_value_round_trips_through_bytes() {
    let value = JsonValue::object([
        ("s", JsonValue::from("hello \"world\"\n")),
        ("n", JsonValue::from(42_i64)),
        ("b", JsonValue::Bool(true)),
        ("nil", JsonValue::Null),
        (
            "arr",
            JsonValue::Array(vec![JsonValue::from(1_i64), JsonValue::from(2_i64)]),
        ),
    ]);
    let bytes = value.to_bytes();
    let decoded = JsonValue::parse(&bytes).unwrap();
    assert_eq!(decoded, value);
}

// Go 的 GetTriggerTTLJobRequest 对合法 JSON 中缺失的字符串字段保留零值并返回 true。
#[test]
fn trigger_ttl_job_request_accepts_missing_fields_like_go_json_unmarshal() {
    let request = CmdRequest {
        request_id: "request-1".to_owned(),
        cmd_type: crate::command::TTL_CMD_TYPE_TRIGGER_TTL_JOB.to_owned(),
        data: JsonValue::object([] as [(String, JsonValue); 0]),
    };

    let trigger = request
        .get_trigger_ttl_job_request()
        .expect("Go json.Unmarshal accepts an empty object for this struct");
    assert_eq!(trigger.db_name, "");
    assert_eq!(trigger.table_name, "");

    let response =
        TriggerNewTtlJobResponse::from_json(&JsonValue::object([] as [(String, JsonValue); 0]))
            .expect("Go json.Unmarshal leaves a missing table_result at its zero value");
    assert!(response.table_result.is_empty());
}

// Go mock 的 TakeCommand/ResponseCommand 参数名为 `_ context.Context`，不会读取取消状态。
#[test]
fn mock_take_and_response_ignore_cancelled_context_like_go() {
    let client = new_mock_command_client();
    let ctx = ClientContext::new();
    ctx.cancel();

    assert_eq!(client.take_command(&ctx, "missing"), Ok(false));
    assert_eq!(
        client.response_command(&ctx, "missing", CommandResult::data(JsonValue::Null)),
        Err(ClientError::ResponseKeyNotFound("missing".to_owned()))
    );
    assert_eq!(
        ClientError::ResponseKeyNotFound("missing".to_owned()).to_string(),
        "response key not found for: missing"
    );
}

// Go 的 util.PutKVToEtcd(..., 1, ...) 为真实通知键附加 1 秒租约。
#[test]
fn etcd_notification_expires_after_go_one_second_lease() {
    let store = Arc::new(EtcdStore::default());
    let client = new_notification_client(Arc::clone(&store));
    let ctx = ClientContext::new();
    let key = format!("{TTL_NOTIFICATION_PREFIX}refresh");

    client.notify(&ctx, "refresh", "payload").unwrap();
    assert!(store.get(&ctx, &key).unwrap().is_some());
    thread::sleep(Duration::from_millis(1_100));
    assert_eq!(store.get(&ctx, &key).unwrap(), None);
}

// Go mock 在无 watcher 时直接成功；有 watcher 时发送空 WatchResponse，不携带 data。
#[test]
fn mock_notification_keeps_go_empty_event_and_no_watcher_cancel_semantics() {
    let client = new_mock_notification_client();
    let cancelled = ClientContext::new();
    cancelled.cancel();
    client
        .notify(&cancelled, "without-watcher", "ignored")
        .expect("Go returns nil before consulting ctx when this type has no watchers");

    let ctx = ClientContext::new();
    let watcher = client.watch_notification(ctx.clone(), "refresh");
    client.notify(&ctx, "refresh", "ignored").unwrap();
    let event = watcher.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(event.data, "");
}

// Go encoding/json combines UTF-16 surrogate pairs and replaces lone surrogates.
#[test]
fn json_string_unicode_surrogates_match_go_encoding_json() {
    assert_eq!(
        JsonValue::parse(br#""\ud83d\ude00""#).unwrap(),
        JsonValue::String("😀".to_owned())
    );
    assert_eq!(
        JsonValue::parse(br#""\ud83d""#).unwrap(),
        JsonValue::String("\u{fffd}".to_owned())
    );
    assert_eq!(
        JsonValue::parse(br#""\ude00""#).unwrap(),
        JsonValue::String("\u{fffd}".to_owned())
    );
}
