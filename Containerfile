FROM quay.io/centos/centos:stream9 as builder

RUN dnf install --enablerepo=crb -y \
        clang \
        libbpf-devel \
        protobuf-compiler \
        protobuf-devel \
        cargo-1.84.1 \
        rust-1.84.1 \
        rpm \
        rpm-devel && \
    mkdir /app

WORKDIR /app

COPY . .

RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
    cp target/release/fact fact

FROM registry.access.redhat.com/ubi9/ubi-micro:latest

RUN microdnf install -y rpm && microdnf clean all

COPY --from=builder /app/fact /usr/local/bin

ENV FACT_RPMDB=/host/var/lib/rpm
ENV FACT_HOST_MOUNT=/host

ENTRYPOINT ["fact"]
