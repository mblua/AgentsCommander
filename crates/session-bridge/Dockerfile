FROM rust:1-bookworm AS build

WORKDIR /src
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY src-tauri/Cargo.toml src-tauri/Cargo.toml
RUN mkdir -p src-tauri/src \
    && printf 'fn main() {}\n' > src-tauri/src/main.rs \
    && printf '' > src-tauri/src/lib.rs
COPY crates/session-bridge crates/session-bridge
COPY crates/terminal-snapshot-renderer crates/terminal-snapshot-renderer
RUN cargo build --locked --release -p session-bridge --bins

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/session-bridge /usr/local/bin/session-bridge
COPY --from=build /src/target/release/agentscommander-api-helper /usr/local/bin/agentscommander-api-helper

ENTRYPOINT ["session-bridge"]
