#!/bin/bash
set -ex

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
export PATH="$HOME/.cargo/bin:$PATH"

curl -LsSf https://astral.sh/uv/install.sh | sh
uv venv --python /opt/python/cp314-cp314/bin/python3.14 /opt/venv
source /opt/venv/bin/activate
uv pip install --upgrade "maturin>=1,<2"
cd /io/
maturin build --release --strip --manylinux --sdist
mkdir -p dist/
cp target/wheels/pyuppsala*.whl ./dist/
cp target/wheels/pyuppsala*.tar.gz ./dist/
