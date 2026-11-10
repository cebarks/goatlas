# First Red Hat artifact validation

First real-artifact run of the golang-symbol CLI (schema v1, producer patch
`strict-buildinfo-v1+strict-inline-v1`) against an owner-supplied Red Hat
Enterprise Linux Go binary. This record is the acceptance evidence required by
the implementation plan's Task 4. It documents coverage achieved, cross-checks,
a demonstrated address-derivation defect in the vendored parser, and the
resulting limitations. No CVE affectedness or source-patch conclusions are
drawn anywhere in this record.

## Artifact

- Package: `cri-o-5.0.0-202610292257.p2.g9186725.assembly.stream.el10.x86_64.rpm`
  (RPM v3, 21,139,147 bytes, SHA-256
  `84bcc957ddefd4daf8ee0c2e5e5f4a3de1462b5f582e975b4bf40e8981543ece`),
  kept outside this repository.
- Target: `usr/bin/crio` — ELF64 LSB x86-64 PIE, stripped, dynamically linked,
  BuildID sha1 `24de83bc2709560113e4fff746b505d748f208dc`, 74,396,632 bytes,
  SHA-256 `437578a1cde4cfbc16f2979076e2265158025c561d01bea04622ade030a08868`.
  Treated as untrusted input: extracted read-only, never executed, never
  installed; the raw binary and raw JSON report stay outside Git under the
  ignored `target/acceptance/` directory.
- Recorded Go toolchain: `go1.26.10 (Red Hat 1.26.10-1.el10_2)
  X:strictfipsruntime` (pre-1.27, moduledata word-40 family), CGO_ENABLED=1,
  `-buildmode=pie`, `-linkmode=external` with Red Hat hardened/annobin
  ldflags, GOAMD64=v3, main module path `github.com/cri-o/cri-o/cmd/crio`.
- The sibling `usr/bin/pinns` payload is not a Go executable and is out of
  scope.

## Old-version fixture gate (binding, executed before any artifact verdict)

Because the artifact is pre-1.27, the plan's binding gate required an
old-version fixture exercising the word-40 gofunc locator path (gofunc
location and inline decoding) before any real-artifact verdict. That gate was
closed first:

- `just fixtures-oldgo` builds `target/fixtures/app.word40.stripped` from the
  standard fixture source with a pinned pre-installed mise go1.25.7 toolchain
  (`GOTOOLCHAIN=local GOPROXY=off`, no toolchain download; the go1.27.3
  fixtures are untouched).
- End-to-end scan of that fixture: exit 0, all three core walks **decoded**
  (build identity, 3,217 function observations, 9,106 inline occurrences with
  `observations == expected_observations`), and the repeated-inlining control
  (`main.twoInlineCalls`) retains its two distinct `main.inlineOnly` records.
- Regression coverage: `word40_layout_fixture_decodes_inline_evidence`
  (CLI-level) and `word40_fixture_locates_gofunc_and_decodes_inline_trees`
  (locator-level, including the family-gate differential) fail when the
  fixture or the word-40 path regresses.

## Capture and cross-checks

Scanner: release build of golang-symbol 0.1.0, invoked by absolute path with
an empty `PATH` and an execution sentinel set; the sentinel was never
created. Output: one JSON object under `target/acceptance/`, actual exit code
**2** (valid report, partial coverage).

Independent cross-checks:

- The RPM SHA-256 recomputed and matched the controller-recorded digest; the
  extracted binary's SHA-256 matches the report's `artifact.sha256`.
- `go version -m` (developer PATH) on the extracted binary agrees with the
  report on the raw version string, the main module path, all 15 build
  settings, and the absence of dependency and replacement rows. The report's
  `build.main` is null with `build.path` set because this binary records a
  `path` row without a `mod` row; that mirrors the artifact's metadata.
- `go tool nm` cannot read the stripped binary (no symbol section), so
  address-level corroboration used `readelf`/`objdump` on the artifact plus a
  locally built, unstripped reproduction (below).

Coverage achieved on the artifact:

- build identity: **decoded** (expected/visited/observations 1/1/1).
- functions: **partial** — expected = visited = 71,934 rows, 71,932
  observations, exactly two `invalid_function_body` rejections
  (`internal/abi.(*RegArgs).Dump`, `internal/abi.(*StructField).Embedded`).
- inlines: **partial** — all 71,932 attempted trees fully checked, 132,239
  inline occurrences recovered through the word-40 gofunc path, expected
  observation total null via `function_walk_partial` propagation.

## The two rejected spans: demonstrated vendored-parser defect

Both rejected entries are symptoms of one root cause, demonstrated and
reproduced during this validation:

- In externally linked PIE builds, the pclntab header's `textStart` field is
  zero in the file (the runtime fills it by relocation at load time). The
  vendored parser then falls back to the address of the first executable
  section. In such binaries `.init`/`.plt` precede `.text`, and C runtime
  code precedes the Go text inside `.text`, so that fallback is not Go's
  `runtime.text`.
- Ground truth for this artifact: `moduledata.text` (word 22, statically
  present in the file) is `0xdb7fc0`; the derived base was `0xdb7000` (the
  `.init` address). Every reported function entry and inline container VA is
  therefore shifted low by 0xfc0, and the two spans that straddle the
  `.init`/`.plt`/`.text` boundaries were conservatively rejected.
- With the true base, both rejected spans lie inside `.text`
  (`(*RegArgs).Dump` at `0xdb7fc0..0xdb8260`, size 0x2a0;
  `(*StructField).Embedded` at `0xdb8c20..0xdb8c40`, size 0x20), so the
  rejections are not evidence about the artifact — they are the visible edge
  of the base error. The span-validation logic itself (`checked_body`)
  behaved as designed: it refused to certify spans that do not resolve to
  file-backed executable text.
- Reproduction independent of Red Hat tooling: building the standard
  development fixture with stock go1.27.3, `CGO_ENABLED=1`,
  `-linkmode=external -buildmode=pie` shows the same behavior
  (`runtime.text` = 0x780 per `go tool nm` while the derived base is the
  `.init` address 0x374), confirming a generic external-linking condition,
  not an artifact-specific quirk.
- Blast radius: function and inline evidence *identity* is unaffected —
  names, body sizes, file/line metadata, inline records and all counts are
  entryOff- and funcdata-relative. The absolute `entry` and `container_entry`
  VAs in this report are shifted low by 0xfc0 for this artifact, which
  matters when correlating them with other tools' addresses.

Per the task's execution rules the owning feature of this defect is the
vendored `unstrip` pclntab (its published `textStart` fallback), so it was
not patched here; the correction is escalated to the owner for a vendor-patch
decision. No parser fork was introduced and the evidence contract was not
weakened.

## Limitations

- Partial recovery stays partial: this run does not satisfy full
  first-artifact acceptance while the function walk is partial, and exit 2 is
  the honest outcome. The partiality is fully explained by the vendored-parser
  defect above.
- Entry and container VAs in this report are shifted low by 0xfc0 (see
  above) until the vendor decision lands.
- The word-40 gofunc path is fixture-executed on go1.25.7 and the word-43
  path on go1.27.3; intermediate 1.20-1.26 releases are layout-verified only.
- `go tool nm` cannot corroborate symbols on this stripped binary; the
  oracles used were `go version -m`, `readelf`, `objdump`, and the local
  unstripped reproduction.
- Decoded coverage describes traversal of recorded metadata, not
  source-version proof, runtime reachability, vulnerability affectedness, or
  proven absence.

## Commands

Extraction (read-only, never executed; RPM stays in `~/Downloads`):

```sh
rpm2cpio <rpm> | cpio -idm --no-absolute-filenames './usr/bin/crio'
```

Scan (absolute Cargo path, empty `PATH`, sentinel set, never executes the
target):

```sh
cargo build --locked --release
env -i GOLANG_SYMBOL_EXECUTION_SENTINEL=<sentinel> \
  <repo>/target/release/golang-symbol <extracted>/usr/bin/crio \
  > target/acceptance/crio-first-artifact.json
```

Oracles (developer `PATH`): `go version -m`, `go tool nm` (stripped: no
symbol section), `readelf -SW/-lW/-n`, `objdump -d`.

Old-version fixture gate:

```sh
just fixtures-oldgo   # pinned mise go1.25.7; never downloads a toolchain
```
