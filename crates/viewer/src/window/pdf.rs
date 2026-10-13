//! The PDF side of the viewer window: the bar (page panel button, search), the
//! page panel, the search matches drawn on the page and the links.

use super::{State, _name};
use crate::keys::page_after;
use crate::keys::Action;
use crate::kind::MediaKind;
use crate::loader::RawImage;
use crate::pdf::RenderedPage;
use crate::pdf_bar::PdfBar;
use crate::pdf_links::{link_at, Link, LinkTarget};
use crate::pdf_pages::PagePanel;
use crate::pdf_search::{start_worker, Hits, SearchAnswer, SearchRequest};
use gtk4 as gtk;
use gtk::{gdk, gio, glib};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

/// Tint of the search matches, and of the current one.
const MATCH_COLOR: [u8; 3] = [255, 235, 60];
const CURRENT_MATCH_COLOR: [u8; 3] = [255, 140, 0];
/// Pixels added around a match box.
const MATCH_PAD: f64 = 1.0;

/// Widgets and state of the PDF side.
pub(super) struct PdfUi {
    pub(super) bar: PdfBar,
    pub(super) panel: Rc<PagePanel>,
    search_tx: mpsc::Sender<SearchRequest>,
    /// Bumped for every search; an answer for an older one is dropped.
    search_generation: Cell<u64>,
    hits: RefCell<Hits>,
    /// The page on screen without the match marks, the pixels per point it was
    /// rendered at and its links.
    clean: RefCell<Option<RawImage>>,
    scale: Cell<f32>,
    links: RefCell<Vec<Link>>,
    /// Scroll to the current match once the page being rendered arrives.
    reveal_hit: Cell<bool>,
}

/// Channels the window loops read for the PDF side.
pub(super) struct PdfChannels {
    page_pick_rx: async_channel::Receiver<usize>,
    search_rx: async_channel::Receiver<SearchAnswer>,
}

/// Build the PDF bar, the page panel and the search worker.
pub(super) fn build() -> (PdfUi, PdfChannels) {
    let (page_pick_tx, page_pick_rx) = async_channel::unbounded();
    let (search_answer_tx, search_rx) = async_channel::unbounded();
    let panel = PagePanel::new(page_pick_tx);
    panel.root.set_visible(false);
    let ui = PdfUi { bar: PdfBar::new(), panel, search_tx: start_worker(search_answer_tx), search_generation: Cell::new(0), hits: RefCell::new(Hits::default()), clean: RefCell::new(None), scale: Cell::new(1.0), links: RefCell::new(Vec::new()), reveal_hit: Cell::new(false) };
    (ui, PdfChannels { page_pick_rx, search_rx })
}

impl State {
    /// Connect the bar, the panel, the search worker and the link controllers.
    pub(super) fn _wire_pdf(self: &Rc<Self>, channels: PdfChannels) {
        let s = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(index) = channels.page_pick_rx.recv().await {
                s._pdf_go_to(index, false);
            }
        });
        let s = self.clone();
        glib::spawn_future_local(async move {
            while let Ok((generation, result)) = channels.search_rx.recv().await {
                if generation == s.pdf.search_generation.get() { s._search_done(result); }
            }
        });
        let bar = &self.pdf.bar;
        let s = self.clone();
        bar.pages.connect_toggled(move |_| s._sync_pdf_panels());
        let s = self.clone();
        bar.dark.connect_active_notify(move |sw| s.markdown.set_dark(sw.is_active()));
        let s = self.clone();
        bar.entry.connect_search_changed(move |_| s._start_search());
        let s = self.clone();
        bar.entry.connect_activate(move |_| s._step_hit(true));
        let s = self.clone();
        bar.entry.connect_next_match(move |_| s._step_hit(true));
        let s = self.clone();
        bar.entry.connect_previous_match(move |_| s._step_hit(false));
        let s = self.clone();
        bar.entry.connect_stop_search(move |e| {
            e.set_text("");
            gtk::prelude::GtkWindowExt::set_focus(&s.window, None::<&gtk::Widget>);
            s._sync_pdf_panels();
            if s._is_markdown() { s.markdown.focus(); }
        });
        let s = self.clone();
        bar.next.connect_clicked(move |_| s._step_hit(true));
        let s = self.clone();
        bar.prev.connect_clicked(move |_| s._step_hit(false));
        // Shift+Enter in the field: previous match.
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let s = self.clone();
        key.connect_key_pressed(move |_, k, _, mods| {
            let enter = matches!(k, gdk::Key::Return | gdk::Key::KP_Enter);
            if !(enter && mods.contains(gdk::ModifierType::SHIFT_MASK)) { return glib::Propagation::Proceed; }
            s._step_hit(false);
            glib::Propagation::Stop
        });
        bar.entry.add_controller(key);
        self._wire_links();
    }

    /// Click on a link follows it; over a link the pointer becomes a hand and
    /// the tooltip tells where it goes.
    fn _wire_links(self: &Rc<Self>) {
        let picture = self.image.picture();
        let click = gtk::GestureClick::new();
        let s = self.clone();
        click.connect_released(move |_, n, x, y| {
            if n != 1 { return; }
            let Some(target) = s._link_target(x, y) else { return };
            s._follow(target);
        });
        picture.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let s = self.clone();
        motion.connect_motion(move |_, x, y| {
            let target = s._link_target(x, y);
            let picture = s.image.picture();
            picture.set_cursor_from_name(target.as_ref().map(|_| "pointer"));
            let tip = target.map(|t| match t {
                LinkTarget::Page(i) => format!("Go to page {}", i + 1),
                LinkTarget::Uri(uri) => uri,
            });
            if picture.tooltip_text().map(|t| t.to_string()) != tip { picture.set_tooltip_text(tip.as_deref()); }
        });
        let s = self.clone();
        motion.connect_leave(move |_| s.image.picture().set_cursor_from_name(None));
        picture.add_controller(motion);
    }

    /// Keys of the PDF side, before the normal ones: Ctrl+F, F3, and every key
    /// goes to the search field while it has the focus. `None` when the key is
    /// not one of them.
    pub(super) fn _pdf_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> Option<glib::Propagation> {
        if !(self._is_pdf() || self._is_markdown()) { return None; }
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        if ctrl && matches!(key, gdk::Key::f | gdk::Key::F) {
            self.pdf.bar.root.set_visible(true);
            self.pdf.bar.entry.grab_focus();
            return Some(glib::Propagation::Stop);
        }
        if key == gdk::Key::F3 {
            self._step_hit(!mods.contains(gdk::ModifierType::SHIFT_MASK));
            return Some(glib::Propagation::Stop);
        }
        if self.pdf.bar.entry_has_focus(&self.window) { return Some(glib::Propagation::Proceed); }
        None
    }

    /// True while a PDF is on screen.
    fn _is_pdf(&self) -> bool {
        matches!(self._current(), Some((_, MediaKind::Pdf)))
    }

    /// True while a Markdown text is on screen.
    fn _is_markdown(&self) -> bool {
        matches!(self._current(), Some((_, MediaKind::Markdown)))
    }

    /// A new file is on screen: reset the PDF side; the bar shows only for a PDF.
    pub(super) fn _pdf_file_changed(&self) {
        self.pdf.panel.clear();
        self.pdf.hits.borrow_mut().clear();
        self.pdf.search_generation.set(self.pdf.search_generation.get() + 1);
        self.pdf.bar.status.set_text("");
        *self.pdf.clean.borrow_mut() = None;
        self.pdf.links.borrow_mut().clear();
        self.pdf.reveal_hit.set(false);
        self._sync_pdf_panels();
    }

    /// Show or hide the bar and the page panel for the file on screen and the
    /// full screen state; start the thumbnails when the panel shows.
    pub(super) fn _sync_pdf_panels(&self) {
        let pdf = self._is_pdf();
        let searchable = pdf || self._is_markdown();
        self.pdf.bar.pages.set_visible(pdf);
        self.pdf.bar.dark_box.set_visible(!pdf);
        let full = self.window.is_fullscreen();
        let searching = !self.pdf.bar.entry.text().is_empty() || self.pdf.bar.entry_has_focus(&self.window);
        self.pdf.bar.root.set_visible(searchable && (!full || searching));
        let panel = pdf && !full && self.pdf.bar.pages.is_active();
        self.pdf.panel.root.set_visible(panel);
        if panel { self.pdf.panel.start_thumbnails(); }
    }

    /// F9 / the Pages button: open or close the page panel.
    pub(super) fn _toggle_pages(&self) {
        let button = &self.pdf.bar.pages;
        button.set_active(!button.is_active());
    }

    /// A page answer from the worker, for the file on screen.
    pub(super) fn _pdf_page_loaded(self: &Rc<Self>, result: Result<RenderedPage, String>) {
        let page = match result {
            Ok(page) => page,
            Err(e) => return self._picture_loaded(Err(e)),
        };
        let first = self.pages.get() == 0;
        self.pages.set(page.count);
        self.pdf.scale.set(page.scale);
        *self.pdf.links.borrow_mut() = page.links;
        *self.pdf.clean.borrow_mut() = Some(page.raw);
        if first {
            if let Some((path, _)) = self._current() { self.pdf.panel.load(&path, page.count); }
            self._sync_pdf_panels();
            if !self.pdf.bar.entry.text().is_empty() { self._start_search(); }
        }
        self.pdf.panel.set_current(self.page.get());
        let Some(raw) = self._marked_page() else { return };
        self._picture_loaded(Ok(raw));
        if self.pdf.reveal_hit.replace(false) { self._reveal_current_hit(); }
    }

    /// Move to page `index` of the PDF on screen; `reveal` scrolls to the
    /// current match once the page is there.
    pub(super) fn _pdf_go_to(self: &Rc<Self>, index: usize, reveal: bool) {
        let Some((path, MediaKind::Pdf)) = self._current() else { return };
        if index >= self.pages.get() { return; }
        if index == self.page.get() {
            if reveal { self._refresh_marks(true); }
            return;
        }
        self.generation.set(self.generation.get() + 1);
        self._cancel_timer();
        self.page.set(index);
        self.keep_zoom.set(true);
        self.pdf.reveal_hit.set(reveal);
        self.pdf.panel.set_current(index);
        self._request_page(&path);
        self._update_info();
    }

    /// Page keys: Page Up / Down, Home, End.
    pub(super) fn _turn_page(self: &Rc<Self>, action: Action) {
        let Some(index) = page_after(action, self.page.get(), self.pages.get()) else { return };
        self._pdf_go_to(index, false);
    }

    /// The clean page with the matches of this page tinted.
    fn _marked_page(&self) -> Option<RawImage> {
        let clean = self.pdf.clean.borrow();
        let clean = clean.as_ref()?;
        let scale = self.pdf.scale.get() as f64;
        let marks: Vec<_> = self.pdf.hits.borrow().on_page(self.page.get()).into_iter().map(|(r, current)| {
            let rect = (r.x0 * scale - MATCH_PAD, r.y0 * scale - MATCH_PAD, r.x1 * scale + MATCH_PAD, r.y1 * scale + MATCH_PAD);
            (rect, if current { CURRENT_MATCH_COLOR } else { MATCH_COLOR })
        }).collect();
        Some(if marks.is_empty() { clean.clone() } else { clean.highlighted(&marks) })
    }

    /// Draw the marks again on the page on screen; `reveal` scrolls to the current match.
    fn _refresh_marks(&self, reveal: bool) {
        if self.stack.visible_child_name().as_deref() != Some("image") { return; }
        let Some(raw) = self._marked_page() else { return };
        self.image.replace_image(raw, false);
        if reveal { self._reveal_current_hit(); }
    }

    /// Scroll to the current match when it is on the page on screen.
    fn _reveal_current_hit(&self) {
        let hits = self.pdf.hits.borrow();
        let Some(hit) = hits.current().filter(|h| h.page == self.page.get()) else { return };
        let Some(r) = hit.rects.first() else { return };
        let scale = self.pdf.scale.get() as f64;
        self.image.reveal((r.x0 * scale, r.y0 * scale, r.x1 * scale, r.y1 * scale));
    }

    /// The text of the search field changed: start a new search (or clear the marks).
    fn _start_search(self: &Rc<Self>) {
        if self._is_markdown() { return self._markdown_search(); }
        let generation = self.pdf.search_generation.get() + 1;
        self.pdf.search_generation.set(generation);
        self.pdf.hits.borrow_mut().clear();
        let query = self.pdf.bar.entry.text().trim().to_string();
        let path = self._current().filter(|(_, k)| *k == MediaKind::Pdf).map(|(p, _)| p);
        let (Some(path), false) = (path, query.is_empty()) else {
            self.pdf.bar.status.set_text("");
            return self._refresh_marks(false);
        };
        self.pdf.bar.status.set_text("searching...");
        self._refresh_marks(false);
        if self.pdf.search_tx.send(SearchRequest { generation, path, query }).is_err() { self.pdf.bar.status.set_text("search stopped"); }
    }

    /// Search the Markdown text on screen for the text of the search field.
    pub(super) fn _markdown_search(&self) {
        let query = self.pdf.bar.entry.text().trim().to_string();
        self.markdown.search(&query);
        self.pdf.bar.status.set_text(&if query.is_empty() { String::new() } else { self.markdown.label() });
    }

    /// The search worker answered for the last search.
    fn _search_done(self: &Rc<Self>, result: Result<Vec<crate::pdf_search::Hit>, String>) {
        match result {
            Ok(list) => {
                self.pdf.hits.borrow_mut().set(list, self.page.get());
                self.pdf.bar.status.set_text(&self.pdf.hits.borrow().label());
                self._show_hit();
            }
            Err(e) => {
                let name = self._current().map(|(p, _)| _name(&p)).unwrap_or_default();
                opusdm_ipc::err_log!("viewer", "search in {name}: {e}");
                self.pdf.bar.status.set_text("search failed");
            }
        }
    }

    /// Move to the next (or previous) match.
    fn _step_hit(self: &Rc<Self>, forward: bool) {
        if self._is_markdown() {
            if self.markdown.step(forward).is_some() { self.pdf.bar.status.set_text(&self.markdown.label()); }
            return;
        }
        if self.pdf.hits.borrow_mut().step(forward).is_none() { return; }
        self.pdf.bar.status.set_text(&self.pdf.hits.borrow().label());
        self._show_hit();
    }

    /// Show the current match: go to its page, or mark it on this page.
    fn _show_hit(self: &Rc<Self>) {
        let page = self.pdf.hits.borrow().current().map(|h| h.page);
        match page {
            Some(page) if page != self.page.get() => self._pdf_go_to(page, true),
            Some(_) => self._refresh_marks(true),
            None => self._refresh_marks(false),
        }
    }

    /// Where the link under `(x, y)` (picture widget coordinates) leads.
    fn _link_target(&self, x: f64, y: f64) -> Option<LinkTarget> {
        if !self._is_pdf() || self.stack.visible_child_name().as_deref() != Some("image") { return None; }
        let p = self.image.image_point(x, y)?;
        let scale = self.pdf.scale.get() as f64;
        link_at(&self.pdf.links.borrow(), (p.0 / scale, p.1 / scale)).map(|l| l.target.clone())
    }

    /// Follow a link: another page, or the default application for a URI.
    fn _follow(self: &Rc<Self>, target: LinkTarget) {
        match target {
            LinkTarget::Page(index) => self._pdf_go_to(index, false),
            LinkTarget::Uri(uri) => {
                let shown = uri.clone();
                gtk::UriLauncher::new(&uri).launch(Some(&self.window), gio::Cancellable::NONE, move |result| {
                    if let Err(e) = result { opusdm_ipc::err_log!("viewer", "cannot open {shown}: {e}"); }
                });
            }
        }
    }
}
