FROM rustlang/rust:nightly-trixie-slim AS chef
WORKDIR /empath

RUN apt update && apt install -y git clang mold

ENV RUSTFLAGS="-C linker=clang -C link-arg=-fuse-ld=/usr/bin/mold"
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
ENV CARGO_UNSTABLE_SPARSE_REGISTRY=true
ENV CARGO_INCREMENTAL=0

RUN cargo +nightly install cargo-chef

FROM chef AS planner
COPY . .
RUN cargo +nightly chef prepare --recipe-path recipe.json

FROM chef AS empath
COPY --from=planner /empath/recipe.json recipe.json
RUN cargo +nightly chef cook --release --recipe-path recipe.json

RUN apt-get update && apt-get install -y libpython3.13-dev

COPY . .
RUN cargo +nightly build --release

FROM debian:trixie-slim

RUN apt-get update && apt-get install -y libpython3.13-dev

WORKDIR /empath

COPY --from=empath /empath/target/release .

VOLUME /config

ENV LOG_LEVEL="info"
ENV LD_LIBRARY_PATH="/empath/modules:${LD_LIBRARY_PATH}"

ENTRYPOINT [ "bash", "--login", "-c" ]

CMD [ "/empath/empath" ]
