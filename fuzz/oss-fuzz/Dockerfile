# OSS-Fuzz build image for pyuppsala.
#
# pyuppsala is a maturin/PyO3 project, so the image needs a Rust toolchain in
# addition to the Python base builder. Copy this file (with build.sh and
# project.yaml) into oss-fuzz/projects/pyuppsala/.
FROM gcr.io/oss-fuzz-base/base-builder-python

# Rust toolchain for building the native extension. A nightly component is
# installed so the address-sanitizer build can pass -Zsanitizer=address to
# instrument the Rust code itself (see build.sh).
RUN apt-get update && apt-get install -y --no-install-recommends curl && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --default-toolchain stable --component rust-src && \
    rm -rf /var/lib/apt/lists/*
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup toolchain install nightly --component rust-src && \
    pip3 install --no-cache-dir maturin

# The upstream repo carries the harnesses under fuzz/ (OSS-Fuzz Python layout).
RUN git clone --depth 1 https://github.com/kushaldas/pyuppsala.git $SRC/pyuppsala
WORKDIR $SRC/pyuppsala
COPY build.sh $SRC/
