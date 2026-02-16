#!/bin/bash

set -e

RUSTFLAGS=-Dwarnings cargo build -p volexc

cargo test -p volexc

cargo run --bin tester -- --lang rust
cargo run --bin tester -- --lang go
cargo run --bin tester -- --lang typescript

cargo run -p rpc-tester -- --all --transport tcp
cargo run -p rpc-tester -- --all --transport http
