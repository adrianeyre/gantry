# The sandbox image an agent runs inside.
#
# Four things Sandcastle needs and this file must keep: a non-root `agent` user
# (Claude Code refuses to run as root), `git` for the commits, `gh` for the
# issues, and `claude` on PATH. pnpm and the Rust toolchain are Gantry's own
# additions.
#
# Node is pinned to an exact version within the root package.json's minimum.
# `engines` is a warning rather than a wall, so a sandbox on an older Node
# installs happily and then fails somewhere further in. If that minimum moves
# past this pin, move this one with it.
FROM node:26.13.0-bookworm

# build-essential and pkg-config are what Rust crates with a C part (ring,
# aws-lc-rs, libsqlite3-sys…) need to compile. Installing them needs root,
# which the agent is not, so they are baked in here.
RUN apt-get update && apt-get install -y --no-install-recommends \
  git \
  curl \
  jq \
  ca-certificates \
  build-essential \
  pkg-config \
  && rm -rf /var/lib/apt/lists/*

# GitHub CLI — the agent reads the issue it is working and comments on it.
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
  | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
  && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
  | tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
  && apt-get update && apt-get install -y --no-install-recommends gh \
  && rm -rf /var/lib/apt/lists/*

# pnpm, pinned to the root package.json's `packageManager`. Node stopped
# shipping corepack as of 25, so pnpm is installed directly.
RUN npm install -g pnpm@12.16.1 && rm -rf /root/.npm

# UID/GID alignment. `sandcastle docker build-image` defaults these to the host
# user's, so files the image writes and files the bind mount writes share an
# owner and no runtime chown is needed. A mismatch here is the cause of
# "permission denied" on a file the agent just created.
ARG AGENT_UID=1000
ARG AGENT_GID=1000

# Rename the base image's `node` user to `agent` and align its UID/GID.
RUN groupmod -o -g $AGENT_GID node \
  && usermod -o -u $AGENT_UID -g $AGENT_GID -d /home/agent -m -l agent node
USER ${AGENT_UID}:${AGENT_GID}

# Rust, installed for the agent user so `cargo` can write its registry and
# target caches. Pinned rather than `stable`, so every sandbox builds with the
# same compiler. If the workspace gains a `rust-toolchain.toml`, move this pin
# with it in the same pull request.
ARG RUST_VERSION=1.99.1
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path \
  --profile minimal --default-toolchain ${RUST_VERSION} --component clippy,rustfmt
ENV PATH="/home/agent/.cargo/bin:$PATH"

RUN curl -fsSL https://claude.ai/install.sh | bash
ENV PATH="/home/agent/.local/bin:$PATH"

# Sandcastle bind-mounts the git worktree here and makes it the working
# directory at container start, so this has to be able to serve as the
# project root.
WORKDIR /home/agent/workspace

ENTRYPOINT ["sleep", "infinity"]
