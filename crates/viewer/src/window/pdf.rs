//! The PDF side of the viewer window: the bar (page panel button, search), the
//! page panel, the search matches drawn on the page and the links.

use super::{State, _name};
use crate::image_view::Landing;
use crate::keys::page_after;
use crate::keys::Action;
use crate::kind::MediaKind;
use crate::loader::RawImage;
use crate::pdf::RenderedPage;
use crate::pdf_bar::PdfBar;
use crate::pdf_links::{link_at, Link, LinkTarget};
use crate::pdf_pages::PagePanel;
use crate::pdf_search::{start_worker, Hits, SearchAnswer, SearchRequest};
use crate::pdf_text::{PageText, Selection};
use crate::text_doc::TextDoc;
use hayro::kurbo::Point;
use gtk4 as gtk;
use gtk::{gdk, gio, glib};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

/// Tint of the search matches, and of the current one.
const MATCH_COLOR: [u8; 3] = [255, 235, 60];
const CURRENT_MATCH_COLOR: [u8; 3] = [255, 140, 0];
/// Tint of the text picked with the mouse.
const SELECTION_COLOR: [u8; 3] = [140, 180, 255];
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
    /// Show the page being rendered from its bottom (the wheel went back past the top).
    land_bottom: Cell<bool>,
    /// The text of the page on screen, for the selection.
    text: RefCell<PageText>,
    /// The text picked with the mouse on the page on screen.
    selection: RefCell<Option<Selection>>,
    /// The character the drag in progress started on; `None` when no selecting drag runs.
    anchor: Cell<Option<usize>>,
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
    let ui = PdfUi { bar: PdfBar::new(), panel, search_tx: start_worker(search_answer_tx), search_generation: Cell::new(0), hits: RefCell::new(Hits::default()), clean: RefCell::new(None), scale: Cell::new(1.0), links: RefCell::new(Vec::new()), reveal_hit: Cell::new(false), land_bottom: Cell::new(false), text: RefCell::new(PageText::default()), selection: RefCell::new(None), anchor: Cell::new(None) };
    (ui, PdfChannels { page_pick_rx, search_rx })
}

impl State {
    /// Connect the bar, the panel, the search worker and the link controllers.
    pub(super) fn _wire_pdf(self: &Rc<Self>, channels: PdfChannels) {
        super::_listen(self, channels.page_pick_rx, |s, index| s._pdf_go_to(index, false));
        super::_listen(self, channels.search_rx, |s, (generation, result)| {
            if generation == s.pdf.search_generation.get() { s._search_done(result); }
        });
        let bar = &self.pdf.bar;
        let s = self.clone();
        bar.pages.connect_toggled(move |_| s._sync_pdf_panels());
        let s = self.clone();
        bar.page_prev.connect_clicked(move |_| s._turn_page(Action::PrevPage));
        let s = self.clone();
        bar.page_next.connect_clicked(move |_| s._turn_page(Action::NextPage));
        let s = self.clone();
        self.image.connect_page_edge(move |forward| s._pdf_edge(forward));
        let s = self.clone();
        bar.dark.connect_active_notify(move |sw| {
            s.markdown.set_dark(sw.is_active());
            s.source.set_dark(sw.is_active());
        });
        let s = self.clone();
        bar.zoom_out.connect_clicked(move |_| s._zoom_document(false));
        let s = self.clone();
        bar.zoom_in.connect_clicked(move |_| s._zoom_document(true));
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
            if let Some(doc) = s._text_doc() { doc.focus(); }
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
        self._wire_selection();
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
            let cursor = if target.is_some() { Some("pointer") } else if s._page_point(x, y).is_some() { Some("text") } else { None };
            picture.set_cursor_from_name(cursor);
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

    /// Drag on a PDF page picks the text between the character it started on
    /// and the one under the pointer (Shift+drag pans instead); Ctrl+C copies it.
    fn _wire_selection(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        let s = self.clone();
        drag.connect_drag_begin(move |gesture, x, y| {
            let shift = gesture.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            let anchor = if shift { None } else { s._page_point(x, y).and_then(|p| s.pdf.text.borrow().nearest(p)) };
            s.pdf.anchor.set(anchor);
            if anchor.is_some() { s._set_selection(None); }
        });
        let s = self.clone();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(anchor) = s.pdf.anchor.get() else { return };
            let Some((x, y)) = gesture.start_point() else { return };
            let Some(point) = s._page_point(x + dx, y + dy) else { return };
            let picked = s.pdf.text.borrow().nearest(point).and_then(|end| s.pdf.text.borrow().select(anchor, end));
            if *s.pdf.selection.borrow() != picked { s._set_selection(picked); }
        });
        let s = self.clone();
        drag.connect_drag_end(move |_, _, _| s.pdf.anchor.set(None));
        self.image.picture().add_controller(drag);
    }

    /// Replace the picked text and draw the page again with it tinted.
    fn _set_selection(&self, selection: Option<Selection>) {
        let changed = self.pdf.selection.borrow().is_some() || selection.is_some();
        *self.pdf.selection.borrow_mut() = selection;
        if changed { self._refresh_marks(false); }
    }

    /// The point of the PDF page (points, y down) under `(x, y)` of the
    /// picture widget; `None` when no PDF page is there.
    fn _page_point(&self, x: f64, y: f64) -> Option<Point> {
        if !self._is_pdf() || self.stack.visible_child_name().as_deref() != Some("image") { return None; }
        let p = self.image.image_point(x, y)?;
        let scale = self.pdf.scale.get() as f64;
        Some(Point::new(p.0 / scale, p.1 / scale))
    }

    /// Keys of the PDF side, before the normal ones: Ctrl+F, F3, and every key
    /// goes to the search field while it has the focus. `None` when the key is
    /// not one of them.
    pub(super) fn _pdf_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> Option<glib::Propagation> {
        if !self._is_pdf() && self._text_doc().is_none() { return None; }
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
        if ctrl && matches!(key, gdk::Key::c | gdk::Key::C) {
            let picked = self.pdf.selection.borrow().as_ref().map(|s| s.text.clone()).filter(|t| !t.is_empty())?;
            self.window.clipboard().set_text(&picked);
            return Some(glib::Propagation::Stop);
        }
        None
    }

    /// True while a PDF is on screen.
    fn _is_pdf(&self) -> bool {
        matches!(self._current(), Some((_, MediaKind::Pdf)))
    }

    /// True while a document shown one page at a time (PDF, comic) is on screen.
    fn _is_paged(&self) -> bool {
        self._current().is_some_and(|(_, kind)| kind.is_paged())
    }

    /// The Markdown text, EPUB book or source text on screen, if one is.
    fn _text_doc(&self) -> Option<&dyn TextDoc> {
        match self._current()?.1 {
            MediaKind::Markdown | MediaKind::Ebook => Some(self.markdown.as_ref()),
            MediaKind::Source => Some(self.source.as_ref()),
            _ => None,
        }
    }

    /// A new file is on screen: reset the PDF side; the bar shows only for a
    /// PDF, a comic or a text.
    pub(super) fn _pdf_file_changed(&self) {
        self.pdf.panel.clear();
        self.pdf.hits.borrow_mut().clear();
        self.pdf.search_generation.set(self.pdf.search_generation.get() + 1);
        self.pdf.bar.status.set_text("");
        *self.pdf.clean.borrow_mut() = None;
        self.pdf.links.borrow_mut().clear();
        self.pdf.reveal_hit.set(false);
        self.pdf.land_bottom.set(false);
        *self.pdf.text.borrow_mut() = PageText::default();
        *self.pdf.selection.borrow_mut() = None;
        self.pdf.anchor.set(None);
        self.image.set_select_drag(self._is_pdf());
        self.image.set_page_scroll(self._is_paged());
        self._sync_pdf_panels();
        self._sync_page_buttons();
    }

    /// Show or hide the bar and the page panel for the file on screen and the
    /// full screen state; start the thumbnails when the panel shows. A comic
    /// has the page widgets but no search.
    pub(super) fn _sync_pdf_panels(&self) {
        let paged = self._is_paged();
        let searchable = self._is_pdf() || self._text_doc().is_some();
        let bar = &self.pdf.bar;
        bar.pages.set_visible(paged);
        bar.page_prev.set_visible(paged);
        bar.page_next.set_visible(paged);
        bar.dark_box.set_visible(!paged);
        for widget in [bar.entry.upcast_ref::<gtk::Widget>(), bar.prev.upcast_ref(), bar.next.upcast_ref(), bar.status.upcast_ref()] {
            widget.set_visible(searchable);
        }
        let full = self.window.is_fullscreen();
        let searching = searchable && (!bar.entry.text().is_empty() || bar.entry_has_focus(&self.window));
        bar.root.set_visible((paged || searchable) && (!full || searching));
        let panel = paged && !full && bar.pages.is_active();
        self.pdf.panel.root.set_visible(panel);
        if panel { self.pdf.panel.start_thumbnails(); }
    }

    /// The zoom buttons of the bar: the page of a PDF or a comic, the font of a text.
    fn _zoom_document(&self, bigger: bool) {
        match (self._is_paged(), bigger) {
            (true, true) => self.image.zoom_in(),
            (true, false) => self.image.zoom_out(),
            (false, _) => self.font.change(bigger),
        }
        self._update_info();
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
        *self.pdf.text.borrow_mut() = page.text;
        *self.pdf.selection.borrow_mut() = None;
        self.pdf.anchor.set(None);
        *self.pdf.clean.borrow_mut() = Some(page.raw);
        if first {
            if let Some((path, _)) = self._current() { self.pdf.panel.load(&path, page.count); }
            self._sync_pdf_panels();
            if !self.pdf.bar.entry.text().is_empty() { self._start_search(); }
        }
        self.pdf.panel.set_current(self.page.get());
        self._sync_page_buttons();
        let Some(raw) = self._marked_page() else { return };
        self._picture_loaded(Ok(raw));
        if self.pdf.reveal_hit.replace(false) { self._reveal_current_hit(); }
    }

    /// Move to page `index` of the PDF on screen, shown from its top; `reveal`
    /// scrolls to the current match once the page is there.
    pub(super) fn _pdf_go_to(self: &Rc<Self>, index: usize, reveal: bool) {
        self._pdf_go_to_at(index, reveal, false);
    }

    /// The wheel went past the bottom (`forward`) or the top edge of the page:
    /// the next page shows from its top, the previous one from its bottom.
    fn _pdf_edge(self: &Rc<Self>, forward: bool) {
        let page = self.page.get();
        if !forward && page == 0 { return; }
        self._pdf_go_to_at(if forward { page + 1 } else { page - 1 }, false, !forward);
    }

    /// Where the scroll goes when the page being rendered arrives.
    pub(super) fn _pdf_landing(&self) -> Landing {
        if self.pdf.land_bottom.get() { Landing::Bottom } else { Landing::Top }
    }

    /// The previous / next page buttons work only when there is such a page.
    fn _sync_page_buttons(&self) {
        let (page, pages) = (self.page.get(), self.pages.get());
        self.pdf.bar.page_prev.set_sensitive(page > 0);
        self.pdf.bar.page_next.set_sensitive(page + 1 < pages);
    }

    /// [`Self::_pdf_go_to`]; `bottom` shows the page from its bottom.
    fn _pdf_go_to_at(self: &Rc<Self>, index: usize, reveal: bool, bottom: bool) {
        let Some((path, kind)) = self._current() else { return };
        if !kind.is_paged() { return; }
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
        self.pdf.land_bottom.set(bottom);
        self.pdf.panel.set_current(index);
        self._sync_page_buttons();
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
        let marks = [marks, self._selection_marks(scale)].concat();
        Some(if marks.is_empty() { clean.clone() } else { clean.highlighted(&marks) })
    }

    /// The boxes of the picked text in page pixels, tinted.
    fn _selection_marks(&self, scale: f64) -> Vec<crate::loader::Mark> {
        let selection = self.pdf.selection.borrow();
        let Some(selection) = selection.as_ref() else { return Vec::new() };
        selection.rects.iter().map(|r| ((r.x0 * scale, r.y0 * scale, r.x1 * scale, r.y1 * scale), SELECTION_COLOR)).collect()
    }

    /// Draw the marks again on the page on screen; `reveal` scrolls to the current match.
    fn _refresh_marks(&self, reveal: bool) {
        if self.stack.visible_child_name().as_deref() != Some("image") { return; }
        let Some(raw) = self._marked_page() else { return };
        self.image.replace_image(raw, Landing::Keep);
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
        if self._text_doc().is_some() { return self._text_search(); }
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

    /// Search the text on screen (Markdown or source) for the text of the search field.
    pub(super) fn _text_search(&self) {
        let Some(doc) = self._text_doc() else { return };
        let query = self.pdf.bar.entry.text().trim().to_string();
        doc.search(&query);
        self.pdf.bar.status.set_text(&if query.is_empty() { String::new() } else { doc.label() });
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
        if let Some(doc) = self._text_doc() {
            if doc.step(forward).is_some() { self.pdf.bar.status.set_text(&doc.label()); }
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
