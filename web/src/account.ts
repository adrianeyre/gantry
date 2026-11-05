// The API for a user's own account security (two-factor authentication,
// passkeys and API tokens), and the Admin's view of everyone's.

import { request } from "./api";
import type { Role, User } from "./api";
import type { AssertionJSON, CreationOptionsJSON, RegistrationJSON, RequestOptionsJSON } from "./webauthn";

export interface TwoFactor {
  totp_enabled: boolean;
  totp_enabled_at: string | null;
  recovery_codes_left: number;
}

export interface Passkey {
  id: number;
  name: string;
  created_at: string;
  last_used_at: string | null;
}

/** `GET /api/account`: the caller, with their two-factor authentication and passkeys. */
export interface AccountInfo {
  id: number;
  username: string;
  role: Role;
  two_factor: TwoFactor;
  /** An Admin requires two-factor authentication of every user. */
  require_totp: boolean;
  passkeys: Passkey[];
}

/** A TOTP secret being set up, to add to an authenticator app. */
export interface TotpSetup {
  secret: string;
  uri: string;
  /** A QR code of `uri`, drawn by the server as SVG. */
  qr_svg: string;
}

export interface RecoveryCodes {
  recovery_codes: string[];
}

/** An API token, never the token itself. */
export interface ApiToken {
  id: number;
  user_id: number;
  username: string;
  name: string;
  /** Its first characters, such as `gantry_Ab3d`. */
  prefix: string;
  created_at: string;
  expires_at: string | null;
  last_used_at: string | null;
}

/** A token just made: the only time `token` is shown. */
export interface NewApiToken extends ApiToken {
  token: string;
}

export const account = {
  get: () => request<AccountInfo>("GET", "/api/account"),
  totpSetup: (password: string) => request<TotpSetup>("POST", "/api/account/totp/setup", { password }),
  totpEnable: (code: string) => request<RecoveryCodes>("POST", "/api/account/totp/enable", { code }),
  totpDisable: (password: string, code: string) =>
    request<void>("POST", "/api/account/totp/disable", { password, code }),
  newRecoveryCodes: (code: string) => request<RecoveryCodes>("POST", "/api/account/totp/recovery-codes", { code }),
  passkeyStart: () => request<CreationOptionsJSON>("POST", "/api/account/passkeys/register/start"),
  passkeyFinish: (name: string, credential: RegistrationJSON) =>
    request<Passkey>("POST", "/api/account/passkeys/register/finish", { name, credential }),
  removePasskey: (id: number) => request<void>("DELETE", `/api/account/passkeys/${id}`),
  loginPasskeyStart: () => request<RequestOptionsJSON>("POST", "/api/login/passkey/start"),
  loginPasskeyFinish: (credential: AssertionJSON) =>
    request<User>("POST", "/api/login/passkey/finish", { credential }),
};

export const tokens = {
  /** The caller's tokens, or with `all` (Admins) everyone's. */
  list: (all = false) => request<ApiToken[]>("GET", all ? "/api/tokens?all=true" : "/api/tokens"),
  create: (name: string, expiresInDays?: number) =>
    request<NewApiToken>(
      "POST",
      "/api/tokens",
      expiresInDays ? { name, expires_in_days: expiresInDays } : { name },
    ),
  revoke: (id: number) => request<void>("DELETE", `/api/tokens/${id}`),
};

/** What an Admin sees and does about another user's sign-in. */
export const userSecurity = {
  passkeys: (userId: number) => request<Passkey[]>("GET", `/api/users/${userId}/passkeys`),
  removePasskey: (userId: number, id: number) => request<void>("DELETE", `/api/users/${userId}/passkeys/${id}`),
  resetTotp: (userId: number) => request<void>("DELETE", `/api/users/${userId}/totp`),
};

/** The one-line curl example for a new token. */
export function curlExample(token: string, origin: string = window.location.origin): string {
  return `curl -H "Authorization: Bearer ${token}" ${origin}/api/printers`;
}

/** Recovery codes as a text file to keep. */
export function recoveryCodesFile(username: string, codes: string[]): string {
  return [
    `Gantry recovery codes for ${username}`,
    "Each works once, in place of a code from your authenticator app.",
    "",
    ...codes,
    "",
  ].join("\n");
}

/** Whether `iso` is in the past. */
export function expired(iso: string | null, now: number = Date.now()): boolean {
  return iso !== null && new Date(iso).getTime() <= now;
}
