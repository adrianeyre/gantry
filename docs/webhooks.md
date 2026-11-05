# Webhooks

A webhook sends Gantry's events, as they happen, to a URL of yours: Home Assistant, n8n, Node-RED, a script (PRD §7.7). The events are the ones Alerts use: a Job ended (with its Outcome), a Job paused, a Fault raised, a Connection lost, or a Filament Slot running low. As with an Alert Rule, a webhook can be limited to some Printers or Tags, and `job_ended` to some Outcomes.

Admins manage webhooks on the **Webhooks** page (also linked from Alerts), or through the [REST API](rest-api.md) at `/api/webhooks`.

## Setting one up

Give it a name, the URL (`http://` or `https://`), and the events it wants. Gantry makes a **signing secret**, `whsec_` and 43 more characters, and shows it **once**. Store it with your receiver: it is how the receiver knows a delivery came from your Gantry. Gantry keeps the secret encrypted with its secret key, and never shows it again. To replace it, choose **New secret** (`{"new_secret": true}`). The old one stops working at once, and the new one is shown once.

**Send test** delivers a signed `test` event, once, and says what your receiver answered.

## A delivery

Each event is one `POST` with a JSON body:

```json
{
  "id": "1f0c2c4e-7d4b-4b5e-9f56-0d7a1f4e2b11",
  "event": "job_ended",
  "created_at": "2026-11-04T18:26:30.663830Z",
  "printer": { "id": "8f6c…", "name": "X1" },
  "outcome": "succeeded",
  "title": "X1: Job succeeded",
  "message": "benchy.gcode.3mf (Plate 1) succeeded on X1. It is Awaiting Clear.",
  "level": "good"
}
```

| Field | |
|---|---|
| `id` | This event's id. It is the same on every retry, so a receiver can drop duplicates. |
| `event` | `job_ended`, `job_paused`, `fault_raised`, `connection_lost`, `low_filament`, or `test` |
| `created_at` | When Gantry saw it, in RFC 3339 |
| `printer` | Which Printer (`null` for `test`) |
| `outcome` | For `job_ended`: `succeeded`, `failed`, `cancelled` or `lost`; otherwise `null` |
| `title`, `message` | The same words an Alert would use |
| `level` | `good`, `info`, `warning` or `bad` |

And these headers:

| Header | |
|---|---|
| `Content-Type` | `application/json` |
| `Gantry-Event` | The same as `event` |
| `Gantry-Delivery` | The same as `id` |
| `Gantry-Signature` | `t=<unix seconds>,v1=<hex>`, described below |
| `User-Agent` | `Gantry-Webhook/<version>` |

## Verifying the signature

`v1` is the lower-case hex of an **HMAC-SHA256**, keyed with the webhook's secret (the whole `whsec_…` string, as text), of:

```
<t>.<body>
```

That is the timestamp from the header, a full stop, and the **exact bytes** of the request body. To check a delivery:

1. Read `t` and `v1` from `Gantry-Signature`.
2. Compute the HMAC of `t + "." + raw body` with your secret.
3. Compare it with `v1` in constant time. If they differ, refuse the delivery.
4. Refuse a `t` more than about five minutes from your clock, so a recorded delivery can't be replayed later.

Hash the raw body as received, before any JSON parsing: re-serialised JSON won't match.

Python:

```python
import hashlib, hmac, time

def verify(secret: str, header: str, body: bytes, tolerance: int = 300) -> bool:
    parts = dict(p.split("=", 1) for p in header.split(","))
    t, given = int(parts["t"]), parts["v1"]
    if abs(time.time() - t) > tolerance:
        return False
    expected = hmac.new(secret.encode(), f"{t}.".encode() + body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, given)
```

Node.js:

```js
import { createHmac, timingSafeEqual } from "node:crypto";

export function verify(secret, header, body /* Buffer */, tolerance = 300) {
  const parts = Object.fromEntries(header.split(",").map((p) => p.split("=", 2)));
  const t = Number(parts.t);
  if (Math.abs(Date.now() / 1000 - t) > tolerance) return false;
  const expected = createHmac("sha256", secret).update(`${t}.`).update(body).digest("hex");
  return expected.length === parts.v1.length && timingSafeEqual(Buffer.from(expected), Buffer.from(parts.v1));
}
```

A shell, to check one by hand:

```bash
printf '%s.%s' "$t" "$body" | openssl dgst -sha256 -hmac "$secret"
```

## Retries

A delivery succeeds when your receiver answers any `2xx`, within 15 seconds. Otherwise Gantry tries again, waiting longer each time: 1 s, then 2, 4, 8, 16, 32 and 64 s, for 8 tries in all. Each retry is signed afresh, with a new `t` but the same `id`.

It doesn't retry when your receiver refuses the request itself, with a `4xx` other than `408` or `429`, because no retry would change that answer. Redirects aren't followed: give the final URL. Failed and given-up deliveries are in Gantry's log. Deliveries run on their own, so a slow or broken receiver never holds up the dashboard, Alerts or other webhooks.

Answer quickly, and do slow work after answering. Make your receiver safe to call twice with the same `id`.
