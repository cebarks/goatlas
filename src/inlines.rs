//! Checked recovery of inline call evidence.
//!
//! Inline records live in the per-function `FUNCDATA_InlTree` arrays of the
//! gofunc blob, addressed through the moduledata `gofunc` base. This adapter
//! establishes that base once, gates on the verified layout family, and maps
//! the vendored checked decoder's typed outcomes into the report contract.
//! A retained record keeps its tree index, raw parent-PC and positive start
//! line; an unreadable callee name is an indexed occurrence plus a core
//! `unresolved_inline_name` gap, never a dropped or invented identity.

use crate::functions::{Attribution, FunctionWalk, symbol_identity};
use crate::report::*;
use unstrip::gobin::SectionKind;
use unstrip::inline::{
    InlineIssueKind, InlinedCall, decode_inline_tree_checked, resolve_leaf_name,
};
use unstrip::pclntab::FuncEntry;
use unstrip::{GoBinary, Pclntab};

/// pclntab magic of the go1.20+ table layout.
const MAGIC_1_20: u32 = 0xffff_fff1;

/// Moduledata word holding `gofunc` on the go1.27 layout (go1.27 inserted
/// `typedesclen`/`itaboffset`/`itabsize` before `rodata`/`gofunc`).
const GOFUNC_WORD_1_27: usize = 43;
/// Moduledata word holding `gofunc` on the go1.20-1.26 layouts.
const GOFUNC_WORD_1_20: usize = 40;

pub(crate) struct InlineWalk {
    pub(crate) observations: Vec<InlineOccurrence>,
    pub(crate) coverage: Coverage,
}

/// Locate the moduledata `gofunc` base once.
///
/// The vendored `ModuleData::locate` carries go1.20-1.26 layouts only; go1.27
/// moved the `gofunc` field. This minimal, version-gated reader scans the
/// data sections for the pcHeader anchor (as the vendored locator does) and
/// reads `gofunc` at the family-correct word, corroborating each candidate
/// with the runtime's region sanity before accepting it.
pub(crate) fn locate_gofunc(bin: &GoBinary, go_version: Option<&str>) -> Result<u64, Issue> {
    if bin.pclntab_addr == 0 {
        return Err(issue(
            "moduledata_unavailable",
            "the pclntab runtime address is zero, so no moduledata anchor exists",
        ));
    }
    let Some(version) = go_version else {
        return Err(issue(
            "missing_go_version",
            "no recorded Go version establishes the moduledata layout family",
        ));
    };
    let Some(minor) = version_minor(version) else {
        return Err(issue(
            "unsupported_inline_layout",
            format!("Go version {version} is not a verified moduledata layout family"),
        ));
    };
    let gofunc_word = if minor >= 27 {
        GOFUNC_WORD_1_27
    } else {
        GOFUNC_WORD_1_20
    };
    let ps = bin.pointer_size();
    let mut target = vec![0u8; ps];
    match (ps, bin.little_endian) {
        (8, true) => target.copy_from_slice(&bin.pclntab_addr.to_le_bytes()),
        (8, false) => target.copy_from_slice(&bin.pclntab_addr.to_be_bytes()),
        (_, true) => target.copy_from_slice(&(bin.pclntab_addr as u32).to_le_bytes()),
        (_, false) => target.copy_from_slice(&(bin.pclntab_addr as u32).to_be_bytes()),
    }
    for kind in [
        SectionKind::NoPtrData,
        SectionKind::Data,
        SectionKind::ReadOnlyData,
    ] {
        for section in bin.sections.iter().filter(|s| s.kind == kind) {
            let Some(end) = section.file_offset.checked_add(section.file_size) else {
                continue;
            };
            if end > bin.bytes.len() {
                continue;
            }
            let buf = &bin.bytes[section.file_offset..end];
            let mut pos = 0;
            while buf.len() - pos >= ps {
                if buf[pos..pos + ps] == target[..] {
                    let file_off = section.file_offset + pos;
                    if let Some(gofunc) = candidate_gofunc(bin, file_off, ps, gofunc_word) {
                        return Ok(gofunc);
                    }
                }
                pos += ps;
            }
        }
    }
    Err(issue(
        "moduledata_unavailable",
        "no moduledata candidate was found in the data sections",
    ))
}

/// Read and validate `gofunc` from a moduledata candidate. A wrong layout or
/// a corrupt candidate is rejected structurally rather than trusted.
fn candidate_gofunc(bin: &GoBinary, file_off: usize, ps: usize, gofunc_word: usize) -> Option<u64> {
    let bytes = &bin.bytes;
    let word = |index: usize| -> Option<u64> {
        let at = file_off.checked_add(index.checked_mul(ps)?)?;
        let end = at.checked_add(ps)?;
        let slice = bytes.get(at..end)?;
        Some(match (ps, bin.little_endian) {
            (8, true) => u64::from_le_bytes(slice.try_into().ok()?),
            (8, false) => u64::from_be_bytes(slice.try_into().ok()?),
            (_, true) => u32::from_le_bytes(slice.try_into().ok()?) as u64,
            (_, false) => u32::from_be_bytes(slice.try_into().ok()?) as u64,
        })
    };
    // Region sanity the runtime guarantees for a real moduledata.
    let (minpc, maxpc) = (word(20)?, word(21)?);
    let (text, etext) = (word(22)?, word(23)?);
    if minpc > maxpc || text == 0 || text > etext {
        return None;
    }
    // go1.27 inserted typedesclen between types and etypes.
    let (types, etypes) = if gofunc_word == GOFUNC_WORD_1_27 {
        (word(37)?, word(39)?)
    } else {
        (word(37)?, word(38)?)
    };
    if types == 0 || etypes < types {
        return None;
    }
    let gofunc = word(gofunc_word)?;
    if gofunc == 0 || bin.section_for_addr(gofunc).is_none() {
        return None;
    }
    Some(gofunc)
}

/// The inline-tree layout is decoded only for the official go1.20+ table
/// magic and the documented 16-byte `runtime.inlinedCall` release families
/// 1.20-1.26 plus source-inspected 1.27; this is not a version-wide support
/// claim.
fn inline_layout_supported(magic: u32, go_version: &str) -> bool {
    magic == MAGIC_1_20 && version_minor(go_version).is_some()
}

/// Release-family minor version of a recorded Go version, when it names a
/// verified 16-byte `runtime.inlinedCall` family (1.20 through 1.27).
/// Development, unknown and future versions stay unsupported.
fn version_minor(go_version: &str) -> Option<u32> {
    let rest = go_version.strip_prefix("go1.")?;
    let digits_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let minor: u32 = rest[..digits_end].parse().ok()?;
    let tail = &rest[digits_end..];
    if !tail.is_empty() && !tail.starts_with('.') {
        return None;
    }
    (20..=27).contains(&minor).then_some(minor)
}

/// The gofunc base must be a mapped, nonzero address.
fn validate_inline_base(bin: &GoBinary, gofunc: u64) -> Result<(), Issue> {
    if gofunc == 0 {
        return Err(issue(
            "gofunc_base_invalid",
            "the gofunc base is zero; inline funcdata cannot be located",
        ));
    }
    if bin.section_for_addr(gofunc).is_none() {
        return Err(issue(
            "gofunc_base_invalid",
            format!(
                "the gofunc base 0x{gofunc:016x} is not mapped by any section of \
                 the binary"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn collect_inlines(
    bin: &GoBinary,
    pcln: Pclntab<'_>,
    walk: &FunctionWalk,
    attribution: &mut Attribution,
    go_version: Option<&str>,
) -> InlineWalk {
    let Some(version) = go_version else {
        return unavailable_walk(
            "missing_go_version",
            "no recorded Go version establishes the inline layout family",
        );
    };
    if !inline_layout_supported(pcln.magic(), version) {
        return unavailable_walk(
            "unsupported_inline_layout",
            format!(
                "pclntab magic 0x{:08x} with Go version {version} is not a verified \
                 16-byte inlinedCall layout family",
                pcln.magic()
            ),
        );
    }
    if walk.entries.is_empty() {
        return unavailable_walk(
            "no_inspected_functions",
            "no body-validated functions are available for inline inspection",
        );
    }
    let Some(gofunc) = pcln.gofunc() else {
        return unavailable_walk(
            "missing_gofunc_base",
            "the gofunc base was not established on the table",
        );
    };
    if let Err(problem) = validate_inline_base(bin, gofunc) {
        return unavailable_walk(&problem.code, problem.message);
    }

    let attempted = walk.entries.len() as u64;
    let mut out = InlineWalk {
        observations: Vec::new(),
        coverage: Coverage {
            state: CoverageState::Unavailable,
            expected: Some(attempted),
            visited: 0,
            expected_observations: None,
            observations: 0,
            issues: Vec::new(),
        },
    };
    let mut expected_sum = 0u64;
    let mut all_complete = true;
    for func in &walk.entries {
        let decoded = decode_inline_tree_checked(bin, &pcln, func);
        if decoded.complete {
            out.coverage.visited += 1;
            expected_sum += decoded.expected_entries.unwrap_or(0) as u64;
        } else {
            all_complete = false;
            for problem in decoded.issues {
                out.coverage.issues.push(inline_issue(problem, func));
            }
        }
        for (index, call) in decoded.entries.iter().enumerate() {
            record_inline(&mut out, &pcln, func, index as u32, call, attribution);
        }
    }
    let function_walk_decoded = walk.coverage.state == CoverageState::Decoded;
    if !function_walk_decoded {
        // Trees of functions the function walk could not validate were never
        // inspected; the inline component cannot claim a decoded traversal.
        out.coverage.issues.push(Issue {
            code: "function_walk_partial".into(),
            message: "the function walk was not decoded, so inline trees of \
                     uninspected functions were not examined"
                .into(),
            function: None,
            entry: None,
        });
    }
    out.coverage.expected_observations =
        (all_complete && function_walk_decoded).then_some(expected_sum);
    out.coverage.state = if out.coverage.issues.is_empty() && all_complete && function_walk_decoded
    {
        CoverageState::Decoded
    } else {
        CoverageState::Partial
    };
    // Fixed-width hex entries sort identically to numeric order; one sort
    // for the whole walk keeps the report deterministic.
    out.observations.sort_by(|a, b| {
        (&a.container_entry, a.inline_index).cmp(&(&b.container_entry, b.inline_index))
    });
    out
}

/// Append one inline record's evidence, marking an unreadable callee name as
/// a core gap atomically with the observation.
fn record_inline(
    out: &mut InlineWalk,
    pcln: &Pclntab<'_>,
    func: &FuncEntry,
    index: u32,
    call: &InlinedCall,
    attribution: &mut Attribution,
) {
    let raw_name = resolve_leaf_name(pcln, call);
    let (package_path, name, module) = raw_name
        .as_deref()
        .map_or((None, None, None), |raw| symbol_identity(raw, attribution));
    out.observations.push(InlineOccurrence {
        raw_name: raw_name.clone(),
        package_path,
        name,
        module,
        container_entry: format!("0x{:016x}", func.address),
        inline_index: index,
        parent_pc: call.parent_pc,
        start_line: (call.start_line > 0).then_some(call.start_line),
    });
    out.coverage.observations += 1;
    if raw_name.is_none() {
        out.coverage.state = CoverageState::Partial;
        out.coverage.issues.push(Issue {
            code: "unresolved_inline_name".into(),
            message: format!(
                "inline record {index} of {name} has an unreadable name offset",
                name = func.name
            ),
            function: Some(func.name.clone()),
            entry: Some(format!("0x{:016x}", func.address)),
        });
    }
}

/// Map a typed decoder issue to a stable `inline_`-prefixed code with the
/// containing function as context.
fn inline_issue(problem: unstrip::inline::InlineDecodeIssue, func: &FuncEntry) -> Issue {
    let kind = match problem.kind {
        InlineIssueKind::TruncatedFunction => "truncated_function",
        InlineIssueKind::InvalidPcData => "invalid_pc_data",
        InlineIssueKind::EntryLimit => "entry_limit",
        InlineIssueKind::AddressOverflow => "address_overflow",
        InlineIssueKind::TruncatedTree => "truncated_tree",
        InlineIssueKind::InvalidParentPc => "invalid_parent_pc",
        InlineIssueKind::UnsupportedLayout => "unsupported_layout",
        InlineIssueKind::MissingGofunc => "missing_gofunc",
    };
    Issue {
        code: format!("inline_{kind}"),
        message: problem.message,
        function: Some(func.name.clone()),
        entry: Some(format!("0x{:016x}", func.address)),
    }
}

fn unavailable_walk(code: &str, message: impl Into<String>) -> InlineWalk {
    InlineWalk {
        observations: vec![],
        coverage: Coverage {
            state: CoverageState::Unavailable,
            expected: None,
            visited: 0,
            expected_observations: None,
            observations: 0,
            issues: vec![issue(code, message)],
        },
    }
}

fn issue(code: &str, message: impl Into<String>) -> Issue {
    Issue {
        code: code.into(),
        message: message.into(),
        function: None,
        entry: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit_code;
    use crate::functions::collect_functions;
    use std::path::Path;

    fn binary(name: &str) -> GoBinary {
        GoBinary::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/fixtures")
                .join(name),
        )
        .expect("required fixture: run just fixtures")
    }

    fn empty_coverage(state: CoverageState) -> Coverage {
        Coverage {
            state,
            expected: None,
            visited: 0,
            expected_observations: None,
            observations: 0,
            issues: vec![],
        }
    }

    fn inventory(states: [CoverageState; 3], issues: Vec<Issue>) -> Inventory {
        let [build_info_state, functions_state, inlining_state] = states;
        let mut functions = empty_coverage(functions_state);
        functions.issues = issues;
        Inventory {
            schema_version: 1,
            producer: Producer {
                tool_version: "test".into(),
                extractor_version: "1.2.0".into(),
                extractor_patch: None,
            },
            artifact: Artifact {
                sha256: String::new(),
                size_bytes: 0,
                format: None,
                arch: None,
                go_version: None,
            },
            build: None,
            functions: vec![],
            inline_occurrences: vec![],
            coverage: CoverageSet {
                build_info: empty_coverage(build_info_state),
                functions,
                inlining: empty_coverage(inlining_state),
            },
        }
    }

    #[test]
    fn unknown_layout_stays_unavailable() {
        assert!(inline_layout_supported(0xffff_fff1, "go1.27.3"));
        assert!(inline_layout_supported(0xffff_fff1, "go1.20"));
        assert!(inline_layout_supported(0xffff_fff1, "go1.20.1"));
        assert!(inline_layout_supported(0xffff_fff1, "go1.26.9"));
        assert!(!inline_layout_supported(0xffff_fff0, "go1.27.3"));
        assert!(!inline_layout_supported(0xffff_fff1, "go1.19.5"));
        assert!(!inline_layout_supported(0xffff_fff1, "go1.28.0"));
        assert!(!inline_layout_supported(0xffff_fff1, "go1.100.0"));
        assert!(!inline_layout_supported(0xffff_fff1, "(devel)"));
        assert!(!inline_layout_supported(0xffff_fff1, "unknown"));
        assert!(!inline_layout_supported(0xffff_fff1, ""));
    }

    #[test]
    fn missing_base_is_unavailable() {
        // Zero/unmapped gofunc guard errors.
        let bin = binary("app.stripped");
        assert!(validate_inline_base(&bin, 0).is_err());
        assert!(validate_inline_base(&bin, 0x0001_0000_0000_0000).is_err());

        // A healthy function walk with a table whose sections no longer map
        // the gofunc base is unavailable, not a decoded zero.
        let gofunc = locate_gofunc(&bin, Some("go1.27.3")).expect("locator works on the fixture");
        let pcln = Pclntab::parse(&bin).unwrap().with_gofunc(gofunc);
        let mut attribution = Attribution::new(None);
        let walk = collect_functions(&bin, &pcln, &mut attribution);
        assert!(!walk.entries.is_empty());
        let mut cleared = GoBinary::parse(bin.bytes.clone()).unwrap();
        cleared.sections.clear();
        let reparsed = Pclntab::parse(&cleared).unwrap().with_gofunc(gofunc);
        let out = collect_inlines(
            &cleared,
            reparsed,
            &walk,
            &mut attribution,
            Some("go1.27.3"),
        );
        assert_eq!(out.coverage.state, CoverageState::Unavailable);
    }

    #[test]
    fn all_trees_failing_is_partial_not_unavailable() {
        let bin = binary("app.stripped");
        let gofunc = locate_gofunc(&bin, Some("go1.27.3")).expect("locator works on the fixture");
        let pcln = Pclntab::parse(&bin).unwrap().with_gofunc(gofunc);
        let mut attribution = Attribution::new(None);
        let mut walk = collect_functions(&bin, &pcln, &mut attribution);
        assert!(!walk.entries.is_empty());
        // Point every body-validated entry's `_func` struct past the table so
        // each tree decode fails structurally. Typed issues are evidence of
        // attempted inspection, so the walk stays partial — never
        // unavailable, and never a decoded zero.
        let past_end = pcln.data().len();
        for func in &mut walk.entries {
            func.func_off = past_end;
        }
        let out = collect_inlines(&bin, pcln, &walk, &mut attribution, Some("go1.27.3"));
        assert_eq!(out.coverage.state, CoverageState::Partial);
        assert_eq!(out.coverage.visited, 0);
        assert_eq!(out.coverage.observations, 0);
        assert!(out.observations.is_empty());
        assert!(out.coverage.expected_observations.is_none());
        assert!(
            out.coverage
                .issues
                .iter()
                .all(|i| i.code.starts_with("inline_"))
        );
    }

    #[test]
    fn unresolved_name_is_retained_with_partial_coverage() {
        let bin = binary("app.stripped");
        let gofunc = locate_gofunc(&bin, Some("go1.27.3")).expect("locator works on the fixture");
        let pcln = Pclntab::parse(&bin).unwrap().with_gofunc(gofunc);
        let func = pcln
            .functions_with_offsets()
            .unwrap()
            .into_iter()
            .find(|f| f.name == "main.twoInlineCalls")
            .expect("fixture control");
        let mut out = InlineWalk {
            observations: vec![],
            coverage: empty_coverage(CoverageState::Unavailable),
        };
        let mut attribution = Attribution::new(None);
        record_inline(
            &mut out,
            &pcln,
            &func,
            0,
            &InlinedCall {
                func_id: 0,
                name_off: 0,
                parent_pc: -1,
                start_line: 10,
            },
            &mut attribution,
        );
        record_inline(
            &mut out,
            &pcln,
            &func,
            1,
            &InlinedCall {
                func_id: 0,
                name_off: -1,
                parent_pc: 5,
                start_line: 0,
            },
            &mut attribution,
        );
        assert!(out.observations[0].raw_name.is_none());
        assert!(out.observations[1].raw_name.is_none());
        assert_eq!(out.coverage.observations, 2);
        assert_eq!(out.coverage.state, CoverageState::Partial);
        assert!(
            out.coverage
                .issues
                .iter()
                .any(|i| i.code == "unresolved_inline_name")
        );
    }

    #[test]
    fn exit_status_tracks_core_coverage() {
        use CoverageState::{Decoded, Partial, Unavailable};
        for build_info in [Decoded, Partial, Unavailable] {
            for functions in [Decoded, Partial, Unavailable] {
                for inlining in [Decoded, Partial, Unavailable] {
                    let label = format!("{build_info:?}/{functions:?}/{inlining:?}");
                    let all_decoded =
                        build_info == Decoded && functions == Decoded && inlining == Decoded;
                    let r = inventory([build_info, functions, inlining], vec![]);
                    assert_eq!(exit_code(&r), if all_decoded { 0 } else { 2 }, "{label}");
                }
            }
        }
        // Attribution-only issues do not fail decoded extraction.
        let r = inventory(
            [Decoded, Decoded, Decoded],
            vec![issue(
                "module_attribution_unavailable",
                "no recorded module prefixes the package",
            )],
        );
        assert_eq!(exit_code(&r), 0);
    }
}
