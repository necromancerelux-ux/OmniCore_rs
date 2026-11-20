# OmniCore Discord bot — multi-stage build (static musl binary, x86_64 or aarch64)
# Build:  docker build -t omnicore .
#         docker build --build-arg WITH_CHROMIUM=1 -t omnicore .   (adds Chromium for Oracle renewal)
# Run:    docker run --env-file .env -v omnicore-data:/data omnicore

FROM rust:1-bookworm AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config musl-tools \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets

# Build for whatever CPU this image is being built on (x86_64 or aarch64).
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64)  T=x86_64-unknown-linux-musl ;; \
      aarch64) T=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported CPU $(uname -m)"; exit 1 ;; \
    esac; \
    rustup target add "$T"; \
    export CC_$(echo "$T" | tr - _)=musl-gcc; \
    cargo build --locked --release --target "$T"; \
    cp "target/$T/release/omnicore" /omnicore

FROM debian:bookworm-slim
ARG WITH_CHROMIUM=0
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && if [ "$WITH_CHROMIUM" = "1" ]; then apt-get install -y --no-install-recommends chromium; fi \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m omnicore
WORKDIR /app
COPY --from=builder /omnicore /usr/local/bin/omnicore
COPY assets /app/assets
RUN mkdir -p /data && chown -R omnicore:omnicore /data /app
USER omnicore
ENV DATABASE_PATH=/data/bot.sqlite
ENV RUST_LOG=info
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/omnicore"]
