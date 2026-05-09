#ARG RUST_VERSION=1.94

FROM cgr.dev/chainguard/rust:latest-dev AS build
WORKDIR /app
COPY --chown=nonroot:nonroot . .
RUN cargo build --release --bin tg-music-bot

FROM cgr.dev/chainguard/glibc-dynamic AS runtime
COPY --from=build --chown=nonroot:nonroot /app/target/release/tg-music-bot /usr/local/bin/tg-music-bot
CMD ["/usr/local/bin/tg-music-bot"]
