//! Pinned, bounded CLI integration for the independently maintained components.
//!
//! These are native host processes, not a sandbox. Callers choose private,
//! project-scoped workspaces; no developer store is discovered or imported.
//!
//! Two layouts are accepted (ADR 0024). A *source checkout* is the Luuna
//! repository with its exact-commit submodules: identity is proved by Git
//! metadata (gitlinks, the committed manifest, a clean checkout at the pin). A
//! *portable layout* is what `scripts/stage-components.mjs` writes for the
//! installer: the same `components/manifest.json`, a
//! `components/dist-manifest.json` recording each pin's commit and a content
//! digest of its staged tree, and optionally the pinned Bun executable. Its
//! identity is proved by recomputing those digests; a mismatch is refused by
//! component name and never silently degraded.
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub type Result<T> = std::result::Result<T, ComponentError>;
const MAX_STDOUT: usize = 4 * 1024 * 1024;
const MAX_STDERR: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const CLI_BRIDGE: &str = include_str!("../assets/cli-bridge.mjs");
/// Written by `scripts/stage-components.mjs`; its presence selects the portable layout.
pub const DIST_MANIFEST: &str = "components/dist-manifest.json";
const DIST_KIND: &str = "luuna-portable-components";
const DIST_SCHEMA_VERSION: u32 = 1;
const DIGEST_MAX_ENTRIES: usize = 200_000;
const DIGEST_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DIGEST_MAX_DEPTH: usize = 64;

#[derive(Debug, Error)]
pub enum ComponentError {
    #[error(
        "component integration is unavailable; install the pinned component dependencies and Bun"
    )]
    Unavailable,
    #[error(
        "component identity, source checkout or runtime version differs from its immutable pin"
    )]
    Identity,
    #[error("component workspace is invalid or contains links outside its managed data")]
    Workspace,
    #[error("component operation exceeded its deadline and the owned process was stopped")]
    Timeout,
    #[error("component output exceeded its bounded contract")]
    OutputLimit,
    #[error("component returned invalid structured output")]
    Protocol,
    #[error("component refused the requested operation")]
    Refused,
    #[error("component configuration contains executable hooks; managed integration does not authorize them")]
    Hooks,
    #[error("portable component `{component}` does not match its distribution manifest: {detail}; reinstall the packaged distribution")]
    Distribution { component: String, detail: String },
}

fn distribution(component: &str, detail: impl Into<String>) -> ComponentError {
    ComponentError::Distribution {
        component: component.to_owned(),
        detail: detail.into(),
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Pin {
    path: String,
    package: String,
    #[serde(rename = "sourceVersion")]
    source_version: String,
    repository: String,
    commit: String,
    cli: Cli,
}
#[derive(Clone, Debug, Deserialize)]
struct Cli {
    primary: String,
    short: String,
    legacy: Vec<String>,
    entrypoint: String,
}
#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    runtime: Runtime,
    components: Vec<Pin>,
}
#[derive(Deserialize)]
struct Runtime {
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct DistManifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    kind: String,
    source: DistSource,
    runtime: DistRuntime,
    components: Vec<DistComponent>,
}
#[derive(Deserialize)]
struct DistSource {
    #[serde(rename = "manifestSha256")]
    manifest_sha256: String,
}
#[derive(Deserialize)]
struct DistRuntime {
    name: String,
    version: String,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
}
#[derive(Deserialize)]
struct DistComponent {
    name: String,
    path: String,
    commit: String,
    digest: String,
}

/// How the pinned components were found and how their identity is proved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    /// The Luuna repository with exact-commit submodules (Git metadata).
    SourceCheckout,
    /// A staged layout with `components/dist-manifest.json` (content digests).
    Portable,
}

#[derive(Clone, Debug)]
enum Layout {
    Source,
    Portable {
        /// Component name to the recorded `sha256:` tree digest.
        digests: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug)]
pub struct Components {
    root: PathBuf,
    bun: PathBuf,
    version: String,
    manifest: Value,
    pins: BTreeMap<String, Pin>,
    layout: Layout,
}

impl Components {
    /// Resolve installation paths without reading a developer credential store.
    /// The exact checkout and Bun executable are revalidated before every call.
    ///
    /// The root is `LUUNA_COMPONENTS_ROOT`, else a portable layout beside the
    /// running executable (the packaged installation of ADR 0011/0024), else
    /// the compile-time source checkout. Bun is `LUUNA_BUN`, else the staged
    /// `bun/bun[.exe]` of a portable root, else the checkout's `.tools` copy,
    /// else `PATH`.
    pub fn discover() -> Result<Self> {
        let root = std::env::var_os("LUUNA_COMPONENTS_ROOT")
            .map(PathBuf::from)
            .or_else(packaged_root_beside_executable)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
        let platform = if cfg!(windows) {
            "windows"
        } else {
            std::env::consts::OS
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        let file = if cfg!(windows) { "bun.exe" } else { "bun" };
        let staged = root.join("bun").join(file);
        let local = root
            .join(".tools/bun/node_modules/@oven")
            .join(format!("bun-{platform}-{arch}/bin/{file}"));
        let bun = std::env::var_os("LUUNA_BUN")
            .map(PathBuf::from)
            .or_else(|| (staged.is_file() && root.join(DIST_MANIFEST).is_file()).then_some(staged))
            .or_else(|| local.is_file().then_some(local))
            .or_else(|| find_on_path(file))
            .ok_or(ComponentError::Unavailable)?;
        Self::from_root(root, bun)
    }

    pub fn from_root(root: PathBuf, bun: PathBuf) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|_| ComponentError::Unavailable)?;
        let bun = if bun.is_absolute() {
            bun
        } else {
            find_on_path(&bun.to_string_lossy()).ok_or(ComponentError::Unavailable)?
        };
        if !bun.is_file() {
            return Err(ComponentError::Unavailable);
        }
        let bytes = std::fs::read(root.join("components/manifest.json"))
            .map_err(|_| ComponentError::Unavailable)?;
        if bytes.len() > 256 * 1024 {
            return Err(ComponentError::Identity);
        }
        let manifest_value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ComponentError::Identity)?;
        let manifest: Manifest =
            serde_json::from_value(manifest_value.clone()).map_err(|_| ComponentError::Identity)?;
        if manifest.schema_version != 2
            || manifest.runtime.name != "bun"
            || manifest.runtime.version != "1.4.3"
            || manifest.components.len() != 3
        {
            return Err(ComponentError::Identity);
        }
        let mut pins = BTreeMap::new();
        for pin in manifest.components {
            let name = pin
                .cli
                .primary
                .strip_prefix("luuna-")
                .ok_or(ComponentError::Identity)?
                .to_owned();
            if !["missions", "atlas", "beacon"].contains(&name.as_str())
                || pin.path != format!("components/{name}")
                || pin.repository != format!("https://github.com/luuna-ai/luuna-{name}.git")
                || pin.package != format!("@luuna/{name}-cli")
                || pin.commit.len() != 40
                || !pin
                    .commit
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || pin.cli.entrypoint
                    != if name == "atlas" {
                        "src/cli.ts"
                    } else {
                        "src/index.ts"
                    }
                || pins.insert(name, pin).is_some()
            {
                return Err(ComponentError::Identity);
            }
        }
        let layout = if root.join(DIST_MANIFEST).is_file() {
            Layout::Portable {
                digests: portable_digests(&root, &bun, &bytes, &manifest.runtime.version, &pins)?,
            }
        } else {
            Layout::Source
        };
        Ok(Self {
            root,
            bun,
            version: manifest.runtime.version,
            manifest: manifest_value,
            pins,
            layout,
        })
    }

    pub fn layout(&self) -> LayoutKind {
        match self.layout {
            Layout::Source => LayoutKind::SourceCheckout,
            Layout::Portable { .. } => LayoutKind::Portable,
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn bun(&self) -> &Path {
        &self.bun
    }
    /// The pinned Bun version every call revalidates.
    pub fn runtime_version(&self) -> &str {
        &self.version
    }
    /// One line for diagnostics: the layout kind and where it was found.
    pub fn describe(&self) -> String {
        match self.layout {
            Layout::Source => format!(
                "source checkout at {} (Git gitlinks and pins)",
                self.root.display()
            ),
            Layout::Portable { .. } => format!(
                "portable layout at {} (distribution manifest digests verified)",
                self.root.display()
            ),
        }
    }

    /// Invoke only the pinned source entrypoint, with explicit argument arrays.
    /// Raw command arguments, stdout and stderr are never put into error text.
    pub async fn call(&self, component: &str, workspace: &Path, args: &[String]) -> Result<Value> {
        let name = component.strip_prefix("luuna-").unwrap_or(component);
        let pin = self.pins.get(name).ok_or(ComponentError::Identity)?;
        if args.is_empty()
            || args.len() > 128
            || args
                .iter()
                .any(|arg| arg.len() > 256 * 1024 || arg.contains('\0'))
        {
            return Err(ComponentError::Protocol);
        }
        let workspace = prepare_workspace(workspace)?;
        self.validate(pin, &workspace).await?;
        adopt_legacy_layout(&workspace)?;
        let data_dir = data_dir(name);
        if args[0] != "init" && !workspace.join(data_dir).is_dir() {
            return Err(ComponentError::Workspace);
        }
        check_project_data_links(&workspace, data_dir)?;
        if name == "missions"
            && args[0] != "init"
            && !workspace.join(data_dir).join("config.yaml").is_file()
        {
            return Err(ComponentError::Workspace);
        }
        if name == "atlas" && args[0] != "init" {
            let cfg = self
                .invoke(pin, &workspace, &["config".into(), "show".into()])
                .await?;
            if cfg.get("hooks").is_some_and(nonempty_hooks) {
                return Err(ComponentError::Hooks);
            }
        }
        let value = self.invoke(pin, &workspace, args).await?;
        if value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(ComponentError::Protocol);
        }
        Ok(value)
    }

    async fn validate(&self, pin: &Pin, workspace: &Path) -> Result<()> {
        let version = output(&self.bun, &["--version".into()], workspace, workspace, None).await?;
        if !version.status.success()
            || std::str::from_utf8(&version.stdout).ok().map(str::trim) != Some(&self.version)
        {
            return Err(ComponentError::Identity);
        }
        let directory = self.root.join(&pin.path);
        match &self.layout {
            Layout::Source => self.validate_source(pin, &directory, workspace).await?,
            Layout::Portable { digests } => {
                let name = pin.cli.primary.strip_prefix("luuna-").unwrap_or("");
                let expected = digests.get(name).ok_or(ComponentError::Identity)?;
                verify_portable_tree(&self.root, name, &directory, expected)?;
            }
        }
        let pkg: Value = serde_json::from_slice(
            &std::fs::read(directory.join("package.json")).map_err(|_| ComponentError::Identity)?,
        )
        .map_err(|_| ComponentError::Identity)?;
        if pkg["name"].as_str() != Some(&pin.package)
            || pkg["version"].as_str() != Some(&pin.source_version)
            || [&pin.cli.primary, &pin.cli.short]
                .into_iter()
                .chain(pin.cli.legacy.iter())
                .any(|alias| pkg["bin"][alias].as_str() != Some(&pin.cli.entrypoint))
        {
            return Err(ComponentError::Identity);
        }
        Ok(())
    }

    /// Source-checkout identity: the exact gitlink, committed manifest,
    /// submodule URL and a clean checkout at the pinned commit.
    async fn validate_source(&self, pin: &Pin, directory: &Path, workspace: &Path) -> Result<()> {
        if !directory.join(".git").exists()
            || !directory.join("node_modules").is_dir()
            || !directory
                .canonicalize()
                .map_err(|_| ComponentError::Identity)?
                .starts_with(&self.root)
        {
            return Err(ComponentError::Unavailable);
        }
        let index = git(
            &self.root,
            &["ls-files", "--stage", "--", &pin.path],
            workspace,
        )
        .await?;
        if index != format!("160000 {} 0\t{}", pin.commit, pin.path) {
            return Err(ComponentError::Identity);
        }
        // Runtime activation consumes a committed distribution, never a local
        // edit to its manifest, submodule URL, or staged/unstaged gitlink.
        git(
            &self.root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=dirty",
                "--quiet",
                "HEAD",
                "--",
                "components/manifest.json",
                ".gitmodules",
                &pin.path,
            ],
            workspace,
        )
        .await?;
        let committed_manifest = git(
            &self.root,
            &["show", "HEAD:components/manifest.json"],
            workspace,
        )
        .await?;
        if serde_json::from_str::<Value>(&committed_manifest)
            .map_err(|_| ComponentError::Identity)?
            != self.manifest
        {
            return Err(ComponentError::Identity);
        }
        let key = format!("submodule.{}.url", pin.path);
        if git(
            &self.root,
            &["config", "-f", ".gitmodules", "--get", &key],
            workspace,
        )
        .await?
            != pin.repository
            || git(directory, &["rev-parse", "HEAD"], workspace).await? != pin.commit
            || !git(
                directory,
                &["status", "--porcelain", "--untracked-files=all"],
                workspace,
            )
            .await?
            .is_empty()
        {
            return Err(ComponentError::Identity);
        }
        Ok(())
    }

    async fn invoke(&self, pin: &Pin, workspace: &Path, args: &[String]) -> Result<Value> {
        let mut cli_args = args.to_vec();
        if !cli_args.iter().any(|arg| arg == "--json") {
            cli_args.push("--json".into());
        }
        if pin.cli.primary == "luuna-atlas"
            && !cli_args.iter().any(|arg| arg == "--quiet" || arg == "-q")
        {
            cli_args.push("--quiet".into());
        }
        let input = serde_json::to_vec(&serde_json::json!({
            "entrypoint": self.root.join(&pin.path).join(&pin.cli.entrypoint), "args": cli_args
        }))
        .map_err(|_| ComponentError::Protocol)?;
        if input.len() > 1024 * 1024 {
            return Err(ComponentError::OutputLimit);
        }
        let arguments = ["--no-env-file", "--no-install", "--eval", CLI_BRIDGE].map(Into::into);
        let result = output(&self.bun, &arguments, workspace, workspace, Some(&input)).await?;
        if !result.status.success() {
            // Diagnostic data for the operator's log only: the component's
            // structured `error` field when its JSON answer carries one,
            // bounded; the typed error stays opaque and carries no output.
            let detail: String = serde_json::from_slice::<Value>(&result.stdout)
                .ok()
                .and_then(|value| value["error"].as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| String::from_utf8_lossy(&result.stdout).into_owned())
                .chars()
                .take(300)
                .collect();
            let stderr_line: String = String::from_utf8_lossy(&result.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(400)
                .collect();
            eprintln!(
                "luuna-components: {} refused the operation ({:?}): {detail} {stderr_line}",
                pin.cli.primary, result.status
            );
            return Err(ComponentError::Refused);
        }
        serde_json::from_slice(&result.stdout).map_err(|_| ComponentError::Protocol)
    }
}

fn nonempty_hooks(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Object(map) => map.values().any(nonempty_hooks),
        Value::Array(values) => !values.is_empty(),
        _ => true,
    }
}
fn packaged_root_beside_executable() -> Option<PathBuf> {
    let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
    directory.join(DIST_MANIFEST).is_file().then_some(directory)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Deterministic SHA-256 over a directory tree, shared with
/// `scripts/stage-components.mjs` (`treeDigest`): one line per regular file
/// (`<relative path>\0file\0<sha256 of contents>`) or symbolic link
/// (`<relative path>\0link\0<sha256 of the link target>`), relative paths
/// with `/` separators sorted by their UTF-8 bytes and joined with `\n`.
/// Empty directories contribute nothing; any other entry kind is an error.
pub fn tree_digest(directory: &Path) -> std::io::Result<String> {
    fn invalid(detail: &str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, detail.to_owned())
    }
    fn visit(
        directory: &Path,
        rel: &str,
        depth: usize,
        lines: &mut Vec<(Vec<u8>, String)>,
        bytes: &mut u64,
    ) -> std::io::Result<()> {
        if depth > DIGEST_MAX_DEPTH {
            return Err(invalid("tree nests too deeply"));
        }
        let full = if rel.is_empty() {
            directory.to_path_buf()
        } else {
            directory.join(rel)
        };
        for entry in std::fs::read_dir(&full)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("file name is not valid UTF-8"))?;
            let entry_rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if lines.len() >= DIGEST_MAX_ENTRIES {
                return Err(invalid("tree has too many entries"));
            }
            let metadata = std::fs::symlink_metadata(entry.path())?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                let target = std::fs::read_link(entry.path())?;
                let target = target
                    .to_str()
                    .ok_or_else(|| invalid("link target is not valid UTF-8"))?;
                let line = format!("{entry_rel}\0link\0{}", sha256_hex(target.as_bytes()));
                lines.push((entry_rel.into_bytes(), line));
            } else if kind.is_dir() {
                visit(directory, &entry_rel, depth + 1, lines, bytes)?;
            } else if kind.is_file() {
                *bytes += metadata.len();
                if *bytes > DIGEST_MAX_BYTES {
                    return Err(invalid("tree is too large"));
                }
                let line = format!("{entry_rel}\0file\0{}", sha256_file(&entry.path())?);
                lines.push((entry_rel.into_bytes(), line));
            } else {
                return Err(invalid("tree contains a special file"));
            }
        }
        Ok(())
    }
    let mut lines = Vec::new();
    let mut bytes = 0;
    visit(directory, "", 0, &mut lines, &mut bytes)?;
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    let joined = lines
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("sha256:{}", sha256_hex(joined.as_bytes())))
}

/// Recompute one staged component's digest and refuse any difference by name.
fn verify_portable_tree(root: &Path, name: &str, directory: &Path, expected: &str) -> Result<()> {
    if !directory.is_dir() {
        return Err(distribution(name, "its staged tree is missing"));
    }
    if !directory.join("node_modules").is_dir() {
        return Err(distribution(name, "its staged dependencies are missing"));
    }
    if !directory
        .canonicalize()
        .map_err(|_| ComponentError::Identity)?
        .starts_with(root)
    {
        return Err(ComponentError::Identity);
    }
    let actual = tree_digest(directory)
        .map_err(|error| distribution(name, format!("its staged tree cannot be read ({error})")))?;
    if actual != expected {
        return Err(distribution(
            name,
            format!("its staged tree digest {actual} differs from the recorded {expected}"),
        ));
    }
    Ok(())
}

/// Load and check `components/dist-manifest.json` against the pins, prove the
/// Bun executable is the staged one and verify every staged tree now, so a
/// tampered or incomplete installation is refused at discovery.
fn portable_digests(
    root: &Path,
    bun: &Path,
    manifest_bytes: &[u8],
    version: &str,
    pins: &BTreeMap<String, Pin>,
) -> Result<BTreeMap<String, String>> {
    let bytes = std::fs::read(root.join(DIST_MANIFEST)).map_err(|_| ComponentError::Unavailable)?;
    if bytes.len() > 256 * 1024 {
        return Err(ComponentError::Identity);
    }
    let dist: DistManifest = serde_json::from_slice(&bytes)
        .map_err(|_| distribution("manifest", "it is not readable"))?;
    if dist.schema_version != DIST_SCHEMA_VERSION || dist.kind != DIST_KIND {
        return Err(distribution("manifest", "its schema is not supported"));
    }
    if dist.source.manifest_sha256 != sha256_hex(manifest_bytes) {
        return Err(distribution(
            "manifest",
            "components/manifest.json is not the one it was staged with",
        ));
    }
    if dist.runtime.name != "bun" || dist.runtime.version != version {
        return Err(distribution(
            "bun",
            "the recorded Bun version differs from the pin",
        ));
    }
    if dist.components.len() != pins.len() {
        return Err(distribution(
            "manifest",
            "it does not stage exactly Missions, Atlas and Beacon",
        ));
    }
    let mut digests = BTreeMap::new();
    for component in dist.components {
        let pin = pins
            .get(&component.name)
            .ok_or_else(|| distribution(&component.name, "it is not a pinned component"))?;
        if component.path != pin.path {
            return Err(distribution(&component.name, "its staged path differs"));
        }
        if component.commit != pin.commit {
            return Err(distribution(
                &component.name,
                format!(
                    "its recorded commit {} differs from the pinned {}",
                    component.commit, pin.commit
                ),
            ));
        }
        let hex = component.digest.strip_prefix("sha256:").unwrap_or("");
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(distribution(
                &component.name,
                "its recorded digest is malformed",
            ));
        }
        if digests
            .insert(component.name.clone(), component.digest)
            .is_some()
        {
            return Err(distribution(&component.name, "it is recorded twice"));
        }
    }
    if let Some(executable) = dist.runtime.executable {
        let relative = Path::new(&executable);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(distribution(
                "bun",
                "its recorded path is not inside the layout",
            ));
        }
        let staged = root.join(relative);
        let same = match (staged.canonicalize(), bun.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            return Err(distribution(
                "bun",
                "the Bun executable in use is not the staged one",
            ));
        }
        let expected = dist.runtime.sha256.unwrap_or_default();
        let actual = sha256_file(&staged)
            .map_err(|_| distribution("bun", "the staged executable cannot be read"))?;
        if actual != expected {
            return Err(distribution(
                "bun",
                "the staged executable differs from its recorded hash",
            ));
        }
    }
    for (name, pin) in pins {
        let expected = digests
            .get(name)
            .ok_or_else(|| distribution(name, "it is not recorded in the distribution manifest"))?;
        verify_portable_tree(root, name, &root.join(&pin.path), expected)?;
    }
    Ok(digests)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    find_in(&std::env::var_os("PATH")?, name)
}
/// On Windows a bare name is looked up only as `name.exe`, as process
/// creation does: extensionless POSIX shims (npm installs one beside
/// `bun.cmd`) cannot start, and batch launchers would re-parse the bridge
/// script argument. A name that carries an extension is used as written.
fn find_in(search: &OsStr, name: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(search)
        .map(|path| path.join(&name))
        .find(|path| path.is_file())
}
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|_| ComponentError::Workspace)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| ComponentError::Workspace)?;
    }
    Ok(())
}
fn prepare_workspace(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| ComponentError::Workspace)?
            .join(path)
    };
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return Err(ComponentError::Workspace);
        }
        current.push(part);
        if std::fs::symlink_metadata(&current).is_ok_and(|metadata| linked(&metadata)) {
            return Err(ComponentError::Workspace);
        }
    }
    private_dir(&path)?;
    let path = path.canonicalize().map_err(|_| ComponentError::Workspace)?;
    let home = path.join(".luuna-component-home");
    if std::fs::symlink_metadata(&home).is_ok_and(|metadata| linked(&metadata)) {
        return Err(ComponentError::Workspace);
    }
    private_dir(&home)?;
    check_data_links(&home, 0, &mut 0)?;
    Ok(path)
}
/// Project data root shared by the three component stores (ADR 0028).
const DATA_ROOT: &str = ".luuna";

/// Relative store directory for a component name (`missions`, `atlas`; any
/// other name is `beacon`), below the workspace.
fn data_dir(name: &str) -> &'static str {
    match name {
        "missions" => ".luuna/missions",
        "atlas" => ".luuna/atlas",
        _ => ".luuna/beacon",
    }
}

type LegacyStore = (
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
);
/// Layout written by runtime versions before ADR 0028: (legacy directory,
/// new directory, optional (legacy config file, new config file) rename).
/// Kept in one place so it can be deleted once no managed store predates it.
const LEGACY_LAYOUT: [LegacyStore; 3] = [
    (".seeds", ".luuna/missions", None),
    (
        ".mulch",
        ".luuna/atlas",
        Some(("mulch.config.yaml", "config.yaml")),
    ),
    (".canopy", ".luuna/beacon", None),
];

/// Move a managed workspace created by an earlier runtime version to the
/// `.luuna/<component>` layout. Idempotent: a store already at the new
/// location is untouched (a store present at both locations is left for the
/// component CLI to refuse), and an interrupted Atlas config rename resumes.
/// Links are refused rather than followed.
pub fn adopt_legacy_layout(workspace: &Path) -> Result<()> {
    let root = workspace.join(DATA_ROOT);
    for (legacy, current, config) in LEGACY_LAYOUT {
        let (legacy, current) = (workspace.join(legacy), workspace.join(current));
        let legacy_is_dir = match std::fs::symlink_metadata(&legacy) {
            Ok(metadata) if linked(&metadata) => return Err(ComponentError::Workspace),
            Ok(metadata) => metadata.is_dir(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(ComponentError::Workspace),
        };
        if legacy_is_dir && std::fs::symlink_metadata(&current).is_err() {
            match std::fs::symlink_metadata(&root) {
                Ok(metadata) if linked(&metadata) || !metadata.is_dir() => {
                    return Err(ComponentError::Workspace);
                }
                Ok(_) => {}
                Err(_) => private_dir(&root)?,
            }
            std::fs::rename(&legacy, &current).map_err(|_| ComponentError::Workspace)?;
        }
        if let Some((old, new)) = config {
            let (old, new) = (current.join(old), current.join(new));
            if std::fs::symlink_metadata(&old).is_ok_and(|metadata| metadata.is_file())
                && std::fs::symlink_metadata(&new).is_err()
            {
                std::fs::rename(&old, &new).map_err(|_| ComponentError::Workspace)?;
            }
        }
    }
    Ok(())
}

/// `.luuna` itself must be a real directory (when present) and the component
/// store below it must contain no links.
fn check_project_data_links(workspace: &Path, data_dir: &str) -> Result<()> {
    match std::fs::symlink_metadata(workspace.join(DATA_ROOT)) {
        Ok(metadata) if linked(&metadata) || !metadata.is_dir() => {
            return Err(ComponentError::Workspace);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ComponentError::Workspace),
    }
    check_data_links(&workspace.join(data_dir), 0, &mut 0)
}
fn check_data_links(path: &Path, depth: usize, count: &mut usize) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(ComponentError::Workspace),
    };
    *count += 1;
    if linked(&metadata) || depth > 16 || *count > 20_000 {
        return Err(ComponentError::Workspace);
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|_| ComponentError::Workspace)? {
            check_data_links(
                &entry.map_err(|_| ComponentError::Workspace)?.path(),
                depth + 1,
                count,
            )?;
        }
    }
    Ok(())
}

async fn git(cwd: &Path, args: &[&str], workspace: &Path) -> Result<String> {
    let mut arguments: Vec<OsString> = [
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.autocrlf=true",
        "-c",
        if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        },
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if matches!(args.first(), Some(&"diff" | &"status")) {
        // A local info/attributes file can select arbitrary configured content
        // filters even when the committed source has no filter attributes.
        // Enumerating keys is read-only and never invokes the filter itself.
        let mut listed_args = arguments.clone();
        listed_args.extend(
            [
                "config",
                "--null",
                "--name-only",
                "--get-regexp",
                r"^filter\.",
            ]
            .map(OsString::from),
        );
        let listed = output(Path::new("git"), &listed_args, cwd, workspace, None).await?;
        if !listed.status.success() && listed.status.code() != Some(1) {
            return Err(ComponentError::Identity);
        }
        let keys = std::str::from_utf8(&listed.stdout).map_err(|_| ComponentError::Identity)?;
        let mut drivers = std::collections::BTreeSet::new();
        for key in keys.split('\0').filter(|key| !key.is_empty()) {
            let driver = key
                .strip_prefix("filter.")
                .and_then(|rest| rest.rsplit_once('.'))
                .map(|(driver, _)| driver)
                .filter(|driver| !driver.is_empty() && driver.len() <= 1024)
                .ok_or(ComponentError::Identity)?;
            drivers.insert(driver);
            if drivers.len() > 128 {
                return Err(ComponentError::Identity);
            }
        }
        for driver in drivers {
            for setting in ["clean=", "smudge=", "process=", "required=false"] {
                arguments.push("-c".into());
                arguments.push(format!("filter.{driver}.{setting}").into());
            }
        }
    }
    arguments.extend(args.iter().map(OsString::from));
    let result = output(Path::new("git"), &arguments, cwd, workspace, None).await?;
    if !result.status.success() {
        return Err(ComponentError::Identity);
    }
    String::from_utf8(result.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| ComponentError::Identity)
}

struct Captured {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
async fn bounded(mut stream: impl tokio::io::AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // Two readers remain inside the captured process future while a request is
    // pending. Keep their buffers off native thread stacks, including debug
    // builds where nested async state is copied through request middleware.
    let mut chunk = vec![0_u8; 8192];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| ComponentError::Protocol)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len() + read > limit {
            return Err(ComponentError::OutputLimit);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}
async fn output(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    workspace: &Path,
    input: Option<&[u8]>,
) -> Result<Captured> {
    output_with_deadline(program, args, cwd, workspace, input, TIMEOUT).await
}
async fn output_with_deadline(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    workspace: &Path,
    input: Option<&[u8]>,
    deadline: Duration,
) -> Result<Captured> {
    let home = workspace.join(".luuna-component-home");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .env("LOCALAPPDATA", &home)
        .env("NO_COLOR", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        // Git ignores a ceiling equal to its starting directory. Stop before
        // entering the managed workspace's parent, which can be a source repo.
        .env(
            "GIT_CEILING_DIRECTORIES",
            workspace.parent().ok_or(ComponentError::Workspace)?,
        )
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "core.hooksPath")
        .env(
            "GIT_CONFIG_VALUE_1",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("BUN_RUNTIME_TRANSPILER_CACHE_PATH", home.join("bun-cache"));
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|_| ComponentError::Unavailable)?;
    let owner = ProcessOwner::new(&child)?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or(ComponentError::Unavailable)?;
    let stderr = child.stderr.take().ok_or(ComponentError::Unavailable)?;
    let capture = async {
        let (status, stdout, stderr, _) = tokio::try_join!(
            async { child.wait().await.map_err(|_| ComponentError::Unavailable) },
            bounded(stdout, MAX_STDOUT),
            bounded(stderr, MAX_STDERR),
            async {
                if let (Some(mut stdin), Some(input)) = (stdin, input) {
                    stdin
                        .write_all(input)
                        .await
                        .map_err(|_| ComponentError::Protocol)?;
                    stdin
                        .shutdown()
                        .await
                        .map_err(|_| ComponentError::Protocol)?;
                }
                Ok::<(), ComponentError>(())
            }
        )?;
        Ok(Captured {
            status,
            stdout,
            stderr,
        })
    };
    let result = tokio::time::timeout(deadline, capture).await;
    drop(owner); // Ends all owned descendants before any error/result is returned.
    if !matches!(result, Ok(Ok(_))) {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
    result.map_err(|_| ComponentError::Timeout)?
}

#[cfg(unix)]
struct ProcessOwner {
    group: Option<u32>,
}
#[cfg(unix)]
impl ProcessOwner {
    fn new(child: &tokio::process::Child) -> Result<Self> {
        Ok(Self { group: child.id() })
    }
}
#[cfg(unix)]
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        if let Some(group) = self.group {
            unsafe {
                libc::killpg(group as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
struct ProcessOwner {
    job: windows_sys::Win32::Foundation::HANDLE,
}
#[cfg(windows)]
impl ProcessOwner {
    fn new(child: &tokio::process::Child) -> Result<Self> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(ComponentError::Unavailable);
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let Some(process) = child.raw_handle() else {
            unsafe {
                CloseHandle(job);
            }
            return Err(ComponentError::Unavailable);
        };
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if configured == 0 || unsafe { AssignProcessToJobObject(job, process as _) } == 0 {
            unsafe {
                CloseHandle(job);
            }
            return Err(ComponentError::Unavailable);
        }
        Ok(Self { job })
    }
}
#[cfg(windows)]
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}
#[cfg(windows)]
unsafe impl Send for ProcessOwner {}

#[cfg(not(any(unix, windows)))]
compile_error!("component process ownership is implemented only for Unix and Windows");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_stores_move_to_the_luuna_layout_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        for (legacy, _, _) in LEGACY_LAYOUT {
            std::fs::create_dir_all(ws.join(legacy)).unwrap();
            std::fs::write(ws.join(legacy).join("data.jsonl"), "{}\n").unwrap();
        }
        std::fs::write(ws.join(".mulch/mulch.config.yaml"), "version: 1\n").unwrap();
        for _ in 0..2 {
            adopt_legacy_layout(ws).unwrap();
            for (legacy, current, _) in LEGACY_LAYOUT {
                assert!(!ws.join(legacy).exists(), "{legacy}");
                assert_eq!(
                    std::fs::read_to_string(ws.join(current).join("data.jsonl")).unwrap(),
                    "{}\n"
                );
            }
            assert!(ws.join(".luuna/atlas/config.yaml").is_file());
            assert!(!ws.join(".luuna/atlas/mulch.config.yaml").exists());
        }
    }

    #[test]
    fn interrupted_config_rename_resumes_and_existing_new_store_wins() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".luuna/atlas")).unwrap();
        std::fs::write(ws.join(".luuna/atlas/mulch.config.yaml"), "a\n").unwrap();
        std::fs::create_dir_all(ws.join(".luuna/missions")).unwrap();
        std::fs::create_dir_all(ws.join(".seeds")).unwrap();
        adopt_legacy_layout(ws).unwrap();
        assert!(ws.join(".luuna/atlas/config.yaml").is_file());
        // Both locations present: nothing is merged or overwritten.
        assert!(ws.join(".seeds").is_dir() && ws.join(".luuna/missions").is_dir());
        // Nothing to do on an untouched workspace.
        let empty = tempfile::tempdir().unwrap();
        adopt_legacy_layout(empty.path()).unwrap();
        assert!(!empty.path().join(".luuna").exists());
    }

    #[test]
    fn luuna_root_must_be_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".luuna"), "file").unwrap();
        assert!(check_project_data_links(dir.path(), data_dir("atlas")).is_err());
        std::fs::create_dir_all(dir.path().join(".seeds")).unwrap();
        assert!(adopt_legacy_layout(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_luuna_root_and_legacy_stores_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (ws, elsewhere) = (dir.path().join("ws"), dir.path().join("elsewhere"));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(elsewhere.join("missions")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ws.join(".luuna")).unwrap();
        assert!(check_project_data_links(&ws, data_dir("missions")).is_err());
        std::fs::remove_file(ws.join(".luuna")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ws.join(".seeds")).unwrap();
        assert!(adopt_legacy_layout(&ws).is_err());
    }

    #[test]
    fn component_futures_fit_small_native_thread_stacks() {
        let runner = Components {
            root: PathBuf::new(),
            bun: PathBuf::new(),
            version: "1.4.3".into(),
            manifest: Value::Null,
            pins: BTreeMap::new(),
            layout: Layout::Source,
        };
        let args = ["list".into()];
        let call = runner.call("atlas", Path::new("unused"), &args);
        let read = bounded(tokio::io::empty(), MAX_STDOUT);
        let call_bytes = std::mem::size_of_val(&call);
        let read_bytes = std::mem::size_of_val(&read);
        eprintln!(
            "component call future: {call_bytes} bytes; bounded read future: {read_bytes} bytes"
        );
        assert!(read_bytes <= 1024, "read buffers must stay on the heap");
        assert!(
            call_bytes <= 4096,
            "component calls must not inflate native request stacks"
        );
    }

    fn node() -> PathBuf {
        find_on_path(if cfg!(windows) { "node.exe" } else { "node" })
            .expect("Node is a repository prerequisite")
    }

    #[test]
    fn bare_launcher_names_resolve_like_process_creation() {
        let dir = tempfile::tempdir().unwrap();
        let (shims, binaries) = (dir.path().join("shims"), dir.path().join("bin"));
        std::fs::create_dir(&shims).unwrap();
        std::fs::create_dir(&binaries).unwrap();
        // npm's extensionless POSIX shim is earlier on PATH than the binary.
        std::fs::write(shims.join("bun"), "#!/bin/sh\n").unwrap();
        std::fs::write(binaries.join("bun.exe"), "").unwrap();
        let search = std::env::join_paths([&shims, &binaries]).unwrap();
        let expected = if cfg!(windows) {
            binaries.join("bun.exe")
        } else {
            shims.join("bun")
        };
        assert_eq!(find_in(&search, "bun"), Some(expected));
        assert_eq!(find_in(&search, "bun.exe"), Some(binaries.join("bun.exe")));
        assert_eq!(find_in(&search, "missing"), None);
    }

    #[tokio::test]
    async fn component_capture_strips_credentials_and_uses_a_private_home() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = prepare_workspace(&dir.path().canonicalize().unwrap()).unwrap();
        std::env::set_var("LUUNA_COMPONENT_TEST_SECRET", "synthetic-test-only");
        let code = "process.stdout.write(JSON.stringify({secret:process.env.LUUNA_COMPONENT_TEST_SECRET??null,home:process.env.HOME,git:process.env.GIT_CONFIG_GLOBAL}))";
        let result = output(
            &node(),
            &["-e".into(), code.into()],
            &workspace,
            &workspace,
            None,
        )
        .await
        .unwrap();
        std::env::remove_var("LUUNA_COMPONENT_TEST_SECRET");
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(result.status.success());
        assert!(value["secret"].is_null());
        assert_eq!(
            PathBuf::from(value["home"].as_str().unwrap()),
            workspace.join(".luuna-component-home")
        );
        assert_eq!(
            value["git"],
            if cfg!(windows) { "NUL" } else { "/dev/null" }
        );
    }

    #[tokio::test]
    async fn oversized_output_is_rejected_and_the_child_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = prepare_workspace(&dir.path().canonicalize().unwrap()).unwrap();
        let result = output(
            &node(),
            &[
                "-e".into(),
                "process.stdout.write('x'.repeat(5*1024*1024));setInterval(()=>{},1000)".into(),
            ],
            &workspace,
            &workspace,
            None,
        )
        .await;
        assert!(matches!(result, Err(ComponentError::OutputLimit)));
    }

    #[tokio::test]
    async fn git_children_cannot_discover_a_parent_repository_or_use_local_hooks() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let workspace = prepare_workspace(&parent.join("managed")).unwrap();
        let initialized = output(
            Path::new("git"),
            &["init".into(), "--quiet".into()],
            &parent,
            &workspace,
            None,
        )
        .await
        .unwrap();
        assert!(initialized.status.success());
        let discovered = output(
            Path::new("git"),
            &["rev-parse".into(), "--show-toplevel".into()],
            &workspace,
            &workspace,
            None,
        )
        .await
        .unwrap();
        assert!(!discovered.status.success());
        std::fs::write(
            parent.join(".git/config"),
            "[core]\nfsmonitor = synthetic-untrusted-command\nhooksPath = synthetic-untrusted-hooks\n",
        )
        .unwrap();
        for (key, expected) in [
            ("core.fsmonitor", "false"),
            (
                "core.hooksPath",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            ),
        ] {
            let config = output(
                Path::new("git"),
                &["config".into(), "--get".into(), key.into()],
                &parent,
                &workspace,
                None,
            )
            .await
            .unwrap();
            assert!(config.status.success());
            assert_eq!(String::from_utf8(config.stdout).unwrap().trim(), expected);
        }
    }

    #[tokio::test]
    async fn source_identity_probes_never_execute_configured_content_filters() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let workspace = prepare_workspace(&parent.join("managed")).unwrap();
        git(&parent, &["init", "--quiet"], &workspace)
            .await
            .unwrap();
        std::fs::write(parent.join("tracked.txt"), "before\n").unwrap();
        git(&parent, &["add", "tracked.txt"], &workspace)
            .await
            .unwrap();
        git(
            &parent,
            &[
                "config",
                "filter.synthetic.clean",
                "node -e \"require('fs').writeFileSync('filter-ran','synthetic')\"",
            ],
            &workspace,
        )
        .await
        .unwrap();
        git(
            &parent,
            &["config", "filter.synthetic.required", "true"],
            &workspace,
        )
        .await
        .unwrap();
        std::fs::write(
            parent.join(".git/info/attributes"),
            "tracked.txt filter=synthetic\n",
        )
        .unwrap();
        std::fs::write(parent.join("tracked.txt"), "after has different length\n").unwrap();
        let status = git(&parent, &["status", "--porcelain"], &workspace)
            .await
            .unwrap();
        assert!(status.contains("tracked.txt"));
        assert!(!parent.join("filter-ran").exists());
    }

    #[tokio::test]
    async fn deadline_stops_the_owned_descendant_tree() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = prepare_workspace(&dir.path().canonicalize().unwrap()).unwrap();
        let file = workspace.join("descendant.pid");
        let code = format!("const c=require('child_process').spawn(process.execPath,['-e','setInterval(()=>{{}},1000)'],{{stdio:'ignore'}});require('fs').writeFileSync({},String(c.pid));setInterval(()=>{{}},1000)", serde_json::to_string(&file).unwrap());
        let result = output_with_deadline(
            &node(),
            &["-e".into(), code.into()],
            &workspace,
            &workspace,
            None,
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(result, Err(ComponentError::Timeout)));
        let pid = std::fs::read_to_string(file)
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let probe = format!("let alive=false;try{{process.kill({pid},0);alive=true}}catch{{}};if(process.platform==='linux'&&alive){{try{{alive=require('fs').readFileSync('/proc/{pid}/stat','utf8').split(') ')[1][0]!=='Z'}}catch{{alive=false}}}};process.stdout.write(JSON.stringify(alive))");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let result = output(
            &node(),
            &["-e".into(), probe.into()],
            &workspace,
            &workspace,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.stdout, b"false");
    }

    /// A synthetic portable layout: three pinned trees with installed
    /// dependencies, a Bun stand-in answering the pinned version and the
    /// distribution manifest the stager would write for them.
    struct PortableFixture {
        root: PathBuf,
        bun: PathBuf,
    }
    impl PortableFixture {
        fn write(dir: &Path) -> Self {
            let root = dir.canonicalize().unwrap();
            let pins = [
                (
                    "missions",
                    "0.6.0",
                    "src/index.ts",
                    "lm",
                    Vec::<&str>::new(),
                ),
                ("atlas", "0.11.0", "src/cli.ts", "la", Vec::new()),
                ("beacon", "0.2.6", "src/index.ts", "lb", Vec::new()),
            ];
            let mut components = Vec::new();
            let mut staged = Vec::new();
            for (index, (name, version, entrypoint, short, legacy)) in pins.iter().enumerate() {
                let commit = format!("{index}").repeat(40);
                let directory = root.join("components").join(name);
                std::fs::create_dir_all(directory.join("src")).unwrap();
                std::fs::create_dir_all(directory.join("node_modules/dep")).unwrap();
                let mut bin = serde_json::Map::new();
                for alias in [format!("luuna-{name}"), (*short).to_owned()]
                    .into_iter()
                    .chain(legacy.iter().map(|alias| (*alias).to_owned()))
                {
                    bin.insert(alias, Value::String((*entrypoint).to_owned()));
                }
                std::fs::write(
                    directory.join("package.json"),
                    serde_json::to_vec(&serde_json::json!({
                        "name": format!("@luuna/{name}-cli"), "version": version, "bin": bin
                    }))
                    .unwrap(),
                )
                .unwrap();
                std::fs::write(
                    directory.join(entrypoint),
                    format!("console.log(JSON.stringify({{success:true,component:{name:?}}}));\n"),
                )
                .unwrap();
                std::fs::write(
                    directory.join("node_modules/dep/index.js"),
                    "module.exports = 1;\n",
                )
                .unwrap();
                components.push(serde_json::json!({
                    "brand": format!("Luuna {name}"), "path": format!("components/{name}"),
                    "sourceVersion": version, "package": format!("@luuna/{name}-cli"),
                    "cli": {"primary": format!("luuna-{name}"), "short": short, "legacy": legacy, "entrypoint": entrypoint},
                    "license": "MIT", "published": false,
                    "repository": format!("https://github.com/luuna-ai/luuna-{name}.git"),
                    "commit": commit,
                }));
                staged.push(serde_json::json!({
                    "name": name, "path": format!("components/{name}"), "commit": commit,
                    "digest": tree_digest(&directory).unwrap(),
                }));
            }
            let manifest = serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 2, "components": components, "runtime": {"name": "bun", "version": "1.4.3"}
            }))
            .unwrap();
            std::fs::write(root.join("components/manifest.json"), &manifest).unwrap();
            std::fs::create_dir_all(root.join("bun")).unwrap();
            let (file, script) = if cfg!(windows) {
                ("bun.exe", "")
            } else {
                ("bun", "#!/bin/sh\necho 1.4.3\n")
            };
            let bun = root.join("bun").join(file);
            std::fs::write(&bun, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&bun, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let dist = serde_json::json!({
                "schemaVersion": 1, "kind": "luuna-portable-components",
                "generatedAt": "2026-10-22T00:00:00.000Z",
                "source": {"luunaCommit": null, "manifestSha256": sha256_hex(&manifest)},
                "runtime": {"name": "bun", "version": "1.4.3", "executable": format!("bun/{file}"), "sha256": sha256_file(&bun).unwrap()},
                "components": staged,
            });
            std::fs::write(
                root.join(DIST_MANIFEST),
                serde_json::to_vec_pretty(&dist).unwrap(),
            )
            .unwrap();
            Self { root, bun }
        }
        fn components(&self) -> Result<Components> {
            Components::from_root(self.root.clone(), self.bun.clone())
        }
        fn rewrite_dist(&self, edit: impl FnOnce(&mut Value)) {
            let path = self.root.join(DIST_MANIFEST);
            let mut dist: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            edit(&mut dist);
            std::fs::write(&path, serde_json::to_vec(&dist).unwrap()).unwrap();
        }
    }

    fn component_named(error: &ComponentError) -> Option<&str> {
        match error {
            ComponentError::Distribution { component, .. } => Some(component),
            _ => None,
        }
    }

    #[test]
    fn tree_digests_agree_with_the_staging_script() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = PortableFixture::write(dir.path());
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/stage-components.mjs");
        for name in ["missions", "atlas", "beacon"] {
            let directory = fixture.root.join("components").join(name);
            let printed = std::process::Command::new(node())
                .arg(&script)
                .arg("digest")
                .arg(&directory)
                .output()
                .unwrap();
            assert!(printed.status.success());
            assert_eq!(
                String::from_utf8(printed.stdout).unwrap().trim(),
                tree_digest(&directory).unwrap(),
                "{name}"
            );
        }
        // Order-independent, byte-sensitive, and empty directories do not count.
        let directory = fixture.root.join("components/atlas");
        let before = tree_digest(&directory).unwrap();
        std::fs::create_dir(directory.join("empty")).unwrap();
        assert_eq!(tree_digest(&directory).unwrap(), before);
        std::fs::write(directory.join("src/cli.ts"), "changed\n").unwrap();
        assert_ne!(tree_digest(&directory).unwrap(), before);
    }

    #[tokio::test]
    async fn portable_layout_is_verified_against_its_distribution_manifest() {
        if cfg!(windows) {
            eprintln!("the Bun stand-in is a POSIX shell script");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fixture = PortableFixture::write(dir.path());
        let components = fixture.components().unwrap();
        assert_eq!(components.layout(), LayoutKind::Portable);
        assert!(components.describe().contains("portable layout"));
        assert_eq!(components.bun(), fixture.bun);
        let workspace_dir = tempfile::tempdir().unwrap();
        let workspace = prepare_workspace(&workspace_dir.path().canonicalize().unwrap()).unwrap();
        let atlas = components.pins["atlas"].clone();
        components.validate(&atlas, &workspace).await.unwrap();

        // A file changed after discovery is refused at the next call, by name.
        let edited = fixture.root.join("components/atlas/src/cli.ts");
        let original = std::fs::read(&edited).unwrap();
        std::fs::write(&edited, "console.log('tampered');\n").unwrap();
        let refused = components.validate(&atlas, &workspace).await.unwrap_err();
        assert_eq!(component_named(&refused), Some("atlas"));
        assert!(refused.to_string().contains("digest"));
        components
            .validate(&components.pins["beacon"].clone(), &workspace)
            .await
            .unwrap();
        // And at discovery.
        let refused = fixture.components().unwrap_err();
        assert_eq!(component_named(&refused), Some("atlas"));
        std::fs::write(&edited, original).unwrap();
        fixture.components().unwrap();

        // A missing component tree and missing dependencies are refused by name.
        std::fs::rename(
            fixture.root.join("components/beacon"),
            fixture.root.join("components/beacon-moved"),
        )
        .unwrap();
        let refused = fixture.components().unwrap_err();
        assert_eq!(component_named(&refused), Some("beacon"));
        assert!(refused.to_string().contains("missing"));
        std::fs::rename(
            fixture.root.join("components/beacon-moved"),
            fixture.root.join("components/beacon"),
        )
        .unwrap();
        fixture.components().unwrap();

        // A pin that differs between the manifests, and a replaced Bun, are refused.
        fixture
            .rewrite_dist(|dist| dist["components"][0]["commit"] = Value::String("f".repeat(40)));
        let refused = fixture.components().unwrap_err();
        assert_eq!(component_named(&refused), Some("missions"));
        assert!(refused.to_string().contains("commit"));
        fixture
            .rewrite_dist(|dist| dist["components"][0]["commit"] = Value::String("0".repeat(40)));
        fixture.components().unwrap();
        std::fs::write(&fixture.bun, "#!/bin/sh\necho 1.4.3\n# replaced\n").unwrap();
        let refused = fixture.components().unwrap_err();
        assert_eq!(component_named(&refused), Some("bun"));
        let other = fixture.root.join("other-bun");
        std::fs::copy(&fixture.bun, &other).unwrap();
        fixture.rewrite_dist(|dist| {
            dist["runtime"]["sha256"] = Value::String(sha256_file(&fixture.bun).unwrap())
        });
        fixture.components().unwrap();
        let refused = Components::from_root(fixture.root.clone(), other).unwrap_err();
        assert_eq!(component_named(&refused), Some("bun"));
        assert!(refused.to_string().contains("not the staged one"));

        // Without the distribution manifest the same root is a source checkout.
        std::fs::remove_file(fixture.root.join(DIST_MANIFEST)).unwrap();
        let source = fixture.components().unwrap();
        assert_eq!(source.layout(), LayoutKind::SourceCheckout);
        assert!(source.validate(&atlas, &workspace).await.is_err());
    }

    #[test]
    fn links_and_parent_traversal_in_workspaces_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("outside")).unwrap();
        let code = format!(
            "require('fs').symlinkSync({},{},process.platform==='win32'?'junction':'dir')",
            serde_json::to_string(&root.join("outside")).unwrap(),
            serde_json::to_string(&root.join("link")).unwrap()
        );
        assert!(std::process::Command::new(node())
            .args(["-e", &code])
            .status()
            .unwrap()
            .success());
        assert!(matches!(
            prepare_workspace(&root.join("link/data")),
            Err(ComponentError::Workspace)
        ));
        assert!(matches!(
            prepare_workspace(Path::new("../synthetic-component-outside")),
            Err(ComponentError::Workspace)
        ));
    }
}
