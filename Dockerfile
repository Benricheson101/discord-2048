FROM rust:alpine AS builder
RUN apk add gcc musl-dev
WORKDIR /app
COPY . .
RUN cargo build --release

FROM alpine AS runtime
EXPOSE 3000
COPY --from=builder /app/target/release/discord-2048 /usr/bin/
ENTRYPOINT ["/usr/bin/discord-2048"]
