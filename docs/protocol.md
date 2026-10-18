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
decrypting it proves the peer knows `psk`. Each side sends its `auth` right after deriving the
keys, without waiting for the other's, so both sides always receive one.

**Stale pairings.** Nothing new goes on the wire for these; each side detects them on its own,
so older peers interoperate (they just keep retrying as before):

- The phone checks the `id` in the PC's `hello` against the PC id it paired with. A different
  id means another PC, or the same PC after its config was reset, now answers at that
  address. If no known address or LAN discovery (§5) reaches the right id, the phone treats it
  like `unknown_device` and asks the user to pair again.
- If the peer's `auth` record fails AES-GCM authentication (as opposed to a closed or timed-out
  connection), the two sides hold different `psk`s for each other: one of them was re-paired
  without the other. The PC reports a `keyMismatch` for that phone, the phone shows the PC as
  needing re-pairing, and it retries only every 60 s. Pairing again (§2) replaces the psk on
  both sides.

## 4. Messages

| `t` | header fields | body | notes |
|---|---|---|---|
| `ping` / `pong` | — | — | ping every 30 s; drop the connection after 90 s of silence |
| `clip.text` | `id`, `hash` | UTF-8 text | ≤ 1 MiB |
| `clip.image` | `id`, `hash`, `mime`, `size` | — | followed by `chunk`s, then `end` |
| `file.offer` | `id`, `name`, `size`, `dir`? | — | `dir`: a phone folder chosen in the PC's file manager (§8) |
| `file.accept` / `file.reject` / `file.cancel` | `id` | — | |
| `chunk` | `id`, `seq` | ≤ 256 KiB | belongs to a `clip.image` or accepted `file.offer` |
| `end` | `id`, `sha256` (hex) | — | receiver verifies |
| `service.stop` | — | — | PC → phone: the daemon exits (user turned the phone service off) |
| `unpaired` | — | — | either way: the sender removed the other side, which deletes it too and closes the connection |

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

## 6. Wake after reboot

Without root, the phone daemon dies when the phone reboots. If the app holds
WRITE_SECURE_SETTINGS (granted over adb during setup), it turns wireless debugging on at boot
and tells the PC its adb port with a single plain frame on TCP 53317:

```
{"t":"adb_wake","v":1,"id":<phoneId>,"port":<adb tls port>,"ts":<unix seconds>,
 "mac":b64(HMAC-SHA256(psk, "wake" ‖ port ‖ ts))}
```

`port` and `ts` are written as decimal strings inside the MAC. The PC accepts `ts` within
±300 s, runs `adb connect <phone ip>:<port>` with the key it already paired, and starts the
daemon. It answers with one plain frame before closing: `{"t":"wake_ok"}`, or
`{"t":"wake_rejected","code":"unknown_device"|"bad_request"}`. `unknown_device` means the PC
removed this phone; the app drops that PC from its cached wake list. Older PCs close without
answering, which the app treats as `wake_ok`.

## 7. The pairing QR

The PC's 添加手机 step shows one QR in Android's wireless-debugging format:

```
WIFI:T:ADB;S:sb-<ip1>-<ip2>...-<random>;P:<token>;;
```

`<ipN>` are the PC's IPv4 addresses as 8 lowercase hex digits (at most four); `<token>` is a
fresh QR pairing token (base64url, as `t` in §2, valid for 5 minutes). The same code serves
both ways in:

- **Phone service not running.** Android's wireless-debugging scanner (使用二维码配对设备) uses
  `S` as the pairing service name and `P` as the password. The PC finds the phone's
  `_adb-tls-pairing._tcp` service named `S`, runs `adb pair` and `adb connect`, and starts the
  daemon with a fresh `syncboard://pair` URI (§2).
- **Phone service running.** The SyncBoard app's scanner reads the hosts from `S` and the token
  from `P`, and pairs as with `syncboard://pair?h=<hosts>&p=53317&t=<token>`.

On networks that drop multicast between devices, the PC never sees the phone's mDNS. While the
user scans, the app watches its own phone's `_adb-tls-pairing._tcp` services; on one named
`sb-...` it decodes the hosts and sends one plain frame to TCP 53317:

```
{"t":"adb_pair","v":1,"name":<S>,"pair_port":<port>,"connect_port":<adb tls port>|null}
```

The PC answers `{"t":"ok"}` and, if `name` is the QR it is showing, pairs with
`<sender ip>:<pair_port>` using its own password. The frame carries no secret: adb pairing still
needs the password from the QR.

## 8. File manager

The PC browses and changes the phone's shared storage over the session (§3). Requests:

```
PC → phone  {"t":"fs","rid":<id>,"op":<op>, ...args}
phone → PC  {"t":"fs.res","rid":<id>,"ok":true, ...}   body: op-specific
            {"t":"fs.res","rid":<id>,"ok":false,"error":<message for the user>}
```

The phone only accepts paths under `/storage` (and `/sdcard`), after resolving `..` and symlinks.
Phones without this section never answer; the PC gives up after a few seconds.

| `op` | args | reply |
|---|---|---|
| `roots` | — | `home`, `volumes`: `[{name, path, total, free}]` |
| `list` | `path`, `offset` | body: JSON `[{n, d, s, m}]` (name, is folder, size or item count, mtime ms); `next` when more pages remain |
| `walk` | `path`, `offset` | body: JSON `[{p, d, s}]`, every file and folder below `path` (relative paths), paged like `list` |
| `mkdir` | `path` | |
| `rename` | `path`, `to` | |
| `delete` | `paths` | recursive, permanent |
| `copy` / `move` | `paths`, `dir` | names that exist get ` (n)` appended |
| `thumb` | `path`, `px` | body: JPEG preview of an image or video |
| `pull` | `path` | see below |

**Pull** (phone → PC): the reply is `{"t":"fs.res","rid","ok":true,"size","name"}` followed by `chunk`s
and an `end` with the same id as `rid` (as in §4). Either side stops it with `file.cancel`; the
phone confirms a PC's cancel with its own `file.cancel`.

**Push** (PC → phone): a `file.offer` with `dir` set. The phone accepts it whatever its
自动接收文件 setting is (the user chose the folder on the PC) and stores it there under a free name.

## 9. Phone controls

The same request format as §8, with these ops (no body):

| `op` | args | reply |
|---|---|---|
| `status` | — | `model`, `android`, `battery` (0–100), `charging`, `full`, `temp` (°C), `storageTotal`, `storageFree` |
| `ring` | `stop`? | starts (or stops) a loud tone on the alarm stream for up to 60 s, alarm volume restored after |
| `media` | `action`: `playPause`, `next`, `previous`, `volumeUp`, `volumeDown`, `mute`, `lock` | volume actions reply `volume`, `max` (media stream) |
| `open` | `url` (http/https) | opens it on the phone |

Phone → PC, no reply:

| `t` | fields | |
|---|---|---|
| `open.url` | `url` | open an http(s) link in the PC's browser (share sheet 在电脑上打开) |

## 10. Text messages

The phone daemon (shell holds `READ_SMS`) checks the newest SMS inbox id every 3 s (Android 16
refuses content observers from the daemon) and, per the phone's settings, sends one of:

| `t` | fields | when |
|---|---|---|
| `sms.code` | `code`, `source`?, `ts` | 短信验证码同步 (default on): a new message that carries a verification code. Only the code and the sender's 【签名】 leave the phone |
| `sms.new` | `from`, `body` (≤ 4000 chars), `code`?, `source`?, `ts` | 新短信在电脑上提醒 (default off): every new message |

Messages older than 10 minutes when seen (restored backups) are skipped. The PC copies `code`
to its clipboard and shows a Windows notification, each per its own settings.

5G messages (RCS) are not in the SMS database; with 手机通知同步 on, the messaging app's
notification stands in for them (a code becomes `sms.code`; other text becomes `sms.new` when
新短信在电脑上提醒 is on and the inbox didn't have it within 5 s).

## 11. Phone notifications

With 手机通知同步 on, the daemon grants the app's `NotificationListenerService` notification
access (`cmd notification allow_listener`); the listener hands each new notification to the daemon
over the local IPC socket, and the daemon sends:

| `t` | fields | |
|---|---|---|
| `notif` | `pkg`, `app` (label), `title`, `text` (≤ 2000 chars), `ts` | shown as a Windows notification |

Ongoing, foreground-service and group-summary notifications, and updates that change nothing,
are skipped.
