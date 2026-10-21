/**
 * The update server. It hands out nonces and checks boards' signed update requests.
 *
 * - `POST /v1/challenge` returns `{ nonce }`, 32 bytes in hex.
 * - `POST /v1/update` takes `{ device, version, nonce, signature }`, which `tools/bridge.py` builds
 *   from the board's `request <nonce>`. It returns `{ authorized, device, running, latest }`.
 *
 * Boards are registered in KV as `device:<id>` by `tools/enroll.py`. A request is only checked
 * against the registered identity key, never a key that comes with the request. The signed format
 * is described in `firmware/app/src/identity.rs`.
 */

interface Env {
  KV: KVNamespace;
  /** 32 random bytes in hex. From `.dev.vars` locally, a Worker secret once deployed. */
  NONCE_KEY: string;
}

/** A registered board, as `tools/enroll.py` stores it. Keys are SEC1 uncompressed points in hex. */
interface Device {
  identity: string;
  decrypt: string;
  enrolled: string;
}

/** Must match `REQUEST_DOMAIN` in `firmware/app/src/identity.rs`. */
const REQUEST_DOMAIN = new TextEncoder().encode("kb2040-fw-update/update-request/v1");
const NONCE_LEN = 32;
/** How long a nonce is accepted after it's handed out. */
const NONCE_TTL_S = 300;
/** Device ids are the first 16 bytes of SHA-256(identity key). */
const DEVICE_ID_LEN = 16;

export default {
  async fetch(request, env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (request.method === "POST" && pathname === "/v1/challenge") {
      return Response.json({ nonce: toHex(await makeNonce(env)) });
    }
    if (request.method === "POST" && pathname === "/v1/update") {
      return update(request, env);
    }
    return error(404, "not found");
  },
} satisfies ExportedHandler<Env>;

async function update(request: Request, env: Env): Promise<Response> {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return error(400, "body must be JSON");
  }
  if (typeof body !== "object" || body === null) {
    return error(400, "body must be a JSON object");
  }
  const { device, version, nonce, signature } = body as Record<string, unknown>;
  if (
    typeof device !== "string" ||
    typeof version !== "string" ||
    typeof nonce !== "string" ||
    typeof signature !== "string"
  ) {
    return error(400, "need device, version, nonce and signature, as strings");
  }
  const deviceId = fromHex(device, DEVICE_ID_LEN);
  const nonceBytes = fromHex(nonce, NONCE_LEN);
  const signatureBytes = fromHex(signature, 64);
  if (!deviceId || !nonceBytes || !signatureBytes) {
    return error(400, "device, nonce and signature must be 16, 32 and 64 bytes in hex");
  }

  const nonceError = await checkNonce(env, nonceBytes);
  if (nonceError) {
    return error(401, nonceError);
  }
  // Lower case, as enroll.py stores it.
  const id = toHex(deviceId);
  const record = await env.KV.get<Device>(`device:${id}`, "json");
  if (!record) {
    return error(403, "unknown device, enroll it with tools/enroll.py");
  }
  const key = await crypto.subtle.importKey(
    "raw",
    fromHex(record.identity, 65)!,
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["verify"],
  );
  // WebCrypto takes ECDSA signatures as r || s, the board's format.
  const signed = concat(REQUEST_DOMAIN, nonceBytes, new TextEncoder().encode(version));
  if (
    !(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, signatureBytes, signed))
  ) {
    return error(403, "bad signature");
  }

  // Set by the release tooling (a later step); null until then.
  const latest = await env.KV.get("release:latest");
  return Response.json({ authorized: true, device: id, running: version, latest });
}

/**
 * A nonce the server can check later without storing it:
 * `expiry (8 bytes, big-endian Unix seconds) || random (8) || HMAC-SHA256(NONCE_KEY, first 16)[..16]`.
 *
 * It can be used more than once until it expires. A replayed request only gets the same answer
 * for the same board, so that's acceptable here. Single use would need state, such as a Durable
 * Object.
 */
async function makeNonce(env: Env): Promise<Uint8Array> {
  const nonce = new Uint8Array(NONCE_LEN);
  new DataView(nonce.buffer).setBigUint64(0, BigInt(now() + NONCE_TTL_S));
  crypto.getRandomValues(nonce.subarray(8, 16));
  nonce.set((await mac(env, nonce.subarray(0, 16))).subarray(0, 16), 16);
  return nonce;
}

/** Why `nonce` isn't acceptable, or null if it is. */
async function checkNonce(env: Env, nonce: Uint8Array): Promise<string | null> {
  const expected = (await mac(env, nonce.subarray(0, 16))).subarray(0, 16);
  if (!crypto.subtle.timingSafeEqual(expected, nonce.subarray(16))) {
    return "nonce wasn't issued by this server";
  }
  const expiry = new DataView(nonce.buffer, nonce.byteOffset).getBigUint64(0);
  if (BigInt(now()) > expiry) {
    return "nonce expired, get a new one";
  }
  return null;
}

async function mac(env: Env, data: Uint8Array): Promise<Uint8Array> {
  const keyBytes = fromHex(env.NONCE_KEY ?? "", 32);
  if (!keyBytes) {
    throw new Error("NONCE_KEY must be 32 bytes in hex (see README)");
  }
  const key = await crypto.subtle.importKey(
    "raw",
    keyBytes,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, data));
}

function now(): number {
  return Math.floor(Date.now() / 1000);
}

function error(status: number, message: string): Response {
  return Response.json({ error: message }, { status });
}

/** Exactly `len` bytes of hex, either case, or null. */
function fromHex(text: string, len: number): Uint8Array | null {
  if (text.length !== 2 * len || !/^[0-9a-fA-F]*$/.test(text)) {
    return null;
  }
  const out = new Uint8Array(len);
  for (let i = 0; i < len; i++) {
    out[i] = parseInt(text.slice(2 * i, 2 * i + 2), 16);
  }
  return out;
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}
