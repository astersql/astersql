// Copyright 2026 AsterSQL.
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

// trxevents 迁移补充单元测试。
//
// 校验 EventType iota、WrapCopMeetLock 载荷保留 / typed-nil，以及 EventCallback 调用。

use std::sync::{Arc, Mutex};

use super::{CopMeetLock, EventCallback, EventTypeCopMeetLock, WrapCopMeetLock};
use tikv_client_proto::kvrpcpb::LockInfo;

/// 确认 CopMeetLock 事件类型常量与 Go iota 首值一致（0）。
#[test]
fn cop_meet_lock_event_type_matches_go_iota() {
    assert_eq!(EventTypeCopMeetLock, 0);
}

/// 包装带 LockInfo 的 CopMeetLock 后，GetCopMeetLock 能取回原 primary/key/version。
#[test]
fn wrap_cop_meet_lock_preserves_the_lock_payload() {
    let mut lock_info = LockInfo::default();
    lock_info.primary_lock = b"primary".to_vec();
    lock_info.key = b"locked-key".to_vec();
    lock_info.lock_version = 42;

    let event = WrapCopMeetLock(Some(Box::new(CopMeetLock {
        LockInfo: Some(Box::new(lock_info)),
    })));
    let wrapped = event.GetCopMeetLock().expect("CopMeetLock event");
    let lock_info = wrapped.LockInfo.as_deref().expect("LockInfo payload");

    assert_eq!(lock_info.primary_lock, b"primary");
    assert_eq!(lock_info.key, b"locked-key");
    assert_eq!(lock_info.lock_version, 42);
}

/// 传入 None（Go typed nil）时 GetCopMeetLock 返回 None。
#[test]
fn wrap_nil_cop_meet_lock_returns_nil_like_go_typed_nil() {
    let event = WrapCopMeetLock(None);
    assert!(event.GetCopMeetLock().is_none());
}

/// EventCallback 能收到包装后的事件并执行一次。
#[test]
fn event_callback_receives_the_wrapped_event() {
    let calls = Arc::new(Mutex::new(0));
    let callback_calls = Arc::clone(&calls);
    let callback: EventCallback = Box::new(move |event| {
        assert!(event.GetCopMeetLock().is_some());
        *callback_calls.lock().expect("callback counter") += 1;
    });

    callback(WrapCopMeetLock(Some(Box::new(CopMeetLock {
        LockInfo: None,
    }))));
    assert_eq!(*calls.lock().expect("final callback counter"), 1);
}
