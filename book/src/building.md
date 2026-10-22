# Build, lint, and test

## Build

From `firmware/`:

```sh
cargo build --release              # all crates
cargo build --release -p app       # just one
```

The target (`thumbv6m-none-eabi`) is set in `firmware/.cargo/config.toml`.

## Check / lint

From `firmware/`:

```sh
cargo check
cargo clippy
```

- Run them from inside `firmware/`. With `--manifest-path` from the repo root, cargo doesn't
  see `.cargo/config.toml` and builds for the host (`unwinding panics are not supported without std`).

## Test

The libraries in `crates/` have host tests. From `crates/`:

```sh
cargo test
```

- `tpm`: SPI framing and register logic, against mocks. Two more tests run the TPM2 commands
  against [swtpm](https://github.com/stefanberger/swtpm), a software TPM 2.0 (`dnf install swtpm`).
  They're skipped by default; run them with `cargo test -p tpm -- --include-ignored`.
- `hpke`: RFC 9180's test vector for its cipher suite.

They live outside `firmware/` because `firmware/.cargo/config.toml` builds everything under it for
the board, which has no `std` or test harness.
