//! The editor of one JavaScript script: the code, coloured with the shared
//! syntect highlighter and numbered in the gutter; a reference panel listing
//! the `opus` API and the internal commands (activating an entry inserts it);
//! and the output of the last run. Save writes the file, Run hands the text
//! as it is to the hub, so unsaved changes can be tried out.

use crate::scripts_ui::Host;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use opusdm_highlight::{self as highlight, Category};
use opusdm_ipc::function::{schema, InternalCmd};
use opusdm_ipc::scripts;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// Output tag of error lines.
pub const TAG_ERROR: &str = "out-error";
/// Output tag of the lines the window itself writes.
pub const TAG_DIM: &str = "out-dim";
/// Code tag of the line an error points at.
const TAG_ERROR_LINE: &str = "error-line";

/// Spaces a Tab inserts.
const INDENT: &str = "    ";

/// Delay between the last keystroke and the recolouring.
const RECOLOR_DELAY: Duration = Duration::from_millis(120);

/// Whether the GTK theme is dark.
fn is_dark() -> bool {
    gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme())
}

/// A read-only, monospaced output view and its buffer, with the
/// [`TAG_ERROR`] and [`TAG_DIM`] tags.
pub fn output_view() -> (gtk::TextView, gtk::TextBuffer) {
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_left_margin(6);
    let buffer = view.buffer();
    let (error, dim) = if is_dark() { ("#ff7b72", "#8b949e") } else { ("#b31d28", "#6a737d") };
    buffer.create_tag(Some(TAG_ERROR), &[("foreground", &error)]);
    buffer.create_tag(Some(TAG_DIM), &[("foreground", &dim)]);
    // Follow the output as it arrives.
    let end = buffer.create_mark(Some("end"), &buffer.end_iter(), false);
    let weak = view.downgrade();
    buffer.connect_changed(move |_| {
        if let Some(view) = weak.upgrade() {
            view.scroll_mark_onscreen(&end);
        }
    });
    (view, buffer)
}

/// Append `text` to `buffer`, with `tag` when given.
pub fn append(buffer: &gtk::TextBuffer, text: &str, tag: Option<&str>) {
    let mut end = buffer.end_iter();
    match tag {
        Some(tag) => buffer.insert_with_tags_by_name(&mut end, text, &[tag]),
        None => buffer.insert(&mut end, text),
    }
}

/// The 1-based line and column an engine error points at: `(name.js:3:7)`
/// for a runtime error, `at line 3, col 7` for a syntax error.
pub fn error_position(text: &str) -> Option<(u32, u32)> {
    let number = |s: &str| -> Option<(u32, usize)> {
        let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        Some((digits.parse().ok()?, digits.len()))
    };
    if let Some(at) = text.rfind(".js:") {
        let rest = &text[at + 4..];
        if let Some((line, len)) = number(rest) {
            let col = rest[len..].strip_prefix(':').and_then(number).map(|(c, _)| c).unwrap_or(1);
            return Some((line, col));
        }
    }
    let at = text.find("at line ")?;
    let rest = &text[at + 8..];
    let (line, len) = number(rest)?;
    let col = rest[len..].strip_prefix(", col ").and_then(number).map(|(c, _)| c).unwrap_or(1);
    Some((line, col))
}

/// Where the error `text` points in `code`, 1-based. Boa gives a stale
/// position for "x is not defined" (the last call made, or none at all), so
/// that error points at the first use of `x` instead.
pub fn error_location(text: &str, code: &str) -> Option<(u32, u32)> {
    if let Some(name) = undefined_name(text) {
        if let Some(found) = find_word(code, name) {
            return Some(found);
        }
    }
    error_position(text)
}

/// The identifier of a "ReferenceError: x is not defined".
fn undefined_name(text: &str) -> Option<&str> {
    let rest = &text[text.find("ReferenceError: ")? + "ReferenceError: ".len()..];
    let name = &rest[..rest.find(" is not defined")?];
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')).then_some(name)
}

/// The first place `word` appears as a whole identifier in `code`, outside
/// `//` comments: 1-based line and column (in characters).
fn find_word(code: &str, word: &str) -> Option<(u32, u32)> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    for (index, line) in code.lines().enumerate() {
        let code_part = line.split("//").next().unwrap_or("");
        let mut from = 0;
        while let Some(at) = code_part[from..].find(word) {
            let start = from + at;
            let end = start + word.len();
            let before_ok = !code_part[..start].chars().next_back().is_some_and(is_ident);
            let after_ok = !code_part[end..].chars().next().is_some_and(is_ident);
            if before_ok && after_ok {
                return Some((index as u32 + 1, code_part[..start].chars().count() as u32 + 1));
            }
            from = end;
        }
    }
    None
}

/// The indentation the line after `before` (the text of the current line up
/// to the cursor) starts with: the same as it, one level more after an
/// opening bracket.
pub fn next_indent(before: &str) -> String {
    let indent: String = before.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    if before.trim_end().ends_with(['{', '[', '(']) {
        format!("{indent}{INDENT}")
    } else {
        indent
    }
}

/// The text the reference panel inserts for internal command `cmd`: a
/// `opus.command` call with its required parameters.
pub fn command_snippet(cmd: InternalCmd) -> String {
    let required: Vec<String> = schema::params_for(cmd).iter().filter(|p| p.required).map(|p| format!("{}: \"\"", p.key)).collect();
    if required.is_empty() {
        format!("opus.command(\"{cmd:?}\")")
    } else {
        format!("opus.command(\"{cmd:?}\", {{ {} }})", required.join(", "))
    }
}

/// One script open for editing.
pub struct Editor {
    host: Weak<Host>,
    name: RefCell<String>,
    pub window: gtk::Window,
    view: gtk::TextView,
    buffer: gtk::TextBuffer,
    numbers: opusdm_ui::line_numbers::LineNumbers,
    output: gtk::TextBuffer,
    args: gtk::Entry,
    status: gtk::Label,
    run_btn: gtk::Button,
    /// The text as last saved, to know whether there are changes.
    saved: RefCell<String>,
    /// Bumped on every edit; a recolouring runs only for the latest one.
    generation: Cell<u64>,
    /// Closing without asking (the script was deleted).
    discard: Cell<bool>,
}

impl Editor {
    /// Open an editor window on script `name` whose text is `code`.
    pub fn open(host: &Rc<Host>, app: &gtk::Application, name: &str, code: &str) -> Rc<Editor> {
        let window = gtk::Window::builder().application(app).default_width(980).default_height(680).build();
        window.add_css_class(opusdm_ui::REQUESTER_CLASS);

        let view = gtk::TextView::new();
        view.set_monospace(true);
        view.set_left_margin(6);
        view.set_top_margin(4);
        view.set_wrap_mode(gtk::WrapMode::None);
        view.set_accepts_tab(false);
        let buffer = view.buffer();
        create_code_tags(&buffer);
        buffer.set_text(code);
        buffer.place_cursor(&buffer.start_iter());
        let numbers = opusdm_ui::line_numbers::LineNumbers::attach(&view);
        numbers.set_line_count(buffer.line_count());
        let code_scroller = gtk::ScrolledWindow::builder().child(&view).hexpand(true).vexpand(true).build();

        let (output_view, output) = output_view();
        let output_scroller = gtk::ScrolledWindow::builder().child(&output_view).build();
        let output_frame = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let output_label = gtk::Label::new(Some("Output"));
        output_label.set_xalign(0.0);
        output_label.add_css_class("dim-label");
        output_frame.append(&output_label);
        output_frame.append(&output_scroller);
        output_scroller.set_vexpand(true);

        let output_box = gtk::Paned::new(gtk::Orientation::Vertical);
        output_box.set_start_child(Some(&code_scroller));
        output_box.set_end_child(Some(&output_frame));
        output_box.set_resize_end_child(false);
        output_box.set_shrink_end_child(true);
        output_box.set_position(500);

        let reference = reference_panel();
        let main = gtk::Paned::new(gtk::Orientation::Horizontal);
        main.set_start_child(Some(&output_box));
        main.set_end_child(Some(&reference.root));
        main.set_resize_end_child(false);
        main.set_shrink_end_child(true);
        main.set_position(660);
        main.set_vexpand(true);

        // Toolbar: Save, Run with its arguments, Clear; the cursor position.
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.set_margin_start(8);
        bar.set_margin_end(8);
        bar.set_margin_top(6);
        bar.set_margin_bottom(6);
        let save_btn = gtk::Button::with_label("Save");
        save_btn.set_tooltip_text(Some("Save the script (Ctrl+S)"));
        let run_btn = gtk::Button::with_label("Run");
        run_btn.set_tooltip_text(Some("Run the script as it is in the editor (F5)"));
        let args = gtk::Entry::new();
        args.set_placeholder_text(Some("opus.args for Run ({f}, {o}, ... allowed)"));
        args.set_hexpand(true);
        let clear_btn = gtk::Button::with_label("Clear Output");
        let status = gtk::Label::new(None);
        status.add_css_class("dim-label");
        status.set_width_chars(16);
        status.set_xalign(1.0);
        bar.append(&save_btn);
        bar.append(&run_btn);
        bar.append(&args);
        bar.append(&clear_btn);
        bar.append(&status);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&bar);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&main);
        window.set_child(Some(&root));

        let editor = Rc::new(Editor {
            host: Rc::downgrade(host),
            name: RefCell::new(name.to_string()),
            window: window.clone(),
            view: view.clone(),
            buffer: buffer.clone(),
            numbers,
            output,
            args,
            status,
            run_btn: run_btn.clone(),
            saved: RefCell::new(code.to_string()),
            generation: Cell::new(0),
            discard: Cell::new(false),
        });
        editor.update_title();
        editor.update_status();
        editor.recolor();

        {
            let weak = Rc::downgrade(&editor);
            buffer.connect_changed(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.changed();
                }
            });
        }
        {
            let weak = Rc::downgrade(&editor);
            buffer.connect_cursor_position_notify(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.update_status();
                }
            });
        }
        {
            let weak = Rc::downgrade(&editor);
            save_btn.connect_clicked(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.save();
                }
            });
        }
        {
            let weak = Rc::downgrade(&editor);
            run_btn.connect_clicked(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.run();
                }
            });
        }
        {
            let output = editor.output.clone();
            clear_btn.connect_clicked(move |_| output.set_text(""));
        }
        {
            let weak = Rc::downgrade(&editor);
            reference.on_insert(move |snippet| {
                if let Some(editor) = weak.upgrade() {
                    editor.buffer.insert_at_cursor(snippet);
                    editor.view.grab_focus();
                }
            });
        }
        editor.install_keys();
        {
            let weak = Rc::downgrade(&editor);
            window.connect_close_request(move |_| match weak.upgrade() {
                Some(editor) => editor.close_request(),
                None => gtk::glib::Propagation::Proceed,
            });
        }

        window.present();
        view.grab_focus();
        editor
    }

    /// The script's name.
    pub fn name(&self) -> String {
        self.name.borrow().clone()
    }

    /// The script was renamed to `name`.
    pub fn set_name(&self, name: &str) {
        *self.name.borrow_mut() = name.to_string();
        self.update_title();
    }

    /// The text in the editor.
    pub fn text(&self) -> String {
        self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), false).to_string()
    }

    /// Whether the text differs from the saved file.
    pub fn is_dirty(&self) -> bool {
        *self.saved.borrow() != self.text()
    }

    /// Enable Run (`on`), with `tip` saying why it is not.
    pub fn set_can_run(&self, on: bool, tip: Option<&str>) {
        self.run_btn.set_sensitive(on);
        self.run_btn.set_tooltip_text(Some(tip.unwrap_or("Run the script as it is in the editor (F5)")));
    }

    /// Close without asking about unsaved changes.
    pub fn close_discarding(&self) {
        self.discard.set(true);
        self.window.close();
    }

    fn update_title(&self) {
        let dirty = if self.is_dirty() { "*" } else { "" };
        self.window.set_title(Some(&format!("{dirty}{}.{} - Script Editor", self.name.borrow(), scripts::EXTENSION)));
    }

    fn update_status(&self) {
        let iter = self.buffer.iter_at_offset(self.buffer.cursor_position());
        self.status.set_text(&format!("Ln {}, Col {}", iter.line() + 1, iter.line_offset() + 1));
    }

    /// The text changed: title, line numbers, error mark, and a recolouring
    /// once the typing pauses.
    fn changed(self: &Rc<Self>) {
        self.update_title();
        self.numbers.set_line_count(self.buffer.line_count());
        let (start, end) = self.buffer.bounds();
        self.buffer.remove_tag_by_name(TAG_ERROR_LINE, &start, &end);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        gtk::glib::timeout_add_local_once(RECOLOR_DELAY, move || {
            if let Some(editor) = weak.upgrade() {
                if editor.generation.get() == generation {
                    editor.recolor();
                }
            }
        });
    }

    /// Colour the whole text again.
    fn recolor(&self) {
        let (start, end) = self.buffer.bounds();
        for category in Category::ALL {
            self.buffer.remove_tag_by_name(category.tag_name(), &start, &end);
        }
        let Some(syntax) = highlight::syntax_for("script.js", "") else { return };
        let text = self.text();
        highlight::highlight_in_batches(&text, syntax, usize::MAX, |spans| {
            for span in spans {
                let a = self.buffer.iter_at_offset(span.start as i32);
                let b = self.buffer.iter_at_offset(span.end as i32);
                self.buffer.apply_tag_by_name(span.category.tag_name(), &a, &b);
            }
            true
        });
    }

    /// Write the text to the script file.
    fn save(&self) -> bool {
        let text = self.text();
        let name = self.name();
        match scripts::save(&name, &text) {
            Ok(()) => {
                *self.saved.borrow_mut() = text;
                self.update_title();
                if let Some(host) = self.host.upgrade() {
                    host.reload();
                }
                true
            }
            Err(e) => {
                opusdm_ui::requester(&self.window, "Save Script", &format!("cannot save \"{name}\": {e}"), None, &[("OK", ())], |_| {});
                false
            }
        }
    }

    /// Run the text as it is.
    fn run(self: &Rc<Self>) {
        let Some(host) = self.host.upgrade() else { return };
        if !self.run_btn.is_sensitive() {
            return;
        }
        let args = self.args.text().to_string();
        host.run(&self.name(), Some(self.text()), &args, self.output.clone(), Some(self));
    }

    /// The run failed with `error`: put the cursor on the line it names.
    pub fn show_error(&self, error: &str) {
        let Some((line, col)) = error_location(error, &self.text()) else { return };
        let Some(mut iter) = self.buffer.iter_at_line(line.saturating_sub(1) as i32) else { return };
        let mut end = iter;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        self.buffer.apply_tag_by_name(TAG_ERROR_LINE, &iter, &end);
        let max = end.line_offset();
        iter.set_line_offset((col.saturating_sub(1) as i32).min(max));
        self.buffer.place_cursor(&iter);
        self.view.scroll_to_mark(&self.buffer.get_insert(), 0.2, false, 0.0, 0.0);
        self.view.grab_focus();
    }

    /// The window is closing: ask about unsaved changes.
    fn close_request(self: &Rc<Self>) -> gtk::glib::Propagation {
        if self.discard.get() || !self.is_dirty() {
            if let Some(host) = self.host.upgrade() {
                host.editor_closed(self);
            }
            return gtk::glib::Propagation::Proceed;
        }
        let editor = self.clone();
        let text = format!("\"{}\" has unsaved changes.", self.name());
        opusdm_ui::requester(&self.window, "Close Script", &text, None, &[("Save", 1), ("Discard", 2), ("Cancel", 0)], move |choice| match choice {
            1 => {
                if editor.save() {
                    editor.close_discarding();
                }
            }
            2 => editor.close_discarding(),
            _ => {}
        });
        gtk::glib::Propagation::Stop
    }

    /// Ctrl+S, F5 / Ctrl+Enter, Tab / Shift+Tab, Enter with indentation,
    /// Ctrl+/ to comment lines.
    fn install_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(editor) = weak.upgrade() else { return gtk::glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let handled = match key {
                gdk::Key::s | gdk::Key::S if ctrl => {
                    editor.save();
                    true
                }
                gdk::Key::F5 => {
                    editor.run();
                    true
                }
                gdk::Key::Return | gdk::Key::KP_Enter if ctrl => {
                    editor.run();
                    true
                }
                gdk::Key::slash if ctrl => {
                    editor.toggle_comment();
                    true
                }
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab if !ctrl => {
                    editor.indent(!(shift || key == gdk::Key::ISO_Left_Tab));
                    true
                }
                gdk::Key::Return | gdk::Key::KP_Enter if !shift => {
                    editor.newline();
                    true
                }
                _ => false,
            };
            if handled { gtk::glib::Propagation::Stop } else { gtk::glib::Propagation::Proceed }
        });
        self.view.add_controller(keys);
        // Ctrl+S also works with the focus in the arguments field.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(editor) = weak.upgrade() else { return gtk::glib::Propagation::Proceed };
            match key {
                gdk::Key::s | gdk::Key::S if state.contains(gdk::ModifierType::CONTROL_MASK) => {
                    editor.save();
                    gtk::glib::Propagation::Stop
                }
                gdk::Key::F5 => {
                    editor.run();
                    gtk::glib::Propagation::Stop
                }
                _ => gtk::glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(keys);
    }

    /// Enter: a new line indented like the current one (more after `{`).
    fn newline(&self) {
        let buffer = &self.buffer;
        buffer.delete_selection(true, true);
        let cursor = buffer.iter_at_offset(buffer.cursor_position());
        let mut line_start = cursor;
        line_start.set_line_offset(0);
        let before = buffer.text(&line_start, &cursor, false);
        let mut indent = next_indent(&before);
        // Between `{` and `}`: the closing bracket goes on its own line.
        let mut after = cursor;
        after.forward_char();
        let next = buffer.text(&cursor, &after, false);
        buffer.begin_user_action();
        if before.trim_end().ends_with(['{', '[', '(']) && matches!(next.as_str(), "}" | "]" | ")") {
            let outer = indent.strip_suffix(INDENT).unwrap_or("").to_string();
            buffer.insert_at_cursor(&format!("\n{indent}"));
            let mark = buffer.create_mark(None, &buffer.iter_at_offset(buffer.cursor_position()), true);
            buffer.insert_at_cursor(&format!("\n{outer}"));
            buffer.place_cursor(&buffer.iter_at_mark(&mark));
            buffer.delete_mark(&mark);
            indent.clear();
        } else {
            buffer.insert_at_cursor(&format!("\n{indent}"));
        }
        buffer.end_user_action();
        self.view.scroll_to_mark(&buffer.get_insert(), 0.0, false, 0.0, 0.0);
    }

    /// The lines the selection (or the cursor) is on.
    fn selected_lines(&self) -> (i32, i32) {
        let buffer = &self.buffer;
        match buffer.selection_bounds() {
            Some((start, end)) => {
                // A selection ending at the start of a line does not take it.
                let last = if end.line_offset() == 0 && end.line() > start.line() { end.line() - 1 } else { end.line() };
                (start.line(), last)
            }
            None => {
                let line = buffer.iter_at_offset(buffer.cursor_position()).line();
                (line, line)
            }
        }
    }

    /// Tab: indent (`more`) or unindent. Without a selection spanning lines,
    /// Tab inserts spaces at the cursor.
    fn indent(&self, more: bool) {
        let buffer = &self.buffer;
        let multi = buffer.selection_bounds().is_some_and(|(a, b)| a.line() != b.line());
        if more && !multi {
            buffer.delete_selection(true, true);
            let col = buffer.iter_at_offset(buffer.cursor_position()).line_offset() as usize;
            let spaces = INDENT.len() - col % INDENT.len();
            buffer.insert_at_cursor(&" ".repeat(spaces));
            return;
        }
        let (first, last) = self.selected_lines();
        buffer.begin_user_action();
        for line in first..=last {
            let Some(start) = buffer.iter_at_line(line) else { continue };
            if more {
                let mut at = start;
                buffer.insert(&mut at, INDENT);
            } else {
                let mut end = start;
                let mut count = 0;
                while count < INDENT.len() && end.char() == ' ' {
                    end.forward_char();
                    count += 1;
                }
                if count == 0 && end.char() == '\t' {
                    end.forward_char();
                }
                let mut start = start;
                buffer.delete(&mut start, &mut end);
            }
        }
        buffer.end_user_action();
    }

    /// Ctrl+/: comment the lines out with `// `, or back in when every one of
    /// them already is.
    fn toggle_comment(&self) {
        let buffer = &self.buffer;
        let (first, last) = self.selected_lines();
        let line_text = |line: i32| {
            let start = buffer.iter_at_line(line).unwrap_or(buffer.end_iter());
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            buffer.text(&start, &end, false).to_string()
        };
        let all_commented = (first..=last).map(line_text).filter(|t| !t.trim().is_empty()).all(|t| t.trim_start().starts_with("//"));
        buffer.begin_user_action();
        for line in first..=last {
            let text = line_text(line);
            if text.trim().is_empty() {
                continue;
            }
            let indent = text.len() - text.trim_start().len();
            let Some(mut at) = buffer.iter_at_line(line) else { continue };
            at.set_line_index(indent as i32);
            if all_commented {
                let rest = &text[indent..];
                let width = if rest.starts_with("// ") { 3 } else { 2 };
                let mut end = at;
                end.forward_chars(width);
                buffer.delete(&mut at, &mut end);
            } else {
                buffer.insert(&mut at, "// ");
            }
        }
        buffer.end_user_action();
    }
}

/// Create the syntax tags (coloured for the current theme) and the error
/// line tag of a code buffer.
fn create_code_tags(buffer: &gtk::TextBuffer) {
    let dark = is_dark();
    let colors = if dark { &highlight::DARK_COLORS } else { &highlight::LIGHT_COLORS };
    for (category, color) in colors {
        buffer.create_tag(Some(category.tag_name()), &[("foreground", color)]);
    }
    let background = if dark { "#5c1f1f" } else { "#ffd7d7" };
    buffer.create_tag(Some(TAG_ERROR_LINE), &[("paragraph-background", &background)]);
}

/// The reference panel: a filter and the API entries, grouped.
struct Reference {
    root: gtk::Box,
    list: gtk::ListBox,
    /// The snippet of every activatable row, by row index.
    snippets: Rc<RefCell<Vec<Option<String>>>>,
}

impl Reference {
    /// Call `insert` with the snippet of an activated row.
    fn on_insert(&self, insert: impl Fn(&str) + 'static) {
        let snippets = self.snippets.clone();
        self.list.connect_row_activated(move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            if let Some(Some(snippet)) = snippets.borrow().get(index as usize) {
                insert(snippet);
            }
        });
    }
}

/// Every reference entry: `(group, signature, snippet, doc)`, the `opus` API
/// first, then one `opus.command` per internal command.
fn reference_entries() -> Vec<(String, String, String, String)> {
    let mut out: Vec<(String, String, String, String)> =
        scripts::API.iter().map(|e| (e.group.to_string(), e.signature.to_string(), e.snippet.to_string(), e.doc.to_string())).collect();
    for cmd in InternalCmd::ALL {
        let params: Vec<String> = schema::params_for(cmd)
            .iter()
            .map(|p| format!("{}{}: {}", p.key, if p.required { "" } else { "?" }, p.label))
            .collect();
        let doc = if params.is_empty() { cmd.label().to_string() } else { format!("{}. {}", cmd.label(), params.join("; ")) };
        out.push(("Internal commands".to_string(), format!("{cmd:?}"), command_snippet(cmd), doc));
    }
    out
}

fn reference_panel() -> Reference {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_margin_start(6);
    root.set_margin_end(6);
    root.set_margin_top(6);
    root.set_margin_bottom(6);
    root.set_width_request(260);
    let title = gtk::Label::new(Some("Reference (double-click inserts)"));
    title.set_xalign(0.0);
    title.add_css_class("dim-label");
    root.append(&title);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Filter"));
    root.append(&search);
    let list = gtk::ListBox::new();
    list.set_activate_on_single_click(false);
    let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    root.append(&scroller);
    let snippets: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));

    let fill = {
        let list = list.clone();
        let snippets = snippets.clone();
        move |filter: &str| {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            let filter = filter.to_lowercase();
            let mut rows = Vec::new();
            let mut group = String::new();
            for (g, signature, snippet, doc) in reference_entries() {
                if !filter.is_empty() && !signature.to_lowercase().contains(&filter) && !doc.to_lowercase().contains(&filter) {
                    continue;
                }
                if g != group {
                    group = g.clone();
                    let header = gtk::Label::new(None);
                    header.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(&g)));
                    header.set_xalign(0.0);
                    header.set_margin_top(8);
                    let row = gtk::ListBoxRow::new();
                    row.set_child(Some(&header));
                    row.set_activatable(false);
                    row.set_selectable(false);
                    list.append(&row);
                    rows.push(None);
                }
                let col = gtk::Box::new(gtk::Orientation::Vertical, 1);
                col.set_margin_top(3);
                col.set_margin_bottom(3);
                let sig = gtk::Label::new(Some(&signature));
                sig.set_xalign(0.0);
                sig.set_wrap(true);
                sig.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                sig.add_css_class("monospace");
                col.append(&sig);
                let text = gtk::Label::new(Some(&doc));
                text.set_xalign(0.0);
                text.set_wrap(true);
                text.set_max_width_chars(36);
                text.add_css_class("dim-label");
                col.append(&text);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&col));
                row.set_tooltip_text(Some(&snippet));
                list.append(&row);
                rows.push(Some(snippet));
            }
            *snippets.borrow_mut() = rows;
        }
    };
    fill("");
    search.connect_search_changed(move |entry| fill(&entry.text()));
    Reference { root, list, snippets }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_positions_are_found_in_both_shapes() {
        assert_eq!(error_position("script \"t\": Error: boom (t.js:3:7)"), Some((3, 7)));
        assert_eq!(error_position("SyntaxError: unexpected token ';' at line 12, col 9 (native)"), Some((12, 9)));
        assert_eq!(error_position("my.script.js:4"), Some((4, 1)));
        assert_eq!(error_position("nothing here"), None);
    }

    #[test]
    fn an_undefined_name_points_at_its_first_use() {
        let code = "// undefinedThing here is a comment\nopus.log(1);\n\n  undefinedThing.call();";
        let error = "script \"t\": ReferenceError: undefinedThing is not defined (t.js:2:9)";
        assert_eq!(error_location(error, code), Some((4, 3)));
        assert_eq!(error_location("ReferenceError: gone is not defined (t.js:?:?)", "x"), None);
        assert_eq!(error_location("Error: x (t.js:2:7)", "a\nb"), Some((2, 7)));
        assert_eq!(find_word("xa = x;", "x"), Some((1, 6)));
    }

    #[test]
    fn a_new_line_keeps_the_indent_and_opens_a_level_after_a_bracket() {
        assert_eq!(next_indent("    let a = 1;"), "    ");
        assert_eq!(next_indent("if (x) {"), "    ");
        assert_eq!(next_indent("    foo(["), "        ");
        assert_eq!(next_indent("plain"), "");
    }

    #[test]
    fn command_snippets_name_the_required_parameters() {
        assert_eq!(command_snippet(InternalCmd::Copy), "opus.command(\"Copy\")");
        assert_eq!(command_snippet(InternalCmd::CopyAs), "opus.command(\"CopyAs\", { new_name: \"\" })");
    }

    #[test]
    fn the_reference_lists_the_api_and_every_command() {
        let entries = reference_entries();
        assert_eq!(entries.len(), scripts::API.len() + InternalCmd::ALL.len());
        assert!(entries.iter().any(|e| e.1 == "opus.command(name, params)"));
        assert!(entries.iter().any(|e| e.1 == "Script"));
    }
}
