use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bollard::container::{ListContainersOptions, MemoryStatsStats, Stats, StatsOptions};
use bollard::models::{ContainerSummary, PortTypeEnum};
use bollard::Docker;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    pub ports: Vec<String>,
}

pub struct Snapshot {
    pub cpu_percent: f64,
    pub mem_usage: u64,
    pub mem_limit: u64,
    pub mem_percent: f64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub block_read: u64,
    pub block_write: u64,
    pub pids: u64,
}

pub fn connect(socket: Option<&str>) -> Result<Docker, bollard::errors::Error> {
    match socket {
        Some(path) => Docker::connect_with_socket(path, 10, bollard::API_DEFAULT_VERSION),
        None => Docker::connect_with_local_defaults(),
    }
}

pub async fn engine_version(docker: &Docker) -> Result<String, bollard::errors::Error> {
    let v = docker.version().await?;
    Ok(v
        .version
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into()))
}

pub async fn list(docker: &Docker, all: bool) -> Result<Vec<ContainerInfo>, bollard::errors::Error> {
    let opts = ListContainersOptions::<String> {
        all,
        ..Default::default()
    };
    let items: Vec<ContainerSummary> = docker.list_containers(Some(opts)).await?;
    Ok(items.into_iter().map(to_info).collect())
}

fn to_info(c: ContainerSummary) -> ContainerInfo {
    let id = c.id.unwrap_or_default();
    let id12: String = id.chars().take(12).collect();
    let name = c
        .names
        .and_then(|mut n| n.pop())
        .map(|n| n.trim_start_matches('/').to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| id12.clone());
    let ports = c
        .ports
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let proto = p.typ.unwrap_or(PortTypeEnum::EMPTY);
            match (&p.ip, &p.public_port) {
                (Some(ip), Some(public)) => format!("{ip}:{public}->{}/{}", p.private_port, proto),
                _ => format!("{}/{}", p.private_port, proto),
            }
        })
        .collect();
    ContainerInfo {
        id,
        name,
        image: c.image.unwrap_or_default(),
        state: c.state.unwrap_or_default(),
        status: c.status.unwrap_or_default(),
        created: c.created.unwrap_or_default(),
        ports,
    }
}

/// One snapshot read from the daemon's streaming `/stats` endpoint, using the same
/// formulas the `docker stats` CLI uses: CPU % from the cpu/system deltas between
/// consecutive samples, memory minus page cache, cumulative network/block counters.
pub fn compute_snapshot(s: &Stats) -> Option<Snapshot> {
    let cpu_total = s.cpu_stats.cpu_usage.total_usage;
    let precpu_total = s.precpu_stats.cpu_usage.total_usage;
    let sys = s.cpu_stats.system_cpu_usage.unwrap_or(0);
    let presys = s.precpu_stats.system_cpu_usage.unwrap_or(0);
    // the first frame of a stats stream has a zeroed pre-cpu baseline; dropping it
    // avoids a bogus ~0% reading for the first second
    if sys > 0 && presys == 0 {
        return None;
    }
    let ncpu = s
        .cpu_stats
        .online_cpus
        .filter(|&n| n > 0)
        .or_else(|| {
            s.cpu_stats
                .cpu_usage
                .percpu_usage
                .as_ref()
                .map(|v| v.len() as u64)
                .filter(|&n| n > 0)
        })
        .unwrap_or(1);
    let cpu_percent = if sys > presys && cpu_total >= precpu_total {
        ((cpu_total - precpu_total) as f64 / (sys - presys) as f64) * ncpu as f64 * 100.0
    } else {
        0.0
    };

    let usage = s.memory_stats.usage.unwrap_or(0);
    let limit = s.memory_stats.limit.unwrap_or(0);
    // page cache is reclaimable, so it is not counted as "used" (same as `docker stats`)
    let inactive = match s.memory_stats.stats {
        Some(MemoryStatsStats::V1(v)) => v.total_inactive_file,
        Some(MemoryStatsStats::V2(v)) => v.inactive_file,
        None => 0,
    };
    let used = usage.saturating_sub(inactive);
    let mem_percent = if limit > 0 {
        used as f64 / limit as f64 * 100.0
    } else {
        0.0
    };

    let (net_rx, net_tx) = s
        .networks
        .as_ref()
        .map(|nets| {
            nets.values()
                .fold((0u64, 0u64), |(r, t), n| (r + n.rx_bytes, t + n.tx_bytes))
        })
        .unwrap_or((0, 0));

    let (block_read, block_write) = s
        .blkio_stats
        .io_service_bytes_recursive
        .as_ref()
        .map(|items| {
            items.iter().fold((0u64, 0u64), |(r, w), it| match it.op.as_str() {
                "read" | "Read" => (r + it.value, w),
                "write" | "Write" => (r, w + it.value),
                _ => (r, w),
            })
        })
        .unwrap_or((0, 0));

    let pids = s.pids_stats.current.unwrap_or(0);

    Some(Snapshot {
        cpu_percent,
        mem_usage: used,
        mem_limit: limit,
        mem_percent,
        net_rx,
        net_tx,
        block_read,
        block_write,
        pids,
    })
}

pub fn stats_options() -> StatsOptions {
    StatsOptions {
        stream: true,
        one_shot: false,
    }
}

/// How long a stats stream may go silent before it is considered wedged and
/// restarted (the daemon sends a frame roughly every second).
const STATS_STALE_AFTER: Duration = Duration::from_secs(30);

/// Keeps one long-lived streaming task per live container; finished, orphaned or
/// wedged tasks are reaped so containers that restart get a fresh stream.
pub struct StatsManager {
    tx: mpsc::Sender<(String, Snapshot)>,
    tasks: HashMap<String, (JoinHandle<()>, Arc<Mutex<Instant>>)>,
}

impl StatsManager {
    pub fn new() -> (mpsc::Receiver<(String, Snapshot)>, Self) {
        let (tx, rx) = mpsc::channel(256);
        (rx, Self { tx, tasks: HashMap::new() })
    }

    pub fn ensure(&mut self, docker: &Docker, id: &str) {
        if self.tasks.contains_key(id) {
            return;
        }
        let docker = docker.clone();
        let task_id = id.to_string();
        let tx = self.tx.clone();
        let last_seen = Arc::new(Mutex::new(Instant::now()));
        let handle = {
            let last_seen = Arc::clone(&last_seen);
            tokio::spawn(async move {
                let mut stream = docker.stats(&task_id, Some(stats_options()));
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(stats) => {
                            let Some(snap) = compute_snapshot(&stats) else { continue };
                            if tx.send((task_id.clone(), snap)).await.is_err() {
                                break;
                            }
                            *last_seen.lock().unwrap() = Instant::now();
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        self.tasks.insert(id.to_string(), (handle, last_seen));
    }

    pub fn reconcile(&mut self, live: &HashSet<String>) {
        self.tasks.retain(|id, (handle, last_seen)| {
            let alive = live.contains(id)
                && !handle.is_finished()
                && last_seen.lock().unwrap().elapsed() < STATS_STALE_AFTER;
            if alive {
                true
            } else {
                handle.abort();
                false
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats_from_json(json: &str) -> Stats {
        serde_json::from_str(json).unwrap()
    }

    const THROTTLING: &str =
        r#""throttling_data":{"periods":0,"throttled_periods":0,"throttled_time":0}"#;
    const NET: &str = r#""networks":{"eth0":{"rx_bytes":100,"rx_dropped":0,"rx_errors":0,"rx_packets":1,"tx_bytes":10,"tx_dropped":0,"tx_errors":0,"tx_packets":1}}"#;

    #[test]
    fn skips_first_stream_frame_with_zeroed_baseline() {
        let s = stats_from_json(&format!(
            r#"{{"read":"2024-01-01T00:00:00Z","preread":"2024-01-01T00:00:00Z","num_procs":0,
                "pids_stats":{{}},"memory_stats":{{}},"blkio_stats":{{}},"storage_stats":{{}},
                "cpu_stats":{{"cpu_usage":{{"total_usage":100,"usage_in_usermode":0,"usage_in_kernelmode":0}},"system_cpu_usage":1000,{THROTTLING}}},
                "precpu_stats":{{"cpu_usage":{{"total_usage":0,"usage_in_usermode":0,"usage_in_kernelmode":0}},"system_cpu_usage":0,{THROTTLING}}}}}"#
        ));
        assert!(compute_snapshot(&s).is_none());
    }

    #[test]
    fn computes_cpu_memory_net_pids() {
        // cpu delta 100 over system delta 1000 with 2 online cpus -> 20%
        let s = stats_from_json(&format!(
            r#"{{"read":"2024-01-01T00:00:01Z","preread":"2024-01-01T00:00:00Z","num_procs":0,
                "pids_stats":{{"current":7}},
                "memory_stats":{{"usage":1000,"limit":4000}},
                "blkio_stats":{{}},
                "storage_stats":{{}},
                {NET},
                "cpu_stats":{{"cpu_usage":{{"total_usage":150,"usage_in_usermode":100,"usage_in_kernelmode":50}},"system_cpu_usage":1500,"online_cpus":2,{THROTTLING}}},
                "precpu_stats":{{"cpu_usage":{{"total_usage":50,"usage_in_usermode":50,"usage_in_kernelmode":0}},"system_cpu_usage":500,"online_cpus":2,{THROTTLING}}}}}"#
        ));
        let snap = compute_snapshot(&s).expect("frame with valid baseline");
        assert!((snap.cpu_percent - 20.0).abs() < 1e-9);
        assert_eq!(snap.mem_usage, 1000);
        assert_eq!(snap.mem_limit, 4000);
        assert!((snap.mem_percent - 25.0).abs() < 1e-9);
        assert_eq!(snap.net_rx, 100);
        assert_eq!(snap.net_tx, 10);
        assert_eq!(snap.pids, 7);
    }

    #[test]
    fn falls_back_to_percpu_count_for_cpus() {
        // no online_cpus field: percpu_usage length must be used instead
        let s = stats_from_json(&format!(
            r#"{{"read":"2024-01-01T00:00:01Z","preread":"2024-01-01T00:00:00Z","num_procs":0,
                "pids_stats":{{}},"memory_stats":{{}},"blkio_stats":{{}},"storage_stats":{{}},
                "cpu_stats":{{"cpu_usage":{{"percpu_usage":[10,10,10,10],"total_usage":100,"usage_in_usermode":0,"usage_in_kernelmode":0}},"system_cpu_usage":1000,{THROTTLING}}},
                "precpu_stats":{{"cpu_usage":{{"total_usage":50,"usage_in_usermode":0,"usage_in_kernelmode":0}},"system_cpu_usage":500,{THROTTLING}}}}}"#
        ));
        let snap = compute_snapshot(&s).unwrap();
        // (100-50)/(1000-500) * 4 * 100 = 40%
        assert!((snap.cpu_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn cgroup_v1_memory_subtracts_total_inactive_file() {
        let s = stats_from_json(&format!(
            r#"{{"read":"2024-01-01T00:00:01Z","preread":"2024-01-01T00:00:00Z","num_procs":0,
                "pids_stats":{{}},"blkio_stats":{{}},"storage_stats":{{}},
                "memory_stats":{{"usage":1000,"limit":4000,"stats":{{
                    "cache":100,"dirty":0,"mapped_file":0,"total_inactive_file":300,"pgpgout":0,
                    "rss":700,"total_mapped_file":0,"writeback":0,"unevictable":0,"pgpgin":0,
                    "total_unevictable":0,"pgmajfault":0,"total_rss":700,"total_rss_huge":0,
                    "total_writeback":0,"total_inactive_anon":0,"rss_huge":0,
                    "hierarchical_memory_limit":4000,"total_pgfault":0,"total_active_file":0,
                    "active_anon":0,"total_active_anon":0,"total_pgpgout":0,"total_cache":100,
                    "total_dirty":0,"inactive_anon":0,"active_file":0,"pgfault":0,
                    "inactive_file":300,"total_pgmajfault":0,"total_pgpgin":0}}}},
                "cpu_stats":{{"cpu_usage":{{"total_usage":150,"usage_in_usermode":0,"usage_in_kernelmode":0}},"system_cpu_usage":1000,{THROTTLING}}},
                "precpu_stats":{{"cpu_usage":{{"total_usage":50,"usage_in_usermode":0,"usage_in_kernelmode":0}},"system_cpu_usage":500,{THROTTLING}}}}}"#
        ));
        let snap = compute_snapshot(&s).unwrap();
        assert_eq!(snap.mem_usage, 700); // 1000 - 300 page cache
    }
}
