mod client;
mod remote;
mod serve;
mod util;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::util::ENV_SSH_KEY;

#[derive(Parser)]
#[command(
    name = "dropserve",
    version = env!("CARGO_PKG_VERSION"),
    about = "Static sites over SSH: zip → scp → nginx + certbot (see DOC.md)"
)]
struct Cli {
    /// SSH private key path (or set DROPSERVE_SSH_KEY).
    #[arg(long, global = true, env = ENV_SSH_KEY)]
    ssh_key: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload artifact and install site (HTTP → DNS check → certbot).
    Create {
        user_host: String,
        domain: String,
        /// Directory to zip, or an existing .zip file.
        path: PathBuf,
        /// Run this site through a PocketBase service instead of static nginx.
        #[arg(long)]
        pocketbase: bool,
        /// Localhost port for the PocketBase service.
        #[arg(long, default_value_t = 8090)]
        pb_port: u16,
    },
    /// Upload a new release and atomically switch `current`.
    Update {
        user_host: String,
        domain: String,
        path: PathBuf,
    },
    /// Remove nginx config and site directory (requires typing DELETE).
    Delete { user_host: String, domain: String },
    /// List deployed sites on the remote host.
    List { user_host: String },
    /// Point `current` symlink at the previous release.
    Rollback { user_host: String, domain: String },
    /// Prepare this machine: directories, dropserve.conf, nginx include, certbot/nginx checks.
    Serve {
        /// Dropserve home (default: ~/.dropserve or DROPSERVE_HOME).
        #[arg(value_name = "MAIN_DIR")]
        main_dir: Option<PathBuf>,
        #[arg(long, default_value = "/etc/nginx/nginx.conf")]
        nginx_conf: PathBuf,
    },
    /// Server-side commands (normally invoked via SSH by the client).
    #[command(hide = true)]
    Remote {
        #[command(subcommand)]
        cmd: RemoteCmd,
    },
}

#[derive(Subcommand)]
enum RemoteCmd {
    Create {
        domain: String,
        artifact_zip: PathBuf,
        #[arg(long)]
        pocketbase: bool,
        #[arg(long, default_value_t = 8090)]
        pb_port: u16,
    },
    Update {
        domain: String,
        artifact_zip: PathBuf,
    },
    Delete {
        domain: String,
    },
    List,
    Rollback {
        domain: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Create {
            user_host,
            domain,
            path,
            pocketbase,
            pb_port,
        } => client::create(&user_host, &domain, &path, pocketbase, pb_port, cli.ssh_key)?,
        Commands::Update {
            user_host,
            domain,
            path,
        } => client::update(&user_host, &domain, &path, cli.ssh_key)?,
        Commands::Delete { user_host, domain } => client::delete(&user_host, &domain, cli.ssh_key)?,
        Commands::List { user_host } => client::list_sites(&user_host, cli.ssh_key)?,
        Commands::Rollback { user_host, domain } => {
            client::rollback(&user_host, &domain, cli.ssh_key)?
        }
        Commands::Serve {
            main_dir,
            nginx_conf,
        } => serve::run(main_dir, Some(nginx_conf))?,
        Commands::Remote { cmd } => {
            let home = remote::resolve_home(None);
            match cmd {
                RemoteCmd::Create {
                    domain,
                    artifact_zip,
                    pocketbase,
                    pb_port,
                } => remote::remote_create(&home, &domain, &artifact_zip, pocketbase, pb_port)?,
                RemoteCmd::Update {
                    domain,
                    artifact_zip,
                } => remote::remote_update(&home, &domain, &artifact_zip)?,
                RemoteCmd::Delete { domain } => remote::remote_delete(&home, &domain)?,
                RemoteCmd::List => remote::remote_list(&home)?,
                RemoteCmd::Rollback { domain } => remote::remote_rollback(&home, &domain)?,
            }
        }
    }

    Ok(())
}
