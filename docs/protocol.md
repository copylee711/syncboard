# SyncBoard protocol v1

Two peers: the **PC** (listens) and the **phone** (connects). Everything goes over one TCP
connection to port `53317`. UDP `53317` is used only for LAN discovery.

All integers are big-endian. `b64` is standard base64 with padding; `b64url` is URL-safe
base64 without padding.

## 1. Plain frames (before encryption)

```
u32 len | len bytes of UTF-8 JSON
```

`len` ≤ 64 KiB. The first frame on a connection decides its purpose: `pair` or `hello`.

## 2. Pairing

Purpose: agree on a 32-byte pre-shared key (`psk`) and remember each other's device id.
After pairing the connection is closed; the phone reconnects with a session (§3).

```
C → S  {"t":"pair","v":1,"mode":"qr"|"code","id":..,"name":..,"platform":"android",
        "pub":b64(P-256 uncompressed, 65 B),"nonce":b64(32 B)}
S → C  {"t":"pair","v":1,"id":..,"name":..,"platform":"windows","pub":..,"nonce":..}
```

* `shared` = ECDH(P-256) x-coordinate, 32 bytes.
* `th` = SHA-256(pubC ‖ pubS ‖ nonceC ‖ nonceS).

**QR mode.** The PC shows a QR code with
`syncboard://pair?v=1&h=<ip>,<ip>&p=53317&id=<pcId>&n=<name>&t=<b64url token, 32 B>`.
The token is single-use and expires after 5 minutes.

```
C → S  {"t":"pair_proof","mac":b64(HMAC-SHA256(token, "client" ‖ th))}
S → C  {"t":"pair_ok","mac":b64(HMAC-SHA256(token, "server" ‖ th))}   or {"t":"pair_rejected"}
```

The phone verifies the server MAC; that proves it is talking to the PC that showed the QR code.
`psk` = HKDF-SHA256(ikm = shared ‖ token, salt = th, info = "syncboard/v1/psk", 32).

**Code mode** (manual IP / discovery). Both sides show
`code` = (u32 of SHA-256("syncboard/v1/code" ‖ th)[0..4]) mod 1 000 000, as 6 digits.
The user accepts on the PC (timeout 120 s):

```
S → C  {"t":"pair_ok"}   or {"t":"pair_rejected"}
```

`psk` = HKDF-SHA256(ikm = shared, salt = th, info = "syncboard/v1/psk", 32).

## 3. Session

```
C → S  {"t":"hello","v":1,"id":<phoneId>,"nonce":b64(32 B)}
S → C  {"t":"hello","v":1,"id":<pcId>,"nonce":b64(32 B)}  or {"t":"error","code":"unknown_device"}
```

`k` = HKDF-SHA256(ikm = psk, salt = nonceC ‖ nonceS, info = "syncboard/v1/session", 64).
`k[0..32]` encrypts C→S and `k[32..64]` encrypts S→C.

From here on every record is:

```
u32 len | AES-256-GCM ciphertext+tag (len bytes, ≤ 1 MiB + 16)
```

The nonce is 4 zero bytes followed by a u64 per-direction counter that starts at 0. There is
no AAD. A failed decryption closes the connection.

Each plaintext is a message:

```
u32 hlen | hlen bytes JSON header | body bytes (rest)
```

The first message in each direction is `{"t":"auth","name":..,"platform":..}`. Successfully
decrypting it proves the peer knows `psk`.

## 4. Messages

| `t` | header fields | body | notes |
|---|---|---|---|
| `ping` / `pong` | — | — | ping every 30 s; drop the connection after 90 s of silence |
| `clip.text` | `id`, `hash` | UTF-8 text | ≤ 1 MiB |
| `clip.image` | `id`, `hash`, `mime`, `size` | — | followed by `chunk`s, then `end` |
| `file.offer` | `id`, `name`, `size` | — | |
| `file.accept` / `file.reject` / `file.cancel` | `id` | — | |
| `chunk` | `id`, `seq` | ≤ 256 KiB | belongs to a `clip.image` or accepted `file.offer` |
| `end` | `id`, `sha256` (hex) | — | receiver verifies |

* `id` is a random 16-hex-char string.
* `hash` is the hex SHA-256 of the content: UTF-8 text or image bytes.
* Images are always sent as PNG, up to 20 MiB.
* File senders stream `chunk`s only after `file.accept`. Received files are stored in the
  platform's `Download/SyncBoard` folder.

### Echo suppression

When a peer writes remote content into its own clipboard, it records that content's hash, plus
the clipboard sequence number on Windows. The next local clipboard change is not sent if it
matches either one. Received `id`s are de-duplicated with a small LRU.

## 5. Discovery (UDP 53317)

```
phone → broadcast  {"t":"discover","v":1,"id":<phoneId>}
PC → phone (unicast) {"t":"here","v":1,"id":<pcId>,"name":..,"port":53317}
```

The phone uses replies both to list PCs for code pairing and to refresh the address of an
already paired PC.
