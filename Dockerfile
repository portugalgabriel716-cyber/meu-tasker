# Tasker em servidor: só o motor (sem janela), então a imagem é minúscula.
#   docker build -t tasker .
#   docker run -d --restart unless-stopped -v tasker-state:/data \
#       -e TASKER_REFRESH_TOKEN="$(./tasker --bootstrap | sed -n 3p)" tasker
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY assets/icon.png assets/icon.ico assets/
RUN cargo build --release --no-default-features

FROM alpine:3
RUN apk add --no-cache tzdata
# TASKER_STATE mantém o nome da versão cloud antiga: quem já tinha o volume
# continua com a mesma sessão.
ENV TZ=Europe/Berlin \
    TASKER_DATA_DIR=/data \
    TASKER_STATE=/data/cloud_state.json \
    TASKER_NOTIFY=0

COPY --from=build /src/target/release/tasker /usr/local/bin/tasker

# /data PRECISA ser persistente: o refresh token rotaciona, e perder o estado
# quebra a corrente (aí só um --bootstrap novo resolve).
VOLUME ["/data"]
RUN adduser -D -u 10001 tasker && mkdir -p /data && chown tasker:tasker /data
USER tasker

# Saudável = leu o volume nos últimos 15 min (lê o status.json, não abre sessão).
HEALTHCHECK --interval=5m --timeout=30s --start-period=2m --retries=3 \
    CMD tasker --status || exit 1

CMD ["tasker", "--loop"]
