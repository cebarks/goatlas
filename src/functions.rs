//! Checked recovery of retained standalone Go function implementations.
//!
//! The runtime function table (pclntab), not the type/method lists, is the
//! evidence that code was retained in the binary. A table row is only
//! reported as an implementation when its entry/size span resolves to a
//! nonzero stretch of file-backed executable text through link-time virtual
//! addresses. Module attribution is a conservative, cached package-path
//! prefix hint over the original recorded modules; it is never
//! source-version proof, and a null attribution does not erase the
//! code-presence evidence in `raw_name`, `entry` and `size_bytes`.

use crate::report::*;
use std::collections::HashMap;
use unstrip::pclntab::FuncEntry;
use unstrip::{GoBinary, Pclntab};

pub(crate) struct FunctionWalk {
    /// Body-validated rows only, parallel to `observations`. The later inline
    /// walk consumes these raw `_func` offsets instead of reparsing.
    pub(crate) entries: Vec<FuncEntry>,
    pub(crate) observations: Vec<FunctionObservation>,
    pub(crate) coverage: Coverage,
}

pub(crate) struct Attribution {
    pub(crate) records: Vec<ModuleRef>,
    by_package: HashMap<String, Option<ModuleRef>>,
}

impl Attribution {
    /// Copies the original main/dependency module references once.
    /// Replacements stay separate evidence and never back an attribution.
    pub(crate) fn new(build: Option<&BuildIdentity>) -> Self {
        let mut records = Vec::new();
        if let Some(build) = build {
            if let Some(main) = &build.main {
                records.push(module_ref(ModuleKind::Main, main));
            }
            for dependency in &build.dependencies {
                records.push(module_ref(ModuleKind::Dependency, dependency));
            }
        }
        Self {
            records,
            by_package: HashMap::new(),
        }
    }

    /// Resolved once per parsed package; repeated functions reuse the cached
    /// result instead of rescanning the module records.
    fn module_for(&mut self, package: &str) -> Option<ModuleRef> {
        if let Some(cached) = self.by_package.get(package) {
            return cached.clone();
        }
        let resolved = attribute_package(package, &self.records);
        self.by_package.insert(package.to_owned(), resolved.clone());
        resolved
    }
}

fn module_ref(kind: ModuleKind, record: &ModuleRecord) -> ModuleRef {
    ModuleRef {
        kind,
        path: record.path.clone(),
        version: record.version.clone(),
        basis: "package_path_prefix".into(),
    }
}

/// Longest unique original-module path that prefixes `package` at a path
/// boundary. Vendor namespaces and conflicting versions of the same path
/// stay null rather than guessing.
fn attribute_package(package: &str, records: &[ModuleRef]) -> Option<ModuleRef> {
    if package.starts_with("vendor/") || package.contains("/vendor/") {
        return None;
    }
    let matches: Vec<&ModuleRef> = records
        .iter()
        .filter(|record| path_prefix(&record.path, package))
        .collect();
    let longest = matches.iter().map(|record| record.path.len()).max()?;
    let longest_matches: Vec<&ModuleRef> = matches
        .iter()
        .copied()
        .filter(|record| record.path.len() == longest)
        .collect();
    let unique = longest_matches[0];
    if longest_matches.iter().all(|record| record == &unique) {
        Some(unique.clone())
    } else {
        None
    }
}

fn path_prefix(module_path: &str, package: &str) -> bool {
    if package == module_path {
        return true;
    }
    package
        .strip_prefix(module_path)
        .is_some_and(|rest| rest.starts_with('/'))
}

pub(crate) fn collect_functions(
    bin: &GoBinary,
    pcln: &Pclntab<'_>,
    attribution: &mut Attribution,
) -> FunctionWalk {
    if !pcln.magic_is_official() {
        return unavailable_walk(
            "unknown_runtime_layout",
            format!(
                "pclntab magic 0x{:08x} is not an official Go runtime table layout",
                pcln.magic()
            ),
        );
    }
    if pcln.ptrsize() as usize != bin.pointer_size() {
        return unavailable_walk(
            "unknown_runtime_layout",
            format!(
                "pclntab pointer width {} mismatches the container pointer width {}",
                pcln.ptrsize(),
                bin.pointer_size()
            ),
        );
    }
    let nfunc = pcln.nfunc();
    if nfunc == 0 {
        return unavailable_walk(
            "empty_function_table",
            "the runtime function table records zero functions",
        );
    }
    // The pinned walker silently skips malformed rows, so its returned length
    // is compared with the header count; a shortened walk is never decoded.
    let rows = match pcln.functions_with_offsets() {
        Ok(rows) => rows,
        Err(error) => {
            return unavailable_walk(
                "pclntab_walk_failed",
                format!("the pinned walker rejected the function table: {error}"),
            );
        }
    };
    let visited = rows.len() as u64;
    let mut issues = Vec::new();
    if visited != nfunc {
        issues.push(issue(
            "function_count_mismatch",
            format!(
                "the walker returned {visited} of {nfunc} header functions; \
                 malformed entries were skipped"
            ),
            None,
            None,
        ));
    }
    let mut paired: Vec<(FuncEntry, FunctionObservation)> = Vec::with_capacity(rows.len());
    for row in rows {
        let entry = format!("0x{:016x}", row.address);
        if row.name.is_empty() {
            // An unreadable name is a gap in the walk, not an unnamed
            // implementation.
            issues.push(issue(
                "unresolved_function_name",
                "a function row has an unreadable name offset and is not \
                 reported as a named implementation",
                None,
                Some(entry),
            ));
            continue;
        }
        if let Err(mut problem) = checked_body(bin, row.address, row.size) {
            problem.function = Some(row.name.clone());
            issues.push(problem);
            continue;
        }
        let (package_path, name, module) = symbol_identity(&row.name, attribution);
        let (file, start_line) = pcln
            .lookup(row.address)
            .map_or((None, None), |found| (found.file, found.start_line));
        let observation = FunctionObservation {
            raw_name: row.name.clone(),
            package_path,
            name,
            module,
            entry,
            size_bytes: row.size,
            file,
            start_line,
        };
        paired.push((row, observation));
    }
    // Fixed-width lowercase hex entries sort identically to numeric order.
    paired.sort_by(|a, b| (&a.1.entry, &a.1.raw_name).cmp(&(&b.1.entry, &b.1.raw_name)));
    let (entries, observations): (Vec<_>, Vec<_>) = paired.into_iter().unzip();
    let observed = observations.len() as u64;
    if observed == 0 && issues.is_empty() {
        issues.push(issue(
            "no_function_observations",
            "no usable function observation was recoverable",
            None,
            None,
        ));
    }
    let state = if observed == 0 {
        CoverageState::Unavailable
    } else if issues.is_empty() && visited == nfunc {
        CoverageState::Decoded
    } else {
        CoverageState::Partial
    };
    FunctionWalk {
        entries,
        observations,
        coverage: Coverage {
            state,
            expected: Some(nfunc),
            visited,
            expected_observations: Some(nfunc),
            observations: observed,
            issues,
        },
    }
}

/// A retained implementation must occupy a checked, nonzero span of
/// file-backed executable text, addressed through link-time virtual
/// addresses rather than file offsets or runtime load bases.
fn checked_body(bin: &GoBinary, entry: u64, size: u64) -> Result<(), Issue> {
    let invalid = |message: String| {
        Err(issue(
            "invalid_function_body",
            message,
            None,
            Some(format!("0x{entry:016x}")),
        ))
    };
    if entry == 0 {
        return invalid("null function entry is not a text address".into());
    }
    if size == 0 {
        return invalid("zero-length function span".into());
    }
    if entry.checked_add(size).is_none() {
        return invalid("function span overflows the address space".into());
    }
    match bin.text_slice_at(entry, size as usize) {
        Some(text) if text.len() == size as usize => Ok(()),
        _ => invalid("function span is not file-backed executable text".into()),
    }
}

/// Conservative symbol split and module attribution. Only unambiguous
/// simple functions, parenthesized receivers and generic-function tails are
/// decomposed; escaped, generated and ambiguous spellings stay null rather
/// than being guessed.
pub(crate) fn symbol_identity(
    raw: &str,
    attribution: &mut Attribution,
) -> (Option<String>, Option<String>, Option<ModuleRef>) {
    let Some((package, name)) = split_symbol(raw) else {
        return (None, None, None);
    };
    let module = attribution.module_for(&package);
    (Some(package), Some(name), module)
}

fn split_symbol(raw: &str) -> Option<(String, String)> {
    if raw.is_empty() || raw.contains('%') {
        return None;
    }
    // The package/name delimiter is the last '.' before any receiver or
    // generic syntax; the entire tail after it is the function name.
    let syntax = raw.find(['(', '[']).unwrap_or(raw.len());
    let delimiter = raw[..syntax].rfind('.')?;
    let (package, name) = (&raw[..delimiter], &raw[delimiter + 1..]);
    if package.is_empty() || package.ends_with('.') || package.starts_with("type:") {
        return None;
    }
    if name.is_empty() || name.contains('/') || name.contains('\u{b7}') {
        return None;
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '(') {
        return None;
    }
    if is_generated_name(name) {
        return None;
    }
    if name.contains('[') && !name.ends_with(']') {
        return None;
    }
    Some((package.to_owned(), name.to_owned()))
}

/// Closures (`funcN`), ABI wrappers (`abiN`) and method-value wrappers
/// (`-fm`) are compiler-generated spellings, not source-level names.
fn is_generated_name(name: &str) -> bool {
    if name.ends_with("-fm") {
        return true;
    }
    let generated_tail = |prefix: &str| {
        name.starts_with(prefix)
            && name.len() > prefix.len()
            && name[prefix.len()..].bytes().all(|b| b.is_ascii_digit())
    };
    generated_tail("func") || generated_tail("abi")
}

fn unavailable_walk(code: &str, message: impl Into<String>) -> FunctionWalk {
    FunctionWalk {
        entries: vec![],
        observations: vec![],
        coverage: Coverage {
            state: CoverageState::Unavailable,
            expected: None,
            visited: 0,
            expected_observations: None,
            observations: 0,
            issues: vec![issue(code, message, None, None)],
        },
    }
}

fn issue(
    code: &str,
    message: impl Into<String>,
    function: Option<String>,
    entry: Option<String>,
) -> Issue {
    Issue {
        code: code.into(),
        message: message.into(),
        function,
        entry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn binary(name: &str) -> GoBinary {
        GoBinary::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/fixtures")
                .join(name),
        )
        .expect("required fixture: run just fixtures")
    }

    #[test]
    fn pie_entries_are_file_backed_virtual_addresses() {
        let bin = binary("app.pie.stripped");
        let pcln = Pclntab::parse(&bin).unwrap();
        let walk = collect_functions(&bin, &pcln, &mut Attribution::new(None));
        assert_eq!(walk.coverage.state, CoverageState::Decoded);
        assert!(!walk.observations.is_empty());
        assert_eq!(walk.entries.len(), walk.observations.len());
        for f in walk.observations {
            let entry = u64::from_str_radix(f.entry.strip_prefix("0x").unwrap(), 16).unwrap();
            assert!(checked_body(&bin, entry, f.size_bytes).is_ok());
            assert_ne!(bin.file_offset_for_addr(entry).unwrap() as u64, entry);
        }
    }

    #[test]
    fn invalid_body_spans_are_not_implementations() {
        let bin = binary("app.stripped");
        let pcln = Pclntab::parse(&bin).unwrap();
        let entry = pcln.functions_with_offsets().unwrap()[0].address;
        assert!(checked_body(&bin, entry, 0).is_err());
        assert!(checked_body(&bin, u64::MAX, 2).is_err());
        assert!(checked_body(&bin, bin.pclntab_addr, 1).is_err());
        assert!(checked_body(&bin, 0, 1).is_err());
        assert!(checked_body(&bin, entry, u64::MAX).is_err());
    }

    fn build(paths: &[(&str, &str)]) -> BuildIdentity {
        BuildIdentity {
            path: None,
            main: None,
            dependencies: paths
                .iter()
                .map(|(p, v)| ModuleRecord {
                    path: (*p).into(),
                    version: (*v).into(),
                    sum: None,
                })
                .collect(),
            replacements: vec![],
            settings: vec![],
        }
    }

    #[test]
    fn attribution_is_boundary_aware_and_conservative() {
        let mut a_only = Attribution::new(Some(&build(&[("example.com/a", "v1")])));
        let mut nested = Attribution::new(Some(&build(&[
            ("example.com/a", "v1"),
            ("example.com/a/sub", "v2"),
        ])));
        let mut conflicting = Attribution::new(Some(&build(&[
            ("example.com/a", "v1"),
            ("example.com/a", "v2"),
        ])));
        let mut none = Attribution::new(None);
        assert!(
            symbol_identity("example.com/abc.F", &mut a_only)
                .2
                .is_none()
        );
        assert_eq!(
            symbol_identity("example.com/a/sub/p.F", &mut nested)
                .2
                .unwrap()
                .path,
            "example.com/a/sub"
        );
        assert!(
            symbol_identity("example.com/a.F", &mut conflicting)
                .2
                .is_none()
        );
        assert_eq!(
            symbol_identity("main.(*ReflectTarget).ReflectionOnly", &mut none)
                .1
                .as_deref(),
            Some("(*ReflectTarget).ReflectionOnly")
        );
        assert_eq!(
            symbol_identity("example.com/m/p.F[go.shape.int]", &mut none)
                .1
                .as_deref(),
            Some("F[go.shape.int]")
        );
        assert!(
            symbol_identity("gopkg.in/yaml%2ev3.Unmarshal", &mut none)
                .0
                .is_none()
        );
        assert!(
            symbol_identity("example.com/a/vendor/other/pkg.F", &mut a_only)
                .2
                .is_none()
        );
        let before = a_only.by_package.len();
        symbol_identity("example.com/a/p.F", &mut a_only);
        symbol_identity("example.com/a/p.G", &mut a_only);
        assert_eq!(a_only.by_package.len(), before + 1);
        assert_eq!(a_only.records.len(), 1);
        for raw in [
            "type:.eq.X",
            "main.F.func1",
            "main.F.abi0",
            "main..inittask",
            "main.F[broken",
            "main.(*T).M-fm",
        ] {
            assert!(symbol_identity(raw, &mut none).0.is_none(), "{raw}");
        }
    }
}
