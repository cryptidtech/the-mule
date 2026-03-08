FROM python:3.12-slim
RUN apt-get update && apt-get install -y --no-install-recommends redis-tools && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY pyproject.toml ./
COPY the_mule/ ./the_mule/
RUN pip install --no-cache-dir .
COPY reference-peer.py /usr/local/bin/reference-peer
RUN chmod +x /usr/local/bin/reference-peer
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
