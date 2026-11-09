FROM ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c
ARG RUST_VERSION=1.99.1
ARG NODE_VERSION=24.14.1
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl xz-utils pkg-config file python3 git \
    libwebkit2gtk-4.1-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:/usr/local/bin:/usr/bin:/bin
RUN curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION" \
    && chmod -R a+rX /opt/rustup /opt/cargo && rm /tmp/rustup.sh
COPY node-checksums.json /opt/cortex-node-checksums.json
RUN arch="$(dpkg --print-architecture)" \
    && case "$arch" in amd64) node_arch=x64;; arm64) node_arch=arm64;; *) exit 1;; esac \
    && cd /tmp \
    && curl -fsSLO "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-$node_arch.tar.xz" \
    && python3 -c 'import json,sys; d=json.load(open("/opt/cortex-node-checksums.json")); assert d["version"] == sys.argv[1]; print(d["sha256"][sys.argv[2]] + "  " + sys.argv[3])' "$NODE_VERSION" "$node_arch" "node-v$NODE_VERSION-linux-$node_arch.tar.xz" | sha256sum -c - \
    && tar -xJf "node-v$NODE_VERSION-linux-$node_arch.tar.xz" -C /usr/local --strip-components=1 \
    && rm node-*.tar.xz
COPY build.sh /usr/local/bin/build-workspace
ENTRYPOINT ["/bin/sh", "/usr/local/bin/build-workspace"]
