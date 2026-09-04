# speed-test

A fast, terminal-based network diagnostics and speed testing application written in Rust.

`speed-test` provides more than a basic download/upload benchmark. It combines internet speed testing, latency analysis, continuous connection monitoring, DSL modem diagnostics, test history, and a keyboard-driven terminal UI in a single application.

The application is designed to keep working across different network conditions. It uses multiple network endpoints for throughput and latency measurements, retries failed throughput phases, detects connection problems, and provides fallback methods for connection information.

---

## Features

### Internet Speed Test

Measure your connection performance with:

* Download speed
* Upload speed
* Latency
* Jitter
* Minimum latency
* Average latency
* Maximum latency
* Live throughput
* Average throughput

The speed test uses multiple concurrent streams to better utilize high-bandwidth connections.

Download testing uses **4 concurrent streams**, while upload testing uses **3 concurrent streams**.

Throughput samples are collected every 100 ms and smoothed with an exponential moving average. This produces a more readable live graph while still preserving meaningful changes in network performance.

---

### Test Profiles

Three test profiles are available:

| Profile  |   Download |     Upload |
| -------- | ---------: | ---------: |
| Quick    |  5 seconds |  5 seconds |
| Standard | 10 seconds | 10 seconds |
| Maximum  | 20 seconds | 20 seconds |

The profile controls the duration of the download and upload phases.

Use the profile that matches the amount of testing you need:

* **Quick** — fast connection checks
* **Standard** — normal measurements
* **Maximum** — longer measurements for more sustained throughput

---

### Latency Measurement

The latency test performs **12 probes** and calculates:

* Minimum latency
* Average latency
* Maximum latency
* Jitter

Jitter is calculated from the average absolute difference between consecutive round-trip-time measurements.

The application also uses fallback probe endpoints when the preferred endpoint is blocked by the current network or firewall.

---

### Download Endpoint Fallback

The application does not depend on a single download server.

It can use several independent sources:

1. Cloudflare Speed Test
2. CacheFly
3. OVH
4. Hetzner

If a source fails or is blocked, the application tries another source.

The application also remembers the last successful source and prefers it on future tests.

This makes the speed test more tolerant of networks that block or interfere with a specific endpoint.

---

### Upload Testing

Upload measurements use streaming request bodies.

The application generates data in 64 KiB chunks and sends it to the upload endpoint.

Small chunks are used to keep the internal byte counter granular. This helps prevent the live throughput graph from appearing to jump in large bursts.

---

### Connection Information

The application automatically retrieves connection information, including:

* Public IP address
* ISP / organization
* ASN
* City
* Country
* Cloudflare data-center / colo

It first attempts to retrieve metadata from Cloudflare.

If that fails, it falls back to Cloudflare's trace endpoint and uses `ipwho.is` for additional geographic and network information.

---

# Network Monitor

The **Monitor** tab provides continuous latency monitoring.

It can monitor a configurable target host and continuously collect round-trip-time samples.

The monitor tracks:

* Current latency
* Minimum latency
* Average latency
* Maximum latency
* Jitter
* Packet loss
* Sent probes
* Received probes
* Stability
* Connection incidents

The monitor keeps a rolling window of recent measurements instead of growing its data indefinitely.

---

## Connection Incident Detection

The monitor detects several types of network problems:

| Incident      | Meaning                                 |
| ------------- | --------------------------------------- |
| `SPIKE`       | Latency increased significantly         |
| `LOSS`        | A probe was lost                        |
| `OUTAGE`      | Multiple probes were lost consecutively |
| `JITTER`      | Jitter increased significantly          |
| `RECOVERED`   | Connection recovered after a problem    |
| `UNREACHABLE` | The target could not be reached         |

The monitor stores recent incidents and displays them in the terminal interface.

Up to 60 incidents are retained.

---

## Gaming Mode

The network monitor includes a **Gaming Mode**.

Gaming Mode adjusts the latency threshold used to determine whether the current connection is healthy.

This is useful when the main concern is low latency and connection stability rather than general-purpose network performance.

---

# DSL / Modem Diagnostics

The **Modem** tab can retrieve DSL line statistics directly from a compatible modem or router.

The implementation uses the **TR-064 / SOAP management interface**.

This information is obtained directly from the local modem. It is not available from a normal internet speed test.

Supported information includes:

### Line Status

* Connection state
* DSL mode
* Uptime
* Firmware version

### Downstream / Upstream Statistics

* SNR margin
* Line attenuation
* Current synchronization rate
* Maximum synchronization rate
* Line power

### Additional Statistics

* CRC errors
* Wireless client count

The application does not fabricate unavailable modem values. If the modem does not expose a specific value, it is reported as unavailable.

---

## Automatic Modem Discovery

The modem diagnostics component can try multiple hosts.

It starts with the configured modem address and can also detect the default gateway.

Common fallback addresses include:

```text
192.168.1.1
192.168.0.1
fritz.box
```

Once a working TR-064 host is found, it is remembered and used for subsequent polling.

The modem poller normally retrieves updated statistics every **2 seconds**.

After a failed polling attempt, it waits before retrying.

---

## DSL Health Detection

The modem diagnostics include thresholds for detecting abnormal DSL conditions.

Examples include:

* Low SNR margin
* High line attenuation
* Low synchronization rate compared with the maximum rate

Current thresholds include:

```text
SNR warning:       < 6 dB
SNR critical:      < 3 dB

Attenuation warn:  > 49 dB
Attenuation crit:  > 58 dB

Rate warning:      < 50% of maximum rate
```

Detected conditions can be classified as:

* `INFO`
* `WARN`
* `CRIT`

---

# Test History

Completed speed tests can be stored locally.

Each history record contains:

* Timestamp
* Test profile
* Download speed
* Upload speed
* Ping
* Jitter
* Grade

History is stored as JSON.

The default history file is:

```text
~/.speed-test-history.json
```

On Windows, the application also supports the `USERPROFILE` environment variable when resolving the user's home directory.

---

## Safe History Writes

History is not written directly over the existing file.

The application first writes the new data to:

```text
.speed-test-history.json.tmp
```

and then replaces the existing history file.

This reduces the risk of losing the entire history if the application is interrupted during a write.

If the history file is corrupted, it is moved to:

```text
.speed-test-history.json.bad
```

instead of being silently overwritten.

---

## Custom History Location

The history file can be overridden with:

```text
SPEED_TEST_HISTORY_FILE
```

Example:

```powershell
$env:SPEED_TEST_HISTORY_FILE="C:\temp\speed-test-history.json"
```

This is also useful when running tests because the application's real history file does not need to be modified.

---

# Terminal User Interface

The application runs entirely in the terminal.

The UI is implemented with [`ratatui`](https://github.com/ratatui/ratatui) and uses `crossterm` for terminal input.

The application contains five main tabs:

1. **Test**
2. **Monitor**
3. **Modem**
4. **History**
5. **Help**

The UI continuously redraws the application and includes live graphs and status information.

If the terminal is too small, the application displays a message asking the user to resize it instead of attempting to render an unusable layout.

The minimum supported terminal size enforced by the UI is:

```text
50 × 12
```

---

# Keyboard Controls

The application uses a centralized keyboard mapping system.

## Global Controls

| Key         | Action                         |
| ----------- | ------------------------------ |
| `F1`        | Test tab                       |
| `F2`        | Monitor tab                    |
| `F3`        | History tab                    |
| `F4`        | Help tab                       |
| `F9`        | Modem tab                      |
| `F5`        | Refresh connection information |
| `F10`       | Quit                           |
| `ESC`       | Cancel / Back                  |
| `TAB`       | Next tab                       |
| `SHIFT+TAB` | Previous tab                   |

---

## Test Tab

| Key      | Action                               |
| -------- | ------------------------------------ |
| `ENTER`  | Start test / stop continuous session |
| `INSERT` | Toggle single / continuous mode      |
| `M`      | Toggle single / continuous mode      |
| `UP`     | Previous profile                     |
| `DOWN`   | Next profile                         |
| `P`      | Next profile                         |

---

## Monitor Tab

| Key     | Action                  |
| ------- | ----------------------- |
| `ENTER` | Start / stop monitoring |
| `F6`    | Toggle Gaming Mode      |
| `G`     | Toggle Gaming Mode      |
| `F7`    | Edit target host        |
| `T`     | Edit target host        |
| `F8`    | Reset monitor session   |
| `C`     | Reset monitor session   |

---

## Modem Tab

| Key     | Action                   |
| ------- | ------------------------ |
| `ENTER` | Pause / resume polling   |
| `F7`    | Edit modem configuration |
| `T`     | Edit modem configuration |
| `F8`    | Clear modem log          |
| `C`     | Clear modem log          |

---

## History Tab

| Key               | Action                |
| ----------------- | --------------------- |
| `UP` / `K`        | Previous entry        |
| `DOWN` / `J`      | Next entry            |
| `DELETE` / `D`    | Delete selected entry |
| `BACKSPACE` / `X` | Clear all history     |

---

# Keyboard Layout Independence

One of the notable design features of the application is its handling of non-Latin keyboard layouts.

On Windows, the application reads the Windows console input buffer directly and uses **virtual key codes** instead of relying only on the character generated by the active keyboard layout.

This means physical keys such as `VK_M`, `VK_Q`, and `VK_R` can still be recognized correctly when the user has a different keyboard layout active.

The implementation specifically accounts for layouts such as:

* English
* Persian
* Russian
* German
* Other non-US layouts

The normal keyboard mapping system also translates common Russian `ЙЦУКЕН` characters back to their corresponding US keyboard positions.

For example, the physical `M` position on a Russian layout can still resolve to the application's `M` shortcut.

The project also includes tests for layout-independent key handling.

---

# Error Handling

The application is designed to avoid leaving the terminal in a broken state.

Before entering the application loop, it installs a panic hook that restores the terminal when a panic occurs.

If the main application loop exits with an error, the terminal is restored before the error is printed.

Errors are reported with additional context, for example:

```text
speed-test exited with an error:
  terminal draw failed
```

The application also provides a basic suggestion to check the internet connection when a runtime error occurs.

---

# Reliability Features

Several mechanisms are included to make long-running tests more reliable.

### Cancellation

Network operations use shared cancellation flags.

A running test can therefore stop without waiting for the entire test duration to finish.

### Phase Retries

Throughput phases can retry when they complete without transferring any data.

The retry delays are:

```text
400 ms
1500 ms
```

with up to two retries.

This helps with temporary endpoint or connection failures.

### Preferred Endpoints

The application remembers successful download and latency endpoints.

Future tests start with the endpoint that worked previously.

### Stall Watchdog

The application has a watchdog for stalled test activity.

If a test remains inactive for too long, it can abort instead of hanging indefinitely.

### Bounded Live Data

Live throughput data is stored in a bounded collection.

The application keeps up to 600 throughput samples, preventing an indefinitely running session from growing memory usage without limit.

---

# Architecture

The project is organized into focused Rust modules:

```text
src/
├── app.rs
├── dsl.rs
├── history.rs
├── input.rs
├── keys.rs
├── main.rs
├── net.rs
└── ui.rs
```

### `main.rs`

Application entry point.

Responsibilities include:

* Terminal initialization
* Panic handling
* Application startup
* Main event loop
* Terminal redraw
* Keyboard input dispatch
* Application shutdown

The main loop runs asynchronously with Tokio and redraws the interface at approximately 60 Hz.

---

### `app.rs`

Contains the application's main state and orchestration logic.

It manages:

* Current tab
* Test state
* Monitor state
* DSL state
* History
* Connection information
* Test events
* Monitor incidents
* Throughput history
* User actions

---

### `net.rs`

Contains network-related functionality.

Responsibilities include:

* Connection information
* Latency measurement
* Download measurement
* Upload measurement
* Throughput sampling
* Network endpoint fallback
* Test profiles
* Cancellation
* Network monitor probes

The module uses asynchronous Tokio tasks and shared atomic counters for concurrent throughput measurement.

---

### `dsl.rs`

Contains DSL modem diagnostics.

Responsibilities include:

* TR-064 communication
* SOAP requests
* Modem discovery
* Gateway detection
* DSL snapshots
* DSL health thresholds
* Modem incidents
* Periodic polling

---

### `history.rs`

Contains local test-history persistence.

Responsibilities include:

* Loading history
* Saving history
* Corruption recovery
* Safe file replacement
* Custom history paths

---

### `keys.rs`

Contains the centralized keyboard action system.

Responsibilities include:

* Keyboard actions
* Keyboard scopes
* Key bindings
* Layout normalization
* Shortcut resolution
* Editor input handling

Keeping the bindings in one central registry means the application's help screen and footer can use the same source of truth as the actual input handling.

---

### `input.rs`

Windows-specific physical keyboard input.

The module uses the Win32 console API to read virtual key codes directly.

It is compiled only on Windows.

Other platforms use `crossterm`'s normal event system.

---

### `ui.rs`

Contains the terminal interface.

Responsibilities include:

* Main layout
* Tabs
* Test screen
* Monitor screen
* Modem screen
* History screen
* Help screen
* Gauges
* Sparklines
* Throughput graphs
* Status indicators
* Keyboard shortcut footer

---

# Technology Stack

The project is written in Rust and uses the following main dependencies:

| Technology        | Purpose                           |
| ----------------- | --------------------------------- |
| Rust 2024 Edition | Application language              |
| Tokio             | Async runtime                     |
| Reqwest           | HTTP client                       |
| Ratatui           | Terminal UI                       |
| Crossterm         | Terminal input and control        |
| Futures Util      | Async streams and utilities       |
| Serde             | Serialization and deserialization |
| Serde JSON        | JSON handling                     |
| Anyhow            | Error handling                    |
| Chrono            | Date and time                     |
| MD5               | Hashing                           |
| WinAPI            | Windows-specific console input    |

The project uses Rust **2024 Edition**.

---

# Requirements

You need:

* Rust toolchain with Rust 2024 Edition support
* Cargo
* A terminal with ANSI / alternate-screen support
* An active internet connection for internet tests

Windows builds additionally use the Win32 console APIs for physical keyboard input.

---

# Installation

Clone the repository:

```bash
git clone https://github.com/VictorZeZ/speed-test.git
cd speed-test
```

Build the project:

```bash
cargo build
```

Run it:

```bash
cargo run
```

For an optimized build:

```bash
cargo build --release
```

Run the optimized binary:

```bash
cargo run --release
```

The release profile uses optimization level 3 and link-time optimization.

---

# Running the Application

After starting the application, the terminal UI opens automatically.

The application initializes:

1. Local test history
2. Connection information lookup
3. DSL/modem polling
4. Terminal input handling
5. The main application loop

The application then waits for user input.

---

# Typical Workflow

## Run a Standard Speed Test

1. Start the application.
2. Open the **Test** tab.
3. Select a profile.
4. Press `ENTER`.
5. Wait for the latency, download, and upload phases.
6. Review the final measurements.
7. The result is stored in history.

---

## Run Continuous Tests

1. Open the **Test** tab.
2. Toggle continuous mode with `INSERT` or `M`.
3. Press `ENTER`.
4. The application continues running tests.
5. Press `ENTER` again to stop the continuous session.

---

## Monitor a Host

1. Open the **Monitor** tab.
2. Set the target host if needed.
3. Press `ENTER`.
4. Watch latency, jitter, packet loss, and incidents.
5. Press `ENTER` again to stop monitoring.

---

## Check DSL Statistics

1. Open the **Modem** tab.
2. Configure the modem address and credentials if required.
3. Start polling.
4. Review line status, SNR, attenuation, rates, power, and errors.
5. Monitor detected modem incidents.

The modem information is obtained from the local management interface, not from the public internet.

---

# Data and Privacy

The application performs network requests to measure connection performance and retrieve connection information.

Depending on the functionality being used, requests may be made to:

* Cloudflare Speed Test
* CacheFly
* OVH
* Hetzner
* `ipwho.is`
* A user-configured modem/router

The application can retrieve the public IP address and network metadata required to display connection information.

Speed-test history is stored locally as a JSON file.

Modem credentials are used for local modem communication when configured. They are not required for the normal internet speed test.

---

# Limitations

Speed measurements depend on the network environment.

Results can be affected by:

* Wi-Fi conditions
* Ethernet link speed
* VPNs
* Firewalls
* Network congestion
* ISP traffic management
* CPU load
* Other network traffic
* Endpoint availability
* Router performance

A speed-test result should therefore be treated as a measurement of the connection under the current test conditions, not as a guaranteed maximum ISP speed.

DSL statistics are only available when the local modem exposes the required management interface.

---

# Testing

The project contains unit tests for important internal components, including keyboard handling.

Run the test suite with:

```bash
cargo test
```

The keyboard subsystem tests cover cases such as:

* Universal keyboard shortcuts
* Non-Latin keyboard layouts
* Russian keyboard-layout translation
* Scope-specific shortcuts
* Ctrl+C handling
* Modifier handling
* Windows virtual-key mapping

The Windows-specific input implementation also includes tests for physical key mappings.

---

# Project Structure

```text
speed-test/
│
├── Cargo.toml
├── Cargo.lock
├── .gitignore
│
└── src/
    ├── app.rs
    ├── dsl.rs
    ├── history.rs
    ├── input.rs
    ├── keys.rs
    ├── main.rs
    ├── net.rs
    └── ui.rs
```

The repository currently uses `master` as its default branch.

---

# Design Goals

The project is built around several practical goals:

* Keep the interface entirely terminal-based.
* Provide useful network diagnostics in one application.
* Avoid depending on one speed-test endpoint.
* Make measurements resilient to temporary network failures.
* Provide live feedback instead of only final results.
* Keep continuous monitoring bounded in memory.
* Preserve test history safely.
* Support keyboard layouts that would normally break terminal shortcuts.
* Provide direct access to modem-level DSL statistics when available.
* Fail clearly instead of leaving the terminal in a broken state.

---

# License

No license file is currently included in the repository.

Until a license is added to the project, users should not assume that the source code is available for unrestricted redistribution, modification, or commercial use.

---

# Author

Created by **VictorZeZ**.

Repository:

https://github.com/VictorZeZ/speed-test

---

# Contributing

Contributions are welcome.

Before submitting a change:

1. Keep changes focused.
2. Follow the existing Rust structure.
3. Add tests for new behavior where practical.
4. Run:

```bash
cargo test
cargo check
```

5. Verify the terminal UI after changes that affect rendering or keyboard input.
6. Verify Windows-specific behavior when modifying `input.rs`.

For keyboard changes, update the centralized key map rather than implementing shortcuts in multiple locations.

---

# Troubleshooting

## The speed test cannot connect

Check:

* Your internet connection.
* Firewall rules.
* VPN configuration.
* DNS resolution.
* Whether the selected network blocks the test endpoints.

The application already attempts alternative throughput and latency endpoints.

---

## Download testing returns no data

The application tries multiple download sources and can retry a zero-data phase.

If all sources fail, the network or firewall may be blocking the required endpoints.

---

## DSL information is unavailable

Check:

* The modem address.
* The modem supports TR-064.
* The management interface is enabled.
* The username and password are correct.
* The computer can reach the modem over the local network.

DSL information cannot be obtained from the public internet. It must come from the modem's local management interface.

---

## Keyboard shortcuts do not work as expected on Windows

The Windows implementation uses physical virtual-key codes to avoid dependence on the active keyboard language.

If the problem persists:

* Verify that the application is running inside a normal Windows console.
* Check whether another application is consuming console input.
* Try the function-key or navigation-key equivalent of the shortcut.

---

# Summary

`speed-test` is a Rust-based terminal network diagnostics tool that combines:

* Internet speed testing
* Download and upload benchmarks
* Latency and jitter measurement
* Continuous ping monitoring
* Packet-loss detection
* Network incident detection
* Gaming-oriented latency monitoring
* Connection and ISP information
* DSL modem diagnostics
* TR-064 support
* Test history
* Live terminal graphs
* Multiple fallback endpoints
* Retry and watchdog mechanisms
* Layout-independent keyboard controls
* Windows physical-key handling

It is designed as a compact, keyboard-first alternative to browser-based network testing tools, while also providing diagnostics that are normally separated into different applications.
