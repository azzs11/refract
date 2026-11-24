# Pinned Go regexp oracle. Build: docker build -t refract-oracle-go oracle/go
FROM golang:1.27-alpine@sha256:70bd7ddea8b6cbe5d6943cde279d6b67feb10cc924fb9100d1d8757c1575b280 AS build
WORKDIR /src
COPY runner.go .
RUN go mod init runner >/dev/null 2>&1 && CGO_ENABLED=0 go build -o /runner .

FROM scratch
COPY --from=build /runner /runner
ENTRYPOINT ["/runner"]
