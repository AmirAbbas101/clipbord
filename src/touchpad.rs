//! Touchpad on/off helper for laptop typing.
//!
//! Independent of clipboard capture/history. Uses GNOME's
//! `org.gnome.desktop.peripherals.touchpad send-events` setting so a custom
//! keyboard shortcut can disable the pad while typing and re-enable it after.

use anyhow::{bail, Context, Result};
use std::process::Command;

const SCHEMA: &str = "org.gnome.desktop.peripherals.touchpad";
const KEY: &str = "send-events";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Enabled,
    Disabled,
    DisabledOnExternalMouse,
}

impl State {
    fn as_gsettings(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::DisabledOnExternalMouse => "disabled-on-external-mouse",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        // gsettings prints quoted enums: 'enabled'
        let s = raw.trim().trim_matches('\'');
        match s {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            "disabled-on-external-mouse" => Ok(Self::DisabledOnExternalMouse),
            other => bail!("unknown touchpad send-events value: {other}"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::DisabledOnExternalMouse => "disabled (external mouse)",
        }
    }

    fn is_effectively_on(self) -> bool {
        matches!(self, Self::Enabled | Self::DisabledOnExternalMouse)
    }
}

fn gsettings_get() -> Result<State> {
    let output = Command::new("gsettings")
        .args(["get", SCHEMA, KEY])
        .output()
        .context("run gsettings (is GNOME / gsettings installed?)")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("gsettings get failed: {err}");
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    State::parse(&raw)
}

fn gsettings_set(state: State) -> Result<()> {
    let status = Command::new("gsettings")
        .args(["set", SCHEMA, KEY, state.as_gsettings()])
        .status()
        .context("run gsettings set")?;
    if !status.success() {
        bail!("gsettings set {KEY}={} failed", state.as_gsettings());
    }
    Ok(())
}

fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["--app-name=Clipbord", "--expire-time=2000", title, body])
        .status();
}

/// Toggle touchpad: on → off, off → on.
pub fn toggle() -> Result<State> {
    let current = gsettings_get()?;
    let next = if current.is_effectively_on() {
        State::Disabled
    } else {
        State::Enabled
    };
    gsettings_set(next)?;
    notify(
        "Touchpad",
        &format!("Touchpad {}", next.label()),
    );
    Ok(next)
}

pub fn set_enabled(on: bool) -> Result<State> {
    let next = if on {
        State::Enabled
    } else {
        State::Disabled
    };
    gsettings_set(next)?;
    notify(
        "Touchpad",
        &format!("Touchpad {}", next.label()),
    );
    Ok(next)
}

pub fn status() -> Result<State> {
    gsettings_get()
}
