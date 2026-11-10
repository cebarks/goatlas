//! Regressions and checked-API cases for the `unstrip::inline` decoder.
//!
//! Step 2 pinned the silent-empty-success bug on truncated `_func` headers
//! with the pre-existing public API before the vendored dependency was
//! modified. The `checked_*` cases below were appended after that baseline
//! was reproduced; they exercise the checked decoder against the real
//! fixture and against controlled corruptions of it.

mod common;

use common::fixture;
use unstrip::inline::{
    InlineIssueKind, decode_inline_tree, decode_inline_tree_checked, resolve_leaf_name,
};
use unstrip::pclntab::FuncEntry;
use unstrip::{GoBinary, Pclntab};

#[test]
fn truncated_header_is_not_empty_success() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = Pclntab::parse(&bin).unwrap();
    // A modern `_func` header needs 44 readable bytes at
    // `funcdata_off + func_off`. Pointing far past the table end must be an
    // explicit error, never a clean `Ok(vec![])` that erases the failure.
    let bad = FuncEntry {
        address: 0x401_000,
        name: "main.twoInlineCalls".into(),
        func_off: pcln.data().len(),
        size: 64,
    };
    assert!(decode_inline_tree(&bin, &pcln, &bad).is_err());
}

/// Test-only mirror of the application's go1.27 gofunc locator: the moduledata
/// `gofunc` field is word 43 on go1.27 (word 40 on go1.20-1.26). The vendored
/// `ModuleData::locate` does not carry a go1.27 layout, so tests read the
/// field directly from the fixture's `.go.module` section.
fn gofunc(bin: &GoBinary) -> u64 {
    let section = bin
        .sections
        .iter()
        .find(|s| s.name == ".go.module")
        .expect("fixture carries a .go.module section");
    let off = section.file_offset + 43 * 8;
    u64::from_le_bytes(bin.bytes[off..off + 8].try_into().unwrap())
}

fn pclntab(bin: &GoBinary) -> Pclntab<'_> {
    Pclntab::parse(bin).unwrap().with_gofunc(gofunc(bin))
}

fn entry(pcln: &Pclntab<'_>, name: &str) -> FuncEntry {
    pcln.functions_with_offsets()
        .unwrap()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("fixture carries {name}"))
}

/// File offset of the `_func` struct for `func` inside the original bytes.
fn func_start(pcln: &Pclntab<'_>, func: &FuncEntry) -> usize {
    pcln.funcdata_off() + func.func_off
}

/// Value of the `FUNCDATA_InlTree` funcdata slot (u32 gofunc-relative offset).
fn inltree_slot(pcln: &Pclntab<'_>, func: &FuncEntry) -> u64 {
    let data = pcln.data();
    let fs = func_start(pcln, func);
    let npcdata = u32::from_le_bytes(data[fs + 28..fs + 32].try_into().unwrap()) as usize;
    let slot_off = fs + 44 + npcdata * 4 + 3 * 4;
    u32::from_le_bytes(data[slot_off..slot_off + 4].try_into().unwrap()) as u64
}

fn issue_kinds(r: &unstrip::inline::InlineTreeDecode) -> Vec<InlineIssueKind> {
    r.issues.iter().map(|i| i.kind).collect()
}

#[test]
fn checked_short_header_and_slot_are_incomplete() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = pclntab(&bin);
    let two = entry(&pcln, "main.twoInlineCalls");

    // A short header fails before any count can be established.
    let bad = FuncEntry {
        address: two.address,
        name: two.name.clone(),
        func_off: pcln.data().len(),
        size: two.size,
    };
    let r = decode_inline_tree_checked(&bin, &pcln, &bad);
    assert!(!r.complete);
    assert!(r.expected_entries.is_none());
    assert!(issue_kinds(&r).contains(&InlineIssueKind::TruncatedFunction));
    assert!(r.entries.is_empty());

    // A later slot failure retains the count the index scan established:
    // corrupting npcdata pushes the funcdata array far out of range while
    // the pcdata stream (and its two-entry count) stays readable.
    let mut bytes = bin.bytes.clone();
    let fs = bin.pclntab_offset + func_start(&pcln, &two);
    bytes[fs + 28..fs + 32].copy_from_slice(&0x4000_0000u32.to_le_bytes());
    let bin2 = GoBinary::parse(bytes).unwrap();
    let pcln2 = pclntab(&bin2);
    let two2 = entry(&pcln2, "main.twoInlineCalls");
    let r = decode_inline_tree_checked(&bin2, &pcln2, &two2);
    assert!(!r.complete);
    assert_eq!(r.expected_entries, Some(2));
    assert!(issue_kinds(&r).contains(&InlineIssueKind::TruncatedFunction));
    assert!(r.entries.is_empty());
}

#[test]
fn checked_short_tree_retains_prefix() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = pclntab(&bin);
    let two = entry(&pcln, "main.twoInlineCalls");
    let base = gofunc(&bin) + inltree_slot(&pcln, &two);

    // Shrink the section that maps the tree so it ends exactly after the
    // first 16-byte record: entry 0 stays readable, entry 1 does not.
    let mut bin2 = GoBinary::parse(bin.bytes.clone()).unwrap();
    for section in bin2.sections.iter_mut() {
        if section.contains_addr(base) {
            let new_end = base + 16;
            section.vmsize = new_end - section.addr;
            section.file_size = section.file_size.min(section.vmsize as usize);
        }
    }
    let r = decode_inline_tree_checked(&bin2, &pcln, &two);
    assert_eq!(r.expected_entries, Some(2));
    assert_eq!(r.entries.len(), 1);
    assert!(!r.complete);
    assert!(issue_kinds(&r).contains(&InlineIssueKind::TruncatedTree));
    // The retained prefix is the real first record, not fabricated data.
    assert_eq!(
        resolve_leaf_name(&pcln, &r.entries[0]).as_deref(),
        Some("main.inlineOnly")
    );
}

#[test]
fn checked_entry_cap_is_observable() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = pclntab(&bin);
    let two = entry(&pcln, "main.twoInlineCalls");

    // Rewrite the pcdata stream to claim 65,537 entries: a first record with
    // value delta +65,537 (zig-zag 131,074), pc delta 1, then a terminator.
    let fs = func_start(&pcln, &two);
    let data = pcln.data();
    let pcdata_off =
        u32::from_le_bytes(data[fs + 44 + 8..fs + 44 + 12].try_into().unwrap()) as usize;
    let at = bin.pclntab_offset + pcln.pctab_off() + pcdata_off;
    let mut bytes = bin.bytes.clone();
    bytes[at..at + 5].copy_from_slice(&[0x82, 0x80, 0x08, 0x01, 0x00]);
    let bin2 = GoBinary::parse(bytes).unwrap();
    let pcln2 = pclntab(&bin2);
    let two2 = entry(&pcln2, "main.twoInlineCalls");

    let r = decode_inline_tree_checked(&bin2, &pcln2, &two2);
    assert_eq!(r.expected_entries, Some(65_537));
    assert!(r.entries.len() <= 65_536);
    assert!(!r.complete);
    assert!(issue_kinds(&r).contains(&InlineIssueKind::EntryLimit));
}

#[test]
fn checked_address_and_parent_failures_are_explicit() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = pclntab(&bin);
    let two = entry(&pcln, "main.twoInlineCalls");

    // A base at u64::MAX overflows the tree address computation.
    let pcln_max = Pclntab::parse(&bin).unwrap().with_gofunc(u64::MAX);
    let r = decode_inline_tree_checked(&bin, &pcln_max, &two);
    assert!(!r.complete);
    assert!(issue_kinds(&r).contains(&InlineIssueKind::AddressOverflow));
    assert!(r.entries.is_empty());

    // An unmapped base yields an unreadable tree, not a decoded zero.
    // (Low addresses can still fall inside zero-based debug sections, so
    // pick an address no section can contain.)
    let pcln_unmapped = Pclntab::parse(&bin)
        .unwrap()
        .with_gofunc(0x0001_0000_0000_0000);
    let r = decode_inline_tree_checked(&bin, &pcln_unmapped, &two);
    assert!(!r.complete);
    assert!(issue_kinds(&r).contains(&InlineIssueKind::TruncatedTree));
    assert!(r.entries.is_empty());

    // Invalid parent PCs taint the record; the record is never returned.
    for bad_pc in [-2i32, 0x7fff_ffff] {
        let base = gofunc(&bin) + inltree_slot(&pcln, &two);
        let at = bin.file_offset_for_addr(base).unwrap() + 8;
        let mut bytes = bin.bytes.clone();
        bytes[at..at + 4].copy_from_slice(&bad_pc.to_le_bytes());
        let bin2 = GoBinary::parse(bytes).unwrap();
        let pcln2 = pclntab(&bin2);
        let two2 = entry(&pcln2, "main.twoInlineCalls");
        let r = decode_inline_tree_checked(&bin2, &pcln2, &two2);
        assert!(!r.complete, "parent_pc {bad_pc}");
        assert!(issue_kinds(&r).contains(&InlineIssueKind::InvalidParentPc));
        assert!(r.entries.is_empty(), "tainted record returned");
    }
}

#[test]
fn checked_missing_base_and_layout_are_incomplete() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = pclntab(&bin);
    let two = entry(&pcln, "main.twoInlineCalls");

    // Unset base.
    let pcln_unset = Pclntab::parse(&bin).unwrap();
    let r = decode_inline_tree_checked(&bin, &pcln_unset, &two);
    assert!(!r.complete);
    assert!(issue_kinds(&r).contains(&InlineIssueKind::MissingGofunc));
    assert!(r.entries.is_empty());
    assert_eq!(r.expected_entries, Some(2));

    // Zero base.
    let pcln_zero = Pclntab::parse(&bin).unwrap().with_gofunc(0);
    let r = decode_inline_tree_checked(&bin, &pcln_zero, &two);
    assert!(!r.complete);
    assert!(issue_kinds(&r).contains(&InlineIssueKind::MissingGofunc));
    assert!(r.entries.is_empty());

    // Unsupported header: the pcdata counts two entries but the funcdata
    // slot is not addressable, so the tree cannot be located.
    let fs = bin.pclntab_offset + func_start(&pcln, &two);
    let mut bytes = bin.bytes.clone();
    bytes[fs + 43] = 3;
    let bin2 = GoBinary::parse(bytes).unwrap();
    let pcln2 = pclntab(&bin2);
    let two2 = entry(&pcln2, "main.twoInlineCalls");
    let r = decode_inline_tree_checked(&bin2, &pcln2, &two2);
    assert!(!r.complete);
    assert_eq!(r.expected_entries, Some(2));
    assert!(issue_kinds(&r).contains(&InlineIssueKind::UnsupportedLayout));
    assert!(r.entries.is_empty());

    // Unsupported table layout family: a pre-1.20 magic.
    let mut bytes = bin.bytes.clone();
    bytes[bin.pclntab_offset..bin.pclntab_offset + 4]
        .copy_from_slice(&0xffff_fff0u32.to_le_bytes());
    let bin3 = GoBinary::parse(bytes).unwrap();
    let pcln3 = Pclntab::parse(&bin3).unwrap().with_gofunc(gofunc(&bin3));
    let two3 = entry(&pcln3, "main.twoInlineCalls");
    let r = decode_inline_tree_checked(&bin3, &pcln3, &two3);
    assert!(!r.complete);
    assert!(r.expected_entries.is_none());
    assert!(issue_kinds(&r).contains(&InlineIssueKind::UnsupportedLayout));
    assert!(r.entries.is_empty());
}

#[test]
fn checked_valid_zero_and_bounded_streams_complete() {
    let bin = GoBinary::open(fixture("app.unstripped")).unwrap();
    let pcln = pclntab(&bin);

    // Genuine no-inline metadata: absent pcdata stream plus sentinel slot.
    let r = decode_inline_tree_checked(&bin, &pcln, &entry(&pcln, "main.directKeep"));
    assert!(r.complete);
    assert_eq!(r.expected_entries, Some(0));
    assert!(r.entries.is_empty());
    assert!(r.issues.is_empty());

    // The two-record control decodes completely.
    let r = decode_inline_tree_checked(&bin, &pcln, &entry(&pcln, "main.twoInlineCalls"));
    assert!(r.complete);
    assert_eq!(r.expected_entries, Some(2));
    assert_eq!(r.entries.len(), 2);
    assert!(r.issues.is_empty());

    // A real stdlib tree with a bounded multi-entry stream.
    let r = decode_inline_tree_checked(&bin, &pcln, &entry(&pcln, "runtime.main"));
    assert!(r.complete);
    assert_eq!(r.entries.len(), 15);
    assert!(r.issues.is_empty());

    // With fixture-package inlining disabled, the control's tree is a
    // genuinely empty, fully inspected tree.
    let bin_nl = GoBinary::open(fixture("app.noinline")).unwrap();
    let pcln_nl = pclntab(&bin_nl);
    let r = decode_inline_tree_checked(&bin_nl, &pcln_nl, &entry(&pcln_nl, "main.twoInlineCalls"));
    assert!(r.complete);
    assert_eq!(r.expected_entries, Some(0));
    assert!(r.entries.is_empty());
    assert!(r.issues.is_empty());
}
