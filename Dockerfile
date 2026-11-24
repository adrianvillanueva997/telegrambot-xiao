FROM rust:1.100.0-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked

FROM python:3.12.13-slim-bookworm AS python-deps
COPY --from=ghcr.io/astral-sh/uv:0.13.8 /uv /usr/local/bin/uv
WORKDIR /app/transcriber
COPY transcriber/pyproject.toml transcriber/uv.lock ./
RUN uv sync --locked --no-dev --no-install-project

FROM debian:bookworm-slim AS model
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://alphacephei.com/vosk/models/vosk-model-small-es-0.42.zip -o /tmp/model.zip \
    && echo '09b239888f633ef2f0b4e09736e3d9936acfd810bc65d53fad45261762c6511f  /tmp/model.zip' | sha256sum -c - \
    && unzip -q /tmp/model.zip -d /opt \
    && rm /tmp/model.zip

FROM python:3.12.13-slim-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home bot
WORKDIR /home/bot
COPY --from=build /build/target/release/telegrambot-xiao /usr/local/bin/telegrambot-xiao
COPY --from=python-deps /app/transcriber/.venv /app/transcriber/.venv
COPY --from=model /opt/vosk-model-small-es-0.42 /opt/vosk-model-small-es-0.42
COPY transcriber/worker.py /app/transcriber/worker.py
ENV TRANSCRIBER_PYTHON=/app/transcriber/.venv/bin/python
ENV TRANSCRIBER_WORKER=/app/transcriber/worker.py
USER bot
ENV RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["telegrambot-xiao"]
