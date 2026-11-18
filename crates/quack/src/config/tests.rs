use std::collections::BTreeMap;
use std::io::Write as _;

use super::*;

const SECRET_32: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn serve_vars(dashboard: &std::path::Path) -> BTreeMap<String, String> {
    vars(&[
        ("database_url", "postgres://quack:secret@localhost/quack"),
        ("discord_token", "token-value"),
        ("discord_application_id", "123456789012345678"),
        ("discord_client_secret", "client-secret"),
        ("public_url", "https://quack.example.com/"),
        ("session_secret", SECRET_32),
        ("dashboard_dir", dashboard.to_str().unwrap()),
    ])
}

fn dashboard() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("quack-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
    dir
}

#[test]
fn a_complete_serve_configuration_has_the_defaults() {
    let c = validate(Need::Serve, &serve_vars(&dashboard())).unwrap();
    assert_eq!(c.database_max_connections, 20);
    assert_eq!(c.listen_addr.to_string(), "0.0.0.0:8080");
    assert_eq!(c.worker_concurrency, 16);
    assert_eq!(c.service_name, "quack");
    assert_eq!(c.environment, "development");
    assert_eq!(c.log_level, "info");
    assert_eq!(c.trusted_proxy_hops, 0);
    assert_eq!(c.public_url.as_deref(), Some("https://quack.example.com"));
    assert_eq!(
        c.discord.as_ref().map(|d| d.application_id),
        Some(123_456_789_012_345_678)
    );
    assert_ne!(c.session_key_fingerprint(), [0; 8]);
}

#[test]
fn every_problem_is_reported_at_once() {
    let mut v = serve_vars(&dashboard());
    v.remove("database_url");
    v.insert("discord_application_id".into(), "abc".into());
    v.insert("public_url".into(), "http://quack.example.com/path".into());
    v.insert("session_secret".into(), "short".into());
    v.insert("database_max_connections".into(), "500".into());
    v.insert("worker_concurrency".into(), "0".into());
    v.insert("listen_addr".into(), "nowhere".into());
    v.insert("trusted_proxy_hops".into(), "9".into());
    v.insert("log_level".into(), "info,[".into());
    v.insert("otlp_endpoint".into(), "collector".into());
    let problems = validate(Need::Serve, &v).unwrap_err().0;
    for key in [
        "QUACK_DATABASE_URL",
        "QUACK_DISCORD_APPLICATION_ID",
        "QUACK_PUBLIC_URL",
        "QUACK_SESSION_SECRET",
        "QUACK_DATABASE_MAX_CONNECTIONS",
        "QUACK_WORKER_CONCURRENCY",
        "QUACK_LISTEN_ADDR",
        "QUACK_TRUSTED_PROXY_HOPS",
        "QUACK_LOG_LEVEL",
        "QUACK_OTLP_ENDPOINT",
    ] {
        assert!(
            problems.iter().any(|p| p.starts_with(key)),
            "{key} missing from {problems:#?}"
        );
    }
    assert_eq!(problems.len(), 10);
}

#[test]
fn each_command_needs_only_its_keys() {
    let db = vars(&[("database_url", "postgres://localhost/quack")]);
    assert!(validate(Need::Migrate, &db).is_ok());
    let problems = validate(Need::Import, &db).unwrap_err().0;
    assert_eq!(
        problems,
        vec!["QUACK_IMPORT_SOURCE_URL is required.".to_owned()]
    );
    let sync = vars(&[("discord_token", "t"), ("discord_application_id", "42")]);
    assert!(validate(Need::CommandsSync, &sync).is_ok());
    assert!(validate(Need::Serve, &db).is_err());
}

#[test]
fn http_is_allowed_only_for_localhost() {
    let mut v = serve_vars(&dashboard());
    v.insert("public_url".into(), "http://localhost:5173".into());
    assert!(validate(Need::Serve, &v).is_ok());
    for bad in [
        "http://quack.example.com",
        "https://user:pw@quack.example.com",
        "https://quack.example.com/?q=1",
        "ftp://localhost",
    ] {
        v.insert("public_url".into(), bad.into());
        assert!(validate(Need::Serve, &v).is_err(), "{bad}");
    }
}

#[test]
fn base64_lengths() {
    assert_eq!(parse::base64_len(SECRET_32), Some(32));
    assert_eq!(
        parse::base64_len("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        Some(32)
    );
    assert_eq!(parse::base64_len("not base64!"), None);
    assert_eq!(parse::base64_len("AAAA"), Some(3));
}

#[test]
fn the_environment_wins_over_the_file_and_unknown_keys_fail() {
    let dir = dashboard();
    let path = dir.join("quack.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(
        f,
        "database_url = \"postgres://file/quack\"\nworker_concurrency = 4"
    )
    .unwrap();
    let v = values(
        Some(&path),
        [
            (
                "QUACK_DATABASE_URL".to_owned(),
                "postgres://env/quack".to_owned(),
            ),
            ("PATH".to_owned(), "/bin".to_owned()),
        ],
    )
    .unwrap();
    assert_eq!(
        v.get("database_url").map(String::as_str),
        Some("postgres://env/quack")
    );
    assert_eq!(v.get("worker_concurrency").map(String::as_str), Some("4"));
    assert_eq!(validate(Need::Migrate, &v).unwrap().worker_concurrency, 4);

    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "database_url = \"x\"\ncolour = \"blue\"\n").unwrap();
    let problems = values(Some(&bad), Vec::new()).unwrap_err().0;
    assert_eq!(
        problems,
        vec!["The config file has an unknown key `colour`.".to_owned()]
    );
}

#[test]
fn secrets_never_print() {
    let c = validate(Need::Serve, &serve_vars(&dashboard())).unwrap();
    let shown = format!("{c:?}");
    for secret in [
        "token-value",
        "client-secret",
        SECRET_32,
        "secret@localhost",
    ] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    assert!(shown.contains("[redacted]"));
}
