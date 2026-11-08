use savepoint_state::config::{redact_url, ProviderKind, StateConfig};
use std::path::Path;

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
}

fn parse(yaml: &str) -> savepoint_state::Result<StateConfig> {
    StateConfig::parse(
        yaml,
        Path::new("/ws/.sp-state.yaml"),
        &env_of(&[("PG_URL", "postgres://u:secret@db:5432/app"), ("EMPTY", "")]),
    )
}

#[test]
fn parses_every_provider_kind_with_defaults() {
    let cfg = parse(
        r#"
providers:
  - name: app-db
    kind: postgres
    url: ${PG_URL}
  - name: legacy
    kind: mysql
    url: mysql://root:pw@127.0.0.1:3306/shop
    terminate_connections: false
    lock_timeout_secs: 5
  - name: cache
    kind: redis
    url: redis://127.0.0.1:6379/2
    exclude: ["bull:*"]
  - name: sessions
    kind: memcached
    url: memcached://127.0.0.1:11211
    include: ["sess:*"]
  - name: local
    kind: sqlite
    path: data/app.db
"#,
    )
    .unwrap();

    assert_eq!(cfg.providers.len(), 5);
    let pg = &cfg.providers[0];
    assert_eq!(pg.kind, ProviderKind::Postgres);
    assert_eq!(pg.url.as_deref(), Some("postgres://u:secret@db:5432/app"));
    assert!(pg.terminate_connections, "default terminate_connections = true");
    assert!(pg.consistent, "default consistent = true");
    assert_eq!(pg.lock_timeout_secs, 30);

    let my = &cfg.providers[1];
    assert!(!my.terminate_connections);
    assert_eq!(my.lock_timeout_secs, 5);

    assert_eq!(cfg.providers[2].exclude, vec!["bull:*".to_string()]);
    assert_eq!(cfg.providers[3].include, vec!["sess:*".to_string()]);
    assert_eq!(
        cfg.providers[4].path.as_deref(),
        Some(Path::new("data/app.db"))
    );
}

#[test]
fn env_interpolation_defaults_escapes_and_missing_vars() {
    let cfg = parse(
        r#"
providers:
  - name: a
    kind: redis
    url: redis://${HOST:-127.0.0.1}:6379/0
  - name: b
    kind: sqlite
    path: "cost$$/x${EMPTY:-fallback}.db"
"#,
    )
    .unwrap();
    assert_eq!(cfg.providers[0].url.as_deref(), Some("redis://127.0.0.1:6379/0"));
    // `${EMPTY:-x}` uses the default when the variable is set but empty (POSIX semantics).
    assert_eq!(
        cfg.providers[1].path.as_deref(),
        Some(Path::new("cost$/xfallback.db"))
    );

    let err = parse(
        r#"
providers:
  - name: a
    kind: postgres
    url: ${NOPE_NOT_SET}
"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("NOPE_NOT_SET"), "error must name the variable: {err}");
}

#[test]
fn rejects_invalid_configs_with_actionable_errors() {
    let cases: &[(&str, &str)] = &[
        (
            "providers:\n  - name: a\n    kind: postgres\n    url: postgres://x/y\n    terminate_conections: true\n",
            "terminate_conections",
        ),
        (
            "providers:\n  - name: a\n    kind: redis\n    url: redis://x\n  - name: a\n    kind: redis\n    url: redis://y\n",
            "duplicate",
        ),
        ("providers:\n  - name: \"bad name\"\n    kind: redis\n    url: redis://x\n", "name"),
        ("providers:\n  - name: a\n    kind: sqlite\n", "path"),
        ("providers:\n  - name: a\n    kind: postgres\n", "url"),
        (
            "providers:\n  - name: a\n    kind: postgres\n    url: mysql://x/y\n",
            "scheme",
        ),
        (
            "providers:\n  - name: a\n    kind: postgres\n    url: postgres://x/y\n    exclude: [\"a*\"]\n",
            "exclude",
        ),
        ("providers:\n  - name: a\n    kind: mongodb\n    url: mongodb://x\n", "kind"),
    ];
    for (yaml, needle) in cases {
        let err = parse(yaml).expect_err(yaml).to_string();
        assert!(
            err.to_lowercase().contains(&needle.to_lowercase()),
            "error for\n{yaml}\nshould mention `{needle}`, got: {err}"
        );
    }
}

#[test]
fn load_returns_none_without_config_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(StateConfig::load(dir.path()).unwrap().is_none());

    std::fs::write(
        dir.path().join(".sp-state.yaml"),
        "providers:\n  - name: db\n    kind: sqlite\n    path: app.db\n",
    )
    .unwrap();
    let cfg = StateConfig::load(dir.path()).unwrap().unwrap();
    assert_eq!(cfg.providers[0].name, "db");

    std::fs::write(dir.path().join(".sp-state.yaml"), "providers: []\n").unwrap();
    assert!(StateConfig::load(dir.path()).unwrap().is_none());
}

#[test]
fn redacts_passwords_in_urls() {
    assert_eq!(
        redact_url("postgres://user:s3cr3t@db:5432/app"),
        "postgres://user:***@db:5432/app"
    );
    assert_eq!(redact_url("redis://:pw@127.0.0.1:6379/0"), "redis://:***@127.0.0.1:6379/0");
    assert_eq!(redact_url("redis://127.0.0.1:6379/0"), "redis://127.0.0.1:6379/0");
    assert_eq!(
        redact_url("mysql://root@localhost/x"),
        "mysql://root@localhost/x"
    );
}
