# API reference

## Default direct Peer API

`serve` defaults to direct P2P, without `--relay`. Separate P2P listener defaults to `127.0.0.1:8800`; loopback UI/admin defaults to `127.0.0.1:8790`. Remote P2P requires TLS; setup and two-instance testing: [p2p-testing.md](p2p-testing.md).

The loopback Bearer-authenticated API adds:

| Method/path | Body/result |
|---|---|
| `GET /p2p/contact` | This center's signed PeerProfile (identity + address + optional CA) |
| `GET /p2p/peers` | Imported signed PeerProfiles |
| `POST /p2p/peers` | Import verified PeerProfile; pins identity and updates route |
| `POST /p2p/peers/{user_id}/check` | Empty JSON body; mutually authenticated ping, returns pinned ContactCard |

These configuration routes belong to center mode; devices do not own external routes. `GET /status` includes `transport:"direct"|"relay"|"device"`. Existing `/identity` and `/peers` remain bare ContactCard APIs; bare cards alone have no direct route. Configure the PeerProfile on the center before devices send to a new external Peer.

Remote P2P listener exposes only:

| Method/path | Result data inside signed PeerResponse |
|---|---|
| `POST /p2p/v1/ping` | ContactCard; empty request body |
| `POST /p2p/v1/prekeys/claim` | PrekeyBundle; empty request body; atomic one-time consumption |
| `POST /p2p/v1/messages` | Delivery after storing signed Envelope addressed to this center |
| `GET /p2p/v1/messages/{id}` | Delivery; only original sender; confirmed after decryption/commit |

All require the five `x-peer-*` headers and signed target-bound request detailed in [protocol.md](protocol.md). Responses bind identity, request nonce and complete result. Unknown/expired/replayed authentication returns generic 401; invalid envelope/body/ID/capacity errors return 400. No third-party forwarding, public discovery, browser CORS, plaintext history, Topic, search or admin routes. First connection needs both peers online; established sessions can queue locally while offline.

## Optional legacy opaque Relay API

All routes except health require the four `x-td-*` signed-request headers described in [protocol.md](protocol.md). The Relay computes the authenticated user from the signing public key.

| Method/path | Request | Response and authorization |
|---|---|---|
| `GET /health` | none | `{status:"ok",protocol:1}` |
| `POST /prekeys` | PrekeyUpload JSON | `{ok:true}`, authenticated identity must own every signed key |
| `POST /prekeys/{user}/claim` | **empty body** | PrekeyBundle, atomically consumes one one-time key; any authenticated sender may claim |
| `POST /messages` | Envelope JSON | `{id,acknowledged}`, Envelope.from must be authenticated user; repeat identical Envelope is idempotent |
| `GET /messages?cursor=0` | none | `{items:[{cursor,envelope}],next_cursor}`, caller's unacknowledged inbox only; max 100 items |
| `POST /messages/{id}/ack` | **empty body** | `{id,acknowledged:true}`, caller must be recipient; idempotent |
| `GET /messages/{id}/delivery` | none | `{id,acknowledged}`, caller must be sender |

Error JSON is `{error:"..."}`. Unauthorized/expired/replayed signatures: 401. Unknown prekeys or inaccessible messages: 404. Envelope ID with different digest: 409. Per-recipient 10000 unacknowledged queue capacity: 429. Invalid JSON/keys/version: 400. Unknown/inaccessible IDs intentionally share the 404 response. Relay cannot tell whether a recipient's application decrypted an event; it only observes authenticated ACK.

Relay has no Topic, search, Markdown, rendering, reply or attachment APIs. Database has ciphertext queue rows, signed public prekeys, consumed one-time-key records and used request nonces.

## Shared center/client loopback management API

This is local administration, separate from both external peer and internal device transport. The binary refuses non-loopback binds. `serve` operates the center; `connect` operates a separately keyed replica using the same API/UI. Every management route requires `Authorization: Bearer <64-character admin.token or browser-session token>`; there is no unauthenticated status/identity route. Token is generated locally, retained across restart and never logged.

| Method/path | JSON/query | Result |
|---|---|---|
| `GET /identity` | none | Public signed ContactCard |
| `GET /peers` | none | ContactCard[] |
| `POST /peers` | ContactCard | `{ok:true}`; verifies self-signature and pins keys |
| `GET /topics?peer=<user_id>` | optional peer query | Topic[]; includes archived |
| `POST /topics` | `{peer_id,title}` | New local Topic |
| `POST /topics/{id}` | `{title,archived}` | Topic; queues encrypted topic.update |
| `GET /topics/{id}/messages` | none | Message[]; Markdown source |
| `POST /topics/{id}/messages` | `{body,reply_to?:uuid}` | Message in `queued` state; creates initial session when needed |
| `GET /search?q=<phrase>` | literal phrase, URL encoded | Matching Message[]; max 100; local SQLite FTS5 trigram substrings; one/two-character terms use bound literal scanning |
| `POST /sync` | ignored body, e.g. `{}` | `{sent,received,acknowledged,delivered,errors}` |
| `GET /outbox` | none | `[{id,accepted,attempts,nextAttempt,lastError}]`, excludes ciphertext |
| `GET /status` | none | `{protocol,mode,device,deviceServer,lastSync}`; server/client mode; client device includes card/domainId/server/cursor/acknowledgedCursor |
| `GET /devices` | none | `{enabled,devices:[DeviceStatus]}`; center enrollments and revoked records; client returns disabled/empty |
| `POST /devices` | DeviceCard | Center only: verify/enroll, returns signed Pairing |
| `POST /devices/{id}/revoke` | `{}` | Center only: permanent revocation |
| `GET /device-pending` | none | Client: `[{id,operation,state,error}]`; center empty |
| `POST /device-pending/{id}/discard` | `{}` | Client only: discard explicitly rejected operation and local failed body; pending/uncertain operations forbidden |

Request types use snake_case to match Rust protocol. Returned Topic/Message/outbox objects use camelCase; ContactCard uses snake_case consistently with signed identity wire format.

Topic return shape:

```json
{"id":"uuid","peerId":"td_...","title":"数学","createdAt":0,"updatedAt":0,"archived":false}
```

Message return shape:

```json
{"id":"uuid","topicId":"uuid","senderId":"td_...","timestamp":0,"body":"$x^2$","format":"markdown","replyTo":null,"delivery":"queued"}
```

All API content is source text. The embedded local Svelte UI renders it with markdown-it / KaTeX / Shiki and final DOMPurify sanitization. Sending to an archived Topic is rejected; explicitly unarchive first. Default direct P2P requires the peer online for first-session prekey claim; after session establishment, sends queue locally while the peer is offline. Only explicit legacy `--relay` mode uses Relay prekeys/queue.

Public UI routes: `GET /` / `GET /index.html` and embedded `/assets/*`; they contain no local secrets or history. `POST /ui/session {code}` exchanges a single-use, 90-second bootstrap code for an in-memory browser token (401 when expired/used/invalid). `serve --open` sends this code to the browser via URL fragment, never the permanent admin token. Browser tokens expire on server restart; page reload requires another unlock.

All routes check exact local Host and same Origin when supplied, reject cross-site Sec-Fetch-Site, emit no CORS permissions, and set restrictive CSP / no-store / frame protections. Wrong host/origin: 403. UI rendering and operation details: [local-ui.md](local-ui.md).

Management API authentication failures are 401. Application validation failures are 400 with `{error}`. A successful `/sync` request can include per-job errors; jobs remain durable for retries or explicit review after rejection. Background synchronization is serialized with mutations through one workspace mutex to avoid concurrent ratchet advances or cursor commits.

Client sends persist an operation immediately and return a queued Message; accepted center results arrive via sync. Metadata operations wait for center acceptance; on uncertain network outcome they remain durable despite the local API returning an error. Do not resubmit. Rejected operations are retained for review; failed local messages use delivery `failed`. On clients `/identity` returns the center's public identity, `/outbox` describes device operations, and no center private keys or peer sessions are held locally.

## Center's independent HTTPS device API

Enabled only with all `--device-*` options. TLS 1.3 only; device-signed native requests required on every route. Public HTTPS origin/CA are bound in a center-signed Pairing; there is no unauthenticated enrollment route. This listener exposes no local UI or administrator operations. Authentication/revocation/replay failures are 401, application validation errors 400. Successful bodies are nonce-bound center-signed `SignedResponse<T>` described in [protocol.md](protocol.md).

| Method/path | Body/query | Signed data |
|---|---|---|
| `GET /device/v1/changes?epoch=<uuid>&cursor=<n>&limit=100` | Empty body; count 1..100; pages also bound encoded bytes | Page with immutable versioned peer/topic/message snapshots |
| `POST /device/v1/ack` | `{epoch,cursor}` | Ack; cannot exceed served cursor; per-device, no shared-history deletion |
| `POST /device/v1/commands` | `{id,operation}` | CommandReply; durable accepted/rejected receipt, exact command retry idempotent |

Enrolled devices have full history and conversation write access, with no right to manage enrollment. Data is JSON inside authenticated TLS; it is never plaintext on the network. This is domain-owned history, separate from opaque external Peer messages. TLS endpoints are center/device; no third-party transport participates. Operational guide: [device-sync.md](device-sync.md).

## Deployment

Build release binaries with `cargo build --release --workspace --locked`. Default centers connect directly; configure a separate TLS P2P listener as in [p2p-testing.md](p2p-testing.md). If explicitly using legacy Relay, run it and each center under separate least-privilege service accounts, each with its own SQLite directory and passphrase injection. Use a supervised service such as launchd/systemd; no platform installer is included.

Relay 可直接使用内置 TLS：`cipherwhisper relay --bind 0.0.0.0:8787 --tls-cert server.pem --tls-key server-key.pem`。未提供 TLS 时拒绝非 loopback 监听。`cipherwhisper tls-init --host <IP/DNS>` 可为受控测试生成证书；中心端点通过 `--relay-ca` 指定公开 CA。

Example TLS proxy for Relay only (requires independently configured Caddy and domain/DNS):

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

Centers use `--relay https://relay.example.com`. They never change identity because that address changes. Ports 8790/8791 are local administration examples and are not exposed. Device access uses the separate authenticated TLS listener, e.g. 8792. Separate home Relays/Federation remain future work; the sample proxy does not expose the local management UI.

## Chat extension

All endpoints below use the existing loopback authorization and origin checks.

| Endpoint | Request / response |
| --- | --- |
| `GET /topics/{id}/page` | Optional `before` message ID, `around` message ID, `limit` (1..100, default 50). Returns `{items,olderCursor,hasMore,revision}`. |
| `GET /topics/{id}/changes?since=N` | Returns at most 100 changed message views, `{items,revision,hasMore}`. Continue from the returned revision. |
| `GET /topics/{id}/draft` | Returns `{body,replyTo,revision}` for the local encrypted draft. |
| `POST /topics/{id}/draft` | `{body,replyTo,revision}`; monotonically newer writes replace older drafts, including empty drafts. |
| `POST /topics/{id}/special` | `{version,kind,data}`; validates author/topic binding and sends an explicit encrypted `Control` event. Ordinary `/messages` text never dispatches controls. |
| `POST /topics/{id}/files` | `{name,mime,hex}` for a local source file, maximum 16 MiB decoded; stages encrypted chunks and sends an ordinary file invitation. |
| `GET /files/{offerMessageId}/download` | Authenticated binary attachment, available to the receiving side after complete SHA-256 verification. |

`GET /topics` additionally returns `pinned`, `tags` and `status`. Chat page items include `sequence`, `edited`, `withdrawn`, unknown-message details and optional `file` state. The older `/topics/{id}/messages` endpoint remains available for CLI/integration compatibility. See [chat feature protocol](chat-features.md) for ordinary file metadata and versioned control payloads.
