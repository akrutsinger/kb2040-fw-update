/**
 * The update server. It hands out nonces, checks boards' signed update requests, and offers them
 * encrypted releases.
 *
 * - `POST /v1/challenge` returns `{ nonce }`, 32 bytes in hex.
 * - `POST /v1/update` takes `{ device, version, nonce, signature }`, which `tools/bridge.py` builds
 *   from the board's `request <nonce>`. It returns `{ authorized, device, running, latest, update }`,
 *   where `update` is an {@link Offer} if there's a newer release, otherwise null.
 * - `GET /v1/firmware/<version>` returns that release's encrypted image. It's public: without a
 *   ticket, it's useless.
 *
 * Storage, written by the tools:
 * - KV `device:<id>`: a registered board ({@link Device}), by `tools/enroll.py`.
 * - KV `release:<version>` ({@link Release}) and `release:latest` (a version), by
 *   `tools/release.py`.
 * - R2 `<version>.bin`: the encrypted image, by `tools/release.py`.
 *
 * A request is only checked against the registered identity key, and a ticket is only sealed to
 * the registered decryption key, never keys that come with the request. The signed request format
 * is described in `firmware/app/src/identity.rs`.
 */

import { Aes128Gcm, CipherSuite, DhkemP256HkdfSha256, HkdfSha256 } from "@hpke/core";

interface Env {
  KV: KVNamespace;
  RELEASES: R2Bucket;
  /** 32 random bytes in hex. From `.dev.vars` locally, a Worker secret once deployed. */
  NONCE_KEY: string;
  /** Wraps each release's content key (AES-256-GCM). 32 random bytes in hex, like `NONCE_KEY`. */
  RELEASE_KEK: string;
}

/** A registered board, as `tools/enroll.py` stores it. Keys are SEC1 uncompressed points in hex. */
interface Device {
  identity: string;
  decrypt: string;
  enrolled: string;
}

/** A release, as `tools/release.py` stores it. */
interface Release {
  /** Bytes in the encrypted image: the signed image and its 64-byte signature. */
  size: number;
  /** AES-128-CTR initial counter block, 16 bytes in hex. */
  iv: string;
  /** The content key, wrapped: `nonce (12) || AES-256-GCM(RELEASE_KEK, key)`, in hex. */
  key: string;
}

/** What a board is offered: where to get the image, and the ticket to decrypt it. */
interface Offer {
  version: string;
  size: number;
  url: string;
  /** HPKE encapsulated key, a 65-byte SEC1 point in hex. */
  enc: string;
  /** The sealed ticket, in hex. See {@link offer}. */
  ticket: string;
}

/** Must match `REQUEST_DOMAIN` in `firmware/app/src/identity.rs`. */
const REQUEST_DOMAIN = new TextEncoder().encode("kb2040-fw-update/update-request/v1");
const NONCE_LEN = 32;
/** How long a nonce is accepted after it's handed out. */
const NONCE_TTL_S = 300;
/** Device ids are the first 16 bytes of SHA-256(identity key). */
const DEVICE_ID_LEN = 16;
/** HPKE `info` prefix for tickets; the board's id follows it. */
const TICKET_INFO = new TextEncoder().encode("kb2040-fw-update/ticket/v1");
/** AAD prefix for a wrapped content key; the version follows it. Must match `tools/release.py`. */
const RELEASE_KEY_AAD = new TextEncoder().encode("kb2040-fw-update/release-key/v1");
/** Release versions: `major.minor.patch`, as in `firmware/app/Cargo.toml`. */
const VERSION = /^(\d+)\.(\d+)\.(\d+)$/;

export default {
  async fetch(request, env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (request.method === "POST" && pathname === "/v1/challenge") {
      return Response.json({ nonce: toHex(await makeNonce(env)) });
    }
    if (request.method === "POST" && pathname === "/v1/update") {
      return update(request, env);
    }
    const firmware = pathname.match(/^\/v1\/firmware\/([^/]+)$/);
    if (request.method === "GET" && firmware && VERSION.test(firmware[1])) {
      return download(env, firmware[1]);
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

  const latest = await env.KV.get("release:latest");
  const update =
    latest !== null && newer(latest, version)
      ? await offer(env, latest, deviceId, record, nonceBytes)
      : null;
  return Response.json({ authorized: true, device: id, running: version, latest, update });
}

/**
 * Offers release `version` to a board: where to download its encrypted image, and a ticket only
 * that board can open. The ticket is
 * `content key (16) || iv (16) || size (4, little-endian) || nonce (32) || version`,
 * sealed with HPKE (RFC 9180, base mode, DHKEM(P-256, HKDF-SHA256), HKDF-SHA256, AES-128-GCM) to
 * the board's decryption key, with `TICKET_INFO || device id` as `info`. The nonce is the
 * request's, so the board can tell that the ticket answers its own request.
 */
async function offer(
  env: Env,
  version: string,
  deviceId: Uint8Array,
  device: Device,
  nonce: Uint8Array,
): Promise<Offer> {
  const release = await env.KV.get<Release>(`release:${version}`, "json");
  if (!release) {
    throw new Error(`release:latest is ${version}, but release:${version} is missing`);
  }
  const size = new Uint8Array(4);
  new DataView(size.buffer).setUint32(0, release.size, true);
  const ticket = concat(
    await contentKey(env, version, release.key),
    fromHex(release.iv, 16)!,
    size,
    nonce,
    new TextEncoder().encode(version),
  );
  const suite = new CipherSuite({
    kem: new DhkemP256HkdfSha256(),
    kdf: new HkdfSha256(),
    aead: new Aes128Gcm(),
  });
  const recipientPublicKey = await suite.kem.deserializePublicKey(fromHex(device.decrypt, 65)!);
  const { ct, enc } = await suite.seal(
    { recipientPublicKey, info: concat(TICKET_INFO, deviceId) },
    ticket,
  );
  return {
    version,
    size: release.size,
    url: `/v1/firmware/${version}`,
    enc: toHex(new Uint8Array(enc)),
    ticket: toHex(new Uint8Array(ct)),
  };
}

/** Unwraps a release's content key with `RELEASE_KEK`. The AAD ties it to its version. */
async function contentKey(env: Env, version: string, wrapped: string): Promise<Uint8Array> {
  const kek = fromHex(env.RELEASE_KEK ?? "", 32);
  if (!kek) {
    throw new Error("RELEASE_KEK must be 32 bytes in hex (see README)");
  }
  const bytes = fromHex(wrapped, 12 + 16 + 16)!;
  const key = await crypto.subtle.importKey("raw", kek, "AES-GCM", false, ["decrypt"]);
  const plaintext = await crypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv: bytes.subarray(0, 12),
      additionalData: concat(RELEASE_KEY_AAD, new TextEncoder().encode(version)),
    },
    key,
    bytes.subarray(12),
  );
  return new Uint8Array(plaintext);
}

async function download(env: Env, version: string): Promise<Response> {
  const image = await env.RELEASES.get(`${version}.bin`);
  if (!image) {
    return error(404, `no release ${version}`);
  }
  return new Response(image.body, {
    headers: {
      "Content-Type": "application/octet-stream",
      "Content-Length": String(image.size),
    },
  });
}

/**
 * True if `a` is a newer version than `b`. Anything that isn't `major.minor.patch` is never newer,
 * so boards are only ever offered upgrades.
 */
function newer(a: string, b: string): boolean {
  const [ma, mb] = [a.match(VERSION), b.match(VERSION)];
  if (!ma || !mb) {
    return false;
  }
  for (let i = 1; i <= 3; i++) {
    if (Number(ma[i]) !== Number(mb[i])) {
      return Number(ma[i]) > Number(mb[i]);
    }
  }
  return false;
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
