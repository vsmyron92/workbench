# Development image for Workbench: Rust (server/), Node 22 (web/), and the tools the
# test suite and the dev loop shell out to. Everything is in the image rather than in
# devcontainer "features", so it builds the same under VS Code, the devcontainer CLI
# and Workbench's own built-in engine (which does not apply features).
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# - clang, cmake, pkg-config: aws-lc-rs (rustls) builds C code.
# - git, python3, psmisc (fuser), openssl, util-linux (script): used by tests and run configs.
# - nodejs 22 from NodeSource: the web app's toolchain (Vite 8 needs Node >= 20.19).
RUN apt-get update \
 && export DEBIAN_FRONTEND=noninteractive \
 && apt-get install -y --no-install-recommends \
      ca-certificates clang cmake curl git gnupg openssl pkg-config psmisc python3 util-linux \
 && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

# Directories that named volumes mount over: a new volume copies the image's
# ownership, so they must exist here, owned by the remote user.
RUN mkdir -p /home/vscode/.cache/workbench-targets /home/vscode/.claude \
 && chown -R vscode:vscode /home/vscode/.cache /home/vscode/.claude

# Claude Code, for agent sessions that run inside the container (log in once with
# `claude`; the login lives in the per-container ~/.claude volume, never the host's).
USER vscode
RUN curl -fsSL https://claude.ai/install.sh | bash
ENV PATH="/home/vscode/.local/bin:${PATH}"
USER root
