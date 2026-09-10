//! Optional first-party, default-off status dashboard: a read-only HTTP/1.1
//! endpoint serving one embedded HTML page plus a `/stats` JSON snapshot.
//!
//! Part of the Solaris engine.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

/// Percentile latency summary of a measured tick phase, in microseconds.
#[derive(Clone, Copy, Debug, Serialize, Default)]
pub struct LatencyUs {
    pub samples: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

/// Natural-spawn accounting for one spawn category.
#[derive(Clone, Debug, Serialize, Default)]
pub struct SpawnCategoryReport {
    pub attempts: u64,
    pub chunks_sampled: u64,
    pub templates_considered: u64,
    pub committed: u64,
    pub rejected_unloaded: u64,
    pub rejected_time: u64,
    pub rejected_player_distance: u64,
    pub rejected_block_or_fluid: u64,
    pub rejected_darkness: u64,
    pub rejected_collision: u64,
    pub rejected_duplicate: u64,
}

/// Snapshot of the most recent completed world save.
#[derive(Clone, Debug, Serialize)]
pub struct SaveReportView {
    pub age_secs: u64,
    pub players_saved: u64,
    pub entities_saved: u64,
    pub chunks_flushed: u64,
    pub world_metadata_saved: bool,
    pub elapsed_ms: u64,
    pub errors: Vec<String>,
}

/// Identity and dimension-level world facts.
#[derive(Clone, Debug, Serialize)]
pub struct WorldView {
    pub name: String,
    pub motd: String,
    pub seed: i64,
    pub mode: String,
    pub online_mode: bool,
    pub view_distance: u32,
    pub simulation_distance: u32,
    pub max_players: u32,
}

/// Currently connected players.
#[derive(Clone, Debug, Serialize)]
pub struct PlayersView {
    pub count: u32,
    pub max: u32,
    pub names: Vec<String>,
}

/// Tick latency totals plus per-stage breakdown.
#[derive(Clone, Debug, Serialize, Default)]
pub struct TickView {
    pub total: LatencyUs,
    pub stages: BTreeMap<String, LatencyUs>,
}

/// Process memory usage.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryView {
    pub used_mb: u64,
    pub limit_mb: u64,
    pub available_mb: u64,
}

/// CPU admission / autoscale policy state and decisions.
#[derive(Clone, Debug, Serialize)]
pub struct AutoscaleView {
    pub enabled: bool,
    pub profile: String,
    pub view_distance: u32,
    pub chunk_send_rate: u32,
    pub chunk_load_rate: u32,
    pub chunk_generate_rate: u32,
    pub scale_up_decisions: u64,
    pub scale_down_decisions: u64,
    pub draining: bool,
}

/// Chunk pipeline counters.
#[derive(Clone, Debug, Serialize)]
pub struct ChunksView {
    pub ticketed: u64,
    pub prepared: u64,
    pub loaded_total: u64,
    pub generated_total: u64,
    pub streamed_total: u64,
}

/// Entity population totals by category.
#[derive(Clone, Debug, Serialize)]
pub struct EntitiesView {
    pub total: u64,
    pub categories: BTreeMap<String, u64>,
}

/// Spawn metrics split by category.
#[derive(Clone, Debug, Serialize, Default)]
pub struct SpawnView {
    pub friendly: SpawnCategoryReport,
    pub hostile: SpawnCategoryReport,
}

/// Network throughput and reliability counters.
#[derive(Clone, Debug, Serialize)]
pub struct NetworkView {
    pub bytes_written: u64,
    pub reliable_drops: u64,
    pub reliable_retries: u64,
    pub slow_client_sheds: u64,
    pub best_effort_animation_drops: u64,
}

/// A plugin that failed to load or was disabled, with the reason.
#[derive(Clone, Debug, Serialize)]
pub struct PluginDisableView {
    pub plugin: String,
    pub stage: String,
    pub message: String,
}

/// Plugin load/disable status.
#[derive(Clone, Debug, Serialize)]
pub struct PluginsView {
    pub loaded: Vec<String>,
    pub disabled: Vec<PluginDisableView>,
}

/// The complete `/stats` payload rendered by the dashboard page.
#[derive(Clone, Debug, Serialize)]
pub struct StatsPayload {
    pub version: String,
    pub uptime_secs: u64,
    pub world: WorldView,
    pub players: PlayersView,
    pub tps: f64,
    pub tick: TickView,
    pub memory: MemoryView,
    pub autoscale: AutoscaleView,
    pub chunks: ChunksView,
    pub entities: EntitiesView,
    pub spawn: SpawnView,
    pub save: Option<SaveReportView>,
    pub network: NetworkView,
    pub plugins: PluginsView,
    pub warnings: Vec<String>,
}

/// Source of the dashboard's `/stats` payload. `stats` must be cheap and
/// non-blocking enough to call from the tokio runtime.
pub trait DashboardStats: Send + Sync + 'static {
    /// Snapshot of the current server statistics.
    fn stats(&self) -> StatsPayload;
    /// Explicit, potentially expensive capture. Call from a blocking worker.
    fn profile(&self) -> crate::profile::ProfileReport;
}

/// Where the dashboard HTTP listener binds.
pub struct DashboardListenConfig {
    pub bind_address: IpAddr,
    pub port: u16,
}

/// Starts the dashboard accept loop on a background task. The task logs and
/// ends if the listener cannot be bound; otherwise it serves until aborted.
pub fn spawn_dashboard(
    cfg: DashboardListenConfig,
    provider: Arc<dyn DashboardStats>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_dashboard(cfg, provider))
}

/// Request-head read cap: heads larger than this are rejected with 431.
pub(crate) const MAX_HEAD_BYTES: usize = 8 * 1024;
/// How long a connection may take to deliver a complete request head.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a connection may take to consume a response.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bind and serve. Binding failure is logged and the task returns.
async fn run_dashboard(cfg: DashboardListenConfig, provider: Arc<dyn DashboardStats>) {
    let addr = SocketAddr::new(cfg.bind_address, cfg.port);
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(bind = %addr, %error, "dashboard listener bind failed");
            return;
        }
    };
    tracing::info!(bind = ?listener.local_addr().ok(), "dashboard listener started");
    serve_dashboard(listener, provider).await;
}

/// Accept loop over an already-bound listener. Exposed for tests.
pub(crate) async fn serve_dashboard(listener: TcpListener, provider: Arc<dyn DashboardStats>) {
    loop {
        match listener.accept().await {
            Ok((mut socket, _peer)) => {
                let provider = Arc::clone(&provider);
                tokio::spawn(async move {
                    serve_connection(&mut socket, &provider).await;
                });
            }
            Err(error) => {
                tracing::warn!(%error, "dashboard accept failed");
            }
        }
    }
}

/// Serves exactly one request on one connection, then closes it.
async fn serve_connection<S>(stream: &mut S, provider: &Arc<dyn DashboardStats>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let head = match tokio::time::timeout(READ_TIMEOUT, read_head(stream)).await {
        Ok(Ok(head)) => head,
        Ok(Err(HeadReadError::TooLarge)) => {
            let response = text_response(
                "431 Request Header Fields Too Large",
                b"request head too large\n",
            );
            let _ = stream.write_all(&response).await;
            return;
        }
        Ok(Err(HeadReadError::ConnectionClosed)) => {
            let response = text_response("400 Bad Request", b"empty or malformed request\n");
            let _ = stream.write_all(&response).await;
            return;
        }
        Err(_elapsed) => return, // client never finished the head; just close
    };
    let response = match parse_head(&head) {
        Some(request) => route(&request, provider),
        None => text_response("400 Bad Request", b"empty or malformed request\n"),
    };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, async {
        stream.write_all(&response).await?;
        stream.flush().await
    })
    .await;
}

/// Reads bytes until the CRLFCRLF head terminator, bounded by
/// [`MAX_HEAD_BYTES`]. Peer EOF or an oversized head is an error.
async fn read_head<S>(stream: &mut S) -> Result<String, HeadReadError>
where
    S: AsyncRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| HeadReadError::ConnectionClosed)?;
        if read == 0 {
            return Err(HeadReadError::ConnectionClosed);
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(end) = head_end(&buf) {
            if end > MAX_HEAD_BYTES {
                return Err(HeadReadError::TooLarge);
            }
            buf.truncate(end);
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(HeadReadError::TooLarge);
        }
    }
}

/// Why [`read_head`] gave up.
enum HeadReadError {
    /// Peer closed or errored before a complete head arrived.
    ConnectionClosed,
    /// Head exceeded [`MAX_HEAD_BYTES`] without terminating.
    TooLarge,
}

/// Index of the first `\r\n\r\n` in `buf`, if present.
fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

/// The parts of a request the dashboard routes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
}

/// Parses a request head (everything before the CRLFCRLF terminator).
/// Only the request line matters; headers and query strings are ignored.
pub(crate) fn parse_head(head: &str) -> Option<Request> {
    let line = head.split("\r\n").next().unwrap_or_default();
    let mut parts = line.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if method.is_empty() || target.is_empty() {
        return None;
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return None;
    }
    let path = target.split('?').next().unwrap_or_default();
    if !path.starts_with('/') {
        return None;
    }
    Some(Request {
        method: method.to_owned(),
        path: path.to_owned(),
    })
}

/// Builds the response bytes for one parsed request.
fn route(request: &Request, provider: &Arc<dyn DashboardStats>) -> Vec<u8> {
    if request.method != "GET" {
        return text_response("405 Method Not Allowed", b"method not allowed; use GET\n");
    }
    match request.path.as_str() {
        "/" => {
            let mut response = http_head(
                "200 OK",
                "text/html; charset=utf-8",
                INDEX_HTML.len(),
                false,
            );
            response.extend_from_slice(INDEX_HTML.as_bytes());
            response
        }
        "/stats" => match serde_json::to_vec(&provider.stats()) {
            Ok(body) => {
                let mut response = http_head("200 OK", "application/json", body.len(), false);
                response.extend_from_slice(&body);
                response
            }
            Err(_) => text_response("500 Internal Server Error", b"stats serialization failed\n"),
        },
        _ => text_response("404 Not Found", b"not found\n"),
    }
}

/// A plain-text error/status body with the standard close headers.
fn text_response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut response = http_head(status, "text/plain; charset=utf-8", body.len(), true);
    response.extend_from_slice(body);
    response
}

/// Serializes the response head. `allow_get` adds the 405 `Allow` header.
fn http_head(status: &str, content_type: &str, content_length: usize, allow_get: bool) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\n"
    );
    if allow_get {
        head.push_str("Allow: GET\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    head.into_bytes()
}

/// The single dashboard page: dark admin layout, no external assets, polls
/// `/stats` every 2 s with vanilla JS and shows a banner while unreachable.
pub(crate) const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Solaris Dashboard</title>
<style>
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.45 system-ui, sans-serif; background: #101418; color: #d7dde3; }
header { display: flex; flex-wrap: wrap; gap: 10px; align-items: baseline;
  padding: 14px 20px; background: #171d24; border-bottom: 1px solid #26303a; }
h1 { margin: 0; font-size: 18px; }
#summary { color: #8fa1b3; font-size: 13px; }
main { display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  gap: 14px; padding: 16px 20px; }
section { background: #171d24; border: 1px solid #26303a; border-radius: 8px;
  padding: 12px 14px; min-width: 0; }
h2 { margin: 0 0 8px; font-size: 12px; text-transform: uppercase; letter-spacing: .6px; color: #7fb2e5; }
table { width: 100%; border-collapse: collapse; }
th, td { padding: 3px 6px; text-align: left; border-bottom: 1px solid #222b34; vertical-align: top; }
th { font-weight: 500; color: #8fa1b3; white-space: nowrap; }
td { word-break: break-word; }
.error { margin: 12px 20px; padding: 10px 12px; border: 1px solid #a33;
  background: #2a1416; color: #e79696; border-radius: 8px; }
.hidden { display: none; }
.ok { color: #7ec699; }
.bad { color: #e08c8c; }
ul { margin: 0; padding-left: 18px; }
</style>
</head>
<body>
<header><h1>Solaris Dashboard</h1><span id="summary">connecting…</span></header>
<div id="error" class="error hidden"></div>
<noscript><p class="error">This dashboard needs JavaScript to poll server stats.</p></noscript>
<main>
<section><h2>Server</h2><table><tbody id="t-server"></tbody></table></section>
<section><h2>World</h2><table><tbody id="t-world"></tbody></table></section>
<section><h2>Players</h2><table><tbody id="t-players"></tbody></table></section>
<section><h2>Tick latency</h2><table>
<thead><tr><th>stage</th><th>samples</th><th>p50</th><th>p95</th><th>p99</th><th>max</th></tr></thead>
<tbody id="t-tick"></tbody></table></section>
<section><h2>Memory</h2><table><tbody id="t-memory"></tbody></table></section>
<section><h2>Autoscale</h2><table><tbody id="t-autoscale"></tbody></table></section>
<section><h2>Chunks</h2><table><tbody id="t-chunks"></tbody></table></section>
<section><h2>Entities</h2><table>
<thead><tr><th>category</th><th>count</th></tr></thead>
<tbody id="t-entities"></tbody></table></section>
<section><h2>Spawning — friendly</h2><table><tbody id="t-spawn-friendly"></tbody></table></section>
<section><h2>Spawning — hostile</h2><table><tbody id="t-spawn-hostile"></tbody></table></section>
<section><h2>Save</h2><table><tbody id="t-save"></tbody></table><ul id="save-errors"></ul></section>
<section><h2>Network</h2><table><tbody id="t-network"></tbody></table></section>
<section><h2>Plugins</h2><table><tbody id="t-plugins"></tbody></table></section>
<section><h2>Warnings</h2><ul id="warnings"></ul></section>
</main>
<script>
"use strict";
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g,
  (c) => ({"&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"}[c]));
const rows = (pairs) => pairs.map(([k, v]) => `<tr><th>${k}</th><td>${v}</td></tr>`).join("");
const fmtBytes = (b) => b >= 1048576 ? (b / 1048576).toFixed(1) + " MiB"
  : b >= 1024 ? (b / 1024).toFixed(1) + " KiB" : b + " B";
const fmtUptime = (s) => {
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m ${s % 60}s`;
};
const fmtUs = (us) => us >= 1000 ? (us / 1000).toFixed(2) + " ms" : us + " µs";
const yesNo = (b) => b ? '<span class="ok">yes</span>' : "no";
const latencyRow = (name, l) =>
  `<tr><th>${esc(name)}</th><td>${l.samples}</td><td>${fmtUs(l.p50_us)}</td>` +
  `<td>${fmtUs(l.p95_us)}</td><td>${fmtUs(l.p99_us)}</td><td>${fmtUs(l.max_us)}</td></tr>`;
const spawnRows = (r) => rows([
  ["attempts", r.attempts], ["chunks sampled", r.chunks_sampled],
  ["templates considered", r.templates_considered], ["committed", r.committed],
  ["rejected unloaded", r.rejected_unloaded], ["rejected time", r.rejected_time],
  ["rejected player distance", r.rejected_player_distance],
  ["rejected block/fluid", r.rejected_block_or_fluid],
  ["rejected darkness", r.rejected_darkness], ["rejected collision", r.rejected_collision],
  ["rejected duplicate", r.rejected_duplicate]]);

function render(p) {
  $("summary").textContent =
    `${p.version} · up ${fmtUptime(p.uptime_secs)} · ${p.players.count}/${p.players.max} players`;
  $("t-server").innerHTML = rows([["version", esc(p.version)],
    ["uptime", fmtUptime(p.uptime_secs)], ["tps", p.tps.toFixed(2)]]);
  const w = p.world;
  $("t-world").innerHTML = rows([["name", esc(w.name)], ["motd", esc(w.motd)],
    ["seed", w.seed], ["mode", esc(w.mode)], ["online mode", yesNo(w.online_mode)],
    ["view distance", w.view_distance], ["simulation distance", w.simulation_distance],
    ["max players", w.max_players]]);
  $("t-players").innerHTML = rows([["online", `${p.players.count} / ${p.players.max}`],
    ["names", p.players.names.length ? esc(p.players.names.join(", ")) : "—"]]);
  $("t-tick").innerHTML = latencyRow("total", p.tick.total) +
    Object.entries(p.tick.stages).map(([name, l]) => latencyRow(name, l)).join("");
  $("t-memory").innerHTML = rows([["used", `${p.memory.used_mb} MiB`],
    ["limit", `${p.memory.limit_mb} MiB`], ["available", `${p.memory.available_mb} MiB`]]);
  const a = p.autoscale;
  $("t-autoscale").innerHTML = rows([["enabled", yesNo(a.enabled)],
    ["profile", esc(a.profile)], ["view distance", a.view_distance],
    ["chunk send rate", a.chunk_send_rate], ["chunk load rate", a.chunk_load_rate],
    ["chunk generate rate", a.chunk_generate_rate], ["scale-ups", a.scale_up_decisions],
    ["scale-downs", a.scale_down_decisions],
    ["draining", a.draining ? '<span class="bad">yes</span>' : "no"]]);
  const c = p.chunks;
  $("t-chunks").innerHTML = rows([["ticketed", c.ticketed], ["prepared", c.prepared],
    ["loaded (total)", c.loaded_total], ["generated (total)", c.generated_total],
    ["streamed (total)", c.streamed_total]]);
  $("t-entities").innerHTML = `<tr><th>total</th><td>${p.entities.total}</td></tr>` +
    Object.entries(p.entities.categories)
      .map(([name, n]) => `<tr><th>${esc(name)}</th><td>${n}</td></tr>`).join("");
  $("t-spawn-friendly").innerHTML = spawnRows(p.spawn.friendly);
  $("t-spawn-hostile").innerHTML = spawnRows(p.spawn.hostile);
  $("t-save").innerHTML = p.save ? rows([["age", fmtUptime(p.save.age_secs)],
    ["players saved", p.save.players_saved], ["entities saved", p.save.entities_saved],
    ["chunks flushed", p.save.chunks_flushed],
    ["world metadata", yesNo(p.save.world_metadata_saved)],
    ["elapsed", `${p.save.elapsed_ms} ms`]])
    : rows([["status", "no completed save yet"]]);
  $("save-errors").innerHTML =
    (p.save ? p.save.errors : []).map((e) => `<li>${esc(e)}</li>`).join("");
  const n = p.network;
  $("t-network").innerHTML = rows([["bytes written", fmtBytes(n.bytes_written)],
    ["reliable drops", n.reliable_drops], ["reliable retries", n.reliable_retries],
    ["slow client sheds", n.slow_client_sheds],
    ["animation drops", n.best_effort_animation_drops]]);
  $("t-plugins").innerHTML = rows([["loaded",
    p.plugins.loaded.length ? esc(p.plugins.loaded.join(", ")) : "—"]]) +
    p.plugins.disabled.map((d) =>
      `<tr><th>${esc(d.plugin)}</th>` +
      `<td class="bad">disabled at ${esc(d.stage)}: ${esc(d.message)}</td></tr>`).join("");
  $("warnings").innerHTML = p.warnings.length
    ? p.warnings.map((w) => `<li>${esc(w)}</li>`).join("")
    : '<li class="ok">none</li>';
}

async function poll() {
  try {
    const res = await fetch("/stats", { headers: { Accept: "application/json" } });
    if (!res.ok) throw new Error(`status ${res.status}`);
    render(await res.json());
    $("error").classList.add("hidden");
  } catch (e) {
    $("error").textContent = `Dashboard connection error: ${e} — retrying every 2 s`;
    $("error").classList.remove("hidden");
  }
}
poll();
setInterval(poll, 2000);
</script>
</body>
</html>
"#;
