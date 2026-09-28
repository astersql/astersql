// Copyright 2026 AsterSQL.

// `AuthConn` 接口行为的迁移对照单元测试。
//
// 用可记录调用的假连接验证：写入 AuthMoreData、读取包、按上下文 Flush，
// 以及实现侧错误能原样向上传播。

use super::conn::AuthConn;
use std::io;

/// 测试用 Flush 上下文，携带请求 ID 以便断言刷出时机。
#[derive(Default)]
struct TestContext {
    request_id: u64,
}

/// 记录 AuthMoreData / 读包 / Flush 调用的假连接实现。
#[derive(Default)]
struct RecordingConn {
    auth_more_data: Vec<Vec<u8>>,
    packets: Vec<Vec<u8>>,
    flushed_request_ids: Vec<u64>,
    fail_write: bool,
}

impl AuthConn for RecordingConn {
    type Context = TestContext;
    type Error = io::Error;

    fn WriteAuthMoreData(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        // 可选注入写失败，用于错误传播用例。
        if self.fail_write {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "write failed"));
        }
        self.auth_more_data.push(data.to_vec());
        Ok(())
    }

    fn ReadPacket(&mut self) -> Result<Vec<u8>, Self::Error> {
        if self.packets.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no packet"));
        }
        Ok(self.packets.remove(0))
    }

    fn Flush(&mut self, ctx: &Self::Context) -> Result<(), Self::Error> {
        self.flushed_request_ids.push(ctx.request_id);
        Ok(())
    }
}

/// 对照 Go 接口：写挑战、读响应、带上下文 Flush 的数据流。
#[test]
fn auth_conn_matches_go_interface_data_packet_and_context_flow() {
    let mut conn = RecordingConn {
        packets: vec![b"client-response".to_vec()],
        ..RecordingConn::default()
    };

    conn.WriteAuthMoreData(b"server-challenge").unwrap();
    assert_eq!(conn.auth_more_data, vec![b"server-challenge".to_vec()]);
    assert_eq!(conn.ReadPacket().unwrap(), b"client-response");

    conn.Flush(&TestContext { request_id: 42 }).unwrap();
    assert_eq!(conn.flushed_request_ids, vec![42]);
}

/// 确认实现侧 I/O 错误（BrokenPipe）能原样返回给调用方。
#[test]
fn auth_conn_preserves_implementation_errors() {
    let mut conn = RecordingConn {
        fail_write: true,
        ..RecordingConn::default()
    };

    let error = conn.WriteAuthMoreData(b"ignored").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "write failed");
}
