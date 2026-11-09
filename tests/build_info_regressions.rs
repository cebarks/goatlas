mod common;

use common::{fixture, run_json};
use golang_symbol::{CoverageState, exit_code, scan_bytes};
use std::process::Command;

const MAGIC: &[u8] = b"\xff Go buildinf:";

fn header_offset(bytes: &[u8]) -> usize {
    bytes
        .windows(MAGIC.len())
        .position(|w| w == MAGIC)
        .expect("fixture build header")
}

// Test-only fixture mutation, not an application Go-format reader.
fn next_string(bytes: &[u8], pos: &mut usize) {
    let mut len = 0usize;
    let mut shift = 0;
    loop {
        let b = bytes[*pos];
        *pos += 1;
        len |= usize::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        assert!(shift < usize::BITS);
    }
    *pos += len;
    assert!(*pos <= bytes.len());
}

fn replace_build_block(module: &[u8]) -> (Vec<u8>, String) {
    let (_, healthy) = run_json(&fixture("app.stripped"));
    let version = healthy["artifact"]["go_version"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut bytes = std::fs::read(fixture("app.stripped")).unwrap();
    let original = header_offset(&bytes);
    bytes[original] = 0;
    let header = bytes.len();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&[8, 2]);
    bytes.resize(header + 32, 0);
    for data in [version.as_bytes(), module] {
        let mut len = data.len();
        while len >= 128 {
            bytes.push((len as u8 & 0x7f) | 0x80);
            len >>= 7;
        }
        bytes.push(len as u8);
        bytes.extend_from_slice(data);
    }
    (bytes, version)
}

#[test]
fn earlier_non_header_magic_does_not_mask_later_build_info() {
    let original = std::fs::read(fixture("app.stripped")).unwrap();
    let (_, control) = run_json(&fixture("app.stripped"));
    assert_eq!(
        control["coverage"]["build_info"],
        serde_json::json!({
            "state": "decoded", "expected": 1, "visited": 1,
            "expected_observations": 1, "observations": 1, "issues": []
        })
    );
    assert!(control["artifact"]["go_version"].is_string());
    assert_eq!(control["build"]["dependencies"][0]["version"], "v1.2.3");

    // Test-only ELF64 layout inspection: unused padding between occupied ranges,
    // excluding header tables and every file-backed section.
    assert_eq!(&original[..6], b"\x7fELF\x02\x01");
    let word =
        |offset| u16::from_le_bytes(original[offset..offset + 2].try_into().unwrap()) as usize;
    let address = |offset| {
        usize::try_from(u64::from_le_bytes(
            original[offset..offset + 8].try_into().unwrap(),
        ))
        .unwrap()
    };
    let sections = address(40);
    let mut occupied = vec![
        (0, address(32) + word(54) * word(56)),
        (sections, sections + word(58) * word(60)),
    ];
    occupied.extend((0..word(60)).filter_map(|index| {
        let section = sections + index * word(58);
        let kind = u32::from_le_bytes(original[section + 4..section + 8].try_into().unwrap());
        let offset = address(section + 24);
        let size = address(section + 32);
        (kind != 8 && size != 0 && offset != 0).then_some((offset, offset + size))
    }));
    occupied.sort_unstable();
    let real_header = header_offset(&original);
    let mut occupied_end = 0;
    let offset = occupied
        .iter()
        .find_map(|&(start, end)| {
            let gap_start = occupied_end;
            occupied_end = occupied_end.max(end);
            let gap_end = start.min(real_header);
            (gap_start < gap_end)
                .then(|| {
                    original[gap_start..gap_end]
                        .windows(32)
                        .position(|window| window.iter().all(|byte| *byte == 0))
                        .map(|within| gap_start + within)
                })
                .flatten()
        })
        .expect("fixture requires safe zero padding before its build header");
    assert!(
        occupied
            .iter()
            .all(|&(start, end)| offset + 32 <= start || offset >= end)
    );
    assert!(offset + 32 <= real_header);
    let mut bytes = original.clone();
    bytes[offset..offset + MAGIC.len()].copy_from_slice(MAGIC);
    // The pointer and flags remain zero: this occurrence is not a header.
    assert_eq!(bytes.len(), original.len());
    assert_eq!(&bytes[real_header..], &original[real_header..]);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("earlier non-header literal");
    std::fs::write(&path, &bytes).unwrap();
    let (out, mut observed) = run_json(&path);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.is_empty());
    assert_eq!(observed["coverage"]["build_info"]["state"], "decoded");
    assert_ne!(
        observed["artifact"]["sha256"],
        control["artifact"]["sha256"]
    );
    // Every field except the necessarily changed input digest must match control,
    // including raw version, full identity, issues, counters and future arrays.
    observed["artifact"]["sha256"] = control["artifact"]["sha256"].clone();
    assert_eq!(observed, control);
}

#[test]
fn malformed_module_records_keep_validated_evidence_and_raw_version() {
    let body = b"path\tretained/path\nmod\tretained/main\tunknown\ndep\tvalid/dep\tv1\ndep\nbuild\tvcs.time\t2026-01-02T03:04:05Z\n";
    let module = [
        &b"\x30\x77\xaf\x0c\x92\x74\x08\x02\x41\xe1\xc1\x07\xe6\xd6\x18\xe6"[..],
        &body[..],
        &b"\xf9\x32\x43\x31\x86\x18\x20\x72\x00\x82\x42\x10\x41\x16\xd8\xf2"[..],
    ]
    .concat();
    let (bytes, version) = replace_build_block(&module);
    let r = scan_bytes(bytes).unwrap();
    assert_eq!(r.coverage.build_info.state, CoverageState::Partial);
    assert_eq!(r.artifact.go_version.as_deref(), Some(version.as_str()));
    assert_eq!(
        r.coverage.build_info.issues[0].code,
        "build_info_invalid_module_info"
    );
    let build = r.build.unwrap();
    assert_eq!(build.path.as_deref(), Some("retained/path"));
    assert_eq!(build.main.unwrap().version, "unknown");
    assert_eq!(build.dependencies.len(), 1);
    assert_eq!(build.settings[0].value, "2026-01-02T03:04:05Z");
}

#[test]
fn empty_framed_module_is_partial_not_version_only_decoded() {
    let module = b"\x30\x77\xaf\x0c\x92\x74\x08\x02\x41\xe1\xc1\x07\xe6\xd6\x18\xe6\xf9\x32\x43\x31\x86\x18\x20\x72\x00\x82\x42\x10\x41\x16\xd8\xf2";
    let (bytes, version) = replace_build_block(module);
    let r = scan_bytes(bytes).unwrap();
    assert_eq!(r.coverage.build_info.state, CoverageState::Partial);
    assert_eq!(r.artifact.go_version.as_deref(), Some(version.as_str()));
    assert!(r.build.is_none());
    assert_eq!(
        r.coverage.build_info.issues[0].code,
        "build_info_missing_module_info"
    );
    assert_eq!(r.coverage.build_info.observations, 1);
    assert_eq!(exit_code(&r), 2);
}

#[test]
fn invalid_module_framing_preserves_version_but_no_identity() {
    let (bytes, version) = replace_build_block(&[b'x'; 40]);
    let r = scan_bytes(bytes).unwrap();
    assert_eq!(r.coverage.build_info.state, CoverageState::Partial);
    assert_eq!(r.artifact.go_version.as_deref(), Some(version.as_str()));
    assert!(r.build.is_none());
    assert_eq!(
        r.coverage.build_info.issues[0].code,
        "build_info_invalid_module_info"
    );
    assert_eq!(exit_code(&r), 2);
}

#[test]
fn short_build_header_is_reported_not_panicked() {
    let mut bytes = std::fs::read(fixture("app.stripped")).unwrap();
    let offset = header_offset(&bytes);
    bytes[offset] = 0;
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&[8, 2]);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("short header");
    std::fs::write(&path, &bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_golang-symbol"))
        .arg(&path)
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["coverage"]["build_info"]["state"], "unavailable");
    assert!(
        !j["coverage"]["build_info"]["issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let report = scan_bytes(bytes).expect("short header must not unwind");
    assert_eq!(report.coverage.build_info.state, CoverageState::Unavailable);
    assert_eq!(exit_code(&report), 2);
}

#[test]
fn missing_module_blob_preserves_version_with_partial_coverage() {
    let (_, healthy) = run_json(&fixture("app.stripped"));
    assert_eq!(healthy["coverage"]["build_info"]["state"], "decoded");
    assert_eq!(healthy["build"]["dependencies"][0]["version"], "v1.2.3");
    let mut bytes = std::fs::read(fixture("app.stripped")).unwrap();
    let mut pos = header_offset(&bytes) + 32;
    next_string(&bytes, &mut pos);
    bytes[pos] = 0;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing module");
    std::fs::write(&path, &bytes).unwrap();
    let (out, j) = run_json(&path);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        j["artifact"]["go_version"],
        healthy["artifact"]["go_version"]
    );
    assert_eq!(j["coverage"]["build_info"]["state"], "partial");
    assert!(j["build"].is_null());
    assert_eq!(
        j["coverage"]["build_info"]["issues"][0]["code"],
        "build_info_missing_module_info"
    );
    assert_eq!(j["coverage"]["build_info"]["expected"], 1);
    assert_eq!(j["coverage"]["build_info"]["visited"], 1);
    assert_eq!(j["coverage"]["build_info"]["expected_observations"], 1);
    assert_eq!(j["coverage"]["build_info"]["observations"], 1);
    let r = scan_bytes(bytes).unwrap();
    assert_eq!(r.coverage.build_info.state, CoverageState::Partial);
    assert_eq!(
        r.artifact.go_version.as_deref(),
        healthy["artifact"]["go_version"].as_str()
    );
    assert!(r.build.is_none());
}
