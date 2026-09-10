use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::dashboard::{
    AutoscaleView, ChunksView, DashboardListenConfig, DashboardStats, EntitiesView, INDEX_HTML,
    LatencyUs, MAX_HEAD_BYTES, MemoryView, NetworkView, PlayersView, PluginDisableView,
    PluginsView, SaveReportView, SpawnCategoryReport, SpawnView, StatsPayload, TickView, WorldView,
    parse_head, serve_dashboard, spawn_dashboard,
};

/// A provider returning one fixed payload, so responses are assertable.
struct FixedProvider {
    payload: StatsPayload,
}

impl DashboardStats for FixedProvider {
    fn stats(&self) -> StatsPayload {
        self.payload.clone()
    }

    fn profile(&self) -> crate::profile::ProfileReport {
        crate::profile::ProfileSampler::new().capture(self.stats(), || serde_json::Value::Null)
    }
}

/// Fully populated payload exercising every contract field.
fn sample_stats() -> StatsPayload {
    StatsPayload {
        version: "0.0.2-alpha.1".to_owned(),
        uptime_secs: 3_721,
        world: WorldView {
            name: "alpha".to_owned(),
            motd: "Solaris alpha".to_owned(),
            seed: 712_816,
            mode: "survival".to_owned(),
            online_mode: true,
            view_distance: 8,
            simulation_distance: 6,
            max_players: 20,
        },
        players: PlayersView {
            count: 2,
            max: 20,
            names: vec!["KaiserRoman".to_owned(), "Steve".to_owned()],
        },
        tps: 19.87,
        tick: TickView {
            total: LatencyUs {
                samples: 1_200,
                p50_us: 2_400,
                p95_us: 9_800,
                p99_us: 15_000,
                max_us: 41_000,
            },
            stages: BTreeMap::from([
                (
                    "chunk_pipeline".to_owned(),
                    LatencyUs {
                        samples: 1_200,
                        p50_us: 900,
                        p95_us: 3_100,
                        p99_us: 4_700,
                        max_us: 12_000,
                    },
                ),
                (
                    "entities".to_owned(),
                    LatencyUs {
                        samples: 1_200,
                        p50_us: 310,
                        p95_us: 800,
                        p99_us: 1_100,
                        max_us: 2_600,
                    },
                ),
            ]),
        },
        memory: MemoryView {
            used_mb: 142,
            limit_mb: 512,
            available_mb: 370,
        },
        autoscale: AutoscaleView {
            enabled: true,
            profile: "balanced".to_owned(),
            view_distance: 8,
            chunk_send_rate: 32,
            chunk_load_rate: 128,
            chunk_generate_rate: 4,
            scale_up_decisions: 3,
            scale_down_decisions: 1,
            draining: false,
        },
        chunks: ChunksView {
            ticketed: 61,
            prepared: 44,
            loaded_total: 12_904,
            generated_total: 3_312,
            streamed_total: 9_811,
        },
        entities: EntitiesView {
            total: 214,
            categories: BTreeMap::from([("friendly".to_owned(), 96), ("hostile".to_owned(), 118)]),
        },
        spawn: SpawnView {
            friendly: SpawnCategoryReport {
                attempts: 400,
                chunks_sampled: 48,
                templates_considered: 130,
                committed: 12,
                rejected_unloaded: 3,
                rejected_time: 0,
                rejected_player_distance: 40,
                rejected_block_or_fluid: 51,
                rejected_darkness: 60,
                rejected_collision: 22,
                rejected_duplicate: 2,
            },
            hostile: SpawnCategoryReport {
                attempts: 8_000,
                chunks_sampled: 96,
                templates_considered: 2_200,
                committed: 71,
                rejected_unloaded: 30,
                rejected_time: 900,
                rejected_player_distance: 1_500,
                rejected_block_or_fluid: 2_100,
                rejected_darkness: 1_800,
                rejected_collision: 900,
                rejected_duplicate: 5,
            },
        },
        save: Some(SaveReportView {
            age_secs: 94,
            players_saved: 2,
            entities_saved: 214,
            chunks_flushed: 61,
            world_metadata_saved: true,
            elapsed_ms: 182,
            errors: vec!["region 0,3 write retried once".to_owned()],
        }),
        network: NetworkView {
            bytes_written: 81_234_567,
            reliable_drops: 2,
            reliable_retries: 14,
            slow_client_sheds: 1,
            best_effort_animation_drops: 39,
        },
        plugins: PluginsView {
            loaded: vec!["solaris-essentials".to_owned()],
            disabled: vec![PluginDisableView {
                plugin: "solaris-towns".to_owned(),
                stage: "resolve".to_owned(),
                message: "manifest missing capability 'claims'".to_owned(),
            }],
        },
        warnings: vec!["backup directory not configured".to_owned()],
    }
}

/// Binds an ephemeral loopback listener and serves the dashboard on it.
async fn spawn_test_dashboard() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let provider = Arc::new(FixedProvider {
        payload: sample_stats(),
    });
    tokio::spawn(serve_dashboard(listener, provider));
    addr
}

/// Sends one raw request, half-closes, and reads until the server closes.
async fn exchange(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let _ = stream.shutdown().await;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

/// The body half of a raw response.
fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default()
}

/// Sorted key names of a JSON object.
fn sorted_keys(value: &serde_json::Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn parses_request_line_and_strips_query_strings() {
    let request = parse_head("GET /stats?since=3&x=1 HTTP/1.1\r\nHost: x").unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/stats");
    assert!(parse_head("GET / HTTP/1.0\r\nHost: x").is_some());
}

#[test]
fn rejects_malformed_request_lines() {
    for head in [
        "",
        "\r\nHost: x",
        "GET /\r\nHost: x",
        "GET / HTTP/2.0\r\n",
        "GET /x HTTP/1.1 extra\r\n",
        "GET  / HTTP/1.1\r\n",
        "CONNECT server:25565 HTTP/1.1\r\n",
    ] {
        assert!(parse_head(head).is_none(), "expected rejection of {head:?}");
    }
}

#[test]
fn stats_payload_serializes_every_contract_field() {
    let value = serde_json::to_value(sample_stats()).unwrap();

    assert_eq!(
        sorted_keys(&value),
        [
            "autoscale",
            "chunks",
            "entities",
            "memory",
            "network",
            "players",
            "plugins",
            "save",
            "spawn",
            "tick",
            "tps",
            "uptime_secs",
            "version",
            "warnings",
            "world",
        ]
    );
    assert_eq!(
        sorted_keys(&value["world"]),
        [
            "max_players",
            "mode",
            "motd",
            "name",
            "online_mode",
            "seed",
            "simulation_distance",
            "view_distance",
        ]
    );
    assert_eq!(sorted_keys(&value["players"]), ["count", "max", "names"]);
    assert_eq!(sorted_keys(&value["tick"]), ["stages", "total"]);
    assert_eq!(
        sorted_keys(&value["tick"]["total"]),
        ["max_us", "p50_us", "p95_us", "p99_us", "samples"]
    );
    assert_eq!(
        sorted_keys(&value["tick"]["stages"]["entities"]),
        ["max_us", "p50_us", "p95_us", "p99_us", "samples"]
    );
    assert_eq!(
        sorted_keys(&value["memory"]),
        ["available_mb", "limit_mb", "used_mb"]
    );
    assert_eq!(
        sorted_keys(&value["autoscale"]),
        [
            "chunk_generate_rate",
            "chunk_load_rate",
            "chunk_send_rate",
            "draining",
            "enabled",
            "profile",
            "scale_down_decisions",
            "scale_up_decisions",
            "view_distance",
        ]
    );
    assert_eq!(
        sorted_keys(&value["chunks"]),
        [
            "generated_total",
            "loaded_total",
            "prepared",
            "streamed_total",
            "ticketed",
        ]
    );
    assert_eq!(sorted_keys(&value["entities"]), ["categories", "total"]);
    assert_eq!(sorted_keys(&value["spawn"]), ["friendly", "hostile"]);
    assert_eq!(
        sorted_keys(&value["spawn"]["friendly"]),
        [
            "attempts",
            "chunks_sampled",
            "committed",
            "rejected_block_or_fluid",
            "rejected_collision",
            "rejected_darkness",
            "rejected_duplicate",
            "rejected_player_distance",
            "rejected_time",
            "rejected_unloaded",
            "templates_considered",
        ]
    );
    assert_eq!(
        sorted_keys(&value["save"]),
        [
            "age_secs",
            "chunks_flushed",
            "elapsed_ms",
            "entities_saved",
            "errors",
            "players_saved",
            "world_metadata_saved",
        ]
    );
    assert_eq!(
        sorted_keys(&value["network"]),
        [
            "best_effort_animation_drops",
            "bytes_written",
            "reliable_drops",
            "reliable_retries",
            "slow_client_sheds",
        ]
    );
    assert_eq!(sorted_keys(&value["plugins"]), ["disabled", "loaded"]);
    assert_eq!(
        sorted_keys(&value["plugins"]["disabled"][0]),
        ["message", "plugin", "stage"]
    );

    assert_eq!(value["world"]["seed"].as_i64(), Some(712_816));
    assert_eq!(value["tps"].as_f64(), Some(19.87));
    assert_eq!(
        value["warnings"].as_array().unwrap().len(),
        1,
        "warnings must serialize as an array"
    );
}

#[test]
fn stats_payload_serializes_missing_save_as_null() {
    let mut payload = sample_stats();
    payload.save = None;
    let value = serde_json::to_value(payload).unwrap();
    assert!(value.get("save").is_some_and(serde_json::Value::is_null));
}

#[tokio::test]
async fn get_root_serves_embedded_html() {
    let addr = spawn_test_dashboard().await;
    let response = exchange(addr, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(response.contains(&format!("Content-Length: {}\r\n", INDEX_HTML.len())));
    assert!(response.contains("Connection: close\r\n"));
    assert_eq!(body_of(&response), INDEX_HTML);
}

#[tokio::test]
async fn get_stats_returns_provider_json() {
    let addr = spawn_test_dashboard().await;
    let response = exchange(addr, "GET /stats HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Content-Type: application/json\r\n"));
    assert!(response.contains("Connection: close\r\n"));
    let body: serde_json::Value = serde_json::from_str(body_of(&response)).unwrap();
    assert_eq!(body, serde_json::to_value(sample_stats()).unwrap());
}

#[tokio::test]
async fn query_strings_are_ignored() {
    let addr = spawn_test_dashboard().await;
    let response = exchange(
        addr,
        "GET /stats?since=3&fresh=1 HTTP/1.1\r\nHost: x\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    let body: serde_json::Value = serde_json::from_str(body_of(&response)).unwrap();
    assert_eq!(body, serde_json::to_value(sample_stats()).unwrap());

    let root = exchange(addr, "GET /?tab=world HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(root.starts_with("HTTP/1.1 200 OK\r\n"), "{root}");
}

#[tokio::test]
async fn non_get_methods_get_405_with_allow_header() {
    let addr = spawn_test_dashboard().await;
    for method in ["HEAD", "POST", "PUT", "DELETE", "OPTIONS", "get"] {
        let response = exchange(
            addr,
            &format!("{method} /stats HTTP/1.1\r\nHost: x\r\n\r\n"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 405"), "{method}: {response}");
        assert!(response.contains("Allow: GET\r\n"), "{method}: {response}");
        assert!(
            response.contains("Connection: close\r\n"),
            "{method}: {response}"
        );
    }
}

#[tokio::test]
async fn unknown_paths_get_404() {
    let addr = spawn_test_dashboard().await;
    for path in ["/nope", "/stats/", "/index.html", "/stats/extra"] {
        let response = exchange(addr, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n")).await;
        assert!(response.starts_with("HTTP/1.1 404"), "{path}: {response}");
        assert!(
            response.contains("Connection: close\r\n"),
            "{path}: {response}"
        );
    }
}

#[tokio::test]
async fn malformed_request_line_gets_400() {
    let addr = spawn_test_dashboard().await;
    for request in [
        "NOT A REQUEST\r\n\r\n",
        "\r\n\r\n",
        "GET /\r\n\r\n",
        "GET / HTTP/9.9\r\n\r\n",
    ] {
        let response = exchange(addr, request).await;
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "{request:?}: {response}"
        );
        assert!(response.contains("Connection: close\r\n"));
    }
}

#[tokio::test]
async fn empty_request_gets_400() {
    let addr = spawn_test_dashboard().await;
    let response = exchange(addr, "").await;
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
}

#[tokio::test]
async fn oversized_request_head_gets_431() {
    let addr = spawn_test_dashboard().await;
    // More bytes than the head cap, with no terminator anywhere.
    let raw = "A".repeat(MAX_HEAD_BYTES + 16);
    let response = exchange(addr, &raw).await;
    assert!(response.starts_with("HTTP/1.1 431"), "{response}");
    assert!(response.contains("Connection: close\r\n"));
}

#[tokio::test]
async fn spawn_dashboard_task_ends_when_bind_fails() {
    let hold = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let taken_port = hold.local_addr().unwrap().port();
    let cfg = DashboardListenConfig {
        bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: taken_port,
    };
    let provider = Arc::new(FixedProvider {
        payload: sample_stats(),
    });
    let handle = spawn_dashboard(cfg, provider);
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("bind failure should end the dashboard task")
        .unwrap();
}

#[test]
fn html_page_is_self_contained() {
    let lowered = INDEX_HTML.to_ascii_lowercase();
    assert!(!lowered.contains("http"), "no external URLs expected");
    assert!(
        !lowered.contains("<link"),
        "no external stylesheets expected"
    );
    assert!(
        !lowered.contains("src="),
        "no external scripts or images expected"
    );
    assert!(!lowered.contains("@import"), "no css imports expected");
    assert!(!lowered.contains("url("), "no url() references expected");
    assert!(INDEX_HTML.contains("Solaris Dashboard"));
    assert!(INDEX_HTML.contains("\"/stats\""), "page must poll /stats");
    assert!(
        INDEX_HTML.contains("setInterval(poll, 2000)"),
        "poll cadence must be 2 s"
    );
}
