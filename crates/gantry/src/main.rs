//! The `gantry` binary.

mod drivers;
mod logging;
#[cfg(feature = "bambu")]
mod probe;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use gantry_server::AppState;
use gantry_server::config::Config;
use gantry_server::store::users::Role;

/// Monitor and control a Fleet of 3D printers from one screen.
#[derive(Debug, Parser)]
#[command(name = "gantry", version = gantry_server::VERSION)]
struct Cli {
    /// The config file. Defaults to `gantry.toml` in the working directory,
    /// when it exists. Every setting can also be set by environment variable.
    #[arg(long, short, global = true, env = "GANTRY_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the server (the default).
    Serve,
    /// Manage accounts. The first account is created here: there is no
    /// first-run setup page. In a container: `docker exec -it gantry gantry
    /// user create --admin --username <name>`.
    #[command(subcommand)]
    User(UserCommand),
    /// Check a real Bambu printer against what the Driver expects: model,
    /// firmware, status, and the camera. Changes nothing on the printer. The
    /// serial number and access code are read from GANTRY_PROBE_SERIAL and
    /// GANTRY_PROBE_ACCESS_CODE, or asked for.
    #[cfg(feature = "bambu")]
    BambuProbe(probe::ProbeArgs),
}

#[derive(Debug, Subcommand)]
enum UserCommand {
    /// Create an account.
    Create {
        /// The new account's username.
        #[arg(long, short)]
        username: String,
        /// The account's role: viewer (sees everything, changes nothing),
        /// operator (also controls Printers and manages Jobs and the Queue) or
        /// admin (also manages Printers, users and settings).
        #[arg(long, value_parser = clap::value_parser!(Role), conflicts_with = "admin")]
        role: Option<Role>,
        /// The same as `--role admin`. The first account must be an Admin.
        #[arg(long)]
        admin: bool,
        /// Read the password from the first line of standard input instead of
        /// prompting for it.
        #[arg(long)]
        password_stdin: bool,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("gantry: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let config = Config::from_process(cli.config.as_deref())?;
    logging::init(config.log_format);
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config).await,
        Command::User(UserCommand::Create {
            username,
            role,
            admin,
            password_stdin,
        }) => {
            let role = match (role, admin) {
                (Some(role), _) => role,
                (None, true) => Role::Admin,
                (None, false) => anyhow::bail!(
                    "choose the account's role with --role viewer, --role operator or --role admin"
                ),
            };
            create_user(&config, &username, role, password_stdin).await
        }
        #[cfg(feature = "bambu")]
        Command::BambuProbe(args) => probe::run(args).await,
    }
}

async fn create_user(
    config: &Config,
    username: &str,
    role: Role,
    password_stdin: bool,
) -> anyhow::Result<()> {
    let password = if password_stdin {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("could not read the password from standard input")?;
        line.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        let first = rpassword::prompt_password(format!("Password for {username}: "))
            .context("could not read the password (use --password-stdin without a terminal)")?;
        let second = rpassword::prompt_password("Repeat the password: ")?;
        if first != second {
            anyhow::bail!("the passwords do not match");
        }
        first
    };
    let store = gantry_server::store::Store::open(&config.database_path()).await?;
    let user = store.create_user(username, &password, role).await?;
    tracing::info!(username = %user.username, role = role.as_str(), "created an account");
    println!("Created the {} account `{}`.", role.label(), user.username);
    Ok(())
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let drivers = drivers::registry();
    tracing::info!(drivers = ?drivers, "Drivers compiled in");
    // Read before anything starts, so a bad certificate stops Gantry at once.
    let tls = match &config.tls {
        Some(files) => Some(gantry_server::tls::load(&files.cert, &files.key)?),
        None => None,
    };
    let state = AppState::start(&config, drivers).await?;
    // Prometheus metrics on their own address (PRD §9.2), when asked for.
    if let Some(address) = config.metrics_bind {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .with_context(|| format!("could not listen for metrics on {address}"))?;
        tracing::info!(address = %address, token = config.metrics_token.is_some(), "metrics at /metrics");
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = gantry_server::metrics::serve_on(listener, state).await {
                tracing::error!(error = %e, "the metrics server stopped");
            }
        });
    } else if config.metrics_token.is_some() {
        tracing::info!("metrics at /metrics, for requests with the bearer token");
    }
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("could not listen on {}", config.bind))?;
    tracing::info!(
        version = gantry_server::VERSION,
        address = %gantry_server::tls::describe(listener.local_addr()?, tls.is_some()),
        data_dir = %config.data_dir.display(),
        "Gantry listening"
    );
    match tls {
        Some(tls) => gantry_server::tls::serve(listener, state.clone(), tls).await?,
        None => gantry_server::serve(listener, state.clone()).await?,
    }
    state.fleet().stop_all().await;
    Ok(())
}
