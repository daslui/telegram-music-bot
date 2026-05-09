ARG RUST_VERSION=1.94

FROM cgr.dev/chainguard/rust:latest-dev AS build
USER root
RUN apk add --no-cache openssl-dev pkgconf
USER nonroot
WORKDIR /app
COPY --chown=nonroot:nonroot . .
RUN cargo build --release --bin tg-music-bot

FROM cgr.dev/chainguard/wolfi-base AS libs
USER root
COPY --from=cgr.dev/chainguard/glibc-dynamic:latest / /chroot/
RUN apk add --no-cache --no-scripts --root /chroot --initdb \
      --keys-dir /etc/apk/keys \
      --repositories-file /etc/apk/repositories \
      openssl \
      ca-certificates-bundle
RUN rm -rf /chroot/lib/apk /chroot/var/cache/apk


FROM scratch
COPY --from=libs /chroot/ /
COPY --from=build --chown=nonroot:nonroot /app/target/release/tg-music-bot /usr/local/bin/tg-music-bot
CMD ["/usr/local/bin/tg-music-bot"]
