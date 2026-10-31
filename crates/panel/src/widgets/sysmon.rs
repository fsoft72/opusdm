//! CPU and network monitors: a small text readout plus a sparkline of the
//! last minute, sampled from `/proc/stat` and `/proc/net/dev` once a second.
//! A click on either meter opens a borderless details popup below it.

use crate::sysmon_model::{self as model, CpuTimes, IfaceBytes, IfaceRate, LoadAvg, ProcSample, ProcUsage};
use gtk4 as gtk;
use gtk::{gio, glib};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// Sample interval and sparkline history length.
const TICK: Duration = Duration::from_secs(1);
const HISTORY: usize = 60;
/// Busiest processes listed in the CPU details.
const TOP_PROCESSES: usize = 5;
/// Size of the graph in the details popups.
const DETAILS_GRAPH_W: i32 = 300;
const DETAILS_GRAPH_H: i32 = 80;

/// Human-readable transfer rate, e.g. `1.2M/s`, `340K/s`, `0B/s`.
pub fn human_rate(bytes_per_s: f64) -> String {
    opusdm_ipc::size::rate(bytes_per_s)
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A shared trace and its line color as (r, g, b).
type TraceColor = (Rc<RefCell<Trace>>, (f64, f64, f64));

/// A fixed-length ring of recent samples, oldest first.
struct Trace {
    samples: std::collections::VecDeque<f64>,
}

impl Trace {
    fn new() -> Self {
        Trace { samples: std::collections::VecDeque::with_capacity(HISTORY) }
    }
    fn push(&mut self, v: f64) {
        if self.samples.len() == HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back(v);
    }
}

/// Install `area`'s draw function once. The closure reads the shared traces
/// on every redraw, scaled to `fixed_max` (or the observed maximum when
/// `None`), so each tick only needs a `queue_draw` - no callback churn.
fn install_sparkline(
    area: &gtk::DrawingArea,
    traces: Vec<TraceColor>,
    fixed_max: Option<f64>,
) {
    area.set_draw_func(move |_, cr, width, height| {
        let w = width as f64;
        let h = height as f64;
        let step = w / (HISTORY - 1).max(1) as f64;
        let mut observed = 0.0_f64;
        for (t, _) in &traces {
            for v in t.borrow().samples.iter() {
                observed = observed.max(*v);
            }
        }
        let max = fixed_max.unwrap_or(observed).max(1e-6);
        for (trace, (r, g, b)) in &traces {
            let trace = trace.borrow();
            if trace.samples.len() < 2 {
                continue;
            }
            cr.set_source_rgb(*r, *g, *b);
            cr.set_line_width(1.0);
            for (i, v) in trace.samples.iter().enumerate() {
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
}

/// Width of the memory level bar left of the CPU graph, in pixels.
const MEM_BAR_W: i32 = 6;
/// Colour of the memory level.
const MEM_RGB: (f64, f64, f64) = (0.36, 0.60, 0.90);

/// Install the draw function of the memory level bar: a track filled from the
/// bottom as high as the percentage held in `level`.
fn install_mem_level(area: &gtk::DrawingArea, level: Rc<Cell<f64>>) {
    area.set_draw_func(move |_, cr, width, height| {
        let (w, h) = (width as f64, height as f64);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
        cr.rectangle(0.0, 0.0, w, h);
        let _ = cr.fill();
        let fill = (level.get() / 100.0).clamp(0.0, 1.0) * h;
        cr.set_source_rgb(MEM_RGB.0, MEM_RGB.1, MEM_RGB.2);
        cr.rectangle(0.0, h - fill, w, fill);
        let _ = cr.fill();
    });
}

struct State {
    cpu_prev: CpuTimes,
    cores_prev: Vec<CpuTimes>,
    net_prev: (u64, u64),
    ifaces_prev: Vec<IfaceBytes>,
    cpu_trace: Rc<RefCell<Trace>>,
    rx_trace: Rc<RefCell<Trace>>,
    tx_trace: Rc<RefCell<Trace>>,
}

/// The busiest processes shown in the CPU details. Sampled only while the
/// details are open, on a worker thread, since reading every process costs
/// more than the rest of a tick.
#[derive(Default)]
struct ProcWatch {
    /// The last sample, `None` until the first scan after opening.
    prev: Option<ProcSample>,
    /// The busiest processes between the last two samples.
    top: Option<Vec<ProcUsage>>,
    /// Whether a scan is running on a worker.
    busy: bool,
    /// Bumped by `reset`, so a scan started before it is dropped.
    generation: u64,
}

impl ProcWatch {
    /// Forget every sample (the details were opened or closed).
    fn reset(&mut self) {
        self.prev = None;
        self.top = None;
        self.generation += 1;
    }

    /// Fold in one finished scan started at `generation`. A scan older than
    /// the last `reset` only clears `busy`.
    fn fold(&mut self, generation: u64, now: ProcSample) {
        self.busy = false;
        if generation != self.generation { return; }
        if let Some(prev) = &self.prev {
            self.top = Some(top_between(prev, &now));
        }
        self.prev = Some(now);
    }
}

/// Start a process scan on a worker thread, unless one is already running.
fn scan_processes(watch: &Rc<RefCell<ProcWatch>>) {
    let generation = {
        let mut w = watch.borrow_mut();
        if w.busy { return; }
        w.busy = true;
        w.generation
    };
    let watch = watch.clone();
    glib::spawn_future_local(async move {
        let sample = gio::spawn_blocking(model::read_proc_sample).await.unwrap_or_default();
        watch.borrow_mut().fold(generation, sample);
    });
}

/// The busiest processes between two samples, busiest first.
fn top_between(prev: &ProcSample, now: &ProcSample) -> Vec<ProcUsage> {
    let total_delta = now.cpu_total.saturating_sub(prev.cpu_total);
    model::top_processes(&ticks_by_pid(&prev.procs), &now.procs, total_delta, TOP_PROCESSES)
}

/// Trace colours. CPU is one green line; network download ("in") is green,
/// upload ("out") is blue.
const CPU_RGB: (f64, f64, f64) = (0.55, 0.79, 0.29);
const NET_IN_RGB: (f64, f64, f64) = (0.45, 0.82, 0.38);
const NET_OUT_RGB: (f64, f64, f64) = (0.36, 0.60, 0.90);

/// Build a labelled sparkline "meter": a `DrawingArea` inside an `Overlay`,
/// with `.opusdm-panel-meter` for the card background. Returns both so the
/// caller can add its own overlaid labels and drive the drawing.
fn meter(width: i32) -> (gtk::Overlay, gtk::DrawingArea) {
    let area = gtk::DrawingArea::new();
    area.set_content_width(width);
    area.set_content_height(26);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_css_class("opusdm-panel-meter");
    overlay.set_valign(gtk::Align::Center);
    (overlay, area)
}

/// One value label overlaid on a meter, pinned to a top corner.
fn meter_label(align: gtk::Align, extra_class: Option<&str>) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("opusdm-panel-meter-val");
    if let Some(c) = extra_class {
        label.add_css_class(c);
    }
    label.set_halign(align);
    label.set_valign(gtk::Align::Start);
    label.set_margin_start(3);
    label.set_margin_end(3);
    label
}

/// Load average as shown in the CPU details.
pub fn format_load(load: &LoadAvg) -> String {
    format!("Load {:.2}  {:.2}  {:.2}    Tasks {}/{}", load.one, load.five, load.fifteen, load.running, load.total)
}

/// Highest value of a trace, zero when empty.
fn peak(trace: &Trace) -> f64 {
    trace.samples.iter().copied().fold(0.0, f64::max)
}

/// A small left aligned label, optionally with a CSS class.
fn text_label(text: &str, class: Option<&str>) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    if let Some(c) = class { label.add_css_class(c); }
    label
}

/// A borderless popup below `anchor`, opened by a left click on it. Its
/// content is `content`.
fn details_popover(anchor: &gtk::Overlay, content: &gtk::Box) -> gtk::Popover {
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_child(Some(content));
    popover.set_parent(anchor);
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    let p = popover.clone();
    click.connect_released(move |_, _, _, _| p.popup());
    anchor.add_controller(click);
    // A popover is not a child the anchor drops by itself.
    let p = popover.clone();
    anchor.connect_destroy(move |_| p.unparent());
    popover
}

/// The CPU details popup: graph, per core usage, load and busiest processes.
struct CpuDetails {
    popover: gtk::Popover,
    graph: gtk::DrawingArea,
    summary: gtk::Label,
    cores: gtk::Grid,
    load: gtk::Label,
    procs: gtk::Grid,
}

impl CpuDetails {
    /// Build the popup under `anchor`, drawing `trace`.
    fn new(anchor: &gtk::Overlay, trace: &Rc<RefCell<Trace>>) -> Self {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let graph = gtk::DrawingArea::new();
        graph.set_content_width(DETAILS_GRAPH_W);
        graph.set_content_height(DETAILS_GRAPH_H);
        graph.add_css_class("opusdm-panel-meter");
        install_sparkline(&graph, vec![(trace.clone(), CPU_RGB)], Some(100.0));
        let summary = text_label("CPU --%", Some("heading"));
        let cores = gtk::Grid::new();
        cores.set_column_spacing(8);
        cores.set_row_spacing(2);
        let load = text_label("", None);
        let procs = gtk::Grid::new();
        procs.set_column_spacing(12);
        content.append(&summary);
        content.append(&graph);
        content.append(&cores);
        content.append(&load);
        content.append(&text_label("Busiest processes", Some("heading")));
        content.append(&procs);
        let popover = details_popover(anchor, &content);
        Self { popover, graph, summary, cores, load, procs }
    }

    /// Show one sample: the total, each core, the load and the processes
    /// (`None` while they are still being measured).
    fn update(&self, total: f64, cores: &[f64], load: Option<LoadAvg>, procs: Option<&[ProcUsage]>) {
        self.summary.set_text(&format!("CPU {total:.0}%  ({} cores)", cores.len()));
        self.graph.queue_draw();
        _clear_grid(&self.cores);
        for (i, pct) in cores.iter().enumerate() {
            let (col, row) = ((i % 2) as i32 * 3, (i / 2) as i32);
            let bar = gtk::LevelBar::for_interval(0.0, 100.0);
            bar.set_value(*pct);
            bar.set_hexpand(true);
            bar.set_valign(gtk::Align::Center);
            self.cores.attach(&text_label(&format!("cpu{i}"), Some("dim-label")), col, row, 1, 1);
            self.cores.attach(&bar, col + 1, row, 1, 1);
            self.cores.attach(&text_label(&format!("{pct:>3.0}%"), Some("monospace")), col + 2, row, 1, 1);
        }
        self.load.set_text(&load.map(|l| format_load(&l)).unwrap_or_default());
        _clear_grid(&self.procs);
        let Some(procs) = procs else {
            self.procs.attach(&text_label("measuring...", Some("dim-label")), 0, 0, 1, 1);
            return;
        };
        for (row, p) in procs.iter().enumerate() {
            let row = row as i32;
            self.procs.attach(&text_label(&format!("{:.1}%", p.percent), Some("monospace")), 0, row, 1, 1);
            self.procs.attach(&text_label(&p.pid.to_string(), Some("dim-label")), 1, row, 1, 1);
            self.procs.attach(&text_label(&p.name, None), 2, row, 1, 1);
        }
    }
}

/// The network details popup: graph, rates and totals per interface, peaks.
struct NetDetails {
    popover: gtk::Popover,
    graph: gtk::DrawingArea,
    summary: gtk::Label,
    ifaces: gtk::Grid,
    peaks: gtk::Label,
}

impl NetDetails {
    /// Build the popup under `anchor`, drawing the download and upload traces.
    fn new(anchor: &gtk::Overlay, rx: &Rc<RefCell<Trace>>, tx: &Rc<RefCell<Trace>>) -> Self {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let graph = gtk::DrawingArea::new();
        graph.set_content_width(DETAILS_GRAPH_W);
        graph.set_content_height(DETAILS_GRAPH_H);
        graph.add_css_class("opusdm-panel-meter");
        install_sparkline(&graph, vec![(rx.clone(), NET_IN_RGB), (tx.clone(), NET_OUT_RGB)], None);
        let summary = text_label("Network", Some("heading"));
        let ifaces = gtk::Grid::new();
        ifaces.set_column_spacing(12);
        ifaces.set_row_spacing(2);
        let peaks = text_label("", Some("dim-label"));
        content.append(&summary);
        content.append(&graph);
        content.append(&ifaces);
        content.append(&peaks);
        let popover = details_popover(anchor, &content);
        Self { popover, graph, summary, ifaces, peaks }
    }

    /// Show one sample: the totals, every interface and the peaks of the
    /// last minute.
    fn update(&self, rx: f64, tx: f64, ifaces: &[IfaceRate], peaks: (f64, f64)) {
        self.summary.set_text(&format!("Network  \u{2193} {}  \u{2191} {}", human_rate(rx), human_rate(tx)));
        self.graph.queue_draw();
        _clear_grid(&self.ifaces);
        let header = ["Interface", "\u{2193} rate", "\u{2191} rate", "\u{2193} total", "\u{2191} total"];
        for (col, title) in header.iter().enumerate() {
            self.ifaces.attach(&text_label(title, Some("dim-label")), col as i32, 0, 1, 1);
        }
        for (row, i) in ifaces.iter().enumerate() {
            let row = row as i32 + 1;
            let cells = [
                (i.name.clone(), None),
                (human_rate(i.rx_rate), Some("opusdm-panel-net-in")),
                (human_rate(i.tx_rate), Some("opusdm-panel-net-out")),
                (opusdm_ipc::size::binary_size(i.rx_total), None),
                (opusdm_ipc::size::binary_size(i.tx_total), None),
            ];
            for (col, (text, class)) in cells.iter().enumerate() {
                self.ifaces.attach(&text_label(text, *class), col as i32, row, 1, 1);
            }
        }
        self.peaks.set_text(&format!("Peak last minute  \u{2193} {}  \u{2191} {}", human_rate(peaks.0), human_rate(peaks.1)));
    }
}

/// Remove every child of `grid`.
fn _clear_grid(grid: &gtk::Grid) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
}

/// Build the combined CPU + network widget.
pub fn build() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    root.set_valign(gtk::Align::Center);

    // CPU: the percentage sits inside the sparkline, pinned to the top-right.
    let (cpu_box, cpu_spark) = meter(56);
    let cpu_label = meter_label(gtk::Align::End, None);
    cpu_label.set_text("--%");
    cpu_box.add_overlay(&cpu_label);
    // Memory: one vertical level bar to the left of the CPU graph. The graph
    // moves into a box next to it, so the bar never covers the graph.
    let mem_bar = gtk::DrawingArea::new();
    mem_bar.set_content_width(MEM_BAR_W);
    mem_bar.set_margin_start(3);
    mem_bar.set_margin_end(3);
    mem_bar.set_margin_top(3);
    mem_bar.set_margin_bottom(3);
    cpu_box.set_child(None::<&gtk::Widget>);
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    inner.append(&mem_bar);
    inner.append(&cpu_spark);
    cpu_box.set_child(Some(&inner));
    cpu_box.set_tooltip_text(Some("CPU graph, memory level on the left - click for details"));

    // Network: two traces (green download, blue upload); the rates sit inside
    // the graph, download top-left, upload top-right.
    let (net_box, net_spark) = meter(120);
    let net_in = meter_label(gtk::Align::Start, Some("opusdm-panel-net-in"));
    let net_out = meter_label(gtk::Align::End, Some("opusdm-panel-net-out"));
    net_in.set_text("\u{2193} --");
    net_out.set_text("\u{2191} --");
    net_box.add_overlay(&net_in);
    net_box.add_overlay(&net_out);
    net_box.set_tooltip_text(Some("Network - click for details"));

    root.append(&cpu_box);
    root.append(&net_box);

    let cpu_trace = Rc::new(RefCell::new(Trace::new()));
    let rx_trace = Rc::new(RefCell::new(Trace::new()));
    let tx_trace = Rc::new(RefCell::new(Trace::new()));
    let mem_level = Rc::new(Cell::new(0.0_f64));
    install_mem_level(&mem_bar, mem_level.clone());

    // One draw function per meter, reading the shared traces; a tick only
    // queues a redraw.
    install_sparkline(&cpu_spark, vec![(cpu_trace.clone(), CPU_RGB)], Some(100.0));
    install_sparkline(
        &net_spark,
        vec![(rx_trace.clone(), NET_IN_RGB), (tx_trace.clone(), NET_OUT_RGB)],
        None,
    );

    let cpu_details = CpuDetails::new(&cpu_box, &cpu_trace);
    let net_details = NetDetails::new(&net_box, &rx_trace, &tx_trace);

    let stat = read("/proc/stat");
    let net = read("/proc/net/dev");
    let state = Rc::new(RefCell::new(State {
        cpu_prev: model::parse_proc_stat(&stat).unwrap_or_default(),
        cores_prev: model::parse_proc_stat_cores(&stat),
        net_prev: model::parse_net_dev(&net),
        ifaces_prev: model::parse_net_ifaces(&net),
        cpu_trace,
        rx_trace,
        tx_trace,
    }));

    // Opening the CPU details starts a first process sample, so a later one
    // can tell the busiest processes; closing drops them.
    let procs = Rc::new(RefCell::new(ProcWatch::default()));
    {
        let procs = procs.clone();
        cpu_details.popover.connect_show(move |_| {
            procs.borrow_mut().reset();
            scan_processes(&procs);
        });
    }
    {
        let procs = procs.clone();
        cpu_details.popover.connect_closed(move |_| procs.borrow_mut().reset());
    }

    // The timer stops once the bar is destroyed (a monitor change or a
    // screen resize rebuilds every bar), like the clock's.
    glib::timeout_add_local(TICK, move || {
        if cpu_label.root().is_none() { return glib::ControlFlow::Break; }
        let mut s = state.borrow_mut();

        let stat = read("/proc/stat");
        if let Some(now) = model::parse_proc_stat(&stat) {
            let pct = model::cpu_percent(s.cpu_prev, now);
            s.cpu_prev = now;
            s.cpu_trace.borrow_mut().push(pct);
            cpu_label.set_text(&format!("{pct:.0}%"));
            cpu_spark.queue_draw();

            let cores = model::parse_proc_stat_cores(&stat);
            if cpu_details.popover.is_visible() {
                scan_processes(&procs);
                let load = model::parse_loadavg(&read("/proc/loadavg"));
                let per_core = model::cores_percent(&s.cores_prev, &cores);
                cpu_details.update(pct, &per_core, load, procs.borrow().top.as_deref());
            }
            s.cores_prev = cores;
        }

        match model::parse_mem_percent(&read("/proc/meminfo")) {
            Some(pct) => {
                mem_level.set(pct);
                mem_bar.queue_draw();
            }
            None => opusdm_ipc::err_log!("opusdm-panel", "sysmon: /proc/meminfo has no usable MemTotal/MemAvailable"),
        }

        let net = read("/proc/net/dev");
        let now = model::parse_net_dev(&net);
        let rx = now.0.saturating_sub(s.net_prev.0) as f64 / TICK.as_secs_f64();
        let tx = now.1.saturating_sub(s.net_prev.1) as f64 / TICK.as_secs_f64();
        s.net_prev = now;
        s.rx_trace.borrow_mut().push(rx);
        s.tx_trace.borrow_mut().push(tx);
        net_in.set_text(&format!("\u{2193} {}", human_rate(rx)));
        net_out.set_text(&format!("\u{2191} {}", human_rate(tx)));
        net_spark.queue_draw();

        let ifaces = model::parse_net_ifaces(&net);
        if net_details.popover.is_visible() {
            let rates = model::iface_rates(&s.ifaces_prev, &ifaces, TICK.as_secs_f64());
            let peaks = (peak(&s.rx_trace.borrow()), peak(&s.tx_trace.borrow()));
            net_details.update(rx, tx, &rates, peaks);
        }
        s.ifaces_prev = ifaces;

        glib::ControlFlow::Continue
    });

    root.upcast()
}

/// Keep only the CPU ticks of each process.
fn ticks_by_pid(procs: &HashMap<u32, (String, u64)>) -> HashMap<u32, u64> {
    procs.iter().map(|(pid, (_, ticks))| (*pid, *ticks)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_keeps_the_last_history_samples_oldest_first() {
        let mut t = Trace::new();
        for i in 0..(HISTORY + 10) {
            t.push(i as f64);
        }
        assert_eq!(t.samples.len(), HISTORY);
        let got: Vec<f64> = t.samples.iter().copied().collect();
        let want: Vec<f64> = (10..(HISTORY + 10)).map(|i| i as f64).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn load_is_formatted_with_two_decimals_and_tasks() {
        let load = LoadAvg { one: 0.5, five: 1.25, fifteen: 2.0, running: 3, total: 400 };
        assert_eq!(format_load(&load), "Load 0.50  1.25  2.00    Tasks 3/400");
    }

    #[test]
    fn peak_is_the_highest_sample() {
        let mut t = Trace::new();
        assert_eq!(peak(&t), 0.0);
        for v in [3.0, 9.0, 1.0] { t.push(v); }
        assert_eq!(peak(&t), 9.0);
    }

    #[test]
    fn ticks_keep_the_pid() {
        let procs = HashMap::from([(7, ("x".to_string(), 42))]);
        assert_eq!(ticks_by_pid(&procs), HashMap::from([(7, 42)]));
    }

    #[test]
    fn top_between_uses_the_cpu_total_of_both_samples() {
        let prev = ProcSample { cpu_total: 1000, procs: HashMap::from([(1, ("a".to_string(), 10)), (2, ("b".to_string(), 50))]) };
        let now = ProcSample { cpu_total: 1200, procs: HashMap::from([(1, ("a".to_string(), 110)), (2, ("b".to_string(), 70))]) };
        let top = top_between(&prev, &now);
        assert_eq!(top.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![1, 2]);
        assert!((top[0].percent - 50.0).abs() < 1e-9);
        assert!((top[1].percent - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_scan_from_before_a_reset_is_dropped() {
        let sample = ProcSample { cpu_total: 10, procs: HashMap::from([(1, ("a".to_string(), 1))]) };
        let mut w = ProcWatch { busy: true, ..ProcWatch::default() };
        let stale = w.generation;
        w.reset();
        w.fold(stale, sample.clone());
        assert!(!w.busy);
        assert!(w.prev.is_none());
        w.fold(w.generation, sample.clone());
        assert!(w.prev.is_some() && w.top.is_none());
        w.fold(w.generation, sample);
        assert_eq!(w.top, Some(Vec::new()));
    }

    #[test]
    fn human_rate_scales_units() {
        assert_eq!(human_rate(0.0), "0B/s");
        assert_eq!(human_rate(512.0), "512B/s");
        assert_eq!(human_rate(2048.0), "2.0K/s");
        assert_eq!(human_rate(3.0 * 1024.0 * 1024.0), "3.0M/s");
    }
}
