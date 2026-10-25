mod app;
mod docker;
mod human;
mod plain;
mod ui;

use std::error::Error;
use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use bollard::Docker;
use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Real-time Docker container stats in your terminal.
#[derive(Parser, Debug)]
#[command(name = "dockerdash", version, about, long_about = None)]
struct Args {
    /// Container list refresh interval in seconds
    #[arg(short, long, default_value_t = 5)]
    interval: u64,

    /// Include stopped containers, not just running ones
    #[arg(short, long)]
    all: bool,

    /// Docker daemon socket path (overrides DOCKER_HOST)
    #[arg(short, long)]
    socket: Option<String>,

    /// Render a single snapshot as plain text and exit (default when stdout is not a TTY)
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Args::parse()).await {
        eprintln!("dockerdash: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let docker = docker::connect(args.socket.as_deref())?;
    // negotiate + version probe, bounded so a wedged daemon can't hang startup
    let docker = tokio::time::timeout(Duration::from_secs(10), docker.negotiate_version())
        .await
        .map_err(|_| "timed out contacting the Docker daemon (10s)".to_string())??;
    let engine = match tokio::time::timeout(Duration::from_secs(10), docker::engine_version(&docker))
        .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            return Err(format!(
                "cannot reach the Docker daemon: {e}\n  hint: is `dockerd` running and do you have access to the socket? (try `docker ps`)"
            )
            .into())
        }
        Err(_) => return Err("timed out contacting the Docker daemon (10s)".into()),
    };

    if args.once || !io::stdout().is_terminal() {
        plain::run(&docker, args.all).await?;
        return Ok(());
    }

    run_tui(docker, engine, args).await
}

async fn run_tui(docker: Docker, engine: String, args: Args) -> Result<(), Box<dyn Error>> {
    let mut terminal = init_terminal()?;
    let (app, rx) = app::App::new(
        docker,
        engine,
        args.all,
        Duration::from_secs(args.interval.max(1)),
    );
    let result = app.run(&mut terminal, rx).await;
    restore_terminal()?;
    result
}

fn init_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        panic_hook(info);
    }));
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

fn restore_terminal() -> io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    Ok(())
}
