# The REST API

Everything the dashboard does, other tools can do too, through the same JSON API under `/api` (PRD §7.7). Use it to show the Fleet on a home dashboard, queue Print Files from a script, or pause every Printer from a chat bot.

- **The reference** is the OpenAPI 3.1 document every Gantry serves at **`/api/openapi.json`**. It needs no login, and holds nothing about your Gantry. Load it into Swagger UI, Postman, Insomnia or a client generator. Each operation says the least role it needs (`x-gantry-role`) and, where it takes a body, gives an example.
- **Changes are audited.** Everything a token changes goes in the audit log, as if its owner did it in the UI, with the token's name beside theirs (`admin (API token: home assistant)`).
- **Events can be pushed to you** instead of polled, as signed [webhooks](webhooks.md).

## API tokens

Make one under **Account → API tokens**: give it a name, and an expiry if you like (1 day to 10 years, or never). Gantry shows the token, `gantry_` and 43 more characters, **once**. Copy it then: only a hash of it is stored, so nobody, Admins included, can see it again. If it is lost, revoke it and make another.

Send it as a bearer token:

```bash
curl -H "Authorization: Bearer gantry_…" https://gantry.example.com/api/printers
```

- **A token acts as its owner, with the owner's role as it is now.** An Operator's token can pause Printers and manage the Queue. A Viewer's can only read. If an Admin changes the owner's role, the token's role changes with it, at once.
- **A token can't manage tokens or account security.** It can't make, list or revoke tokens, set up two-factor authentication, or add passkeys. A leaked token can't be used to make more. Log in to do those.
- **Revoke** a token under Account at any time. An Admin can see and revoke everyone's tokens. Removing a user removes their tokens.
- A request with a revoked, expired or made-up token is refused with `401`. A request without a token or a session gets `401` too.
- An Admin who requires two-factor authentication blocks the tokens of users who haven't set it up yet, until they do.

Make a token per tool, with the least privileged account that can do the job, so each can be revoked on its own and the audit log says which tool did what.

## Examples

Every Printer's live status: Connection, Printer State, the Job, temperatures and Faults.

```bash
curl -s -H "Authorization: Bearer $GANTRY_TOKEN" "$GANTRY/api/printers" | jq '.[] | {name, driver}'
```

Upload a Print File, then queue its first Plate for any Compatible Printer (an Operator's token):

```bash
id=$(curl -s -H "Authorization: Bearer $GANTRY_TOKEN" \
  --data-binary @benchy.gcode.3mf "$GANTRY/api/print-files?name=benchy.gcode.3mf" | jq .id)
curl -s -H "Authorization: Bearer $GANTRY_TOKEN" -H "Content-Type: application/json" \
  -d "{\"print_file_id\": $id, \"plates\": [1]}" "$GANTRY/api/queue"
```

Pause one Printer:

```bash
curl -s -H "Authorization: Bearer $GANTRY_TOKEN" -H "Content-Type: application/json" \
  -d '{"command": "pause"}' "$GANTRY/api/printers/$PRINTER/commands"
```

Live updates are on the WebSocket at `/api/ws`, which takes the same `Authorization` header: a snapshot of the Fleet, then one message per change.

## Errors

Failures are JSON: `{"error": "…"}`, plus `fields` (`[{"field", "message"}]`) for a `422`. The status says what kind:

| Status | Meaning |
|---|---|
| `401` | No session, or the token isn't valid |
| `403` | The role isn't enough (`needed` says which role is), or two-factor setup is required first |
| `404` | No such Printer, Print File, Job… |
| `409` | Not now: for example, the Printer is disconnected or isn't printing (`reason` holds the details) |
| `422` | The input needs attention (`fields`) |
| `504` | The Printer didn't answer in time |

The API is versioned with Gantry itself. The OpenAPI document's `info.version` is the Gantry version serving it.
