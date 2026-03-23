use std::io::IsTerminal;
use std::io::Write;
use std::process;

use clap::{Parser, Subcommand};
use tracing::debug;

use meygned_core::{AccessPolicy, ContentRef, KnsName};
use meygned_iroh::{IrohFetcher, IrohNode};
use meygned_kaspa::{KnsClient, PayloadScanner};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Meygned — decentralized web resolution built on KNS + Iroh
#[derive(Parser)]
#[command(
    name = "meygned",
    version,
    about = "Resolve and interact with Meygned-hosted content on .kas domains",
    long_about = None,
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

        /// Output the resolution record as JSON instead of content bytes.
        /// Includes KNS owner, Kaspa tx ID, Iroh content ref, and access policy.
        #[arg(long)]
        json: bool,
    },

    /// Show resolution metadata for a .kas name without fetching content
    Info {
        /// The .kas name to look up
        name: String,
    },

    /// Publish a Meygned content binding for a .kas name you own
    /// (stub — post-MVP, requires Kaspa wallet integration)
    Publish {
        /// The .kas name you own
        name: String,

        /// Iroh DocTicket or blob hash to bind
        #[arg(long)]
        ticket: Option<String>,

        /// Iroh blob hash to bind (for static sites)
        #[arg(long)]
        hash: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Init tracing — only shown if RUST_LOG is set, so it never pollutes output
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Resolve { name, path, json } => {
            cmd_resolve(&name, &path, json).await
        }
        Commands::Info { name } => {
            cmd_info(&name).await
        }
        Commands::Publish { name, ticket, hash } => {
            cmd_publish(&name, ticket, hash).await
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

async fn cmd_resolve(name: &str, path: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let is_tty = std::io::stdout().is_terminal();

    // Step 1: parse name
    let kns_name = KnsName::parse(name)?;

    if is_tty && !json {
        eprint_status(&format!("Resolving {}...", kns_name));
    }

    // Step 2: KNS owner lookup
    let kns = KnsClient::new();
    let kns_record = kns.get_domain(&kns_name.label).await?;

    debug!(owner = %kns_record.owner, "KNS owner found");

    if is_tty && !json {
        eprint_status(&format!(
            "Owner: {} (tx: {})",
            truncate(&kns_record.owner, 24),
            truncate(&kns_record.tx_id, 12)
        ));
    }

    // Step 3: Meygned payload scan
    let scanner = PayloadScanner::new();
    let scan_result = scanner
        .find_payload(&kns_record.owner, &kns_name.full())
        .await?;

    debug!(payload_tx = %scan_result.tx_id, "Meygned payload found");

    // --json: print record metadata and exit, no Iroh fetch needed
    if json {
        print_json_record(&kns_name, &kns_record, &scan_result)?;
        return Ok(());
    }

    // Step 4: access check
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
        eprint_status(&format!(
            "Fetching content via Iroh (path: {})...",
            path
        ));
    }

    // Step 5: spawn Iroh node and fetch content
    let node = IrohNode::spawn().await?;
    let fetcher = IrohFetcher::new(&node);
    let bytes = fetcher.fetch(&scan_result.payload.content_ref, path).await?;

    node.shutdown().await?;

    debug!(bytes = bytes.len(), "Content fetched");

    if is_tty {
        // Print a subtle stderr header so the user knows what they're seeing,
        // but stdout stays clean for piping
        eprintln!(
            "── {} ─── {} bytes ──────────────────",
            kns_name,
            bytes.len()
        );
    }

    // Write raw bytes to stdout — pipe-safe
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

    // Always pretty-print for info command
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
    println!("Content ref   {}", describe_content_ref(&scan_result.payload.content_ref));

    Ok(())
}

// ---------------------------------------------------------------------------
// publish (stub)
// ---------------------------------------------------------------------------

async fn cmd_publish(
    name: &str,
    ticket: Option<String>,
    hash: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let kns_name = KnsName::parse(name)?;

    eprintln!(
        "publish is not yet implemented in this MVP release."
    );
    eprintln!();
    eprintln!("To bind content to {}, you would need to:", kns_name);
    eprintln!("  1. Own {} in KNS (https://knsdomains.org)", kns_name);
    eprintln!("  2. Have an Iroh Doc ticket or blob hash ready");
    eprintln!("  3. Send a Kaspa transaction from your owner address");
    eprintln!("     with a MEYGNED: payload containing your ContentRef");
    eprintln!();

    if let Some(t) = ticket {
        eprintln!("Provided ticket: {}", truncate(&t, 40));
    }
    if let Some(h) = hash {
        eprintln!("Provided hash:   {}", h);
    }

    eprintln!();
    eprintln!("Wallet integration and publish command coming in a future release.");

    Ok(())
}

// ---------------------------------------------------------------------------
// JSON output for --json flag
// ---------------------------------------------------------------------------

fn print_json_record(
    name: &KnsName,
    kns_record: &meygned_core::KnsRecord,
    scan: &meygned_kaspa::PayloadScanResult,
) -> Result<(), Box<dyn std::error::Error>> {
    // Hand-built JSON so we don't need serde_json as a direct dep in main,
    // though it's already a transitive dep — this keeps the output shape
    // explicit and readable.
    let content_ref_json = match &scan.payload.content_ref {
        ContentRef::Blob { hash } => {
            format!(r#"{{"type":"blob","hash":"{}"}}"#, hash)
        }
        ContentRef::Doc { namespace_id, ticket, node_id, relay_url } => {
            let ticket_field = ticket
                .as_deref()
                .map(|t| format!(r#","ticket":"{}""#, t))
                .unwrap_or_default();
            let node_id_field = node_id
                .as_deref()
                .map(|n| format!(r#","node_id":"{}""#, n))
                .unwrap_or_default();
            let relay_field = relay_url
                .as_deref()
                .map(|r| format!(r#","relay_url":"{}""#, r))
                .unwrap_or_default();
            format!(
                r#"{{"type":"doc","namespace_id":"{}"{}{}{}}}"#,
                namespace_id, ticket_field, node_id_field, relay_field
            )
        }
    };

    let access_json = match &scan.payload.access_policy {
        None | Some(AccessPolicy::Public) => r#""public""#.to_string(),
        Some(AccessPolicy::Paywall { tx_id, description }) => {
            format!(
                r#"{{"type":"paywall","tx_id":"{}","description":"{}"}}"#,
                tx_id,
                description.as_deref().unwrap_or("")
            )
        }
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
        name,
        kns_record.owner,
        kns_record.tx_id,
        scan.tx_id,
        content_ref_json,
        access_json,
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Print a status line to stderr — never pollutes stdout data stream.
fn eprint_status(msg: &str) {
    eprintln!("\x1b[2m  → {}\x1b[0m", msg); // dim text
}

/// Truncate a long string for display, appending "…"
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Human-readable summary of a ContentRef.
fn describe_content_ref(cr: &ContentRef) -> String {
    match cr {
        ContentRef::Blob { hash } => format!("blob:{}", truncate(hash, 16)),
        ContentRef::Doc { namespace_id, ticket, .. } => {
            if ticket.is_some() {
                format!("doc:{} (ticket present)", truncate(namespace_id, 16))
            } else {
                format!("doc:{} (no ticket — DHT discovery)", truncate(namespace_id, 16))
            }
        }
    }
}
