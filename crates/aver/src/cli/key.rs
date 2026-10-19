use std::path::PathBuf;

use aver::keys::{self, Home};

#[derive(clap::Args)]
pub struct Args {
    /// The AVER_HOME whose verifier key to print, created if missing.
    #[arg(long)]
    home: Option<PathBuf>,
}

/// Prints the verifier (server) public key as unpadded base64url, the form
/// `public-key.txt` and the receipt viewer use, initializing the home first
/// if it has no keys. Only the key goes to stdout, so scripts can capture it.
pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let chosen;
    let home = match args.home {
        Some(dir) => {
            chosen = Home::new(dir);
            &chosen
        }
        None => home,
    };
    let init = keys::init(home)?;
    if init.changed {
        eprintln!("initialized {}", home.root().display());
    }
    println!("{}", init.server.to_file().public_key);
    Ok(())
}
