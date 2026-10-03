# Tasker em servidor: sem navegador e sem dependências, então a imagem é minúscula.
#   docker build -t tasker .
#   docker run -d --restart unless-stopped -v tasker-state:/data \
#       -e TASKER_REFRESH_TOKEN="$(python3 Tasker.pyw --bootstrap | sed -n 3p)" tasker
FROM python:3.12-alpine

RUN apk add --no-cache tzdata
# TASKER_STATE mantém o nome da versão cloud antiga: quem já tinha o volume
# continua com a mesma sessão.
ENV TZ=Europe/Berlin \
    PYTHONUNBUFFERED=1 \
    TASKER_DATA_DIR=/data \
    TASKER_STATE=/data/cloud_state.json \
    TASKER_NOTIFY=0

WORKDIR /app
COPY Tasker.pyw ./
COPY tasker/ ./tasker/

# /data PRECISA ser persistente: o refresh token rotaciona, e perder o estado
# quebra a corrente (aí só um --bootstrap novo resolve).
VOLUME ["/data"]
RUN adduser -D -u 10001 tasker && mkdir -p /data && chown tasker:tasker /data
USER tasker

# Saudável = leu o volume nos últimos 15 min (lê o status.json, não abre sessão).
HEALTHCHECK --interval=5m --timeout=30s --start-period=2m --retries=3 \
    CMD python Tasker.pyw --status || exit 1

CMD ["python", "Tasker.pyw", "--loop"]
