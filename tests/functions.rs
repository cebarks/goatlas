mod common;

use common::{fixture, run_json};
use golang_symbol::{CoverageState, exit_code, scan_bytes, scan_file};
use std::collections::BTreeSet;
use unstrip::{GoBinary, Pclntab};

#[test]
fn reflection_only_implementation_survives_stripping() {
    let r = scan_file(&fixture("app.stripped")).unwrap();
    assert_eq!(r.coverage.functions.state, CoverageState::Decoded);
    assert!(r.functions.iter().any(|f| f.raw_name == "main.directKeep"));
    assert!(
        r.functions
            .iter()
            .any(|f| f.raw_name == "main.(*ReflectTarget).ReflectionOnly")
    );
    assert!(r.functions.iter().any(|f| f.raw_name == "runtime.main"));
    assert!(!r.functions.iter().any(|f| f.raw_name == "main.deadUnused"));
    assert!(
        !r.functions
            .iter()
            .any(|f| f.raw_name == "main.(*ReflectTarget).metadataOnly")
    );
    let direct = r
        .functions
        .iter()
        .find(|f| f.raw_name == "main.directKeep")
        .unwrap();
    assert_eq!(
        direct.file.as_deref(),
        Some("example.com/golang-symbol/fixture/main.go")
    );
    assert_eq!(exit_code(&r), 0);
    assert!(r.functions.iter().all(|f| f.size_bytes > 0));
    assert_eq!(
        r.coverage.functions.expected,
        Some(r.functions.len() as u64)
    );
    assert_eq!(r.coverage.functions.visited, r.functions.len() as u64);
    assert_eq!(
        r.coverage.functions.expected_observations,
        r.coverage.functions.expected
    );
    assert_eq!(r.coverage.functions.observations, r.functions.len() as u64);
    assert!(r.coverage.functions.issues.is_empty());
    let (out, j) = run_json(&fixture("app.stripped"));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(j["coverage"]["functions"]["state"], "decoded");
}

#[test]
fn stripped_and_unstripped_have_the_same_function_identities() {
    let a = scan_file(&fixture("app.unstripped")).unwrap();
    let b = scan_file(&fixture("app.stripped")).unwrap();
    assert!(!a.functions.is_empty());
    let names_a: BTreeSet<_> = a.functions.iter().map(|f| &f.raw_name).collect();
    let names_b: BTreeSet<_> = b.functions.iter().map(|f| &f.raw_name).collect();
    assert_eq!(names_a, names_b);
}

fn mutate(
    change: impl FnOnce(&GoBinary, &Pclntab<'_>, &mut Vec<u8>),
) -> (golang_symbol::Inventory, u64) {
    let bin = GoBinary::open(fixture("app.stripped")).unwrap();
    let pcln = Pclntab::parse(&bin).unwrap();
    assert!(pcln.magic_is_official());
    assert_eq!(pcln.ptrsize(), 8);
    let nfunc = pcln.nfunc();
    assert!(nfunc > 10);
    let mut bytes = bin.bytes.clone();
    change(&bin, &pcln, &mut bytes);
    (scan_bytes(bytes).unwrap(), nfunc)
}

#[test]
fn skipped_function_entry_is_partial_not_empty_success() {
    let (r, nfunc) = mutate(|bin, pcln, bytes| {
        let off = bin.pclntab_offset + pcln.funcdata_off() + (pcln.nfunc() as usize / 2) * 8 + 4;
        bytes[off..off + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    });
    assert!(r.build.is_some());
    assert!(!r.functions.is_empty());
    assert_eq!(r.coverage.functions.state, CoverageState::Partial);
    assert_eq!(r.coverage.functions.visited + 1, nfunc);
    assert!(
        r.coverage
            .functions
            .issues
            .iter()
            .any(|i| i.code == "function_count_mismatch")
    );
}

#[test]
fn unreadable_function_name_is_partial() {
    let (r, _) = mutate(|bin, pcln, bytes| {
        let entries = pcln.functions_with_offsets().unwrap();
        let off =
            bin.pclntab_offset + pcln.funcdata_off() + entries[entries.len() / 2].func_off + 4;
        bytes[off..off + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    });
    assert_eq!(r.coverage.functions.state, CoverageState::Partial);
    assert!(
        r.coverage
            .functions
            .issues
            .iter()
            .any(|i| i.code == "unresolved_function_name")
    );
}

#[test]
fn unknown_runtime_layout_is_not_decoded() {
    for pointer in [false, true] {
        let (r, _) = mutate(|bin, _, bytes| {
            if pointer {
                bytes[bin.pclntab_offset + 7] = 4;
            } else {
                bytes[bin.pclntab_offset..bin.pclntab_offset + 4]
                    .copy_from_slice(&0x12345678u32.to_le_bytes());
            }
        });
        assert_eq!(r.coverage.functions.state, CoverageState::Unavailable);
        assert_eq!(exit_code(&r), 2);
    }
}

#[test]
fn functions_survive_missing_build_info() {
    let bin = GoBinary::open(fixture("app.stripped")).unwrap();
    let mut bytes = bin.bytes;
    let magic = b"\xff Go buildinf:";
    let at = bytes.windows(magic.len()).position(|w| w == magic).unwrap();
    bytes[at] = 0;
    let r = scan_bytes(bytes).unwrap();
    assert!(r.build.is_none());
    assert_eq!(r.coverage.functions.state, CoverageState::Decoded);
    assert!(r.functions.iter().any(|f| f.raw_name == "main.directKeep"));
}

/// Test-only ground truth: the runtime text base recorded statically in
/// moduledata (word 22, counting words from the pclntab pointer anchor). Go
/// 1.26+ emits the moduledata in a dedicated `.go.module` section starting
/// exactly at the anchor.
fn moduledata_text(bin: &GoBinary) -> u64 {
    let section = bin
        .sections
        .iter()
        .find(|s| s.name == ".go.module")
        .expect("fixture carries a .go.module section");
    let off = section.file_offset + 22 * 8;
    u64::from_le_bytes(bin.bytes[off..off + 8].try_into().unwrap())
}

#[test]
fn externally_linked_pie_entries_use_the_moduledata_text_base() {
    // CGO with -linkmode=external places .init/.plt before .text and C
    // runtime code before the Go text inside .text, while the pclntab
    // header's textStart stays zero in the file (the dynamic linker fills it
    // by relocation). Function entries must resolve through moduledata.text
    // — the runtime's own base for functab entryOff — never through the
    // first executable section.
    let path = fixture("app.extpie.stripped");
    let (out, j) = run_json(&path);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(j["coverage"]["functions"]["state"], "decoded");
    assert_eq!(j["coverage"]["inlining"]["state"], "decoded");

    let bin = GoBinary::open(&path).unwrap();
    let base = moduledata_text(&bin);
    let text = bin
        .sections
        .iter()
        .find(|s| s.name == ".text")
        .expect(".text section");
    assert!(
        base >= text.addr && base < text.addr + text.vmsize,
        "moduledata text base {base:#x} must lie inside .text"
    );
    // The first Go function's entry is the base itself (entryOff 0).
    let functions = j["functions"].as_array().unwrap();
    assert!(
        functions
            .iter()
            .any(|f| f["entry"] == format!("0x{base:016x}")),
        "no function entry equals the moduledata text base {base:#x}"
    );
    // No entry may fall below the Go text base (the old .init fallback put
    // the first rows into .init/.plt).
    for f in functions {
        let entry =
            u64::from_str_radix(f["entry"].as_str().unwrap().strip_prefix("0x").unwrap(), 16)
                .unwrap();
        assert!(entry >= base, "entry {entry:#x} is below the Go text base");
    }
}
