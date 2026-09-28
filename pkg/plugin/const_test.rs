// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 插件常量与事件枚举的字符串化、解析单元测试。
//
// 覆盖 `Kind`/`State`/`ConnectionEvent`/`GeneralEvent` 与 Go 侧 `String()` /
// `GeneralEventFromString` 行为一致，含未知连接事件码与大小写不敏感解析。

use crate::{ConnectionEvent, GeneralEvent, Kind, State, general_event_from_string};

/// Corresponds to Go `TestConstToString`.
/// 校验各类枚举 `Display`/`to_string` 结果与 Go 固定字符串一致。
#[test]
fn test_const_to_string() {
    let kinds: Vec<(String, &str)> = vec![
        (Kind::Audit.to_string(), "Audit"),
        (Kind::Authentication.to_string(), "Authentication"),
        (Kind::Schema.to_string(), "Schema"),
        (Kind::Daemon.to_string(), "Daemon"),
        (State::Uninitialized.to_string(), "Uninitialized"),
        (State::Ready.to_string(), "Ready"),
        (State::Dying.to_string(), "Dying"),
        (State::Disable.to_string(), "Disable"),
        (ConnectionEvent::Connected.to_string(), "Connected"),
        (ConnectionEvent::Disconnect.to_string(), "Disconnect"),
        (ConnectionEvent::ChangeUser.to_string(), "ChangeUser"),
        (ConnectionEvent::PreAuth.to_string(), "PreAuth"),
        (ConnectionEvent::Reject.to_string(), "Reject"),
        // Go casts `byte(15)` to ConnectionEvent; unknown values stringify to "".
        // 未知事件码应输出空串，与 Go 对非法 byte 的行为对齐。
        (ConnectionEvent(15).to_string(), ""),
    ];
    for (key, value) in kinds {
        assert_eq!(value, key);
    }

    // GeneralEvent 使用大写字符串，且 COUNT 与枚举成员数一致。
    let general_events = vec![
        (GeneralEvent::Starting, "STARTING"),
        (GeneralEvent::Completed, "COMPLETED"),
        (GeneralEvent::Error, "ERROR"),
    ];
    assert_eq!(GeneralEvent::COUNT as usize, general_events.len());
    for (key, value) in general_events {
        assert_eq!(value, key.to_string());
    }
}

/// Corresponds to Go `TestGeneralEventString`.
/// 校验通用事件往返解析：规范化大写、忽略大小写、非法输入报错。
#[test]
fn test_general_event_string() {
    for raw in 0..GeneralEvent::COUNT {
        let event = GeneralEvent::from_u8(raw).expect("valid general event");
        assert_eq!(event.to_string().to_uppercase(), event.to_string());
        let got = general_event_from_string(&event.to_string()).expect("parse");
        assert_eq!(got, event);
    }

    let event = general_event_from_string("starting").expect("parse");
    assert_eq!(GeneralEvent::Starting, event);

    let event = general_event_from_string("starTing").expect("parse");
    assert_eq!(GeneralEvent::Starting, event);

    assert!(general_event_from_string("").is_err());
    assert!(general_event_from_string("xx").is_err());
}
