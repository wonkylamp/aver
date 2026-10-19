use aver::keys::Home;
use aver::services::Services;
use aver_core::{Receipt, ReceiptId};

#[derive(clap::Args)]
pub struct Args {
    id: ReceiptId,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let services = Services::open(home)?;
    let receipt: Receipt = services.store.get(&args.id)?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
