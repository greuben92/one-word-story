FROM rust:1 as builder
WORKDIR /usr/src/myapp
COPY . .
RUN cargo install --path .

FROM debian:buster-slim
COPY --from=builder /usr/local/cargo/bin/one-word-story /usr/local/bin/one-word-story

CMD ["one-word-story"]
