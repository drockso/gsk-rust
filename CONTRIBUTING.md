# Contributing

Bug reports, protocol evidence and small focused pull requests are welcome. English and Russian are both fine.

## Local development

Install Rust and Python 3. The release was built with Rust 1.94.1. Keep `Cargo.lock` checked in.

```text
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python tests/integration.py target/release/gsk.exe
```

On Linux use `python3` and `target/release/gsk`. Integration tests bind to loopback only; no CNC is needed. A protocol change should include a regression test for an observable behavior or a sanitized captured packet.

## Working with protocol evidence

Distinguish captured behavior from assumptions. Describe the firmware/model if known, operation, expected result and actual result. Remove personal paths, network configuration and production program contents from logs before sharing. Do not attach vendor DLLs or proprietary binaries.

Hardware experiments must be explicitly authorized by the machine owner, use unused test program numbers, and never execute a program. The opt-in hardware harness only touches O3996/O3997 and refuses existing names. It is not part of CI.

Preserve file bytes, validate response lengths before indexing, and consider lost replies before adding retries to an operation that changes remote state. Keep the CLI independent of the vendor application.

## Pull requests

Explain the user-visible change, why it is needed, and the checks you ran. Document compatibility limits rather than assuming that one controller represents every GSK model. Contributions are accepted under the project's MIT license.
