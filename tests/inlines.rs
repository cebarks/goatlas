//! Inline evidence behavior of the whole scanner.

mod common;

use common::fixture;
use goatlas::report::InlineOccurrence;
use goatlas::{CoverageState, exit_code, scan_bytes, scan_file};
use std::collections::HashSet;
use unstrip::{GoBinary, Pclntab};

/// Test-only mirror of the application's go1.27 gofunc locator (moduledata
/// word 43 on go1.27, word 40 on go1.20-1.26).
fn gofunc(bin: &GoBinary) -> u64 {
    let section = bin
        .sections
        .iter()
        .find(|s| s.name == ".go.module")
        .expect("fixture carries a .go.module section");
    let off = section.file_offset + 43 * 8;
    u64::from_le_bytes(bin.bytes[off..off + 8].try_into().unwrap())
}

/// gofunc-relative base of `main.twoInlineCalls`' inline tree.
fn two_inline_calls_base(bin: &GoBinary, pcln: &Pclntab<'_>) -> u64 {
    let func = pcln
        .functions_with_offsets()
        .unwrap()
        .into_iter()
        .find(|f| f.name == "main.twoInlineCalls")
        .expect("fixture control");
    let data = pcln.data();
    let fs = pcln.funcdata_off() + func.func_off;
    let npcdata = u32::from_le_bytes(data[fs + 28..fs + 32].try_into().unwrap()) as usize;
    let slot_off = fs + 44 + npcdata * 4 + 3 * 4;
    let slot = u32::from_le_bytes(data[slot_off..slot_off + 4].try_into().unwrap()) as u64;
    gofunc(bin) + slot
}

#[test]
fn inline_only_helper_survives_stripping() {
    let r = scan_file(&fixture("app.stripped")).unwrap();
    assert_eq!(r.coverage.inlining.state, CoverageState::Decoded);
    assert!(
        r.inline_occurrences
            .iter()
            .any(|o| o.raw_name.as_deref() == Some("main.inlineOnly"))
    );
    assert!(!r.functions.iter().any(|f| f.raw_name == "main.inlineOnly"));
    assert_eq!(exit_code(&r), 0);
}

#[test]
fn repeated_and_mixed_evidence_is_not_collapsed() {
    for name in ["app.unstripped", "app.stripped", "app.pie.stripped"] {
        let r = scan_file(&fixture(name)).unwrap();
        assert_eq!(r.coverage.inlining.state, CoverageState::Decoded, "{name}");

        // Repeated evidence: the helper's records stay distinct rows.
        let helper: Vec<&InlineOccurrence> = r
            .inline_occurrences
            .iter()
            .filter(|o| o.raw_name.as_deref() == Some("main.inlineOnly"))
            .collect();
        assert!(helper.len() >= 2, "{name}");
        let unique: HashSet<(&str, u32)> = helper
            .iter()
            .map(|o| (o.container_entry.as_str(), o.inline_index))
            .collect();
        assert_eq!(unique.len(), helper.len(), "{name}");

        // The two-record control: two helper records in one containing
        // function, with preserved indexes, raw parent-PCs and positive
        // start lines.
        let two = r
            .functions
            .iter()
            .find(|f| f.raw_name == "main.twoInlineCalls")
            .unwrap();
        let in_two = helper
            .iter()
            .filter(|o| o.container_entry == two.entry)
            .count();
        assert_eq!(in_two, 2, "{name}");
        for o in &helper {
            assert!(o.parent_pc >= -1, "{name}");
            assert!(o.start_line.is_some_and(|line| line > 0), "{name}");
        }

        // Mixed evidence: main.mixed appears in both arrays.
        assert!(
            r.functions.iter().any(|f| f.raw_name == "main.mixed"),
            "{name}"
        );
        assert!(
            r.inline_occurrences
                .iter()
                .any(|o| o.raw_name.as_deref() == Some("main.mixed")),
            "{name}"
        );

        // Every inspected tree is complete and counters agree.
        assert_eq!(
            r.coverage.inlining.visited,
            r.coverage.inlining.expected.unwrap(),
            "{name}"
        );
        assert_eq!(
            r.coverage.inlining.observations,
            r.coverage.inlining.expected_observations.unwrap(),
            "{name}"
        );
        assert!(r.coverage.inlining.issues.is_empty(), "{name}");
        assert_eq!(exit_code(&r), 0, "{name}");
    }
}

#[test]
fn noinline_helper_has_an_inspected_empty_tree() {
    let r = scan_file(&fixture("app.noinline")).unwrap();
    // The helpers are standalone and absent from the inline occurrences.
    assert!(
        r.functions
            .iter()
            .any(|f| f.raw_name == "main.twoInlineCalls")
    );
    assert!(r.functions.iter().any(|f| f.raw_name == "main.inlineOnly"));
    assert!(r.functions.iter().any(|f| f.raw_name == "main.mixed"));
    assert!(
        !r.inline_occurrences
            .iter()
            .any(|o| o.raw_name.as_deref() == Some("main.inlineOnly"))
    );
    assert!(
        !r.inline_occurrences
            .iter()
            .any(|o| o.raw_name.as_deref() == Some("main.mixed"))
    );
    // Their trees were inspected and are genuinely empty, not skipped.
    assert_eq!(r.coverage.inlining.state, CoverageState::Decoded);
    assert_eq!(
        r.coverage.inlining.visited,
        r.coverage.inlining.expected.unwrap()
    );
    assert!(r.coverage.inlining.expected_observations.is_some());
    // Stdlib trees still carry inline evidence; every stdlib tree being
    // empty is not required.
    assert!(r.coverage.inlining.observations > 0);
    assert!(r.coverage.inlining.issues.is_empty());
    assert_eq!(exit_code(&r), 0);
}

#[test]
fn partial_tree_and_function_walk_propagate() {
    let bin = GoBinary::open(fixture("app.stripped")).unwrap();
    let pcln = Pclntab::parse(&bin).unwrap();

    // A tainted record in one tree: the checked prefix is retained, other
    // functions' evidence survives, and the totals stay unknown.
    let base = two_inline_calls_base(&bin, &pcln);
    let at = bin.file_offset_for_addr(base).unwrap() + 16 + 8;
    let mut bytes = bin.bytes.clone();
    bytes[at..at + 4].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    let r = scan_bytes(bytes).unwrap();
    assert_eq!(r.coverage.functions.state, CoverageState::Decoded);
    assert_eq!(r.coverage.inlining.state, CoverageState::Partial);
    assert!(r.coverage.inlining.expected_observations.is_none());
    assert!(
        r.inline_occurrences
            .iter()
            .any(|o| o.raw_name.as_deref() == Some("main.inlineOnly"))
    );
    assert!(
        r.inline_occurrences
            .iter()
            .any(|o| o.raw_name.as_deref() == Some("main.mixed"))
    );
    assert!(
        r.coverage
            .inlining
            .issues
            .iter()
            .any(|i| i.code == "inline_invalid_parent_pc")
    );
    assert_eq!(exit_code(&r), 2);

    // A partial function walk propagates: uninspected functions keep the
    // inline walk partial even when every attempted tree decoded.
    let entries = pcln.functions_with_offsets().unwrap();
    let off = bin.pclntab_offset + pcln.funcdata_off() + entries[entries.len() / 2].func_off + 4;
    let mut bytes = bin.bytes.clone();
    bytes[off..off + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let r = scan_bytes(bytes).unwrap();
    assert_eq!(r.coverage.functions.state, CoverageState::Partial);
    assert_eq!(r.coverage.inlining.state, CoverageState::Partial);
    assert!(r.coverage.inlining.expected_observations.is_none());
    assert_eq!(exit_code(&r), 2);
}

/// Old-version-fixture gate: a go1.25.7 build exercises the pre-1.27
/// moduledata layout (gofunc word 40), which no go1.27.3 fixture covers.
#[test]
fn word40_layout_fixture_decodes_inline_evidence() {
    let r = scan_file(&fixture("app.word40.stripped")).unwrap();
    assert_eq!(r.artifact.go_version.as_deref(), Some("go1.25.7"));
    assert_eq!(r.coverage.build_info.state, CoverageState::Decoded);
    assert_eq!(r.coverage.functions.state, CoverageState::Decoded);
    assert_eq!(r.coverage.inlining.state, CoverageState::Decoded);
    assert_eq!(exit_code(&r), 0);
    assert!(r.coverage.inlining.observations > 0);
    assert_eq!(
        r.coverage.inlining.observations,
        r.coverage
            .inlining
            .expected_observations
            .expect("complete walk")
    );
    // The repeated-inlining control keeps distinct records per occurrence
    // through the word-40 path, exactly as on the go1.27.3 fixtures.
    let container = r
        .functions
        .iter()
        .find(|f| f.raw_name == "main.twoInlineCalls")
        .expect("fixture control")
        .entry
        .clone();
    let records: Vec<&InlineOccurrence> = r
        .inline_occurrences
        .iter()
        .filter(|o| o.container_entry == container)
        .collect();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|o| o.raw_name.as_deref() == Some("main.inlineOnly"))
    );
    assert_eq!(records[0].inline_index, 0);
    assert_eq!(records[1].inline_index, 1);
}
