use crate::config::Config;
use crate::ipc;
use crate::ui::HistoryWindow;
use anyhow::{Context, Result};
use gio::ApplicationFlags;
use gtk::prelude::*;
use libadwaita as adw;

/// Short-lived UI. No application id and no .desktop entry → never appears in the GNOME dock.
pub fn run_ui(config: Config) -> Result<()> {
    if ipc::ping().is_err() {
        anyhow::bail!(
            "clipbord daemon is not running.\nStart: systemctl --user enable --now clipbord.service"
        );
    }

    // Kick ingest in the background — don't block window open on clipboard sample.
    std::thread::spawn(|| {
        let _ = ipc::request(&ipc::Request::Ingest);
    });

    // Wayland forbids clients from placing top-levels. Use Xwayland so we can
    // open the history popup in the top-right corner (Win+V style).
    std::env::set_var("GDK_BACKEND", "x11");

    adw::init().context("init libadwaita")?;
    load_css();

    // Intentionally NO application_id — a named Gio app is what GNOME pins in the dock.
    let app = adw::Application::new(None::<&str>, ApplicationFlags::NON_UNIQUE);

    let config = config.clone();
    app.connect_activate(move |app| {
        let window = HistoryWindow::new(app, config.clone());
        window.present();
    });

    let code = app.run_with_args::<&str>(&[]);
    if code != glib::ExitCode::SUCCESS {
        anyhow::bail!("ui exited with {code:?}");
    }
    Ok(())
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        .clipbord-window {
            border-radius: 14px;
        }
        .clipbord-search {
            margin-bottom: 4px;
        }
        .clipbord-scroll {
            min-height: 260px;
        }
        .clipbord-row {
            padding: 2px 0;
        }
        .clipbord-preview {
            font-weight: 500;
        }
        .clipbord-meta {
            opacity: 0.75;
        }
        .clipbord-status {
            opacity: 0.7;
            font-size: 0.85em;
        }
        "#,
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
