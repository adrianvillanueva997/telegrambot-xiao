FROM rust:1.100.0-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home bot
WORKDIR /home/bot
COPY --from=build /build/target/release/telegrambot-xiao /usr/local/bin/telegrambot-xiao
USER bot
ENV RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["telegrambot-xiao"]
