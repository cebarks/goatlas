mod build_info;
mod functions;
pub mod report;

use report::*;
pub use report::{CoverageState, Inventory};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

/// Maximum inspected input size: 1 GiB. Oversized inputs are never scanned as prefixes.
pub const INPUT_LIMIT: u64 = 1 << 30;

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("input I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("input must be a regular file")]
    NotRegular,
    #[error("input exceeds {limit} byte limit")]
    InputTooLarge { limit: u64 },
}

pub fn scan_file(path: &Path) -> Result<Inventory, ScanError> {
    // Check before opening so a non-regular input (notably a FIFO) is not read.
    check_metadata(&fs::metadata(path)?)?;
    let file = File::open(path)?;
    check_metadata(&file.metadata()?)?;
    let mut bytes = Vec::new();
    // One extra byte distinguishes EOF at the limit from an oversized/growing file.
    file.take(INPUT_LIMIT + 1).read_to_end(&mut bytes)?;
    scan_bytes(bytes)
}

fn check_metadata(metadata: &fs::Metadata) -> Result<(), ScanError> {
    if !metadata.is_file() {
        return Err(ScanError::NotRegular);
    }
    if metadata.len() > INPUT_LIMIT {
        return Err(ScanError::InputTooLarge { limit: INPUT_LIMIT });
    }
    Ok(())
}

pub fn scan_bytes(bytes: Vec<u8>) -> Result<Inventory, ScanError> {
    if bytes.len() as u64 > INPUT_LIMIT {
        return Err(ScanError::InputTooLarge { limit: INPUT_LIMIT });
    }
    let mut report = Inventory {
        schema_version: 1,
        producer: Producer {
            tool_version: env!("CARGO_PKG_VERSION").into(),
            extractor_version: "1.2.0".into(),
            extractor_patch: Some("strict-buildinfo-v1".into()),
        },
        artifact: Artifact {
            sha256: Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            size_bytes: bytes.len() as u64,
            format: None,
            arch: None,
            go_version: None,
        },
        build: None,
        functions: vec![],
        inline_occurrences: vec![],
        coverage: CoverageSet {
            build_info: unavailable("not_attempted", "Build identity has not been inspected"),
            functions: unavailable(
                "not_attempted",
                "Function recovery was not reached for this input",
            ),
            inlining: unavailable(
                "not_attempted",
                "Inline recovery is not implemented in this slice",
            ),
        },
    };
    recover_evidence(bytes, &mut report);
    Ok(report)
}

fn recover_evidence(bytes: Vec<u8>, report: &mut Inventory) {
    let probe = match unstrip::probe(&bytes) {
        Ok(probe) => probe,
        Err(error) => {
            report.coverage.build_info = unavailable("unrecognized_container", error.to_string());
            return;
        }
    };
    let container = probe.container.to_ascii_lowercase();
    report.artifact.format = Some(container.clone());
    report.artifact.arch = Some(probe.arch.clone());
    if !is_supported_target(&container, &probe.arch, probe.bits, probe.little_endian) {
        report.coverage.build_info = unavailable(
            "unsupported_target",
            "Only little-endian 64-bit x86-64 ELF is supported",
        );
        return;
    }
    let bin = match unstrip::GoBinary::parse(bytes) {
        Ok(bin) => bin,
        Err(error) => {
            let message = format!("Go metadata prerequisite failed: {error}");
            report.coverage.build_info = unavailable("go_metadata_unavailable", message.clone());
            report.coverage.functions = unavailable("go_metadata_unavailable", message);
            return;
        }
    };
    let walk = build_info::extract_build_info(&bin);
    report.artifact.go_version = walk.version;
    report.build = walk.identity;
    report.coverage.build_info = walk.coverage;
    // Build-info partial/unavailable alone never suppresses recoverable
    // function evidence. One Pclntab and one owned package Attribution stay
    // alive here for this walk and the later inline walk.
    match unstrip::Pclntab::parse(&bin) {
        Ok(pcln) => {
            let mut attribution = functions::Attribution::new(report.build.as_ref());
            let walk = functions::collect_functions(&bin, &pcln, &mut attribution);
            debug_assert_eq!(
                walk.entries.len(),
                walk.observations.len(),
                "body-validated entries parallel the reported observations"
            );
            report.functions = walk.observations;
            report.coverage.functions = walk.coverage;
        }
        Err(error) => {
            report.coverage.functions = unavailable(
                "pclntab_unavailable",
                format!("runtime function table could not be parsed: {error}"),
            );
        }
    }
}

fn unavailable(code: &str, message: impl Into<String>) -> Coverage {
    Coverage {
        state: CoverageState::Unavailable,
        expected: None,
        visited: 0,
        expected_observations: None,
        observations: 0,
        issues: vec![Issue {
            code: code.into(),
            message: message.into(),
            function: None,
            entry: None,
        }],
    }
}

pub fn exit_code(report: &Inventory) -> u8 {
    if [
        &report.coverage.build_info,
        &report.coverage.functions,
        &report.coverage.inlining,
    ]
    .iter()
    .all(|coverage| coverage.state == CoverageState::Decoded)
    {
        0
    } else {
        2
    }
}

fn is_supported_target(container: &str, arch: &str, bits: u32, little_endian: bool) -> bool {
    container == "elf" && arch == "x86-64" && bits == 64 && little_endian
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrecognized_bytes_report_unavailable_and_exit_2() {
        let r = scan_bytes(b"abc".to_vec()).unwrap();
        assert_eq!(r.coverage.build_info.state, CoverageState::Unavailable);
        assert_eq!(exit_code(&r), 2);
    }

    #[test]
    fn sha256_identifies_the_inspected_bytes() {
        assert_eq!(
            scan_bytes(b"abc".to_vec()).unwrap().artifact.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn elf_without_go_metadata_is_unavailable() {
        let mut bytes = vec![0; 64];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        let r = scan_bytes(bytes).unwrap();
        assert_eq!(r.artifact.format.as_deref(), Some("elf"));
        assert!(r.build.is_none());
        assert_eq!(exit_code(&r), 2);
        for c in [
            &r.coverage.build_info,
            &r.coverage.functions,
            &r.coverage.inlining,
        ] {
            assert_eq!(c.state, CoverageState::Unavailable);
            assert!(!c.issues.is_empty());
        }
    }

    #[test]
    fn unsupported_target_is_never_decoded() {
        assert!(is_supported_target("elf", "x86-64", 64, true));
        assert!(!is_supported_target("pe", "x86-64", 64, true));
        assert!(!is_supported_target("elf", "arm64", 64, true));
        assert!(!is_supported_target("elf", "x86-64", 32, true));
        assert!(!is_supported_target("elf", "x86-64", 64, false));
    }

    #[test]
    fn short_build_header_does_not_unwind() {
        let mut bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/fixtures/app.stripped"
        ))
        .expect("run just fixtures");
        let magic = b"\xff Go buildinf:";
        let offset = bytes.windows(magic.len()).position(|w| w == magic).unwrap();
        bytes[offset] = 0;
        bytes.extend_from_slice(magic);
        bytes.extend_from_slice(&[8, 2]);
        let report = scan_bytes(bytes).expect("invalid header must produce a report, not unwind");
        assert_eq!(report.coverage.build_info.state, CoverageState::Unavailable);
        assert_eq!(report.coverage.build_info.expected, None);
        assert_eq!(report.coverage.build_info.visited, 0);
        assert_eq!(report.coverage.build_info.expected_observations, None);
        assert_eq!(report.coverage.build_info.observations, 0);
        assert_eq!(
            report.coverage.build_info.issues[0].code,
            "build_info_invalid_header"
        );
        assert!(report.artifact.go_version.is_none());
        assert!(report.build.is_none());
        assert_eq!(exit_code(&report), 2);
    }

    #[test]
    fn truncated_go_image_is_not_clean_success() {
        let mut bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/fixtures/app.stripped"
        ))
        .expect("run just fixtures");
        bytes.truncate(128);
        let r = scan_bytes(bytes).unwrap();
        assert_eq!(exit_code(&r), 2);
        assert!(r.build.is_none());
        assert!(r.functions.is_empty());
        assert!(r.inline_occurrences.is_empty());
    }
}
