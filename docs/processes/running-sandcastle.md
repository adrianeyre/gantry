# Running Sandcastle

[Sandcastle](https://github.com/mattpocock/sandcastle) runs a coding agent
against this repository inside a Docker container and brings the commits it
makes back to a branch. It is how Gantry's issues get worked without somebody
at the keyboard: an agent picks up an issue in a sandbox, and a person reviews
and lands the result.

The configuration lives in `.sandcastle/`. This file explains how to use it **in this
project**. For `run()`, the providers and the hooks, see the upstream README.
This file covers only what is specific to Gantry.

Gantry itself is Rust (see the [PRD](../product-requirements-document/gantry.md), §5).
Node and pnpm are here only because the harness is a TypeScript script. Nothing
the product ships depends on them.

---

## What is in `.sandcastle/`

| Path           | What it is                                                                                         |
| -------------- | -------------------------------------------------------------------------------------------------- |
| `Dockerfile`   | The sandbox image: Node 26.13.0, pnpm 12.16.1, Rust 1.99.1 (with clippy and rustfmt), `git`, `gh`, `claude` |
| `main.ts`      | The orchestration entrypoint: picks the prompt, refuses to run on `main`, sets up hooks and the sandbox |
| `export-ca.ts` | `pnpm sandcastle:ca`: writes a gateway's private CA to a file the sandbox can be given             |
| `prompt.md`    | The committed prompt template, with the sections every brief here needs                            |
| `prompt-*.md`  | Your working briefs. Git ignores them, so they stay out of the branch's diff                       |
| `.env.example` | Every variable a run needs, with why each one exists and no values                                 |
| `.gitignore`   | Keeps `.env`, `*.pem`, `logs/`, `worktrees/` and every prompt except the template out of git       |

`logs/` and `worktrees/` are created the first time you run something. Git
ignores both, and `.env` too. **Never commit a credential.**

---

## One-time setup

### 1. Docker

Docker must be running. Check that `docker info` prints its details instead
of failing.

### 2. Dependencies at this checkout

```bash
pnpm install
```

You need Node 26 or later on the host: the root `devEngines` refuses anything
older. `@ai-hero/sandcastle` and `tsx` are root devDependencies. If you get an
`ERR_MODULE_NOT_FOUND` naming `@ai-hero/…`, this step was skipped.

### 3. Credentials

```bash
cp .sandcastle/.env.example .sandcastle/.env
```

**If a key in the file has no value, its value comes from your shell.**
Sandcastle forwards exactly the keys `.sandcastle/.env` names. For each key it
uses the file's value, or `process.env`'s when the file has none. So you can
leave secrets out of the file and export them from your shell instead.

The catch is a silent failure: **a variable your shell exports is not
forwarded unless `.env` names it.** The agent starts unauthenticated and the
log says nothing about the missing variable. When you add a variable to your
environment, add its name to `.env` as well.

- `CLAUDE_CODE_OAUTH_TOKEN` (from `claude setup-token` on the host) runs the
  agent on your Claude subscription. Alternatively set `ANTHROPIC_API_KEY`.
- `GH_TOKEN` is a
  [fine-grained token](https://github.com/settings/personal-access-tokens/new)
  scoped to this repository with **Issues: read and write**, **Metadata: read**
  and **Contents: read**. The agent reads the issue and the docs it cites, and
  comments its verification back on the issue. It deliberately has no
  `Contents: write` and no `Pull requests: write`. The commits come back over
  the bind mount, and a person pushes and opens the pull request.

If you reach Anthropic through a gateway with its own certificate authority,
export `NODE_EXTRA_CA_CERTS` on the host as usual. `main.ts` mounts that file
into the container and points the variable at the mounted copy. If you
forward only the host path, the agent fails with
`unable to verify the first certificate`.

If the host trusts that CA only through its system store, there is no file to
export, so create one:

```bash
pnpm sandcastle:ca    # writes .sandcastle/ca-certificates.pem (ignored)
```

The script connects to `ANTHROPIC_BASE_URL`, finds the root CA that issued the
gateway's certificate in the system store, and writes out its public
certificate. Add it to `.sandcastle/.env` as
`NODE_EXTRA_CA_CERTS=./.sandcastle/ca-certificates.pem`. `main.ts` prefers the
shell's `NODE_EXTRA_CA_CERTS` when that file exists, and otherwise uses the one
in `.env`. If the CA is missing, the agent fails with
`UNABLE_TO_VERIFY_LEAF_SIGNATURE` as soon as it starts.

### 4. Build the image

```bash
pnpm exec sandcastle docker build-image
```

This builds `.sandcastle/Dockerfile` as `sandcastle:<repo-directory-name>` and
passes in your own UID/GID. That way, files the image writes and files written
through the bind mount have the same owner. Rebuild after any change to the
Dockerfile, and use `pnpm exec sandcastle docker remove-image` to delete the
image. Run these with `pnpm exec`, not `npx`: the root `devEngines` requires
pnpm, so npm refuses with `EBADDEVENGINES`.

---

## The image has to match the project

The root `package.json` requires Node 26 or later (`engines` and `devEngines`)
and pins pnpm 12.16.1 (`packageManager`). The image pins Node 26.13.0, the
current Node 26 LTS, and the same pnpm, so every sandbox is identical. If
either changes, change the Dockerfile in the same pull request. A sandbox on
the wrong Node or pnpm usually installs fine and only fails later, which is
much harder to diagnose.

Whatever else you add, keep the four things Sandcastle itself needs: the
non-root `agent` user (Claude Code refuses to run as root), `git`, `gh`, and
`claude` on `PATH`. pnpm and Rust are Gantry's own additions.

### Rust

The agent isn't root, so it can't install system packages. The image therefore
includes everything a Rust build here needs:

- **Rust 1.99.1** (`RUST_VERSION` in the Dockerfile), installed with rustup's
  minimal profile plus `clippy` and `rustfmt`. It lives in the `agent` user's
  home, so `cargo` can write its own registry and build caches.
- **`build-essential` and `pkg-config`**, for crates with C code in them,
  such as `ring` and `aws-lc-rs` (under `rustls`) and `libsqlite3-sys`.

**Keep the image's Rust in step with the workspace.** Once there is a
`rust-toolchain.toml`, change `RUST_VERSION` in the same pull request whenever
it moves, and rebuild the image. If a crate turns out to need another system
library, add it to the `apt-get install` line rather than asking the agent to
install it: the agent can't.

The image deliberately leaves out the Tauri desktop shell's system libraries
(WebKitGTK and friends). The sandbox can't show a window, and the headless
server is the primary deployment shape (PRD §5.2).

---

## Dependencies are installed by a hook, not baked into the image

The image includes no workspace dependencies, because they would go stale with
the next lockfile change. `main.ts` installs them when the sandbox starts:

```ts
onSandboxReady: [
  {
    command: "pnpm install --frozen-lockfile && { [ ! -f Cargo.lock ] || cargo fetch --locked; }",
    timeoutMs: 600_000,
  },
];
```

`--frozen-lockfile` and `--locked` are deliberate. A slice that genuinely adds
a dependency has to commit the lockfile change, which keeps the lockfile
reviewable. The `cargo fetch` does nothing until there is a `Cargo.lock`. pnpm
12 refuses to run a dependency's install scripts unless `pnpm-workspace.yaml`
lists the package under `allowBuilds`, which is why `esbuild` (pulled in by
`tsx`) is listed there.

Docker is a bind-mount provider, so the worktree the agent works in is on your
disk under `.sandcastle/worktrees/…`. You can see the agent's edits on the host
as soon as it makes them.

**`onSandboxReady` hooks run in parallel.** A hook that needs `node_modules` or
the fetched crates must not be a separate entry next to the one that installs
them. Chain commands with `&&` in a single entry instead.
If you get this wrong, you'll see a "command not found" for a tool the install
was about to provide.

---

## Running it

```bash
pnpm sandcastle 9                      # .sandcastle/prompt.md, against issue #9
pnpm sandcastle prompt-issue-9.md 9    # a prompt you wrote beside it
pnpm sandcastle                        # only for a prompt that names no issue
```

| Invocation                            | Prompt                          | Issue |
| ------------------------------------- | ------------------------------- | ----- |
| `pnpm sandcastle 9`                   | `.sandcastle/prompt.md`         | #9    |
| `pnpm sandcastle prompt-issue-9.md 9` | `.sandcastle/prompt-issue-9.md` | #9    |
| `pnpm sandcastle`                     | `.sandcastle/prompt.md`         | none  |

A bare number is taken as the issue. A bare file name is looked up inside
`.sandcastle/` unless it contains a `/`. If the prompt file doesn't exist, the
script prints the usage and exits. The committed `prompt.md` contains
`{{ISSUE}}`, so if you run it without an issue number, the host refuses before
any container is built.

Progress streams to the terminal and a full log is written under
`.sandcastle/logs/`. When the run finishes, `main.ts` prints the commit count,
the completion signal, the log path, and any worktree it had to keep.

`claudeCode()` takes the model and options such as `effort`. Be careful with
`permissionMode`: setting it **replaces** Sandcastle's default
`--dangerously-skip-permissions`, which is what lets a run proceed unattended.

### One prompt is tracked; the rest are yours

`.sandcastle/prompt.md` is the committed template. Git ignores every other
`prompt*.md`, so a brief written for one issue never ends up in a commit. Copy
the template instead of editing it:

```bash
cp .sandcastle/prompt.md .sandcastle/prompt-issue-9.md
$EDITOR .sandcastle/prompt-issue-9.md
pnpm sandcastle prompt-issue-9.md 9
```

---

## Branch strategy: `merge-to-head`

| Strategy        | Where the commits land                                           |
| --------------- | ---------------------------------------------------------------- |
| `head`          | Straight into this working directory, with no worktree or branch |
| `merge-to-head` | A temporary worktree branch, merged back to HEAD                 |
| `branch`        | A named branch in a worktree, kept                               |

`head` is Sandcastle's default for bind-mount providers, which means the agent
edits the files you're looking at. This project uses `merge-to-head` instead:
the agent gets its own worktree, and its commits are merged back onto the
branch you launched from.

**Always launch from a fresh branch cut from `origin/main`. Never launch from
`main` itself or from the previous run's branch.** `main.ts` refuses to start
on `main`. If you build on the last run's branch, its commits end up in your
pull request too, and the review covers two issues instead of one.

```bash
git fetch origin
git checkout -b issue-9-simulated-driver origin/main
pnpm sandcastle 9
```

Work through issues in dependency order. The prompt tells the agent to stop
and say so if any issue under "Blocked by" is still open.

---

## Writing a prompt

Three pieces of syntax do the work:

- <code>!&#96;command&#96;</code> is replaced by the command's stdout **before**
  the agent sees the prompt. These commands run inside the sandbox after the
  `onSandboxReady` hooks have finished. If one exits non-zero, the run fails
  immediately, which is why the template's `ls` of the ADR directory falls
  back to an `echo` while there are no ADRs.
- `{{KEY}}` is filled in from `promptArgs` on the host before any of those
  commands run, so `{{ISSUE}}` is already substituted by the time
  `gh issue view` runs. `{{SOURCE_BRANCH}}` and `{{TARGET_BRANCH}}` are built
  in.
- `<promise>COMPLETE</promise>` ends the iteration loop early. Sandcastle never
  adds it for you. If the prompt doesn't ask the agent to output it, the run
  continues until `maxIterations` or the idle timeout.

### What a prompt here must include

`prompt.md` has these parts, and any brief you write should keep them:

1. **The reading, before the task:** `README.md`, the
   [PRD](../product-requirements-document/gantry.md), the
   [glossary](../../GLOSSARY.md), and every ADR in
   [`docs/architectural-decision-record/`](../architectural-decision-record/).
2. **The brief, fetched live** with <code>!&#96;gh issue view {{ISSUE}}&#96;</code>
   instead of pasted, so it can't drift from the issue.
3. **What already exists and must survive.** In particular, the root
   `package.json` holds the harness that is running the agent. An agent given
   a scaffolding task, such as setting up the Cargo workspace, will otherwise
   rewrite it from scratch.
4. **The invariants:** the PRD and every ADR are binding, and no credential,
   printer access code or serial number ever appears in a prompt, log or
   commit.
5. **Verification:** the exact commands to run (`cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test`), an instruction
   to paste their real output back on the issue, and an instruction never to
   get to green by weakening a test.

Point 5 matters most. An agent that grades its own work will report success
based on the same wrong assumptions it built with. The pasted output is what a
person checks before opening the pull request.

---

## What a sandbox here cannot do

- **It cannot reach a printer.** The sandbox is a container with no route to
  your home network. That means no Bambu MQTT broker, no camera stream, no
  FTPS upload and no LAN discovery. Drivers are tested against the
  Simulated Driver, the mock Bambu broker, a simulated-Klipper container or
  OctoPrint's virtual printer (PRD §5.6).
  Anything that can only be proven against real firmware goes on the pull
  request as unverified.
- **It cannot run the Tauri desktop app.** It has no display and none of
  Tauri's system libraries. The core and server crates build and test there;
  the desktop shell can only be run from the host.
- **It has no Docker of its own.** The sandbox is a container, so it cannot
  build or run Gantry's container image.
- **It must not reach GitHub repositories other than this one.**
- **It cannot push or open a pull request.** This is by design, because
  `GH_TOKEN` has no write access to contents. A person lands the work.

A pull request should list, one by one, every test that was skipped or couldn't
run in the sandbox, so the reviewer knows exactly what still has to be proven
on the host or in CI.

---

## Landing a run

A run isn't finished when the agent stops. It's finished when the work is on
the remote, behind a pull request, on a branch that contains this issue and
nothing else. Do these steps on the host:

```bash
# 1. The gate. Run every check that exists.
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test

# 2. Commit anything the agent left uncommitted, as coherent commits.
git add -A && git commit

# 3. Bring main in, and resolve any conflicts here rather than in the PR.
git fetch origin && git merge origin/main

# 4. Re-run the gate on the merged result. A clean merge doesn't mean a passing build.
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test

# 5. Push and open the pull request, closing the issue.
git push -u origin HEAD
gh pr create --base main --fill
```

Then check that the diff covers only this issue: `git diff origin/main --stat`
should list this issue's files and nothing from an earlier run.

### What the pull request should say

1. **What the slice does**, in the [glossary](../../GLOSSARY.md)'s vocabulary
   (Printer, Driver, Fleet, Job), and `Closes #<issue>`.
2. **Which acceptance criteria are met, and how each was verified.**
3. **The verification output, pasted**: the real output from step 4, not a
   claim that it passed.
4. **What wasn't verified here**, such as behaviour against a real printer, and
   **what is deliberately left out**, with the reason.

---

## Cleaning up

Worktrees live under `.sandcastle/worktrees/`. A clean worktree is deleted when
the run ends. A worktree with uncommitted changes is **kept** so that work
survives a crash, and `main.ts` prints its path. Remove it with
`git worktree remove` once you've dealt with it.
`pnpm exec sandcastle docker remove-image` deletes the image.

---

## Troubleshooting

| Symptom                                             | Cause                                                                                                                   |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `ERR_MODULE_NOT_FOUND` for `@ai-hero/…`             | `pnpm install` not run at this checkout                                                                                 |
| `pnpm install` refuses on the host                  | The host's Node is older than 26, which `devEngines` refuses                                                            |
| The agent exits asking to authenticate              | `.sandcastle/.env` missing, or it names no working credential                                                           |
| The agent is unauthenticated despite a set variable | `.sandcastle/.env` does not name that key, so it was never forwarded                                                    |
| `unable to verify the first certificate`            | A private CA that isn't reachable inside the container. Export `NODE_EXTRA_CA_CERTS` on the host so `main.ts` mounts it |
| `gh` says it cannot read the issue                  | `GH_TOKEN` has no **Issues: read** on this repository                                                                   |
| Permission errors on files the agent wrote          | Image built without your UID. Rebuild with `pnpm exec sandcastle docker build-image`                                    |
| ``Refusing to run on `main` ``                      | Working as intended. Cut a branch; the message prints the command                                                       |
| The run hangs long after the work is done           | The prompt never asked for `<promise>COMPLETE</promise>`                                                                |
| A tool is "not found" in an `onSandboxReady` hook   | Hooks run in parallel and it raced the install. Chain them with `&&`                                                    |
| `cargo fetch --locked` fails in the setup hook      | `Cargo.toml` changed without its `Cargo.lock`. Commit the lockfile                                                      |
| A crate's build script can't find a C library       | The image lacks that system package. Add it to the Dockerfile's `apt-get install` and rebuild                           |
| `npm error EBADDEVENGINES`                          | Ran via `npx`. Use `pnpm exec sandcastle …`                                                                             |
| `ERR_PNPM_IGNORED_BUILDS`                           | pnpm 12 needs the package in `allowBuilds` in `pnpm-workspace.yaml`                                                     |
| A usage message instead of a run                    | No prompt at that path, or the prompt names `{{ISSUE}}` and no issue was given                                          |

---

## See also

- [Sandcastle](https://github.com/mattpocock/sandcastle): the upstream
  reference for `run()`, providers and hooks
- [`docs/product-requirements-document/gantry.md`](../product-requirements-document/gantry.md):
  the requirements every issue cites
