//! CPU and network monitors: a small text readout plus a sparkline of the
//! last minute, sampled from `/proc/stat` and `/proc/net/dev` once a second.

use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Sample interval and sparkline history length.
const TICK: Duration = Duration::from_secs(1);
const HISTORY: usize = 60;

/// Aggregate CPU times from the `cpu` line of `/proc/stat`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct CpuTimes {
    pub idle: u64,
    pub total: u64,
}

/// Parse the summary `cpu` line of `/proc/stat`. `idle` is `idle + iowait`;
/// `total` is the sum of every field. Returns `None` if the line is missing
/// or malformed.
pub fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|t| t.parse().ok())
        .collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    let total: u64 = nums.iter().sum();
    Some(CpuTimes { idle, total })
}

/// Busy percentage between two samples. Zero when the totals did not advance.
pub fn cpu_percent(prev: CpuTimes, now: CpuTimes) -> f64 {
    let dt = now.total.saturating_sub(prev.total);
    if dt == 0 {
        return 0.0;
    }
    let di = now.idle.saturating_sub(prev.idle);
    let busy = dt.saturating_sub(di) as f64;
    (busy / dt as f64 * 100.0).clamp(0.0, 100.0)
}

/// Sum received and transmitted bytes across every interface in
/// `/proc/net/dev` except loopback. Returns `(rx, tx)`.
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        if name == "lo" || name.is_empty() {
            continue;
        }
        let cols: Vec<u64> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        // Columns: rx bytes .. (0), then 8 more rx fields, then tx bytes (8).
        if cols.len() >= 9 {
            rx += cols[0];
            tx += cols[8];
        }
    }
    (rx, tx)
}

/// Human-readable transfer rate, e.g. `1.2M/s`, `340K/s`, `0B/s`.
pub fn human_rate(bytes_per_s: f64) -> String {
    let b = bytes_per_s.max(0.0);
    if b < 1024.0 {
        format!("{:.0}B/s", b)
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1}K/s", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1}M/s", b / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G/s", b / (1024.0 * 1024.0 * 1024.0))
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A fixed-length ring of recent samples, oldest first once full.
struct Trace {
    samples: Vec<f64>,
}

impl Trace {
    fn new() -> Self {
        Trace { samples: Vec::with_capacity(HISTORY) }
    }
    fn push(&mut self, v: f64) {
        if self.samples.len() == HISTORY {
            self.samples.remove(0);
        }
        self.samples.push(v);
    }
}

/// Draw `traces` as polylines into `area`, each scaled to `fixed_max` (or the
/// observed maximum when `None`), in the given RGB colours. Reinstalls the
/// draw function each call with a fresh copy of the samples.
fn draw_sparkline(
    area: &gtk::DrawingArea,
    traces: &[(&Trace, (f64, f64, f64))],
    fixed_max: Option<f64>,
) {
    let observed = traces
        .iter()
        .flat_map(|(t, _)| t.samples.iter().copied())
        .fold(0.0_f64, f64::max);
    let max = fixed_max.unwrap_or(observed).max(1e-6);
    let series: Vec<(Vec<f64>, (f64, f64, f64))> =
        traces.iter().map(|(t, c)| (t.samples.clone(), *c)).collect();

    area.set_draw_func(move |_, cr, width, height| {
        let w = width as f64;
        let h = height as f64;
        let step = w / (HISTORY - 1).max(1) as f64;
        for (samples, (r, g, b)) in &series {
            if samples.len() < 2 {
                continue;
            }
            cr.set_source_rgb(*r, *g, *b);
            cr.set_line_width(1.0);
            for (i, v) in samples.iter().enumerate() {
                let x = i as f64 * step;
                let y = h - (v / max).clamp(0.0, 1.0) * h;
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();
        }
    });
    area.queue_draw();
}

struct State {
    cpu_prev: CpuTimes,
    net_prev: (u64, u64),
    cpu_trace: Trace,
    rx_trace: Trace,
    tx_trace: Trace,
}

/// Build the combined CPU + network widget.
pub fn build() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    root.set_valign(gtk::Align::Center);

    let cpu_spark = gtk::DrawingArea::new();
    cpu_spark.set_content_width(44);
    cpu_spark.set_content_height(24);
    let cpu_label = gtk::Label::new(Some("CPU --%"));
    let cpu_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    cpu_box.append(&cpu_spark);
    cpu_box.append(&cpu_label);

    let net_spark = gtk::DrawingArea::new();
    net_spark.set_content_width(44);
    net_spark.set_content_height(24);
    let net_label = gtk::Label::new(None);
    net_label.set_markup("<small>\u{2193} --\n\u{2191} --</small>");
    net_label.set_xalign(0.0);
    let net_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    net_box.append(&net_spark);
    net_box.append(&net_label);

    root.append(&cpu_box);
    root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    root.append(&net_box);

    let state = Rc::new(RefCell::new(State {
        cpu_prev: parse_proc_stat(&read("/proc/stat")).unwrap_or_default(),
        net_prev: parse_net_dev(&read("/proc/net/dev")),
        cpu_trace: Trace::new(),
        rx_trace: Trace::new(),
        tx_trace: Trace::new(),
    }));

    gtk::glib::timeout_add_local(TICK, move || {
        let mut s = state.borrow_mut();

        if let Some(now) = parse_proc_stat(&read("/proc/stat")) {
            let pct = cpu_percent(s.cpu_prev, now);
            s.cpu_prev = now;
            s.cpu_trace.push(pct);
            cpu_label.set_text(&format!("CPU {pct:.0}%"));
            draw_sparkline(&cpu_spark, &[(&s.cpu_trace, (0.55, 0.79, 0.29))], Some(100.0));
        }

        let now = parse_net_dev(&read("/proc/net/dev"));
        let rx = now.0.saturating_sub(s.net_prev.0) as f64 / TICK.as_secs_f64();
        let tx = now.1.saturating_sub(s.net_prev.1) as f64 / TICK.as_secs_f64();
        s.net_prev = now;
        s.rx_trace.push(rx);
        s.tx_trace.push(tx);
        net_label.set_markup(&format!(
            "<small>\u{2193} {}\n\u{2191} {}</small>",
            human_rate(rx),
            human_rate(tx),
        ));
        draw_sparkline(
            &net_spark,
            &[
                (&s.rx_trace, (0.39, 0.62, 0.85)),
                (&s.tx_trace, (0.90, 0.55, 0.28)),
            ],
            None,
        );

        gtk::glib::ControlFlow::Continue
    });

    root.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_stat_reads_the_summary_line() {
        let text = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 10 0 5 80 5 0 0 0\n";
        let t = parse_proc_stat(text).unwrap();
        assert_eq!(t.idle, 850); // idle 800 + iowait 50
        assert_eq!(t.total, 1000); // 100 + 50 + 800 + 50
    }

    #[test]
    fn parse_proc_stat_rejects_garbage() {
        assert!(parse_proc_stat("nothing here").is_none());
        assert!(parse_proc_stat("cpu 1 2").is_none());
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
    fn parse_net_dev_sums_non_loopback() {
        let text = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets
    lo: 1000       10    0    0    0     0          0         0    1000       10
  eth0: 5000       50    0    0    0     0          0         0    2000       20
  wlan0: 100        1    0    0    0     0          0         0     50         1
";
        let (rx, tx) = parse_net_dev(text);
        assert_eq!(rx, 5100); // eth0 + wlan0, lo excluded
        assert_eq!(tx, 2050);
    }

    #[test]
    fn human_rate_scales_units() {
        assert_eq!(human_rate(0.0), "0B/s");
        assert_eq!(human_rate(512.0), "512B/s");
        assert_eq!(human_rate(2048.0), "2.0K/s");
        assert_eq!(human_rate(3.0 * 1024.0 * 1024.0), "3.0M/s");
    }
}
