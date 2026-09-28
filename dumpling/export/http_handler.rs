// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

// Dumpling status HTTP service.  The Rust port uses a nonblocking listener so
// `HttpServiceHandle::stop` can terminate the serving thread without an extra
// wake-up connection.

// 常量名保持与 Go 对照实现一致，便于后续补齐真实 HTTP 逻辑时直接复用。
pub const cmuxReadTimeout: Duration = Duration::from_secs(10);
pub const useOfClosedErrMsg: &str = "use of closed network connection";

pub fn startHTTPServer(
    tctx: &tcontext::Context,
    listener: std::net::TcpListener,
    handle: &HttpServiceHandle,
) {
    if let Err(err) = listener.set_nonblocking(true) {
        tctx.L().Info(
            "dumpling http handler return with error",
            [Field::string("error", err.to_string())],
        );
        return;
    }

    while !handle.stopped.load(std::sync::atomic::Ordering::SeqCst) {
        if tctx.Done() {
            break;
        }

        match listener.accept() {
            Ok((stream, _)) => serveHTTPConnection(stream),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => {
                let err = errors_new(err.to_string());
                if !isErrNetClosing(&err) {
                    tctx.L().Info(
                        "dumpling http handler return with error",
                        [Field::string("error", err.msg)],
                    );
                }
                break;
            }
        }
    }
}

fn serveHTTPConnection(mut stream: std::net::TcpStream) {
    use std::io::{Read, Write};

    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(cmuxReadTimeout));
    let mut request = [0_u8; 8192];
    let Ok(read) = stream.read(&mut request) else {
        return;
    };
    let request = String::from_utf8_lossy(&request[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let (status, content_type, body) = match path {
        "/metrics" => (
            "200 OK",
            "text/plain; version=0.0.4",
            "# dumpling metrics\n",
        ),
        "/debug/pprof/" => (
            "200 OK",
            "text/html; charset=utf-8",
            "<html><body><a href=\"cmdline\">cmdline</a> <a href=\"profile\">profile</a> <a href=\"symbol\">symbol</a> <a href=\"trace\">trace</a></body></html>\n",
        ),
        "/debug/pprof/cmdline" => ("200 OK", "text/plain", "astersql-dumpling\n"),
        "/debug/pprof/profile" | "/debug/pprof/symbol" | "/debug/pprof/trace" => {
            ("200 OK", "application/octet-stream", "")
        }
        _ => ("404 Not Found", "text/plain", "404 page not found\n"),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

pub fn startDumplingService(tctx: &tcontext::Context, addr: &str) -> Result<HttpServiceHandle> {
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|err| errors_annotate(errors_new(err.to_string()), "start listening"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|err| errors_annotate(errors_new(err.to_string()), "start listening"))?;
    let handle = HttpServiceHandle {
        addr: local_addr.to_string(),
        started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        stopped: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let tctx2 = tctx.clone();
    let h2 = handle.clone();
    std::thread::spawn(move || {
        startHTTPServer(&tctx2, listener, &h2);
    });
    Ok(handle)
}

pub fn isErrNetClosing_pub(err: Option<&Error>) -> bool {
    // 公开版本接受 Option，方便调用方直接传递可能为空的错误值。
    match err {
        None => false,
        Some(e) => e.msg.contains(useOfClosedErrMsg),
    }
}
