# syntax=docker/dockerfile:1
#
# Quack: one image with the `quack` binary and the dashboard's built files.
# Build from the repository root:
#   docker build -t quack .
# Run `quack serve` (the default command) with the QUACK_* settings of
# docs/design.md §17.7; the dashboard is served from QUACK_DASHBOARD_DIR.

# The dashboard (Vite+ through bun). It imports the shared icon set.
FROM node:24-slim AS dashboard
COPY --from=oven/bun:1 /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /src/apps/dashboard
COPY apps/dashboard/package.json apps/dashboard/bun.lock ./
RUN bun install --frozen-lockfile
COPY assets/icons/quack/svg /src/assets/icons/quack/svg
COPY apps/dashboard/ ./
RUN bun run build

# The Rust toolchain of rust-toolchain.toml, with cargo-chef so the
# dependencies are a cached layer of their own.
FROM rust:1.100.0-slim-trixie AS chef
RUN cargo install cargo-chef --version 0.1.78 --locked
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# sqlx checks its queries against the committed .sqlx data, not a database.
ENV SQLX_OFFLINE=true
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --package quack --recipe-path recipe.json
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .sqlx ./.sqlx
COPY crates ./crates
RUN cargo build --release --locked --package quack \
    && cp target/release/quack /quack

# Distroless with glibc (aws-lc and rustls link against it), no shell, and
# a non-root user.
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=builder /quack /usr/local/bin/quack
COPY --from=dashboard /src/apps/dashboard/dist/app /usr/share/quack/dashboard
ENV QUACK_DASHBOARD_DIR=/usr/share/quack/dashboard \
    QUACK_LISTEN_ADDR=0.0.0.0:8080 \
    QUACK_ENVIRONMENT=production
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/quack"]
# Migrate, then serve: the image has no shell, so a host can't chain the two
# itself.
CMD ["serve", "--migrate"]
