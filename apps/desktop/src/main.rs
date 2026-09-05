mod bootstrap;
mod infrastructure;
mod model;
mod presenter;
mod view;

fn main() -> anyhow::Result<()> {
    bootstrap::run()
}
