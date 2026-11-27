default:
    @just --list

# Install workspace dependencies.
sync:
    uv sync --locked --all-packages

# Check the complete project.
check: rust-check python-check

rust-check:
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked

python-check:
    uv run --locked --only-group dev ruff format --check .
    uv run --locked --only-group dev ruff check .
    uv run --locked --all-packages pyrefly check
    uv run --locked --only-group dev bandit -c pyproject.toml -r .
    uv run --locked --all-packages python -m unittest discover -s transcriber

format:
    cargo fmt --all
    uv run --locked --only-group dev ruff format .

run:
    cargo run --release
