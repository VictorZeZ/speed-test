mod app;
mod dsl;
mod history;
#[cfg(windows)]
mod input;
mod keys;
mod net;
mod ui;

use app::App;
use std::time::Duration;

#[tokio::main]
async fn main() {
    // If anything panics, the terminal must be handed back to the user in a
    // usable state instead of leaving them inside an alt-screen full of junk.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();

    let result = run(&mut terminal).await;

    if let Err(err) = result {
        let _ = terminal.clear();
        ratatui::restore();
        eprintln!("speed-test exited with an error:");
        eprintln!("  {err:#}");
        eprintln!();
        eprintln!("If this keeps happening, check your internet connection and try again.");
        std::process::exit(1);
    }
}

async fn run(terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
    let mut app = App::new();
    app.load_history();
    app.begin_connection_lookup();
    app.begin_dsl_polling();

    // Windows: read physical virtual-key codes so shortcuts work on every
    // keyboard language. Other platforms: crossterm's own event source.
    #[cfg(windows)]
    let key_rx = input::spawn_key_reader();

    loop {
        if let Err(draw_err) = terminal.draw(|f| ui::draw(f, &mut app)) {
            // A transient draw failure (e.g. during exotic resize races)
            // should not kill the session; retry on the next tick.
            if draw_err.kind() == std::io::ErrorKind::Interrupted {
                app.tick();
                continue;
            }
            return Err(anyhow::Error::new(draw_err).context("terminal draw failed"));
        }

        #[cfg(windows)]
        {
            while let Ok(key) = key_rx.try_recv() {
                app.on_key(key);
            }
        }

        #[cfg(not(windows))]
        match crossterm::event::poll(Duration::from_millis(0)) {
            Ok(true) => match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => app.on_key(key),
                Ok(_) => {}
                Err(read_err) => {
                    return Err(
                        anyhow::Error::new(read_err).context("failed to read keyboard input")
                    );
                }
            },
            Ok(false) => {}
            Err(poll_err) => {
                return Err(anyhow::Error::new(poll_err).context("failed to poll keyboard input"));
            }
        }

        #[cfg(windows)]
        {
            let _ = &key_rx; // keep alive for the loop
        }

        app.tick();

        if app.should_quit {
            break;
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    Ok(())
}
