# Decryption key

`pubkey decrypt` prints the board's second key, in the same format. It's made the same way, on the
first `pubkey decrypt`, and persists at handle `0x81000003`.

- It does key agreement (ECDH) only, and can't sign. The identity key in turn can't do ECDH. Each
  key does one job, so a mistake in how one is used can't weaken the other.
- Encrypted updates use it. The update server encrypts each update's AES key to this key (HPKE).
  The board gets that key back with TPM2_ECDH_ZGen, which multiplies the server's one-time public
  key by the private key inside the TPM (see [How releases are protected](releases.md)).
- There's deliberately no console command that runs ECDH_ZGen on its own: anyone at the USB port
  could then use the board to decrypt update keys. It only runs inside an encrypted update.
