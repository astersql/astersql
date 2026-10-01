// Copyright 2026 AsterSQL.
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};
#[test]
fn normal_ddl_plan_create_table_affinity_http_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let worker = std::thread::spawn(move || {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 1024];
            let header_end = loop {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
            let len = headers
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + len {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let path = std::str::from_utf8(&bytes[..header_end])
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned();
            let body = if len == 0 {
                Value::Null
            } else {
                serde_json::from_slice(&bytes[header_end..header_end + len]).unwrap()
            };
            recorded.lock().unwrap().push((path, body));
            let reply = br#"{"affinity_groups":{"task12":{"id":"task12","range_count":1}}}"#;
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",reply.len()).unwrap();
            socket.write_all(reply).unwrap();
        }
    });
    let client = HttpClient::new(vec![address.to_string()], None).unwrap();
    let ctx = crate::BackgroundContext;
    let groups = HashMap::from([(
        "task12".into(),
        vec![AffinityGroupKeyRange::new([0, 255], [255, 0])],
    )]);
    assert_eq!(
        client.create_affinity_groups(&ctx, &groups, true).unwrap()["task12"].range_count,
        1
    );
    client
        .get_affinity_groups(&ctx, &["task12".into()])
        .unwrap();
    client
        .batch_delete_affinity_groups(&ctx, &["task12".into()], true)
        .unwrap();
    client.get_all_affinity_groups(&ctx).unwrap();
    worker.join().unwrap();
    let r = requests.lock().unwrap();
    assert!(
        r[0].0
            .contains("POST /pd/api/v2/affinity-groups?skip_exist_check=true")
    );
    assert_eq!(
        r[0].1["affinity_groups"]["task12"]["ranges"][0]["start_key"],
        "AP8="
    );
    assert_eq!(
        r[0].1["affinity_groups"]["task12"]["ranges"][0]["end_key"],
        "/wA="
    );
    assert!(r[1].0.contains("?ids=task12"));
    assert!(r[2].0.contains("?delete"));
    assert_eq!(r[2].1["force"], true);
}
