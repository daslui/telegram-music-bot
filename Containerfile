ARG RUST_VERSION=1.94
ARG APP_NAME=tg-music-bot

FROM quay.io/hummingbird/rust:${RUST_VERSION}-builder AS build
ARG APP_NAME
WORKDIR /app

COPY . /app/

RUN cargo build --release && \
cp ./target/release/$APP_NAME /bin/bot_server

FROM quay.io/hummingbird/core-runtime:latest-openssl AS final

COPY --from=build /bin/bot_server /bin/

CMD ["/bin/bot_server"]

