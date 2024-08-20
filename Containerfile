FROM rust:latest AS builder

RUN useradd --home /home/user -M user -K UID_MIN=10000 -K GID_MIN=10000 && mkdir /home/user && chown user /home/user && chmod 0770 /home/user

USER user

WORKDIR /home/user

COPY --chown=user . /home/user/vdirsyncer-rs

RUN cd /home/user/vdirsyncer-rs && cargo build

FROM debian:stable-slim

RUN useradd --home /home/user -M user -K UID_MIN=10000 -K GID_MIN=10000 && mkdir /home/user && chown user /home/user && chmod 0770 /home/user && apt update && apt upgrade -y && apt install -y libsqlite3-0 libgcc-s1 && apt clean && rm -rf /var/lib/apt/lists/*

USER user

WORKDIR /home/user

COPY --from=builder /home/user/vdirsyncer-rs/target/debug/vdirsyncer /usr/local/bin/

ENTRYPOINT ["/usr/local/bin/vdirsyncer"]
