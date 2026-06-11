slint::include_modules!();

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let window = MainWindow::new()?;
    window.run()?;

    Ok(())
}
