use std::io::IsTerminal;
use std::io::Write;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use tracing::debug;

use meygned_core::{AccessPolicy, ContentRef, KnsName};
use meygned_iroh::{IrohFetcher, IrohNode};
use meygned_kaspa::{
    publisher::{Publisher, DEFAULT_RPC_URL},
    wallet::{WalletHandle, default_wallet_path},
    KnsClient, PayloadScanner,
};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Meygned — decentralized web resolution built on KNS + Iroh
#[derive(Parser)]
#[command(
    name = "meygned",
    version,
    about = "Resolve and publish Meygned-hosted content on .kas domains",
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Resolve a .kas name and fetch its content
    Resolve {
        /// The .kas name to resolve, e.g. "ezra.kas" or "ezra"
        name: String,

        /// Path within a Doc to fetch (default: "/")
        #[arg(long, default_value = "/")]
        path: String,

        /// Output the resolution record as JSON instead of content bytes
        #[arg(long)]
        json: bool,
    },

    /// Show resolution metadata for a .kas name without fetching content
    Info {
        /// The .kas name to look up
        name: String,
    },

    /// Bind Iroh content to a .kas name you own via a signed Kaspa transaction
    Publish {
        /// The .kas name you own (must be registered in KNS)
        name: String,

        /// Iroh DocTicket for a mutable Doc (dynamic sites)
        #[arg(long, conflicts_with = "hash")]
        ticket: Option<String>,

        /// Iroh BLAKE3 blob hash (static sites)
        #[arg(long, conflicts_with = "ticket")]
        hash: Option<String>,

        /// Make content paywall-gated (provide a Kaspa tx_id as proof)
        #[arg(long)]
        paywall: Option<String>,

        /// Paywall description shown to users attempting access
        #[arg(long, requires = "paywall")]
        paywall_desc: Option<String>,

        /// Path to wallet file (default: ~/.meygned/wallet.wallet)
        #[arg(long)]
        wallet: Option<PathBuf>,

        /// Wallet account index to use (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,

        /// Kaspa node wRPC URL (default: ws://127.0.0.1:17110)
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        node: String,
    },

    /// Wallet management commands
    Wallet {
        #[command(subcommand)]
        action: WalletCommands,
    },
}

#[derive(Subcommand)]
enum WalletCommands {
    /// Create a new Meygned wallet
    Create {
        /// Path to save the wallet file (default: ~/.meygned/wallet.wallet)
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Show the receive address for your wallet
    Address {
        /// Wallet file path (default: ~/.meygned/wallet.wallet)
        #[arg(long)]
        wallet: Option<PathBuf>,
        /// Account index (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Resolve { name, path, json } => cmd_resolve(&name, &path, json).await,
        Commands::Info { name } => cmd_info(&name).await,
        Commands::Publish {
            name, ticket, hash, paywall, paywall_desc, wallet, account, node,
        } => cmd_publish(name, ticket, hash, paywall, paywall_desc, wallet, account, node).await,
        Commands::Wallet { action } => match action {
            WalletCommands::Create { path } => cmd_wallet_create(path).await,
            WalletCommands::Address { wallet, account } => cmd_wallet_address(wallet, account).await,
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

async fn cmd_resolve(
    name: &str,
    path: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let is_tty = std::io::stdout().is_terminal();
    let kns_name = KnsName::parse(name)?;

    if is_tty && !json {
        eprint_status(&format!("Resolving {}...", kns_name));
    }

    let kns = KnsClient::new();
    let kns_record = kns.get_domain(&kns_name.label).await?;

    if is_tty && !json {
        eprint_status(&format!(
            "Owner: {} (kns tx: {})",
            truncate(&kns_record.owner, 24),
            truncate(&kns_record.tx_id, 12)
        ));
    }

    let scanner = PayloadScanner::new();
    let scan_result = scanner
        .find_payload(&kns_record.owner, &kns_name.full())
        .await?;

    if json {
        print_json_record(&kns_name, &kns_record, &scan_result)?;
        return Ok(());
    }

    if let Some(AccessPolicy::Paywall { tx_id, description }) =
        &scan_result.payload.access_policy
    {
        let desc = description.as_deref().unwrap_or("payment required");
        return Err(format!(
            "access denied: {} (paywall tx: {})",
            desc,
            truncate(tx_id, 12)
        )
        .into());
    }

    if is_tty {
        eprint_status(&format!("Fetching content via Iroh (path: {})...", path));
    }

    let node = IrohNode::spawn().await?;
    let fetcher = IrohFetcher::new(&node);
    let bytes = fetcher.fetch(&scan_result.payload.content_ref, path).await?;
    node.shutdown().await?;

    if is_tty {
        eprintln!(
            "── {} ─── {} bytes ──────────────────",
            kns_name,
            bytes.len()
        );
    }

    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

async fn cmd_info(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let kns_name = KnsName::parse(name)?;
    eprint_status(&format!("Looking up {}...", kns_name));

    let kns = KnsClient::new();
    let kns_record = kns.get_domain(&kns_name.label).await?;

    let scanner = PayloadScanner::new();
    let scan_result = scanner
        .find_payload(&kns_record.owner, &kns_name.full())
        .await?;

    println!("Name          {}", kns_name);
    println!("Owner         {}", kns_record.owner);
    println!("KNS tx        {}", kns_record.tx_id);
    println!("Payload tx    {}", scan_result.tx_id);
    println!(
        "Access        {}",
        match &scan_result.payload.access_policy {
            None | Some(AccessPolicy::Public) => "public".to_string(),
            Some(AccessPolicy::Paywall { description, .. }) => format!(
                "paywall ({})",
                description.as_deref().unwrap_or("no description")
            ),
        }
    );
    println!(
        "Content ref   {}",
        describe_content_ref(&scan_result.payload.content_ref)
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// publish
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn cmd_publish(
    name: String,
    ticket: Option<String>,
    hash: Option<String>,
    paywall: Option<String>,
    paywall_desc: Option<String>,
    wallet_path: Option<PathBuf>,
    account: u32,
    node_url: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let kns_name = KnsName::parse(&name)?;

    // Build ContentRef from flags
    let content_ref = match (ticket, hash) {
        (Some(t), None) => ContentRef::Doc {
            namespace_id: extract_namespace_from_ticket(&t)?,
            ticket: Some(t),
            node_id: None,
            relay_url: None,
        },
        (None, Some(h)) => ContentRef::Blob { hash: h },
        (None, None) => {
            return Err(
                "provide either --ticket <iroh-doc-ticket> or --hash <blake3-hash>".into(),
            )
        }
        (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents this"),
    };

    // Build access policy
    let access_policy = paywall.map(|tx_id| AccessPolicy::Paywall {
        tx_id,
        description: paywall_desc,
    });

    // Verify KNS ownership before doing any wallet work
    eprint_status(&format!("Verifying KNS ownership of {}...", kns_name));
    let kns = KnsClient::new();
    let kns_record = kns.get_domain(&kns_name.label).await?;

    eprint_status(&format!(
        "Confirmed owner: {}",
        truncate(&kns_record.owner, 32)
    ));

    // Open wallet
    let wallet_file = wallet_path.unwrap_or_else(default_wallet_path);
    eprint_status(&format!("Opening wallet: {}", wallet_file.display()));

    let wallet = WalletHandle::open(&wallet_file, meygned_kaspa::signer::SignerNetwork::Mainnet)
        .await?;

    // Verify wallet address matches KNS owner
    let wallet_address = wallet.receive_address(account).await?;
    if wallet_address.to_string().to_lowercase() != kns_record.owner.to_lowercase() {
        return Err(format!(
            "wallet address {} does not match KNS owner {} for {}.\n\
             Make sure you are using the correct wallet and account (--account {})",
            truncate(&wallet_address.to_string(), 24),
            truncate(&kns_record.owner, 24),
            kns_name,
            account
        )
        .into());
    }

    eprint_status(&format!("Address verified — matches KNS owner ✓"));
    eprint_status(&format!("Connecting to Kaspa node at {}...", node_url));

    // Publish
    let publisher = Publisher::new(&node_url);
    let result = publisher
        .publish(&kns_name, content_ref, access_policy, &wallet, account)
        .await?;

    // Success output
    eprintln!();
    eprintln!("  ✓ Published successfully");
    eprintln!("  ─────────────────────────────────────────");
    println!("{}", result.tx_id);  // tx_id to stdout for scripting
    eprintln!("  Name          {}", kns_name);
    eprintln!("  Payload tx    {}", result.tx_id);
    eprintln!("  Payload size  {} bytes", result.payload_size_bytes);
    eprintln!(
        "  Fee paid      {} sompi ({:.8} KAS)",
        result.fee_sompi,
        result.fee_sompi as f64 / 100_000_000.0
    );
    eprintln!();
    eprintln!("  Resolvers can now find your content with:");
    eprintln!("    meygned resolve {}", kns_name);

    Ok(())
}

// ---------------------------------------------------------------------------
// wallet subcommands
// ---------------------------------------------------------------------------

async fn cmd_wallet_create(
    path: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let wallet_file = path.unwrap_or_else(default_wallet_path);

    if wallet_file.exists() {
        return Err(format!(
            "wallet already exists at '{}' — delete it first if you want a new one",
            wallet_file.display()
        )
        .into());
    }

    eprintln!("Creating new Meygned wallet at: {}", wallet_file.display());

    WalletHandle::create_new(&wallet_file, meygned_kaspa::signer::SignerNetwork::Mainnet).await?;

    eprintln!("Wallet created. Get your receive address with:");
    eprintln!("  meygned wallet address");

    Ok(())
}

async fn cmd_wallet_address(
    wallet_path: Option<PathBuf>,
    account: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let wallet_file = wallet_path.unwrap_or_else(default_wallet_path);
    let wallet = WalletHandle::open(&wallet_file, meygned_kaspa::signer::SignerNetwork::Mainnet)
        .await?;
    let address = wallet.receive_address(account).await?;

    // Address to stdout for scripting
    println!("{}", address);
    eprintln!("  ↑ Fund this address with KAS before publishing.");
    eprintln!("  Register it as your KNS name owner at https://knsdomains.org");

    Ok(())
}

// ---------------------------------------------------------------------------
// JSON record output
// ---------------------------------------------------------------------------

fn print_json_record(
    name: &KnsName,
    kns_record: &meygned_core::KnsRecord,
    scan: &meygned_kaspa::PayloadScanResult,
) -> Result<(), Box<dyn std::error::Error>> {
    let content_ref_json = match &scan.payload.content_ref {
        ContentRef::Blob { hash } => format!(r#"{{"type":"blob","hash":"{}"}}"#, hash),
        ContentRef::Doc { namespace_id, ticket, node_id, relay_url } => {
            let ticket_f = ticket.as_deref().map(|t| format!(r#","ticket":"{}""#, t)).unwrap_or_default();
            let node_f = node_id.as_deref().map(|n| format!(r#","node_id":"{}""#, n)).unwrap_or_default();
            let relay_f = relay_url.as_deref().map(|r| format!(r#","relay_url":"{}""#, r)).unwrap_or_default();
            format!(r#"{{"type":"doc","namespace_id":"{}"{}{}{}}}"#, namespace_id, ticket_f, node_f, relay_f)
        }
    };

    let access_json = match &scan.payload.access_policy {
        None | Some(AccessPolicy::Public) => r#""public""#.to_string(),
        Some(AccessPolicy::Paywall { tx_id, description }) => format!(
            r#"{{"type":"paywall","tx_id":"{}","description":"{}"}}"#,
            tx_id,
            description.as_deref().unwrap_or("")
        ),
    };

    println!(
        r#"{{
  "name": "{}",
  "owner": "{}",
  "kns_tx_id": "{}",
  "payload_tx_id": "{}",
  "content_ref": {},
  "access_policy": {}
}}"#,
        name, kns_record.owner, kns_record.tx_id, scan.tx_id, content_ref_json, access_json,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn eprint_status(msg: &str) {
    eprintln!("\x1b[2m  → {}\x1b[0m", msg);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn describe_content_ref(cr: &ContentRef) -> String {
    match cr {
        ContentRef::Blob { hash } => format!("blob:{}", truncate(hash, 16)),
        ContentRef::Doc { namespace_id, ticket, .. } => {
            if ticket.is_some() {
                format!("doc:{} (ticket present)", truncate(namespace_id, 16))
            } else {
                format!("doc:{} (DHT discovery)", truncate(namespace_id, 16))
            }
        }
    }
}

/// Extract the namespace_id from an Iroh DocTicket string.
/// Iroh tickets encode the namespace as the first component.
/// For MVP, store the full ticket as namespace_id if parsing is ambiguous.
fn extract_namespace_from_ticket(ticket: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Iroh DocTickets are structured strings; the namespace can be derived
    // from the ticket itself via the iroh-docs crate at runtime.
    // For the ContentRef we store the ticket and use a placeholder namespace_id
    // that gets resolved when the iroh node opens the doc.
    // TODO: use iroh_docs::DocTicket::from_str(ticket)?.capability.id().to_string()
    // once meygned-iroh is in scope here.
    Ok(format!("from_ticket:{}", &ticket[..ticket.len().min(32)]))
}
