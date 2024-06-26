# Web Search Engine
## Overview

Natural Language Processing powered Web Seach Engine.
It leverages a stack consisting of:
1. **RabbitMQ** - messaging
2. **MongoDB** - storing metadata
3. **Qdrant** - document vector search
4. **Jaeger** - tracing
5. **Grafana** - monitoring
6. **[thenlper/gte-small](https://huggingface.co/thenlper/gte-small)** model - for generating textual embeddings out of page contents

To run simply use docker compose:
```sh
docker compose up -d
```

In order for the crawling to be triggered, an initial manual crawl request must be sent:
```sh
curl -X "POST" "http://localhost:8000/v1/crawl/" -d "https://en.wikipedia.org/wiki/Main_Page"
```
