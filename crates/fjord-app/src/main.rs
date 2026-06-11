slint::include_modules!();

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();

    if let Some(url) = args.get(1) {
        tracing::info!("playing: {}", url);
        fjord_player::Player::play(url)?.wait()?;
    } else {
        let window = MainWindow::new()?;
        window.run()?;
    }

    Ok(())
}
