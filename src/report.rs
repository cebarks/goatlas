use serde::Serialize;

#[derive(Debug, PartialEq, Serialize)]
pub struct Inventory {
    pub schema_version: u32,
    pub producer: Producer,
    pub artifact: Artifact,
    pub build: Option<BuildIdentity>,
    pub functions: Vec<FunctionObservation>,
    pub inline_occurrences: Vec<InlineOccurrence>,
    pub coverage: CoverageSet,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Producer {
    pub tool_version: String,
    pub extractor_version: String,
    pub extractor_patch: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Artifact {
    pub sha256: String,
    pub size_bytes: u64,
    pub format: Option<String>,
    pub arch: Option<String>,
    pub go_version: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct BuildIdentity {
    pub path: Option<String>,
    pub main: Option<ModuleRecord>,
    pub dependencies: Vec<ModuleRecord>,
    pub replacements: Vec<Replacement>,
    pub settings: Vec<BuildSetting>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ModuleRecord {
    pub path: String,
    pub version: String,
    pub sum: Option<String>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Replacement {
    pub from: ModuleRecord,
    pub to: ModuleRecord,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BuildSetting {
    pub key: String,
    pub value: String,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct FunctionObservation {
    pub raw_name: String,
    pub package_path: Option<String>,
    pub name: Option<String>,
    pub module: Option<ModuleRef>,
    pub entry: String,
    pub size_bytes: u64,
    pub file: Option<String>,
    pub start_line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleRef {
    pub kind: ModuleKind,
    pub path: String,
    pub version: String,
    pub basis: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    Main,
    Dependency,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct InlineOccurrence {
    pub raw_name: Option<String>,
    pub package_path: Option<String>,
    pub name: Option<String>,
    pub module: Option<ModuleRef>,
    pub container_entry: String,
    pub inline_index: u32,
    pub parent_pc: i32,
    pub start_line: Option<i32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct CoverageSet {
    pub build_info: Coverage,
    pub functions: Coverage,
    pub inlining: Coverage,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Coverage {
    pub state: CoverageState,
    pub expected: Option<u64>,
    pub visited: u64,
    pub expected_observations: Option<u64>,
    pub observations: u64,
    pub issues: Vec<Issue>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    Decoded,
    Partial,
    Unavailable,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Issue {
    pub code: String,
    pub message: String,
    pub function: Option<String>,
    pub entry: Option<String>,
}
