mod index;
mod model;
mod server;
mod sources;
mod update;

use clap::{Args, Parser, Subcommand};

/// Receipts: review and explore your agentic chats across Claude Code, Codex and more.
#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    /// Options for the web UI when no command is given
    #[command(flatten)]
    serve: ServeArgs,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value_t = 7878, env = "RECEIPTS_PORT")]
    port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Don't open a browser window
    #[arg(long)]
    no_open: bool,
    /// Also accept requests for this hostname, such as a Tailscale name (repeatable)
    #[arg(long = "allow-host", env = "RECEIPTS_ALLOW_HOSTS", value_delimiter = ',')]
    allow_hosts: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the local web UI (default)
    Serve(ServeArgs),
    /// Index all agent sessions and exit
    Index,
    /// Full-text search across all sessions from the terminal
    Search {
        query: Vec<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Print where Receipts stores its index
    Where,
    /// Update to the newest release from GitHub
    Update,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let db_path = index::data_dir().join("receipts.db");

    let command = cli.command.unwrap_or(Command::Serve(cli.serve));
    if !matches!(command, Command::Update) {
        update::auto_update();
    }

    match command {
        Command::Serve(args) => server::serve(db_path, args.host, args.port, !args.no_open, args.allow_hosts).await?,
        Command::Index => {
            let mut conn = index::open(&db_path)?;
            let r = index::reindex(&mut conn)?;
            println!("Scanned {} files, updated {}, errors {}", r.scanned, r.updated, r.errors);
        }
        Command::Search { query, source, limit } => {
            let mut conn = index::open(&db_path)?;
            index::reindex(&mut conn)?;
            let hits = index::search(&conn, &query.join(" "), source.as_deref(), limit)?;
            for h in hits {
                let snippet = h.snippet.replace('\u{2}', "\x1b[1;33m").replace('\u{3}', "\x1b[0m").replace('\n', " ");
                println!(
                    "\x1b[2m{} [{}]\x1b[0m {}\n  {}\n  \x1b[2m{}\x1b[0m\n",
                    h.updated_at.as_deref().unwrap_or("").get(..10).unwrap_or(""),
                    h.source,
                    h.title.as_deref().unwrap_or("(untitled)"),
                    snippet,
                    h.session_id,
                );
            }
        }
        Command::Where => println!("{}", db_path.display()),
        Command::Update => update::update_command()?,
    }
    Ok(())
}
