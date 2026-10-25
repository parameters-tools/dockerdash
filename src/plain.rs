use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;

use bollard::Docker;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::docker::{self, Snapshot};
use crate::human;

/// Write one output line; a closed pipe downstream (e.g. `dockerdash | head`)
/// exits quietly instead of panicking.
fn out(line: &str) -> Result<(), Box<dyn Error>> {
    use std::io::Write;
    let result = writeln!(std::io::stdout().lock(), "{line}");
    if let Err(e) = &result {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
    }
    Ok(result?)
}

/// One-shot plain text table (used with `--once` or when stdout is not a TTY).
pub async fn run(docker: &Docker, all: bool) -> Result<(), Box<dyn Error>> {
    let infos = docker::list(docker, all).await?;
    if infos.is_empty() {
        out(&format!("no {} containers", if all { "matching" } else { "running" }))?;
        return Ok(());
    }

    let (tx, mut rx) = mpsc::channel::<(String, Snapshot)>(infos.len().max(1));
    let mut pending = 0usize;
    for info in &infos {
        if info.state != "running" {
            continue;
        }
        pending += 1;
        let docker = docker.clone();
        let id = info.id.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            // stream=false makes the daemon return one complete sample with a
            // valid pre-cpu baseline (the first streaming frame always shows ~0%)
            let opts = bollard::container::StatsOptions {
                stream: false,
                one_shot: false,
            };
            let mut stream = docker.stats(&id, Some(opts));
            if let Some(Ok(stats)) = stream.next().await {
                if let Some(snap) = docker::compute_snapshot(&stats) {
                    let _ = tx.send((id, snap)).await;
                }
            }
        });
    }
    drop(tx);

    let mut snaps: HashMap<String, Snapshot> = HashMap::new();
    if pending > 0 {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some((id, s)) = rx.recv().await {
                snaps.insert(id, s);
            }
        })
        .await;
    }

    struct Out {
        cells: Vec<String>,
        cpu: f64,
        mem: u64,
    }
    let mut rows: Vec<Out> = infos
        .iter()
        .map(|i| match snaps.get(&i.id) {
            Some(s) => Out {
                cells: vec![
                    i.name.clone(),
                    i.id.chars().take(12).collect(),
                    i.image.clone(),
                    i.status.clone(),
                    format!("{:.2}", s.cpu_percent),
                    format!("{} / {}", human::bytes(s.mem_usage), human::bytes(s.mem_limit)),
                    format!("{:.1}", s.mem_percent),
                    format!("{} / {}", human::bytes(s.net_rx), human::bytes(s.net_tx)),
                    format!("{} / {}", human::bytes(s.block_read), human::bytes(s.block_write)),
                    s.pids.to_string(),
                ],
                cpu: s.cpu_percent,
                mem: s.mem_usage,
            },
            None => Out {
                cells: vec![
                    i.name.clone(),
                    i.id.chars().take(12).collect(),
                    i.image.clone(),
                    i.status.clone(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                    "—".into(),
                ],
                cpu: -1.0,
                mem: 0,
            },
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.mem.cmp(&a.mem))
    });

    let headers = [
        "NAME", "ID", "IMAGE", "STATUS", "CPU %", "MEM USAGE / LIMIT", "MEM %", "NET I/O", "BLOCK I/O", "PIDS",
    ];
    const CAP: usize = 40;
    let mut w = [0usize; 10];
    for (i, h) in headers.iter().enumerate() {
        w[i] = h.len();
    }
    for r in &rows {
        for (i, c) in r.cells.iter().enumerate() {
            w[i] = w[i].max(c.chars().count().min(CAP));
        }
    }

    let cell = |s: &str, i: usize| -> String {
        let width = w[i];
        let clipped: String = s.chars().take(CAP).collect();
        if i >= 4 {
            format!("{clipped:>width$}")
        } else {
            format!("{clipped:<width$}")
        }
    };

    let header_line: String = headers
        .iter()
        .enumerate()
        .map(|(i, h)| cell(h, i))
        .collect::<Vec<_>>()
        .join("  ");
    out(&header_line)?;
    let sep: String = w.iter().map(|n| "-".repeat(*n)).collect::<Vec<_>>().join("  ");
    out(&sep)?;
    for r in &rows {
        let line: String = r
            .cells
            .iter()
            .enumerate()
            .map(|(i, c)| cell(c, i))
            .collect::<Vec<_>>()
            .join("  ");
        out(&line)?;
    }

    if rows.iter().any(|r| r.cpu >= 0.0) {
        let cpu: f64 = rows.iter().filter(|r| r.cpu >= 0.0).map(|r| r.cpu).sum();
        let mu: u64 = rows.iter().filter(|r| r.cpu >= 0.0).map(|r| r.mem).sum();
        out("")?;
        out(&format!("TOTALS   CPU {cpu:.2}%   MEM {}", human::bytes(mu)))?;
    }
    Ok(())
}
