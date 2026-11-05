import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createPasskey,
  creationOptions,
  fromBase64url,
  getPasskey,
  onIpAddress,
  passkeyError,
  passkeysSupported,
  requestOptions,
  toBase64url,
} from "./webauthn";

const bytes = (...b: number[]) => new Uint8Array(b).buffer;
const array = (buffer: BufferSource) => Array.from(new Uint8Array(buffer as ArrayBuffer));

afterEach(() => vi.unstubAllGlobals());

describe("base64url", () => {
  it("round-trips bytes without padding, using - and _", () => {
    const data = bytes(0xfb, 0xff, 0xfe, 0x00, 0x01);
    const text = toBase64url(data);
    expect(text).toBe("-__-AAE");
    expect(array(fromBase64url(text))).toEqual([0xfb, 0xff, 0xfe, 0x00, 0x01]);
  });

  it("reads padded and plain base64 too", () => {
    expect(array(fromBase64url("AQI="))).toEqual([1, 2]);
    expect(array(fromBase64url("+/8"))).toEqual([0xfb, 0xff]);
  });

  it("encodes a view of part of a buffer", () => {
    const whole = new Uint8Array([9, 1, 2, 9]);
    expect(toBase64url(whole.subarray(1, 3))).toBe("AQI");
  });
});

describe("options", () => {
  it("turns the server's creation options into create() arguments", () => {
    const options = creationOptions({
      publicKey: {
        challenge: "AQID",
        rp: { id: "gantry.local", name: "Gantry" },
        user: { id: "BAU", name: "adrian", displayName: "adrian" },
        pubKeyCredParams: [{ type: "public-key", alg: -7 }],
        timeout: 120000,
        attestation: "none",
        excludeCredentials: [{ type: "public-key", id: "Bwg" }],
        authenticatorSelection: { residentKey: "required", userVerification: "preferred" },
      },
    });
    const pk = options.publicKey!;
    expect(array(pk.challenge)).toEqual([1, 2, 3]);
    expect(array(pk.user.id)).toEqual([4, 5]);
    expect(pk.rp).toEqual({ id: "gantry.local", name: "Gantry" });
    expect(array(pk.excludeCredentials![0].id)).toEqual([7, 8]);
    expect(pk.authenticatorSelection?.residentKey).toBe("required");
    expect(pk.attestation).toBe("none");
  });

  it("turns the server's request options into get() arguments", () => {
    const options = requestOptions({
      publicKey: { challenge: "AQID", rpId: "gantry.local", timeout: 1000, userVerification: "preferred", allowCredentials: [] },
    });
    expect(array(options.publicKey!.challenge)).toEqual([1, 2, 3]);
    expect(options.publicKey!.rpId).toBe("gantry.local");
    expect(options.publicKey!.allowCredentials).toEqual([]);
  });
});

function credential(response: Record<string, ArrayBuffer | null>) {
  return { id: "Bwg", rawId: bytes(7, 8), type: "public-key", response };
}

describe("the browser", () => {
  it("makes a passkey and answers with base64url fields", async () => {
    const create = vi.fn(() =>
      Promise.resolve(credential({ clientDataJSON: bytes(1), attestationObject: bytes(2, 3) })),
    );
    vi.stubGlobal("navigator", { credentials: { create } });
    const json = await createPasskey({
      publicKey: {
        challenge: "AQID",
        rp: { name: "Gantry" },
        user: { id: "BAU", name: "a", displayName: "a" },
        pubKeyCredParams: [],
      },
    });
    expect(create).toHaveBeenCalledOnce();
    expect(json).toEqual({
      id: "Bwg",
      rawId: "Bwg",
      type: "public-key",
      response: { clientDataJSON: "AQ", attestationObject: "AgM" },
    });
  });

  it("gets a passkey for a login, with its user handle", async () => {
    const get = vi.fn(() =>
      Promise.resolve(
        credential({ clientDataJSON: bytes(1), authenticatorData: bytes(2), signature: bytes(3), userHandle: bytes(4) }),
      ),
    );
    vi.stubGlobal("navigator", { credentials: { get } });
    const json = await getPasskey({ publicKey: { challenge: "AQID" } });
    expect(json.response).toEqual({ clientDataJSON: "AQ", authenticatorData: "Ag", signature: "Aw", userHandle: "BA" });
  });

  it("sends a missing user handle as null", async () => {
    const get = vi.fn(() =>
      Promise.resolve(
        credential({ clientDataJSON: bytes(1), authenticatorData: bytes(2), signature: bytes(3), userHandle: null }),
      ),
    );
    vi.stubGlobal("navigator", { credentials: { get } });
    expect((await getPasskey({ publicKey: { challenge: "AQID" } })).response.userHandle).toBeNull();
  });

  it("says when nothing came back", async () => {
    vi.stubGlobal("navigator", { credentials: { get: () => Promise.resolve(null) } });
    await expect(getPasskey({ publicKey: { challenge: "AQID" } })).rejects.toThrow("No passkey was used.");
  });

  it("knows whether passkeys are there", () => {
    vi.stubGlobal("PublicKeyCredential", undefined);
    expect(passkeysSupported()).toBe(false);
    vi.stubGlobal("PublicKeyCredential", function PublicKeyCredential() {});
    vi.stubGlobal("navigator", { credentials: {} });
    expect(passkeysSupported()).toBe(true);
  });

  it("words the browser's refusals", () => {
    expect(passkeyError(new DOMException("x", "NotAllowedError"))).toMatch(/cancelled or timed out/);
    expect(passkeyError(new DOMException("x", "InvalidStateError"))).toMatch(/already registered/);
    expect(passkeyError(new DOMException("x", "SecurityError"))).toMatch(/host name/);
    expect(passkeyError(new Error("odd"))).toBe("odd");
  });

  it("spots an IP address, where passkeys don't work", () => {
    expect(onIpAddress("192.168.1.5")).toBe(true);
    expect(onIpAddress("[::1]")).toBe(true);
    expect(onIpAddress("localhost")).toBe(false);
    expect(onIpAddress("gantry.local")).toBe(false);
  });
});
