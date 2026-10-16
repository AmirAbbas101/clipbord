use crate::config::Config;
use crate::ipc::{self, ClipSummary, Request, Response};
use crate::store::human_bytes;
use chrono::{Local, TimeZone, Utc};
use gtk::gdk;
use gtk::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, DropTarget, Label, ListBox, ListBoxRow, Orientation, PolicyType,
    ScrolledWindow, SearchEntry, SelectionMode,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct HistoryWindow {
    window: adw::Window,
    app: adw::Application,
    list: ListBox,
    search: SearchEntry,
    status: Label,
    selected_id: Rc<RefCell<Option<String>>>,
}

impl HistoryWindow {
    pub fn new(app: &adw::Application, config: Config) -> Self {
        let window = adw::Window::new();
        // Keep a Gtk.Application for the main loop, but this is a transient popup —
        // not a dock/taskbar app. (No .desktop file + no application id.)
        window.set_application(Some(app));
        window.set_title(Some("Clipbord"));
        window.set_default_width(config.window_width);
        window.set_default_height(config.window_height);
        window.set_resizable(true);
        window.set_modal(true);
        window.set_destroy_with_parent(true);
        window.add_css_class("clipbord-window");

        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&Label::new(Some("Clipboard History"))));

        let clear_btn = Button::from_icon_name("edit-clear-all-symbolic");
        clear_btn.set_tooltip_text(Some("Clear unpinned history"));
        clear_btn.add_css_class("flat");
        header.pack_end(&clear_btn);
        toolbar.add_top_bar(&header);

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(12);
        content.set_margin_end(12);

        let search = SearchEntry::new();
        search.set_placeholder_text(Some("Search…"));
        search.set_hexpand(true);
        search.set_margin_bottom(8);
        search.add_css_class("clipbord-search");

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Single);
        list.add_css_class("boxed-list");
        list.set_hexpand(true);
        list.set_vexpand(true);
        list.set_show_separators(true);

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .child(&list)
            .vexpand(true)
            .build();
        scrolled.add_css_class("clipbord-scroll");

        let status = Label::new(Some("Enter paste · Esc close"));
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.add_css_class("clipbord-status");
        status.set_halign(Align::Start);
        status.set_margin_top(6);

        content.append(&search);
        content.append(&scrolled);
        content.append(&status);
        toolbar.set_content(Some(&content));
        window.set_content(Some(&toolbar));

        let selected_id = Rc::new(RefCell::new(None));
        let this = Self {
            window,
            app: app.clone(),
            list,
            search,
            status,
            selected_id,
        };
        this.bind_actions(clear_btn);
        this.refresh();
        this
    }

    pub fn present(&self) {
        let window = self.window.clone();
        // Place after map — Mutter may ignore an earlier move.
        self.window.connect_map(move |win| {
            let win = win.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(16), {
                let win = win.clone();
                move || position_top_right(&win)
            });
            glib::timeout_add_local_once(std::time::Duration::from_millis(80), {
                let win = win.clone();
                move || position_top_right(&win)
            });
        });
        self.window.present();
        position_top_right(&window);
        self.search.grab_focus();
    }

    fn refresh(&self) {
        rebuild_list(
            &self.app,
            &self.list,
            &self.status,
            &self.selected_id,
            &self.search.text(),
        );
    }

    fn bind_actions(&self, clear_btn: Button) {
        let list = self.list.clone();
        let search = self.search.clone();
        let status = self.status.clone();
        let selected_id = self.selected_id.clone();
        let window = self.window.clone();
        let app = self.app.clone();

        {
            let list = list.clone();
            let status = status.clone();
            let selected_id = selected_id.clone();
            let app = app.clone();
            self.search.connect_search_changed(move |entry| {
                rebuild_list(&app, &list, &status, &selected_id, &entry.text());
            });
        }

        {
            let window = window.clone();
            let app = app.clone();
            self.list.connect_row_activated(move |_list, row| {
                if let Some(id) = row_id(row) {
                    paste_and_quit(&id, &window, &app);
                }
            });
        }

        {
            let selected_id = selected_id.clone();
            self.list.connect_row_selected(move |_list, row| {
                *selected_id.borrow_mut() = row.and_then(row_id);
            });
        }

        {
            let list = list.clone();
            let status = status.clone();
            let selected_id = selected_id.clone();
            let search = search.clone();
            let app = app.clone();
            clear_btn.connect_clicked(move |_| {
                let _ = ipc::request(&Request::Clear);
                rebuild_list(&app, &list, &status, &selected_id, &search.text());
            });
        }

        {
            let app = app.clone();
            self.window.connect_close_request(move |_| {
                app.quit();
                glib::Propagation::Proceed
            });
        }

        let key = gtk::EventControllerKey::new();
        {
            let list = list.clone();
            let status = status.clone();
            let selected_id = selected_id.clone();
            let search = search.clone();
            let window = window.clone();
            let app = app.clone();
            key.connect_key_pressed(move |_, keyval, _, _| {
                use gtk::gdk::Key;
                let can_edit = !search.is_focus();
                match keyval {
                    Key::Escape => {
                        window.close();
                        app.quit();
                        glib::Propagation::Stop
                    }
                    Key::Return | Key::KP_Enter => {
                        if let Some(id) = selected_id.borrow().clone() {
                            paste_and_quit(&id, &window, &app);
                        }
                        glib::Propagation::Stop
                    }
                    key if matches!(key, Key::Delete | Key::BackSpace) && can_edit => {
                        if let Some(id) = selected_id.borrow().clone() {
                            let _ = ipc::request(&Request::Remove { id });
                            rebuild_list(&app, &list, &status, &selected_id, &search.text());
                        }
                        glib::Propagation::Stop
                    }
                    Key::p | Key::P if can_edit => {
                        if let Some(id) = selected_id.borrow().clone() {
                            let _ = ipc::request(&Request::Pin { id });
                            rebuild_list(&app, &list, &status, &selected_id, &search.text());
                        }
                        glib::Propagation::Stop
                    }
                    Key::_1
                    | Key::_2
                    | Key::_3
                    | Key::_4
                    | Key::_5
                    | Key::_6
                    | Key::_7
                    | Key::_8
                    | Key::_9
                        if can_edit =>
                    {
                        let n = match keyval {
                            Key::_1 => 1,
                            Key::_2 => 2,
                            Key::_3 => 3,
                            Key::_4 => 4,
                            Key::_5 => 5,
                            Key::_6 => 6,
                            Key::_7 => 7,
                            Key::_8 => 8,
                            Key::_9 => 9,
                            _ => 0,
                        };
                        if n > 0 {
                            if let Some(row) = list.row_at_index(n as i32 - 1) {
                                if let Some(id) = row_id(&row) {
                                    paste_and_quit(&id, &window, &app);
                                }
                            }
                        }
                        glib::Propagation::Stop
                    }
                    Key::Down if can_edit || search.text().is_empty() => {
                        select_delta(&list, 1);
                        glib::Propagation::Stop
                    }
                    Key::Up if can_edit || search.text().is_empty() => {
                        select_delta(&list, -1);
                        glib::Propagation::Stop
                    }
                    Key::slash if can_edit => {
                        search.grab_focus();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        self.window.add_controller(key);
    }
}

fn rebuild_list(
    app: &adw::Application,
    list: &ListBox,
    status: &Label,
    selected_id: &Rc<RefCell<Option<String>>>,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let (items, count, bytes) = match ipc::request(&Request::List {
        query: query.to_string(),
    }) {
        Ok(Response::List {
            items,
            count,
            bytes,
        }) => (items, count, bytes),
        Ok(_) => (Vec::new(), 0, 0),
        Err(e) => {
            status.set_text(&format!("Daemon error: {e}"));
            return;
        }
    };

    if items.is_empty() {
        let empty = Label::new(Some(if query.trim().is_empty() {
            "No clipboard history yet.\nCopy something to get started."
        } else {
            "No matches."
        }));
        empty.set_justify(gtk::Justification::Center);
        empty.add_css_class("dim-label");
        empty.set_margin_top(32);
        empty.set_margin_bottom(32);
        let row = ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        row.set_child(Some(&empty));
        list.append(&row);
    } else {
        for (idx, item) in items.iter().enumerate() {
            list.append(&build_row(app, list, status, selected_id, query, item, idx));
        }
    }

    status.set_text(&format!(
        "{count} · {} · Enter paste · Esc close",
        human_bytes(bytes)
    ));

    let want = selected_id.borrow().clone();
    let mut selected = false;
    if let Some(id) = want {
        let mut i = 0;
        while let Some(row) = list.row_at_index(i) {
            if row_id(&row).as_deref() == Some(id.as_str()) {
                list.select_row(Some(&row));
                selected = true;
                break;
            }
            i += 1;
        }
    }
    if !selected {
        if let Some(row) = list.row_at_index(0) {
            if row.is_selectable() {
                list.select_row(Some(&row));
                *selected_id.borrow_mut() = row_id(&row);
            }
        }
    }
}

fn build_row(
    app: &adw::Application,
    list: &ListBox,
    status: &Label,
    selected_id: &Rc<RefCell<Option<String>>>,
    query: &str,
    item: &ClipSummary,
    idx: usize,
) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_activatable(true);
    row.set_widget_name(&item.id);
    row.add_css_class("clipbord-row");

    let outer = GtkBox::new(Orientation::Horizontal, 10);
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer.set_margin_start(8);
    outer.set_margin_end(6);

    let icon = if item.kind == "image" {
        image_thumbnail_widget(item.thumbnail_base64.as_deref())
    } else {
        let icon = gtk::Image::from_icon_name("edit-copy-symbolic");
        icon.set_pixel_size(22);
        icon.set_valign(Align::Center);
        icon
    };

    let text_col = GtkBox::new(Orientation::Vertical, 2);
    text_col.set_hexpand(true);

    let title = Label::new(Some(&item.preview));
    title.set_halign(Align::Start);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_max_width_chars(46);
    title.add_css_class("clipbord-preview");

    let ts = Utc
        .timestamp_opt(item.created_at, 0)
        .single()
        .unwrap_or_else(Utc::now);
    let meta = format!(
        "{} · {}{}",
        relative_time(ts),
        human_bytes(item.byte_size),
        if idx < 9 {
            format!(" · {}", idx + 1)
        } else {
            String::new()
        }
    );
    let subtitle = Label::new(Some(&meta));
    subtitle.set_halign(Align::Start);
    subtitle.set_xalign(0.0);
    subtitle.add_css_class("caption");
    subtitle.add_css_class("dim-label");
    subtitle.add_css_class("clipbord-meta");

    text_col.append(&title);
    text_col.append(&subtitle);

    let pin = Button::from_icon_name(if item.pinned {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
    pin.set_tooltip_text(Some("Pin / unpin"));
    pin.add_css_class("flat");
    pin.set_valign(Align::Center);

    let delete = Button::from_icon_name("user-trash-symbolic");
    delete.set_tooltip_text(Some("Delete"));
    delete.add_css_class("flat");
    delete.set_valign(Align::Center);

    {
        let app = app.clone();
        let list = list.clone();
        let status = status.clone();
        let selected_id = selected_id.clone();
        let query = query.to_string();
        let id = item.id.clone();
        pin.connect_clicked(move |_| {
            let _ = ipc::request(&Request::Pin { id: id.clone() });
            rebuild_list(&app, &list, &status, &selected_id, &query);
        });
    }
    {
        let app = app.clone();
        let list = list.clone();
        let status = status.clone();
        let selected_id = selected_id.clone();
        let query = query.to_string();
        let id = item.id.clone();
        delete.connect_clicked(move |_| {
            let _ = ipc::request(&Request::Remove { id: id.clone() });
            rebuild_list(&app, &list, &status, &selected_id, &query);
        });
    }

    outer.append(&icon);
    outer.append(&text_col);
    outer.append(&pin);
    outer.append(&delete);
    row.set_child(Some(&outer));

    // Drag to reorder (disabled while searching — order is global).
    if query.trim().is_empty() {
        setup_row_drag(&row, &item.id, app, list, status, selected_id, idx);
    }

    row
}

fn setup_row_drag(
    row: &ListBoxRow,
    id: &str,
    app: &adw::Application,
    list: &ListBox,
    status: &Label,
    selected_id: &Rc<RefCell<Option<String>>>,
    row_index: usize,
) {
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    let drag_id = id.to_string();
    drag.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(&glib::Value::from(
            drag_id.as_str(),
        )))
    });
    row.add_controller(drag);

    let drop = DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    let app = app.clone();
    let list = list.clone();
    let status = status.clone();
    let selected_id = selected_id.clone();
    let dest_index = row_index;
    drop.connect_drop(move |_, value, _x, _y| {
        let Ok(src_id) = value.get::<String>() else {
            return false;
        };
        *selected_id.borrow_mut() = Some(src_id.clone());
        let _ = ipc::request(&Request::MoveTo {
            id: src_id,
            index: dest_index,
        });
        rebuild_list(&app, &list, &status, &selected_id, "");
        true
    });
    row.add_controller(drop);
}

fn row_id(row: &ListBoxRow) -> Option<String> {
    let name = row.widget_name();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn image_thumbnail_widget(thumbnail_base64: Option<&str>) -> gtk::Image {
    if let Some(b64) = thumbnail_base64 {
        if let Ok(bytes) = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            b64,
        ) {
            let gbytes = glib::Bytes::from(&bytes[..]);
            if let Ok(texture) = gdk::Texture::from_bytes(&gbytes) {
                let image = gtk::Image::from_paintable(Some(&texture));
                image.set_pixel_size(48);
                image.set_valign(Align::Center);
                return image;
            }
        }
    }
    let image = gtk::Image::from_icon_name("image-x-generic-symbolic");
    image.set_pixel_size(32);
    image.set_valign(Align::Center);
    image
}

/// Pin the popup to the top-right of the primary (or first) monitor.
/// Requires the UI to run under GDK_BACKEND=x11 (see app.rs).
fn position_top_right(window: &impl IsA<gtk::Window>) {
    let window = window.as_ref();
    let Some(surface) = window.surface() else {
        return;
    };
    let Ok(x11_surface) = surface.downcast::<gdk4_x11::X11Surface>() else {
        return;
    };

    let display = gtk::prelude::WidgetExt::display(window);
    let monitors = display.monitors();
    // Prefer the monitor that currently shows this surface; else first output.
    let monitor = display
        .monitor_at_surface(&x11_surface)
        .or_else(|| {
            monitors
                .item(0)
                .and_then(|o| o.downcast::<gdk::Monitor>().ok())
        });
    let Some(geom) = monitor.map(|m| m.geometry()) else {
        return;
    };

    let margin = 16;
    let width = {
        let w = window.width();
        if w > 1 {
            w
        } else {
            window.default_width().max(1)
        }
    };

    let x = geom.x() + geom.width() - width - margin;
    let y = geom.y() + margin;
    let xid = x11_surface.xid();

    unsafe {
        use std::ffi::CString;
        use x11::xlib;
        let name = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        let Ok(c_name) = CString::new(name) else {
            return;
        };
        let dpy = xlib::XOpenDisplay(c_name.as_ptr());
        if dpy.is_null() {
            return;
        }
        xlib::XMoveWindow(dpy, xid as xlib::Window, x, y);
        xlib::XRaiseWindow(dpy, xid as xlib::Window);
        xlib::XFlush(dpy);
        xlib::XCloseDisplay(dpy);
    }
}

fn paste_and_quit(id: &str, window: &adw::Window, app: &adw::Application) {
    let id = id.to_string();
    // Hide immediately so the previous input field can regain focus before
    // the daemon injects Ctrl+V (auto-paste runs async after this returns).
    window.set_visible(false);
    if let Err(e) = ipc::request(&Request::Paste { id }) {
        tracing::warn!("paste failed: {e:#}");
    }
    app.quit();
}

fn select_delta(list: &ListBox, delta: i32) {
    let current = list.selected_row().map(|r| r.index()).unwrap_or(-1);
    let mut next = current + delta;
    if next < 0 {
        next = 0;
    }
    if let Some(row) = list.row_at_index(next) {
        if row.is_selectable() {
            list.select_row(Some(&row));
            row.grab_focus();
        }
    }
}

fn relative_time(ts: chrono::DateTime<Utc>) -> String {
    let local = ts.with_timezone(&Local);
    let now = Local::now();
    let delta = now.signed_duration_since(local);
    if delta.num_seconds() < 60 {
        "just now".into()
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else if delta.num_days() < 7 {
        format!("{}d ago", delta.num_days())
    } else {
        local.format("%b %d %H:%M").to_string()
    }
}
