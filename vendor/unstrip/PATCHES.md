# Local unstrip patch provenance

## Upstream baseline

- Crate: `unstrip` **1.2.0**, still pinned by the root `=1.2.0` requirement.
- Archive: <https://static.crates.io/crates/unstrip/unstrip-1.2.0.crate>
- Archive SHA-256 (verified against the original registry Cargo.lock entry before
  switching to the local path):
  `cae558eb27eae38dd74ba71e129efdf23d5948cf49ae5c33a3857cc3a53a96e6`
- Published `.cargo_vcs_info.json` revision:
  `7a96e2da723984850f55301da3abfcaa2aa59356`, empty `path_in_vcs`.
- All 49 archive files were copied byte-for-byte before patching. Registry cache
  bookkeeping such as `.cargo-ok` and `.cargo-checksum.json` is not vendored.
- Original `LICENSE-MIT`, `LICENSE-APACHE`, VCS metadata, Cargo manifests and
  Cargo.lock are retained. The upstream dual-license notices continue to apply.

## Functional patch: strict-buildinfo-v1

The only functional decoder changes are in `src/buildinfo/mod.rs`, with
self-contained regression tests in the same module:

- `BuildInfo::parse_checked` returns owned `BuildInfoDecode` status and typed
  issues. Complete requires a validated header, checked strings, valid module
  framing and no detected record errors.
- Bounds-check the full 32-byte header, pointer/inline flags, string positions,
  varint overflow, integer conversion and offset/length arithmetic.
- Discover the first structurally usable header, skipping invalid magic lookalikes
  with the same checked header validation used by decoding. Do not rank or retry
  string/module payloads. No magic yields MissingHeader; all-invalid candidates
  yield one deterministic InvalidHeader issue, without per-match accumulation.
- Preserve a recovered raw Go version when subsequent module-string reads,
  framing or records fail. Preserve individually validated records from a
  framed module body; invalid records produce issues rather than fabricated
  empty fields or silently decoded success. Replacements require an immediately
  preceding validated parent.
- Keep `BuildInfo::parse` as a wrapper over the same checked implementation.
  Intentional missing/short-module version-only compatibility remains; malformed
  headers, strings, framing and records return errors.
- Validate the module sentinels from `cmd/go/internal/modload.infoStart/infoEnd`.
  Unsupported pointer-indirected layouts remain unsupported.

The application's owned BuildWalk maps complete/partial/unavailable status
without reparsing, preserving the raw version separately from module identity.
Producer patch identity is `strict-buildinfo-v1`, base version `1.2.0`.
Function/inline recovery is not implemented in this slice. In particular,
`src/inline/mod.rs` is byte-identical to the archive. This is not a general
parser hardening or crash-proof parsing claim.

## Separately authorized mechanical hygiene

The published baseline did not pass full rustfmt and library Clippy on the tested
toolchain. Only these mechanical deltas accompany the functional patch:

- Rustfmt-only changes to `src/gobin/mod.rs`, `src/moduledata/mod.rs`,
  `src/pclntab/mod.rs`, and `src/types/mod.rs`.
- In moduledata's two NameEnc probes (original lines 179/182), pass `nbytes`
  directly instead of the needless `&nbytes` borrow.

No other implementation modules were changed. There are no gate waivers, lint
suppressions or additional parser/ABI changes.

## Validation and replacement criteria

Run `just patch-check` from the application root for **full** vendor formatting,
locked library tests and warning-denied library Clippy. Build output is under
ignored root `target/vendor-unstrip/`. The full application gate is `just check`.

Remove this local patch only when an upstream release provides equivalent checked
bounds, version-preserving recovery status and detected-record-error reporting,
and both dependency tests and application regressions pass against that release
without weakening their assertions. Revisit provenance and the producer patch
label as part of that change; do not silently substitute version-only legacy
parse for the checked API. Future inline work must carry its own explicit patch
identity and tests; it is not part of `strict-buildinfo-v1`.
