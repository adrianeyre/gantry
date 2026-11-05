# Read first

Before touching anything, read these in full. They are the canon your work is judged against:

- `README.md`
- `docs/product-requirements-document/gantry.md`
- `GLOSSARY.md` (use its terms exactly, and avoid the words it lists under _Avoid_)
- Every ADR in `docs/architectural-decision-record/`, if there are any yet:

!`ls docs/architectural-decision-record/ 2>/dev/null || echo "(no ADRs yet)"`

# The brief

!`gh issue view {{ISSUE}} --comments`

Every issue listed under "Blocked by" must already be closed. If one is open, stop, comment on this issue saying which, and output <promise>COMPLETE</promise>.

# What already exists and must survive

- The root `package.json` holds the Sandcastle harness that is running you (`@ai-hero/sandcastle`, `tsx`, the `sandcastle` scripts, `engines`, `devEngines`). Extend it; never rewrite it from scratch. Gantry itself is Rust (PRD §5), so the harness is the only reason Node is here.
- `.sandcastle/` is the harness configuration. Do not edit it unless the issue says to.
- `docs/` is the canon. Do not change the PRD or an ADR to match your code; if they disagree, say so on the issue.

# Invariants

- Follow the PRD and every ADR. They are decisions, not suggestions.
- Never put a credential, a printer access code or a serial number in a prompt, a log or a commit.
- Rust 1.99.1 in the sandbox (`cargo`, `clippy`, `rustfmt`); Node 26+ (26.13.0 in the sandbox) and pnpm 12.16.1 for the harness.

# Task

Implement the issue above as a single vertical slice that meets every acceptance criterion. Commit in coherent steps with conventional commit messages. Do not widen the scope beyond "What to build"; anything you notice outside it goes in a comment on the issue.

# Verification

Run every check that exists in the workspace (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` once there is a Cargo workspace), plus anything the acceptance criteria name. Paste their **real output** into a comment on the issue, and tick each acceptance criterion you have verified, saying how. Never reach green by skipping, deleting or weakening a test. This sandbox has no 3D printer and no route to a home network: test Drivers against the Simulated Driver, the mock Bambu broker, a simulated-Klipper container or OctoPrint's virtual printer (PRD §5.6), and say explicitly what could only be verified against a real printer.

# Done

When every acceptance criterion is met and verified, or you are blocked, output <promise>COMPLETE</promise>.
