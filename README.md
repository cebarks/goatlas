# golang-symbol

Offline inventory of recorded Go build identity and retained standalone function
and inline-call evidence in a local executable. The initial validated profile is
ordinary gc-compiled Linux ELF x86-64, including binaries
stripped with `-s -w` and PIE builds. This slice reports modules, replacements,
build settings, retained standalone implementations (including
reflection-only methods) and inline occurrences recovered from
`FUNCDATA_InlTree` metadata.

## Use

Build with a current Rust toolchain (Rust 2024 edition support required):

```sh
cargo build --locked --release
./target/release/golang-symbol /path/to/executable
```

Pass exactly one file path. `--help` and `--version` print plain text, not inventory
reports. A scan emits a single JSON object and newline to stdout; diagnostics go
to stderr.

Exit codes:

- **0**: all three core metadata walks decoded (build identity, standalone
  functions, inline occurrences).
- **2**: valid report with partial or unavailable coverage.
- **1**: no valid report, such as an unreadable, non-regular or oversized input,
  incorrect invocation, or output failure.

Inputs must be regular files of at most **1 GiB (1,073,741,824 bytes)**. The scanner
uses one bounded input buffer, rejects growth past the limit rather than inspecting
a prefix, and hashes the same bytes passed to the parser. Avoid modifying files
during scanning; the digest identifies the inspected bytes, not a filesystem
snapshot or a later file state.

## Report and limitations

Schema version 1 includes producer versions, SHA-256, byte size, recognized
container/architecture, raw Go version, recorded main/dependency modules,
replacement relationships, settings, and one observation per retained standalone
function: raw symbol name, entry address (`0x{address:016x}`), body size in
bytes, optional package path, source function name, module attribution, and
optional file/line metadata. Arrays are sorted deterministically
without deduplicating records; functions sort by entry then raw name.
Unknown/development versions and embedded paths
and `vcs.time` remain unchanged. The scanning host's input path and analysis time
are not added. Reports may contain sensitive embedded provenance.

The parser is pinned to `unstrip =1.2.0` with the local `strict-buildinfo-v1`,
`strict-inline-v1` and `strict-textstart-v1` checked patches (combined
producer identity
`strict-buildinfo-v1+strict-inline-v1+strict-textstart-v1`). Their upstream
checksum, revision, licenses, exact patch scope and replacement criteria are
documented in
[vendor/unstrip/PATCHES.md](vendor/unstrip/PATCHES.md).
Unsupported targets and unrecovered metadata yield unavailable coverage with
issues, not a clean empty inventory. When a raw Go version survives but module
metadata is missing or malformed, build-info coverage is partial: the raw version
and any validated module/path/setting evidence survive, with explicit issues.
Version-only recovery has a null build identity, never a decoded module walk.
Build-info extraction still depends on the parser opening Go metadata, so damaged
prerequisite metadata can prevent it; function recovery then proceeds
independently of the missing build identity. Parser/compiler coverage is limited;
this patch is not a crash-proof parsing claim, and recognizing a container does
not establish support for its Go metadata.

Function evidence comes from the Go runtime function table (pclntab), not from
type/method lists. A row is reported as an implementation only when its checked,
nonzero entry/size span resolves to file-backed executable text through link-time
virtual addresses; PIE entries are validated the same way, without file-offset
or runtime load-base assumptions. **Retained code is not runtime reachability**:
the Go linker retains implementations conservatively (notably for reflection),
and missing names are not proof of absence. Module attribution is a conservative,
cached package-path-prefix hint over the original recorded modules: it stays
null for escaped, generated or ambiguous symbol spellings, vendor namespaces and
conflicting module versions, and a null attribution does not erase the underlying
code-presence evidence. The walker's returned row count is cross-checked against
the table header count; skipped malformed rows, unreadable names and unverified
body spans yield partial coverage with explicit issues, never a silently
shortened "decoded" walk.
For externally linked PIE builds (CGO with `-linkmode=external`), the pclntab
header's `textStart` is zero in the file — the runtime fills it by relocation —
so the base is resolved from the statically present `moduledata.text` (the
runtime's own base for functab entryOff arithmetic) via the
`strict-textstart-v1` vendored patch; a non-zero header value is used
verbatim and a missing or incredible moduledata candidate keeps the
`.text` section fallback. A permanent externally linked PIE fixture pins
correct entry VAs against the moduledata base. Details and the measured
impact on the first Red Hat artifact are recorded in
[docs/validation/first-artifact.md](docs/validation/first-artifact.md).

Decoded coverage describes traversal of recorded metadata, not source-version
proof, runtime reachability, vulnerability affectedness, or proven absence.
A replacement is separate evidence, not a substitution of the original release.
This tool makes no binary-wide completeness claim. Standalone function evidence
is independent of optional build identity and module attribution.
The first representative Red Hat artifact run is recorded (partially closed,
with a pending vendored-parser decision) in
[docs/validation/first-artifact.md](docs/validation/first-artifact.md); full
first-artifact acceptance remains pending.

Inline evidence comes from the per-function `FUNCDATA_InlTree` arrays addressed
through the moduledata `gofunc` base. Decoding is gated to the go1.20+ table
magic and the documented 16-byte `runtime.inlinedCall` release families
1.20-1.26 plus source-inspected 1.27 (witnessed on go1.27.3 fixtures); other
magics, development/unknown versions and future releases stay unavailable
rather than guessed. Each tree's expected entry count is derived from a
completed `PCDATA_InlTreeIndex` scan, and decoding preserves validated
prefixes, per-record indexes, raw parent-PCs and positive start lines while
mapping every decoder failure to a stable `inline_`-prefixed issue with the
containing function as context. A genuine empty tree is complete with zero
expected entries; a truncated, contradictory or unlocatable tree is never
reported as a clean empty success, and unresolved callee names are indexed
occurrences plus a core `unresolved_inline_name` gap. Repeated records inside
one containing function are distinct evidence rows and never collapse. The
gofunc base is located by a minimal version-gated reader in the application
because the vendored moduledata locator predates the go1.27 field moves; that
reader shares the locator's replacement fate when upstream handles go1.27.
Both of its layout paths are fixture-executed — the pre-1.27 word-40 path on a
go1.25.7 build and the go1.27 word-43 path on go1.27.3 builds — but the
intermediate 1.20-1.26 releases are layout-verified only, not fixture-executed.
Inline coverage counts expected functions / fully checked trees and inline
records; expected observation totals stay null when any function's tree total
cannot be established, and a partial function walk keeps the inline walk
partial because uninspected functions' trees were never examined.

Scanning is local and in-process: no target execution, Go invocation, helper
processes, network lookups, downloads, or external symbol resolvers. Go is needed
only for development fixtures, not to run the scanner.

## Development

Install Rust, `just`, and a local Go compiler. Missing Go or fixtures is a failing
prerequisite, never a skipped test. No external Go modules are needed:

```sh
just fixtures-base  # builds unstripped/stripped Linux amd64 fixtures; never runs them
just fixtures-pie   # builds the stripped PIE fixture variant
just fixtures-noinline  # builds the stripped fixture with package inlining disabled
just fixtures-oldgo  # builds the stripped pre-1.27 (word-40) fixture with pinned mise go1.25.7
just fixtures-extpie  # builds the stripped externally linked PIE fixture (CGO, gcc required)
just fixtures       # fixtures-base + fixtures-pie + fixtures-noinline + fixtures-oldgo + fixtures-extpie
just test           # fixtures, cargo test --locked
just patch-check    # full vendor formatting, locked library tests, strict library Clippy
just check          # test, patch-check, application formatting and strict Clippy
```

Fixture recipes require a working local `go version` before changing outputs.
The word-40 fixture additionally requires a pre-installed `mise` go1.25.7
(`mise where go@1.25.7` must succeed; recipes never download toolchains).
All Go commands use `GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off`; builds use
`CGO_ENABLED=0`, readonly modules, trimpath, and disabled VCS stamping.
Compiler diagnostics and the compiler version are recorded under `target/fixtures/`.
Tests compare build identity with read-only `go version -m`; they invoke the scanner
by its absolute Cargo path with empty PATH and never execute the target.
