#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod bootstrap;
mod infrastructure;
mod model;
mod presenter;
mod view;

fn main() -> anyhow::Result<()> {
    bootstrap::run()
}
