# Pinned V8 oracle. Build: docker build -t refract-oracle-node oracle/node
FROM node:26.13.1-alpine@sha256:6bd9ab43c0ec0190ca7b2d193906cb8b7edf6e7f4afa43ddeccd6452710bffb5
COPY runner.mjs /runner.mjs
ENTRYPOINT ["node", "/runner.mjs"]
