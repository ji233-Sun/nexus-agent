#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod bootstrap;
mod i18n;
mod infrastructure;
mod model;
mod presenter;
mod remote_control;
mod view;

fn main() -> anyhow::Result<()> {
    bootstrap::run()
}
