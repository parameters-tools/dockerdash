use std::time::{SystemTime, UNIX_EPOCH};

/// Docker-style compact byte formatting: `999B`, `1.54kB`, `23.7MB`, `1.09GB`.
pub fn bytes(v: u64) -> String {
    if v < 1000 {
        return format!("{v}B");
    }
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    let mut val = v as f64;
    let mut unit = 0;
    while val >= 1000.0 && unit < UNITS.len() {
        val /= 1000.0;
        unit += 1;
    }
    // `unit` counts the divisions performed; after one division the value is in kB
    let unit = &UNITS[unit - 1];
    if val < 10.0 {
        format!("{val:.2}{unit}")
    } else {
        format!("{val:.1}{unit}")
    }
}

/// Rough human elapsed time since a unix timestamp: `42s`, `17m`, `3h12m`, `4d6h`, `1.5y`.
pub fn ago(created_unix: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let s = (now - created_unix).max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else if s < 365 * 86_400 {
        format!("{}d{}h", s / 86_400, (s % 86_400) / 3600)
    } else {
        format!("{:.1}y", s as f64 / (365.0 * 86_400.0))
    }
}

/// Cluster memory denominator for the header. When every container reports the
/// same non-zero limit (the default: each cgroup is capped at host RAM), summing
/// the limits would report N× host RAM, so the shared limit is used instead;
/// otherwise the limits are summed.
pub fn cluster_mem(used: u64, limits: &[u64]) -> (u64, f64) {
    let distinct: std::collections::HashSet<u64> =
        limits.iter().copied().filter(|&l| l > 0).collect();
    let limit = if distinct.len() == 1 {
        *distinct.iter().next().unwrap()
    } else {
        limits.iter().sum()
    };
    let pct = if limit > 0 {
        used as f64 / limit as f64 * 100.0
    } else {
        0.0
    };
    (limit, pct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_small() {
        assert_eq!(bytes(0), "0B");
        assert_eq!(bytes(999), "999B");
    }

    #[test]
    fn bytes_scaled() {
        assert_eq!(bytes(1536), "1.54kB");
        assert_eq!(bytes(23_700_000), "23.7MB");
        assert_eq!(bytes(1_090_000_000), "1.09GB");
    }

    #[test]
    fn ago_buckets() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(ago(now - 42), "42s");
        assert_eq!(ago(now - 17 * 60), "17m");
        assert_eq!(ago(now - 3 * 3600 - 12 * 60), "3h12m");
        assert_eq!(ago(now - 26 * 3600), "1d2h");
    }

    #[test]
    fn cluster_mem_host_limited() {
        // all containers share the host-RAM limit: don't sum it
        let (limit, pct) = cluster_mem(2_000_000_000, &[15_000_000_000; 4]);
        assert_eq!(limit, 15_000_000_000);
        assert!((pct - 200.0 / 15.0).abs() < 1e-9);
    }

    #[test]
    fn cluster_mem_real_limits() {
        // distinct limits (e.g. --memory=1g) are summed
        let (limit, _) = cluster_mem(1_500_000_000, &[1_000_000_000, 2_000_000_000]);
        assert_eq!(limit, 3_000_000_000);
    }

    #[test]
    fn cluster_mem_zero_limit() {
        assert_eq!(cluster_mem(100, &[0, 0]), (0, 0.0));
        // zero limits are ignored; a single real one still counts as "shared"
        let (limit, pct) = cluster_mem(100, &[0, 5_000_000_000]);
        assert_eq!(limit, 5_000_000_000);
        assert!((pct - 2.0e-6).abs() < 1e-12);
    }
}
