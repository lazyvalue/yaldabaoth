use std::io;
use std::process;

use clap::Parser;

mod app;

#[derive(Parser)]
#[command(name = "sketch", about = "A beautiful TUI markdown viewer")]
struct Cli {
    /// Markdown file to view
    file: Option<String>,

    /// Color theme (dracula, nightfox)
    #[arg(long)]
    theme: Option<String>,
}

fn main() {
    // Install panic hook for terminal restoration
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        );
        default_hook(info);
    }));

    let cli = Cli::parse();

    let file_path = match cli.file {
        Some(f) => f,
        None => {
            eprintln!("Usage: sketch <file.md>");
            process::exit(1);
        }
    };

    // Read file
    let content = match std::fs::read(&file_path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("Error: {} is not valid UTF-8", file_path);
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error: cannot open {}: {}", file_path, e);
            process::exit(1);
        }
    };

    let abs_path = std::path::Path::new(&file_path)
        .canonicalize()
        .unwrap_or_else(|_| file_path.clone().into())
        .display()
        .to_string();

    let mut config = match sketch::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    if let Some(ref theme_str) = cli.theme {
        match sketch::theme::ThemeName::parse(theme_str) {
            Some(name) => config.theme = name,
            None => {
                eprintln!("Unknown theme: {}", theme_str);
                process::exit(1);
            }
        }
    }

    let mut app = app::App::new(abs_path, content, &config);

    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
