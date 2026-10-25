// Copyright 2026 AsterSQL.
//! Real external libpq client regression. Install libpq 18 and Python 3;
//! PG_LIBPQ_LIBRARY selects the system library. No protocol code is reimplemented.
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn postgres_client_protocol_versions() {
    pg_introspection_clients();
}

#[test]
fn pg_introspection_clients() {
    run_clients(true);
}

#[test]
fn pg_introspection_clients_mysql_only() {
    run_clients(false);
}

#[test]
fn pg_introspection_clients_stored_view_source() {
    for (stored, expected) in [
        (
            "CREATE VIEW `as` AS SELECT 'AS WITH LOCAL CHECK OPTION' AS label",
            "SELECT 'AS WITH LOCAL CHECK OPTION' AS label",
        ),
        (
            "CREATE VIEW v AS SELECT id FROM t WITH LOCAL CHECK OPTION;",
            "SELECT id FROM t",
        ),
        (
            "CREATE VIEW v AS SELECT id FROM t WITH CASCADED CHECK OPTION",
            "SELECT id FROM t",
        ),
        (
            "CREATE VIEW v AS /* AS ignored */ SELECT 1",
            "/* AS ignored */ SELECT 1",
        ),
        ("SELECT 1", "SELECT 1"),
    ] {
        assert_eq!(
            crate::pg_name::stored_view_select(stored).unwrap(),
            expected
        );
    }
    assert!(crate::pg_name::stored_view_select("DROP TABLE t").is_err());
    assert!(crate::pg_name::stored_view_select("SELECT 1; SELECT 2").is_err());
}

struct CloseServer<'a>(&'a crate::server::Server);

impl Drop for CloseServer<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[test]
fn pg_core_libpq_insert_readback() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr().unwrap();
    let database = format!("pg_core_{}_{}", std::process::id(), address.port());
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    // Capture spawn/client failures until after teardown; the server guard also
    // closes listeners and their sessions if setup or cleanup panics.
    let result = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_libpq.py"
        ))
        .arg(address.port().to_string())
        .arg(&database)
        .env_remove("PG_JDBC_LIBRARIES")
        .env_remove("PG_JAVA")
        .env_remove("JAVA_HOME")
        .env_remove("CLASSPATH")
        .output();
    server.close();
    native
        .execute(&format!("DROP DATABASE {database}"))
        .unwrap();
    assert!(TcpStream::connect(address).is_err(), "PG listener leaked");
    let output = result.expect("Python 3 is required for the independent libpq client");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "independent libpq workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn pg_core_libpq_integer_differential() {
    use std::io::BufRead;
    use std::process::Stdio;

    assert!(
        std::env::var_os("PG18_REFERENCE_DSN").is_some(),
        "PG18_REFERENCE_DSN is required (no skip)"
    );
    let run = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/pg-core-evidence")
        .join(&run);
    std::fs::create_dir_all(evidence.parent().unwrap()).unwrap();
    std::fs::create_dir(&evidence).unwrap();
    println!("PG_CORE_EVIDENCE_DIR={}", evidence.display());
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repository)
        .output()
        .expect("git HEAD is required for provenance");
    assert!(head.status.success());
    let build = serde_json::json!({
        "head": String::from_utf8(head.stdout).unwrap().trim(),
        "path": std::env::current_exe().unwrap(),
        "profile": "cargo-test-default-features",
        "storage": "CreateAnalyzeSession test storage",
        "authentication": "InsecureRootOnly"
    });
    std::fs::write(evidence.join("rust-build.json"), build.to_string()).unwrap();
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr().unwrap();
    let database = format!("pg_diff_{}_{}", std::process::id(), address.port());
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    let mysql_address = server.listener_addr().unwrap();
    // Declared before the child guard: on unwind, Python finishes its cleanup first.
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = || {
        server.close();
        let dropped = native.execute(&format!("DROP DATABASE {database}")).is_ok();
        let record = serde_json::json!({"database_dropped": dropped,
            "postgres_port_closed": TcpStream::connect(address).is_err(),
            "mysql_port_closed": TcpStream::connect(mysql_address).is_err()});
        let _ = std::fs::write(evidence.join("rust-cleanup.json"), record.to_string());
    };
    let cleanup_guard = Cleanup(cleanup);
    let mut mysql = TcpStream::connect(mysql_address).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut mysql);
    mysql_isolation(&mut mysql);
    let checkpoint = |mysql: &mut TcpStream, phase: &str| {
        let sql = "SELECT id, CAST('-17' AS SIGNED) FROM public.client_isolation";
        let actual = mysql_query(mysql, sql);
        let record = serde_json::json!({"run": run, "case_id": phase,
            "side": "mysql", "attempt": 1, "input": {"command": sql},
            "observation": {"rows": actual}, "outcome": "bypass_health"});
        use std::fs::OpenOptions;
        let mut archive = OpenOptions::new()
            .create(true)
            .append(true)
            .open(evidence.join("mysql.jsonl"))
            .unwrap();
        writeln!(archive, "{record}").unwrap();
        archive.sync_all().unwrap();
        assert_eq!(
            actual,
            vec![vec![Some(b"31".to_vec()), Some(b"-17".to_vec())]]
        );
    };
    checkpoint(&mut mysql, "before");
    struct ChildGuard(std::process::Child, std::path::PathBuf);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            // EOF releases the Python checkpoint even if a MySQL assertion panics,
            // allowing its finally block to clean the reference endpoint as well.
            drop(self.0.stdin.take());
            if let Ok(status) = self.0.wait() {
                let _ = std::fs::write(
                    &self.1,
                    serde_json::json!({
                    "success": status.success(), "code": status.code(), "status": status.to_string()})
                    .to_string(),
                );
            }
        }
    }
    let mut child = ChildGuard(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/pg_core_libpq.py"
            ))
            .args([
                address.port().to_string(),
                database.clone(),
                "integer-differential".into(),
            ])
            .env("PG_CORE_EVIDENCE_DIR", &evidence)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Python 3 is required"),
        evidence.join("child-exit.json"),
    );
    let stdout = child.0.stdout.take().unwrap();
    let mut middle = false;
    for line in std::io::BufReader::new(stdout).lines() {
        let line = line.unwrap();
        println!("{line}");
        if line == "PG_DIFFERENTIAL_MID" {
            checkpoint(&mut mysql, "during");
            middle = true;
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"continue\n")
                .unwrap();
        }
    }
    let status = child.0.wait().unwrap();
    checkpoint(&mut mysql, "after");
    mysql_query(&mut mysql, "DROP DATABASE public");
    mysql_packet(&mut mysql, 0, &[0x01]);
    std::fs::write(
        evidence.join("child-exit.json"),
        serde_json::json!({"success": status.success(), "code": status.code(), "status": status.to_string()}).to_string(),
    )
    .unwrap();
    drop(cleanup_guard);
    let cleanup: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("rust-cleanup.json")).unwrap())
            .unwrap();
    assert_eq!(cleanup["database_dropped"], true);
    assert_eq!(cleanup["postgres_port_closed"], true);
    assert_eq!(cleanup["mysql_port_closed"], true);
    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["run"], run);
    if std::env::var("PG_CORE_INJECT_CLEANUP_FAILURE").as_deref() == Ok("reference") {
        assert!(
            !status.success(),
            "cleanup failure must fail the Python client"
        );
        assert_eq!(summary["passed"], false);
        assert_eq!(summary["cleanup_ok"], false);
        assert_eq!(summary["unique_effective"], 0);
        assert!(summary["failed"].as_u64().unwrap() >= 1);
    } else {
        assert!(status.success(), "integer differential client failed");
        assert_eq!(summary["passed"], true);
        assert_eq!(summary["discovered"], 104);
        assert_eq!(summary["executed"], 104);
        assert_eq!(summary["unique_effective"], 101);
        assert_eq!(summary["self_checks"], 3);
        assert!(summary["assertions"].as_u64().unwrap() > 0);
        for field in ["failed", "exceptions", "timeouts", "skipped", "ignored"] {
            assert_eq!(summary[field], 0, "unexpected {field}");
        }
    }
    let mysql_records: Vec<serde_json::Value> =
        std::fs::read_to_string(evidence.join("mysql.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(mysql_records.len(), 3);
    for (record, phase) in mysql_records.iter().zip(["before", "during", "after"]) {
        assert_eq!(record["run"], run);
        assert_eq!(record["case_id"], phase);
        assert_eq!(record["outcome"], "bypass_health");
    }
    assert!(middle, "missing middle MySQL checkpoint");
    let matrix_status = Command::new("python3")
        .args(["-c", "import sys; from pg_core_evidence import finalize_capabilities; finalize_capabilities(sys.argv[1])"])
        .arg(&evidence)
        .current_dir(repository.join("pkg/server/testdata"))
        .status()
        .unwrap();
    assert!(matrix_status.success(), "capability finalization failed");
    let matrix: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("capabilities.json")).unwrap())
            .unwrap();
    assert_eq!(matrix["run"], run);
    assert_eq!(
        matrix["capabilities"]["mysql.bypass_health"]["status"],
        "unverified"
    );
    let eligible: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("provenance-gate.json")).unwrap(),
    )
    .unwrap();
    let support = eligible["eligible"] == true && summary["passed"] == true;
    for (key, capability) in matrix["capabilities"].as_object().unwrap() {
        if key != "mysql.bypass_health" {
            assert_eq!(
                capability["status"],
                if support { "supported" } else { "unverified" },
                "{key}"
            );
        }
    }

    println!(
        "MySQL SELECT/CAST before, during and after PG; prepared regression and listener cleanup passed"
    );
}

pub(super) fn start_client_server(
    domain: Arc<astersql_domain::Domain>,
    postgres_port: Option<u16>,
) -> Arc<crate::server::Server> {
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    struct Driver;
    impl crate::server::ServerDriver for Driver {
        fn name(&self) -> &str {
            "tidb"
        }
    }
    let server = crate::server::Server::new_test(
        crate::server::ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            postgres_port,
            status: crate::server::StatusConfig {
                report_status: false,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(Driver),
    );
    server
        .set_connection_runtime(
            driver,
            Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        )
        .unwrap();
    server
        .run(Arc::new(crate::runtime::CanonicalServerDomain::new(domain)))
        .unwrap();
    server
}

fn run_clients(pg_enabled: bool) {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let ui_port = std::env::var("PG_DATAGRIP_UI_PORT").ok().map(|port| {
        port.parse::<u16>()
            .expect("PG_DATAGRIP_UI_PORT must be a port")
    });
    let database = if ui_port.is_some() {
        "pg_datagrip_ui".into()
    } else {
        format!("pg_clients_{}_{}", std::process::id(), u8::from(pg_enabled))
    };
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    for sql in [
        format!("CREATE TABLE {database}.client_view_base (id INT)"),
        format!(
            "CREATE VIEW `{database}`.`client_view_live` AS SELECT id FROM `{database}`.`client_view_base`"
        ),
        format!("CREATE TABLE {database}.client_isolation (id INT)"),
        format!("INSERT INTO {database}.client_isolation VALUES (17)"),
    ] {
        native.execute(&sql).unwrap();
    }
    if ui_port.is_some() {
        native.execute(&format!("CREATE TABLE {database}.dg_ui_structure (id INT PRIMARY KEY, note VARCHAR(30) NOT NULL DEFAULT 'ready', UNIQUE KEY note_unique (note, id))")).unwrap();
    }
    let view_source = format!("SELECT id FROM `{database}`.`client_view_base`");
    let server = start_client_server(domain, pg_enabled.then_some(ui_port.unwrap_or(0)));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr();
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    mysql_handshake(&mut mysql);
    mysql_isolation(&mut mysql);
    let Some(address) = address else {
        assert!(!pg_enabled);
        mysql_query(&mut mysql, "DROP DATABASE public");
        mysql_packet(&mut mysql, 0, &[0x01]);
        native
            .execute(&format!("DROP DATABASE {database}"))
            .unwrap();
        return;
    };
    if ui_port.is_some() {
        let control = std::path::PathBuf::from(
            std::env::var("PG_DATAGRIP_UI_CONTROL")
                .expect("PG_DATAGRIP_UI_CONTROL must name a temporary control file"),
        );
        assert!(!control.exists(), "UI control file must be fresh");
        println!(
            "DATAGRIP_UI_READY pid={} port={} database={} table=dg_ui_structure",
            std::process::id(),
            address.port(),
            database
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(1200);
        while !control.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(control.exists(), "DataGrip UI session timed out");
        std::fs::remove_file(control).unwrap();
    }
    // The real libpq and JDBC clients use disjoint temporary object names and
    // only share catalog reads, so run the independent client workflows while
    // the same server fixture is alive instead of paying their latency serially.
    let (result, jdbc_result) = std::thread::scope(|scope| {
        let libpq = scope.spawn(|| {
            Command::new("python3")
                .arg("-c")
                .arg(LIBPQ_WORKFLOW)
                .arg(address.port().to_string())
                .arg(crate::pg_catalog::DATABASES_SQL)
                .args([
                    DATAGRIP_VIEW_SOURCES_SQL,
                    DATAGRIP_FUNCTION_SOURCES_SQL,
                    DATAGRIP_RELATIONS_SQL,
                ])
                .args([database.as_str(), view_source.as_str()])
                .output()
        });
        let jdbc_result = run_jdbc(address.port(), &database, &view_source);
        (
            libpq.join().expect("libpq workflow thread panicked"),
            jdbc_result,
        )
    });
    assert_eq!(
        mysql_query(&mut mysql, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(&mut mysql, "DROP DATABASE public");
    // The authenticated MySQL connection stays usable after the PG workload.
    mysql_packet(&mut mysql, 0, &[0x0e]); // COM_PING
    assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
    mysql_packet(&mut mysql, 0, &[0x01]); // COM_QUIT
    server.close();
    native
        .execute(&format!("DROP DATABASE {database}"))
        .unwrap();
    let output = result.expect("Python 3 is required to call the external libpq client");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "external libpq workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    jdbc_result.unwrap();
}

fn run_jdbc(port: u16, database: &str, view_source: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let libraries = std::env::var_os("PG_JDBC_LIBRARIES")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_else(|| ["42.7.13", "42.7.3"].iter().map(|version| {
            std::path::PathBuf::from(&home).join(format!("Library/Application Support/JetBrains/DataGrip2025.1/jdbc-drivers/PostgreSQL/{version}/org/postgresql/postgresql/{version}/postgresql-{version}.jar"))
        }).collect());
    let java = std::env::var("PG_JAVA").unwrap_or_else(|_| {
        "/Applications/DataGrip.app/Contents/jbr/Contents/Home/bin/java".into()
    });
    let directory =
        std::env::temp_dir().join(format!("pg-introspection-{}-{port}", std::process::id()));
    std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let source = directory.join("PgIntrospection.java");
    std::fs::write(&source, JDBC_WORKFLOW).map_err(|e| e.to_string())?;
    if libraries.is_empty() {
        return Err("PG_JDBC_LIBRARIES must include a real installed JDBC driver".into());
    }
    for library in &libraries {
        if !library.is_file() {
            return Err(format!(
                "missing installed PostgreSQL JDBC driver: {}",
                library.display()
            ));
        }
    }
    let outputs = std::thread::scope(|scope| {
        let workers = libraries
            .iter()
            .enumerate()
            .map(|(index, library)| {
                let java = &java;
                let source = &source;
                scope.spawn(move || {
                    Command::new(java)
                        .arg("--class-path")
                        .arg(library)
                        .arg(source)
                        .arg(port.to_string())
                        .args([
                            DATAGRIP_VIEW_SOURCES_SQL,
                            DATAGRIP_FUNCTION_SOURCES_SQL,
                            DATAGRIP_RELATIONS_SQL,
                        ])
                        .args([database, view_source])
                        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/pg_datagrip"))
                        .arg(index.to_string())
                        .output()
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("JDBC workflow thread panicked"))
            .collect::<Vec<_>>()
    });
    let mut failures = Vec::new();
    for output in outputs {
        let output = output.map_err(|e| format!("start installed Java runtime: {e}"))?;
        println!("{}", String::from_utf8_lossy(&output.stdout));
        if !output.status.success() {
            failures.push(format!(
                "JDBC workflow failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

const JDBC_WORKFLOW: &str = r#"
import java.sql.*;
import java.nio.file.*;
import org.postgresql.util.PGobject;
class PgIntrospection {
    static void check(boolean ok, String message) { if (!ok) throw new AssertionError(message); }
    static String viewSource;
    static void verify(ResultSet rows, int count, boolean view) throws Exception {
        check(rows.getMetaData().getColumnCount() == count, "Describe column count");
        if (view) {
            check(rows.next(), "real view missing");
            check(rows.getString(1).equals("v"), "view kind");
            check(rows.getLong(2) > 0, "view oid");
            check(rows.getString(3).equals(viewSource), "persisted SELECT source: " + rows.getString(3));
        }
        check(!rows.next(), "unexpected introspection rows");
    }
    public static void main(String[] args) throws Exception {
        viewSource = args[5];
        String parseTable = "jdbc_parse_ddl_" + args[7];
        String liveTableName = "jdbc_client_live_" + args[7];
        String options = "sslmode=disable&gssEncMode=disable&prepareThreshold=1&connectTimeout=5&socketTimeout=10";
        try (Connection c = DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+args[0]+"/"+args[4]+"?"+options, "root", ""); Statement s = c.createStatement()) {
            // Preserve the installed driver defaults for binary parameters/results.
            System.out.println("installed PostgreSQL JDBC " + c.getMetaData().getDriverVersion() + "; " + options);
            long namespace;
            try (ResultSet r = s.executeQuery("select oid from pg_namespace where nspname='public'")) { check(r.next(), "public missing"); namespace = r.getLong(1); }
            for (int i=1; i<=3; i++) {
                try (ResultSet r = s.executeQuery(args[i].replace("?", Long.toString(namespace)))) { verify(r, i==1?3:i==2?5:3, i==1); }
                try (PreparedStatement p = c.prepareStatement(args[i])) {
                    PGobject oid = new PGobject(); oid.setType("oid"); oid.setValue(Long.toString(namespace)); p.setObject(1, oid);
                    check(p.getMetaData().getColumnCount() == (i==2?5:3), "Statement Describe");
                    for (int repeat=0; repeat<2; repeat++) { try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, i==1); } }
                    p.setLong(1, namespace);
                    for (int repeat=0; repeat<2; repeat++) { try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, i==1); } }
                    p.setNull(1, Types.OTHER, "oid"); try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, false); }
                    oid.setValue("4294967295"); p.setObject(1, oid); try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, false); }
                }
            }
            try (PreparedStatement p=c.prepareStatement("SELECT relname FROM pg_class WHERE relname='"+parseTable+"'")) {
                check(p.getMetaData().getColumnCount()==1, "Parse-before-DDL Describe");
                try(ResultSet r=p.executeQuery()) { check(!r.next(), "before DDL rows"); }
                s.execute("CREATE TABLE public."+parseTable+" (id INT)");
                try { try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1).equals(parseTable) && !r.next(), "live rows after DDL"); } }
                finally { s.execute("DROP TABLE public."+parseTable); }
                try(ResultSet r=p.executeQuery()) { check(!r.next(), "live rows after DROP"); }
            }
            s.execute("CREATE TABLE public."+liveTableName+" (id INT PRIMARY KEY, note VARCHAR(30))");
            try {
                try (PreparedStatement p=c.prepareStatement("SELECT c.oid, c.relname='"+liveTableName+"' AS matched, i.indkey, i.indclass, i.indoption FROM pg_class c JOIN pg_index i ON i.indrelid=c.oid WHERE c.relname='"+liveTableName+"'")) {
                    for (int repeat=0;repeat<3;repeat++) { try (ResultSet r=p.executeQuery()) {
                        check(r.next() && r.getLong(1)>0 && r.getBoolean(2), "default binary catalog identity");
                        check(r.getString(3).equals("1"), "int2vector contents");
                        check(r.getString(4)!=null && r.getArray(5)!=null, "catalog vector/array contents");
                        check(!r.next(), "single primary index");
                    } }
                }
                long liveTable, liveIndex;
                try (ResultSet r=s.executeQuery("SELECT c.oid, i.indexrelid FROM pg_class c JOIN pg_index i ON i.indrelid=c.oid WHERE c.relname='"+liveTableName+"'")) {
                    check(r.next(), "live structure identity"); liveTable=r.getLong(1); liveIndex=r.getLong(2);
                }
                boolean fullFailed=false;
                try (var files=Files.list(Path.of(args[6]))) {
                    var queries=files.filter(f -> f.toString().endsWith(".sql")).sorted().toList();
                    check(queries.size()==27, "frozen DataGrip query count: "+queries.size());
                    for (Path file : queries) {
                        try {
                            String sql=Files.readString(file);
                            for (int mode=0;mode<2;mode++) {
                                try (PreparedStatement p=c.prepareStatement(mode==0?sql.replace("?", Long.toString(namespace)):sql)) {
                                    if (mode==1) for (int parameter=1;parameter<=p.getParameterMetaData().getParameterCount();parameter++) p.setLong(parameter, namespace);
                                    for (int repeat=0;repeat<2;repeat++) { try (ResultSet r=p.executeQuery()) {
                                        int count=r.getMetaData().getColumnCount(); check(count>0, "empty Describe: "+file);
                                        boolean found=false;
                                        while (r.next()) {
                                            for (int col=1;col<=count;col++) r.getObject(col);
                                            String name=file.getFileName().toString();
                                            if (name.equals("1869280142.sql") && liveTableName.equals(r.getString("table_name"))) { check(r.getLong("table_id")>0, "real table OID"); found=true; }
                                            if (name.equals("1869280153.sql") && "PRIMARY".equals(r.getString("index_name"))) { check(r.getBoolean("is_primary") && r.getLong("index_id")>0, "real primary index"); found=true; }
                                            if (name.equals("1869280154.sql") && r.getLong("table_id")==liveTable && "p".equals(r.getString("con_kind"))) { check(r.getArray("con_columns")!=null && r.getLong("index_id")==liveIndex, "primary constraint columns/identity"); found=true; }
                                            if (name.equals("RetrieveColumns.sql") && r.getLong("table_id")==liveTable && "note".equals(r.getString("column_name"))) { check(r.getInt("column_position")==2 && "character varying(30)".equals(r.getString("type_spec")), "real column structure"); found=true; }
                                            if (name.equals("RetrieveIndexColumns.sql") && r.getLong("index_id")==liveIndex) { check(r.getInt("column_position")==1 && r.getBoolean("in_key"), "real index column"); found=true; }
                                        }
                                        String name=file.getFileName().toString();
                                        if (name.equals("1869280142.sql") || name.equals("1869280153.sql") || name.equals("1869280154.sql") || name.equals("RetrieveColumns.sql") || name.equals("RetrieveIndexColumns.sql")) check(found, "real structure missing: "+file);
                                    } }
                                }
                            }
                            System.out.println("full DataGrip SQL passed: "+file.getFileName());
                        } catch (SQLException | AssertionError e) {
                            fullFailed=true; System.out.println("full DataGrip SQL FAILED: "+file.getFileName()+": "+e);
                        }
                    }
                }
                try (PreparedStatement p=c.prepareStatement("INSERT INTO public."+liveTableName+" VALUES (?, ?)")) { p.setInt(1, 7); p.setNull(2, Types.VARCHAR); check(p.executeUpdate()==1, "prepared insert count"); }
                try (PreparedStatement p=c.prepareStatement("SELECT note FROM public."+liveTableName+" WHERE id=?")) { p.setInt(1,7); try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1)==null && r.wasNull() && !r.next(), "prepared NULL row"); } }
                try (PreparedStatement p=c.prepareStatement("UPDATE public."+liveTableName+" SET note=? WHERE id=?")) { p.setString(1,"updated"); p.setInt(2,7); check(p.executeUpdate()==1, "prepared update count"); }
                try (PreparedStatement p=c.prepareStatement("SELECT note FROM public."+liveTableName+" WHERE id=?")) { p.setInt(1,7); try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1).equals("updated") && !r.next(), "prepared update row"); } }
                try (PreparedStatement p=c.prepareStatement("DELETE FROM public."+liveTableName+" WHERE id=?")) { p.setInt(1,7); check(p.executeUpdate()==1, "prepared delete count"); }
                try (ResultSet r=s.executeQuery("SELECT id FROM public."+liveTableName)) { check(!r.next(), "delete row"); }
                try { s.executeQuery("SELECT oid FROM pg_catalog.pg_missing"); throw new AssertionError("missing catalog accepted"); } catch (SQLException e) { check("42P01".equals(e.getSQLState()), e.toString()); }
                try (ResultSet r=s.executeQuery("SELECT 1")) { check(r.next() && r.getInt(1)==1, "error recovery"); }
                check(!fullFailed, "full DataGrip SQL acceptance failed; see per-query evidence");
            } finally { s.execute("DROP TABLE public."+liveTableName); }
            System.out.println("JDBC original introspection Statement/PreparedStatement, oid/NULL/boundary, Describe/Parse-before-DDL, public CRUD, SQLSTATE 42P01 and recovery passed");
        }
    }
}
"#;

pub(super) fn mysql_read(socket: &mut TcpStream) -> Vec<u8> {
    let mut header = [0; 4];
    socket.read_exact(&mut header).unwrap();
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    assert!(length < 1 << 20);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    body
}
pub(super) fn mysql_packet(socket: &mut TcpStream, sequence: u8, body: &[u8]) {
    let length = body.len();
    socket
        .write_all(&[
            length as u8,
            (length >> 8) as u8,
            (length >> 16) as u8,
            sequence,
        ])
        .unwrap();
    socket.write_all(body).unwrap();
}
pub(super) fn mysql_handshake(socket: &mut TcpStream) {
    assert_eq!(mysql_read(socket).first(), Some(&10));
    let capabilities: u32 = (1 << 9) | (1 << 15) | (1 << 19);
    let mut response = capabilities.to_le_bytes().to_vec();
    response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
    response.push(45);
    response.extend_from_slice(&[0; 23]);
    response.extend_from_slice(b"root\0\0mysql_native_password\0");
    mysql_packet(socket, 1, &response);
    assert_eq!(mysql_read(socket).first(), Some(&0));
}

// Use the same real MySQL connection while the independent PG listener is live.
// Native `public` remains an ordinary database, not the PG current-db mapping.
fn mysql_isolation(socket: &mut TcpStream) {
    for sql in [
        "CREATE DATABASE public",
        "CREATE TABLE public.client_isolation (id INT)",
        "INSERT INTO public.client_isolation VALUES (31)",
        "USE public",
    ] {
        assert!(mysql_query(socket, sql).is_empty(), "{sql}");
    }
    assert_eq!(
        mysql_query(socket, "SELECT id FROM client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(socket, "BEGIN");
    mysql_query(socket, "UPDATE client_isolation SET id=32");
    mysql_query(socket, "ROLLBACK");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(socket, "USE test");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_packet(socket, 0, b"\x02public"); // COM_INIT_DB
    assert_eq!(mysql_read(socket)[0], 0);
    mysql_query(socket, "BEGIN");
    mysql_query(socket, "UPDATE client_isolation SET id=33");
    mysql_query(socket, "COMMIT");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM client_isolation"),
        vec![vec![Some(b"33".to_vec())]]
    );
    mysql_query(socket, "UPDATE client_isolation SET id=31");
    let mut request = vec![0x16]; // COM_STMT_PREPARE
    request.extend_from_slice(b"SELECT id FROM public.client_isolation WHERE id=?");
    mysql_packet(socket, 0, &request);
    let prepared = mysql_read(socket);
    assert_eq!(prepared[0], 0, "{prepared:?}");
    let id = &prepared[1..5];
    let columns = u16::from_le_bytes(prepared[5..7].try_into().unwrap());
    let parameters = u16::from_le_bytes(prepared[7..9].try_into().unwrap());
    assert_eq!((columns, parameters), (1, 1));
    for count in [parameters, columns] {
        for _ in 0..count {
            mysql_read(socket);
        }
        assert_eq!(mysql_read(socket)[0], 0xfe);
    }
    let mut execute = vec![0x17]; // COM_STMT_EXECUTE with a real integer parameter
    execute.extend_from_slice(id);
    execute.extend_from_slice(&[0, 1, 0, 0, 0, 0, 1, 3, 0]);
    execute.extend_from_slice(&31_i32.to_le_bytes());
    mysql_packet(socket, 0, &execute);
    assert_eq!(mysql_read(socket), vec![1]);
    mysql_read(socket); // column definition
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let row = mysql_read(socket);
    assert_eq!(&row[..2], &[0, 0]); // binary row, no NULL columns
    assert_eq!(i32::from_le_bytes(row[2..6].try_into().unwrap()), 31);
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let mut close = vec![0x19];
    close.extend_from_slice(id);
    mysql_packet(socket, 0, &close);
    println!(
        "MySQL native public database, USE, rollback, real relation rows and prepared integer parameter passed"
    );
}

pub(super) fn mysql_query(socket: &mut TcpStream, sql: &str) -> Vec<Vec<Option<Vec<u8>>>> {
    let mut packet = vec![3];
    packet.extend_from_slice(sql.as_bytes());
    mysql_packet(socket, 0, &packet);
    let head = mysql_read(socket);
    assert_ne!(head[0], 0xff, "{sql}: {}", String::from_utf8_lossy(&head));
    if head[0] == 0 {
        return Vec::new();
    }
    let count = usize::from(head[0]);
    assert!(count < 251);
    for _ in 0..count {
        mysql_read(socket);
    }
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let mut rows = Vec::new();
    loop {
        let row = mysql_read(socket);
        if row[0] == 0xfe && row.len() < 9 {
            break;
        }
        assert_ne!(row[0], 0xff, "{sql}: {row:?}");
        let mut offset = 0;
        let values = (0..count)
            .map(|_| {
                let length = row[offset];
                offset += 1;
                if length == 251 {
                    None
                } else {
                    assert!(length < 251);
                    let value = row[offset..offset + usize::from(length)].to_vec();
                    offset += usize::from(length);
                    Some(value)
                }
            })
            .collect();
        assert_eq!(offset, row.len());
        rows.push(values);
    }
    rows
}

// DataGrip 2025.1.3 database log, 2026-10-03 08:45:58, session 1533977248.
// Keep JDBC display SQL verbatim; the client workflow converts the sole ?
// to a literal for Query or $1 for Parse, without changing inactive comments.
// Statement 1869279758.
const DATAGRIP_VIEW_SOURCES_SQL: &str = r#"select
       T.relkind as view_kind,
       T.oid as view_id,
       pg_catalog.pg_get_viewdef(T.oid, true) as source_text
from pg_catalog.pg_class T
  join pg_catalog.pg_namespace N on T.relnamespace = N.oid
where N.oid = ?::oid
  and T.relkind in ('m','v')
  --  and T.relname in ( :[*f_names] )
  --  and (pg_catalog.age(T.xmin) <= #SRCTXAGE or exists(
  --  select A.attrelid from pg_catalog.pg_attribute A where A.attrelid = T.oid and pg_catalog.age(A.xmin) <= #SRCTXAGE))
"#;

// Statement 1869279759.
const DATAGRIP_FUNCTION_SOURCES_SQL: &str = r#"with system_languages as ( select oid as lang
                           from pg_catalog.pg_language
                           where lanname in ('c','internal') )
select oid as id,
       pg_catalog.pg_get_function_arguments(oid) as arguments_def,
       pg_catalog.pg_get_function_result(oid) as result_def,
       pg_catalog.pg_get_function_sqlbody(oid) /* null */ as sqlbody_def,
       prosrc as source_text
from pg_catalog.pg_proc
where pronamespace = ?::oid
  --  and pg_proc.proname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #SRCTXAGE
  and not (prokind = 'a') /* proisagg */
  and prolang not in (select lang from system_languages)
  and prosrc is not null
"#;

// Statement 1869279760.
const DATAGRIP_RELATIONS_SQL: &str = r#"select D.objid as dependent_id,
       D.refobjid as owner_id,
       D.refobjsubid as owner_subobject_id
from pg_depend D
  join pg_class C_SEQ on D.objid    = C_SEQ.oid and D.classid    = 'pg_class'::regclass::oid
  join pg_class C_TAB on D.refobjid = C_TAB.oid and D.refclassid = 'pg_class'::regclass::oid
where C_SEQ.relkind = 'S'
  and C_TAB.relkind = 'r'
  and D.refobjsubid <> 0
  and (D.deptype = 'a' or D.deptype = 'i')
  and C_TAB.relnamespace = ?::oid
order by owner_id
"#;

const LIBPQ_WORKFLOW: &str = r#"
import ctypes as c, ctypes.util, os, sys
library = os.environ.get('PG_LIBPQ_LIBRARY')
if not library:
    library = '/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin' else ctypes.util.find_library('pq')
assert library, 'install PostgreSQL libpq 18 or set PG_LIBPQ_LIBRARY'
pq = c.CDLL(library)
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
ptr, text, integer = c.c_void_p, c.c_char_p, c.c_int
version = api('PQlibVersion', integer)
assert version() >= 180000, 'protocol 3.2 requires libpq 18'
print('external PostgreSQL libpq version:', version(), flush=True)
connect = api('PQconnectdb', ptr, text)
status = api('PQstatus', integer, ptr)
error = api('PQerrorMessage', text, ptr)
finish = api('PQfinish', None, ptr)
protocol = api('PQfullProtocolVersion', integer, ptr)
parameter_status = api('PQparameterStatus', text, ptr, text)
server_version = api('PQserverVersion', integer, ptr)
execute = api('PQexec', ptr, ptr, text)
params = api('PQexecParams', ptr, ptr, text, integer, c.POINTER(c.c_uint), c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
close_prepared = api('PQclosePrepared', ptr, ptr, text)
prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', ptr, ptr, text)
prepared_execute = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
result_status = api('PQresultStatus', integer, ptr)
result_error = api('PQresultErrorMessage', text, ptr)
rows = api('PQntuples', integer, ptr)
columns = api('PQnfields', integer, ptr)
value = api('PQgetvalue', text, ptr, integer, integer)
is_null = api('PQgetisnull', integer, ptr, integer, integer)
field_name = api('PQfname', text, ptr, integer)
field_type = api('PQftype', c.c_uint, ptr, integer)
error_field = api('PQresultErrorField', text, ptr, integer)
clear = api('PQclear', None, ptr)
txn = api('PQtransactionStatus', integer, ptr)
cancel_create = api('PQcancelCreate', ptr, ptr)
cancel_blocking = api('PQcancelBlocking', integer, ptr)
cancel_finish = api('PQcancelFinish', None, ptr)
base_conninfo = f'host=127.0.0.1 port={sys.argv[1]} user=root dbname={sys.argv[6]} sslmode=disable gssencmode=disable connect_timeout=5'
for options, expected_protocol in [('', 30000), (' min_protocol_version=3.2 max_protocol_version=3.2', 30002)]:
    conninfo = base_conninfo + options
    conn = connect(conninfo.encode())
    assert conn
    try:
        assert status(conn) == 0, error(conn).decode()
        assert protocol(conn) == expected_protocol, protocol(conn)
        assert parameter_status(conn, b'server_version') == b'18.0 (AsterSQL)'
        assert server_version(conn) == 180000, server_version(conn)
        def query(sql, expected=None, parameter=None, metadata=None, extended=False, sqlstate=None, parameter_oid=23, null_parameter=False, binary_parameter=False):
            if extended:
                result = params(conn, sql.encode(), 0, None, None, None, None, 0)
            elif parameter is None and not null_parameter:
                result = execute(conn, sql.encode())
            else:
                oids = (c.c_uint * 1)(parameter_oid)
                payload = None if null_parameter else (int(parameter).to_bytes(4, 'big') if binary_parameter else str(parameter).encode())
                values = (text * 1)(payload)
                lengths = (integer * 1)(0 if payload is None else len(payload))
                formats = (integer * 1)(int(binary_parameter))
                result = params(conn, sql.encode(), 1, oids, values, lengths, formats, 0)
            assert result
            try:
                if sqlstate is not None:
                    assert result_status(result) == 7, result_status(result)
                    assert error_field(result, ord('C')) == sqlstate.encode(), result_error(result).decode()
                    return
                assert result_status(result) in (1, 2), result_error(result).decode()
                if metadata is not None:
                    actual_metadata = [(field_name(result, col).decode(), field_type(result, col)) for col in range(columns(result))]
                    assert actual_metadata == metadata, (sql, actual_metadata, metadata)
                actual = [[None if is_null(result, r, col) else value(result, r, col).decode() for col in range(columns(result))] for r in range(rows(result))]
                if expected is not None:
                    assert actual == expected, (sql, actual, expected)
                return actual
            finally:
                clear(result)
        for invalid_info in [conninfo.replace('user=root', 'user=intruder')]:
            invalid = connect(invalid_info.encode())
            try:
                assert status(invalid) != 0, 'invalid identity must be rejected'
            finally:
                finish(invalid)
        query('SELECT 1', [['1']])
        query('SELECT id FROM public.client_isolation', [['17']])
        # Source SQL remains frozen. Functions have no native stored-program rows;
        # native independent sequences have no auto/internal column ownership.
        # Use the real current namespace ID; no catalog rows are mocked.
        namespace_id = int(query("select oid from pg_catalog.pg_namespace where nspname = 'public'")[0][0])
        for label, displayed, simple_state, parse_state in zip(
                ['RetrieveViewSources', 'RetrieveFunctionSources', 'RetrieveRelations'],
                sys.argv[3:6], [None, None, None], [None, None, None]):
            assert displayed.count('?') == 1, label
            source_metadata = {
                'RetrieveViewSources': [('view_kind', 25), ('view_id', 26), ('source_text', 25)],
                'RetrieveFunctionSources': [('id', 26), ('arguments_def', 25), ('result_def', 25), ('sqlbody_def', 25), ('source_text', 25)],
                'RetrieveRelations': [('dependent_id', 26), ('owner_id', 26), ('owner_subobject_id', 23)],
            }.get(label)
            source_rows = None
            if label == 'RetrieveViewSources':
                view_oid = query("select oid from pg_class where relname = 'client_view_live'")[0][0]
                source_rows = [['v', view_oid, sys.argv[7]]]
            query(displayed.replace('?', str(namespace_id)), sqlstate=simple_state,
                  expected=source_rows if source_rows is not None else [] if source_metadata else None, metadata=source_metadata)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter=namespace_id,
                  parameter_oid=26, sqlstate=parse_state,
                  expected=source_rows if source_rows is not None else [] if source_metadata else None, metadata=source_metadata)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter_oid=26, null_parameter=True, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=4294967295, parameter_oid=26, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=namespace_id, parameter_oid=26, binary_parameter=True,
                  expected=source_rows if source_rows is not None else [], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter_oid=26, null_parameter=True, binary_parameter=True, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=4294967295, parameter_oid=26, binary_parameter=True, expected=[], metadata=source_metadata)
            print(f'{expected_protocol}: {label}: Query/Execute(oid 26)={source_rows if source_rows is not None else "typed empty result"}; recovery passed', flush=True)
        for extended in [False, True]:
            for expression in ['pg_get_viewdef(NULL, true)', 'pg_get_viewdef(4294967295::oid, true)', 'pg_get_viewdef(oid, NULL)']:
                query(f"SELECT {expression} AS source_text FROM pg_class WHERE relname='client_view_live'", [[None]], metadata=[('source_text',25)], extended=extended)
            query("SELECT pg_get_viewdef(oid, 'wrong') FROM pg_class", sqlstate='0A000', extended=extended)
            query('SELECT 1', [['1']], extended=extended)
        # PQprepare performs Parse; PQdescribePrepared checks Statement Describe.
        # Keep this statement over DDL and read live rows at each Execute.
        ddl_sql = b"SELECT relname FROM pg_class WHERE relname='pg_parse_ddl_live'"
        result = prepare(conn, b'live_catalog', ddl_sql, 0, None)
        try: assert result_status(result)==1, result_error(result).decode()
        finally: clear(result)
        result = describe(conn, b'live_catalog')
        try:
            assert result_status(result)==1, result_error(result).decode()
            assert columns(result)==1 and field_name(result,0)==b'relname' and field_type(result,0)==25
        finally: clear(result)
        def live_rows():
            result = prepared_execute(conn,b'live_catalog',0,None,None,None,0)
            try:
                assert result_status(result)==2, result_error(result).decode()
                return [[value(result,r,col).decode() for col in range(columns(result))] for r in range(rows(result))]
            finally: clear(result)
        assert live_rows()==[]
        query('CREATE TABLE public.pg_parse_ddl_live (id INT)')
        try: assert live_rows()==[['pg_parse_ddl_live']]
        finally: query('DROP TABLE public.pg_parse_ddl_live')
        assert live_rows()==[]
        result = close_prepared(conn, b'live_catalog')
        try: assert result_status(result)==1, result_error(result).decode()
        finally: clear(result)
        print(f'{expected_protocol}: NULL/boundary view definitions, oid parameters, Describe and Parse-before-DDL live Execute passed', flush=True)
        namespace_sql = """select N.oid::bigint as id, N.xmin as state_number, nspname as name,
            D.description, pg_catalog.pg_get_userbyid(N.nspowner) as "owner"
            from pg_catalog.pg_namespace N left join pg_catalog.pg_description D on N.oid = D.objoid
            order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end"""
        tablespace_sql = 'SELECT oid::bigint AS id, spcname AS name, pg_catalog.pg_get_userbyid(spcowner) AS "owner", spcacl, spcoptions FROM pg_catalog.pg_tablespace ORDER BY oid'
        query('CREATE DATABASE pg_client_catalog_live')
        query('CREATE TABLE public.pg_client_relation_live (id INT)')
        for extended in [False, True]:
            relations = query("SELECT oid, relname, relnamespace, relkind FROM pg_class WHERE relname = 'pg_client_relation_live'", metadata=[('oid', 26), ('relname', 25), ('relnamespace', 26), ('relkind', 25)], extended=extended)
            assert len(relations) == 1 and relations[0][1:] == ['pg_client_relation_live', str(namespace_id), 'r'], relations
            assert int(relations[0][0]) > 0, relations
            databases = query(sys.argv[2], metadata=[('id', 20), ('name', 25), ('description', 25), ('is_template', 16), ('allow_connections', 16), ('owner', 25)], extended=extended)
            assert databases[0][1] == sys.argv[6], databases
            assert any(r[1] == 'pg_client_catalog_live' for r in databases), databases
            assert all(int(r[0]) != 0 and r[2:] == [None, 'f', 't', None] for r in databases), databases
            assert [int(r[0]) for r in databases[1:]] == sorted(int(r[0]) for r in databases[1:]), databases
            namespaces = query(namespace_sql, metadata=[('id', 20), ('state_number', 20), ('name', 25), ('description', 25), ('owner', 25)], extended=extended)
            assert namespaces[0][2] == 'public', namespaces
            assert {r[2] for r in namespaces} == {'public', 'pg_catalog'}, namespaces
            assert all(int(r[0]) > 0 and r[1] is None and r[3:] == [None, None] for r in namespaces), namespaces
            assert [int(r[0]) for r in namespaces[1:]] == sorted(int(r[0]) for r in namespaces[1:]), namespaces
            assert len({r[0] for r in namespaces}) == len(namespaces)
            query(tablespace_sql, [], metadata=[('id', 20), ('name', 25), ('owner', 25), ('spcacl', 25), ('spcoptions', 25)], extended=extended)
            # Extracted from DataGrip PgIntroQueries.sql: the ID-only probe is
            # supported; full introspection requires features beyond this phase.
            query('select oid::bigint from pg_catalog.pg_tablespace', [], metadata=[('oid', 20)], extended=extended)
            full_tablespace_sql = 'select T.oid::bigint as id, T.spcname as name, T.xmin as state_number, pg_catalog.pg_get_userbyid(T.spcowner) as owner, pg_catalog.pg_tablespace_location(T.oid) as location, T.spcoptions as options, D.description as comment from pg_catalog.pg_tablespace T left join pg_catalog.pg_shdescription D on D.objoid = T.oid'
            query(full_tablespace_sql, extended=extended, sqlstate='0A000')
            query('SELECT 1', [['1']], extended=extended)
            for sql, state in [('SELECT oid FROM pg_catalog.pg_missing', '42P01'), ('SELECT oid FROM pg_catalog.pg_namespace GROUP BY oid', '0A000'), ('SELECT (', '42601')]:
                query(sql, extended=extended, sqlstate=state)
                query('SELECT 1', [['1']], extended=extended)
        query('DROP TABLE public.pg_client_relation_live')
        query('DROP DATABASE pg_client_catalog_live')
        assert not any(r[1] == 'pg_client_catalog_live' for r in query(sys.argv[2]))
        query('CREATE TABLE pg_real_client (id INT PRIMARY KEY, v VARCHAR(30))')
        query("INSERT INTO pg_real_client VALUES (1, 'one')")
        query("UPDATE pg_real_client SET v = 'two' WHERE id = 1")
        query('SELECT v FROM pg_real_client WHERE id = $1', [['two']], parameter=1)
        query('BEGIN')
        assert txn(conn) == 2
        query("INSERT INTO pg_real_client VALUES (2, 'committed')")
        query('COMMIT')
        assert txn(conn) == 0
        query('BEGIN')
        query("UPDATE pg_real_client SET v = 'rolled back' WHERE id = 1")
        query('ROLLBACK')
        assert txn(conn) == 0
        query('SELECT v FROM pg_real_client ORDER BY id', [['two'], ['committed']])
        query('BEGIN')
        query("INSERT INTO pg_real_client VALUES (1, 'duplicate')", sqlstate='23505')
        assert txn(conn) == 3
        query('SELECT 1', sqlstate='25P02')
        query('COMMIT', sqlstate='25P02')
        assert txn(conn) == 3
        query('ROLLBACK')
        assert txn(conn) == 0
        query('SELECT v FROM pg_real_client ORDER BY id', [['two'], ['committed']])
        query('BEGIN')
        query('CREATE TABLE pg_transactional_ddl (id INT)', sqlstate='0A000')
        assert txn(conn) == 3
        query('ROLLBACK')
        assert txn(conn) == 0
        query('DELETE FROM pg_real_client WHERE id = 2')
        query('SELECT id FROM pg_real_client', [['1']])
        # libpq owns the version-specific BackendKeyData/CancelRequest framing.
        # An idle cancel must complete without poisoning the next query.
        cancel = cancel_create(conn)
        assert cancel
        try:
            assert cancel_blocking(cancel) == 1
        finally:
            cancel_finish(cancel)
        query('SELECT 1', [['1']])
        query('DROP TABLE pg_real_client')
        print(f'{expected_protocol}: startup, catalogs (simple/extended metadata, NULL, rows, ordering), error recovery, CRUD, typed parameters, I/T/E transactions and idle cancel passed', flush=True)
    finally:
        finish(conn)
"#;

#[test]
fn pg_dialect_01_libpq() {
    pg_dialect_libpq(1);
}

#[test]
fn pg_dialect_02_libpq() {
    pg_dialect_libpq(2);
}

#[test]
fn pg_dialect_03_libpq() {
    pg_dialect_libpq(3);
}

#[test]
fn pg_dialect_04_libpq() {
    pg_dialect_libpq(4);
}

#[test]
fn pg_dialect_05_libpq() {
    pg_dialect_libpq(5);
}

#[test]
fn pg_dialect_06_libpq() {
    pg_dialect_libpq(6);
}

#[test]
fn pg_dialect_07_libpq() {
    pg_dialect_libpq(7);
}

#[test]
fn pg_dialect_08_libpq() {
    pg_dialect_libpq(8);
}

#[test]
fn pg_dialect_09_libpq() {
    pg_dialect_libpq(9);
}

#[test]
fn pg_dialect_10_libpq() {
    pg_dialect_libpq(10);
}

#[test]
fn pg_dialect_11_libpq() {
    pg_dialect_libpq(11);
}

#[test]
fn pg_dialect_12_libpq() {
    pg_dialect_libpq(12);
}

#[test]
fn pg_dialect_13_libpq() {
    pg_dialect_libpq(13);
}

#[test]
fn pg_dialect_14_libpq() {
    pg_dialect_libpq(14);
}

#[test]
fn pg_dialect_15_libpq() {
    pg_dialect_libpq(15);
}

#[test]
fn pg_dialect_16_libpq() {
    pg_dialect_libpq(16);
}

#[test]
fn pg_dialect_17_libpq() {
    pg_dialect_libpq(17);
}

#[test]
fn pg_dialect_18_libpq() {
    pg_dialect_libpq(18);
}

#[test]
fn pg_dialect_19_libpq() {
    pg_dialect_libpq(19);
}

#[test]
fn pg_dialect_24_libpq() {
    pg_dialect_libpq(24);
}

fn pg_dialect_jsonl(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read evidence {}: {error}", path.display()))
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("parse evidence {}: {error}", path.display()))
        })
        .collect()
}

fn pg_dialect_records<'a>(
    records: &'a [serde_json::Value],
    case_id: &str,
    side: &str,
) -> Vec<&'a serde_json::Value> {
    records
        .iter()
        .filter(|record| record["case_id"] == case_id && record["side"] == side)
        .collect()
}

fn pg_dialect_07_origins_match(
    description: &serde_json::Value,
    expected_relation_oid: u64,
    has_base_column: bool,
) -> bool {
    let Some(origins) = description["table_origins"].as_array() else {
        return false;
    };
    if has_base_column {
        origins.len() == 1
            && origins[0][0].as_u64() == Some(expected_relation_oid)
            && origins[0][1] == 1
    } else {
        origins.is_empty()
    }
}

fn pg_dialect_07_audit(evidence: &std::path::Path) -> serde_json::Value {
    use std::collections::{HashMap, HashSet};

    let summary_path = evidence.join("summary.json");
    let mut summary: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&summary_path).expect("Python evidence summary"),
    )
    .expect("Python evidence summary JSON");
    let cases: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("cases.json")).expect("task 7 case registry"),
    )
    .expect("task 7 case registry JSON");
    let cases = cases.as_array().expect("task 7 case registry array");
    assert!(
        cases.len() >= 6,
        "task 7 requires at least six registered cases"
    );
    assert_eq!(summary["cases"].as_u64(), Some(cases.len() as u64));
    assert_eq!(summary["provenance_ok"], true);
    assert_eq!(summary["failures"], serde_json::json!([]));

    let aster = pg_dialect_jsonl(&evidence.join("aster.jsonl"));
    let reference = pg_dialect_jsonl(&evidence.join("reference.jsonl"));
    let mysql = pg_dialect_jsonl(&evidence.join("mysql.jsonl"));
    let mysql_local: Vec<serde_json::Value> =
        std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
            .expect("task 7 MySQL local evidence")
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .expect("task 7 MySQL local evidence JSON")
            })
            .filter(|record| record["mysql_on_duplicate_prepared"] == true)
            .collect();
    let provenance: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("provenance.json")).expect("task 7 provenance"),
    )
    .expect("task 7 provenance JSON");
    let mysql_identity: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("mysql-reference.json")).expect("MySQL identity"),
    )
    .expect("MySQL identity JSON");
    let ref_cleanup: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("reference-cleanup.json")).expect("PG cleanup"),
    )
    .expect("PG cleanup JSON");
    let rust_cleanup: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("rust-cleanup.json")).expect("server cleanup"),
    )
    .expect("server cleanup JSON");
    let child_exit: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("child-exit.json")).expect("Python child exit"),
    )
    .expect("Python child exit JSON");

    assert_eq!(child_exit["success"], true);
    assert_eq!(ref_cleanup["cleanup_ok"], true);
    assert_eq!(ref_cleanup["reference_schema_dropped"], true);
    assert_eq!(ref_cleanup["local_rows_preserved"], true);
    assert_eq!(ref_cleanup["reference_rows_preserved"], true);
    assert_eq!(rust_cleanup["database_dropped"], true);
    assert_eq!(rust_cleanup["postgres_port_closed"], true);
    assert_eq!(rust_cleanup["mysql_port_closed"], true);
    assert_eq!(
        mysql_identity["identity"][0]
            .as_str()
            .unwrap()
            .split('\t')
            .next(),
        Some("8.4.7")
    );
    assert_eq!(
        mysql_identity["identity"][1],
        "\tutf8mb4\tutf8mb4_general_ci"
    );
    assert_eq!(mysql_identity["sql_mode"], "");
    assert_eq!(mysql_identity["character_set_connection"], "utf8mb4");
    assert_eq!(mysql_identity["collation_connection"], "utf8mb4_general_ci");
    assert_eq!(provenance["target_version"], "18.6");
    assert_eq!(provenance["reference"]["server_version_num"], "180006");
    assert_eq!(provenance["client"]["PQlibVersion"], 180000);
    assert!(
        !provenance["source"]["workspace_sha256"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        !provenance["build"]["sha256"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        !provenance["reference_build"]["source_sha256"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        !provenance["reference_build"]["binary_sha256"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert_eq!(provenance["mysql_reference"]["version"], "8.4.7");
    assert!(
        !provenance["mysql_reference"]["image_digest"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );

    let mysql_by_case: HashMap<&str, &serde_json::Value> = mysql_local
        .iter()
        .map(|record| {
            (
                record["case_id"].as_str().expect("MySQL local case ID"),
                record,
            )
        })
        .collect();
    assert_eq!(
        mysql_by_case.len(),
        mysql_local.len(),
        "duplicate MySQL local case record"
    );
    assert_eq!(
        mysql_local.len(),
        cases.len(),
        "one local MySQL bypass record per registered case"
    );

    let aster_relation_record = pg_dialect_records(&aster, "fixture_relation_oid", "aster");
    let reference_relation_record =
        pg_dialect_records(&reference, "fixture_relation_oid", "reference");
    assert_eq!(aster_relation_record.len(), 1);
    assert_eq!(reference_relation_record.len(), 1);
    let aster_relation_oid: u64 = aster_relation_record[0]["observation"]["rows"][0][0]
        .as_str()
        .expect("Aster relation OID")
        .parse()
        .expect("numeric Aster relation OID");
    let reference_relation_oid: u64 = reference_relation_record[0]["observation"]["rows"][0][0]
        .as_str()
        .expect("reference relation OID")
        .parse()
        .expect("numeric reference relation OID");
    assert!(aster_relation_oid > 0 && reference_relation_oid > 0);
    for (records, side) in [(&aster, "aster"), (&reference, "reference")] {
        let attributes = pg_dialect_records(records, "fixture_relation_attributes", side);
        assert_eq!(attributes.len(), 1);
        assert_eq!(attributes[0]["observation"]["status"], 2);
        assert_eq!(
            attributes[0]["observation"]["rows"],
            serde_json::json!([["1", "id"]])
        );
        let expected_oid = if side == "aster" {
            aster_relation_oid
        } else {
            reference_relation_oid
        }
        .to_string();
        assert!(
            attributes[0]["input"]["sql"]
                .as_str()
                .unwrap()
                .contains(&expected_oid)
        );
    }

    let mut unique_ids = HashSet::new();
    let mut valid_parse_describe_ids = HashSet::new();
    let mut negative_parse_ids = HashSet::new();
    let mut simple_gate_ids = HashSet::new();
    let mut execute_gate_ids = HashSet::new();
    let mut mysql_rejection_ids = HashSet::new();
    let mut metadata_origin_ids = HashSet::new();
    let mut explicit_parameter_ids = HashSet::new();
    let mut full_dml_semantic_success_ids = HashSet::new();
    let mut case_audit = Vec::new();

    let state_is_31 = |record: &serde_json::Value| {
        record["observation"]["status"] == 2
            && record["observation"]["rows"] == serde_json::json!([["31"]])
    };
    let recovery_is_healthy = |record: &serde_json::Value| {
        record["observation"]["status"] == 2
            && record["observation"]["state"] == ""
            && record["observation"]["rows"] == serde_json::json!([["1"]])
            && record["observation"]["transaction_after"] == "I"
    };

    for case in cases {
        let case_id = case["case_id"].as_str().expect("registered case ID");
        assert!(
            unique_ids.insert(case_id.to_string()),
            "duplicate registered case {case_id}"
        );
        let valid = case["kind"] == "valid";
        let original_sql = case["input"]["sql"]
            .as_str()
            .expect("registered source SQL");
        let reference_sql = case["reference_sql"]
            .as_str()
            .expect("mapped PG source SQL");
        let reference_relation = case["object_mapping"]["reference"]
            .as_str()
            .expect("mapped PostgreSQL relation");
        let mysql_sql = original_sql.replace("public.t", "t").replace("$1", "31");
        assert_eq!(case["input"]["phase"], "PQprepare");
        assert_eq!(case["encoding"], "UTF8");
        assert_eq!(case["timezone"], "UTC");
        assert_eq!(case["search_path"], "public");
        assert_eq!(case["initial_state"], "I");
        assert_eq!(
            case["object_mapping"]["aster"]
                .as_str()
                .unwrap()
                .ends_with(".t"),
            true
        );
        assert_eq!(
            case["object_mapping"]["reference"]
                .as_str()
                .unwrap()
                .ends_with(".t"),
            true
        );
        assert_eq!(
            reference_sql,
            original_sql.replace("public.t", reference_relation)
        );
        assert_eq!(case["source_relation_oid"]["aster"], aster_relation_oid);
        assert_eq!(
            case["source_relation_oid"]["reference"],
            reference_relation_oid
        );
        assert_eq!(case["source_attributes"]["aster"]["id"], 1);
        assert_eq!(case["source_attributes"]["reference"]["id"], 1);
        if valid {
            assert_eq!(
                case["input"]["parameter_oids"],
                case["execution_input"]["parameter_oids"]
            );
            assert_eq!(case["execution_input"]["phase"], "PQexecPrepared");
            assert_eq!(
                case["execution_input"]["parameter_formats"]
                    .as_array()
                    .unwrap()
                    .len(),
                case["execution_input"]["parameter_oids"]
                    .as_array()
                    .unwrap()
                    .len()
            );
        } else {
            assert!(case["execution_input"].is_null());
        }
        let mysql_case = mysql_by_case[case_id];
        let mysql_reference: Vec<&serde_json::Value> = mysql
            .iter()
            .filter(|record| record["case_id"] == case_id && record["side"] == "mysql")
            .collect();
        assert_eq!(
            mysql_reference.len(),
            1,
            "one external MySQL outcome for {case_id}"
        );
        let mysql_outcome = &mysql_reference[0]["observation"];
        assert_eq!(
            mysql_outcome["error_code"], 1064,
            "MySQL 8.4.7 rejection for {case_id}"
        );
        assert_eq!(
            mysql_outcome["sqlstate"], "42000",
            "MySQL SQLSTATE for {case_id}"
        );
        assert_eq!(mysql_outcome["sql_mode"], "");
        assert_eq!(mysql_outcome["character_set"], "utf8mb4");
        assert_eq!(mysql_outcome["collation"], "utf8mb4_general_ci");
        assert_eq!(mysql_outcome["column_metadata"][0], "id");
        assert_eq!(mysql_outcome["column_metadata"][1], "int");
        assert_eq!(mysql_outcome["column_metadata"][2], "NO");
        assert_eq!(mysql_outcome["column_metadata"][3], "PRI");
        assert_eq!(mysql_outcome["row_before"], "31");
        assert_eq!(mysql_outcome["row_after"], "31");
        assert_eq!(mysql_outcome["recovery"], "1");
        assert_eq!(mysql_outcome["legal_prepare_error_count"], 0);
        assert_eq!(mysql_outcome["subprocess_exited"], true);
        assert_eq!(mysql_outcome["temporary_connection_closed"], true);
        assert!(
            mysql_outcome["legal_prepare_sql"]
                .as_str()
                .unwrap()
                .contains("ON DUPLICATE KEY UPDATE")
        );
        assert!(mysql_outcome["error"].as_str().unwrap().contains("near '"));
        assert_eq!(mysql_case["mysql_error_code"], 1064);
        assert_eq!(mysql_case["mysql_sqlstate"], "42000");
        assert_eq!(mysql_case["postgresql_sql"], original_sql);
        assert_eq!(mysql_case["mysql_sql"], mysql_sql);
        assert_eq!(mysql_case["before"], serde_json::json!([[[51, 49]]]));
        assert_eq!(
            mysql_case["before"], mysql_case["after"],
            "local MySQL state for {case_id}"
        );
        assert_eq!(
            mysql_case["recovery"],
            serde_json::json!([[[49]]]),
            "local MySQL recovery for {case_id}"
        );
        assert_eq!(mysql_case["mysql_on_duplicate_prepared"], true);
        assert_eq!(mysql_case["mysql_on_duplicate_prepare_packet"][0], 0);
        assert_eq!(mysql_case["mysql_on_duplicate_prepare_columns"], 0);
        assert_eq!(mysql_case["mysql_on_duplicate_prepare_parameters"], 0);
        assert_eq!(mysql_case["mysql_on_duplicate_statement_closed"], true);
        let packet = mysql_case["packet"].as_array().expect("MySQL error packet");
        assert_eq!(packet.first().and_then(|byte| byte.as_u64()), Some(255));
        assert_eq!(packet.get(1).and_then(|byte| byte.as_u64()), Some(40));
        assert_eq!(packet.get(2).and_then(|byte| byte.as_u64()), Some(4));
        mysql_rejection_ids.insert(case_id.to_string());

        let reference_parse: Vec<&serde_json::Value> =
            pg_dialect_records(&reference, case_id, "reference")
                .into_iter()
                .filter(|record| record["input"]["phase"] == "PQprepare")
                .collect();
        let local_parse: Vec<&serde_json::Value> = pg_dialect_records(&aster, case_id, "aster")
            .into_iter()
            .filter(|record| record["input"]["phase"] == "PQprepare")
            .collect();
        assert_eq!(reference_parse.len(), 1, "one PG18 Parse/Prepare per case");
        assert_eq!(local_parse.len(), 2, "two Aster endpoints per case");
        assert_eq!(reference_parse[0]["input"]["sql"], reference_sql);
        assert_eq!(
            reference_parse[0]["input"]["parameter_oids"],
            case["input"]["parameter_oids"]
        );
        assert!(local_parse.iter().all(|record| {
            record["input"]["sql"] == original_sql
                && record["input"]["parameter_oids"] == case["input"]["parameter_oids"]
        }));
        let endpoints: HashSet<&str> = local_parse
            .iter()
            .map(|record| {
                record["observation"]["endpoint"]
                    .as_str()
                    .expect("Aster endpoint")
            })
            .collect();
        assert_eq!(endpoints, HashSet::from(["aster", "aster_b"]));

        if valid {
            for record in reference_parse.iter().chain(local_parse.iter()) {
                assert_eq!(
                    record["observation"]["status"], 1,
                    "successful Prepare for {case_id}"
                );
                assert_eq!(
                    record["observation"]["state"], "",
                    "successful Prepare SQLSTATE for {case_id}"
                );
                assert_eq!(record["observation"]["transaction_after"], "I");
            }
            let reference_describe_id = format!("{case_id}_describe");
            let reference_describe =
                pg_dialect_records(&reference, &reference_describe_id, "reference");
            let local_describe = pg_dialect_records(&aster, &reference_describe_id, "aster");
            assert_eq!(
                reference_describe.len(),
                1,
                "one PG18 Describe for {case_id}"
            );
            assert_eq!(
                local_describe.len(),
                2,
                "Describe on both Aster endpoints for {case_id}"
            );
            let expected_metadata = &case["describe_expected"];
            let has_base_column = case_id == "omitted_target";
            let descriptions = reference_describe
                .iter()
                .map(|description| (*description, reference_relation_oid))
                .chain(
                    local_describe
                        .iter()
                        .map(|description| (*description, aster_relation_oid)),
                );
            for (description, expected_relation_oid) in descriptions {
                let observed = &description["observation"];
                assert_eq!(observed["status"], 1, "Describe status for {case_id}");
                assert_eq!(observed["state"], "", "Describe SQLSTATE for {case_id}");
                assert_eq!(
                    observed["fields"], expected_metadata["fields"],
                    "Describe columns for {case_id}"
                );
                assert_eq!(
                    observed["formats"], expected_metadata["formats"],
                    "Describe formats for {case_id}"
                );
                assert_eq!(
                    observed["param_count"], expected_metadata["parameter_count"],
                    "Describe parameter count for {case_id}"
                );
                assert_eq!(
                    observed["params"], expected_metadata["parameter_oids"],
                    "Describe parameter OIDs for {case_id}"
                );
                assert!(
                    pg_dialect_07_origins_match(observed, expected_relation_oid, has_base_column),
                    "Describe source-column origins for {case_id}"
                );
            }
            let reference_metadata = &reference_describe[0]["observation"];
            for description in &local_describe {
                let observed = &description["observation"];
                for key in ["fields", "formats", "param_count", "params"] {
                    assert_eq!(
                        observed[key], reference_metadata[key],
                        "Aster/PG Describe {key} for {case_id}"
                    );
                }
            }
            metadata_origin_ids.insert(case_id.to_string());
            if expected_metadata["parameter_count"] == 1 {
                explicit_parameter_ids.insert(case_id.to_string());
            }

            let simple_id = format!("{case_id}_simple_query_gate");
            let simple = pg_dialect_records(&aster, &simple_id, "aster");
            assert_eq!(
                simple.len(),
                2,
                "Simple Query gate on both endpoints for {case_id}"
            );
            let execute_id = format!("{case_id}_execute_gate");
            let execute = pg_dialect_records(&aster, &execute_id, "aster");
            assert_eq!(
                execute.len(),
                2,
                "Extended Execute gate on both endpoints for {case_id}"
            );
            let expected_simple_sql = original_sql.replace("$1", "31");
            let expected_arguments = &case["execution_input"]["arguments"];
            for record in &simple {
                assert_eq!(record["input"]["phase"], "PQexec");
                assert_eq!(record["input"]["sql"], expected_simple_sql);
            }
            let mut simple_endpoints = HashSet::new();
            for record in &simple {
                assert_eq!(record["observation"]["status"], 7);
                assert_eq!(record["observation"]["state"], "0A000");
                assert_eq!(record["observation"]["transaction_after"], "I");
                simple_endpoints.insert(record["observation"]["endpoint"].as_str().unwrap());
            }
            assert_eq!(simple_endpoints, HashSet::from(["aster", "aster_b"]));
            let mut execute_endpoints = HashSet::new();
            for record in &execute {
                assert_eq!(record["input"]["phase"], "PQexecPrepared");
                assert_eq!(
                    record["input"]["parameter_oids"],
                    case["execution_input"]["parameter_oids"]
                );
                assert_eq!(record["input"]["arguments"], *expected_arguments);
                assert_eq!(
                    record["input"]["parameter_formats"],
                    case["execution_input"]["parameter_formats"]
                );
                assert_eq!(
                    record["input"]["result_format"],
                    case["execution_input"]["result_format"]
                );
                assert_eq!(record["observation"]["status"], 7);
                assert_eq!(record["observation"]["state"], "0A000");
                assert_eq!(record["observation"]["transaction_after"], "I");
                execute_endpoints.insert(record["observation"]["endpoint"].as_str().unwrap());
            }
            assert_eq!(execute_endpoints, HashSet::from(["aster", "aster_b"]));
            simple_gate_ids.insert(case_id.to_string());
            execute_gate_ids.insert(case_id.to_string());

            let reference_close = pg_dialect_records(
                &reference,
                &format!("{case_id}_reference_close"),
                "reference",
            );
            let local_close =
                pg_dialect_records(&aster, &format!("{case_id}_aster_close"), "aster");
            assert_eq!(
                reference_close.len(),
                1,
                "reference prepared statement close for {case_id}"
            );
            assert_eq!(reference_close[0]["observation"]["status"], 1);
            assert_eq!(
                local_close.len(),
                2,
                "Aster prepared statement closes for {case_id}"
            );
            assert!(
                local_close
                    .iter()
                    .all(|record| record["observation"]["status"] == 1)
            );

            let state_phases = [
                (format!("{case_id}_aster_state_before"), "aster", 1),
                (format!("{case_id}_reference_state_before"), "reference", 1),
                (
                    format!("{case_id}_aster_state_after_prepare_describe"),
                    "aster",
                    1,
                ),
                (
                    format!("{case_id}_reference_state_after_prepare_describe"),
                    "reference",
                    1,
                ),
                (
                    format!("{case_id}_aster_state_after_simple_query_gate"),
                    "aster",
                    2,
                ),
                (
                    format!("{case_id}_aster_state_after_execute_gate"),
                    "aster",
                    2,
                ),
                (format!("{case_id}_aster_state_after"), "aster", 1),
                (format!("{case_id}_reference_state_after"), "reference", 1),
            ];
            for (state_id, side, expected_count) in state_phases {
                let records = if side == "aster" {
                    pg_dialect_records(&aster, &state_id, side)
                } else {
                    pg_dialect_records(&reference, &state_id, side)
                };
                assert_eq!(records.len(), expected_count, "state snapshots {state_id}");
                assert!(
                    records.iter().all(|record| state_is_31(record)),
                    "row state changed in {state_id}"
                );
            }
            valid_parse_describe_ids.insert(case_id.to_string());
        } else {
            assert_eq!(reference_parse[0]["observation"]["status"], 7);
            assert_eq!(reference_parse[0]["observation"]["state"], "42601");
            assert_eq!(reference_parse[0]["observation"]["transaction_after"], "I");
            let reference_position: i64 = reference_parse[0]["observation"]["position"]
                .as_str()
                .expect("PG18 parse position")
                .parse()
                .expect("numeric PG18 parse position");
            let mapping_delta =
                reference_sql.chars().count() as i64 - original_sql.chars().count() as i64;
            let expected_local_position = (reference_position - mapping_delta).to_string();
            for record in &local_parse {
                assert_eq!(record["observation"]["status"], 7);
                assert_eq!(record["observation"]["state"], "42601");
                assert_eq!(record["observation"]["position"], expected_local_position);
                assert_eq!(record["observation"]["transaction_after"], "I");
            }
            negative_parse_ids.insert(case_id.to_string());

            for (state_id, side) in [
                (format!("{case_id}_aster_state_before"), "aster"),
                (format!("{case_id}_reference_state_before"), "reference"),
                (format!("{case_id}_aster_state_after"), "aster"),
                (format!("{case_id}_reference_state_after"), "reference"),
            ] {
                let records = if side == "aster" {
                    pg_dialect_records(&aster, &state_id, side)
                } else {
                    pg_dialect_records(&reference, &state_id, side)
                };
                assert_eq!(records.len(), 1, "state snapshot {state_id}");
                assert!(state_is_31(records[0]), "row state changed in {state_id}");
            }
        }

        let recovery_suffixes: Vec<String> = if valid {
            vec![
                format!("{case_id}_recovery"),
                format!("{case_id}_simple_query_gate_recovery"),
                format!("{case_id}_execute_gate_recovery"),
            ]
        } else {
            vec![format!("{case_id}_recovery")]
        };
        let mut recovery_count = 0;
        for suffix in recovery_suffixes {
            let aster_recoveries = pg_dialect_records(&aster, &suffix, "aster");
            let reference_recoveries = pg_dialect_records(&reference, &suffix, "reference");
            let expected_count = if valid {
                2
            } else if suffix.ends_with("_recovery") {
                2
            } else {
                0
            };
            assert_eq!(
                aster_recoveries.len(),
                expected_count,
                "Aster recovery records for {suffix}"
            );
            if !valid && suffix == format!("{case_id}_recovery") {
                assert_eq!(
                    reference_recoveries.len(),
                    1,
                    "PG18 recovery after parse error for {case_id}"
                );
            } else {
                assert!(
                    reference_recoveries.is_empty(),
                    "unexpected PG18 recovery record for {suffix}"
                );
            }
            assert!(
                aster_recoveries
                    .iter()
                    .all(|record| recovery_is_healthy(record))
            );
            assert!(
                reference_recoveries
                    .iter()
                    .all(|record| recovery_is_healthy(record))
            );
            recovery_count += aster_recoveries.len() + reference_recoveries.len();
        }
        let order_recovery = pg_dialect_records(
            &aster,
            &format!("{case_id}_endpoint_order_recovery"),
            "aster",
        );
        assert_eq!(order_recovery.len(), 1);
        assert!(recovery_is_healthy(order_recovery[0]));
        recovery_count += order_recovery.len();

        let semantic_local_execute: Vec<&serde_json::Value> =
            pg_dialect_records(&aster, &format!("{case_id}_execute_gate"), "aster")
                .into_iter()
                .filter(|record| record["observation"]["status"] == 2)
                .collect();
        let semantic_reference_execute: Vec<&serde_json::Value> = reference
            .iter()
            .filter(|record| {
                record["case_id"] == case_id
                    && record["input"]["phase"] == "PQexecPrepared"
                    && record["observation"]["status"] == 2
            })
            .collect();
        if !semantic_local_execute.is_empty()
            && !semantic_reference_execute.is_empty()
            && semantic_local_execute.iter().all(|local| {
                semantic_reference_execute.iter().any(|reference| {
                    local["observation"]["fields"] == reference["observation"]["fields"]
                        && local["observation"]["rows"] == reference["observation"]["rows"]
                        && local["observation"]["command_tag"]
                            == reference["observation"]["command_tag"]
                })
            })
        {
            full_dml_semantic_success_ids.insert(case_id.to_string());
        }

        case_audit.push(serde_json::json!({
            "case_id": case_id,
            "kind": if valid { "valid" } else { "invalid" },
            "prepare_describe": if valid { "success" } else { "not_applicable_parse_reject" },
            "source_position": if valid { "not_applicable" } else { "PG18_position_mapped_to_both_Aster_endpoints" },
            "simple_query": if valid { "0A000_pre_write_gate_on_A_and_B" } else { "not_applicable" },
            "extended_execute": if valid { "0A000_pre_write_gate_on_A_and_B" } else { "not_applicable" },
            "metadata": if valid { "fields_formats_parameter_oids_and_source_origins_verified" } else { "not_applicable_parse_reject" },
            "side_effect_rows_preserved": true,
            "recoveries": recovery_count,
            "postgresql_reference_rejection": mysql_reference[0]["observation"]["error_code"] == 1064,
            "mysql_local_rejection_recovery_and_legacy_prepare": true,
            "full_dml_semantic_success": full_dml_semantic_success_ids.contains(case_id),
            "effective": true
        }));
    }

    assert_eq!(unique_ids.len(), cases.len());
    let valid_case_count = cases.iter().filter(|case| case["kind"] == "valid").count();
    assert_eq!(valid_parse_describe_ids.len(), valid_case_count);
    assert_eq!(metadata_origin_ids.len(), valid_case_count);
    assert_eq!(negative_parse_ids.len() + valid_case_count, cases.len());
    assert_eq!(simple_gate_ids.len(), valid_case_count);
    assert_eq!(execute_gate_ids.len(), valid_case_count);
    assert_eq!(mysql_rejection_ids.len(), cases.len());
    assert_eq!(explicit_parameter_ids.len(), 1);

    let effective_ids: Vec<String> = case_audit
        .iter()
        .filter(|case| case["effective"] == true)
        .map(|case| case["case_id"].as_str().unwrap().to_string())
        .collect();
    let semantic_ids: Vec<String> = full_dml_semantic_success_ids.into_iter().collect();
    assert_eq!(
        summary["syntax_eligible"].as_u64(),
        Some(effective_ids.len() as u64)
    );
    assert_eq!(
        summary["reference_syntax_cases"].as_u64(),
        Some(valid_case_count as u64)
    );
    assert_eq!(
        summary["reference_describe_cases"].as_u64(),
        Some(valid_case_count as u64)
    );
    assert_eq!(
        summary["local_prepare_describe_cases"].as_u64(),
        Some(valid_case_count as u64)
    );
    assert_eq!(
        summary["local_simple_query_gate_cases"].as_u64(),
        Some(valid_case_count as u64)
    );
    assert_eq!(
        summary["local_execute_gate_cases"].as_u64(),
        Some(valid_case_count as u64)
    );
    assert_eq!(
        summary["mysql_rejection_cases"].as_u64(),
        Some(cases.len() as u64)
    );

    summary["unique_effective"] = serde_json::json!(effective_ids.len());
    summary["effective_case_ids"] = serde_json::json!(effective_ids);
    summary["valid_prepare_describe_cases"] = serde_json::json!(valid_parse_describe_ids.len());
    summary["syntax_negative_parse_cases"] = serde_json::json!(negative_parse_ids.len());
    summary["metadata_origin_verified_cases"] = serde_json::json!(metadata_origin_ids.len());
    summary["explicit_parameter_metadata_cases"] = serde_json::json!(explicit_parameter_ids.len());
    summary["mysql_local_bypass_recovery_cases"] = serde_json::json!(mysql_by_case.len());
    summary["case_audit"] = serde_json::Value::Array(case_audit);
    summary["full_dml_semantic_successes"] = serde_json::json!(semantic_ids.len());
    summary["full_dml_semantic_success_case_ids"] = serde_json::json!(semantic_ids);
    summary["dml_semantic_scope"] = serde_json::json!(
        "not executed: the three registered valid cases have only archived Aster PQexecPrepared 0A000 gates; no paired successful PostgreSQL/Aster DML result exists"
    );
    summary["cleanup_ok"] = serde_json::json!(
        ref_cleanup["cleanup_ok"] == true
            && rust_cleanup["database_dropped"] == true
            && rust_cleanup["postgres_port_closed"] == true
            && rust_cleanup["mysql_port_closed"] == true
    );
    assert_eq!(summary["unique_effective"], cases.len() as u64);
    assert_eq!(
        summary["valid_prepare_describe_cases"],
        valid_case_count as u64
    );
    assert_eq!(
        summary["syntax_negative_parse_cases"],
        negative_parse_ids.len() as u64
    );
    assert_eq!(summary["full_dml_semantic_successes"], 0);
    assert_eq!(summary["cleanup_ok"], true);
    std::fs::write(
        &summary_path,
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    summary
}

fn pg_dialect_libpq(task: u8) {
    use std::io::BufRead;
    use std::process::Stdio;
    assert!(std::env::var_os("PG18_REFERENCE_DSN").is_some());
    assert!(std::env::var_os("PG_CORE_REFERENCE_BUILD_FILE").is_some());
    assert!(std::env::var_os("MYSQL_REFERENCE_DSN").is_some());
    if matches!(task, 6 | 7 | 8 | 10) {
        assert!(std::env::var_os("MYSQL_REFERENCE_BUILD_FILE").is_some());
    }
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr().unwrap();
    let mysql_address = server.listener_addr().unwrap();
    let database = format!(
        "pg_dialect_{task:02}_{}_{}",
        std::process::id(),
        address.port()
    );
    let run = format!("{}-{}", std::process::id(), address.port());
    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/pg-dialect-{task:02}-evidence"))
        .join(run);
    std::fs::create_dir_all(&evidence).unwrap();
    println!("PG_DIALECT_EVIDENCE_DIR={}", evidence.display());
    let executable = std::env::current_exe().unwrap();
    let build = serde_json::json!({"path": executable, "profile": "cargo-test-default-features"});
    std::fs::write(evidence.join("rust-build.json"), build.to_string()).unwrap();
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup_guard = Cleanup(|| {
        server.close();
        let dropped = native.execute(&format!("DROP DATABASE {database}")).is_ok();
        let other_dropped = task != 14
            || native
                .execute(&format!("DROP DATABASE {database}_other"))
                .is_ok();
        let cleanup = serde_json::json!({"database_dropped": dropped, "other_database_dropped": other_dropped,
            "postgres_port_closed": TcpStream::connect(address).is_err(),
            "mysql_port_closed": TcpStream::connect(mysql_address).is_err()});
        let _ = std::fs::write(evidence.join("rust-cleanup.json"), cleanup.to_string());
    });
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    if task == 6 {
        native
            .execute(&format!(
                "CREATE TABLE {database}.dialect_06_baseline_t (id INT)"
            ))
            .unwrap();
        native
            .execute(&format!(
                "CREATE TABLE {database}.dialect_06_baseline_s (id INT)"
            ))
            .unwrap();
        native
            .execute(&format!(
                "INSERT INTO {database}.dialect_06_baseline_t VALUES (31)"
            ))
            .unwrap();
        native
            .execute(&format!(
                "INSERT INTO {database}.dialect_06_baseline_s VALUES (31)"
            ))
            .unwrap();
    }
    if task == 7 {
        native
            .execute(&format!(
                "CREATE TABLE {database}.t (id INT NOT NULL PRIMARY KEY)"
            ))
            .unwrap();
        native
            .execute(&format!("INSERT INTO {database}.t VALUES (31)"))
            .unwrap();
    }
    if task == 8 {
        native
            .execute(&format!(
                "CREATE TABLE {database}.t (id INT NOT NULL PRIMARY KEY, v VARCHAR(64))"
            ))
            .unwrap();
        native
            .execute(&format!("INSERT INTO {database}.t VALUES (31, 'before')"))
            .unwrap();
    }
    if task == 14 {
        native
            .execute(&format!("CREATE DATABASE {database}_other"))
            .unwrap();
    }
    let mut mysql = TcpStream::connect(mysql_address).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut mysql);
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let _ = self.0.wait();
        }
    }
    if task == 19 {
        mysql_query(&mut mysql, &format!("USE {database}"));
        mysql_query(&mut mysql, "CREATE TABLE t(v INT)");
        mysql_query(&mut mysql, "INSERT INTO t VALUES(9)");
    }
    let mut child = ChildGuard(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/pg_dialect_libpq.py"
            ))
            .args([
                task.to_string(),
                address.port().to_string(),
                database.clone(),
            ])
            .env("PG_DIALECT_EVIDENCE_DIR", &evidence)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    fn column_type(column: &[u8]) -> u8 {
        let mut offset = 0;
        for _ in 0..6 {
            let length = usize::from(column[offset]);
            assert!(length < 251);
            offset += length + 1;
        }
        assert_eq!(column[offset], 12);
        column[offset + 7]
    }
    let checkpoint = |socket: &mut TcpStream, phase: &str| {
        if task == 12 {
            use std::io::Write;
            mysql_query(socket, "SET time_zone='+00:00'");
            assert_eq!(
                mysql_query(socket, "SELECT @@time_zone"),
                vec![vec![Some(b"+00:00".to_vec())]]
            );
            let mut record = serde_json::json!({"phase":phase});
            if let Some(case) = phase.strip_prefix("PG_DIALECT_CHECKPOINT ") {
                let registry: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(evidence.join("cases.json")).unwrap(),
                )
                .unwrap();
                let original = registry
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["case_id"] == case)
                    .unwrap()["sql"]
                    .as_str()
                    .unwrap();
                let sql = original
                    .replace("public.t", &format!("{database}.t"))
                    .replace("$1", "7")
                    .replace("$2", "9");
                record["case_id"] = serde_json::json!(case);
                record["sql"] = serde_json::json!(sql);
                if matches!(case, "grouped" | "subquery" | "protected") {
                    let values = mysql_query(socket, &sql);
                    let expected = if case == "protected" {
                        b"(1,2)".to_vec()
                    } else {
                        b"1".to_vec()
                    };
                    assert_eq!(values, vec![vec![Some(expected)]]);
                    record["rows"] = serde_json::json!(values);
                } else {
                    let mut request = vec![3];
                    request.extend_from_slice(sql.as_bytes());
                    mysql_packet(socket, 0, &request);
                    let error = mysql_read(socket);
                    assert_eq!(error[0], 0xff);
                    let code = u16::from_le_bytes(error[1..3].try_into().unwrap());
                    let expected = if matches!(case, "missing_close" | "relation_group") {
                        1064
                    } else {
                        1105
                    };
                    assert_eq!(code, expected, "MySQL existing row/scalar boundary {case}");
                    assert_eq!(
                        &error[4..9],
                        if expected == 1064 { b"42000" } else { b"HY000" }
                    );
                    record["error_code"] = serde_json::json!(code);
                    record["state"] = serde_json::json!(String::from_utf8_lossy(&error[4..9]));
                    record["packet"] = serde_json::json!(error);
                }
                record["recovery"] = serde_json::json!(mysql_query(socket, "SELECT 1"));
            }
            let mut output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-grouping-baseline.jsonl"))
                .unwrap();
            writeln!(output, "{record}").unwrap();
        }
        if task == 10 {
            use std::io::Write;

            mysql_query(socket, "SET SESSION sql_mode=''");
            mysql_query(socket, "SET NAMES utf8mb4 COLLATE utf8mb4_general_ci");
            mysql_query(socket, "SET time_zone='+00:00'");
            let sql = if let Some(case_id) = phase.strip_prefix("PG_DIALECT_CHECKPOINT ") {
                let cases: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(evidence.join("cases.json")).unwrap(),
                )
                .unwrap();
                Some(
                    cases
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|case| case["case_id"] == case_id)
                        .unwrap_or_else(|| panic!("unregistered MySQL checkpoint: {case_id}"))["sql"]
                        .as_str()
                        .expect("original PostgreSQL SQL in task case registry")
                        .to_owned(),
                )
            } else {
                None
            };
            let before = mysql_query(socket, "SELECT 1");
            assert_eq!(before, vec![vec![Some(b"1".to_vec())]]);
            let isolation_sql = format!("SELECT id FROM {database}.dialect_isolation");
            let isolation_before = mysql_query(socket, &isolation_sql);
            assert_eq!(isolation_before, vec![vec![Some(b"31".to_vec())]]);
            if phase == "before" {
                let mut prepare = vec![0x16];
                prepare.extend_from_slice(b"SELECT JSON_ARRAY(1,2)");
                mysql_packet(socket, 0, &prepare);
                let prepared = mysql_read(socket);
                assert_eq!(prepared[0], 0, "MySQL JSON_ARRAY prepare control");
                let columns = u16::from_le_bytes(prepared[5..7].try_into().unwrap());
                let parameters = u16::from_le_bytes(prepared[7..9].try_into().unwrap());
                assert_eq!((columns, parameters), (1, 0));
                let column = mysql_read(socket);
                let local_type = column_type(&column);
                assert_eq!(
                    local_type, 253,
                    "freeze local MySQL JSON_ARRAY metadata baseline"
                );
                assert_eq!(mysql_read(socket)[0], 0xfe);
                let mut close = vec![0x19];
                close.extend_from_slice(&prepared[1..5]);
                mysql_packet(socket, 0, &close);
                let mut prepare_isolation = vec![0x16];
                prepare_isolation.extend_from_slice(isolation_sql.as_bytes());
                mysql_packet(socket, 0, &prepare_isolation);
                let prepared_isolation = mysql_read(socket);
                assert_eq!(prepared_isolation[0], 0);
                assert_eq!(
                    (
                        u16::from_le_bytes(prepared_isolation[5..7].try_into().unwrap()),
                        u16::from_le_bytes(prepared_isolation[7..9].try_into().unwrap())
                    ),
                    (1, 0)
                );
                let isolation_column = mysql_read(socket);
                let isolation_type = column_type(&isolation_column);
                assert_eq!(isolation_type, 3, "freeze local INT column type");
                assert_eq!(mysql_read(socket)[0], 0xfe);
                let mut close_isolation = vec![0x19];
                close_isolation.extend_from_slice(&prepared_isolation[1..5]);
                mysql_packet(socket, 0, &close_isolation);
                assert_eq!(mysql_query(socket, "SELECT 1"), before);
                std::fs::write(
                    evidence.join("mysql-local-prepare.json"),
                    serde_json::json!({
                        "sql": "SELECT JSON_ARRAY(1,2)",
                        "prepared": true,
                        "columns": columns,
                        "parameters": parameters,
                        "mysql_type": local_type,
                        "isolation_column_type": isolation_type,
                        "statement_closed": true
                    })
                    .to_string(),
                )
                .unwrap();
            }
            if let Some(case_id) = phase.strip_prefix("PG_DIALECT_CHECKPOINT ") {
                let mut request = vec![3];
                request.extend_from_slice(sql.as_deref().unwrap().as_bytes());
                mysql_packet(socket, 0, &request);
                let response = mysql_read(socket);
                assert_eq!(response[0], 0xff, "MySQL accepted PostgreSQL ARRAY input");
                let error_code = u16::from_le_bytes(response[1..3].try_into().unwrap());
                let sqlstate = String::from_utf8_lossy(&response[4..9]).to_string();
                assert_eq!(error_code, 1064);
                assert_eq!(sqlstate, "42000");
                let control = mysql_query(socket, "SELECT JSON_ARRAY(1,2)");
                assert_eq!(control, vec![vec![Some(b"[1, 2]".to_vec())]]);
                let isolation_after = mysql_query(socket, &isolation_sql);
                assert_eq!(
                    isolation_after, isolation_before,
                    "rejected input changed the MySQL fixture"
                );
                let recovery = mysql_query(socket, "SELECT 1");
                assert_eq!(
                    recovery, before,
                    "MySQL did not recover on the same connection"
                );
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(evidence.join("mysql-local.jsonl"))
                    .unwrap();
                writeln!(
                    file,
                    "{}",
                    serde_json::json!({
                        "case_id": case_id,
                        "phase": phase,
                        "postgresql_sql": sql,
                        "mysql_error_code": error_code,
                        "mysql_sqlstate": sqlstate,
                        "before": before,
                        "isolation_before": isolation_before,
                        "isolation_after": isolation_after,
                        "json_array_control": control,
                        "recovery": recovery,
                        "sql_mode": "",
                        "character_set": "utf8mb4",
                        "collation": "utf8mb4_general_ci",
                        "time_zone": "+00:00",
                        "packet": response
                    })
                )
                .unwrap();
            } else {
                assert_eq!(mysql_query(socket, "SELECT 1"), before);
                assert_eq!(mysql_query(socket, &isolation_sql), isolation_before);
            }
        }
        if task == 7 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            use std::io::Write;

            let case_id = phase
                .strip_prefix("PG_DIALECT_CHECKPOINT ")
                .expect("checkpoint prefix");
            let cases: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(evidence.join("cases.json")).unwrap(),
            )
            .unwrap();
            let sql =
                cases
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|case| case["case_id"] == case_id)
                    .unwrap_or_else(|| panic!("unregistered MySQL checkpoint: {case_id}"))["input"]
                    ["sql"]
                    .as_str()
                    .expect("original SQL in task case registry")
                    .to_owned();
            let mysql_sql = sql.replace("public.t", "t").replace("$1", "31");
            mysql_query(socket, &format!("USE {database}"));
            let before = mysql_query(socket, "SELECT id FROM t ORDER BY id");
            assert_eq!(before, vec![vec![Some(b"31".to_vec())]]);

            let legal = "INSERT INTO t(id) VALUES (31) ON DUPLICATE KEY UPDATE id=VALUES(id)";
            let mut prepare = vec![0x16];
            prepare.extend_from_slice(legal.as_bytes());
            mysql_packet(socket, 0, &prepare);
            let prepared = mysql_read(socket);
            assert_eq!(prepared[0], 0, "MySQL ON DUPLICATE prepare baseline");
            assert!(prepared.len() >= 9);
            assert_eq!(u16::from_le_bytes(prepared[5..7].try_into().unwrap()), 0);
            assert_eq!(u16::from_le_bytes(prepared[7..9].try_into().unwrap()), 0);
            let mut close = vec![0x19];
            close.extend_from_slice(&prepared[1..5]);
            mysql_packet(socket, 0, &close);

            let mut request = vec![3];
            request.extend_from_slice(mysql_sql.as_bytes());
            mysql_packet(socket, 0, &request);
            let response = mysql_read(socket);
            assert_eq!(response[0], 0xff, "MySQL accepted PostgreSQL ON CONFLICT");
            let error_code = u16::from_le_bytes(response[1..3].try_into().unwrap());
            let sqlstate = String::from_utf8_lossy(&response[4..9]).to_string();
            assert_eq!(error_code, 1064);
            assert_eq!(sqlstate, "42000");
            let after = mysql_query(socket, "SELECT id FROM t ORDER BY id");
            assert_eq!(after, before, "rejected MySQL input changed the row");
            let recovery = mysql_query(socket, "SELECT 1");
            assert_eq!(
                recovery,
                vec![vec![Some(b"1".to_vec())]],
                "MySQL connection did not recover after the syntax error"
            );
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-local.jsonl"))
                .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "case_id": case_id,
                    "phase": phase,
                    "postgresql_sql": sql,
                    "mysql_sql": mysql_sql,
                    "mysql_error_code": error_code,
                    "mysql_sqlstate": sqlstate,
                    "before": before,
                    "after": after,
                    "recovery": recovery,
                    "mysql_on_duplicate_prepared": true,
                    "mysql_on_duplicate_prepare_packet": prepared,
                    "mysql_on_duplicate_prepare_statement_id": prepared[1..5].to_vec(),
                    "mysql_on_duplicate_prepare_columns": u16::from_le_bytes(prepared[5..7].try_into().unwrap()),
                    "mysql_on_duplicate_prepare_parameters": u16::from_le_bytes(prepared[7..9].try_into().unwrap()),
                    "mysql_on_duplicate_statement_closed": true,
                    "packet": response
                })
            )
            .unwrap();
        }
        if task == 8 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            use std::io::Write;

            let case_id = phase
                .strip_prefix("PG_DIALECT_CHECKPOINT ")
                .expect("checkpoint prefix");
            let cases: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(evidence.join("cases.json")).unwrap(),
            )
            .unwrap();
            let sql = cases
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["case_id"] == case_id)
                .unwrap_or_else(|| panic!("unregistered MySQL checkpoint: {case_id}"))["sql"]
                .as_str()
                .expect("original SQL in task case registry")
                .to_owned();
            mysql_query(socket, &format!("USE {database}"));
            let before = mysql_query(socket, "SELECT id, v FROM t ORDER BY id");
            assert_eq!(
                before,
                vec![vec![Some(b"31".to_vec()), Some(b"before".to_vec())]]
            );

            let legal =
                "INSERT INTO t(id,v) VALUES (31,'before') ON DUPLICATE KEY UPDATE v=VALUES(v)";
            let mut prepare = vec![0x16];
            prepare.extend_from_slice(legal.as_bytes());
            mysql_packet(socket, 0, &prepare);
            let prepared = mysql_read(socket);
            assert_eq!(prepared[0], 0, "MySQL ON DUPLICATE prepare baseline");
            assert!(prepared.len() >= 9);
            assert_eq!(u16::from_le_bytes(prepared[5..7].try_into().unwrap()), 0);
            assert_eq!(u16::from_le_bytes(prepared[7..9].try_into().unwrap()), 0);
            let mut close = vec![0x19];
            close.extend_from_slice(&prepared[1..5]);
            mysql_packet(socket, 0, &close);

            let mysql_sql = sql
                .replace("public.t", "t")
                .replace("$1", "31")
                .replace("$2", "'after'");
            let mut request = vec![3];
            request.extend_from_slice(mysql_sql.as_bytes());
            mysql_packet(socket, 0, &request);
            let response = mysql_read(socket);
            assert_eq!(response[0], 0xff, "MySQL accepted PostgreSQL ON CONFLICT");
            let error_code = u16::from_le_bytes(response[1..3].try_into().unwrap());
            let sqlstate = String::from_utf8_lossy(&response[4..9]).to_string();
            assert_eq!(error_code, 1064);
            assert_eq!(sqlstate, "42000");
            let after = mysql_query(socket, "SELECT id, v FROM t ORDER BY id");
            assert_eq!(after, before, "rejected MySQL input changed the row");
            let recovery = mysql_query(socket, "SELECT 1");
            assert_eq!(recovery, vec![vec![Some(b"1".to_vec())]]);
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-local.jsonl"))
                .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "case_id": case_id,
                    "postgresql_sql": sql,
                    "mysql_sql": mysql_sql,
                    "error_code": error_code,
                    "sqlstate": sqlstate,
                    "before": before,
                    "after": after,
                    "recovery": recovery,
                    "mysql_on_duplicate_prepared": true,
                    "mysql_on_duplicate_prepare_statement_id": prepared[1..5].to_vec(),
                    "mysql_on_duplicate_prepare_columns": u16::from_le_bytes(prepared[5..7].try_into().unwrap()),
                    "mysql_on_duplicate_prepare_parameters": u16::from_le_bytes(prepared[7..9].try_into().unwrap()),
                    "mysql_on_duplicate_statement_closed": true,
                    "packet": response
                })
            )
            .unwrap();
        }
        if task == 5 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            mysql_query(socket, &format!("USE {database}"));
            assert_eq!(
                mysql_query(socket, "SELECT id, v FROM t ORDER BY id"),
                vec![
                    vec![Some(b"1".to_vec()), Some(b"one".to_vec())],
                    vec![Some(b"2".to_vec()), Some(b"two".to_vec())]
                ],
                "UPDATE RETURNING Prepare/Describe must not modify rows"
            );
            let sql = "UPDATE t AS a SET id=3 FROM s AS b WHERE a.id=b.id RETURNING b.id";
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(socket, 0, &request);
            let response = mysql_read(socket);
            assert_eq!(
                response[0], 0xff,
                "MySQL must reject PostgreSQL UPDATE FROM"
            );
            assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1064);
            assert_eq!(&response[4..9], b"42000");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-update-from.jsonl"))
                .unwrap();
            use std::io::Write;
            writeln!(file, "{}", serde_json::json!({"phase":phase,"sql":sql,"error_code":1064,"sqlstate":"42000","packet":response})).unwrap();
        }
        if task == 6 {
            mysql_query(socket, &format!("USE {database}"));
            assert_eq!(
                mysql_query(socket, "SELECT id FROM dialect_06_baseline_t ORDER BY id"),
                vec![vec![Some(b"31".to_vec())]],
                "the same MySQL connection lost its own table state"
            );
            mysql_query(socket, "DELETE FROM dialect_06_baseline_t WHERE id=999");
            assert_eq!(
                mysql_query(socket, "SELECT id FROM dialect_06_baseline_t ORDER BY id"),
                vec![vec![Some(b"31".to_vec())]],
                "MySQL control DML changed its fixture"
            );
            if phase == "before" {
                let sql = "DELETE FROM dialect_06_baseline_t WHERE id=31 RETURNING id";
                let mut request = vec![0x16]; // COM_STMT_PREPARE; do not execute the DML.
                request.extend_from_slice(sql.as_bytes());
                mysql_packet(socket, 0, &request);
                let response = mysql_read(socket);
                let accepted = response.first() == Some(&0);
                if accepted {
                    assert!(response.len() >= 9);
                    let columns = u16::from_le_bytes(response[5..7].try_into().unwrap()) as usize;
                    let parameters =
                        u16::from_le_bytes(response[7..9].try_into().unwrap()) as usize;
                    for count in [parameters, columns] {
                        for _ in 0..count {
                            mysql_read(socket);
                        }
                        if count > 0 {
                            assert_eq!(mysql_read(socket).first(), Some(&0xfe));
                        }
                    }
                    let mut close = vec![0x19];
                    close.extend_from_slice(&response[1..5]);
                    mysql_packet(socket, 0, &close);
                }
                let baseline = serde_json::json!({
                    "phase": "before PG Parse",
                    "sql": sql,
                    "accepted": accepted,
                    "error_code": if response.first() == Some(&0xff) && response.len() >= 3 { Some(u16::from_le_bytes([response[1], response[2]])) } else { None },
                    "sqlstate": if response.first() == Some(&0xff) && response.len() >= 9 { Some(String::from_utf8_lossy(&response[4..9]).to_string()) } else { None },
                    "packet": response,
                    "executed": false
                });
                std::fs::write(
                    evidence.join("mysql-local-delete-baseline.json"),
                    baseline.to_string(),
                )
                .unwrap();
            }
        }
        if task == 19 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            mysql_query(socket, &format!("USE {database}"));
            for (sql, expected) in [
                (
                    "WITH t AS (SELECT 1 AS v) SELECT t.v,q.v FROM t CROSS JOIN (WITH t AS (SELECT 2 AS v) SELECT v FROM t) q",
                    vec![Some(b"1".to_vec()), Some(b"2".to_vec())],
                ),
                (
                    "SELECT t.v,q.v FROM (WITH t AS (SELECT 2 AS v) SELECT v FROM t) q CROSS JOIN t",
                    vec![Some(b"9".to_vec()), Some(b"2".to_vec())],
                ),
                ("SELECT v FROM t", vec![Some(b"9".to_vec())]),
            ] {
                assert_eq!(mysql_query(socket, sql), vec![expected]);
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-ctes.jsonl"))
                .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"phase":phase,"nested":[1,2],"released":[9,2],"statement":[9]})
            )
            .unwrap();
        }
        if task == 18 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            let table = format!("t18_{}_{}", std::process::id(), address.port());
            mysql_query(socket, &format!("USE {database}"));
            for (sql, value) in [
                (
                    format!("WITH {table} AS (SELECT 1 AS v) SELECT v FROM {table}"),
                    b"1".to_vec(),
                ),
                (
                    format!("WITH {table} AS (SELECT 1 AS v) SELECT v FROM {database}.{table}"),
                    b"9".to_vec(),
                ),
                (
                    "WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b".to_string(),
                    b"1".to_vec(),
                ),
                (
                    format!(
                        "WITH a AS (SELECT v FROM {table}), {table} AS (SELECT 2 AS v) SELECT v FROM a"
                    ),
                    b"9".to_vec(),
                ),
                (
                    format!("WITH {table} AS (SELECT v FROM {table}) SELECT v FROM {table}"),
                    b"9".to_vec(),
                ),
            ] {
                assert_eq!(mysql_query(socket, &sql), vec![vec![Some(value)]]);
            }
            let sql =
                "WITH a AS (SELECT v FROM absent18), absent18 AS (SELECT 2 AS v) SELECT v FROM a";
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(socket, 0, &request);
            let response = mysql_read(socket);
            assert_eq!(response[0], 0xff);
            assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1105);
            assert_eq!(&response[4..9], b"HY000");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-ctes.jsonl"))
                .unwrap();
            writeln!(file, "{}", serde_json::json!({"phase":phase,"successful_cte_values":[1,9,1,9,9],"forward_missing_code":1105,"packet":response})).unwrap();
        }
        if task == 17 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            use std::io::Write;
            mysql_query(socket, &format!("USE {database}"));
            let before = mysql_query(socket, "SELECT id,v FROM excluded");
            mysql_query(
                socket,
                "INSERT INTO excluded(id,v) VALUES(1,'before') ON DUPLICATE KEY UPDATE v=VALUES(v)",
            );
            assert_eq!(mysql_query(socket, "SELECT id,v FROM excluded"), before);
            let legal =
                "INSERT INTO excluded(id,v) VALUES(1,'before') ON DUPLICATE KEY UPDATE v=VALUES(v)";
            let mut prepare = vec![0x16];
            prepare.extend_from_slice(legal.as_bytes());
            mysql_packet(socket, 0, &prepare);
            let prepared = mysql_read(socket);
            assert_eq!(prepared[0], 0);
            assert_eq!(u16::from_le_bytes(prepared[7..9].try_into().unwrap()), 0);
            let mut close = vec![0x19];
            close.extend_from_slice(&prepared[1..5]);
            mysql_packet(socket, 0, &close);
            let sql = "INSERT INTO excluded(id,v) VALUES(1,'after') ON CONFLICT(id) DO UPDATE SET v=excluded.v";
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(socket, 0, &request);
            let response = mysql_read(socket);
            assert_eq!(response[0], 0xff);
            assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1064);
            assert_eq!(&response[4..9], b"42000");
            assert_eq!(mysql_query(socket, "SELECT id,v FROM excluded"), before);
            let mut out = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-excluded.jsonl"))
                .unwrap();
            writeln!(out,"{}",serde_json::json!({"phase":phase,"sql":sql,"error_code":1064,"sqlstate":"42000","rows":before,"prepared":true,"statement_closed":true})).unwrap();
        }
        if task == 16 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            let table = format!("{database}.t16_{}_{}", std::process::id(), address.port());
            assert_eq!(
                mysql_query(socket, &format!("SELECT a.id FROM {table} a")),
                vec![vec![Some(b"1".to_vec())]]
            );
            assert_eq!(
                mysql_query(
                    socket,
                    &format!("SELECT a.id,b.id FROM {table} a JOIN {table} b ON a.id=b.id")
                ),
                vec![vec![Some(b"1".to_vec()), Some(b"1".to_vec())]]
            );
            for sql in [
                format!(
                    "SELECT t16_{}_{}.id FROM {table} a",
                    std::process::id(),
                    address.port()
                ),
                format!("SELECT a.missing FROM {table} a"),
            ] {
                let mut request = vec![3];
                request.extend_from_slice(sql.as_bytes());
                mysql_packet(socket, 0, &request);
                let response = mysql_read(socket);
                assert_eq!(response[0], 0xff);
                assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1054);
                assert_eq!(&response[4..9], b"42S22");
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(evidence.join("mysql-aliases.jsonl"))
                    .unwrap();
                writeln!(file,"{}",serde_json::json!({"phase":phase,"sql":sql,"packet":response,"error_code":1054,"sqlstate":"42S22","alias_value":1,"self_join":[1,1]})).unwrap();
            }
        }
        if task == 14 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            let table = format!("t14_{}_{}", std::process::id(), address.port());
            assert_eq!(
                mysql_query(socket, &format!("SELECT id FROM {database}.{table}")),
                vec![vec![Some(b"1".to_vec())]]
            );
            assert_eq!(
                mysql_query(socket, &format!("SELECT id FROM {database}_other.{table}")),
                vec![vec![Some(b"2".to_vec())]]
            );
            for sql in [
                format!("SELECT id FROM {database}.public.{table}"),
                format!("SELECT id FROM {database}.{table}.extra"),
            ] {
                let mut request = vec![3];
                request.extend_from_slice(sql.as_bytes());
                mysql_packet(socket, 0, &request);
                let response = mysql_read(socket);
                assert_eq!(response[0], 0xff);
                assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1064);
                assert_eq!(&response[4..9], b"42000");
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(evidence.join("mysql-relations.jsonl"))
                    .unwrap();
                writeln!(file,"{}",serde_json::json!({"phase":phase,"sql":sql,"error_code":1064,"sqlstate":"42000","packet":response,"current_value":1,"other_value":2})).unwrap();
            }
        }
        if matches!(task, 13 | 15) {
            mysql_query(socket, "SET SESSION sql_mode = ''");
            assert_eq!(
                mysql_query(socket, "SELECT \"Foo\""),
                vec![vec![Some(b"Foo".to_vec())]]
            );
            mysql_query(socket, "SET SESSION sql_mode = 'ANSI_QUOTES'");
            let mut request = vec![3];
            request.extend_from_slice(b"SELECT \"Foo\"");
            mysql_packet(socket, 0, &request);
            let response = mysql_read(socket);
            assert_eq!(response[0], 0xff);
            assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1054);
            assert_eq!(&response[4..9], b"42S22");
            mysql_query(socket, "SET SESSION sql_mode = ''");
            let mut output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-identifier-isolation.jsonl"))
                .unwrap();
            writeln!(output,"{}",serde_json::json!({"phase":phase,"default_value":"Foo","ansi_error_code":1054,"ansi_sqlstate":"42S22"})).unwrap();
        }
        if task == 3 {
            for (mode, expected) in [
                ("", b"1".as_slice()),
                ("HIGH_NOT_PRECEDENCE", b"0".as_slice()),
            ] {
                mysql_query(socket, &format!("SET SESSION sql_mode = '{mode}'"));
                assert_eq!(
                    mysql_query(socket, "SELECT NOT 'Ab' LIKE 'x%'"),
                    vec![vec![Some(expected.to_vec())]]
                );
            }
            mysql_query(socket, "SET SESSION sql_mode = ''");
        }
        if task == 15 && phase.starts_with("PG_DIALECT_CHECKPOINT ") {
            mysql_query(socket, &format!("USE {database}"));
            for sql in [
                "SELECT id FROM foo",
                "SELECT id FROM Foo",
                "SELECT ID FROM foo",
            ] {
                assert_eq!(mysql_query(socket, sql), vec![vec![Some(b"1".to_vec())]]);
            }
            assert_eq!(
                mysql_query(socket, "SELECT \"Foo\""),
                vec![vec![Some(b"Foo".to_vec())]]
            );
        }
        if task == 24 {
            let sql = "SELECT CAST(1 AS SIGNED) AS pg_core_mysql_identity_24";
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(socket, 0, &request);
            assert_eq!(mysql_read(socket), vec![1]);
            let cast_column = mysql_read(socket);
            assert_eq!(column_type(&cast_column), 253);
            assert_eq!(mysql_read(socket)[0], 0xfe);
            assert_eq!(mysql_read(socket), b"\x011");
            assert_eq!(mysql_read(socket)[0], 0xfe);
            // Freeze the existing bare scalar CAST metadata separately; the
            // lifecycle contract uses the typed table projection below.
            let table_sql = format!(
                "SELECT id AS pg_core_mysql_identity_24 FROM {database}.dialect_isolation WHERE id=CAST(31 AS SIGNED)"
            );
            let mut table_request = vec![3];
            table_request.extend_from_slice(table_sql.as_bytes());
            mysql_packet(socket, 0, &table_request);
            assert_eq!(mysql_read(socket), vec![1]);
            let table_column = mysql_read(socket);
            assert_eq!(column_type(&table_column), 3);
            assert_eq!(mysql_read(socket)[0], 0xfe);
            assert_eq!(mysql_read(socket), b"\x0231");
            assert_eq!(mysql_read(socket)[0], 0xfe);
            let mut request = vec![3];
            request.extend_from_slice(b"SELECT 1::int4");
            mysql_packet(socket, 0, &request);
            let error = mysql_read(socket);
            assert_eq!(error[0], 0xff);
            assert_eq!(u16::from_le_bytes(error[1..3].try_into().unwrap()), 1064);
            assert_eq!(&error[4..9], b"42000");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-lifecycle.jsonl"))
                .unwrap();
            writeln!(file, "{}", serde_json::json!({"phase":phase,"sql":"SELECT 1::int4", "code":1064,"state":"42000", "packet":error,"bare_cast_type_baseline":253,"bare_cast_column":cast_column,"table_cast_type":3,"table_cast_sql":table_sql,"table_cast_column":table_column})).unwrap();
        }
        let sql = format!("SELECT id AS v FROM {database}.dialect_isolation WHERE id=31");
        let mut request = vec![3];
        request.extend_from_slice(sql.as_bytes());
        mysql_packet(socket, 0, &request);
        assert_eq!(mysql_read(socket), vec![1]);
        let column = mysql_read(socket);
        assert_eq!(column_type(&column), 3);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let row = mysql_read(socket);
        assert_eq!(&row, b"\x0231");
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut request = vec![0x16];
        request.extend_from_slice(
            format!(
                "SELECT id AS v FROM {database}.dialect_isolation WHERE id={}",
                if task == 24 {
                    "CAST(? AS SIGNED)"
                } else if task == 12 {
                    "(?)"
                } else {
                    "?"
                }
            )
            .as_bytes(),
        );
        mysql_packet(socket, 0, &request);
        let prepared = mysql_read(socket);
        assert_eq!(prepared[0], 0);
        let id = &prepared[1..5];
        assert_eq!(u16::from_le_bytes(prepared[5..7].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(prepared[7..9].try_into().unwrap()), 1);
        let parameter = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let prepared_column = mysql_read(socket);
        assert_eq!(column_type(&prepared_column), 3);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut request = vec![0x17];
        request.extend_from_slice(id);
        request.extend_from_slice(&[0, 1, 0, 0, 0, 0, 1, 3, 0]);
        request.extend_from_slice(&31_i32.to_le_bytes());
        mysql_packet(socket, 0, &request);
        assert_eq!(mysql_read(socket), vec![1]);
        let executed_column = mysql_read(socket);
        assert_eq!(column_type(&executed_column), 3);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let binary = mysql_read(socket);
        assert_eq!(&binary[..2], &[0, 0]);
        assert_eq!(i32::from_le_bytes(binary[2..6].try_into().unwrap()), 31);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut close = vec![0x19];
        close.extend_from_slice(id);
        mysql_packet(socket, 0, &close);
        let record = serde_json::json!({"run": evidence.file_name().unwrap().to_str().unwrap(),
            "case_id": phase, "side": "mysql", "attempt": 1,
            "input": {"command": sql}, "observation": {"column": column, "text_row": row,
                "parameter": parameter, "prepared_column": prepared_column,
                "binary_row": binary, "executed_column": executed_column}, "outcome": "bypass_health"});
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(evidence.join("mysql-local.jsonl"))
            .unwrap();
        writeln!(file, "{record}").unwrap();
        file.sync_all().unwrap();
    };
    mysql_query(&mut mysql, "SET SESSION sql_mode = ''");
    mysql_query(&mut mysql, "SET NAMES utf8mb4 COLLATE utf8mb4_general_ci");
    let environment = mysql_query(
        &mut mysql,
        "SELECT @@sql_mode, @@character_set_connection, @@collation_connection",
    );
    assert_eq!(
        environment,
        vec![vec![
            Some(b"".to_vec()),
            Some(b"utf8mb4".to_vec()),
            Some(b"utf8mb4_general_ci".to_vec())
        ]]
    );
    std::fs::write(
        evidence.join("mysql-local-environment.json"),
        serde_json::to_string(&environment).unwrap(),
    )
    .unwrap();
    let baseline = mysql_query(&mut mysql, "SELECT 'Ab' ILIKE 'a%'");
    std::fs::write(
        evidence.join("mysql-legacy-extension.json"),
        serde_json::to_string(&baseline).unwrap(),
    )
    .unwrap();
    mysql_query(
        &mut mysql,
        &format!("CREATE TABLE {database}.dialect_isolation (id INT)"),
    );
    mysql_query(
        &mut mysql,
        &format!("INSERT INTO {database}.dialect_isolation VALUES (31)"),
    );
    if task == 4 {
        // Observe the existing MySQL extension inside a rolled-back transaction.
        // Keep its acceptance distinct from the official MySQL reference rejection.
        mysql_query(&mut mysql, "BEGIN");
        let baseline_sql =
            format!("INSERT INTO {database}.dialect_isolation VALUES (47) RETURNING id");
        let returned = mysql_query(&mut mysql, &baseline_sql);
        let stored = mysql_query(
            &mut mysql,
            &format!("SELECT id FROM {database}.dialect_isolation ORDER BY id"),
        );
        assert_eq!(
            stored,
            vec![vec![Some(b"31".to_vec())], vec![Some(b"47".to_vec())]]
        );
        mysql_query(&mut mysql, "ROLLBACK");
        std::fs::write(
            evidence.join("mysql-returning-baseline.json"),
            serde_json::json!({"sql": baseline_sql, "accepted": true,
                "returned": returned, "stored_before_rollback": stored,
                "rollback": true})
            .to_string(),
        )
        .unwrap();
    }
    if task == 9 {
        for sql in [
            "SELECT ARRAY[1,2]",
            "SELECT ARRAY[]",
            "SELECT ARRAY[NULL,1]",
            "SELECT ARRAY[$2,$1,$2]",
            "SELECT ARRAY[1,2",
        ] {
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(&mut mysql, 0, &request);
            let response = mysql_read(&mut mysql);
            assert_eq!(response[0], 0xff);
            assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1064);
            assert_eq!(&response[4..9], b"42000");
            let record = serde_json::json!({"sql": sql, "error_code":1064, "sqlstate":"42000", "packet":response});
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-array-isolation.jsonl"))
                .unwrap();
            writeln!(file, "{record}").unwrap();
        }
        assert_eq!(
            mysql_query(&mut mysql, "SELECT 'ARRAY[1,2]' /* ARRAY[] */"),
            vec![vec![Some(b"ARRAY[1,2]".to_vec())]]
        );
    }
    if task == 11 {
        let mut baseline = vec![];
        for sql in [
            "SELECT ROW(1,2)",
            "SELECT ROW(1)",
            "SELECT ROW(1,ROW(2,3))",
            "SELECT ROW(1,",
        ] {
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(&mut mysql, 0, &request);
            let response = mysql_read(&mut mysql);
            assert_eq!(
                response[0], 0xff,
                "freeze existing MySQL projection/syntax errors"
            );
            let expected = if sql == "SELECT ROW(1)" || sql == "SELECT ROW(1," {
                (1064, b"42000".as_slice())
            } else {
                (1105, b"HY000".as_slice())
            };
            assert_eq!(
                u16::from_le_bytes(response[1..3].try_into().unwrap()),
                expected.0
            );
            assert_eq!(&response[4..9], expected.1);
            baseline.push(serde_json::json!({"sql":sql, "error_code":u16::from_le_bytes(response[1..3].try_into().unwrap()), "sqlstate":String::from_utf8_lossy(&response[4..9]), "packet":response}));
        }
        std::fs::write(
            evidence.join("mysql-row-baseline.json"),
            serde_json::to_string(&baseline).unwrap(),
        )
        .unwrap();
        assert_eq!(
            mysql_query(&mut mysql, "SELECT ROW(1,2)=ROW(1,2)"),
            vec![vec![Some(b"1".to_vec())]]
        );
        assert_eq!(
            mysql_query(&mut mysql, "SELECT 'ROW(1,2)' /* ROW() */"),
            vec![vec![Some(b"ROW(1,2)".to_vec())]]
        );
    }
    checkpoint(&mut mysql, "before");
    let mut checkpoints = 0;
    for line in std::io::BufReader::new(child.0.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        println!("{line}");
        if line.starts_with("PG_DIALECT_CHECKPOINT ") {
            checkpoint(&mut mysql, &line);
            checkpoints += 1;
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"continue\n")
                .unwrap();
        }
    }
    let status = child.0.wait().unwrap();
    std::fs::write(
        evidence.join("child-exit.json"),
        serde_json::json!({"success": status.success(), "code": status.code()}).to_string(),
    )
    .unwrap();
    drop(child);
    checkpoint(&mut mysql, "after");
    mysql_packet(&mut mysql, 0, &[1]);
    drop(cleanup_guard);
    let cleanup: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("rust-cleanup.json")).unwrap())
            .unwrap();
    assert_eq!(cleanup["database_dropped"], true);
    assert_eq!(cleanup["postgres_port_closed"], true);
    assert_eq!(cleanup["mysql_port_closed"], true);
    assert!(status.success(), "dialect handoff failed; see evidence");
    if task == 6 {
        assert_eq!(checkpoints, 10, "one libpq checkpoint per DELETE case");
        let baseline: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(evidence.join("mysql-local-delete-baseline.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(baseline["phase"], "before PG Parse");
        assert_eq!(baseline["accepted"], true);
        assert_eq!(baseline["executed"], false);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cases"], 10);
        assert_eq!(summary["unique_effective"], 6);
        assert!(summary["assertions"].as_u64().unwrap_or_default() > 0);
        assert_eq!(summary["using_gate_cases"], 2);
        assert_eq!(summary["semantic_returning_success"], 0);
        assert_eq!(summary["cleanup_ok"], true);
        return;
    }
    if task == 24 {
        assert_eq!(checkpoints, 6);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 6);
        let observed = crate::runtime::DIALECT_EXECUTIONS.lock().unwrap();
        for (marker, dialect) in [
            (
                "pg_core_pg_identity_24",
                astersql_parser_ast::SqlDialect::PostgreSQL,
            ),
            (
                "pg_core_mysql_identity_24",
                astersql_parser_ast::SqlDialect::MySQL,
            ),
        ] {
            let entries = observed
                .iter()
                .filter(|(sql, _)| sql.contains(marker))
                .collect::<Vec<_>>();
            assert!(
                entries.len() >= 8,
                "missing current connection identity evidence: {marker}"
            );
            assert!(entries.iter().all(|(_, actual)| *actual == dialect));
        }
        return;
    }
    if task == 19 {
        assert_eq!(checkpoints, 6);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 6);
        assert_eq!(summary["physical_dependency_cases"], 3);
        assert_eq!(summary["primary_prepare_cases"], 5);
        assert_eq!(summary["primary_describe_cases"], 5);
        assert_eq!(summary["primary_execute_cases"], 5);
        assert_eq!(summary["diagnostic_cases"], 1);
        assert_eq!(summary["failures"], serde_json::json!([]));
        return;
    }
    if task == 18 {
        assert_eq!(checkpoints, 24);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 6);
        assert_eq!(summary["integer_dependency_cases"], 18);
        assert_eq!(summary["primary_prepare_cases"], 5);
        assert_eq!(summary["primary_describe_cases"], 5);
        assert_eq!(summary["primary_execute_cases"], 5);
        assert_eq!(summary["diagnostic_cases"], 1);
        assert_eq!(summary["failures"], serde_json::json!([]));
        return;
    }
    if task == 17 {
        assert_eq!(checkpoints, 9);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 9);
        assert_eq!(summary["prepare_describe_cases"], 3);
        assert_eq!(summary["diagnostic_cases"], 6);
        assert_eq!(summary["full_dml_execution_cases"], 0);
        assert_eq!(
            pg_dialect_jsonl(&evidence.join("mysql-excluded.jsonl")).len(),
            9
        );
        return;
    }
    if task == 16 {
        assert_eq!(checkpoints, 5);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 5);
        return;
    }
    if task == 15 {
        assert_eq!(checkpoints, 8);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 8);
        assert!(summary["assertions"].as_u64().unwrap() > 0);
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-identifier-isolation.jsonl"))
                .unwrap()
                .lines()
                .count(),
            10
        );
        return;
    }
    if task == 14 {
        assert_eq!(checkpoints, 11);
        assert_eq!(cleanup["other_database_dropped"], true);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 10);
        assert_eq!(summary["value_type_cases"], 6);
        assert_eq!(summary["gate_cases"], 1);
        return;
    }
    if task == 13 {
        assert_eq!(checkpoints, 9);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["syntax_eligible"], 9);
        assert_eq!(summary["gate_cases"], 0);
        assert_eq!(summary["value_type_cases"], 6);
        assert_eq!(summary["unique_effective"], 9);
        assert_eq!(summary["origin_metadata_mismatch_cases"], 0);
        assert_eq!(
            summary["unique_effective"].as_u64().unwrap()
                + summary["origin_metadata_mismatch_cases"].as_u64().unwrap(),
            9
        );
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
                .unwrap()
                .lines()
                .count(),
            11
        );
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-identifier-isolation.jsonl"))
                .unwrap()
                .lines()
                .count(),
            11
        );
        return;
    }
    if task == 12 {
        assert_eq!(checkpoints, 10);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["unique_effective"], 10);
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["gate_cases"], 0);
        assert_eq!(summary["semantic_row_success"], 5);
        assert_eq!(summary["semantic_expression_success"], 7);
        return;
    }
    if task == 11 {
        assert_eq!(checkpoints, 10);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["unique_effective"], 10);
        assert_eq!(summary["semantic_row_success"], 7);
        assert_eq!(summary["gate_cases"], 0);
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
                .unwrap()
                .lines()
                .count(),
            12
        );
        return;
    }
    if task == 9 {
        assert_eq!(checkpoints, 6);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["syntax_eligible"], 6);
        assert_eq!(summary["gate_cases"], 0);
        assert_eq!(summary["semantic_array_success"], 3);
        assert_eq!(summary["unique_effective"], 6);
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
                .unwrap()
                .lines()
                .count(),
            8
        );
        return;
    }
    if task == 10 {
        assert_eq!(checkpoints, 8);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cases"], 8);
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["syntax_eligible"], 8);
        assert_eq!(summary["gate_cases"], 0);
        assert_eq!(summary["semantic_array_success"], 5);
        assert_eq!(summary["semantic_error_cases"], 3);
        assert_eq!(summary["unique_effective"], 8);
        assert!(summary["assertions"].as_u64().unwrap() >= 100);
        assert_eq!(summary["self_checks"], 2);
        let mysql_local = pg_dialect_jsonl(&evidence.join("mysql-local.jsonl"));
        let mysql_array_cases = mysql_local
            .iter()
            .filter(|record| record["mysql_error_code"].as_u64() == Some(1064))
            .filter_map(|record| record["case_id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(mysql_array_cases.len(), 8);
        let mysql: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(evidence.join("provenance.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(mysql["reference"]["server_version_num"], "180006");
        assert!(mysql["mysql_reference_build"].is_object());
        let mysql_prepare: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(evidence.join("mysql-local-prepare.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(mysql_prepare["prepared"], true);
        assert_eq!(mysql_prepare["mysql_type"], 253);
        assert_eq!(mysql_prepare["isolation_column_type"], 3);
        return;
    }
    if task == 5 {
        assert_eq!(checkpoints, 6);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cases"], 6);
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["syntax_eligible"], 6);
        assert_eq!(summary["unique_effective"], 5);
        assert_eq!(summary["update_from_gate_cases"], 1);
        assert_eq!(summary["update_execute_gate_cases"], 1);
        assert_eq!(summary["mysql_reference_cases"], 2);
        assert!(summary["assertions"].as_u64().unwrap() >= 100);
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-update-from.jsonl"))
                .unwrap()
                .lines()
                .count(),
            6
        );
        let cases: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("cases.json")).unwrap())
                .unwrap();
        assert_eq!(cases.as_array().unwrap().len(), 6);
        let mysql: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(evidence.join("mysql-reference.json")).unwrap(),
        )
        .unwrap();
        assert!(
            mysql["identity"][0]
                .as_str()
                .unwrap()
                .starts_with("8.4.7\t")
        );
        assert_ne!(mysql["update_from"]["code"], 0);
        return;
    }
    if task == 7 {
        assert_eq!(checkpoints, 8);
        let summary = pg_dialect_07_audit(&evidence);
        assert_eq!(summary["cases"], 8);
        assert_eq!(summary["unique_effective"], 8);
        assert_eq!(summary["valid_prepare_describe_cases"], 3);
        assert_eq!(summary["syntax_negative_parse_cases"], 5);
        assert_eq!(summary["full_dml_semantic_successes"], 0);
        return;
    }
    if task == 8 {
        let cases: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("cases.json")).unwrap())
                .unwrap();
        let case_count = cases.as_array().unwrap().len();
        assert_eq!(checkpoints, case_count);
        let mysql_local_cases = std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
            .unwrap()
            .lines()
            .filter(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .is_ok_and(|record| record["postgresql_sql"].is_string())
            })
            .count();
        assert_eq!(mysql_local_cases, case_count);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cases"], case_count);
        assert_eq!(summary["unique_effective"], case_count);
        assert_eq!(summary["valid_prepare_describe_cases"], 3);
        assert_eq!(summary["syntax_negative_parse_cases"], case_count - 3);
        assert_eq!(summary["full_dml_semantic_successes"], 0);
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["failures"], serde_json::json!([]));
        return;
    }
    assert_eq!(checkpoints, if task == 4 { 8 } else { 10 });
    let summary_path = evidence.join("summary.json");
    let mut summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["cleanup_ok"], true);
    assert_eq!(summary["syntax_eligible"], if task == 4 { 8 } else { 10 });
    assert_eq!(
        summary["self_checks"],
        if task == 1 {
            15
        } else if task == 2 {
            14
        } else if task == 3 {
            13
        } else {
            21
        }
    );
    assert_eq!(
        std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
            .unwrap()
            .lines()
            .count(),
        if task == 4 { 10 } else { 12 }
    );
    if task == 4 {
        // Gate evidence is distinct from RETURNING result semantics.
        assert_eq!(summary["gate_cases"], 4);
        assert_eq!(summary["unique_effective"], 8);
        assert_eq!(summary["returning_metadata_success"], 4);
        assert_eq!(summary["semantic_returning_success"], 0);
    } else {
        summary["unique_effective"] = 10.into();
    }
    std::fs::write(summary_path, summary.to_string()).unwrap();
}
