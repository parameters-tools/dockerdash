# DockerDash ⚡

```text
  ██████╗  ██████╗  ██████╗██╗  ██╗███████╗██████╗ ██████╗  █████╗ ███████╗██╗  ██╗
  ██╔══██╗██╔═══██╗██╔════╝██║ ██╔╝██╔════╝██╔══██╗██╔══██╗██╔══██╗██╔════╝██║  ██║
  ██║  ██║██║   ██║██║     █████═╝ █████╗  ██████╔╝██║  ██║███████║███████╗███████║
  ██║  ██║██║   ██║██║     ██╔═██╗ ██╔══╝  ██╔══██╗██║  ██║██╔══██║╚════██║██╔══██║
  ██████╔╝╚██████╔╝╚██████╗██║  ██╗███████╗██║  ██║██████╔╝██║  ██║███████║██║  ██║
  ╚═════╝  ╚═════╝  ╚═════╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚═════╝ ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝
```

<div align="center">

[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/docker-%230db7ed.svg?style=for-the-badge&logo=docker&logoColor=white)](https://www.docker.com/)
[![Ratatui](https://img.shields.io/badge/ratatui-%23179299.svg?style=for-the-badge&logo=terminal&logoColor=white)](https://ratatui.rs/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=for-the-badge)](https://opensource.org/licenses/MIT)

**Fast, lightweight, real-time Docker container telemetry directly in your terminal.**

[Key Features](#-key-features) •
[Installation](#-installation) •
[Usage](#-usage) •
[Interactive Controls](#-interactive-controls) •
[Architecture](#-architecture) •
[Troubleshooting](#-troubleshooting)

</div>

---

```text
┌─ DockerDash ──────────────────────────────────────────── engine 29.9.2 ─┐
│ ⠹ 4 containers · 4 running   CPU 2.3%   MEM 512.4MB / 6.21GB (8%)      │
├─ containers (4) — sorted by CPU ↓ ──────────────────────────────────────┤
│ NAME             ID           IMAGE         STATUS     CPU %   MEM ...  │
│ nginx-proxy      a1b2c3d4e5f6 nginx:latest  Up 3 days  1.42   104MB ... │
│ postgres         9f8e7d6c5b4a pg:16         Up 3 days  0.61   302MB ... │
├─ postgres ──────────────────────────────────────────────── 9f8e7d6c5b4a ┤
│ image    pg:16                                │┌ CPU % (recent) ┐      │
│ state    running  (Up 3 days)                 ││    ▂▄▆█▆▄▂      │     │
│ ports    5432/tcp, 0.0.0.0:5432->5432/tcp     │└────────────────┘      │
└─────────────────────────────────────────────────────────────────────────┘
 ↑/↓ select · c/m/n/p/u sort · a all · space pause · r refresh · ? help · q quit
```

## ✨ Key Features

- **⚡ Real-Time Streaming Telemetry**: Directly multiplexes persistent Docker Engine `/stats?stream=1` API endpoints without shell-outs or polling overhead.
- **📊 Instant Metric Calculation**: Accurate CPU % deltas matching official `docker stats` math, plus cgroup v1/v2 memory accounting (subtracts inactive page cache).
- **📈 CPU History Sparkline**: Live 120-point sparkline graph visualizing recent CPU activity trends for the selected container.
- **🖥️ Container Inspection Pane**: Dynamic details drawer revealing port mappings (`host:port -> container:port`), detailed container state, runtime uptime, and network/block I/O totals.
- **🌐 Cluster Aggregate Totals**: Live header tracking overall running container count, cumulative CPU utilization, and total memory consumption.
- **🎯 Flexible Sorting**: One-key sorting by CPU (`c`), Memory (`m`), Container Name (`n`), Thread/PID count (`p`), or Network I/O (`u`).
- **⏸️ Pause & Freeze**: Pause live telemetry on demand with `space` to inspect fluctuating metrics.
- **🤖 Scriptable & Pipe-Friendly**:
  - Automatically detects non-TTY execution and outputs a clean plain-text snapshot.
  - One-shot mode via `--once` for CI/CD pipelines, cron jobs, and scripts (e.g. `dockerdash --once | grep api`).
- **🛡️ Resilient Reconnection**: Gracefully preserves last-known state during Docker daemon restarts and reconnects without crashing.
- **🔌 Flexible Connection Targets**: Works out-of-the-box with standard Unix sockets, custom paths (`--socket`), and remote TCP endpoints (`DOCKER_HOST`).

---

## 📥 Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (1.75 or later)
- Docker Engine running locally or reachable via network
- Permission to access the Docker daemon socket (e.g., user in the `docker` group)

### Building from Source

```bash
# Clone the repository
git clone https://github.com/parameters-tools/dockerdash.git
cd dockerdash

# Build optimized release binary
cargo build --release

# The compiled binary will be located at:
./target/release/dockerdash
```

### Installing via Cargo

```bash
cargo install --path .
```

---

## 🚀 Usage

### Live Interactive Dashboard

```bash
# Launch interactive TUI (running containers)
dockerdash

# Include stopped/exited containers
dockerdash --all

# Set container list refresh interval (default: 5s)
dockerdash --interval 2

# Connect via a specific socket path
dockerdash --socket /var/run/docker.sock

# Connect via remote Docker daemon
DOCKER_HOST=tcp://192.168.1.50:2375 dockerdash
```

### Scripting & One-Shot Mode

`dockerdash` automatically falls back to plain-text output when piped:

```bash
# Snapshot as plain text table
dockerdash --once

# Pipe container list into other CLI tools
dockerdash | grep production

# Print plain text table including stopped containers
dockerdash --once -a
```

---

## ⌨️ Interactive Controls

| Key | Description |
| :--- | :--- |
| `q` / `Esc` | Quit DockerDash |
| `j` / `↓` | Move selection down |
| `k` / `↑` | Move selection up |
| `g` / `G` | Jump to top / bottom of container list |
| `PgUp` / `PgDn` | Scroll page up / down |
| `c` | Sort by **CPU %** (descending) |
| `m` | Sort by **Memory usage** (descending) |
| `n` | Sort by **Container Name** (alphabetical) |
| `p` | Sort by **PID / Thread count** (descending) |
| `u` | Sort by **Network I/O** (descending) |
| `a` | Toggle display of stopped / dead containers |
| `Space` | Pause / resume live metric updates |
| `r` | Force refresh container list immediately |
| `?` / `h` | Toggle keyboard shortcuts modal |

---

## 🧩 CLI Options

```text
Usage: dockerdash [OPTIONS]

Options:
  -i, --interval <INTERVAL>  Container list refresh interval in seconds [default: 5]
  -a, --all                  Include stopped containers, not just running ones
  -s, --socket <SOCKET>      Docker daemon socket path (overrides DOCKER_HOST)
      --once                 Render a single snapshot as plain text and exit
  -h, --help                 Print help
  -V, --version              Print version
```

---

## 🔬 Architecture & Internals

`DockerDash` is engineered for zero bloat, high responsiveness, and minimal system footprint:

```
┌────────────────┐     Unix Socket / TCP      ┌─────────────────────────┐
│ Docker Daemon  │ ◄────────────────────────► │ Bollard Client (Rust)   │
│ Engine API     │   GET /stats?stream=1      └────────────┬────────────┘
└────────────────┘                                         │
                                             tokio broadcast / mpsc channels
                                                           │
                                              ┌────────────▼────────────┐
                                              │ App State & Ring Buffer │
                                              │  - CPU deltas (cgroup)  │
                                              │  - Memory / I/O accum   │
                                              │  - 120-pt Sparkline     │
                                              └────────────┬────────────┘
                                                           │
                                              ┌────────────▼────────────┐
                                              │ Ratatui TUI + Crossterm │
                                              │ Double-Buffered Render  │
                                              └─────────────────────────┘
```

1. **Direct Bollard Client**: Interfaces directly with the Docker Engine API over Unix domain sockets or HTTP/HTTPS endpoints without spawning subprocesses.
2. **True Event Streams**: Avoids polling stampedes. When a container starts, a streaming task hooks into `/containers/{id}/stats?stream=1`.
3. **Accurate Delta Calculations**: Computes CPU usage through consecutive sample deltas:
   $$\text{CPU \%} = \frac{\Delta \text{container\_cpu}}{\Delta \text{system\_cpu}} \times N_{\text{cpus}} \times 100$$
4. **Memory Normalization**: Properly discounts cache/inactive memory on both cgroup v1 and cgroup v2 systems.

---

## 🛠️ Troubleshooting

- **`cannot reach the Docker daemon`**:
  - Verify that Docker is running: `docker info`
  - Ensure your user has permissions to access the docker socket:
    ```bash
    sudo usermod -aG docker $USER
    newgrp docker
    ```
- **Empty table on startup**:
  - If you have no active containers running, pass `-a` or press `a` inside the dashboard to display stopped containers.
- **Custom Docker Host**:
  - If using Docker Desktop, Colima, or Podman, supply the custom socket:
    ```bash
    dockerdash --socket ~/.colima/default/docker.sock
    ```

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
