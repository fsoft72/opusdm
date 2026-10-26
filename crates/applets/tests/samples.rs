//! Checks of the sample applets shipped in `applets/`: their manifests parse
//! and `site-news` turns a local fake feed into a list. Nothing here opens a
//! URL, shows a window or reaches the Internet.

use gtk4::glib::MainContext;
use opusdm_applets::manifest;
use opusdm_applets::runtime::{Event, Runtime, Sink};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Longest a test waits for something to happen on the main loop.
const WAIT: Duration = Duration::from_secs(10);

/// A fake RSS feed: a CDATA title, an entity title, an item without a link.
const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>Fake &amp; Site</title>
<item><title><![CDATA[Plain <b>CDATA</b> title]]></title><link>http://127.0.0.1/a</link></item>
<item><title>Tom &amp; Jerry &#39;s &#x27;day&#x27;</title><link>http://127.0.0.1/b</link></item>
<item><title>No link here</title></item>
</channel></rss>"#;

/// The repository folder `applets/`.
fn applets_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../applets")
}

/// Serve one HTTP answer (`status` line text and `body`) on a local port; returns the port.
fn serve_once(status: &'static str, body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let reply = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = stream.write_all(reply.as_bytes());
    });
    port
}

/// Every `Event::Ui` "show" tree sent so far.
type Trees = Rc<RefCell<Vec<Value>>>;

/// Start the real `site-news` applet with `feedUrl` and collect its shown trees.
fn start_site_news(feed_url: &str, trees: &Trees) -> Runtime {
    let dir = applets_root().join("site-news");
    let manifest = manifest::parse(&std::fs::read_to_string(dir.join("applet.json")).unwrap()).unwrap();
    let mut settings = Map::new();
    settings.insert("feedUrl".into(), json!(feed_url));
    settings.insert("maxItems".into(), json!(8));
    settings.insert("refreshMinutes".into(), json!(15));
    let trees = trees.clone();
    let sink: Sink = Rc::new(move |e| {
        if let Event::Ui(v) = e {
            if v["op"] == "show" {
                trees.borrow_mut().push(v["tree"].clone());
            }
        }
    });
    Runtime::start("site-news", &dir, &manifest, settings, sink).unwrap()
}

/// Every node of `tree`, depth first.
fn nodes(tree: &Value, out: &mut Vec<Value>) {
    out.push(tree.clone());
    for child in tree["children"].as_array().into_iter().flatten() {
        nodes(child, out);
    }
}

/// The texts of the items of the first `list` in `tree`, if there is one.
fn list_texts(tree: &Value) -> Option<Vec<String>> {
    let mut all = Vec::new();
    nodes(tree, &mut all);
    let list = all.iter().find(|n| n["type"] == "list")?;
    Some(list["items"].as_array()?.iter().map(|i| i["text"].as_str().unwrap_or_default().to_string()).collect())
}

/// The texts of every label in `tree`.
fn label_texts(tree: &Value) -> Vec<String> {
    let mut all = Vec::new();
    nodes(tree, &mut all);
    all.iter().filter(|n| n["type"] == "label").filter_map(|n| n["text"].as_str().map(str::to_string)).collect()
}

/// Iterate the thread-default main context until `done` or [`WAIT`] passes.
fn pump_until(done: impl Fn() -> bool) -> bool {
    let ctx = MainContext::ref_thread_default();
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if done() {
            return true;
        }
        if !ctx.iteration(false) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    done()
}

#[test]
fn the_sample_manifests_parse() {
    for id in ["site-news", "weather-clock"] {
        let path = applets_root().join(id).join("applet.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let m = manifest::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!m.name.is_empty());
        assert!(applets_root().join(id).join(&m.entry).is_file(), "{id}: missing entry script");
    }
}

#[test]
fn site_news_extracts_titles_and_links() {
    MainContext::new()
        .with_thread_default(|| {
            let port = serve_once("200 OK", FEED);
            let trees = Trees::default();
            let _rt = start_site_news(&format!("http://127.0.0.1:{port}/feed"), &trees);
            let got = pump_until(|| trees.borrow().iter().any(|t| list_texts(t).is_some()));
            assert!(got, "no list was shown: {:?}", trees.borrow());
            let tree = trees.borrow().iter().find(|t| list_texts(t).is_some()).unwrap().clone();
            assert_eq!(list_texts(&tree).unwrap(), ["Plain <b>CDATA</b> title", "Tom & Jerry 's 'day'"]);
            assert!(label_texts(&tree).iter().any(|t| t == "Fake & Site"), "channel title: {:?}", label_texts(&tree));
        })
        .unwrap();
}

#[test]
fn site_news_shows_a_message_on_http_error() {
    MainContext::new()
        .with_thread_default(|| {
            let port = serve_once("500 Internal Server Error", "oops");
            let trees = Trees::default();
            let _rt = start_site_news(&format!("http://127.0.0.1:{port}/feed"), &trees);
            let got = pump_until(|| trees.borrow().iter().any(|t| label_texts(t).iter().any(|l| l.contains("500"))));
            assert!(got, "no error label: {:?}", trees.borrow());
            assert!(trees.borrow().iter().all(|t| list_texts(t).is_none_or(|l| l.is_empty())));
        })
        .unwrap();
}
