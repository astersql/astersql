// Copyright 2026 AsterSQL.

use crate::upgrade_handler::{NewClusterUpgradeHandler, Request, ServerInfo, Storage, VersionInfo};
use crate::util::ResponseWriter;

fn server(id: &str, git_hash: &str, json_server_id: u64) -> ServerInfo {
    ServerInfo {
        version: VersionInfo {
            version: "v8.5.0".to_owned(),
            git_hash: git_hash.to_owned(),
        },
        id: id.to_owned(),
        ip: "127.0.0.1".to_owned(),
        port: 4000 + json_server_id as u32,
        json_server_id,
    }
}

#[test]
fn show_treats_different_git_hashes_as_different_version_info() {
    let storage = Storage::new();
    storage.set_owner_id("owner-1");
    storage.set_servers(vec![
        server("ddl-1", "hash-a", 1),
        server("ddl-2", "hash-b", 2),
    ]);
    let handler = NewClusterUpgradeHandler(storage);
    assert!(!handler.StartUpgrade().expect("upgrade starts"));

    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &Request::post("show"));
    let body = String::from_utf8(writer.body_bytes().to_vec()).expect("JSON response");

    assert!(body.contains("\"servers_num\":2"), "{body}");
    assert!(body.contains("\"upgraded_percent\":50"), "{body}");
    assert!(body.contains("\"all_servers_diff_info\""), "{body}");
    assert!(
        !body.contains("\"is_all_server_version_consistent\":true"),
        "{body}"
    );
}
