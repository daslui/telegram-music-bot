ARG RUST_VERSION=1.94
ARG APP_NAME=tg-music-bot

FROM cgr.dev/chainguard/rust:latest-dev as build
USER root
RUN apk add --no-cache openssl-dev pkgconf
USER nonroot
WORKDIR /app
COPY --chown=nonroot:nonroot . .
RUN cargo build --release

FROM cgr.dev/chainguard/glibc-dynamic
COPY --from=build --chown=nonroot:nonroot /app/target/release/${PACKAGE} /usr/local/bin/${PACKAGE}
CMD ["/usr/local/bin/${PACKAGE}"]
