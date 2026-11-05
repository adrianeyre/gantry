// Passkeys in the browser: Gantry's server sends WebAuthn options as JSON
// with base64url fields, and takes the browser's answer back the same way
// (the shape `PublicKeyCredential.toJSON()` gives). These helpers turn one
// into the other, so they work in browsers that lack `parseCreationOptionsFromJSON`.

/** Bytes as base64url, without padding. */
export function toBase64url(bytes: ArrayBuffer | ArrayBufferView): string {
  const view = ArrayBuffer.isView(bytes)
    ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    : new Uint8Array(bytes);
  let binary = "";
  for (const byte of view) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** base64url (or plain base64, padded or not) as bytes. */
export function fromBase64url(text: string): ArrayBuffer {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const padded = base64 + "=".repeat((4 - (base64.length % 4)) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

interface CredentialDescriptorJSON {
  type: string;
  id: string;
  transports?: string[];
}

/** `POST /api/account/passkeys/register/start`'s answer. */
export interface CreationOptionsJSON {
  publicKey: {
    challenge: string;
    rp: { id?: string; name: string };
    user: { id: string; name: string; displayName: string };
    pubKeyCredParams: { type: string; alg: number }[];
    timeout?: number;
    attestation?: string;
    excludeCredentials?: CredentialDescriptorJSON[];
    authenticatorSelection?: AuthenticatorSelectionCriteria;
  };
}

/** `POST /api/login/passkey/start`'s answer. */
export interface RequestOptionsJSON {
  publicKey: {
    challenge: string;
    rpId?: string;
    timeout?: number;
    userVerification?: string;
    allowCredentials?: CredentialDescriptorJSON[];
  };
}

function descriptor(d: CredentialDescriptorJSON): PublicKeyCredentialDescriptor {
  return {
    type: d.type as PublicKeyCredentialType,
    id: fromBase64url(d.id),
    ...(d.transports ? { transports: d.transports as AuthenticatorTransport[] } : {}),
  };
}

/** What `navigator.credentials.create()` takes. */
export function creationOptions(json: CreationOptionsJSON): CredentialCreationOptions {
  const o = json.publicKey;
  return {
    publicKey: {
      challenge: fromBase64url(o.challenge),
      rp: o.rp,
      user: { ...o.user, id: fromBase64url(o.user.id) },
      pubKeyCredParams: o.pubKeyCredParams.map((p) => ({ type: p.type as PublicKeyCredentialType, alg: p.alg })),
      timeout: o.timeout,
      attestation: o.attestation as AttestationConveyancePreference | undefined,
      excludeCredentials: (o.excludeCredentials ?? []).map(descriptor),
      authenticatorSelection: o.authenticatorSelection,
    },
  };
}

/** What `navigator.credentials.get()` takes. */
export function requestOptions(json: RequestOptionsJSON): CredentialRequestOptions {
  const o = json.publicKey;
  return {
    publicKey: {
      challenge: fromBase64url(o.challenge),
      rpId: o.rpId,
      timeout: o.timeout,
      userVerification: o.userVerification as UserVerificationRequirement | undefined,
      allowCredentials: (o.allowCredentials ?? []).map(descriptor),
    },
  };
}

/** A new passkey, as `register/finish` takes it. */
export interface RegistrationJSON {
  id: string;
  rawId: string;
  type: string;
  response: { clientDataJSON: string; attestationObject: string };
}

/** A passkey's answer to a login challenge, as `login/passkey/finish` takes it. */
export interface AssertionJSON {
  id: string;
  rawId: string;
  type: string;
  response: { clientDataJSON: string; authenticatorData: string; signature: string; userHandle: string | null };
}

export function registrationJSON(credential: PublicKeyCredential): RegistrationJSON {
  const response = credential.response as AuthenticatorAttestationResponse;
  return {
    id: credential.id,
    rawId: toBase64url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJSON: toBase64url(response.clientDataJSON),
      attestationObject: toBase64url(response.attestationObject),
    },
  };
}

export function assertionJSON(credential: PublicKeyCredential): AssertionJSON {
  const response = credential.response as AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: toBase64url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJSON: toBase64url(response.clientDataJSON),
      authenticatorData: toBase64url(response.authenticatorData),
      signature: toBase64url(response.signature),
      userHandle: response.userHandle ? toBase64url(response.userHandle) : null,
    },
  };
}

/** Whether this browser can use passkeys at all. */
export function passkeysSupported(): boolean {
  return typeof globalThis.PublicKeyCredential !== "undefined" && typeof navigator !== "undefined" && !!navigator.credentials;
}

/**
 * Whether this page's address is an IP address, where browsers refuse
 * passkeys: they need a host name, such as gantry.local or localhost.
 */
export function onIpAddress(hostname: string = window.location.hostname): boolean {
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(hostname) || hostname.includes(":") || hostname.startsWith("[");
}

/** Why the browser gave no passkey, in words. */
export function passkeyError(e: unknown): string {
  if (e instanceof DOMException) {
    if (e.name === "NotAllowedError") return "No passkey was used: it was cancelled or timed out.";
    if (e.name === "InvalidStateError") return "This passkey is already registered.";
    if (e.name === "SecurityError")
      return "This browser won't use passkeys at this address. Open Gantry by its host name, not an IP address.";
  }
  return e instanceof Error ? e.message : String(e);
}

/** Asks the browser to make a passkey, from the server's options. */
export async function createPasskey(options: CreationOptionsJSON): Promise<RegistrationJSON> {
  const credential = (await navigator.credentials.create(creationOptions(options))) as PublicKeyCredential | null;
  if (!credential) throw new Error("No passkey was made.");
  return registrationJSON(credential);
}

/** Asks the browser for a passkey for this site, from the server's options. */
export async function getPasskey(options: RequestOptionsJSON): Promise<AssertionJSON> {
  const credential = (await navigator.credentials.get(requestOptions(options))) as PublicKeyCredential | null;
  if (!credential) throw new Error("No passkey was used.");
  return assertionJSON(credential);
}
