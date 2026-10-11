//! Search in the text of a PDF: a worker thread that keeps the text of the
//! pages it already read, and the list of matches the window moves through.

use crate::pdf::open;
use crate::pdf_text::{extract, PageText};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Rect;
use std::path::PathBuf;
use std::sync::mpsc;

/// What the window wants found.
pub struct SearchRequest {
    pub generation: u64,
    pub path: PathBuf,
    pub query: String,
}

/// One match: its page (0-based) and its boxes (page points, y down), one per line.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub rects: Vec<Rect>,
}

/// Answer of the search worker: the generation of the request and the matches.
pub type SearchAnswer = (u64, Result<Vec<Hit>, String>);

/// The document the worker has open and the text of the pages read so far.
struct OpenDoc {
    path: PathBuf,
    pdf: Pdf,
    texts: Vec<Option<PageText>>,
}

/// Start the search worker. A newer request stops the search in progress; the
/// worker ends when the sender is dropped.
pub fn start_worker(answers: async_channel::Sender<SearchAnswer>) -> mpsc::Sender<SearchRequest> {
    let (tx, rx) = mpsc::channel::<SearchRequest>();
    std::thread::spawn(move || {
        let mut doc: Option<OpenDoc> = None;
        let mut pending = rx.recv().ok();
        while let Some(request) = pending.take() {
            let mut newer = None;
            let result = _search(&mut doc, &request, || {
                while let Ok(r) = rx.try_recv() { newer = Some(r); }
                newer.is_some()
            });
            if let Some(result) = result {
                if answers.send_blocking((request.generation, result)).is_err() { break; }
            }
            pending = newer.or_else(|| rx.recv().ok());
        }
    });
    tx
}

/// Look for `request.query` in every page; `None` when `stop` said a newer
/// request arrived.
fn _search(doc: &mut Option<OpenDoc>, request: &SearchRequest, mut stop: impl FnMut() -> bool) -> Option<Result<Vec<Hit>, String>> {
    if doc.as_ref().is_none_or(|d| d.path != request.path) {
        *doc = None;
        match open(&request.path) {
            Ok(pdf) => {
                let count = pdf.pages().len();
                *doc = Some(OpenDoc { path: request.path.clone(), pdf, texts: vec![None; count] });
            }
            Err(e) => return Some(Err(e)),
        }
    }
    let doc = doc.as_mut().expect("document opened above");
    let mut hits = Vec::new();
    for page in 0..doc.texts.len() {
        if stop() { return None; }
        let text = doc.texts[page].get_or_insert_with(|| extract(&doc.pdf, page));
        hits.extend(text.find(&request.query).into_iter().map(|rects| Hit { page, rects }));
    }
    Some(Ok(hits))
}

/// The matches of the last search and the one on screen.
#[derive(Debug, Default)]
pub struct Hits {
    list: Vec<Hit>,
    current: Option<usize>,
}

impl Hits {
    /// New matches; the current one is the first on `page` or after it (the
    /// first of all when none is after it).
    pub fn set(&mut self, list: Vec<Hit>, page: usize) {
        self.current = if list.is_empty() { None } else { Some(list.iter().position(|h| h.page >= page).unwrap_or(0)) };
        self.list = list;
    }

    /// No matches.
    pub fn clear(&mut self) {
        self.list.clear();
        self.current = None;
    }

    /// The match on screen.
    pub fn current(&self) -> Option<&Hit> {
        self.list.get(self.current?)
    }

    /// Move to the next (or previous) match, wrapping around.
    pub fn step(&mut self, forward: bool) -> Option<&Hit> {
        let len = self.list.len();
        let i = self.current?;
        self.current = Some(if forward { (i + 1) % len } else { (i + len - 1) % len });
        self.current()
    }

    /// Boxes of the matches on `page`, each with whether it is the current one.
    pub fn on_page(&self, page: usize) -> Vec<(Rect, bool)> {
        let mut out = Vec::new();
        for (i, hit) in self.list.iter().enumerate().filter(|(_, h)| h.page == page) {
            out.extend(hit.rects.iter().map(|r| (*r, Some(i) == self.current)));
        }
        out
    }

    /// `3/17`, `no matches`.
    pub fn label(&self) -> String {
        match self.current {
            Some(i) => format!("{}/{}", i + 1, self.list.len()),
            None => "no matches".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::pdf_with_contents;

    fn hit(page: usize) -> Hit {
        Hit { page, rects: vec![Rect::new(0.0, 0.0, 1.0, 1.0)] }
    }

    #[test]
    fn hits_start_on_the_current_page_and_wrap() {
        let mut hits = Hits::default();
        assert!(hits.current().is_none());
        assert_eq!(hits.label(), "no matches");
        hits.set(vec![hit(0), hit(2), hit(2), hit(5)], 1);
        assert_eq!(hits.current().unwrap().page, 2);
        assert_eq!(hits.label(), "2/4");
        assert_eq!(hits.step(true).unwrap().page, 2);
        assert_eq!(hits.step(true).unwrap().page, 5);
        assert_eq!(hits.step(true).unwrap().page, 0);
        assert_eq!(hits.step(false).unwrap().page, 5);
        hits.set(vec![hit(0), hit(1)], 7);
        assert_eq!(hits.label(), "1/2");
        hits.clear();
        assert!(hits.step(true).is_none());
    }

    #[test]
    fn on_page_marks_the_current_match() {
        let mut hits = Hits::default();
        hits.set(vec![hit(1), hit(1), hit(2)], 1);
        hits.step(true);
        let marks: Vec<bool> = hits.on_page(1).into_iter().map(|(_, current)| current).collect();
        assert_eq!(marks, vec![false, true]);
        assert!(hits.on_page(0).is_empty());
    }

    #[test]
    fn the_worker_finds_text_on_every_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        let data = pdf_with_contents(&["BT /F1 12 Tf 10 50 Td (alpha beta) Tj ET", "BT /F1 12 Tf 10 50 Td (Beta gamma) Tj ET"]);
        std::fs::write(&path, data).unwrap();
        let (answer_tx, answer_rx) = async_channel::unbounded();
        let requests = start_worker(answer_tx);
        requests.send(SearchRequest { generation: 3, path: path.clone(), query: "beta".to_string() }).unwrap();
        let (generation, result) = answer_rx.recv_blocking().unwrap();
        assert_eq!(generation, 3);
        let pages: Vec<usize> = result.unwrap().iter().map(|h| h.page).collect();
        assert_eq!(pages, vec![0, 1]);
        requests.send(SearchRequest { generation: 4, path, query: "delta".to_string() }).unwrap();
        assert_eq!(answer_rx.recv_blocking().unwrap(), (4, Ok(vec![])));
    }
}
