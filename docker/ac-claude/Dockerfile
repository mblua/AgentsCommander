ARG BRIDGE_IMAGE=agentscommander/session-bridge:latest
FROM ${BRIDGE_IMAGE} AS bridge

FROM node:22-bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @anthropic-ai/claude-code \
    && npm cache clean --force

RUN mkdir -p /workspace /home/node/.claude \
    && chown -R node:node /workspace /home/node/.claude

COPY --from=bridge /usr/local/bin/session-bridge /usr/local/bin/session-bridge
COPY --from=bridge /usr/local/bin/agentscommander-api-helper /usr/local/bin/agentscommander-api-helper

ENV HOME=/home/node
USER node
WORKDIR /workspace

ENTRYPOINT ["session-bridge"]
