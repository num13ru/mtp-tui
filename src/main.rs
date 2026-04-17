mod app;
mod backend;
mod types;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let terminal = ratatui::init();
    let result = app::App::new().and_then(|app| app.run(terminal));
    ratatui::restore();
    result
}
