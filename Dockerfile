# Builder
FROM rust:1.96-slim-bookworm AS builder
WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && echo "fn main(){}" > src/main.rs && \
    cargo build --release || true

COPY src ./src
RUN cargo build --release --bin devilray-sui

# Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*
RUN useradd -r -u 1000 -g users appuser

COPY --from=builder /app/target/release/devilray-sui /usr/local/bin/devilray-sui

USER appuser
EXPOSE 3000
CMD ["devilray-sui"]
