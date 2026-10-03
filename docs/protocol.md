# Topicairn protocol v1

Transport-independent conversation events are encrypted between **Trust Domain center endpoints**. Local devices are outside this protocol. Wire format is UTF-8 JSON; time values are Unix seconds. IDs are UUID strings except user IDs, which are Ed25519 fingerprints.

## Public identity

```json
{
  "version": 1,
  "user_id": "td_<64 lowercase hex>",
  "signing_key": "<Ed25519 unpadded base64>",
  "curve_key": "<X25519 unpadded base64>",
  "label": "Alice",
  "signature": "<Ed25519 unpadded base64>"
}
```

The signature authenticates the exact compact JSON array:

```text
["topicairn.contact.v1", version, user_id, signing_key, curve_key, label]
```

Array order is fixed. There are no floats or object key ordering concerns. Encoding uses serde_json compact JSON serialization, including its escaping of strings, and no trailing newline. Interoperating implementations must match those bytes exactly. The `user_id` hash uses the decoded 32-byte signing public key, not its base64 text. Validate strict Ed25519 signatures and reject mismatched ID/version or invalid Curve25519 public-key encodings. A Contact Card must be exchanged and verified out of band, then pinned.

## Signed prekeys

A prekey is `{key, expires_at, signature}`. Sign compact JSON:

```text
["topicairn.prekey.v1", user_id, curve_key, prekey.key, expires_at, fallback_boolean]
```

`fallback_boolean` is true for signed_prekey, false for one-time keys. Expires_at must be future and at most 32 days ahead of the validating machine's current time. Publishing:

```json
{
  "identity": "<ContactCard object>",
  "signed_prekey": "<SignedPrekey object>",
  "one_time_prekeys": ["<SignedPrekey object>"]
}
```

These angle-bracket strings stand for nested objects, not literal string wire values. Claim returns:

```text
{identity: ContactCard, signed_prekey: SignedPrekey, one_time_prekey: SignedPrekey | null}
```

The Relay atomically marks one one-time key consumed, keeping a tombstone so republishing does not resurrect it. The sender validates all signatures and both identity keys against the imported Peer. It passes the selected one-time key (or fallback) and pinned Curve25519 identity to vodozemac Account::create_outbound_session. Receiver uses pinned sender Curve25519 identity with Account::create_inbound_session.

## Envelope

```json
{
  "version": 1,
  "id": "<uuid>",
  "from": "td_<Alice fingerprint>",
  "to": "td_<Bob fingerprint>",
  "ciphertext": "<serialized opaque crypto container>",
  "timestamp": 1790990000,
  "signature": "<Ed25519 signature>"
}
```

Sign compact JSON:

```text
["topicairn.envelope.v1", version, id, from, to, ciphertext, timestamp]
```

The opaque crypto container is serialized JSON `{session_id, message}`. `message` is vodozemac's standard OlmMessage serialization `{type, body}`: 0 prekey, 1 normal; body is unpadded base64 library message bytes. Relay does not parse this container or understand session internals. It may learn visible cryptographic headers and session linkability, but no conversation semantics.

Envelope digest is SHA-256 of its signing_bytes. Reusing an ID with a different digest is rejected. Signature is checked against the pinned sender before client decryption. Full routing metadata is redundantly authenticated inside encrypted Payload.

## Encrypted Payload

```json
{
  "version": 1,
  "envelope_id": "<matching outer UUID>",
  "sender": "<matching from>",
  "recipient": "<matching to>",
  "timestamp": 1790990000,
  "event": {
    "type": "message",
    "message_id": "<uuid>",
    "topic_id": "<uuid>",
    "topic_title": "数学",
    "created_at": 1790990000,
    "body": "证明：$$E=mc^2$$",
    "format": "markdown",
    "reply_to": null
  }
}
```

Body is source text, never HTML output. No normalization or parsing is performed. First received message creates its Topic if missing. Existing Topic must belong to the same Peer; existing reply targets must belong to that Topic. Message IDs are unique locally; a reused application message ID in a different fresh Envelope is treated as a collision. Normal transport retries reuse the original Envelope and are ignored by received_envelopes deduplication.

Topic update event:

```json
{
  "type": "topic.update",
  "topic_id": "<uuid>",
  "title": "新标题",
  "created_at": 1790990000,
  "archived": true
}
```

Unknown payload fields, event types and protocol versions fail closed; no automatic fallback to plaintext. Future edit/delete/read/reaction/attachment types extend Conversation Layer after their authorization/conflict rules are specified. There are no separate Relay routes for these semantics.

## Relay HTTP authentication

All Relay routes except `/health` require these headers:

```text
x-td-key: <Ed25519 signing public key>
x-td-time: <Unix seconds>
x-td-nonce: <fresh UUID>
x-td-signature: <Ed25519 signature>
```

Sign compact JSON array:

```text
["topicairn.http.v1", UPPERCASE_METHOD, PATH_WITH_EXACT_QUERY,
 SHA256_HEX(EXACT_BODY_BYTES), timestamp, nonce]
```

For GET or no-body POST, body bytes are empty. For POST JSON, client serializes the body once, signs those exact bytes, then transmits them. Requests with times more than 300 seconds apart are rejected. Maintain synchronized system clocks. Nonce is persisted at Relay and cannot be reused within the validity window. Retries receive a fresh request nonce/signature while keeping immutable Envelope ciphertext unchanged. Transport must use TLS outside loopback.

## Delivery

Local delivery states:

- `queued`: ratchet, plaintext and encrypted outbox committed locally.
- `sent`: Relay accepted the immutable Envelope.
- `delivered`: recipient center committed plaintext/ratchet and ACK reached Relay.
- `received`: locally received message.

Delivery states are based on Relay reports, not cryptographic end-to-end receipts. An honest Relay reports delivered after a recipient ACK; a malicious Relay can lie. Delivery is not a human read receipt. Sender keeps outbox ciphertext until recipient ACK, enabling resubmit if Relay loses its database. Backoff after error is 2, 4, 8, …, 256 seconds; accepted-envelope status is polled at 5-second intervals. An explicit CLI/API sync can force immediate retry.

Each sync starts cursor 0, walks at most 100 pages of 100 unacknowledged messages, and ACKs only committed items. Unknown/tampered messages remain unacknowledged and reported, while later valid items can still be processed. Relay inbox cursors are durable monotonic row sequences, not client-managed ordering semantics.
