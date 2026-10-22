# Identity key

`pubkey` prints the board's identity key, a P-256 ECDSA signing key that the TPM generated and
never lets out. It's printed as a SEC1 uncompressed point in hex (`04 || x || y`):

```
> pubkey
created identity key
04a1b2...
```

- The first `pubkey` creates the key (TPM2_CreatePrimary in the owner hierarchy) and makes it
  persistent at handle `0x81000002`. After that, `pubkey` just reads it back, across resets and
  power loss. Clearing the TPM deletes it; a new one would then be different.
- It's a signing key only, unrestricted so the TPM signs any digest the firmware gives it. Key
  agreement for encrypted updates uses its own key (see [Decryption key](decryption-key.md)).
- Creating it needs the owner hierarchy's password to be empty, which it is on a fresh TPM. If the
  TPM was used elsewhere and has an owner password, `pubkey` fails with an authorization error
  such as `ERR TPM error [TPM_RC_BAD_AUTH, session[1]]`.

## Challenges

`sign <nonce>` proves the board holds the key. The nonce is 32 random bytes in hex (64 digits). The
board signs `SHA-256("kb2040-fw-update/challenge/v1" || nonce)` and prints the ECDSA signature,
`r || s` in hex. The fixed prefix keeps `sign` from being used to sign anything else with this
key. `tools/challenge.py` does the whole exchange with a fresh nonce and checks the signature
(close tio first):

```sh
uv run tools/challenge.py /dev/ttyACM0
```

It verifies against the key the board itself reports, so it proves the board holds that key, not
that it's the board you expect. The update server does pin each board's key, when the board is
enrolled (see [The update server](update-server.md)).

## Update requests

`request <nonce>` makes an update request for the update server, which supplies the nonce. The
board signs `SHA-256("kb2040-fw-update/update-request/v1" || nonce || version)` with the same key,
where `version` is the running firmware's (`version` in `firmware/app/Cargo.toml`). It prints
`<version> <signature>`:

```
> request 3f9a...
0.1.0 9c1e...
```

In `firmware/app/src/identity.rs`:

```rust,ignore
{{#include ../../firmware/app/src/identity.rs:sign-request}}
```

Checked against the board's registered key, a request proves which board asked, that it asked just
now (the nonce is the server's), and which version it runs. The different prefix keeps a `sign`
signature from passing as a request, and the other way around. `challenge.py` checks a request
too.
