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
    assert_eq!(exit_code(&r), 2);
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
    assert_eq!(out.status.code(), Some(2));
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
