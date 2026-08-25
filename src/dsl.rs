//! DSL modem statistics via the TR-064/SOAP management API.
//!
//! Values like SNR margin, line attenuation and data rates exist only inside
//! the modem itself, so they must be read from its LAN management interface —
//! they can never come over the internet. TR-064 is the closest thing to a
//! standard (FRITZ!Box and many ISP-supplied routers speak it); anything else
//! is reported as "not available" instead of showing fake numbers.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Local};
use reqwest::Client;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(15);
const SOAP_PORT: u16 = 4900;

/// One poll of every configured value.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DslSnapshot {
    pub available: bool,
    /// Why values are missing (shown in the banner).
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    pub fetched_at: Option<DateTime<Local>>,

    // Line status
    pub state: Option<String>,
    pub mode: Option<String>,
    pub uptime_secs: Option<u64>,
    pub firmware: Option<String>,

    // Per-direction metrics (dB / dB / Mbps / Mbps / dBm)
    pub snr_down_db: Option<f64>,
    pub snr_up_db: Option<f64>,
    pub atten_down_db: Option<f64>,
    pub atten_up_db: Option<f64>,
    pub rate_down_mbps: Option<f64>,
    pub rate_up_mbps: Option<f64>,
    pub max_rate_down_mbps: Option<f64>,
    pub max_rate_up_mbps: Option<f64>,
    pub power_down_dbm: Option<f64>,
    pub power_up_dbm: Option<f64>,

    // Counters & extras
    pub crc_errors: Option<u64>,
    pub wireless_clients: Option<u32>,
}

pub enum DslEvent {
    Snapshot(DslSnapshot),
}

// ---- abnormal-value thresholds ------------------------------------------------
pub const SNR_WARN_DB: f64 = 6.0;
pub const SNR_CRIT_DB: f64 = 3.0;
pub const SNR_EXPECTED: &str = ">= 6 dB (critical below 3)";
pub const ATT_WARN_DB: f64 = 49.0;
pub const ATT_CRIT_DB: f64 = 58.0;
pub const ATT_EXPECTED: &str = "< 45 dB typical for DSL";
pub const RATE_MIN_FRAC: f64 = 0.5;
pub const RATE_EXPECTED: &str = ">= 50% of the max rate";

/// A detected abnormality or notable event on the modem side.
#[derive(Debug, Clone)]
pub struct ModemIncident {
    pub at: DateTime<Local>,
    pub severity: Severity,
    pub field: &'static str,
    /// What was observed, human-formatted.
    pub observed: String,
    /// The normal/expected range for this value.
    pub expected: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Warning => "WARN",
            Severity::Critical => "CRIT",
        }
    }

    pub fn color(self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            Severity::Info => Color::Green,
            Severity::Warning => Color::Yellow,
            Severity::Critical => Color::Red,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub struct ModemConfig {
    pub host: String,
    pub username: String,
    pub password: String,
}

impl Default for ModemConfig {
    fn default() -> Self {
        Self {
            host: "fritz.box".to_string(),
            username: String::new(),
            password: String::new(),
        }
    }
}

/// Long-running poller: fetches a snapshot every couple of seconds while the
/// app runs, emitting [`DslEvent::Snapshot`]s through `tx`.
pub async fn run_modem_poller(
    mut config: ModemConfig,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<DslEvent>,
) {
    // Direct connection to the LAN device: a VPN client often installs a
    // system or environment proxy, and routing the modem request through it
    // breaks access even though the browser reaches the panel just fine.
    let client = match Client::builder()
        .user_agent("speed-test/0.1")
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    // Candidate hosts: whatever the user set first, then the detected
    // default gateway and the usual defaults. The first host that answers
    // TR-064 is remembered in `working_host` for all later polls.
    let mut candidates: Vec<String> = vec![normalize_host(&config.host)];
    if let Some(gw) = detect_default_gateway() {
        if !candidates.contains(&gw) {
            candidates.push(gw);
        }
    }
    for fallback in ["192.168.1.1", "192.168.0.1", "fritz.box"] {
        let fb = fallback.to_string();
        if !candidates.contains(&fb) {
            candidates.push(fb);
        }
    }

    let mut working_host: Option<String> = None;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let started = Instant::now();

        let snapshot = match &working_host {
            Some(host) => {
                config.host = host.clone();
                fetch_snapshot(&client, &config).await
            }
            None => {
                // Try each candidate; remember the winner.
                let mut result = None;
                let mut tried: Vec<String> = Vec::new();
                let mut reasons: Vec<String> = Vec::new();
                for candidate in &candidates.clone() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    config.host = candidate.clone();
                    let snap = fetch_snapshot(&client, &config).await;
                    if snap.available {
                        working_host = Some(candidate.clone());
                        result = Some(snap);
                        break;
                    }
                    tried.push(candidate.clone());
                    if let Some(r) = snap.unavailable_reason {
                        if !reasons.contains(&r) {
                            reasons.push(r);
                        }
                    }
                }
                match result {
                    Some(s) => s,
                    None => DslSnapshot {
                        available: false,
                        fetched_at: Some(Local::now()),
                        unavailable_reason: Some(format!(
                            "tried {} - {}",
                            tried.join(", "),
                            reasons.last().cloned().unwrap_or_else(|| "no answer".into())
                        )),
                        ..Default::default()
                    },
                }
            }
        };

        let ok = snapshot.available;
        if tx.send(DslEvent::Snapshot(snapshot)).is_err() {
            return;
        }

        // Failures retry less aggressively; sleeps stay cancellable.
        let sleep_for = if ok { POLL_INTERVAL } else { RETRY_AFTER_FAILURE };
        let mut remaining = sleep_for.saturating_sub(started.elapsed());
        while remaining > Duration::ZERO && !cancel.load(Ordering::Relaxed) {
            let slice = remaining.min(Duration::from_millis(100));
            tokio::time::sleep(slice).await;
            remaining = remaining.saturating_sub(slice);
        }
    }
}

/// Best-effort default gateway detection (the router that owns the line).
#[cfg(windows)]
fn detect_default_gateway() -> Option<String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | \
             Sort-Object RouteMetric | Select-Object -First 1).NextHop",
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() || value.starts_with("0.") || !value.parse::<std::net::Ipv4Addr>().is_ok()
    {
        return None;
    }
    Some(value)
}

#[cfg(not(windows))]
fn detect_default_gateway() -> Option<String> {
    // Parse /proc/net/route: little-endian hex gateway for the default route.
    let data = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in data.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() > 3 && cols[1] == "00000000" {
            let gw = cols[2];
            let ip = std::net::Ipv4Addr::new(
                u8::from_str_radix(&gw[6..8], 16).ok()?,
                u8::from_str_radix(&gw[4..6], 16).ok()?,
                u8::from_str_radix(&gw[2..4], 16).ok()?,
                u8::from_str_radix(&gw[0..2], 16).ok()?,
            );
            if !ip.is_unspecified() {
                return Some(ip.to_string());
            }
        }
    }
    None
}

#[cfg(windows)]
use std::os::windows::process::CommandExt as _CommandExt;

/// Turn a transport error into a short reason without leaking long URLs.
fn describe_transport_error(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(req) = cause.downcast_ref::<reqwest::Error>() {
            if req.is_timeout() {
                return "no response from the modem (timeout)".into();
            }
            let text = req.to_string().to_lowercase();
            if text.contains("refused") {
                return "nothing listening on the TR-064 port at that address".into();
            }
            if text.contains("dns") || text.contains("resolve") {
                return "address could not be resolved".into();
            }
            if text.contains("timed out") {
                return "connection timed out".into();
            }
            return "could not reach the modem".into();
        }
    }
    "could not reach the modem".into()
}

// ---------------------------------------------------------------------------
// Snapshot fetching
// ---------------------------------------------------------------------------

async fn fetch_snapshot(client: &Client, cfg: &ModemConfig) -> DslSnapshot {
    let mut snap = DslSnapshot {
        fetched_at: Some(Local::now()),
        ..Default::default()
    };

    // Allow an explicit port in the address ("192.168.1.1:4900"); otherwise
    // use the standard TR-064 port.
    let hostport = normalize_host(&cfg.host);
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse::<u16>().unwrap()),
        _ => (hostport.clone(), SOAP_PORT),
    };
    let base = format!("http://{host}:{port}");

    // 1) DSL line info: rates, SNR, attenuation, power.
    match soap_call(
        client,
        &base,
        "urn:dslforum-org:service:WANDSLInterfaceConfig:1",
        "wandslifconfig1",
        "GetInfo",
        "",
        &cfg.username,
        &cfg.password,
    )
    .await
    {
        Ok(xml) => {
            snap.available = true;
            snap.state = xml_val(&xml, "NewStatus");
            snap.mode = xml_val(&xml, "NewDSLMode").or_else(|| xml_val(&xml, "NewModulationType"));

            // TR-064 exposes these as integers; noise margin / attenuation in
            // tenths of a dB, rates in kbit/s.
            snap.max_rate_down_mbps = num_val(&xml, "NewDownstreamMaxRate").map(|v| v / 1000.0);
            snap.max_rate_up_mbps = num_val(&xml, "NewUpstreamMaxRate").map(|v| v / 1000.0);
            snap.rate_down_mbps = num_val(&xml, "NewDownstreamCurrentRate").map(|v| v / 1000.0);
            snap.rate_up_mbps = num_val(&xml, "NewUpstreamCurrentRate").map(|v| v / 1000.0);
            snap.snr_down_db = num_val(&xml, "NewDownstreamNoiseMargin").map(|v| v / 10.0);
            snap.snr_up_db = num_val(&xml, "NewUpstreamNoiseMargin").map(|v| v / 10.0);
            snap.atten_down_db = num_val(&xml, "NewDownstreamAttenuation").map(|v| v / 10.0);
            snap.atten_up_db = num_val(&xml, "NewUpstreamAttenuation").map(|v| v / 10.0);
            snap.power_down_dbm = num_val(&xml, "NewDownstreamPower").map(|v| v / 10.0);
            snap.power_up_dbm = num_val(&xml, "NewUpstreamPower").map(|v| v / 10.0);
        }
        Err(e) => {
            snap.available = false;
            snap.unavailable_reason = Some(format!(
                "no TR-064 answer from {} ({})",
                hostport,
                describe_transport_error(&e)
            ));
            return snap;
        }
    }

    // 2) PPP session uptime + status (reconnect detection needs this).
    if let Ok(xml) = soap_call(
        client,
        &base,
        "urn:dslforum-org:service:WANPPPConnection:1",
        "wanpppconn1",
        "GetInfo",
        "",
        &cfg.username,
        &cfg.password,
    )
    .await
    {
        snap.uptime_secs = num_val(&xml, "NewUptime").map(|v| v as u64);
        if snap.state.is_none() {
            snap.state = xml_val(&xml, "NewConnectionStatus");
        }
    }

    // 3) Wireless client count (standard TR-064 WLAN service).
    if let Ok(xml) = soap_call(
        client,
        &base,
        "urn:dslforum-org:service:WLANConfiguration:1",
        "wlanconfig1",
        "GetTotalAssociations",
        "",
        &cfg.username,
        &cfg.password,
    )
    .await
    {
        snap.wireless_clients =
            num_val(&xml, "NewNumberOfAssociatedDevices").map(|v| v as u32);
    }

    // 4) CRC errors — vendor extension where present; "-" otherwise.
    if let Ok(xml) = soap_call(
        client,
        &base,
        "urn:dslforum-org:service:WANCommonInterfaceConfig:1",
        "wancommonifconfig1",
        "X_AVM-DE_GetOnlineMonitor",
        "<s:NewSyncGroupIndex>0</s:NewSyncGroupIndex>",
        &cfg.username,
        &cfg.password,
    )
    .await
    {
        snap.crc_errors = find_crc_total(&xml);
    }

    snap
}

// ---------------------------------------------------------------------------
// Minimal SOAP + digest-auth client
// ---------------------------------------------------------------------------

async fn soap_call(
    client: &Client,
    base: &str,
    service_type: &str,
    control_path: &str,
    action: &str,
    body_args: &str,
    user: &str,
    pass: &str,
) -> Result<String> {
    let url = format!("{base}/upnp/control/{control_path}");
    let envelope = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{service}\">{args}</u:{action}>\
         </s:Body></s:Envelope>",
        action = action,
        service = service_type,
        args = body_args,
    );
    let soap_action = format!("\"{service}#{action}\"", service = service_type, action = action);

    let mut request = client
        .post(&url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", soap_action.as_str())
        .body(envelope.clone());

    if !user.is_empty() || !pass.is_empty() {
        request = request.basic_auth(user, Some(pass));
    }

    let response = request.send().await?;
    let status = response.status();

    // If the router answered 401 with a Digest challenge and we have
    // credentials, redo the call properly authenticated.
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let auth_header = response
            .headers()
            .get("WWW-Authenticate")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("modem requires authentication"))?;
        let uri_path = url.splitn(4, '/').nth(3).unwrap_or("").to_string();
        let authorization = respond_digest(
            &auth_header,
            user,
            pass,
            "POST",
            &format!("/{}", uri_path),
        )?;
        let retry = client
            .post(&url)
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header("SOAPACTION", soap_action.clone())
            .header("Authorization", authorization)
            .body(envelope)
            .send()
            .await?;
        return Ok(retry.error_for_status()?.text().await?);
    }

    Ok(response.error_for_status()?.text().await?)
}

/// Build a Digest `Authorization` header value from a WWW-Authenticate
/// challenge (MD5 + qop="auth", which is what routers commonly use).
fn respond_digest(
    challenge: &str,
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
) -> Result<String> {
    let original = |key: &str| -> Option<String> {
        let needle = format!("{key}=\"");
        let pos = challenge.find(&needle)?;
        let after = &challenge[pos + needle.len()..];
        Some(after[..after.find('"')?].to_string())
    };
    let realm = original("realm").ok_or_else(|| anyhow!("digest challenge missing realm"))?;
    let nonce = original("nonce").ok_or_else(|| anyhow!("digest challenge missing nonce"))?;
    let opaque = original("opaque");

    let nc = "00000001";
    let cnonce = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ (std::process::id() as u128)
    );

    let ha1 = md5_hex(&format!("{user}:{realm}:{pass}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let qop = "auth";
    let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"));

    let mut out = format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", \
         algorithm=MD5, response=\"{response}\", qop={qop}, nc={nc}, cnonce=\"{cnonce}\""
    );
    if let Some(opaque) = opaque {
        out.push_str(&format!(", opaque=\"{opaque}\""));
    }
    Ok(out)
}

fn md5_hex(input: &str) -> String {
    let digest = md5::compute(input.as_bytes());
    format!("{digest:x}")
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

fn xml_val(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let value = xml[start..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn num_val(xml: &str, tag: &str) -> Option<f64> {
    xml_val(xml, tag)?.trim().parse::<f64>().ok()
}

/// Vendor extensions name CRC counters differently; accept any element whose
/// tag contains "CRC" and sum the numeric contents so at least a total shows.
fn find_crc_total(xml: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut found = false;
    let mut rest = xml;

    while let Some(lt_rel) = rest.find('<') {
        let gt_rel = match rest[lt_rel..].find('>') {
            Some(p) => p,
            None => break,
        };
        let tag_inner = rest[lt_rel + 1..lt_rel + gt_rel].trim();
        // Skip closing tags, declarations and attribute-carrying elements;
        // every opening tag is scanned individually so nested children of
        // non-CRC containers are still examined.
        if tag_inner.starts_with('/')
            || tag_inner.starts_with('?')
            || tag_inner.contains(' ')
        {
            rest = &rest[lt_rel + gt_rel + 1..];
            continue;
        }

        let name_lc = tag_inner.to_lowercase();
        let after_open = &rest[lt_rel + gt_rel + 1..];
        if name_lc.contains("crc") {
            let close_tag = format!("</{}>", tag_inner);
            if let Some(v_end) = after_open.find(&close_tag) {
                if let Ok(v) = after_open[..v_end].trim().parse::<u64>() {
                    total += v;
                    found = true;
                }
            }
        }
        rest = &rest[lt_rel + gt_rel + 1..];
    }

    if found {
        Some(total)
    } else {
        None
    }
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_INFO: &str = concat!(
        "<?xml version=\"1.0\"?>",
        "<s:Envelope><s:Body><u:GetInfoResponse>",
        "<NewStatus>Connected</NewStatus>",
        "<NewDownstreamMaxRate>118700</NewDownstreamMaxRate>",
        "<NewUpstreamMaxRate>38500</NewUpstreamMaxRate>",
        "<NewDownstreamCurrentRate>103200</NewDownstreamCurrentRate>",
        "<NewUpstreamCurrentRate>32100</NewUpstreamCurrentRate>",
        "<NewDownstreamNoiseMargin>87</NewDownstreamNoiseMargin>",
        "<NewUpstreamNoiseMargin>62</NewUpstreamNoiseMargin>",
        "<NewDownstreamAttenuation>213</NewDownstreamAttenuation>",
        "<NewUpstreamAttenuation>121</NewUpstreamAttenuation>",
        "</u:GetInfoResponse></s:Body></s:Envelope>"
    );

    #[test]
    fn extracts_values_from_soap_response() {
        assert_eq!(xml_val(SAMPLE_INFO, "NewStatus").as_deref(), Some("Connected"));
        assert_eq!(num_val(SAMPLE_INFO, "NewDownstreamMaxRate"), Some(118700.0));
        // Tenths of dB -> 8.7 dB
        assert_eq!(num_val(SAMPLE_INFO, "NewDownstreamNoiseMargin"), Some(87.0));
        assert_eq!(xml_val(SAMPLE_INFO, "NewMissingTag"), None);
        assert_eq!(num_val(SAMPLE_INFO, "NotANumberTag"), None);
    }

    #[test]
    fn crc_finder_sums_all_crc_tags() {
        let xml = "<A><NewDSLCRCErrorsTotal>12</NewDSLCRCErrorsTotal>\
                   <Other>5</Other>\
                   <NewUSLCCrcErrors>3</NewUSLCCrcErrors></A>";
        assert_eq!(find_crc_total(xml), Some(15));
        assert_eq!(find_crc_total("<Foo>1</Foo>"), None);
    }

    #[test]
    fn digest_header_contains_required_fields() {
        let challenge = "Digest realm=\"fritzbox\", nonce=\"ABC123\", qop=\"auth\"";
        let header = respond_digest(
            challenge,
            "user",
            "secret",
            "POST",
            "/upnp/control/wandslifconfig1",
        )
        .unwrap();
        assert!(header.starts_with("Digest "));
        assert!(header.contains("username=\"user\""));
        assert!(header.contains("realm=\"fritzbox\""));
        assert!(header.contains("nonce=\"ABC123\""));
        assert!(header.contains("response=\""));
        assert!(header.contains("qop=auth"));
    }
}