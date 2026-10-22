# How releases are protected

Once a release is published ([step 8](encrypted-updates.md)), `bridge.py` gets an offer for any
board running an older version, downloads the encrypted image, and installs it with
`update encrypted` (step 9).

The board opens the ticket with its TPM (TPM2_ECDH_ZGen, then the rest of HPKE in the `hpke`
crate), checks that its nonce is the one the board just signed in `request`, and decrypts the
image block by block as it arrives. From there it's the same as [Updates over USB](usb-update.md):
the signature check, then the trial boot. The protocol is described at the top of
`firmware/app/src/update.rs`.

It's envelope encryption, as in MCUboot's encrypted images.

## Once per release

`release.py` encrypts the whole `.signed` file (image and signature) with a fresh random AES-128
key, in CTR mode. The encrypted image goes to R2, and is public: it's useless without the key. The
key itself is stored in KV wrapped with `RELEASE_KEK` (AES-256-GCM, with the version as AAD).
`release.py` reads `RELEASE_KEK` from `worker/.dev.vars`.

```python
{{#include ../../tools/release.py:encrypt}}
```

## Once per request

The server unwraps the key and seals a ticket to the board's registered decryption key, with HPKE
(RFC 9180: DHKEM(P-256, HKDF-SHA256), HKDF-SHA256, AES-128-GCM). The ticket is
`content key || IV || size || nonce || version`, and HPKE's `info` holds the board's id:

```typescript
{{#include ../../worker/src/index.ts:seal-ticket}}
```

Only that board's TPM can open it, with TPM2_ECDH_ZGen, and the nonce shows it answers the board's
own request. On the board, in `firmware/app/src/ticket.rs`:

```rust,ignore
{{#include ../../firmware/app/src/ticket.rs:open}}
```

## What it does and doesn't protect

- **Integrity** still comes from the Ed25519 signature over the plaintext image, which the board
  checks before installing. The update server never has the signing key (`keys/update`), so even a
  compromised server can't make a board run its code.
- **Only upgrades** are offered: versions compare as numbers (`0.0.10` is older than `0.1.0`), and a
  board on the latest version or newer gets `up to date`. The board itself doesn't refuse older
  images yet (anti-rollback is a stretch goal).
- **One key per release** means that anyone who gets that key out of one board (the RP2040 has no
  flash read protection or secure boot) can decrypt that release. The TPM protects the key in
  transit and at rest, not the firmware once it's installed.
