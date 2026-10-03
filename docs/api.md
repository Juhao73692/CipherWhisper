# API reference

## Opaque Relay API

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

## Center's loopback management API

This is Local Trust Layer administration, **not** the external peer protocol and **not** domain-internal device networking. The binary refuses non-loopback binds. Every management route requires `Authorization: Bearer <64-character admin.token or browser-session token>`; there is no unauthenticated status/identity route. Token is generated locally, retained across restart and never logged.

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
| `GET /status` | none | `{protocol,lastSync}`; background sync report |

Request types use snake_case to match Rust protocol. Returned Topic/Message/outbox objects use camelCase; ContactCard uses snake_case consistently with signed identity wire format.

Topic return shape:

```json
{"id":"uuid","peerId":"td_...","title":"数学","createdAt":0,"updatedAt":0,"archived":false}
```

Message return shape:

```json
{"id":"uuid","topicId":"uuid","senderId":"td_...","timestamp":0,"body":"$x^2$","format":"markdown","replyTo":null,"delivery":"queued"}
```

All API content is source text. The embedded local Svelte UI renders it with markdown-it / KaTeX / Shiki and final DOMPurify sanitization. Sending to an archived Topic is rejected; explicitly unarchive first. Sending can fail if the first session needs a Relay that is unavailable or has no prekeys. After session establishment, local sends can queue while Relay is offline.

Public UI routes: `GET /` / `GET /index.html` and embedded `/assets/*`; they contain no local secrets or history. `POST /ui/session {code}` exchanges a single-use, 90-second bootstrap code for an in-memory browser token (401 when expired/used/invalid). `serve --open` sends this code to the browser via URL fragment, never the permanent admin token. Browser tokens expire on server restart; page reload requires another unlock.

All routes check exact local Host and same Origin when supplied, reject cross-site Sec-Fetch-Site, emit no CORS permissions, and set restrictive CSP / no-store / frame protections. Wrong host/origin: 403. UI rendering and operation details: [local-ui.md](local-ui.md).

Management API authentication failures are 401. Application validation failures are 400 with `{error}`. A successful `/sync` request can include per-job errors; jobs remain durable for retries. Background synchronization is serialized with management mutations through one endpoint mutex to avoid concurrent ratchet advances.

## Deployment

Build release binaries with `cargo build --release --workspace --locked`. Run Relay and each center under separate least-privilege service accounts, each with its own SQLite directory and passphrase injection. Use a supervised service such as launchd/systemd; no platform installer is included.

Relay 可直接使用内置 TLS：`topicairn relay --bind 0.0.0.0:8787 --tls-cert server.pem --tls-key server-key.pem`。未提供 TLS 时拒绝非 loopback 监听。`topicairn tls-init --host <IP/DNS>` 可为受控测试生成证书；中心端点通过 `--relay-ca` 指定公开 CA。

Example TLS proxy for Relay only (requires independently configured Caddy and domain/DNS):

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

Centers use `--relay https://relay.example.com`. They never change identity because that address changes. Ports 8790/8791 above are local administration examples and are not exposed. Separate home Relays/Federation and device access need future implementations; do not infer those capabilities from the sample proxy.
