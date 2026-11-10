//! Inlined-call recovery from Go's `FUNCDATA_InlTree`.
//!
//! Decodes the per-function `FUNCDATA_InlTree` array into a flat
//! `Vec<InlinedCall>`, one entry per inlined call site recorded by the
//! Go compiler.
//!
//! # Supported Go envelope
//!
//! The 16-byte `runtime.inlinedCall` record has been format-stable
//! since Go 1.20 (the version that added `startLine`). Same layout,
//! same `FUNCDATA_InlTree=3`, same `PCDATA_InlTreeIndex=2` through
//! Go 1.26. A returned error on an unrecognized funcdata layout is
//! the honest fallback if a future Go release drifts. Garble's
//! `entryoff` XOR rewrite does not touch the inline-tree FUNCDATA
//! section, so this decoder works on garble-obfuscated binaries
//! unchanged.
//!
//! # On-disk encoding
//!
//! Packed 16-byte struct per entry:
//!
//! ```text
//!   off  size  field
//!   0    1     funcID          u8
//!   1    3     _pad            [3]u8
//!   4    4     nameOff         i32   (offset into pclntab funcname table)
//!   8    4     parentPc        i32   (PC offset within the physical fn,
//!                                     of the call site that triggered the
//!                                     inline; -1 for top-level entries)
//!   12   4     startLine       i32
//! ```
//!
//! The array lives in the `gofunc` blob (a contiguous mapped region
//! described by moduledata). The per-function entry point is funcdata
//! index 3 (`FUNCDATA_InlTree`), stored as a u32 offset relative to
//! `gofunc`. A value of `u32::MAX` means "no inline tree for this
//! function". The decoder returns `Ok(vec![])` in that case.
//!
//! Safety: every decoded `parent_pc` is asserted to lie strictly inside
//! the host function's PC range. This catches the most common
//! garble-style corruption (a truncated/reused inltree blob from another
//! function), and is the headline invariant we check during the Day-2
//! probe.

use crate::error::Error;
use crate::gobin::GoBinary;
use crate::pclntab::{FuncEntry, Pclntab};
use crate::Result;

/// One inlined-call record from `FUNCDATA_InlTree`.
#[derive(Debug, Clone, Copy)]
pub struct InlinedCall {
    pub func_id: u8,
    pub name_off: i32,
    pub parent_pc: i32,
    pub start_line: i32,
}

/// Typed reason an inline-tree decode could not be established as complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineIssueKind {
    TruncatedFunction,
    InvalidPcData,
    EntryLimit,
    AddressOverflow,
    TruncatedTree,
    InvalidParentPc,
    UnsupportedLayout,
    MissingGofunc,
}

/// One observed decoder problem, with a human-readable explanation.
#[derive(Debug, Clone)]
pub struct InlineDecodeIssue {
    pub kind: InlineIssueKind,
    pub message: String,
}

/// Checked decode result for one function's inline tree.
///
/// `complete` requires a known expected entry count, exactly that many
/// validated entries and no suppressed errors. A genuine empty tree is
/// `complete` with `expected_entries == Some(0)`; a truncated or unknown
/// tree is never complete.
#[derive(Debug, Clone)]
pub struct InlineTreeDecode {
    pub entries: Vec<InlinedCall>,
    pub expected_entries: Option<usize>,
    pub complete: bool,
    pub issues: Vec<InlineDecodeIssue>,
}

/// Number of funcdata pointer slots required before `FUNCDATA_InlTree`
/// is addressable in a function's funcdata array.
const FUNCDATA_INLTREE: usize = 3;
/// PCDATA index whose stream yields the current inline-tree index at each
/// PC. The array length we need is `max(stream) + 1`.
const PCDATA_INLTREE_INDEX: usize = 2;

/// Hard cap on entries per function. A real inline tree fits in a handful
/// of KB; 65_536 entries (1 MiB) is a very generous structural ceiling
/// and protects against a garbage length being inferred from a corrupt
/// next-pointer. Exceeding it is reported as [`InlineIssueKind::EntryLimit`]
/// instead of silently truncating the walk.
const MAX_INLINE_ENTRIES: usize = 65_536;

/// Guard against a non-terminating pcvalue stream. Exhausting the budget
/// is an explicit decode issue, never a silent best-effort maximum.
const SCAN_BUDGET: usize = 1_000_000;

/// Decode the `FUNCDATA_InlTree` array for a single function.
///
/// Decode the `FUNCDATA_InlTree` array for a single function.
///
/// Compatibility wrapper over [`decode_inline_tree_checked`]: it returns
/// `Ok(entries)` only for complete results (including a genuine empty
/// tree) and an error for any incomplete outcome, so a truncated or
/// unlocatable tree is never reported as a clean empty success.
pub fn decode_inline_tree(
    bin: &GoBinary,
    pcln: &Pclntab,
    func: &FuncEntry,
) -> Result<Vec<InlinedCall>> {
    let decoded = decode_inline_tree_checked(bin, pcln, func);
    if decoded.complete {
        return Ok(decoded.entries);
    }
    let detail = decoded
        .issues
        .first()
        .map(|issue| issue.message.clone())
        .unwrap_or_else(|| "no decoder issue recorded".to_string());
    Err(Error::ModuleData(format!(
        "inline tree for {} could not be decoded completely: {detail}",
        func.name
    )))
}

/// Decode the `FUNCDATA_InlTree` array for a single function with explicit
/// checked outcomes. See [`InlineTreeDecode`] for the completeness contract;
/// the legacy [`decode_inline_tree`] wraps this and only returns `Ok` for
/// complete results.
///
/// The decode is a chain of checked prerequisites, each of which either
/// establishes part of the outcome or fails with a typed issue:
///
/// 1. Only the go1.20+ table layout (`magic 0xfffffff1`) is decodable; any
///    other layout is [`InlineIssueKind::UnsupportedLayout`].
/// 2. The 44-byte `_func` header must be fully readable, or the result is
///    [`InlineIssueKind::TruncatedFunction`] with no expected count.
/// 3. The expected entry count is derived only from a completed
///    PCDATA_InlTreeIndex scan; a malformed stream is
///    [`InlineIssueKind::InvalidPcData`] and leaves the count unknown.
/// 4. The tree location comes from the `FUNCDATA_InlTree` slot; a short
///    slot read is `TruncatedFunction` but retains the count established
///    in step 3. An absent stream/slot (offset 0 or the `u32::MAX`
///    sentinel) is a genuine empty tree when the index scan agrees.
/// 5. A tree the index scan counted but the funcdata cannot address (or
///    the reverse contradiction) is `UnsupportedLayout`, never a decoded
///    zero.
/// 6. A present tree requires a nonzero gofunc base ([
///    `InlineIssueKind::MissingGofunc`]) and checked address arithmetic
///    ([`InlineIssueKind::AddressOverflow`]).
/// 7. Entries are decoded up to the expected count, bounded by
///    [`MAX_INLINE_ENTRIES`] ([`InlineIssueKind::EntryLimit`] makes the cap
///    observable). An unreadable record is
///    [`InlineIssueKind::TruncatedTree`]; an invalid `parent_pc` is
///    [`InlineIssueKind::InvalidParentPc`]. In both cases the validated
///    prefix is retained and the bad record is never returned.
pub fn decode_inline_tree_checked(
    bin: &GoBinary,
    pcln: &Pclntab,
    func: &FuncEntry,
) -> InlineTreeDecode {
    if pcln.magic() != crate::pclntab::MAGIC_1_20 {
        return incomplete(
            InlineIssueKind::UnsupportedLayout,
            format!(
                "pclntab magic 0x{:08x} does not carry the go1.20+ inline layout",
                pcln.magic()
            ),
            None,
        );
    }
    let data = pcln.data();
    let le = pcln.little_endian();

    // Layout of `_func` (Go 1.20+, 40-byte fixed header + packed tail):
    //   off=0  entryOff       u32
    //   off=4  nameOff        i32
    //   off=8  args           i32
    //   off=12 deferreturn    u32
    //   off=16 pcsp           u32
    //   off=20 pcfile         u32
    //   off=24 pcln           u32
    //   off=28 npcdata        u32
    //   off=32 cuOffset       u32
    //   off=36 startLine      i32
    //   off=40 funcID         u8
    //   off=41 flag           u8
    //   off=42 _pad           u8
    //   off=43 nfuncdata      u8
    //   off=44 [npcdata]u32   pcdata table
    //   off=44+npcdata*4 [nfuncdata]u32 funcdata table
    let Some(func_begin) = pcln
        .funcdata_off()
        .checked_add(func.func_off)
        .filter(|begin| begin.checked_add(44).is_some_and(|end| end <= data.len()))
    else {
        return incomplete(
            InlineIssueKind::TruncatedFunction,
            format!(
                "_func header for {} at table offset {:#x} is not fully readable",
                func.name, func.func_off
            ),
            None,
        );
    };
    let npcdata = read_u32(data, func_begin + 28, le).unwrap_or(0);
    let nfuncdata = data[func_begin + 43] as usize;
    let pcdata_addressable = npcdata as usize > PCDATA_INLTREE_INDEX;
    let funcdata_addressable = nfuncdata > FUNCDATA_INLTREE;

    // Resolve the expected entry count first by scanning the
    // PCDATA_InlTreeIndex pcvalue stream for its maximum index across the
    // function's PC range. The inline-tree subarray for this function
    // holds (max_idx + 1) entries. Only a completed scan establishes a
    // count; anything else leaves it unknown.
    let expected: Option<usize> = if pcdata_addressable {
        let pcdata_off = func_begin + 44;
        let pcdata_inltree_off = match read_u32(data, pcdata_off + PCDATA_INLTREE_INDEX * 4, le) {
            Ok(offset) => offset,
            Err(_) => {
                return incomplete(
                    InlineIssueKind::TruncatedFunction,
                    format!(
                        "pcdata table for {} is truncated before the inline index slot",
                        func.name
                    ),
                    None,
                );
            }
        };
        if pcdata_inltree_off == 0 {
            // No inline-index stream: the runtime never attributes an
            // inlined frame to any PC of this function.
            Some(0)
        } else {
            match max_pcvalue(
                pcln_pctab(pcln),
                pcdata_inltree_off as usize,
                func.size,
                pcln.quantum(),
            ) {
                Ok(max_idx) if max_idx < 0 => Some(0),
                Ok(max_idx) => Some(max_idx as usize + 1),
                Err(issue) => {
                    return InlineTreeDecode {
                        entries: vec![],
                        expected_entries: None,
                        complete: false,
                        issues: vec![issue],
                    };
                }
            }
        }
    } else {
        None
    };

    // Locate the tree through the FUNCDATA_InlTree slot. A short slot read
    // is a truncated function, but the count established above is retained.
    let slot: Option<u32> = if funcdata_addressable {
        let funcdata_arr_off = (npcdata as usize)
            .checked_mul(4)
            .and_then(|scaled| func_begin.checked_add(44)?.checked_add(scaled))
            .filter(|off| {
                off.checked_add(FUNCDATA_INLTREE * 4 + 4)
                    .is_some_and(|end| end <= data.len())
            });
        match funcdata_arr_off.and_then(|off| read_u32(data, off + FUNCDATA_INLTREE * 4, le).ok()) {
            Some(slot) => Some(slot),
            None => {
                return incomplete(
                    InlineIssueKind::TruncatedFunction,
                    format!(
                        "funcdata table for {} is truncated before the inline tree slot",
                        func.name
                    ),
                    expected,
                );
            }
        }
    } else {
        None
    };

    // An absent stream/slot combination is how the runtime encodes "no
    // inline tree"; it is a genuine zero only when nothing contradicts it.
    let tree_absent = slot.is_none_or(|slot| slot == u32::MAX);
    if tree_absent {
        return match expected {
            Some(count) if count > 0 => incomplete(
                InlineIssueKind::UnsupportedLayout,
                format!(
                    "pcdata counted {count} inline entries for {} but no inline \
                     tree is addressable",
                    func.name
                ),
                expected,
            ),
            _ => InlineTreeDecode {
                entries: vec![],
                expected_entries: Some(0),
                complete: true,
                issues: vec![],
            },
        };
    }
    let slot = slot.unwrap_or_default();

    let Some(expected) = expected else {
        return incomplete(
            InlineIssueKind::UnsupportedLayout,
            format!(
                "inline tree for {} is present but no index stream establishes \
                 its length",
                func.name
            ),
            None,
        );
    };
    if expected == 0 {
        // The index scan proved no PC of this function is attributed to an
        // inlined frame, so the tree carries no records.
        return InlineTreeDecode {
            entries: vec![],
            expected_entries: Some(0),
            complete: true,
            issues: vec![],
        };
    }

    let Some(gofunc) = pcln.gofunc().filter(|base| *base != 0) else {
        return incomplete(
            InlineIssueKind::MissingGofunc,
            format!(
                "inline tree for {} requires a nonzero gofunc base; \
                 call Pclntab::with_gofunc(moduledata.gofunc) first",
                func.name
            ),
            Some(expected),
        );
    };
    let Some(inltree_addr) = gofunc.checked_add(slot as u64) else {
        return incomplete(
            InlineIssueKind::AddressOverflow,
            format!(
                "gofunc 0x{gofunc:x} + inline tree offset 0x{slot:x} overflows \
                 the address space"
            ),
            Some(expected),
        );
    };

    // Decode the bounded, validated prefix. A read that fails to map or a
    // record whose parent_pc falls outside the host function stops the
    // walk; the validated prefix is retained and the bad record is never
    // returned. The absolute entry cap stays observable as an issue.
    let mut issues = Vec::new();
    let decode_limit = expected.min(MAX_INLINE_ENTRIES);
    let mut entries = Vec::with_capacity(decode_limit);
    for i in 0..decode_limit {
        let Some(entry_addr) = inltree_addr.checked_add((i as u64) * 16) else {
            issues.push(InlineDecodeIssue {
                kind: InlineIssueKind::AddressOverflow,
                message: format!(
                    "inline tree address for {} overflows at entry {i}",
                    func.name
                ),
            });
            break;
        };
        let Some(bytes) = bin.read_at_addr(entry_addr, 16) else {
            issues.push(InlineDecodeIssue {
                kind: InlineIssueKind::TruncatedTree,
                message: format!(
                    "inline tree for {} is not fully mapped at entry {i}",
                    func.name
                ),
            });
            break;
        };
        let call = InlinedCall {
            func_id: bytes[0],
            name_off: i32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            parent_pc: i32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            start_line: i32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        };
        // parent_pc == -1 is the runtime's marker for a top-level inline
        // (no parent call site); any other negative value or an offset past
        // func.size is corruption (mis-resolved funcdata, recycled blob)
        // and taints the record.
        if call.parent_pc != -1 && (call.parent_pc < 0 || call.parent_pc as u64 >= func.size) {
            issues.push(InlineDecodeIssue {
                kind: InlineIssueKind::InvalidParentPc,
                message: format!(
                    "inline tree for {} has parent_pc {} outside [0, 0x{:x}) at \
                     index {i}",
                    func.name, call.parent_pc, func.size
                ),
            });
            break;
        }
        entries.push(call);
    }
    if expected > MAX_INLINE_ENTRIES {
        issues.push(InlineDecodeIssue {
            kind: InlineIssueKind::EntryLimit,
            message: format!(
                "inline tree for {} claims {expected} entries; decoding is capped \
                 at {MAX_INLINE_ENTRIES}",
                func.name
            ),
        });
    }
    let complete = issues.is_empty() && entries.len() == expected;
    InlineTreeDecode {
        entries,
        expected_entries: Some(expected),
        complete,
        issues,
    }
}

fn incomplete(kind: InlineIssueKind, message: String, expected: Option<usize>) -> InlineTreeDecode {
    InlineTreeDecode {
        entries: vec![],
        expected_entries: expected,
        complete: false,
        issues: vec![InlineDecodeIssue { kind, message }],
    }
}

/// Resolve a leaf entry's `name_off` to a function name. Returns `None`
/// if the offset is zero, out of range, or resolves to an empty string.
/// Those are the three "unresolved" buckets the probe measurement tool reports.
pub fn resolve_leaf_name(pcln: &Pclntab, leaf: &InlinedCall) -> Option<String> {
    if leaf.name_off == 0 {
        return None;
    }
    if leaf.name_off < 0 {
        return None;
    }
    let off = leaf.name_off as usize;
    let abs = pcln.funcname_off().checked_add(off)?;
    if abs >= pcln.data().len() {
        return None;
    }
    let name = pcln.read_name_at(off).ok()?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Slice of pclntab `data()` starting at the pctab base. Convenience for
/// passing to [`max_pcvalue`].
fn pcln_pctab<'a>(pcln: &'a Pclntab<'_>) -> &'a [u8] {
    let off = pcln.pctab_off();
    let data = pcln.data();
    if off >= data.len() {
        &[]
    } else {
        &data[off..]
    }
}

/// Walk a pc-value stream and return the maximum `value` it ever produces
/// while `pc < pc_limit`, using the runtime's rules: value deltas are
/// zig-zag varints, pc deltas are varints scaled by `quantum`, a zero
/// value-delta after the first record terminates the stream, and reaching
/// `pc_limit` completes it. `Ok(-1)` means the stream never records a
/// value. `Err` means the stream is malformed — truncated or overflowing
/// varints, an out-of-range start offset, arithmetic overflow, an exhausted
/// scan budget, or exhaustion without a proven terminator — and those are
/// never silently folded into a maximum.
fn max_pcvalue(
    table: &[u8],
    start: usize,
    pc_limit: u64,
    quantum: u8,
) -> std::result::Result<i64, InlineDecodeIssue> {
    let invalid = |message: String| {
        Err(InlineDecodeIssue {
            kind: InlineIssueKind::InvalidPcData,
            message,
        })
    };
    if start >= table.len() {
        return invalid(format!(
            "pcdata offset {start} is out of range for the {len}-byte pctab",
            len = table.len()
        ));
    }
    let overflow = |message: String| {
        Err(InlineDecodeIssue {
            kind: InlineIssueKind::AddressOverflow,
            message,
        })
    };
    let mut value: i64 = -1;
    let mut pc: u64 = 0;
    let mut pos = start;
    let mut max_seen: i64 = -1;
    let mut iter_guard = 0;
    while pos < table.len() {
        iter_guard += 1;
        if iter_guard > SCAN_BUDGET {
            return invalid(format!(
                "pcdata scan exceeded {SCAN_BUDGET} records without terminating"
            ));
        }
        let (uval, n) = match read_varint(&table[pos..]) {
            Some(v) => v,
            None => {
                return invalid(format!("truncated pcdata value varint at byte {pos}"));
            }
        };
        // A zero value-delta after the first record terminates the stream
        // (Go runtime convention; the leading record is always non-zero).
        if uval == 0 && pc != 0 {
            return Ok(max_seen);
        }
        pos += n;
        let delta = zig_zag(uval);
        value = match value.checked_add(delta) {
            Some(v) => v,
            None => {
                return overflow(format!("pcdata value accumulator overflows at byte {pos}"));
            }
        };
        let (pc_delta, n2) = match read_varint(&table[pos..]) {
            Some(v) => v,
            None => {
                return invalid(format!("truncated pcdata pc varint at byte {pos}"));
            }
        };
        pos += n2;
        let step = match (pc_delta).checked_mul(u64::from(quantum)) {
            Some(v) => v,
            None => {
                return overflow(format!("pcdata pc delta overflows at byte {pos}"));
            }
        };
        pc = match pc.checked_add(step) {
            Some(v) => v,
            None => {
                return overflow(format!("pcdata pc accumulator overflows at byte {pos}"));
            }
        };
        if value > max_seen {
            max_seen = value;
        }
        if pc >= pc_limit {
            return Ok(max_seen);
        }
    }
    invalid("pcdata stream exhausted without a terminator or pc bound".to_string())
}

fn zig_zag(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    const MAX_BYTES: usize = 10;
    let mut result: u64 = 0;
    let mut shift = 0;
    for (i, &b) in buf.iter().take(MAX_BYTES).enumerate() {
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

// ---------------------------------------------------------------------------
//
// Inlined call graph.
//
// `inline_callgraph` walks every physical function's `FUNCDATA_InlTree` and
// stitches the inlined-call records into a directed graph. v1.1 ships
// inlined edges only; pcdata-derived direct-call edges land in v1.2 (the
// `EdgeKind::Direct` variant is reserved now so its addition does not
// break callers).
//
// Edge semantics: `Edge { from, to, call_site, kind: Inlined }` means
// "the physical function at `from` inlined a call at `call_site` whose
// callee is the node at `to`". `call_site` is the link-time VA of the
// inlined call within the physical caller's text.
//
// Node identity:
//   - `NodeKind::Physical` nodes use the function's real entry PC as
//     their `addr`. Every function in pclntab is emitted exactly once.
//   - `NodeKind::AnonymousInline` nodes have no name (the compiler
//     emitted `name_off == 0`, typically because `-tiny` zeroed it).
//     They carry their parent's PC and their inline-tree start line,
//     and their `addr` is a synthetic, deterministic ID drawn from the
//     anonymous-ID space (high bit set) so they never collide with a
//     real text VA. Two anonymous-inline records with the same
//     `(parent_addr, parent_pc, start_line)` collapse to one node.
//
// Cycles: inline trees themselves are acyclic by Go compiler invariant
// (the compiler will not inline a function into itself). Across
// functions, an A->B inlined edge and a B->A inlined edge can both
// exist when both directions inlined a peer call site; the graph
// allows that. No cycle-breaking is performed; consumers walking the
// graph are responsible for their own visit tracking if they need it.

/// A node in the inlined call graph: either a physical function from
/// pclntab or a synthetic anonymous-inline placeholder for an inlined
/// call site whose callee name was stripped (`name_off == 0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Real text VA for `Physical` nodes; a synthetic high-bit-tagged
    /// ID for `AnonymousInline` nodes (see [`Self::is_anonymous_addr`]).
    pub addr: u64,
    /// Function name. Empty string for anonymous-inline nodes.
    pub name: String,
    pub kind: NodeKind,
}

impl Node {
    /// Synthetic anonymous-inline addresses have the top bit set so they
    /// cannot collide with a 63-bit text VA produced by any real Go
    /// binary on a 64-bit target.
    const ANONYMOUS_ADDR_BIT: u64 = 1 << 63;

    /// True if `addr` was synthesized by the anonymous-inline ID
    /// generator rather than read out of pclntab.
    pub fn is_anonymous_addr(addr: u64) -> bool {
        addr & Self::ANONYMOUS_ADDR_BIT != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// Real function from pclntab. `addr` is its link-time entry PC.
    Physical,
    /// Inlined callee whose name was stripped from the inline tree.
    /// Topology is fully recovered; only the name is gone.
    AnonymousInline {
        /// Entry PC of the physical function the inlined record was
        /// found in. Anchors the anonymous node to a real call site.
        parent: u64,
        /// `startLine` field from the inlined-call record.
        start_line: i32,
    },
}

/// An edge in the inlined call graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// `addr` of the caller node.
    pub from: u64,
    /// `addr` of the callee node.
    pub to: u64,
    /// Link-time VA of the call site within the caller's text.
    pub call_site: u64,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// The caller inlined the callee at `call_site`.
    Inlined,
    /// Reserved for v1.2 pcdata-derived direct-call edges. Not
    /// emitted by [`inline_callgraph`] today.
    Direct,
}

/// The inlined call graph for a Go binary.
///
/// Built by [`inline_callgraph`]; see the function's docs for the
/// Go envelope this is witnessed on.
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl CallGraph {
    /// Look up a node by `addr`. O(n); for callers walking the graph
    /// in bulk, build a `HashMap<u64, &Node>` once.
    pub fn node(&self, addr: u64) -> Option<&Node> {
        self.nodes.iter().find(|n| n.addr == addr)
    }

    /// All outgoing edges from `from`. O(n).
    pub fn edges_from(&self, from: u64) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| e.from == from)
    }

    /// All incoming edges to `to`. O(n).
    pub fn edges_to(&self, to: u64) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |e| e.to == to)
    }
}

/// Build the inlined call graph for `bin`.
///
/// For every physical function recovered from `pcln`, decodes its
/// `FUNCDATA_InlTree` and emits one `Edge` per inlined call record
/// (those with `parent_pc != -1`; the `-1` root entry represents
/// the physical function itself and produces no edge). Inlined
/// callees with a resolvable name dedupe across the binary (one
/// `Node` per distinct name); anonymous-inline records dedupe within
/// their parent function by `(parent, parent_pc, start_line)`.
///
/// The `runtime.inlinedCall` layout has been stable since Go 1.20
/// and runs unchanged through Go 1.26. Garble-obfuscated binaries
/// produce a graph with obfuscated callee names; structure is
/// unchanged.
pub fn inline_callgraph(bin: &GoBinary, pcln: &Pclntab) -> Result<CallGraph> {
    use std::collections::HashMap;

    let funcs = pcln.functions_with_offsets()?;

    let mut graph = CallGraph::default();
    // Track named-callee nodes by name so we emit one per distinct
    // inlined callee, not one per call site.
    let mut named_nodes: HashMap<String, u64> = HashMap::new();
    // Track anonymous-inline nodes by their identity tuple within
    // their parent function so the same inlined record across
    // multiple PCs collapses to one node.
    let mut anon_nodes: HashMap<(u64, i32, i32), u64> = HashMap::new();
    // Dedupe edges by full identity so a parent function recorded
    // twice in the corpus does not produce duplicate edges.
    let mut edge_keys: std::collections::HashSet<(u64, u64, u64)> =
        std::collections::HashSet::new();
    // Counter for synthesizing anonymous-inline addresses.
    let mut next_anon: u64 = 1;

    // Emit a Physical node for every real function. Some of these
    // will have no inline tree at all (the ~50% of Go functions
    // that inline nothing); they remain in the graph as isolated
    // nodes so consumers can iterate every function uniformly.
    for f in &funcs {
        graph.nodes.push(Node {
            addr: f.address,
            name: f.name.clone(),
            kind: NodeKind::Physical,
        });
        named_nodes.insert(f.name.clone(), f.address);
    }

    for f in &funcs {
        let entries = match decode_inline_tree(bin, pcln, f) {
            Ok(v) => v,
            // A single function with a corrupt inline tree must not
            // poison the whole graph. Skip it; its physical node is
            // already in the graph.
            Err(_) => continue,
        };

        for entry in &entries {
            // parent_pc == -1 marks the synthetic root entry that
            // represents the physical function itself, not an
            // inlined call. No edge to emit.
            if entry.parent_pc == -1 {
                continue;
            }

            let call_site = f.address.wrapping_add(entry.parent_pc as u64);
            let resolved = resolve_leaf_name(pcln, entry);

            let callee_addr = match resolved {
                Some(name) => *named_nodes.entry(name.clone()).or_insert_with(|| {
                    // Inlined callee whose name we know but which has
                    // no physical body in this binary (the compiler
                    // fully inlined it away). Allocate a synthetic
                    // address in the anonymous space and emit a node
                    // labelled with the recovered name. The node's
                    // kind stays AnonymousInline because we have no
                    // physical entry PC to back it; the name is the
                    // identity, the addr is just a handle.
                    let addr = Node::ANONYMOUS_ADDR_BIT | next_anon;
                    next_anon += 1;
                    graph.nodes.push(Node {
                        addr,
                        name,
                        kind: NodeKind::AnonymousInline {
                            parent: f.address,
                            start_line: entry.start_line,
                        },
                    });
                    addr
                }),
                None => {
                    // True anonymous-inline (name_off == 0). Dedupe
                    // within the parent by (parent, parent_pc,
                    // start_line); different parent_pcs within the
                    // same function are distinct inlined call sites
                    // and get distinct nodes.
                    let key = (f.address, entry.parent_pc, entry.start_line);
                    *anon_nodes.entry(key).or_insert_with(|| {
                        let addr = Node::ANONYMOUS_ADDR_BIT | next_anon;
                        next_anon += 1;
                        graph.nodes.push(Node {
                            addr,
                            name: String::new(),
                            kind: NodeKind::AnonymousInline {
                                parent: f.address,
                                start_line: entry.start_line,
                            },
                        });
                        addr
                    })
                }
            };

            let ek = (f.address, callee_addr, call_site);
            if edge_keys.insert(ek) {
                graph.edges.push(Edge {
                    from: f.address,
                    to: callee_addr,
                    call_site,
                    kind: EdgeKind::Inlined,
                });
            }
        }
    }

    Ok(graph)
}

fn read_u32(buf: &[u8], off: usize, little_endian: bool) -> Result<u32> {
    if off + 4 > buf.len() {
        return Err(Error::ShortRead {
            wanted: 4,
            offset: off,
            available: buf.len().saturating_sub(off),
        });
    }
    let s = &buf[off..off + 4];
    Ok(if little_endian {
        u32::from_le_bytes(s.try_into().unwrap())
    } else {
        u32::from_be_bytes(s.try_into().unwrap())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a value the way the pcvalue streams do (LE 7-bit varint).
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    #[test]
    fn checked_pcdata_errors_are_not_a_prefix_maximum() {
        // Truncated varint in the value half.
        assert!(max_pcvalue(&[0x80], 0, 100, 1).is_err());
        // Truncated varint in the pc half.
        assert!(max_pcvalue(&[0x02, 0x80], 0, 100, 1).is_err());
        // Out-of-range start offsets.
        assert!(max_pcvalue(&[0x02, 0x01, 0x00], 3, 1, 1).is_err());
        assert!(max_pcvalue(&[], 0, 1, 1).is_err());
        // Exhaustion without proven termination: the stream simply ends
        // before a terminator or the PC bound.
        assert!(max_pcvalue(&[0x02, 0x01], 0, 100, 1).is_err());
        // Arithmetic overflow of the accumulated pc: a second record whose
        // pc delta is the largest 10-byte varint cannot be added.
        let overflow_pc = [
            &[0x02u8, 0x01][..],
            &[0x02][..],
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f][..],
        ]
        .concat();
        assert!(max_pcvalue(&overflow_pc, 0, u64::MAX, 1).is_err());
        // Arithmetic overflow of the accumulated value: two maximal positive
        // zig-zag deltas exceed i64.
        let overflow_value = [
            &varint(u64::MAX - 1)[..],
            &[0x01][..],
            &varint(u64::MAX - 1)[..],
            &[0x01][..],
        ]
        .concat();
        assert!(max_pcvalue(&overflow_value, 0, u64::MAX, 1).is_err());
        // Exhausted scan budget: two mebibytes of zero records neither
        // terminate nor reach the pc bound.
        let zeros = vec![0u8; 2 * 1024 * 1024];
        assert!(max_pcvalue(&zeros, 0, 1_000, 1).is_err());
    }

    #[test]
    fn checked_valid_zero_and_bounded_streams_complete() {
        // First value delta reaches index 0 and the PC bound completes the
        // scan (brief example).
        assert_eq!(max_pcvalue(&[2, 1, 0], 0, 1, 1).unwrap(), 0);
        // Normal terminator: a zero value-delta after a recorded PC.
        assert_eq!(max_pcvalue(&[2, 1, 0, 5, 1], 0, 100, 1).unwrap(), 0);
        // PC-bound completion; trailing bytes are never read.
        assert_eq!(max_pcvalue(&[2, 5], 0, 4, 1).unwrap(), 0);
        // Quantum scaling: pc deltas count in quantum units.
        assert_eq!(max_pcvalue(&[2, 1, 0], 0, 2, 2).unwrap(), 0);
        assert_eq!(max_pcvalue(&[2, 2, 3, 1, 0], 0, 8, 2).unwrap(), 0);
        // A leading zero value-delta is a legal first record, not a
        // terminator; this is the two-record control's real stream.
        assert_eq!(
            max_pcvalue(&[0, 4, 2, 4, 2, 4, 3, 9, 0], 0, 64, 1).unwrap(),
            1
        );
    }
}
