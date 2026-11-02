ARG RUST_VERSION=latest

FROM quay.io/hummingbird/rust:${RUST_VERSION}-builder AS build
WORKDIR /app
COPY . .
RUN cargo build --release --bin tg-music-bot

FROM quay.io/hummingbird/core-runtime:latest AS runtime
COPY --from=build --chown=65532:65532 /app/target/release/tg-music-bot /usr/local/bin/tg-music-bot
