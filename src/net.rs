use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const BASE: &str = "https://speed.cloudflare.com";
const PING_PROBES: usize = 12;
const DOWNLOAD_STREAMS: usize = 4;
const UPLOAD_STREAMS: usize = 3;

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
    for _ in 0..PING_PROBES {
        if is_cancelled(&cancel) {
            break;
        }
        let start = Instant::now();
        let result = client
            .get(format!("{BASE}/__down?bytes=0"))
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        let rtt = start.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(resp) if resp.status().is_success() => {
                rtts.push(rtt);
                let _ = tx.send(TestEvent::PingSample(rtt));
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    anyhow::ensure!(!rtts.is_empty(), "latency probe failed");
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
    counter: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) {
    let chunk_url = format!("{BASE}/__down?bytes=25000000");
    while Instant::now() < deadline && !is_cancelled(&cancel) {
        let resp = match client.get(&chunk_url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
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
    const CHUNK: usize = 262_144;
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

fn run_sampler(
    counter: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    started: Instant,
    tx: UnboundedSender<TestEvent>,
) {
    let mut last_bytes = 0u64;
    let mut last_instant = Instant::now();
    while Instant::now() < deadline && !is_cancelled(&cancel) {
        std::thread::sleep(Duration::from_millis(200));
        let now = Instant::now();
        let bytes = counter.load(Ordering::Relaxed);
        let dt = now.duration_since(last_instant).as_secs_f64();
        if dt > 0.0 {
            let instant_mbps = ((bytes - last_bytes) as f64 * 8.0) / (dt * 1_000_000.0);
            let avg_mbps = (bytes as f64 * 8.0)
                / (now.duration_since(started).as_secs_f64() * 1_000_000.0);
            let _ = tx.send(TestEvent::Throughput {
                instant_mbps,
                avg_mbps,
            });
        }
        last_bytes = bytes;
        last_instant = now;
    }
}

pub async fn measure_download(
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
            counter.clone(),
            cancel.clone(),
            deadline,
        )));
    }

    let sampler_tx = tx.clone();
    let sampler_counter = counter.clone();
    let sampler = tokio::task::spawn_blocking(move || {
        run_sampler(sampler_counter, cancel, deadline, started, sampler_tx)
    });

    for h in handles {
        let _ = h.await;
    }
    let _ = sampler.abort();

    let elapsed = started.elapsed().as_secs_f64();
    anyhow::ensure!(elapsed > 0.2, "download interrupted");
    let mbps = counter.load(Ordering::Relaxed) as f64 * 8.0 / (elapsed * 1_000_000.0);
    let _ = tx.send(TestEvent::PhaseDone);
    Ok(mbps)
}

pub async fn measure_upload(
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
    anyhow::ensure!(elapsed > 0.2, "upload interrupted");
    let mbps = counter.load(Ordering::Relaxed) as f64 * 8.0 / (elapsed * 1_000_000.0);
    let _ = tx.send(TestEvent::PhaseDone);
    Ok(mbps)
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
        let _ = tx.send(TestEvent::Failed(format!("connection check failed: {e}")));
        return;
    }

    let _ = tx.send(TestEvent::Phase(Phase::Latency));
    let latency = match measure_latency(&client, cancel.clone(), tx.clone()).await {
        Ok(l) => l,
        Err(e) => {
            let _ = tx.send(TestEvent::Failed(e.to_string()));
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
            let _ = tx.send(TestEvent::Failed(e.to_string()));
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
            let _ = tx.send(TestEvent::Failed(e.to_string()));
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

    #[tokio::test]
    async fn connection_info_and_download_work() {
        let client = build_client().unwrap();
        let info = fetch_connection_info(&client).await.expect("connection info");
        assert!(!info.client_ip.is_empty());

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mbps = measure_download(&client, 2, cancel, tx.clone()).await.unwrap();
        assert!(mbps > 0.0, "download measured {mbps} Mbps");
        let mut got_sample = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, TestEvent::Throughput { .. }) {
                got_sample = true;
            }
        }
        assert!(got_sample, "throughput samples were emitted");
    }
}