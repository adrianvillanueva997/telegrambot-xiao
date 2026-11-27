FROM rust:1.100.0-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked

FROM python:3.15.2-slim-bookworm AS python-deps
COPY --from=ghcr.io/astral-sh/uv:0.13.10 /uv /usr/local/bin/uv
WORKDIR /app
COPY pyproject.toml uv.lock ./
COPY transcriber/pyproject.toml transcriber/pyproject.toml
RUN UV_PROJECT_ENVIRONMENT=/app/transcriber/.venv uv sync --locked --package xiao-transcriber --no-dev --no-install-project

FROM python-deps AS model
RUN HOME=/tmp/vosk /app/transcriber/.venv/bin/python -c 'from vosk import Model; Model(lang="es")' \
    && mv /tmp/vosk/.cache/vosk/vosk-model* /opt/vosk-model

FROM python:3.15.2-slim-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home bot
WORKDIR /home/bot
COPY --from=build /build/target/release/telegrambot-xiao /usr/local/bin/telegrambot-xiao
COPY --from=python-deps /app/transcriber/.venv /app/transcriber/.venv
COPY --from=model /opt/vosk-model /opt/vosk-model
COPY transcriber/worker.py /app/transcriber/worker.py
ENV TRANSCRIBER_PYTHON=/app/transcriber/.venv/bin/python
ENV TRANSCRIBER_WORKER=/app/transcriber/worker.py
USER bot
ENV RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["telegrambot-xiao"]
