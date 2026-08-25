use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const BASE: &str = "https://speed.cloudflare.com";
const PING_PROBES: usize = 12;
const DOWNLOAD_STREAMS: usize = 4;
const UPLOAD_STREAMS: usize = 3;
pub const MONITOR_PROBE_TIMEOUT_MS: u64 = 2000;

/// Bulk-download endpoints tried in order. Some networks/firewalls block
/// Cloudflare's `__down` specifically (while `__up` works), so several
/// independent mirrors are available. The first source that yields data is
/// remembered and used first from then on.
const DOWNLOAD_SOURCES: [&str; 4] = [
    "https://speed.cloudflare.com/__down?bytes=25000000",
    "https://cachefly.cachefly.net/100mb.test",
    "https://proof.ovh.net/files/100Mb.dat",
    "https://speed.hetzner.de/100MB.bin",
];
static PREFERRED_SOURCE: AtomicUsize = AtomicUsize::new(0);

/// Tiny endpoints for latency probes, same fallback idea. `/cdn-cgi/trace`
/// responds even where `/__down` is blocked.
const PROBE_PATHS: [&str; 2] = ["/__down?bytes=0", "/cdn-cgi/trace"];
static PREFERRED_PROBE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConnectionInfo {
    #[serde(rename = "clientIp")]
    pub client_ip: String,
    #[serde(rename = "asOrganization")]
    pub as_organization: String,
    pub asn: Option<i64>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub colo: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Latency,
    Download,
    Upload,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Connect => "Connecting",
            Phase::Latency => "Measuring latency",
            Phase::Download => "Download test",
            Phase::Upload => "Upload test",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LatencyStats {
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub jitter_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Metrics {
    pub down_mbps: f64,
    pub up_mbps: f64,
    pub latency: LatencyStats,
}

pub enum TestEvent {
    Phase(Phase),
    PingSample(f64),
    LatencyDone(LatencyStats),
    Throughput { instant_mbps: f64, avg_mbps: f64 },
    PhaseDone,
    Finished(Metrics),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Quick,
    Standard,
    Max,
}

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Quick, Profile::Standard, Profile::Max];

    pub fn name(self) -> &'static str {
        match self {
            Profile::Quick => "Quick",
            Profile::Standard => "Standard",
            Profile::Max => "Maximum",
        }
    }

    pub fn phase_seconds(self) -> (u64, u64) {
        match self {
            Profile::Quick => (5, 5),
            Profile::Standard => (10, 10),
            Profile::Max => (20, 20),
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
}

fn is_cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 speed-test/0.1";

pub fn build_client() -> Result<Client> {
    Ok(Client::builder().user_agent(USER_AGENT).build()?)
}

async fn try_meta(client: &Client) -> Result<ConnectionInfo> {
    let info: ConnectionInfo = client
        .get(format!("{BASE}/meta"))
        .timeout(Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(info)
}

#[derive(Debug, Deserialize)]
struct IpWhoIs {
    ip: String,
    city: Option<String>,
    country: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    success: bool,
    #[serde(default)]
    connection: Option<IpWhoIsConnection>,
}

#[derive(Debug, Deserialize)]
struct IpWhoIsConnection {
    asn: Option<i64>,
    org: Option<String>,
    isp: Option<String>,
}

async fn try_trace_lookup(client: &Client) -> Result<ConnectionInfo> {
    let trace = client
        .get(format!("{BASE}/cdn-cgi/trace"))
        .timeout(Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let mut cf_ip = None;
    let mut colo = None;
    let mut loc = None;
    for line in trace.lines() {
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("").trim().to_string();
        match key {
            "ip" => cf_ip = Some(value),
            "colo" => colo = Some(value),
            "loc" => loc = Some(value),
            _ => {}
        }
    }

    let geo: Option<IpWhoIs> = match client
        .get("https://ipwho.is/")
        .timeout(Duration::from_secs(8))
        .send()
        .await
    {
        Ok(resp) => resp.json::<IpWhoIs>().await.ok(),
        Err(_) => None,
    };

    let org = geo
        .as_ref()
        .and_then(|g| g.connection.as_ref())
        .map(|c| {
            c.org
                .clone()
                .or_else(|| c.isp.clone())
                .unwrap_or_else(|| "unknown".into())
        })
        .unwrap_or_else(|| "unknown".into());
    let asn = geo.as_ref().and_then(|g| g.connection.as_ref()).and_then(|c| c.asn);
    let ip = cf_ip.or_else(|| geo.as_ref().map(|g| g.ip.clone()));

    let country = geo
        .as_ref()
        .and_then(|g| g.country.clone())
        .filter(|c| !c.is_empty())
        .or(loc);

    anyhow::ensure!(ip.is_some(), "could not determine connection info");

    Ok(ConnectionInfo {
        client_ip: ip.unwrap(),
        as_organization: org,
        asn,
        city: geo.as_ref().and_then(|g| g.city.clone()),
        country,
        colo,
    })
}

pub async fn fetch_connection_info(client: &Client) -> Result<ConnectionInfo> {
    match try_meta(client).await {
        Ok(info) => Ok(info),
        Err(_) => try_trace_lookup(client).await,
    }
}

pub async fn measure_latency(
    client: &Client,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) -> Result<LatencyStats> {
    let mut rtts: Vec<f64> = Vec::with_capacity(PING_PROBES);
    let mut probe_idx = PREFERRED_PROBE.load(Ordering::Relaxed);

    for _ in 0..PING_PROBES {
        if is_cancelled(&cancel) {
            break;
        }
        let url = format!("{BASE}{}", PROBE_PATHS[probe_idx]);
        let start = Instant::now();
        let result = client.get(&url).timeout(Duration::from_secs(5)).send().await;
        let rtt = start.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(resp) if resp.status().is_success() => {
                rtts.push(rtt);
                let _ = tx.send(TestEvent::PingSample(rtt));
                PREFERRED_PROBE.store(probe_idx, Ordering::Relaxed);
            }
            _ => {
                // This endpoint is blocked on the current network — switch
                // to the alternative for the remaining probes.
                probe_idx = (probe_idx + 1) % PROBE_PATHS.len();
            }
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    anyhow::ensure!(
        !rtts.is_empty(),
        "no latency probe endpoint responded — the network may be blocking the test"
    );
    let min = rtts.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = rtts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = rtts.iter().sum::<f64>() / rtts.len() as f64;
    let jitter = if rtts.len() > 1 {
        let diffs: Vec<f64> = rtts.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        diffs.iter().sum::<f64>() / diffs.len() as f64
    } else {
        0.0
    };
    let stats = LatencyStats {
        min_ms: min,
        avg_ms: avg,
        max_ms: max,
        jitter_ms: jitter,
    };
    let _ = tx.send(TestEvent::LatencyDone(stats.clone()));
    Ok(stats)
}

async fn download_worker(
    client: Client,
    url: &'static str,
    counter: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) {
    while Instant::now() < deadline && !is_cancelled(&cancel) {
        let resp = match client.get(url).send().await {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !resp.status().is_success() {
            continue;
        }
        let mut stream = resp.bytes_stream();
        while let Some(item) = stream.next().await {
            if Instant::now() >= deadline || is_cancelled(&cancel) {
                return;
            }
            if let Ok(chunk) = item {
                counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
        }
    }
}

async fn upload_worker(
    client: Client,
    counter: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) {
    // Small chunks keep the byte counter granular: large chunks make the
    // counter jump in bursts whenever the socket drains its buffers, which
    // used to render as a stuttering throughput graph.
    const CHUNK: usize = 65_536;
    loop {
        if Instant::now() >= deadline || is_cancelled(&cancel) {
            break;
        }
        let counter = counter.clone();
        let cancel = cancel.clone();
        let stream = futures_util::stream::unfold(0usize, move |sent| {
            let counter = counter.clone();
            let cancel = cancel.clone();
            async move {
                if Instant::now() >= deadline || is_cancelled(&cancel) || sent >= 512 * 1024 * 1024
                {
                    None
                } else {
                    counter.fetch_add(CHUNK as u64, Ordering::Relaxed);
                    Some((Ok::<_, std::convert::Infallible>(vec![0u8; CHUNK]), sent + CHUNK))
                }
            }
        });
        let body = reqwest::Body::wrap_stream(stream);
        let _ = client
            .post(format!("{BASE}/__up"))
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .timeout(Duration::from_secs(60))
            .send()
            .await;
    }
}

/// How often throughput is sampled and pushed to the UI. 100 ms keeps the
/// live graph feeling fluid at the app's ~30 fps redraw rate.
const SAMPLE_INTERVAL_MS: u64 = 100;
/// Exponential-moving-average weight for new samples. Raw per-interval rates
/// from socket buffers are bursty; smoothing turns them into a steady curve
/// without hiding real spikes for long.
const EMA_ALPHA: f64 = 0.4;

fn run_sampler(
    counter: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    started: Instant,
    tx: UnboundedSender<TestEvent>,
) {
    let mut last_bytes = 0u64;
    let mut last_instant = Instant::now();
    let mut ema: Option<f64> = None;

    while Instant::now() < deadline && !is_cancelled(&cancel) {
        std::thread::sleep(Duration::from_millis(SAMPLE_INTERVAL_MS));
        let now = Instant::now();
        let bytes = counter.load(Ordering::Relaxed);
        let dt = now.duration_since(last_instant).as_secs_f64();
        if dt > 0.0 {
            let raw_mbps = ((bytes - last_bytes) as f64 * 8.0) / (dt * 1_000_000.0);
            let avg_mbps = (bytes as f64 * 8.0)
                / (now.duration_since(started).as_secs_f64() * 1_000_000.0);

            let smoothed = match ema {
                Some(prev) => prev * (1.0 - EMA_ALPHA) + raw_mbps.max(0.0) * EMA_ALPHA,
                None => raw_mbps.max(0.0),
            };
            ema = Some(smoothed);

            let _ = tx.send(TestEvent::Throughput {
                instant_mbps: smoothed,
                avg_mbps,
            });
        }
        last_bytes = bytes;
        last_instant = now;
    }
}

/// Marker error: a phase completed without transferring any bytes. Treated
/// as retryable — transient blocks/handshake failures often clear up at once.
#[derive(Debug)]
struct ZeroData;
impl std::fmt::Display for ZeroData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "every download source was blocked or returned no data — the network or firewall may be blocking the test"
        )
    }
}
impl std::error::Error for ZeroData {}

fn is_zero_data(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<ZeroData>().is_some())
}



const PHASE_RETRIES: usize = 2;
const RETRY_DELAYS_MS: [u64; PHASE_RETRIES] = [400, 1500];

/// Run one throughput phase, retrying when it produced no data at all.
async fn with_zero_data_retry<F, Fut>(mut run: F) -> Result<f64>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<f64>>,
{
    let mut attempt = 0;
    loop {
        match run().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if attempt > PHASE_RETRIES || !is_zero_data(&e) {
                    return Err(e);
                }
                let delay = RETRY_DELAYS_MS[attempt - 1.min(RETRY_DELAYS_MS.len() - 1)];
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

pub async fn measure_download(
    client: &Client,
    seconds: u64,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) -> Result<f64> {
    with_zero_data_retry(|| measure_download_once(client, seconds, cancel.clone(), tx.clone()))
        .await
}

async fn measure_download_once(
    client: &Client,
    seconds: u64,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) -> Result<f64> {
    // Try every known source, starting with the last one that worked.
    let start = PREFERRED_SOURCE.load(Ordering::Relaxed);
    let mut last_err: Option<anyhow::Error> = None;

    for offset in 0..DOWNLOAD_SOURCES.len() {
        if is_cancelled(&cancel) {
            anyhow::bail!("cancelled");
        }
        let idx = (start + offset) % DOWNLOAD_SOURCES.len();
        match download_from(DOWNLOAD_SOURCES[idx], client, seconds, cancel.clone(), tx.clone())
            .await
        {
            Ok(mbps) => {
                PREFERRED_SOURCE.store(idx, Ordering::Relaxed);
                return Ok(mbps);
            }
            Err(e) => {
                if is_cancelled(&cancel) {
                    return Err(e);
                }
                // Keep the first real failure for reporting.
                last_err.get_or_insert(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| ZeroData.into()))
}

async fn download_from(
    url: &'static str,
    client: &Client,
    seconds: u64,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) -> Result<f64> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let counter = Arc::new(AtomicU64::new(0));
    let started = Instant::now();

    let mut handles = Vec::new();
    for _ in 0..DOWNLOAD_STREAMS {
        handles.push(tokio::spawn(download_worker(
            client.clone(),
            url,
            counter.clone(),
            cancel.clone(),
            deadline,
        )));
    }

    let sampler_tx = tx.clone();
    let sampler_counter = counter.clone();
    let sampler_cancel = cancel.clone();
    let sampler = tokio::task::spawn_blocking(move || {
        run_sampler(sampler_counter, sampler_cancel, deadline, started, sampler_tx)
    });

    for h in handles {
        let _ = h.await;
    }
    let _ = sampler.abort();

    if is_cancelled(&cancel) {
        anyhow::bail!("phase was interrupted before it could run");
    }
    let elapsed = started.elapsed().as_secs_f64();
    anyhow::ensure!(
        elapsed > 0.2,
        "phase was interrupted before it could run"
    );
    let total_bytes = counter.load(Ordering::Relaxed);
    if total_bytes == 0 {
        return Err(ZeroData.into());
    }
    let mbps = total_bytes as f64 * 8.0 / (elapsed * 1_000_000.0);
    let _ = tx.send(TestEvent::PhaseDone);
    Ok(mbps)
}

pub async fn measure_upload(
    client: &Client,
    seconds: u64,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) -> Result<f64> {
    with_zero_data_retry(|| measure_upload_once(client, seconds, cancel.clone(), tx.clone())).await
}

async fn measure_upload_once(
    client: &Client,
    seconds: u64,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) -> Result<f64> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let counter = Arc::new(AtomicU64::new(0));
    let started = Instant::now();

    let mut handles = Vec::new();
    for _ in 0..UPLOAD_STREAMS {
        handles.push(tokio::spawn(upload_worker(
            client.clone(),
            counter.clone(),
            cancel.clone(),
            deadline,
        )));
    }

    let sampler_tx = tx.clone();
    let sampler_cancel = cancel.clone();
    let sampler = {
        let counter = counter.clone();
        tokio::task::spawn_blocking(move || {
            run_sampler(counter, sampler_cancel, deadline, started, sampler_tx)
        })
    };

    for h in handles {
        let _ = h.await;
    }
    let _ = sampler.abort();

    let elapsed = started.elapsed().as_secs_f64();
    anyhow::ensure!(
        elapsed > 0.2,
        "phase was interrupted before it could run"
    );
    let total_bytes = counter.load(Ordering::Relaxed);
    if total_bytes == 0 {
        return Err(ZeroData.into());
    }
    let mbps = total_bytes as f64 * 8.0 / (elapsed * 1_000_000.0);
    let _ = tx.send(TestEvent::PhaseDone);
    Ok(mbps)
}

// ---------- Continuous ping monitor ----------

#[derive(Debug, Clone, Copy)]
pub struct ProbeSample {
    /// Round-trip time in ms, or `None` if the probe was lost.
    pub rtt_ms: Option<f64>,
}

pub enum MonitorEvent {
    Sample(ProbeSample),
}

fn normalize_target(target: &str) -> String {
    let t = target
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    if t.is_empty() {
        "speed.cloudflare.com".to_string()
    } else {
        t
    }
}

/// One tiny latency probe. Uses a 0-byte Cloudflare download for its own edge
/// (a few hundred bytes on the wire) and a bare HEAD request elsewhere.
/// Transport errors / timeouts count as packet loss; any HTTP response counts
/// as a successful round trip regardless of status code.
async fn ping_once(client: &Client, target: &str) -> Option<f64> {
    let host = normalize_target(target);
    let url = if host == "speed.cloudflare.com" {
        format!("{BASE}/__down?bytes=0")
    } else {
        format!("https://{host}/")
    };
    let request = if host == "speed.cloudflare.com" {
        client.get(&url)
    } else {
        client.head(&url)
    };
    let start = Instant::now();
    match request
        .timeout(Duration::from_millis(MONITOR_PROBE_TIMEOUT_MS))
        .send()
        .await
    {
        Ok(_) => Some(start.elapsed().as_secs_f64() * 1000.0),
        Err(_) => None,
    }
}

/// Long-running monitor loop. Emits one [`MonitorEvent::Sample`] per probe and
/// stops when `cancel` is set. The interval can be tuned live through
/// `interval_ms` (read before every probe).
pub async fn run_monitor(
    client: Client,
    target: String,
    interval_ms: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<MonitorEvent>,
) {
    while !is_cancelled(&cancel) {
        let started = Instant::now();
        let rtt = ping_once(&client, &target).await;
        if is_cancelled(&cancel) {
            break;
        }
        let _ = tx.send(MonitorEvent::Sample(ProbeSample { rtt_ms: rtt }));
        let interval = Duration::from_millis(interval_ms.load(Ordering::Relaxed).max(100));
        let elapsed = started.elapsed();
        if elapsed < interval {
            tokio::time::sleep(interval - elapsed).await;
        }
    }
}

/// Translate a low-level network error into a short, human-friendly message.
pub fn describe_network_error(err: &anyhow::Error) -> String {
    let mut source: Option<&dyn std::error::Error> = Some(err.as_ref());
    while let Some(e) = source {
        if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
            if reqwest_err.is_connect() {
                return "could not connect — check your internet connection or DNS".into();
            }
            if reqwest_err.is_timeout() {
                return "the server did not respond in time".into();
            }
            if reqwest_err.is_decode() {
                return "received a malformed response from the server".into();
            }
            return format!("network error: {reqwest_err}");
        }
        source = e.source();
    }
    err.to_string()
}

pub async fn run_full_test(
    profile: Profile,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<TestEvent>,
) {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(TestEvent::Failed(e.to_string()));
            return;
        }
    };

    let _ = tx.send(TestEvent::Phase(Phase::Connect));
    if let Err(e) = fetch_connection_info(&client).await {
        if !is_cancelled(&cancel) {
            let _ = tx.send(TestEvent::Failed(format!(
                "connection check failed: {}",
                describe_network_error(&e)
            )));
        }
        return;
    }

    let _ = tx.send(TestEvent::Phase(Phase::Latency));
    let latency = match measure_latency(&client, cancel.clone(), tx.clone()).await {
        Ok(l) => l,
        Err(e) => {
            if !is_cancelled(&cancel) {
                let _ = tx.send(TestEvent::Failed(format!(
                    "latency test failed: {}",
                    describe_network_error(&e)
                )));
            }
            return;
        }
    };
    if is_cancelled(&cancel) {
        return;
    }

    let (down_s, up_s) = profile.phase_seconds();

    let _ = tx.send(TestEvent::Phase(Phase::Download));
    let down = match measure_download(&client, down_s, cancel.clone(), tx.clone()).await {
        Ok(d) => d,
        Err(e) => {
            if !is_cancelled(&cancel) {
                let _ = tx.send(TestEvent::Failed(format!(
                    "download test failed: {}",
                    describe_network_error(&e)
                )));
            }
            return;
        }
    };
    if is_cancelled(&cancel) {
        return;
    }

    let _ = tx.send(TestEvent::Phase(Phase::Upload));
    let up = match measure_upload(&client, up_s, cancel.clone(), tx.clone()).await {
        Ok(u) => u,
        Err(e) => {
            if !is_cancelled(&cancel) {
                let _ = tx.send(TestEvent::Failed(format!(
                    "upload test failed: {}",
                    describe_network_error(&e)
                )));
            }
            return;
        }
    };
    if is_cancelled(&cancel) {
        return;
    }

    let _ = tx.send(TestEvent::Finished(Metrics {
        down_mbps: down,
        up_mbps: up,
        latency,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_env_blocked(err: &anyhow::Error) -> bool {
        is_zero_data(err)
    }

    /// Diagnostic: follow every event the engine emits and print timings, so
    /// a silent hang shows exactly where it stops. Run with --nocapture.
    #[tokio::test]
    async fn trace_engine_events() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(run_full_test(Profile::Quick, cancel, tx));
        let start = Instant::now();
        loop {
            let ev = match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
                Ok(Some(ev)) => ev,
                Ok(None) => {
                    println!("[{:.2}s] channel closed", start.elapsed().as_secs_f64());
                    break;
                }
                Err(_) => {
                    println!(
                        "[{:.2}s] NO EVENT FOR 30s — engine is stuck here",
                        start.elapsed().as_secs_f64()
                    );
                    break;
                }
            };
            let label = match &ev {
                TestEvent::Phase(p) => format!("Phase {p:?}"),
                TestEvent::PingSample(ms) => format!("Ping {:.1} ms", ms),
                TestEvent::LatencyDone(s) => format!("LatencyDone avg {:.1}", s.avg_ms),
                TestEvent::Throughput { instant_mbps, .. } => {
                    format!("Throughput {:.1} Mbps", instant_mbps)
                }
                TestEvent::PhaseDone => "PhaseDone".into(),
                TestEvent::Finished(m) => format!(
                    "FINISHED down {:.1} up {:.1}",
                    m.down_mbps, m.up_mbps
                ),
                TestEvent::Failed(e) => format!("FAILED: {e}"),
            };
            println!("[{:.2}s] {}", start.elapsed().as_secs_f64(), label);
            if matches!(ev, TestEvent::Finished(_) | TestEvent::Failed(_)) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn connection_info_and_download_work() {
        let client = build_client().unwrap();
        let info = fetch_connection_info(&client).await.expect("connection info");
        assert!(!info.client_ip.is_empty());

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mbps = match measure_download(&client, 2, cancel, tx.clone()).await {
            Ok(v) => v,
            Err(e) => {
                if is_env_blocked(&e) {
                    eprintln!("SKIPPED download assertion: environment blocks test endpoints");
                    return;
                }
                panic!("download failed: {e}");
            }
        };
        assert!(mbps > 0.0, "download measured {mbps} Mbps");
        let mut got_sample = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, TestEvent::Throughput { .. }) {
                got_sample = true;
            }
        }
        assert!(got_sample, "throughput samples were emitted");
    }

    #[tokio::test]
    async fn ping_probe_measures_rtt_and_loss() {
        let client = build_client().unwrap();
        let rtt = ping_once(&client, "speed.cloudflare.com").await;
        assert!(rtt.is_some_and(|v| v > 0.0), "probe should succeed: {rtt:?}");

        // Unroutable address must be reported as loss, not hang.
        let lost = ping_once(&client, "10.255.255.1").await;
        assert!(lost.is_none(), "unroutable target counts as loss");

        let normalized = normalize_target("https://Example.com/");
        assert_eq!(normalized, "Example.com");
    }
}
