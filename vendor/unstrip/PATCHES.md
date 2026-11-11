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
Producer patch identity is `strict-buildinfo-v1` (build-info slice only), base
version `1.2.0`. The inline decoder changes above are a separate patch layer
(`strict-inline-v1`); the combined producer identity reported by the
application after the inline slice is `strict-buildinfo-v1+strict-inline-v1`.
This is not a general parser hardening or crash-proof parsing claim.

## Functional patch: strict-inline-v1

The inline decoder changes are confined to `src/inline/mod.rs`, with
self-contained synthetic pcvalue-stream tests in the same module and
fixture-backed regressions in the application's `tests/inline_regressions.rs`:

- `decode_inline_tree_checked` returns owned `InlineTreeDecode` status with
  typed `InlineIssueKind` issues. Complete requires a known expected entry
  count derived from a completed PCDATA_InlTreeIndex scan, exactly that many
  validated entries, and no suppressed errors. A genuine empty tree
  (absent stream/slot, sentinel, or a scan that proves zero indices) is
  complete with `Some(0)`; truncated or contradictory metadata never is.
- Short `_func` headers, short pcdata/funcdata slot reads and unlocatable
  trees are explicit `TruncatedFunction`/`TruncatedTree` outcomes instead of
  `Ok(vec![])`; a later slot failure retains the count the index scan
  already established.
- Metadata contradictions (a counted tree with no addressable funcdata slot,
  an absent tree with a counted pcdata stream, a tree with no index stream,
  or a non-go1.20 table magic) are `UnsupportedLayout`, never a decoded zero.
- A missing or zero gofunc base is `MissingGofunc`; tree addresses use
  checked arithmetic (`AddressOverflow` replaces wrapping adds).
- `max_pcvalue` walks pcvalue streams with the runtime's rules (zig-zag value
  deltas, quantum-scaled pc deltas, zero-value-delta terminator, pc-bound
  completion) under checked arithmetic. Truncated/overflowing varints,
  out-of-range offsets, arithmetic overflow, an exhausted scan budget
  (`SCAN_BUDGET`), and exhaustion without proven termination are errors,
  never a best-effort maximum.
- Per-tree decoding is bounded by `MAX_INLINE_ENTRIES`; exceeding the cap is
  reported as `EntryLimit` instead of silently truncating. A record whose
  `parent_pc` falls outside the host function is `InvalidParentPc`; the
  validated prefix is retained and the tainted record is never returned.
- `decode_inline_tree` remains as a wrapper and returns `Ok` only for
  complete results, so the legacy API no longer reports truncated or
  unlocatable trees as clean empty successes.

This is not a general parser-hardening or crash-proof-parsing claim, and it
is not a version-wide support claim: the 16-byte `runtime.inlinedCall`
layout is decoded only for the go1.20+ table magic, and the application
additionally gates on the recorded Go version family.

## Functional patch: strict-textstart-v1
The pclntab changes are confined to `src/pclntab/mod.rs`, with self-contained
synthetic regressions in the same module and a fixture-backed regression in
the application's `tests/functions.rs` (`app.extpie.stripped`, built with
CGO and `-linkmode=external -buildmode=pie`):
- When the modern-layout pclntab header's `textStart` is zero in the file —
  which happens for externally linked PIEs (an `R_X86_64_RELATIVE`
  relocation fills it at load) and some internal links — resolve the text
  base from the statically present `moduledata.text` (word 22 from the
  pclntab-anchored candidate) instead of the first executable section's
  address. The runtime's own functab `entryOff` arithmetic is
  `moduledata.text + entryOff`, so this is the runtime's base, statically
  available; `go tool nm` ground truth on a stock-toolchain external-PIE
  reproduction and the moduledata `minpc`/`etext` sanity on the first real
  Red Hat artifact both confirm it.
- The moduledata candidate is found by scanning the data sections
  (noptrdata, data, rodata, in that order) for the pclntab address anchor,
  mirroring the moduledata locator, and must pass the runtime's region
  sanity (`minpc <= maxpc`, nonzero `text <= etext`) with a section-mapped
  text base; otherwise it is rejected structurally.
- A non-zero header `textStart` keeps the previous behavior verbatim, and a
  missing or incredible moduledata candidate keeps the previous `.text`
  section fallback. Pre-1.18 layouts (absolute functab entries) are
  untouched.
This is not a general parser-hardening or crash-proof-parsing claim. Remove
the strict-textstart-v1 portion only when an upstream release resolves a
zero header `textStart` to the true Go text base (or documents an equivalent
static resolution), and both the vendor pclntab tests and the application's
external-PIE regression pass against that release without weakening their
assertions.

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
without weakening their assertions. Remove the strict-inline-v1 portion only
when an upstream release provides equivalent checked inline outcomes — typed
issues, count-establishing pcdata scans, retained prefixes and observable caps
— and both the vendor inline tests and the application inline regressions pass
without weakening their assertions. Revisit provenance and the producer patch
label as part of that change; do not silently substitute version-only legacy
parse for the checked API. The combined producer identity reported by the
application after all three slices is
`strict-buildinfo-v1+strict-inline-v1+strict-textstart-v1`.
