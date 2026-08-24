mod app;
mod history;
mod net;
mod ui;

use app::App;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
    let mut app = App::new();
    app.load_history();

    // Kick off connection info lookup immediately
    {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.pending_meta = Some(rx);
        tokio::spawn(async move {
            let client = match net::build_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            match net::fetch_connection_info(&client).await {
                Ok(info) => {
                    let _ = tx.send(Ok(info));
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                }
            }
        });
    }

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        while crossterm::event::poll(Duration::from_millis(0))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                app.on_key(key);
            }
        }

        app.tick();

        if let Some(rx) = app.pending_meta.as_mut() {
            if let Ok(res) = rx.try_recv() {
                app.pending_meta = None;
                match res {
                    Ok(info) => {
                        if app.connection.is_none() {
                            app.connection = Some(info);
                        }
                    }
                    Err(e) => {
                        if app.error.is_none() {
                            app.error = Some(format!("could not reach speed test server: {e}"));
                        }
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    Ok(())
}
