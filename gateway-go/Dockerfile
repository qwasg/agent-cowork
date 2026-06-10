# Build the Go edge gateway. Build context: ./gateway-go
FROM golang:1.23-bookworm AS build
WORKDIR /app
COPY go.mod go.sum ./
RUN go mod download
COPY *.go ./
RUN CGO_ENABLED=0 go build -o gateway .

FROM gcr.io/distroless/static-debian12
COPY --from=build /app/gateway /gateway
ENV AGENT_DEBUG_HOST=0.0.0.0 \
    AGENT_DEBUG_HTTP_PORT=8002
EXPOSE 8002
ENTRYPOINT ["/gateway"]
