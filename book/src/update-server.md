# The update server

`worker/` is the update server, a [Cloudflare Worker](https://developers.cloudflare.com/workers/) in
TypeScript. Cloudflare runs Workers on its own network, close to whoever calls them, so there's no
server to run. While developing, `wrangler dev` runs the same runtime locally, with a local copy of
its storage (KV for records, R2 for images) in `worker/.wrangler/`. The board has no network, so
`tools/bridge.py` carries its messages over USB serial.

It checks that an update request comes from a registered board, and offers that board a newer
release, encrypted so that only the board can read it, which `bridge.py` then installs. The
[next chapter](encrypted-updates.md) runs through it step by step.

## How a request works

1. `bridge.py` gets a nonce from the server (`POST /v1/challenge`).
2. The board signs an update request for it (`request <nonce>`, see
   [Identity key](identity-key.md#update-requests)).
3. `bridge.py` passes the request on (`POST /v1/update`). The server checks the nonce, looks up the
   board, and verifies the signature with the identity key it registered for that board.

The signature check in step 3:

```typescript
{{#include ../../worker/src/index.ts:verify-request}}
```

## Enrolling

`enroll.py` reads both public keys from the board and stores them in KV as `device:<id>`. The id
is the first 16 bytes of SHA-256(identity key), in hex. The server only ever checks a request
against that registered key, never a key that comes with the request. Enrolling trusts whatever
answers on the port, so do it with the board in front of you. Proving that a key really lives in
a TPM (EK certificates) is the ideal next way to advance the enrollment process.

## Nonces

Nonces carry their own expiry and an HMAC made with `NONCE_KEY` (`expiry || random || HMAC[..16]`),
so the server checks them without storing anything. A nonce is valid for 5 minutes, and can be
used more than once until then. A replayed request only gets the same answer for the same board,
so that's fine for now. Single use would need state (a Durable Object).

```typescript
{{#include ../../worker/src/index.ts:nonce}}
```

## Notes

- `npm run check` type-checks the Worker. It first generates the runtime types with
  `wrangler types`.
- npm 11 blocks the install scripts of esbuild and workerd (both come with wrangler) by default,
  and warns about them during `npm install`. `wrangler dev` works without them.
