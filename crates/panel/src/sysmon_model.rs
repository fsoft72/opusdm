//! Pure parsing and arithmetic behind the CPU and network monitors and their
//! details popups: `/proc/stat`, `/proc/loadavg`, `/proc/net/dev` and
//! `/proc/<pid>/stat`. No GTK here, so everything is unit tested.

use std::collections::HashMap;

/// CPU times of one `cpu` line of `/proc/stat`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct CpuTimes {
    pub idle: u64,
    pub total: u64,
}

/// Parse the numbers after the name of one `cpu` line. `idle` is
/// `idle + iowait`; `total` is the sum of every field. `None` when the line
/// holds fewer than four numbers.
fn _parse_cpu_line(line: &str) -> Option<CpuTimes> {
    let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|t| t.parse().ok()).collect();
    if nums.len() < 4 { return None; }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    Some(CpuTimes { idle, total: nums.iter().sum() })
}

/// Parse the summary `cpu` line of `/proc/stat`. Returns `None` if the line
/// is missing or malformed.
pub fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    _parse_cpu_line(text.lines().find(|l| l.starts_with("cpu "))?)
}

/// Parse the per core `cpuN` lines of `/proc/stat`, in file order.
/// Malformed lines are skipped.
pub fn parse_proc_stat_cores(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .filter_map(_parse_cpu_line)
        .collect()
}

/// Busy percentage between two samples. Zero when the totals did not advance.
pub fn cpu_percent(prev: CpuTimes, now: CpuTimes) -> f64 {
    let dt = now.total.saturating_sub(prev.total);
    if dt == 0 { return 0.0; }
    let di = now.idle.saturating_sub(prev.idle);
    let busy = dt.saturating_sub(di) as f64;
    (busy / dt as f64 * 100.0).clamp(0.0, 100.0)
}

/// Busy percentage of each core. A core missing from `prev` (one that just
/// came online) reads zero.
pub fn cores_percent(prev: &[CpuTimes], now: &[CpuTimes]) -> Vec<f64> {
    now.iter().enumerate().map(|(i, n)| prev.get(i).map_or(0.0, |p| cpu_percent(*p, *n))).collect()
}

/// The `/proc/loadavg` values.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
    /// Runnable scheduling entities right now.
    pub running: u32,
    /// Scheduling entities in the system.
    pub total: u32,
}

/// Parse `/proc/loadavg` (`0.52 0.58 0.59 2/1234 5678`).
pub fn parse_loadavg(text: &str) -> Option<LoadAvg> {
    let mut parts = text.split_whitespace();
    let one = parts.next()?.parse().ok()?;
    let five = parts.next()?.parse().ok()?;
    let fifteen = parts.next()?.parse().ok()?;
    let (running, total) = parts.next()?.split_once('/')?;
    Some(LoadAvg { one, five, fifteen, running: running.parse().ok()?, total: total.parse().ok()? })
}

/// Byte counters of one interface of `/proc/net/dev`.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct IfaceBytes {
    pub name: String,
    pub rx: u64,
    pub tx: u64,
}

/// The interfaces of `/proc/net/dev` except loopback, in file order.
pub fn parse_net_ifaces(text: &str) -> Vec<IfaceBytes> {
    let mut ifaces = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        if name == "lo" || name.is_empty() { continue; }
        let cols: Vec<u64> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        // Columns: rx bytes (0), then 7 more rx fields, then tx bytes (8).
        if cols.len() >= 9 {
            ifaces.push(IfaceBytes { name: name.to_string(), rx: cols[0], tx: cols[8] });
        }
    }
    ifaces
}

/// Sum received and transmitted bytes across every interface in
/// `/proc/net/dev` except loopback. Returns `(rx, tx)`.
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    parse_net_ifaces(text).iter().fold((0, 0), |(rx, tx), i| (rx + i.rx, tx + i.tx))
}

/// Rates and totals of one interface.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct IfaceRate {
    pub name: String,
    /// Bytes per second received and sent since the previous sample.
    pub rx_rate: f64,
    pub tx_rate: f64,
    /// Bytes received and sent since the interface came up.
    pub rx_total: u64,
    pub tx_total: u64,
}

/// Per interface rates over `secs` seconds. An interface missing from `prev`
/// (one that just appeared) reads zero.
pub fn iface_rates(prev: &[IfaceBytes], now: &[IfaceBytes], secs: f64) -> Vec<IfaceRate> {
    let secs = secs.max(1e-6);
    now.iter()
        .map(|n| {
            let p = prev.iter().find(|p| p.name == n.name);
            let rate = |now: u64, before: Option<u64>| before.map_or(0.0, |b| now.saturating_sub(b) as f64 / secs);
            IfaceRate {
                name: n.name.clone(),
                rx_rate: rate(n.rx, p.map(|p| p.rx)),
                tx_rate: rate(n.tx, p.map(|p| p.tx)),
                rx_total: n.rx,
                tx_total: n.tx,
            }
        })
        .collect()
}

/// Parse `/proc/<pid>/stat` into (command name, user + system clock ticks).
/// The name sits in parentheses and may hold spaces and parentheses itself.
pub fn parse_pid_stat(text: &str) -> Option<(String, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open { return None; }
    let name = text[open + 1..close].to_string();
    // After the name: state (0), ppid (1), ... utime (11), stime (12).
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((name, utime + stime))
}

/// One process in the busiest list.
#[derive(Clone, PartialEq, Debug)]
pub struct ProcUsage {
    pub pid: u32,
    pub name: String,
    /// Share of the whole machine (all cores) since the previous sample.
    pub percent: f64,
}

/// The `n` busiest processes between two samples, busiest first (ties by
/// pid). `total_delta` is how far the summary `cpu` total advanced. A
/// process missing from `prev` (just started) is left out, since its share
/// is unknown; idle ones are left out too.
pub fn top_processes(prev: &HashMap<u32, u64>, now: &HashMap<u32, (String, u64)>, total_delta: u64, n: usize) -> Vec<ProcUsage> {
    if total_delta == 0 { return Vec::new(); }
    let mut busy: Vec<ProcUsage> = now
        .iter()
        .filter_map(|(pid, (name, ticks))| {
            let delta = ticks.saturating_sub(*prev.get(pid)?);
            if delta == 0 { return None; }
            let percent = (delta as f64 / total_delta as f64 * 100.0).clamp(0.0, 100.0);
            Some(ProcUsage { pid: *pid, name: name.clone(), percent })
        })
        .collect();
    busy.sort_by(|a, b| b.percent.total_cmp(&a.percent).then(a.pid.cmp(&b.pid)));
    busy.truncate(n);
    busy
}

/// Read the CPU ticks of every process under `/proc`. Processes that exit
/// while being read are skipped.
pub fn read_processes() -> HashMap<u32, (String, u64)> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return HashMap::new() };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let text = std::fs::read_to_string(e.path().join("stat")).ok()?;
            Some((pid, parse_pid_stat(&text)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 10 0 5 80 5 0 0 0\ncpu1 20 0 0 70 10 0 0 0\nintr 1 2 3\n";

    #[test]
    fn parse_proc_stat_reads_the_summary_line() {
        let t = parse_proc_stat(STAT).unwrap();
        assert_eq!(t.idle, 850); // idle 800 + iowait 50
        assert_eq!(t.total, 1000); // 100 + 50 + 800 + 50
    }

    #[test]
    fn parse_proc_stat_rejects_garbage() {
        assert!(parse_proc_stat("nothing here").is_none());
        assert!(parse_proc_stat("cpu 1 2").is_none());
    }

    #[test]
    fn cores_are_read_in_order_without_the_summary() {
        let cores = parse_proc_stat_cores(STAT);
        assert_eq!(cores, vec![CpuTimes { idle: 85, total: 100 }, CpuTimes { idle: 80, total: 100 }]);
    }

    #[test]
    fn cpu_percent_is_busy_over_total() {
        let prev = CpuTimes { idle: 800, total: 1000 };
        let now = CpuTimes { idle: 900, total: 1200 };
        // total advanced 200, idle advanced 100 -> 50% busy
        assert!((cpu_percent(prev, now) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn cpu_percent_zero_when_stalled() {
        let s = CpuTimes { idle: 10, total: 20 };
        assert_eq!(cpu_percent(s, s), 0.0);
    }

    #[test]
    fn a_new_core_reads_zero() {
        let prev = [CpuTimes { idle: 0, total: 0 }];
        let now = [CpuTimes { idle: 50, total: 100 }, CpuTimes { idle: 10, total: 100 }];
        assert_eq!(cores_percent(&prev, &now), vec![50.0, 0.0]);
    }

    #[test]
    fn loadavg_is_parsed() {
        let l = parse_loadavg("0.52 0.58 1.25 2/1234 5678\n").unwrap();
        assert_eq!(l, LoadAvg { one: 0.52, five: 0.58, fifteen: 1.25, running: 2, total: 1234 });
        assert!(parse_loadavg("0.5 0.5").is_none());
        assert!(parse_loadavg("a b c 1/2").is_none());
    }

    const NET: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets
    lo: 1000       10    0    0    0     0          0         0    1000       10
  eth0: 5000       50    0    0    0     0          0         0    2000       20
  wlan0: 100        1    0    0    0     0          0         0     50         1
";

    #[test]
    fn parse_net_dev_sums_non_loopback() {
        let (rx, tx) = parse_net_dev(NET);
        assert_eq!(rx, 5100); // eth0 + wlan0, lo excluded
        assert_eq!(tx, 2050);
    }

    #[test]
    fn interfaces_are_listed_without_loopback() {
        let ifaces = parse_net_ifaces(NET);
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["eth0", "wlan0"]);
        assert_eq!((ifaces[0].rx, ifaces[0].tx), (5000, 2000));
    }

    #[test]
    fn interface_rates_over_the_interval() {
        let prev = vec![IfaceBytes { name: "eth0".into(), rx: 1000, tx: 500 }];
        let now = vec![
            IfaceBytes { name: "eth0".into(), rx: 3000, tx: 1500 },
            IfaceBytes { name: "wg0".into(), rx: 10, tx: 20 },
        ];
        let rates = iface_rates(&prev, &now, 2.0);
        assert_eq!(rates[0], IfaceRate { name: "eth0".into(), rx_rate: 1000.0, tx_rate: 500.0, rx_total: 3000, tx_total: 1500 });
        assert_eq!((rates[1].rx_rate, rates[1].tx_rate, rates[1].rx_total), (0.0, 0.0, 10));
    }

    #[test]
    fn pid_stat_handles_odd_names() {
        let text = "42 (my (odd) prog) S 1 42 42 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 1 0 100 0 0";
        assert_eq!(parse_pid_stat(text), Some(("my (odd) prog".to_string(), 10)));
        assert!(parse_pid_stat("42 (short) S 1").is_none());
        assert!(parse_pid_stat("garbage").is_none());
    }

    #[test]
    fn top_processes_busiest_first_and_skips_new_or_idle() {
        let prev = HashMap::from([(1, 100), (2, 100), (3, 100), (4, 50)]);
        let now = HashMap::from([
            (1, ("a".to_string(), 110)),
            (2, ("b".to_string(), 150)),
            (3, ("idle".to_string(), 100)),
            (4, ("c".to_string(), 60)),
            (5, ("new".to_string(), 999)),
        ]);
        let top = top_processes(&prev, &now, 200, 2);
        assert_eq!(top, vec![
            ProcUsage { pid: 2, name: "b".into(), percent: 25.0 },
            ProcUsage { pid: 1, name: "a".into(), percent: 5.0 },
        ]);
        assert!(top_processes(&prev, &now, 0, 5).is_empty());
    }

    #[test]
    fn the_live_proc_files_parse() {
        let stat = std::fs::read_to_string("/proc/stat").unwrap();
        assert!(parse_proc_stat(&stat).is_some());
        assert!(!parse_proc_stat_cores(&stat).is_empty());
        assert!(parse_loadavg(&std::fs::read_to_string("/proc/loadavg").unwrap()).is_some());
        assert!(read_processes().contains_key(&std::process::id()));
    }
}
