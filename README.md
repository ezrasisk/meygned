# Meygned

> Decentralized web hosting and resolution built on [KNS](https://knsdomains.org) + [Iroh](https://iroh.computer).

Own a `.kas` domain. Point it at content hosted peer-to-peer. Resolve it from anywhere — no Web2 servers, no registrars, no takedowns.

---

## What it does

Meygned lets anyone bind a KNS `.kas` domain to content hosted on the Iroh p2p network, then resolve it with a single command:

```bash
meygned resolve ezra.kas
```

That command does three things under the hood:

1. **KNS lookup** — asks the KNS API who owns `ezra.kas` and gets their Kaspa address
2. **Payload scan** — scans the owner's Kaspa transactions for a `MEYGNED:` payload containing an Iroh content reference
3. **Iroh fetch** — connects to the Iroh network and fetches the content, streaming raw bytes to stdout

No DNS. No ICANN. No central authority.

---

## How ownership works

Meygned does not manage name ownership — that's entirely handled by [KNS](https://knsdomains.org), which inscribes `.kas` names permanently onto the Kaspa blockchain using KIP-14 transaction payloads.

To publish content on Meygned, you need to:

1. Own a `.kas` domain via KNS
2. Host your content on Iroh and get a Doc ticket or blob hash
3. Send a Kaspa transaction from your owner address with a `MEYGNED:` payload binding the two together

The Meygned resolver verifies that the payload signer matches the KNS-registered owner — so no one can hijack your name.

---

## Installation

### Prerequisites

- Rust toolchain (`rustup` recommended) — [install here](https://rustup.rs)
- A KNS-registered `.kas` domain — [register here](https://knsdomains.org)

### Build from source

```bash
git clone https://github.com/ezrasisk/meygned
cd meygned
cargo build --release -p meygned
```

The binary lands at `./target/release/meygned`.

Optionally move it to your PATH:

```bash
cp target/release/meygned ~/.local/bin/
```

---

## Usage

### Resolve a name

```bash
# Fetch content and print to stdout
meygned resolve ezra.kas

# Fetch a specific path within a Doc
meygned resolve ezra.kas --path /about.html

# Pipe directly to a browser or file
meygned resolve ezra.kas > index.html
```

### Inspect a record without fetching content

```bash
# Human-readable metadata
meygned info ezra.kas

# Machine-readable JSON (owner, tx IDs, content ref, access policy)
meygned resolve ezra.kas --json
```

**Example `--json` output:**

```json
{
  "name": "ezra.kas",
  "owner": "kaspa:qz...",
  "kns_tx_id": "a3f9...",
  "payload_tx_id": "7bc1...",
  "content_ref": {
    "type": "doc",
    "namespace_id": "ns_abc...",
    "ticket": "docticket1..."
  },
  "access_policy": "public"
}
```

### Output behaviour

Meygned follows Unix conventions:

- **stdout** — always raw content bytes, safe to pipe
- **stderr** — human-readable status lines (dim text), only shown in a terminal
- Setting `RUST_LOG=debug` prints detailed resolution traces to stderr

---

## Architecture

Meygned is a modular Rust workspace of four crates:

```
meygned/
  meygned-core/     Shared types, MeygnedPayload, KnsName, errors
  meygned-kaspa/    KNS API client + Kaspa transaction payload scanner
  meygned-iroh/     Iroh node management + content fetching
  meygned-cli/      meygned binary — wires everything together
```

### Resolution flow

```
meygned resolve ezra.kas
        │
        ▼
KnsName::parse("ezra.kas")
        │
        ▼
GET api.knsdomains.org/mainnet/api/v1/domain/ezra
→ { owner: "kaspa:qz...", tx_id: "..." }
        │
        ▼
GET api.kaspa.org/addresses/{owner}/full-transactions
→ scan for MEYGNED: payload where payload.name == "ezra.kas"
→ verify signer == KNS owner
        │
        ▼
IrohNode::spawn()
IrohFetcher::fetch(content_ref, "/")
→ Vec<u8>
        │
        ▼
stdout
```

### Content references

A Meygned payload can point to either:

| Type | Use case | Mutability |
|------|----------|-----------|
| `blob` | Static sites, single files | Immutable — content is the hash |
| `doc` | Dynamic sites, apps | Mutable via Iroh CRDT sync |

Doc keys mirror URL paths: `"/"` resolves the root, `"/app.js"` resolves a script, and so on.

### Publishing a payload

A `MeygnedPayload` is a JSON string prefixed with `MEYGNED:` and inscribed into a Kaspa transaction payload from your KNS owner address:

```json
MEYGNED:{
  "version": 1,
  "name": "ezra.kas",
  "content_ref": {
    "type": "doc",
    "namespace_id": "ns_abc...",
    "ticket": "docticket1..."
  },
  "access_policy": { "type": "public" }
}
```

The `publish` subcommand (wallet integration) is planned for a future release. For now, payloads can be inscribed manually via any Kaspa wallet that supports raw payload data.

---

## Crate reference

### `meygned-core`

Shared types used across all crates.

| Type | Description |
|------|-------------|
| `KnsName` | Parsed, validated `.kas` name |
| `MeygnedPayload` | On-chain content binding — serialized into Kaspa tx payload |
| `ContentRef` | Iroh pointer — `Blob { hash }` or `Doc { namespace_id, ticket, ... }` |
| `AccessPolicy` | `Public` or `Paywall { tx_id }` |
| `KnsRecord` | KNS API response — name, owner, tx_id |
| `MeygnedRecord` | Fully assembled resolver output |
| `MeygnedError` | Unified error type |

### `meygned-kaspa`

KNS API client and Kaspa transaction scanner.

```rust
// Full resolution in two steps
let kns = KnsClient::new();
let scanner = PayloadScanner::new();
let (kns_record, scan_result) = resolve_name(&kns, &scanner, "ezra.kas").await?;
```

### `meygned-iroh`

Iroh node lifecycle and content fetching.

```rust
let node = IrohNode::spawn().await?;
let fetcher = IrohFetcher::new(&node);
let bytes = fetcher.fetch(&content_ref, "/").await?;
node.shutdown().await?;
```

Doc resolution uses a three-tier routing strategy: full ticket → node_id dial → local store fallback.

---

## Roadmap

- [x] KNS name resolution via public API
- [x] Meygned payload scanning + anti-hijack validation
- [x] Iroh blob and doc content fetching
- [x] `meygned resolve` CLI with terminal/pipe detection
- [ ] `meygned publish` — wallet integration for payload inscription
- [ ] Paywall access via Igra L2 transaction verification
- [ ] Persistent Iroh blob store (swap in-memory for on-disk)
- [ ] Browser extension / local proxy for native `.kas` resolution
- [ ] Raspberry Pi / low-resource hosting guide

---

## Contributing

This project is in active early development. Issues and PRs welcome.

```bash
cargo test --workspace   # run all tests
cargo clippy --workspace # lint
```

---

## License

MIT
