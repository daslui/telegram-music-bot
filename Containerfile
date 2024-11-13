ARG RUST_VERSION=1.83
ARG APP_NAME=tg-music-bot

FROM docker.io/library/rust:${RUST_VERSION}-alpine AS build
ARG APP_NAME
WORKDIR /app

RUN apk add --no-cache clang lld musl-dev git openssl-dev openssl-libs-static

COPY . /app/

RUN ls -la
RUN cargo build --release && \
cp ./target/release/$APP_NAME /bin/bot_server

FROM alpine:3.18 AS final

ARG UID=1001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    appuser
USER appuser

COPY --from=build /bin/bot_server /bin/

CMD ["/bin/bot_server"]

